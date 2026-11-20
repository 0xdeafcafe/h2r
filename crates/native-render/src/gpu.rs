//! World geometry and materials live on the GPU for the BSP. Guest-selected
//! parts draw their index ranges in list order. Whole-world rendering uses
//! frustum-culled indirect slots.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};

use crate::camera::{Mat4, RenderCamera};
use crate::cull::{CullStats, Frustum, IndirectSlot, SLOT_SIZE, cull, indirect_slots};
use crate::fog_gpu::FogSet;
use crate::light_gpu::{LightDraw, LightSet};
use crate::light_look::LightStage;
use crate::program::{MaterialUse, ProgramSet, ViewInputs};
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

/// A single guest list in draw order. Only uniform destinations are deduplicated.
#[derive(Default)]
pub struct WorldSelection {
    spans: Vec<(usize, usize, usize)>,
    pub(crate) materials: HashMap<u16, MaterialUse>,
    pub(crate) vertices: HashSet<(u16, Option<u32>)>,
}

impl WorldSelection {
    fn push(&mut self, slot: usize, draw: &WorldDraw) {
        let batch = draw.batch as usize;
        if let Some((last, _, end)) = self.spans.last_mut() && *last == batch && *end == slot {
            *end += 1;
        } else {
            self.spans.push((batch, slot, slot + 1));
        }
        let instance = match draw.owner {
            Owner::Cluster(_) => None,
            Owner::Instance(n) => Some(n),
        };
        let usage = self.materials.entry(draw.material).or_default();
        match instance {
            Some(_) => usage.rigid = true,
            None => usage.cluster = true,
        }
        self.vertices.insert((draw.material, instance));
    }
}

