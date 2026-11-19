//! The world on the GPU: buffers, textures and bind groups made once at
//! load; per frame one uniform write, the indirect slots' instance counts
//! and one render pass with a `multi_draw_indexed_indirect` per batch,
//! into the game's own colour target and depth.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::camera::{Mat4, RenderCamera};
use crate::cull::{CullStats, Frustum, IndirectSlot, SLOT_SIZE, cull, indirect_slots};
use crate::fog_gpu::FogSet;
use crate::light_gpu::{LightDraw, LightSet};
use crate::light_look::LightStage;
use crate::program::{ProgramSet, ViewInputs};
use crate::transparent_gpu::TransparentSet;
use crate::world::{LightmapKey, Owner, World, WorldDraw, WorldVertex};

/// An RGBA8 image (linear rows, no padding) with its smaller levels.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Levels 1.. of the mip chain, each half the last (at least 1).
    pub mips: Vec<Vec<u8>>,
    /// The same levels as BC blocks, for a DXT image the D3D path uploads
    /// as BC ([`Image::with_blocks`]): uploaded instead of `rgba` where the
    /// device takes BC textures, so the GPU decodes them as it does the
    /// D3D path's (the CPU decoder rounds the interpolated colours down).
    pub blocks: Option<Blocks>,
}

/// An image's levels as BC blocks ([`Image::blocks`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Blocks {
    /// `Bc1RgbaUnorm`, `Bc2RgbaUnorm` or `Bc3RgbaUnorm`.
    pub format: wgpu::TextureFormat,
    /// Level 0 first, rows of 4x4 blocks, tight.
    pub levels: Vec<Vec<u8>>,
}

/// Bytes per 4x4 block of a BC format (`None`: not one).
pub fn block_bytes(format: wgpu::TextureFormat) -> Option<usize> {
    match format {
        wgpu::TextureFormat::Bc1RgbaUnorm => Some(8),
        wgpu::TextureFormat::Bc2RgbaUnorm | wgpu::TextureFormat::Bc3RgbaUnorm => Some(16),
        _ => None,
    }
}

impl Image {
    pub fn solid(rgba: [u8; 4]) -> Self {
        Self {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
            mips: Vec::new(),
            blocks: None,
        }
    }

    /// `self` with its levels as BC blocks too, when they fit it: a BC
    /// format, the base size a multiple of 4 (wgpu's rule, and the D3D
    /// path's for passing blocks through), one level of blocks per level
    /// of `self`, each of its size. Otherwise `self` as it was.
    pub fn with_blocks(mut self, format: wgpu::TextureFormat, levels: Vec<Vec<u8>>) -> Self {
        let Some(unit) = block_bytes(format) else { return self };
        if !self.width.is_multiple_of(4) || !self.height.is_multiple_of(4) || levels.len() != 1 + self.mips.len() {
            return self;
        }
        let fits = levels.iter().enumerate().all(|(n, l)| {
            let (w, h) = ((self.width >> n).max(1), (self.height >> n).max(1));
            l.len() == w.div_ceil(4) as usize * h.div_ceil(4) as usize * unit
        });
        if fits {
            self.blocks = Some(Blocks { format, levels });
        }
        self
    }

    /// One level, no chain.
    pub fn level(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        Self::from_levels(vec![(width, height, rgba)])
    }

    /// An image from its decoded levels, largest first. The chain stops at
    /// the first level that is not half the one before (at least 1) or
    /// whose bytes do not match its size; `None` when level 0 does not.
    pub fn from_levels(levels: Vec<(u32, u32, Vec<u8>)>) -> Option<Self> {
        let mut levels = levels.into_iter();
        let (width, height, rgba) = levels.next()?;
        if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
            return None;
        }
        let mut mips = Vec::new();
        let (mut w, mut h) = (width, height);
        for (lw, lh, bytes) in levels {
            if w == 1 && h == 1 {
                break;
            }
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
            if (lw, lh) != (w, h) || bytes.len() != (w * h * 4) as usize {
                break;
            }
            mips.push(bytes);
        }
        Some(Self { width, height, rgba, mips, blocks: None })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FrameUniform {
    view_proj: Mat4,
    depth: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BatchUniform {
    flags: [f32; 4],
}

/// The depth the pass writes: the format's full scale and whether to floor
/// to its step (the game's W-buffer; [`crate::camera::w_depth`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthRule {
    pub full_scale: f32,
    pub floor: bool,
}

impl DepthRule {
    pub const D24: Self = Self {
        full_scale: crate::camera::D24_SCALE,
        floor: true,
    };
}

/// Where the pass draws, in target pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The stencil test and write the layer's draws take on the D3D path, as
/// the device holds them when the native pass is asked for, and the
/// sections the game draws in its mode 2 (`skip::StencilPlan::marked`):
/// the same state with reference [`PassStencil::MARK`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PassStencil {
    pub state: wgpu::StencilState,
    pub reference: u32,
    /// Sorted.
    pub marked: Vec<Owner>,
}

impl PassStencil {
    /// The reference mode 2 writes (`0x00015411`: `0xff` but in mode 1).
    pub const MARK: u32 = 0xff;

    /// Whether `s` is the structure draw's mode 1 (`0x0001539d`: enabled,
    /// `ALWAYS`, reference 0, masks `0xff`, `KEEP` on fail and depth fail,
    /// `REPLACE` on pass): what every item leaves after a switch, so
    /// leaving out a switching item's switches changes nothing for the
    /// items after it, and its mode 2 is this state with [`Self::MARK`].
    pub fn is_structure_mode(s: &gfx::state::StencilState) -> bool {
        use gfx::state::{CompareFunc, StencilOp};
        s.enable
            && s.func == CompareFunc::Always
            && s.reference == 0
            && s.read_mask == 0xff
            && s.write_mask == 0xff
            && s.fail == StencilOp::Keep
            && s.depth_fail == StencilOp::Keep
            && s.pass == StencilOp::Replace
    }

    /// The reference `owner`'s parts draw with.
    pub fn reference_of(&self, owner: Owner) -> u32 {
        if self.marked.binary_search(&owner).is_ok() { Self::MARK } else { self.reference }
    }

    /// The device's stencil state as the D3D path's pipeline takes it
    /// (`gfx::translate::depth_stencil_state`): off without a stencil
    /// aspect in `depth`.
    pub fn from_device(s: &gfx::state::StencilState, depth: wgpu::TextureFormat) -> Self {
        if !s.enable || !depth.has_stencil_aspect() {
            return Self::default();
        }
        use gfx::translate::{compare_function, stencil_operation};
        let face = wgpu::StencilFaceState {
            compare: compare_function(s.func),
            fail_op: stencil_operation(s.fail),
            depth_fail_op: stencil_operation(s.depth_fail),
            pass_op: stencil_operation(s.pass),
        };
        Self {
            state: wgpu::StencilState {
                front: face,
                back: face,
                read_mask: u32::from(s.read_mask),
                write_mask: u32::from(s.write_mask),
            },
            reference: u32::from(s.reference),
            marked: Vec::new(),
        }
    }
}

/// Which sections' parts a pass draws ([`GpuWorld::set_owners`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owners {
    #[default]
    All,
    /// Clusters only: placements left to the D3D path.
    Clusters,
    /// Placements only: clusters left to the D3D path.
    Placements,
}

impl Owners {
    /// Whether `owner`'s parts are drawn.
    pub fn keeps(self, owner: Owner) -> bool {
        match (self, owner) {
            (Owners::All, _) => true,
            (Owners::Clusters, Owner::Cluster(_)) | (Owners::Placements, Owner::Instance(_)) => true,
            _ => false,
        }
    }

    /// `all`, `clusters` or `placements`.
    pub fn parse(v: &str) -> Option<Self> {
        match v.trim() {
            "all" | "" => Some(Owners::All),
            "clusters" => Some(Owners::Clusters),
            "placements" => Some(Owners::Placements),
            _ => None,
        }
    }
}