pub struct GpuWorld {
    part_slots: std::collections::HashMap<(Owner, u8), usize>,
    world: World,
    /// The stencil the next pass draws with ([`GpuWorld::set_stencil`]).
    stencil: PassStencil,
    /// The cull the next pass's inheriting programs draw with
    /// ([`GpuWorld::set_inherited_cull`]).
    inherited_cull: crate::program::InheritedCull,
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
    /// The fog materials layer 16 draws ([`crate::fog_gpu`]).
    fog: Option<FogSet>,
    /// The water materials layer 6 draws ([`crate::water_gpu`]).
    water: Option<crate::water_gpu::WaterSet>,
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
        gfx::churn::upload_bytes(bytes.len());
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
        gfx::churn::upload_bytes(bytes.len());
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
        Self::upload(device, queue, world, base_map, lightmap, None)
    }

    /// Program worlds never bind the fixed-function batch groups.
    pub fn with_programs(device: &wgpu::Device, queue: &wgpu::Queue, world: World, programs: ProgramSet) -> Self {
        Self::upload(device, queue, world, &mut |_| None, &mut |_| None, Some(programs))
    }

    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: World,
        base_map: &mut dyn FnMut(u16) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
        programs: Option<ProgramSet>,
    ) -> Self {
        let init = |label, contents: &[u8], usage| {
            gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // An empty buffer cannot be bound: keep one element.
        let empty_vertex = [WorldVertex::default()];
        let verts = if world.vertices.is_empty() { &empty_vertex[..] } else { &world.vertices[..] };
        let idx = if world.indices.is_empty() { &[0][..] } else { &world.indices[..] };
        let slots = indirect_slots(&world);
        let empty_slot = [IndirectSlot::default()];
        let slot_bytes = if slots.is_empty() { &empty_slot[..] } else { &slots[..] };
        let vertices = init("native world vertices", bytemuck::cast_slice(verts), wgpu::BufferUsages::VERTEX);
        let program_vertices = if world.program_vertices.len() == world.vertices.len() && !world.program_vertices.is_empty() {
            init(
                "native world program vertices",
                bytemuck::cast_slice(&world.program_vertices),
                wgpu::BufferUsages::VERTEX,
            )
        } else {
            vertices.clone()
        };
        let indices = init("native world indices", bytemuck::cast_slice(idx), wgpu::BufferUsages::INDEX);
        let indirect = init(
            "native world indirect",
            bytemuck::cast_slice(slot_bytes),
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let frame = gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
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
        let frame_group = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
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
        let fixed_batches = if programs.is_none() { &world.batches[..] } else { &[] };
        let mut batch_groups = Vec::with_capacity(fixed_batches.len());
        for b in fixed_batches {
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
            batch_groups.push(gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
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
        let part_slots = world.draws.iter().enumerate().map(|(i, d)| ((d.owner, d.part), i)).collect();
        Self {
            part_slots, world,
            stencil: PassStencil::default(),
            inherited_cull: crate::program::InheritedCull::default(),
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
            programs,
            fog: None,
            water: None,
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

    /// Draws the world's layer-16 passes of `set`'s materials from now on.
    pub fn set_fog(&mut self, set: FogSet) {
        self.fog = Some(set);
    }

    /// The fog materials, when they are uploaded.
    pub fn fog(&self) -> Option<&FogSet> {
        self.fog.as_ref()
    }

    /// Draws the water edge blend of `set`'s materials from now on.
    pub fn set_water(&mut self, set: crate::water_gpu::WaterSet) {
        self.water = Some(set);
    }

    /// The water materials, when they are uploaded.
    pub fn water(&self) -> Option<&crate::water_gpu::WaterSet> {
        self.water.as_ref()
    }

    /// The water uniforms for one view ([`WaterSet::prepare`]), through
    /// the submission's upload arena.
    pub fn prepare_water(
        &mut self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) {
        if let Some(set) = self.water.as_ref() {
            set.prepare(uploads, view, fogs);
        }
    }

    /// Records one view's water edge blend (layer 6) in reservation
    /// order, each selected part with every pass
    /// of its material's layer-6 set, blended into the target over the
    /// depth (loaded and kept). The view copy for the stages that sample
    /// the scene is made first ([`WaterSet::make_copy`]); `sampled` are
    /// the render targets' views in the set's `targets` order. Returns
    /// how many parts were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_water(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        target_copy: d3d::TargetCopy,
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
    ) -> usize {
        let Some(set) = self.water.as_mut() else { return 0 };
        set.pipelines(device, color_format, depth_format);
        let copy = set
            .samples_view
            .then(|| set.make_copy(device, encoder, color, target_copy));
        let set = &*set;
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native water"),
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
        pass.set_vertex_buffer(0, self.program_vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        set.draw(device, &mut pass, color_format, depth_format, copy.as_ref(), selected, sampled)
    }

    /// The fog uniforms for one view ([`FogSet::prepare`]), through the
    /// submission's upload arena (the reservation records into an
    /// isolated encoder, where a queue write would escape cancellation).
    pub fn prepare_fog(
        &mut self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) {
        if let Some(set) = self.fog.as_ref() {
            set.prepare(uploads, view, fogs);
        }
    }

    /// Records one view's planar fog (layer 16) in reservation order,
    /// each selected part with
    /// every pass of its material's layer-16 set, blended into the target
    /// over the depth (loaded and kept). The uniforms are
    /// [`GpuWorld::prepare_fog`]'s. Returns how many parts were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_fog(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        selected: &[(Owner, u8)],
    ) -> usize {
        let Some(set) = self.fog.as_mut() else { return 0 };
        set.pipelines(device, color_format, depth_format);
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
        // pass covers the whole target, as the world's programs do. The
        // buffer is the program vertices (the layer-3 programs' own):
        // clusters' are their placed world positions, and a placement's
        // unplaced local coordinates, which its rigid program places with
        // its rows.
        pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
        pass.set_vertex_buffer(0, self.program_vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        set.draw(&mut pass, color_format, depth_format, selected)
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
    /// draw's index ranges from the active gathered cluster items),
    /// added into the target over the depth
    /// (loaded and kept), in order. Nothing without [`Self::set_lights`].
    /// Returns the bundles and their range count.
    pub fn prepare_lights(
        &mut self,
        target: &d3d::ExternalTarget<'_>,
        view: &ViewInputs,
        draws: &[LightDraw<'_>],
        stage_image: &mut dyn FnMut(usize, LightStage) -> Option<Image>,
        gel_image: &mut dyn FnMut(u32) -> Option<Image>,
    ) -> Option<crate::light_gpu::LightBundles> {
        self.lights.as_mut()?.bundles(
            target, &self.program_vertices, &self.indices, view, draws, stage_image, gel_image,
        )
    }

    /// The stencil the material programs draw with from the next pass
    /// ([`PassStencil::from_device`]).
    pub fn set_stencil(&mut self, stencil: PassStencil) {
        self.stencil = stencil;
    }

    /// The cull the material programs whose pass does not set `CULLMODE`
    /// draw with from the next pass
    /// ([`crate::program::InheritedCull`]).
    pub fn set_inherited_cull(&mut self, cull: crate::program::InheritedCull) {
        self.inherited_cull = cull;
        if let Some(p) = &mut self.programs {
            p.set_cull(cull);
        }
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

    /// The frame's view for the programs' uniforms (the device's view
    /// constants, the target's sizes and depth scale).
    pub fn prepare_programs(&self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs) {
        if let Some(p) = &self.programs {
            p.prepare(uploads, view);
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
            gfx::churn::create_render_pipeline(device, &wgpu::RenderPipelineDescriptor {
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

    /// Resolve all geometry and program bindings the guest's selected parts need
    /// on this attachment pair, including the per-placement uniform offset.
    pub fn prepare_parts(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        parts: &[(Owner, u8)], camera: &RenderCamera,
    ) -> bool {
        self.select_parts(parts, camera).is_some_and(|selection| self.prepare_selection(device, color, depth, &selection))
    }

    /// Resolve this guest list against the current BSP and camera, once for all target copies.
    pub fn select_parts(&self, parts: &[(Owner, u8)], camera: &RenderCamera) -> Option<WorldSelection> {
        let frustum = Self::draw_frustum(&camera.view_projection(), self.has_programs());
        let mut selection = WorldSelection::default();
        for part in parts {
            let &slot = self.part_slots.get(part)?;
            let draw = &self.world.draws[slot];
            if draw.index_count == 0 || !frustum.may_see(&draw.bounds) {
                return None;
            }
            selection.push(slot, draw);
        }
        Some(selection)
    }

    /// Check bindings on this attachment pair before the guest parts can be suppressed.
    pub fn prepare_selection(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, selection: &WorldSelection,
    ) -> bool {
        let Some(p) = &mut self.programs else {
            self.pipeline(device, color, depth);
            return true;
        };
        let state = &self.stencil.state;
        p.selected_pipelines(device, color, depth, state, &selection.materials);
        selection.spans.iter().all(|&(batch, _, _)| p.batch_textures(batch).is_some())
            && selection.vertices.iter().all(|&(material, instance)| match instance {
                Some(n) => p.rigid(material, n, color, depth, state).is_some(),
                None => p.cluster(material, color, depth, state).is_some(),
            })
    }

    /// Stage one target copy of a selection admitted by `prepare_selection` on this world.
    pub fn write_selection(
        &self, uploads: &mut gfx::ring::EncoderUploads<'_>, selection: &WorldSelection, view: Option<&ViewInputs>,
        camera: &RenderCamera, depth: DepthRule,
    ) -> bool {
        if let Some(p) = &self.programs {
            let Some(view) = view else { return false };
            p.write_selected(uploads, view, selection);
        } else {
            self.write_frame(uploads, camera, depth);
        }
        true
    }

    fn prepare_pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        self.pipeline(device, color, depth);
        if let Some(p) = &mut self.programs {
            p.pipelines(device, color, depth, &self.stencil.state, self.inherited_cull);
        }
    }

    /// The same visibility test for guest admission and indirect arguments.
    /// Material programs supply the depth range. The fixed stage uses the camera's range.
    pub fn draw_frustum(vp: &crate::camera::Mat4, programs: bool) -> Frustum {
        if programs { Frustum::sides(vp) } else { Frustum::from_view_projection(vp) }
    }

    /// The frame's camera: culls on the CPU and writes the uniform and the slots' instance counts.
    pub fn prepare(&mut self, uploads: &mut gfx::ring::EncoderUploads<'_>, camera: &RenderCamera, depth: DepthRule) -> CullStats {
        let stats = self.cull_stats(camera);
        self.write_frame(uploads, camera, depth);
        if !self.slots.is_empty() {
            uploads.write(&self.indirect, 0, bytemuck::cast_slice(&self.slots));
        }
        stats
    }

    pub fn cull_stats(&mut self, camera: &RenderCamera) -> CullStats {
        let frustum = Self::draw_frustum(&camera.view_projection(), self.has_programs());
        cull(&self.world, &frustum, &mut self.slots)
    }

    fn write_frame(&self, uploads: &mut gfx::ring::EncoderUploads<'_>, camera: &RenderCamera, depth: DepthRule) {
        let u = FrameUniform {
            view_proj: camera.view_projection(),
            depth: [depth.full_scale, 1.0 / camera.far, f32::from(u8::from(depth.floor)), 0.0],
        };
        uploads.write(&self.frame, 0, bytemuck::bytes_of(&u));
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
        self.encode_parts(device, encoder, color, color_format, depth, depth_format, viewport, target_size, None);
    }

    /// Record precisely the guest's ordered part sequence, including repeated
    /// parts. Adjacent slots in the same batch may share a multi-draw.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_parts(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        viewport: PassViewport,
        target_size: (u32, u32),
        parts: Option<&[(Owner, u8)]>,
    ) -> usize {
        let mut spans = Vec::<(usize, usize, usize)>::new();
        if let Some(parts) = parts {
            for part in parts {
                let Some(&slot) = self.part_slots.get(part) else { return 0 };
                if self.slots[slot].instance_count == 0 { return 0; }
                let batch = self.world.draws[slot].batch as usize;
                if let Some((last_batch, _, end)) = spans.last_mut() && *last_batch == batch && *end == slot {
                    *end += 1;
                } else {
                    spans.push((batch, slot, slot + 1));
                }
            }
        } else {
            spans.extend(self.world.batches.iter().enumerate().map(|(i, b)| {
                let first = b.first_draw as usize;
                (i, first, first + b.draw_count as usize)
            }));
        }
        self.prepare_pipelines(device, color_format, depth_format);
        self.record_spans(encoder, color, color_format, depth, depth_format, viewport, target_size, &spans, false)
    }

    /// All selected draws passed the owner and frustum checks before admission.
    /// Their index ranges are immutable and can be issued directly.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_selection(
        &mut self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView, depth_format: wgpu::TextureFormat, viewport: PassViewport, target_size: (u32, u32),
        selection: &WorldSelection,
    ) -> usize {
        self.record_spans(encoder, color, color_format, depth, depth_format, viewport, target_size, &selection.spans, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_spans(
        &mut self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView, depth_format: wgpu::TextureFormat, viewport: PassViewport, target_size: (u32, u32),
        spans: &[(usize, usize, usize)], selected: bool,
    ) -> usize {
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
            return 0;
        }
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        // Which pipeline is bound: None before the first batch, Some(false)
        // stage 1's, Some(true) a material program's (its groups are set
        // per run).
        let mut own: Option<bool> = None;
        let mut unbound = 0u64;
        let mut recorded = 0;
        let mut bound_program = None;
        let mut pending_indices = None;
        let issue = |pass: &mut wgpu::RenderPass<'_>, pending: &mut Option<std::ops::Range<u32>>, first: usize, end: usize| {
            if selected {
                for d in &self.world.draws[first..end] {
                    let next = d.first_index..d.first_index + d.index_count;
                    if let Some(previous) = append_triangles(pending, next) {
                        pass.draw_indexed(previous, 0, 0..1);
                    }
                }
            } else {
                pass.multi_draw_indexed_indirect(&self.indirect, first as u64 * SLOT_SIZE, (end - first) as u32);
            }
        };
        for &(i, first, last) in spans {
            let b = &self.world.batches[i];
            if let Some(p) = &self.programs {
                let Some(textures) = p.batch_textures(i) else {
                    unbound += 1;
                    continue;
                };
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
                let draws = &self.world.draws[first..last];
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
                    let instance = match owner {
                        Owner::Cluster(_) => None,
                        Owner::Instance(n) => Some(n),
                    };
                    // Formats and pipeline state are fixed for the pass. The
                    // actual texture group also covers per-page lightmap bindings.
                    let binding = (b.material, textures, instance, reference);
                    if bound_program != Some(binding) {
                        if let Some(previous) = pending_indices.take() {
                            pass.draw_indexed(previous, 0, 0..1);
                        }
                        match owner {
                            Owner::Cluster(_) => {
                                let Some((program, uniforms)) = p.cluster(b.material, color_format, depth_format, &self.stencil.state) else {
                                    unbound += 1;
                                    k = end;
                                    continue;
                                };
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
                        bound_program = Some(binding);
                    }
                    issue(&mut pass, &mut pending_indices, first + k, first + end);
                    recorded += end - k;
                    k = end;
                }
                continue;
            } else {
                if own != Some(false) {
                    pass.set_viewport(viewport.x, viewport.y, viewport.width, viewport.height, 0.0, 1.0);
                    pass.set_vertex_buffer(0, self.vertices.slice(..));
                    pass.set_pipeline(&self.pipelines[&(color_format, depth_format)]);
                    pass.set_bind_group(0, &self.frame_group, &[]);
                }
                own = Some(false);
                if let Some(previous) = pending_indices.take() {
                    pass.draw_indexed(previous, 0, 0..1);
                }
                pass.set_bind_group(1, &self.batch_groups[i], &[]);
            }
            issue(&mut pass, &mut pending_indices, first, last);
            recorded += last - first;
        }
        if let Some(previous) = pending_indices {
            pass.draw_indexed(previous, 0, 0..1);
        }
        drop(pass);
        self.unbound_batches += unbound;
        recorded
    }
}

/// Join adjacent complete triangle lists only while their render state is bound.
fn append_triangles(
    pending: &mut Option<std::ops::Range<u32>>, next: std::ops::Range<u32>,
) -> Option<std::ops::Range<u32>> {
    if let Some(previous) = pending
        && previous.end == next.start && (previous.end - previous.start).is_multiple_of(3)
    {
        previous.end = next.end;
        return None;
    }
    pending.replace(next)
}

#[cfg(test)]
mod tests {
    use super::{Blocks, Image, PassStencil, append_triangles, block_bytes};

    #[test]
    fn consecutive_triangles_merge_without_losing_repeated_or_disjoint_draws() {
        let mut pending = None;
        assert_eq!(append_triangles(&mut pending, 6..9), None);
        assert_eq!(append_triangles(&mut pending, 9..15), None);
        assert_eq!(append_triangles(&mut pending, 9..15), Some(6..15));
        assert_eq!(append_triangles(&mut pending, 18..20), Some(9..15));
        // An incomplete primitive at a draw boundary must remain incomplete.
        assert_eq!(append_triangles(&mut pending, 20..23), Some(18..20));
        assert_eq!(pending, Some(20..23));
    }

    #[test]
    fn selection_preserves_guest_order_and_repeats_but_updates_each_destination_once() {
        use super::*;
        let cluster = WorldDraw {
            owner: Owner::Cluster(3), part: 1, material: 2, batch: 4, lightmap: None, inputs: 0,
            first_index: 3, index_count: 3, bounds: crate::world::Aabb::EMPTY,
        };
        let placement = WorldDraw { owner: Owner::Instance(5), ..cluster };
        let other = WorldDraw { batch: 1, material: 9, ..placement };
        let mut selection = WorldSelection::default();
        for (slot, draw) in [(7, &cluster), (8, &placement), (2, &other), (8, &placement), (8, &placement)] {
            selection.push(slot, draw);
        }
        assert_eq!(selection.spans, [(4, 7, 9), (1, 2, 3), (4, 8, 9), (4, 8, 9)]);
        assert_eq!(selection.materials[&2], MaterialUse { cluster: true, rigid: true });
        assert_eq!(selection.materials[&9], MaterialUse { cluster: false, rigid: true });
        assert_eq!(selection.vertices, [(2, None), (2, Some(5)), (9, Some(5))].into());
    }

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