pub struct GpuWorld {
    world: World,
    /// The stencil the next pass draws with ([`GpuWorld::set_stencil`]).
    stencil: PassStencil,
    /// The cull the next pass's inheriting programs draw with
    /// ([`GpuWorld::set_inherited_cull`]).
    inherited_cull: crate::program::InheritedCull,
    /// Whether [`GpuWorld::prepare`] culls on the CPU ([`GpuWorld::set_cull`]).
    culls: bool,
    /// Which sections' parts it draws ([`GpuWorld::set_owners`]).
    owners: Owners,
    vertices: wgpu::Buffer,
    /// [`World::program_vertices`]: what the material programs read.
    program_vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    indirect: wgpu::Buffer,
    slots: Vec<IndirectSlot>,
    frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    batch_groups: Vec<wgpu::BindGroup>,
    frame_layout: wgpu::BindGroupLayout,
    batch_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
    /// Materials drawn with their own programs ([`crate::program`]); the
    /// rest draw with stage 1's.
    programs: Option<ProgramSet>,
    /// The transparent materials the sorted transparents' runs draw
    /// ([`crate::transparent_gpu`]).
    transparents: Option<TransparentSet>,
    /// The fog materials layer 16 draws ([`crate::fog_gpu`]).
    fog: Option<FogSet>,
    /// The light passes (stage 4), once a light needs them.
    lights: Option<LightSet>,
    /// Batches left out of a pass because their material program could
    /// not be bound (with programs set; see [`GpuWorld::encode`]).
    unbound_batches: u64,
}

const SHADER: &str = include_str!("world.wgsl");

pub(crate) fn texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image, label: &str) -> wgpu::TextureView {
    if let Some(b) = &image.blocks
        && device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return block_texture(device, queue, image, b, label);
    }
    let size = wgpu::Extent3d {
        width: image.width.max(1),
        height: image.height.max(1),
        depth_or_array_layers: 1,
    };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1 + image.mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, bytes) in std::iter::once(&image.rgba).chain(&image.mips).enumerate() {
        let level = level as u32;
        let (w, h) = ((size.width >> level).max(1), (size.height >> level).max(1));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// `image` from its BC blocks (`gfx::upload_texture`'s copies: a level
/// smaller than a block is copied at its physical, rounded-up size).
fn block_texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image, b: &Blocks, label: &str) -> wgpu::TextureView {
    let unit = block_bytes(b.format).expect("a BC format") as u32;
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: image.width, height: image.height, depth_or_array_layers: 1 },
        mip_level_count: b.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: b.format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, bytes) in b.levels.iter().enumerate() {
        let level = level as u32;
        let (w, h) = ((image.width >> level).max(1), (image.height >> level).max(1));
        let (w, h) = (w.next_multiple_of(4), h.next_multiple_of(4));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w / 4 * unit),
                rows_per_image: Some(h / 4),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

impl GpuWorld {
    /// Uploads `world` once. `base_map` gives a material's base map and
    /// `lightmap` a page through its palette; `None` draws white (the
    /// per-material fallback to the D3D path is decided by the caller,
    /// which leaves such materials out of the world).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: World,
        base_map: &mut dyn FnMut(u16) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // An empty buffer cannot be bound: keep one element.
        let mut verts = world.vertices.clone();
        if verts.is_empty() {
            verts.push(WorldVertex::default());
        }
        let mut idx = world.indices.clone();
        if idx.is_empty() {
            idx.push(0);
        }
        let mut slots = indirect_slots(&world);
        let slot_bytes: Vec<IndirectSlot> = if slots.is_empty() {
            vec![IndirectSlot::default()]
        } else {
            slots.clone()
        };
        let vertices = init("native world vertices", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let program_vertices = if world.program_vertices.len() == world.vertices.len() && !world.program_vertices.is_empty() {
            init(
                "native world program vertices",
                bytemuck::cast_slice(&world.program_vertices),
                wgpu::BufferUsages::VERTEX,
            )
        } else {
            vertices.clone()
        };
        let indices = init("native world indices", bytemuck::cast_slice(&idx), wgpu::BufferUsages::INDEX);
        let indirect = init(
            "native world indirect",
            bytemuck::cast_slice(&slot_bytes),
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native world frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world frame"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let batch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world batch"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native world"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white = texture(device, queue, &Image::solid([255; 4]), "native world white");
        let mut bases: HashMap<u16, wgpu::TextureView> = HashMap::new();
        let mut pages: HashMap<LightmapKey, wgpu::TextureView> = HashMap::new();
        let mut batch_groups = Vec::with_capacity(world.batches.len());
        for b in &world.batches {
            let base = bases
                .entry(b.material)
                .or_insert_with(|| {
                    base_map(b.material).map_or_else(|| white.clone(), |i| texture(device, queue, &i, "native world base"))
                })
                .clone();
            let lit = b.lightmap.and_then(|k| {
                if let Some(v) = pages.get(&k) {
                    return Some(v.clone());
                }
                let v = texture(device, queue, &lightmap(k)?, "native world lightmap");
                pages.insert(k, v.clone());
                Some(v)
            });
            let uniform = init(
                "native world batch",
                bytemuck::bytes_of(&BatchUniform {
                    flags: [f32::from(u8::from(lit.is_some())), 0.0, 0.0, 0.0],
                }),
                wgpu::BufferUsages::UNIFORM,
            );
            let page = lit.unwrap_or_else(|| white.clone());
            batch_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native world batch"),
                layout: &batch_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&page),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }));
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        slots.shrink_to_fit();
        Self {
            world,
            stencil: PassStencil::default(),
            inherited_cull: crate::program::InheritedCull::default(),
            culls: true,
            owners: Owners::All,
            vertices,
            program_vertices,
            indices,
            indirect,
            slots,
            frame,
            frame_group,
            batch_groups,
            frame_layout,
            batch_layout,
            shader,
            pipelines: HashMap::new(),
            programs: None,
            transparents: None,
            fog: None,
            lights: None,
            unbound_batches: 0,
        }
    }

    /// Batch draws left out since upload because their material program
    /// could not be bound (zero when the loader's checks hold).
    pub fn unbound_batches(&self) -> u64 {
        self.unbound_batches
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Draws the world's transparent parts of `set`'s materials from now on.
    pub fn set_transparents(&mut self, set: TransparentSet) {
        self.transparents = Some(set);
    }

    /// The transparent materials, when they are uploaded.
    pub fn transparents(&self) -> Option<&TransparentSet> {
        self.transparents.as_ref()
    }

    /// Draws the world's layer-16 passes of `set`'s materials from now on.
    pub fn set_fog(&mut self, set: FogSet) {
        self.fog = Some(set);
    }

    /// The fog materials, when they are uploaded.
    pub fn fog(&self) -> Option<&FogSet> {
        self.fog.as_ref()
    }

    /// Records one run of sorted transparent entries, each a part
    /// `(owner, part index)`, in order: every pass of each, blended into
    /// the target over the depth (loaded and kept). Entries whose part or
    /// material the set does not draw are skipped (the host accepts none
    /// such). Returns how many were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_transparents(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        entries: &[(Owner, u8)],
    ) -> usize {
        let Some(set) = self.transparents.as_mut() else { return 0 };
        set.pipelines(device, color_format, depth_format);
        set.prepare(queue, view);
        let set = &*set;
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native transparents"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: has_stencil.then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The programs carry the game's viewport in their constants: the
        // pass covers the whole target, as the world's programs do.
        pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        let mut drawn = 0;
        for &(owner, part) in entries {
            if let Some(t) = self.world.transparent(owner, part)
                && set.draw(&mut pass, t, color_format, depth_format)
            {
                drawn += 1;
            }
        }
        drawn
    }

    /// Records one view's planar fog (layer 16): every part the fog set
    /// draws, in the load's order (the world's draw order), each with
    /// every pass of its material's layer-16 set, blended into the target
    /// over the depth (loaded and kept). The fog units and colours are
    /// `fogs`'s (per material, resolved over the device's words at the
    /// ask). Returns how many parts were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_fog(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) -> usize {
        let Some(set) = self.fog.as_mut() else { return 0 };
        set.pipelines(device, color_format, depth_format);
        set.prepare(queue, view, fogs);
        let set = &*set;
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native fog"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: has_stencil.then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The programs carry the game's viewport in their constants: the
        // pass covers the whole target, as the world's programs do.
        pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        set.draw(&mut pass, color_format, depth_format)
    }

    /// Draws lights with `set` from now on.
    pub fn set_lights(&mut self, set: LightSet) {
        self.lights = Some(set);
    }

    /// The light passes, when the engine's light textures are uploaded.
    pub fn lights(&self) -> Option<&LightSet> {
        self.lights.as_ref()
    }

    /// Records one run of lights' draws over the world's parts (each
    /// draw's index ranges, which `boot` picks from the parts whose bounds
    /// meet the light's sphere), added into the target over the depth
    /// (loaded and kept), in order. Nothing without [`Self::set_lights`].
    /// Returns how many ranges were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_lights(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        view: &ViewInputs,
        draws: &[LightDraw<'_>],
        stage_image: &mut dyn FnMut(LightStage) -> Option<Image>,
        gel_image: &mut dyn FnMut(u32) -> Option<Image>,
    ) -> usize {
        let Some(set) = self.lights.as_mut() else { return 0 };
        set.encode(
            device,
            queue,
            encoder,
            &self.vertices,
            &self.indices,
            color,
            color_format,
            depth,
            depth_format,
            target_size,
            view,
            draws,
            stage_image,
            gel_image,
        )
    }

    /// The stencil the material programs draw with from the next pass
    /// ([`PassStencil::from_device`]).
    pub fn set_stencil(&mut self, stencil: PassStencil) {
        self.stencil = stencil;
    }

    /// The cull the material programs whose pass does not set `CULLMODE`
    /// draw with from the next pass (and [`GpuWorld::rigid_uniforms`]
    /// reports; [`crate::program::InheritedCull`]).
    pub fn set_inherited_cull(&mut self, cull: crate::program::InheritedCull) {
        self.inherited_cull = cull;
        if let Some(p) = &mut self.programs {
            p.set_cull(cull);
        }
    }

    /// Whether [`GpuWorld::prepare`] culls by the frustum (on by default).
    /// Off, every draw is drawn and the programs' own clipping alone
    /// decides: a diagnostic, telling a part culled on the CPU from one
    /// clipped or failing depth on the GPU.
    pub fn set_cull(&mut self, on: bool) {
        self.culls = on;
    }

    /// Which sections' parts [`GpuWorld::prepare`] leaves drawn (all by
    /// default): a diagnostic, with the host leaving the rest to the D3D
    /// path ([`Owners::keeps`] on the parts it skips).
    pub fn set_owners(&mut self, owners: Owners) {
        self.owners = owners;
    }

    /// Draws `programs`' materials with their own programs from now on.
    pub fn set_programs(&mut self, mut programs: ProgramSet) {
        programs.set_cull(self.inherited_cull);
        self.programs = Some(programs);
    }

    /// Whether material programs are set: the world then draws only with
    /// them, never with stage 1's program.
    pub fn has_programs(&self) -> bool {
        self.programs.is_some()
    }

    /// How many materials draw with their own programs.
    pub fn program_count(&self) -> usize {
        self.programs.as_ref().map_or(0, ProgramSet::len)
    }

    /// Placement `instance`'s rigid draws in a frame of `view`
    /// ([`ProgramSet::rigid_uniforms`]); empty without programs.
    pub fn rigid_uniforms(&self, instance: u32, view: &ViewInputs) -> Vec<crate::program::RigidUniforms> {
        self.programs.as_ref().map_or_else(Vec::new, |p| p.rigid_uniforms(instance, view))
    }

    /// The frame's view for the programs' uniforms (the device's view
    /// constants, the target's sizes and depth scale).
    pub fn prepare_programs(&self, queue: &wgpu::Queue, view: &ViewInputs) {
        if let Some(p) = &self.programs {
            p.prepare(queue, view);
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (shader, fl, bl) = (&self.shader, &self.frame_layout, &self.batch_layout);
        self.pipelines.entry((color, depth)).or_insert_with(|| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native world"),
                bind_group_layouts: &[Some(fl), Some(bl)],
                immediate_size: 0,
            });
            let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x3];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native world"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<WorldVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // S: the BSP's winding is not checked yet; draw both.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// The frame's camera: culls on the CPU and writes the uniform and the
    /// slots' instance counts.
    pub fn prepare(&mut self, queue: &wgpu::Queue, camera: &RenderCamera, depth: DepthRule) -> CullStats {
        let view_proj = camera.view_projection();
        // The material programs clip as the D3D path does: cull by the
        // sides and the eye only (`Frustum::sides`). Stage 1 draws with
        // the render camera's own range.
        let frustum = if self.programs.is_some() {
            Frustum::sides(&view_proj)
        } else {
            Frustum::from_view_projection(&view_proj)
        };
        let stats = if self.culls {
            cull(&self.world, &frustum, &mut self.slots)
        } else {
            for slot in &mut self.slots {
                slot.instance_count = 1;
            }
            CullStats { visible: self.slots.len() as u32, total: self.world.draws.len() as u32 }
        };
        let stats = if self.owners == Owners::All {
            stats
        } else {
            let mut visible = 0;
            for (slot, d) in self.slots.iter_mut().zip(&self.world.draws) {
                if !self.owners.keeps(d.owner) {
                    slot.instance_count = 0;
                }
                visible += slot.instance_count.min(1);
            }
            CullStats { visible, total: stats.total }
        };
        let u = FrameUniform {
            view_proj,
            depth: [depth.full_scale, 1.0 / camera.far, f32::from(u8::from(depth.floor)), 0.0],
        };
        queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&u));
        if !self.slots.is_empty() {
            queue.write_buffer(&self.indirect, 0, bytemuck::cast_slice(&self.slots));
        }
        stats
    }

    /// Records the pass: loads and keeps the target and depth, draws every
    /// batch with its slot range. `target_size` is the colour target's in
    /// texels: a batch drawn with its own program covers it all (the
    /// game's viewport is in its vertex program's constants).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        viewport: PassViewport,
        target_size: (u32, u32),
    ) {
        self.pipeline(device, color_format, depth_format);
        if let Some(p) = &mut self.programs {
            p.pipelines(device, color_format, depth_format, &self.stencil.state, self.inherited_cull);
        }
        let pipeline = &self.pipelines[&(color_format, depth_format)];
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: has_stencil.then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if self.slots.is_empty() {
            return;
        }
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        // Which pipeline is bound: None before the first batch, Some(false)
        // stage 1's, Some(true) a material program's (its groups are set
        // per run).
        let mut own: Option<bool> = None;
        let mut unbound = 0u64;
        for (i, (b, group)) in self.world.batches.iter().zip(&self.batch_groups).enumerate() {
            let programs = self.programs.as_ref();
            let program = programs.and_then(|p| p.batch(i, b.material, color_format, depth_format, &self.stencil.state));
            if let (Some(p), Some((program, uniforms, textures))) = (programs, program) {
                if own != Some(true) {
                    pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
                    pass.set_vertex_buffer(0, self.program_vertices.slice(..));
                }
                own = Some(true);
                // A batch's draws sort by owner: its clusters first, then
                // each placement's parts together. The clusters draw with
                // the world program; a placement's with the rigid one, its
                // rows at its slot's offset, as the structure draw uploads
                // them before each of its draws (`0x0004d830`). A section
                // the game draws in stencil mode 2 is a run of its own, with
                // that reference.
                let first = b.first_draw as usize;
                let draws = &self.world.draws[first..first + b.draw_count as usize];
                let mut k = 0;
                while k < draws.len() {
                    let owner = draws[k].owner;
                    let reference = self.stencil.reference_of(owner);
                    let same = |d: &WorldDraw| match owner {
                        Owner::Cluster(_) => {
                            matches!(d.owner, Owner::Cluster(_)) && self.stencil.reference_of(d.owner) == reference
                        }
                        Owner::Instance(_) => d.owner == owner,
                    };
                    let end = k + draws[k..].iter().take_while(|d| same(d)).count();
                    match owner {
                        Owner::Cluster(_) => {
                            pass.set_pipeline(program);
                            pass.set_bind_group(0, uniforms, &[]);
                        }
                        Owner::Instance(n) => {
                            let Some((rigid, slots, offset)) = p.rigid(b.material, n, color_format, depth_format, &self.stencil.state) else {
                                // The loader keeps a placement's parts only
                                // with a rigid program and its rows.
                                unbound += 1;
                                k = end;
                                continue;
                            };
                            pass.set_pipeline(rigid);
                            pass.set_bind_group(0, slots, &[offset]);
                        }
                    }
                    pass.set_bind_group(1, textures, &[]);
                    if has_stencil {
                        pass.set_stencil_reference(reference);
                    }
                    pass.multi_draw_indexed_indirect(&self.indirect, (first + k) as u64 * SLOT_SIZE, (end - k) as u32);
                    k = end;
                }
                continue;
            } else if self.programs.is_some() {
                // With material programs, stage 1's look is never drawn:
                // the loader keeps only materials whose program uploads,
                // so this is a batch it could not bind (counted, left out).
                unbound += 1;
                continue;
            } else {
                if own != Some(false) {
                    pass.set_viewport(viewport.x, viewport.y, viewport.width, viewport.height, 0.0, 1.0);
                    pass.set_vertex_buffer(0, self.vertices.slice(..));
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.frame_group, &[]);
                }
                own = Some(false);
                pass.set_bind_group(1, group, &[]);
            }
            pass.multi_draw_indexed_indirect(&self.indirect, u64::from(b.first_draw) * SLOT_SIZE, b.draw_count);
        }
        drop(pass);
        self.unbound_batches += unbound;
    }
}

#[cfg(test)]
mod tests {
    use super::{Blocks, Image, PassStencil, block_bytes};

    #[test]
    fn the_pass_takes_the_devices_stencil_as_the_d3d_path_does() {
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        // The structure draw's mode 1: ALWAYS, reference 0, REPLACE.
        let mode1 = StencilState {
            enable: true,
            func: CompareFunc::Always,
            reference: 0,
            read_mask: 0xff,
            write_mask: 0xff,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Replace,
        };
        let s = PassStencil::from_device(&mode1, wgpu::TextureFormat::Depth24PlusStencil8);
        assert!(s.marked.is_empty());
        assert_eq!(s.state.front.compare, wgpu::CompareFunction::Always);
        assert_eq!(s.state.front.pass_op, wgpu::StencilOperation::Replace);
        assert_eq!(s.state.back, s.state.front);
        assert_eq!((s.state.write_mask, s.reference), (0xff, 0));
        let other = gfx::translate::depth_stencil_state(
            &gfx::state::RenderState { stencil: mode1, ..Default::default() },
            Some(wgpu::TextureFormat::Depth24PlusStencil8),
            wgpu::PrimitiveTopology::TriangleList,
        )
        .unwrap();
        assert_eq!(s.state, other.stencil, "what the D3D path's pipeline takes");
        assert_eq!(PassStencil::from_device(&mode1, wgpu::TextureFormat::Depth32Float), PassStencil::default());
        let off = StencilState { enable: false, ..mode1 };
        assert_eq!(PassStencil::from_device(&off, wgpu::TextureFormat::Depth24PlusStencil8), PassStencil::default());
        // Mode 1 is what a switching item leaves; anything else is not.
        assert!(PassStencil::is_structure_mode(&mode1));
        assert!(!PassStencil::is_structure_mode(&off));
        assert!(!PassStencil::is_structure_mode(&StencilState { reference: 0xff, ..mode1 }));
        assert!(!PassStencil::is_structure_mode(&StencilState { write_mask: 0x0f, ..mode1 }));
        assert!(!PassStencil::is_structure_mode(&StencilState { depth_fail: StencilOp::Replace, ..mode1 }));
        // A marked section draws with mode 2's reference, the rest with
        // the device's.
        use crate::world::Owner;
        let s = PassStencil { marked: vec![Owner::Cluster(3), Owner::Instance(1)], ..s };
        assert_eq!(s.reference_of(Owner::Cluster(3)), 0xff);
        assert_eq!(s.reference_of(Owner::Instance(1)), 0xff);
        assert_eq!(s.reference_of(Owner::Cluster(1)), 0);
    }

    fn level(w: u32, h: u32) -> (u32, u32, Vec<u8>) {
        (w, h, vec![0; (w * h * 4) as usize])
    }

    #[test]
    fn a_mip_chain_keeps_the_levels_that_halve() {
        let i = Image::from_levels(vec![level(4, 2), level(2, 1), level(1, 1)]).unwrap();
        assert_eq!(i.mips.len(), 2);
        let i = Image::from_levels(vec![level(4, 2), level(2, 2), level(1, 1)]).unwrap();
        assert!(i.mips.is_empty(), "stops at the first level that does not halve");
        let i = Image::from_levels(vec![level(1, 1), level(1, 1)]).unwrap();
        assert!(i.mips.is_empty(), "nothing below 1x1");
        assert_eq!(Image::from_levels(vec![(2, 2, vec![0; 3])]), None);
        assert_eq!(Image::level(1, 1, vec![9; 4]).unwrap().rgba, vec![9; 4]);
    }

    /// A DXT image keeps its blocks only where the D3D path would pass them
    /// through: a BC format, a base size in whole blocks, and one level
    /// of blocks of the right size per level (a level under 4x4 takes a
    /// whole block).
    #[test]
    fn an_image_keeps_its_blocks_only_where_they_fit() {
        use wgpu::TextureFormat as F;
        let chain = || Image::from_levels(vec![level(8, 4), level(4, 2), level(2, 1), level(1, 1)]).unwrap();
        // BC1: 8 bytes a block; 8x4 is two blocks, every smaller level one.
        let blocks = vec![vec![1; 16], vec![2; 8], vec![3; 8], vec![4; 8]];
        let i = chain().with_blocks(F::Bc1RgbaUnorm, blocks.clone());
        assert_eq!(i.blocks, Some(Blocks { format: F::Bc1RgbaUnorm, levels: blocks.clone() }));
        assert_eq!(i.rgba, chain().rgba, "the decoded texels stay");
        assert_eq!(chain().with_blocks(F::Bc3RgbaUnorm, blocks.clone()).blocks, None, "BC3 blocks are 16 bytes");
        assert_eq!(chain().with_blocks(F::Rgba8Unorm, blocks.clone()).blocks, None, "not a BC format");
        assert_eq!(chain().with_blocks(F::Bc1RgbaUnorm, blocks[..3].to_vec()).blocks, None, "a level short");
        let small = Image::from_levels(vec![level(2, 8)]).unwrap();
        assert_eq!(small.with_blocks(F::Bc1RgbaUnorm, vec![vec![0; 16]]).blocks, None, "2x8: not whole blocks");
        assert_eq!(block_bytes(F::Bc2RgbaUnorm), Some(16));
    }
}
