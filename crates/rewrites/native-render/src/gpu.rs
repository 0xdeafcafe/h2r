//! World geometry and materials live on the GPU for the BSP. Guest-selected
//! parts draw their index ranges in list order. Whole-world rendering draws
//! indirect slots the GPU culls ([`crate::cull_gpu`]).

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::camera::{Mat4, RenderCamera};
use crate::cull::{CullStats, Frustum};
use crate::cull_gpu::{DepthModel, GpuCull, GuestDispatch, OcclusionSkips, SLOT_SIZE, ScreenMap};
use crate::fog_gpu::FogSet;
use crate::light_gpu::{LightDraw, LightSet, LightTarget};
use crate::program::{MaterialUse, PipelineUse, ProgramSet, ViewInputs, WorldPipelines};
use crate::world::{LightmapKey, Owner, World, WorldDraw, WorldVertex};

/// The player viewport in physical target texels. External requests round
/// both edges when scaling from guest pixels.
pub(crate) fn viewport_scissor(viewport: [f32; 4], size: (u32, u32)) -> Option<[u32; 4]> {
    let [x, y, w, h] = viewport.map(|v| v.max(0.0).round() as u32);
    let (x, y) = (x.min(size.0), y.min(size.1));
    let (w, h) = (w.min(size.0 - x), h.min(size.1 - y));
    (w != 0 && h != 0).then_some([x, y, w, h])
}

/// An RGBA8 image (linear rows, no padding) with its smaller levels.
/// Clones share the decoded storage across material programs.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pixels: Arc<ImagePixels>,
}

#[derive(Debug)]
pub struct ImagePixels {
    pub rgba: Vec<u8>,
    /// Levels 1.. of the mip chain, each half the last (at least 1).
    pub mips: Vec<Vec<u8>>,
    /// The same levels as BC blocks, for a DXT image the D3D path uploads
    /// as BC ([`Image::with_blocks`]): uploaded instead of `rgba` where the
    /// device takes BC textures, so the GPU decodes them as it does the
    /// D3D path's (the CPU decoder rounds the interpolated colours down).
    pub blocks: Option<Blocks>,
    bytes: mem_watch::Gauge,
}

impl Deref for Image {
    type Target = ImagePixels;

    fn deref(&self) -> &Self::Target {
        &self.pixels
    }
}

impl PartialEq for ImagePixels {
    fn eq(&self, other: &Self) -> bool {
        self.rgba == other.rgba && self.mips == other.mips && self.blocks == other.blocks
    }
}

impl ImagePixels {
    fn new(rgba: Vec<u8>, mips: Vec<Vec<u8>>) -> Self {
        static DECODED: mem_watch::Counter = mem_watch::Counter::new("native.decoded_image_bytes");
        let bytes = DECODED.track((rgba.capacity() + mips.iter().map(Vec::capacity).sum::<usize>()) as u64);
        Self { rgba, mips, blocks: None, bytes }
    }

    fn set_blocks(&mut self, blocks: Option<Blocks>) {
        let capacity = |b: &Blocks| b.levels.iter().map(Vec::capacity).sum::<usize>() as u64;
        self.bytes.set(self.bytes.get() - self.blocks.as_ref().map_or(0, capacity)
            + blocks.as_ref().map_or(0, capacity));
        self.blocks = blocks;
    }
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
        Self::from_rgba(1, 1, rgba.to_vec(), Vec::new())
    }

    /// Builds a caller-validated RGBA chain. Decoders use `from_levels`
    /// when mip dimensions and sizes come from the source.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>, mips: Vec<Vec<u8>>) -> Self {
        Self { width, height, pixels: Arc::new(ImagePixels::new(rgba, mips)) }
    }

    fn uncompressed_mut(&mut self) -> &mut ImagePixels {
        if Arc::get_mut(&mut self.pixels).is_none() {
            self.pixels = Arc::new(ImagePixels::new(self.rgba.clone(), self.mips.clone()));
        }
        let pixels = Arc::get_mut(&mut self.pixels).expect("image payload is unique");
        pixels.set_blocks(None);
        pixels
    }

    // Fixed lengths keep the payload's byte probe accurate.
    pub(crate) fn edit_rgba(&mut self, edit: impl FnOnce(&mut [u8], &mut [Vec<u8>])) {
        let pixels = self.uncompressed_mut();
        edit(&mut pixels.rgba, &mut pixels.mips);
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
            self.uncompressed_mut().set_blocks(Some(Blocks { format, levels }));
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
        Some(Self::from_rgba(width, height, rgba, mips))
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

/// Which draws a recorded run issues: the selection's index ranges, or
/// the early or late indirect slots of the GPU cull.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slots {
    Selected,
    Early,
    Late,
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
    pub texaccum: Option<crate::world_layers::Texaccum>,
    spans: Vec<(usize, usize, usize)>,
    pub(crate) materials: HashMap<u16, MaterialUse>,
    pub(crate) vertices: HashSet<(u16, Option<u32>)>,
    /// The selected animated materials' constants at this frame's clock
    /// ([`GpuWorld::animate_selection`]).
    pub(crate) constants: HashMap<u16, crate::fog_gpu::LayerConstants>,
    /// The placements the structure draws faded this frame (retail's LOD
    /// display fade), each with the table pattern its fade selects, and
    /// the table's rows ([`GpuWorld::set_fades`]). Empty draws every
    /// placement solid.
    fades: HashMap<u32, u8>,
    stipple: Option<Box<[[u32; 32]; crate::program::STIPPLE_LEVELS]>>,
}

impl WorldSelection {
    /// The pattern placement `instance` draws with, when it is faded.
    pub(crate) fn fade_level(&self, instance: u32) -> Option<u8> {
        self.fades.get(&instance).copied()
    }

    /// The stipple table's rows, when any placement is faded.
    pub(crate) fn stipple_table(&self) -> Option<&[[u32; 32]; crate::program::STIPPLE_LEVELS]> {
        self.stipple.as_deref()
    }

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
    _geometry_bytes: mem_watch::Gauge,
    /// The stencil the next pass draws with ([`GpuWorld::set_stencil`]).
    stencil: PassStencil,
    /// The cull the next pass's inheriting programs draw with
    /// ([`GpuWorld::set_inherited_cull`]).
    inherited_cull: crate::program::InheritedCull,
    vertices: wgpu::Buffer,
    /// A retained light holds the current program vertex allocation.
    light_mesh_device: Option<wgpu::Device>,
    /// [`World::program_vertices`]: what the material programs read.
    program_vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// The whole-world pass's indirect slots and the compute cull that
    /// writes them.
    cull: GpuCull,
    /// The game views' cull of the guest's selected parts
    /// ([`GpuWorld::culled_selection_draws`]).
    guest_cull: GpuCull,
    /// The later world layers' cull of their structure items
    /// ([`GpuWorld::culled_layer_draws`]).
    layer_cull: GpuCull,
    /// The decorators' cull ([`GpuWorld::culled_decorators`]), made on
    /// first use.
    decorator_cull: Option<crate::cull_gpu::DecoratorCull>,
    frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    batch_groups: Vec<wgpu::BindGroup>,
    fixed: WorldPipelines,
    /// Materials drawn with their own programs ([`crate::program`]); the
    /// rest draw with stage 1's.
    programs: Option<ProgramSet>,
    /// The fog materials layer 16 draws ([`crate::fog_gpu`]).
    fog: Option<FogSet>,
    /// The materials the scene's layers 1, 5, 7, 8 and 20 draw from the
    /// scene's plan (`crate::layer_pass`), by layer: each layer's passes
    /// recorded as the fog's are.
    layers: HashMap<u8, FogSet>,
    /// The water materials layer 6 draws ([`crate::water_gpu`]).
    water: Option<crate::water_gpu::WaterSet>,
    /// The light passes (stage 4), once a light needs them.
    lights: Option<LightSet>,
    /// Batches left out of a pass because their material program could
    /// not be bound (with programs set; see [`GpuWorld::encode`]).
    unbound_batches: u64,
    /// The programs' pipelines as packets hold them
    /// ([`GpuWorld::selection_draws`]).
    packet_pipelines: HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
}

const SHADER: &str = include_str!("world.wgsl");

/// What [`texture`] makes a texture for: sampling, and writes in place (a
/// live edit's pixels, [`rewrite_texture`]). The crate's own tests also
/// read one back, which needs `COPY_SRC`; the game never does.
#[cfg(not(test))]
const SAMPLED_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING.union(wgpu::TextureUsages::COPY_DST);
#[cfg(test)]
const SAMPLED_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);

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
        usage: SAMPLED_USAGE,
        view_formats: &[],
    });
    write_rgba_levels(queue, &t, image);
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// `image`'s RGBA8 levels over `t`, largest first.
fn write_rgba_levels(queue: &wgpu::Queue, t: &wgpu::Texture, image: &Image) {
    let (width, height) = (t.width(), t.height());
    for (level, bytes) in std::iter::once(&image.rgba).chain(&image.mips).enumerate() {
        let level = level as u32;
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        gfx::churn::upload_bytes(bytes.len());
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: t,
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
        usage: SAMPLED_USAGE,
        view_formats: &[],
    });
    write_block_levels(queue, &t, b, unit);
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// `b`'s levels over `t` (a level smaller than a block at its physical,
/// rounded-up size), `unit` bytes a block.
fn write_block_levels(queue: &wgpu::Queue, t: &wgpu::Texture, b: &Blocks, unit: u32) {
    let (width, height) = (t.width(), t.height());
    for (level, bytes) in b.levels.iter().enumerate() {
        let level = level as u32;
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        let (w, h) = (w.next_multiple_of(4), h.next_multiple_of(4));
        gfx::churn::upload_bytes(bytes.len());
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: t,
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
}

/// Why a texture's pixels cannot be rewritten in place
/// ([`check_rewrite`]). The texture keeps its pixels; the edit shows
/// after a map reload (docs/editor/realtime-edits.md, "Textures").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRefusal {
    /// Not one 2D layer (a cube, volume or layered texture).
    NotPlain,
    /// The image's size differs from the texture's.
    Size,
    /// The image's level count differs from the texture's.
    Levels,
    /// The texture is block-compressed and the image has no blocks of
    /// its format.
    Format,
    /// The image did not decode.
    NotDecoded,
}

/// Whether `image` can be written over `t` as [`texture`] made it: one 2D
/// layer of the same size, level count and format. RGBA8 takes the
/// image's RGBA levels; a BC texture its blocks of the same format.
pub fn check_rewrite(t: &wgpu::Texture, image: &Image) -> Result<(), ImageRefusal> {
    if t.dimension() != wgpu::TextureDimension::D2 || t.depth_or_array_layers() != 1 {
        return Err(ImageRefusal::NotPlain);
    }
    if (t.width(), t.height()) != (image.width.max(1), image.height.max(1)) {
        return Err(ImageRefusal::Size);
    }
    let levels = if t.format() == wgpu::TextureFormat::Rgba8Unorm {
        1 + image.mips.len()
    } else {
        match &image.blocks {
            Some(b) if b.format == t.format() => b.levels.len(),
            _ => return Err(ImageRefusal::Format),
        }
    };
    if levels as u32 != t.mip_level_count() {
        return Err(ImageRefusal::Levels);
    }
    Ok(())
}

/// Writes every level of `image` over `t` in place ([`check_rewrite`]
/// first): a live edit's same-size pixels, visible to the next pass that
/// samples `t`.
pub fn rewrite_texture(queue: &wgpu::Queue, t: &wgpu::Texture, image: &Image) -> Result<(), ImageRefusal> {
    check_rewrite(t, image)?;
    match (t.format(), &image.blocks) {
        (wgpu::TextureFormat::Rgba8Unorm, _) => write_rgba_levels(queue, t, image),
        (format, Some(b)) => write_block_levels(queue, t, b, block_bytes(format).ok_or(ImageRefusal::Format)? as u32),
        (_, None) => return Err(ImageRefusal::Format),
    }
    Ok(())
}

/// A texture made from a decoded stage image: the stage that sampled it
/// and the channels stored biased in it ([`crate::program::bias_signed`]).
/// What each holder reports for a live edit, and [`rewrite_held`] writes.
pub type Held = (wgpu::Texture, usize, [bool; 4]);

/// The stages of `held`, ascending, each once: what the glue decodes for
/// an edited image.
pub fn held_stages(held: &[Held]) -> Vec<usize> {
    let mut stages: Vec<usize> = held.iter().map(|&(_, n, _)| n).collect();
    stages.sort_unstable();
    stages.dedup();
    stages
}

/// Rewrites every texture of `held` in place with `decoded` (by stage),
/// biased for its signed channels as at upload. Every texture is checked
/// before the first write; a size, level count or format change refuses
/// them all (docs/editor/realtime-edits.md, "The native refresh"). A
/// texture listed twice is written once. Returns how many were written.
pub fn rewrite_held(queue: &wgpu::Queue, held: &[Held], decoded: &HashMap<usize, Image>) -> Result<usize, ImageRefusal> {
    // `wgpu::Texture` hashes and compares by its id, not its interior state.
    #[allow(clippy::mutable_key_type)]
    let mut seen = std::collections::HashSet::new();
    let mut writes = Vec::new();
    for (t, n, signed) in held {
        if !seen.insert((t.clone(), *n)) {
            continue;
        }
        let mut pixels = decoded.get(n).ok_or(ImageRefusal::NotDecoded)?.clone();
        crate::program::bias_signed(&mut pixels, *signed);
        check_rewrite(t, &pixels)?;
        writes.push((t, pixels));
    }
    for (t, pixels) in &writes {
        rewrite_texture(queue, t, pixels)?;
    }
    Ok(writes.len())
}

/// The textures behind the pages a set bound (a page that did not decode
/// has none), by page.
pub(crate) fn page_textures_of(
    pages: &HashMap<LightmapKey, Option<wgpu::TextureView>>,
) -> HashMap<LightmapKey, wgpu::Texture> {
    pages.iter().filter_map(|(&k, v)| Some((k, v.as_ref()?.texture().clone()))).collect()
}

/// Rewrites each of `pages` (the textures that hold one lightmap page)
/// in place with `image`, all or nothing as [`rewrite_held`]. Returns how
/// many were written.
pub fn rewrite_pages(queue: &wgpu::Queue, pages: &[wgpu::Texture], image: &Image) -> Result<usize, ImageRefusal> {
    #[allow(clippy::mutable_key_type)]
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&wgpu::Texture> = pages.iter().filter(|t| seen.insert((*t).clone())).collect();
    for t in &unique {
        check_rewrite(t, image)?;
    }
    for t in &unique {
        rewrite_texture(queue, t, image)?;
    }
    Ok(unique.len())
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
        // COPY_DST: an editor moves a section's vertices in place
        // ([`GpuWorld::set_section_vertices`]).
        let vertex_usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let vertices = init("native world vertices", bytemuck::cast_slice(verts), vertex_usage);
        let program_vertices = if world.program_vertices.len() == world.vertices.len() && !world.program_vertices.is_empty() {
            init(
                "native world program vertices",
                bytemuck::cast_slice(&world.program_vertices),
                vertex_usage,
            )
        } else {
            vertices.clone()
        };
        let indices = init("native world indices", bytemuck::cast_slice(idx), wgpu::BufferUsages::INDEX);
        let cull = GpuCull::new(device, &world);
        let guest_cull = GpuCull::new(device, &world);
        let layer_cull = GpuCull::new(device, &world);
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native world"),
            bind_group_layouts: &[Some(&frame_layout), Some(&batch_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x3];
        let fixed = WorldPipelines::new(device, SHADER, shader, layout, gfx::pipeline::ShaderId(0), "native world", attributes.to_vec());
        let part_slots = world.draws.iter().enumerate().map(|(i, d)| ((d.owner, d.part), i)).collect();
        static GEOMETRY: mem_watch::Counter = mem_watch::Counter::new("native.world_cpu_geometry_bytes");
        let geometry_bytes = (world.vertices.capacity() + world.program_vertices.capacity()) * size_of::<WorldVertex>()
            + world.indices.capacity() * size_of::<u32>();
        Self {
            part_slots, world,
            _geometry_bytes: GEOMETRY.track(geometry_bytes as u64),
            stencil: PassStencil::default(),
            inherited_cull: crate::program::InheritedCull::default(),
            vertices,
            light_mesh_device: None,
            program_vertices,
            indices,
            cull,
            guest_cull,
            layer_cull,
            decorator_cull: None,
            frame,
            frame_group,
            batch_groups,
            fixed,
            programs,
            fog: None,
            layers: HashMap::new(),
            water: None,
            lights: None,
            unbound_batches: 0,
            packet_pipelines: HashMap::new(),
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

    /// Move `owner`'s section to `positions` (and `normals`), in its own
    /// space, and write its run of both vertex buffers: the editor's
    /// live preview of a geometry edit (docs/editor/modeling.md, "The
    /// tools in the window"). False when the world has no such section
    /// or the counts differ. Culling bounds stay as uploaded.
    pub fn set_section_vertices(
        &mut self,
        queue: &wgpu::Queue,
        owner: Owner,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
    ) -> bool {
        let Some(run) = self.world.set_section_vertices(owner, positions, normals) else {
            return false;
        };
        self.write_run(queue, run);
        true
    }

    /// Replace `owner`'s section with `section`, decoded again from the
    /// edited map, and write its run of both vertex buffers: a live
    /// edit's native refresh (docs/editor/realtime-edits.md, "The native
    /// refresh"). The draw and world bounds the culling and the guest's
    /// admission read are recomputed ([`World::replace_section`]). A
    /// refusal changes nothing; the host reloads the world.
    pub fn replace_section(
        &mut self,
        queue: &wgpu::Queue,
        owner: Owner,
        section: &crate::world::SectionInput,
    ) -> Result<crate::world::SectionRun, crate::world::SectionRefusal> {
        let run = self.world.replace_section(owner, section)?;
        self.write_run(queue, run);
        self.cull.write_bounds(queue, &self.world);
        self.guest_cull.write_bounds(queue, &self.world);
        self.layer_cull.write_bounds(queue, &self.world);
        Ok(run)
    }

    /// Every texture of the world that is made from image `image` of
    /// `bitmap`: the material programs' stages, the fog's, each layer's
    /// and the water's.
    pub fn held(&self, bitmap: map::TagId, image: u32) -> Vec<Held> {
        let mut held = self.programs.as_ref().map_or_else(Vec::new, |p| p.held(bitmap, image));
        if let Some(f) = &self.fog {
            held.extend(f.held(bitmap, image));
        }
        let mut layers: Vec<_> = self.layers.iter().collect();
        layers.sort_by_key(|(l, _)| **l);
        for (_, f) in layers {
            held.extend(f.held(bitmap, image));
        }
        if let Some(w) = &self.water {
            held.extend(w.held(bitmap, image));
        }
        held
    }

    /// Rewrite the textures that hold material `material`'s base map (the
    /// editor's base look, `HostStage::BaseMap`) with `image`, as
    /// [`GpuWorld::rewrite_image`] does for a tag's image: the viewport
    /// follows a painted bitmap. Returns how many textures were written;
    /// a different size, level count or format refuses them all.
    pub fn rewrite_base_map(&self, queue: &wgpu::Queue, material: u16, image: &Image) -> Result<usize, ImageRefusal> {
        let want = crate::look::Stage::Host(crate::look::HostStage::BaseMap(material));
        let mut held = Vec::new();
        let mut layers: Vec<_> = self.layers.iter().collect();
        layers.sort_by_key(|(l, _)| **l);
        for (_, f) in layers {
            held.extend(f.held_stage(want));
        }
        let decoded = held_stages(&held).into_iter().map(|n| (n, image.clone())).collect();
        rewrite_held(queue, &held, &decoded)
    }

    /// The stages of the world's textures that sample image `image` of
    /// `bitmap`, ascending, each once: what [`GpuWorld::rewrite_image`]
    /// needs decoded (a palettized image expands through its stage's
    /// palette).
    pub fn image_stages(&self, bitmap: map::TagId, image: u32) -> Vec<usize> {
        held_stages(&self.held(bitmap, image))
    }

    /// The program material `material` draws with, if it has one.
    pub fn material_program(&self, material: u16) -> Option<&crate::program::MaterialProgram> {
        self.programs.as_ref()?.program(material)
    }

    /// Puts `edited` (each material's program compiled from an edited
    /// shader) in place, as [`ProgramSet::refresh`] does: constants
    /// rewritten in place, other changes rebuilt. Without programs there
    /// is nothing to refresh, and a non-empty `edited` is refused.
    pub fn refresh_programs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        edited: HashMap<u16, crate::program::MaterialProgram>,
        stage_image: &mut dyn FnMut(crate::look::Stage) -> Option<crate::gpu::Image>,
        layered: &HashMap<(map::TagId, u32), crate::light_gpu::LayeredImage>,
    ) -> Result<crate::program::Refreshed, String> {
        if edited.is_empty() {
            return Ok(crate::program::Refreshed::default());
        }
        let programs = self.programs.as_mut().ok_or("the world draws no material programs")?;
        programs.refresh(device, queue, &self.world, edited, stage_image, layered)
    }

    /// Rewrite every texture made from image `image` of `bitmap` with
    /// `decoded` (by stage, [`GpuWorld::image_stages`]), each signed
    /// variant biased as at load, in the material programs, the fog, the
    /// layers and the water. All or nothing: a texture whose size, level
    /// count or format differs refuses the lot. Returns how many textures
    /// were written.
    pub fn rewrite_image(
        &self,
        queue: &wgpu::Queue,
        bitmap: map::TagId,
        image: u32,
        decoded: &HashMap<usize, Image>,
    ) -> Result<usize, ImageRefusal> {
        rewrite_held(queue, &self.held(bitmap, image), decoded)
    }

    /// The textures that hold lightmap page `key`: the material programs',
    /// the fog's, each layer's and the water's.
    pub fn page_textures(&self, key: LightmapKey) -> Vec<wgpu::Texture> {
        let mut pages = self.programs.as_ref().map_or_else(Vec::new, |p| p.page_textures(key));
        if let Some(f) = &self.fog {
            pages.extend(f.page_textures(key));
        }
        let mut layers: Vec<_> = self.layers.iter().collect();
        layers.sort_by_key(|(l, _)| **l);
        for (_, f) in layers {
            pages.extend(f.page_textures(key));
        }
        if let Some(w) = &self.water {
            pages.extend(w.page_textures(key));
        }
        pages
    }

    /// Rewrite every texture of lightmap page `key` with `image` (the
    /// page decoded through its palette, as at load). Returns how many
    /// textures were written; a changed size or format refuses them all.
    pub fn rewrite_page(&self, queue: &wgpu::Queue, key: LightmapKey, image: &Image) -> Result<usize, ImageRefusal> {
        rewrite_pages(queue, &self.page_textures(key), image)
    }

    /// Rewrite gel `datum`'s texture with `image` (image 0 of the gel's
    /// bitmap, as the light set uploaded it). Returns how many were
    /// written: none when the world has no light set or no such gel.
    pub fn rewrite_gel(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, datum: u32, image: &Image) -> Result<usize, ImageRefusal> {
        self.lights.as_mut().map_or(Ok(0), |l| l.rewrite_gel(device, queue, datum, image))
    }

    /// Pin the immutable index buffer, the current program vertices and the
    /// light's textures. A subsequent edit publishes a new vertex allocation.
    pub fn retain_light(
        &mut self,
        device: &wgpu::Device,
        gel: Option<u32>,
    ) -> Option<(LightSet, wgpu::Buffer, wgpu::Buffer)> {
        let set = self.lights.as_ref()?.retain(gel);
        self.light_mesh_device = Some(device.clone());
        Some((set, self.program_vertices.clone(), self.indices.clone()))
    }

    fn write_run(&mut self, queue: &wgpu::Queue, run: crate::world::SectionRun) {
        let range = run.first_vertex as usize..(run.first_vertex + run.vertex_count) as usize;
        let offset = (run.first_vertex as usize * std::mem::size_of::<WorldVertex>()) as u64;
        let separate = self.world.program_vertices.len() == self.world.vertices.len()
            && !self.world.program_vertices.is_empty();
        if let Some(device) = self.light_mesh_device.take() {
            let vertices = if separate { &self.world.program_vertices } else { &self.world.vertices };
            self.program_vertices = gfx::churn::create_buffer_init(&device, &wgpu::util::BufferInitDescriptor {
                label: Some("edited light vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            if !separate {
                self.vertices = self.program_vertices.clone();
            } else {
                queue.write_buffer(&self.vertices, offset, bytemuck::cast_slice(&self.world.vertices[range]));
            }
            return;
        }
        queue.write_buffer(&self.vertices, offset, bytemuck::cast_slice(&self.world.vertices[range.clone()]));
        if separate {
            queue.write_buffer(
                &self.program_vertices,
                offset,
                bytemuck::cast_slice(&self.world.program_vertices[range]),
            );
        }
    }

    /// Draws the world's layer-16 passes of `set`'s materials from now on.
    pub fn set_fog(&mut self, set: FogSet) {
        self.fog = Some(set);
    }

    /// The fog materials, when they are uploaded.
    pub fn fog(&self) -> Option<&FogSet> {
        self.fog.as_ref()
    }

    /// Draws `layer`'s passes of `set`'s materials from now on.
    pub fn set_layer(&mut self, layer: u8, set: FogSet) {
        self.layers.insert(layer, set);
    }

    /// `layer`'s materials, when they are uploaded.
    pub fn layer(&self, layer: u8) -> Option<&FogSet> {
        self.layers.get(&layer)
    }

    /// One view's draws of `layer`'s selected parts as packets for the
    /// D3D pass already open on these attachments, the uniforms as writes
    /// ahead of them ([`FogSet::draws_stencil`]; `fogs` the fog unit the
    /// passes hold and `stencil` the stencil state, both the device's at
    /// the layer). `samples` holds the registers the planner evaluates at
    /// the layer call.
    pub fn layer_draws(
        &mut self,
        layer: u8,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        selected: &[(Owner, u8)],
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        stencil: &gfx::state::StencilState,
        samples: &crate::fog_gpu::LayerSamples,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        let Self { layers, packet_pipelines, program_vertices, indices, .. } = self;
        let set = layers.get_mut(&layer).ok_or("layer materials unavailable")?;
        let pipeline_stencil = crate::fog_gpu::layer_stencil(stencil, depth_format);
        set.prepare_pipelines_stencil(device, color_format, depth_format, &pipeline_stencil, PipelineUse::Direct);
        if packet_pipelines.len() > 4096 {
            packet_pipelines.clear();
        }
        set.draws_stencil(packet_pipelines, program_vertices, indices, color_format, depth_format, target_size, gfx::ScissorRect::full(target_size.0, target_size.1), selected, view, fogs, stencil, samples, None)
    }

    /// [`GpuWorld::layer_draws`] for a later layer's structure runs, culled
    /// on the GPU (docs/graphics/native-world.md, "Cull"): each run's
    /// packets draw their parts' slots of the layer cull, which
    /// [`GuestDispatch::record`] writes from `Frustum::sides` of `camera`
    /// over every run's parts. With `occlusion` (the depth target, the
    /// copy's lens and rectangle, and the game view's token) an item every
    /// pass of which draws nothing where hidden (`hidden_draws_nothing`)
    /// is also tested against the pyramid that game view's layer 3 built
    /// on this target ([`GpuWorld::culled_selection_draws`] with the same
    /// token, screen and depth model); without one the frustum alone
    /// culls (`OcclusionSkips::layer_view`). The caller records `record`,
    /// then the runs' packets, in one encoder.
    #[allow(clippy::too_many_arguments)]
    pub fn culled_layer_draws(
        &mut self, layer: u8, uploads: &gfx::ring::SubmissionUploads, device: &wgpu::Device, queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        runs: &[&[(Owner, u8)]], view: &ViewInputs, fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        stencil: &gfx::state::StencilState, samples: &crate::fog_gpu::LayerSamples, camera: &RenderCamera,
        occlusion: Option<(&wgpu::Texture, &RenderCamera, PassViewport, u64)>,
    ) -> Result<(Vec<gfx::external::ExternalDraws>, GuestDispatch), &'static str> {
        let Self { layers, packet_pipelines, program_vertices, indices, part_slots, layer_cull, guest_cull, world, .. } = self;
        let set = layers.get_mut(&layer).ok_or("layer materials unavailable")?;
        let pipeline_stencil = crate::fog_gpu::layer_stencil(stencil, depth_format);
        set.prepare_pipelines_stencil(device, color_format, depth_format, &pipeline_stencil, PipelineUse::Direct);
        if packet_pipelines.len() > 4096 {
            packet_pipelines.clear();
        }
        let mut start = vec![0u32; world.draws.len()];
        let mut out = Vec::with_capacity(runs.len());
        for selected in runs {
            let slots = crate::fog_gpu::LayerSlots { early: &layer_cull.early, slots: &*part_slots, start: &mut start[..] };
            out.push(set.draws_stencil(packet_pipelines, program_vertices, indices, color_format, depth_format, target_size,
                gfx::ScissorRect::full(target_size.0, target_size.1), selected, view, fogs, stencil, samples, Some(slots))?);
        }
        let vp = camera.view_projection();
        // The game view's pyramid, as its layer 3 tested it.
        let scale = target_size.0 as f32 / view.guest_size.0.max(1) as f32;
        let occlusion = occlusion.and_then(|(depth, lens, viewport, token)| {
            let map = ScreenMap::viewport(&lens.view_projection(), viewport).with_margin(2.0 + scale);
            let model = DepthModel::program(view);
            let served = guest_cull.served_pyramid(depth, token, &map, &model)?;
            Some((model, map, served))
        });
        let dispatch = layer_cull.layer_dispatch(device, queue, uploads, &vp, &Frustum::sides(&vp), &start, occlusion);
        Ok((out, dispatch))
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
    pub fn encode_water(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        viewport: [f32; 4],
        target_copy: gfx::external::TargetCopy,
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
    ) -> usize {
        let Some([x, y, w, h]) = viewport_scissor(viewport, target_size) else { return 0 };
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
        pass.set_scissor_rect(x, y, w, h);
        pass.set_vertex_buffer(0, self.program_vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        set.draw(device, &mut pass, color_format, depth_format, copy.as_ref(), selected, sampled)
    }

    /// [`GpuWorld::encode_water`] as packets for the D3D pass already open
    /// on these attachments, for a set that does not sample the view's own
    /// target: one packet per selected part per pass, the render targets'
    /// views `sampled` in the set's `targets` order, and the uniforms
    /// ([`WaterSet::prepare`]'s bytes for the selected materials) as writes
    /// ahead of them.
    pub fn water_draws(
        &mut self,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        viewport: [f32; 4],
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        let [x, y, width, height] = viewport_scissor(viewport, target_size).ok_or("empty layer viewport")?;
        let scissor = gfx::ScissorRect { x, y, width, height };
        let Self { water, packet_pipelines, program_vertices, indices, .. } = self;
        let set = water.as_mut().ok_or("water materials unavailable")?;
        if set.samples_view {
            return Err("the water samples the view's own target");
        }
        set.pipelines(device, color_format, depth_format);
        if packet_pipelines.len() > 4096 {
            packet_pipelines.clear();
        }
        let items = set.items(device, color_format, depth_format, None, selected, sampled)?;
        let packets = crate::water_gpu::item_packets(&items, packet_pipelines, program_vertices, indices, target_size, scissor)?;
        let writes = set.writes(view, fogs, selected)?;
        // Conservative: the programs' alpha tests and kill sources discard.
        Ok(gfx::external::ExternalDraws { ranges: selected.len(), packets, discards: true, writes })
    }

    /// [`GpuWorld::prepare_water`] and [`GpuWorld::encode_water`] as
    /// commands for the frame's open encoder (`Stage::record`): the uniforms staged now and copied into their
    /// buffers by the commands, the view copy's scratch texture made now
    /// and filled by them, then the pass. The commands only record, so
    /// they run any number of times.
    pub fn water_encoded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        viewport: [f32; 4],
        target_copy: gfx::external::TargetCopy,
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) -> Result<gfx::external::ExternalEncoded, &'static str> {
        let [x, y, w, h] = viewport_scissor(viewport, target_size).ok_or("empty water viewport")?;
        let Self { water, program_vertices, indices, .. } = self;
        let set = water.as_mut().ok_or("water materials unavailable")?;
        set.pipelines(device, color_format, depth_format);
        let scene = set.samples_view.then(|| set.scene_copy(device, color, target_copy));
        let items = set.items(device, color_format, depth_format, scene.as_ref().map(|s| &s.view), selected, sampled)?;
        if items.iter().any(|it| (u64::from(it.first) + u64::from(it.count)) * 4 > indices.size()) {
            return Err("a selected water part reaches past the indices");
        }
        let copies: Vec<(wgpu::Buffer, u64, wgpu::Buffer, u64, u64)> = set
            .writes(view, fogs, selected)?
            .into_iter()
            .map(|(dst, offset, bytes)| {
                let (src, at) = uploads.stage(device, queue, &bytes);
                (src, at, dst, offset, bytes.len() as u64)
            })
            .collect();
        let (color, depth) = (color.clone(), depth.clone());
        let (vertices, indices) = (program_vertices.clone(), indices.clone());
        let has_stencil = depth_format.has_stencil_aspect();
        let ranges = selected.len();
        let draws = items.len();
        let commands = move |encoder: &mut wgpu::CommandEncoder| {
            for (src, at, dst, offset, len) in &copies {
                encoder.copy_buffer_to_buffer(src, *at, dst, *offset, *len);
            }
            if let Some(scene) = &scene {
                scene.record(encoder);
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native water"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: has_stencil
                        .then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, target_size.0 as f32, target_size.1 as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for it in &items {
                pass.set_pipeline(&it.pipeline);
                pass.set_bind_group(0, &it.uniforms, it.offset.as_slice());
                pass.set_bind_group(1, &it.textures, &[]);
                pass.set_blend_constant(it.blend);
                pass.draw_indexed(it.first..it.first + it.count, 0, 0..1);
            }
        };
        Ok(gfx::external::ExternalEncoded { ranges, draws, passes: 1, commands: Box::new(commands) })
    }

    /// The fog uniforms for one view ([`FogSet::prepare`]), through the
    /// submission's upload arena (the reservation records into an
    /// isolated encoder, where a queue write would escape cancellation).
    pub fn prepare_fog(
        &mut self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        samples: &crate::fog_gpu::LayerSamples,
    ) {
        if let Some(set) = self.fog.as_ref() {
            set.prepare_layer(uploads, view, fogs, samples);
        }
    }

    /// Records one view's planar fog (layer 16) in reservation order,
    /// each selected part with
    /// every pass of its material's layer-16 set, blended into the target
    /// over the depth (loaded and kept). The uniforms are
    /// [`GpuWorld::prepare_fog`]'s. Returns how many parts were drawn.
    pub fn encode_fog(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        viewport: [f32; 4],
        selected: &[(Owner, u8)],
    ) -> usize {
        let Self { fog, program_vertices, indices, .. } = self;
        let Some(set) = fog.as_mut() else { return 0 };
        encode_set(set, "native fog", program_vertices, indices, device, encoder, color, color_format, depth, depth_format, target_size, viewport, selected)
    }

    /// The uniforms of `layer`'s set for one view ([`FogSet::prepare`];
    /// `fogs` the fog unit each material draws with) and the material
    /// constants at the preview time, through the submission's upload arena.
    pub fn prepare_layer(
        &mut self,
        layer: u8,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        samples: &crate::fog_gpu::LayerSamples,
    ) {
        if let Some(set) = self.layers.get(&layer) {
            set.prepare_layer(uploads, view, fogs, samples);
        }
    }

    /// Records one view's draws of `layer`'s selected parts in a pass of
    /// their own over the target and its depth (loaded and kept), as
    /// [`GpuWorld::encode_fog`] does layer 16's: for a host that owns its
    /// passes (the editor). The uniforms are [`GpuWorld::prepare_layer`]'s.
    /// Returns how many parts were drawn.
    pub fn encode_layer(
        &mut self,
        layer: u8,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        selected: &[(Owner, u8)],
    ) -> usize {
        let Self { layers, program_vertices, indices, .. } = self;
        let Some(set) = layers.get_mut(&layer) else { return 0 };
        encode_set(set, "native layer", program_vertices, indices, device, encoder, color, color_format, depth, depth_format, target_size, [0.0, 0.0, target_size.0 as f32, target_size.1 as f32], selected)
    }
    /// [`GpuWorld::prepare_fog`] and [`GpuWorld::encode_fog`] as packets for
    /// the D3D pass already open on these attachments, the uniforms as
    /// writes ahead of them ([`FogSet::draws`]).
    pub fn fog_draws(
        &mut self,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        viewport: [f32; 4],
        selected: &[(Owner, u8)],
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        samples: &crate::fog_gpu::LayerSamples,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        let [x, y, width, height] = viewport_scissor(viewport, target_size).ok_or("empty layer viewport")?;
        let scissor = gfx::ScissorRect { x, y, width, height };
        let Self { fog, packet_pipelines, program_vertices, indices, .. } = self;
        let set = fog.as_mut().ok_or("fog materials unavailable")?;
        set.pipelines(device, color_format, depth_format);
        if packet_pipelines.len() > 4096 {
            packet_pipelines.clear();
        }
        set.draws(packet_pipelines, program_vertices, indices, color_format, depth_format, target_size, scissor, selected, view, fogs, samples)
    }

    /// Draws lights with `set` from now on.
    pub fn set_lights(&mut self, set: LightSet) {
        self.lights = Some(set);
    }

    /// The light passes, when the engine's light textures are uploaded.
    pub fn lights(&self) -> Option<&LightSet> {
        self.lights.as_ref()
    }

    /// One run of lights' draws over the world's parts (each draw's index
    /// ranges from the active gathered cluster items) as packets for the
    /// D3D pass open on `target`, added over its depth, in order, within
    /// `scissor` ([`LightSet::draws`]). Refused without
    /// [`Self::set_lights`].
    pub fn prepare_lights(
        &mut self,
        target: &LightTarget<'_>,
        scissor: [u32; 4],
        view: &ViewInputs,
        draws: &[LightDraw<'_>],
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        self.lights.as_mut().ok_or("light passes unavailable")?.draws(
            target, &self.program_vertices, &self.indices, view, scissor, draws,
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

    /// Enqueue the selected programs for the live target. A false result leaves
    /// every selected draw to the guest until a later admission check succeeds.
    pub fn prewarm_world(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, selection: &WorldSelection,
    ) -> bool {
        self.selection_ready(device, color, depth, selection, PipelineUse::Preflight)
    }

    /// Enqueue this BSP's fog, water, or translated scene-layer materials for the bound formats.
    pub fn prewarm_fog(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> bool {
        self.fog.as_mut().is_some_and(|set| set.prepare_pipelines(device, color, depth, PipelineUse::Preflight))
    }

    pub fn prewarm_water(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> bool {
        self.water.as_mut().is_some_and(|set| set.prepare_pipelines(device, color, depth, PipelineUse::Preflight))
    }

    pub fn prewarm_layer(
        &mut self, layer: u8, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        stencil: &gfx::state::StencilState,
    ) -> bool {
        let stencil = crate::fog_gpu::layer_stencil(stencil, depth);
        self.layers.get_mut(&layer).is_some_and(|set| set.prepare_pipelines_stencil(device, color, depth, &stencil, PipelineUse::Preflight))
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
        let selection = self.select_unculled(parts)?;
        selection.spans.iter().flat_map(|&(_, first, end)| first..end)
            .all(|slot| frustum.may_see(&self.world.draws[slot].bounds))
            .then_some(selection)
    }

    /// The guest's list as it is, every part kept whatever the camera sees:
    /// for a game view the GPU culls ([`GpuWorld::culled_selection_draws`]).
    /// `None` for a part the world does not hold or one with no triangles.
    pub fn select_unculled(&self, parts: &[(Owner, u8)]) -> Option<WorldSelection> {
        let mut selection = WorldSelection::default();
        for part in parts {
            let &slot = self.part_slots.get(part)?;
            let draw = &self.world.draws[slot];
            if draw.index_count == 0 {
                return None;
            }
            selection.push(slot, draw);
        }
        Some(selection)
    }

    /// Fades the selection's placements: `fades` maps a placement to the
    /// pattern of the engine's stipple table its fade selects
    /// (`engine::render_loop::opaque_material::stipple_table`), `table` the
    /// table's 17 patterns of 32 rows. A placement with no rigid program is
    /// not in the selection to fade; one the selection does not hold is
    /// ignored. A placement left out draws solid, as before.
    pub fn set_fades(
        &self, selection: &mut WorldSelection, fades: &HashMap<u32, u8>,
        table: &[[u32; 32]; crate::program::STIPPLE_LEVELS],
    ) {
        selection.fades.clear();
        for usage in selection.materials.values_mut() {
            usage.stippled = false;
        }
        for &(material, instance) in &selection.vertices {
            let Some(&level) = instance.and_then(|n| fades.get(&n)) else { continue };
            assert!(
                usize::from(level) < crate::program::STIPPLE_LEVELS,
                "native world: stipple pattern {level} is past the table"
            );
            selection.fades.insert(instance.expect("matched above"), level);
            if let Some(usage) = selection.materials.get_mut(&material) {
                usage.stippled = true;
            }
        }
        selection.stipple = (!selection.fades.is_empty()).then(|| Box::new(*table));
    }

    /// The display-only parts this world can draw whole: every part of an
    /// owner (a cluster or a placement) must be loaded and read no texture
    /// accumulation target, or none of its parts is kept. A display-only
    /// stage has no layer 1 of its own, so the accumulation target holds
    /// nothing for those pixels, and a section drawn without some of its
    /// parts is drawn half.
    pub fn display_complete_parts(&self, parts: &[(Owner, u8)]) -> Vec<(Owner, u8)> {
        complete_owners(parts, |part| {
            let &slot = self.part_slots.get(part)?;
            let material = self.world.draws[slot].material;
            Some(!self.programs.as_ref().is_some_and(|p| p.program(material).is_some_and(|p| p.needs_texaccum())))
        })
    }

    /// Bind the live target descriptor only for selections whose sampled stages or
    /// vertex constants use it. A missing descriptor prevents their reservation.
    pub fn select_texaccum(&self, selection: &mut WorldSelection, target: Option<crate::world_layers::Texaccum>) -> bool {
        let needed = self.programs.as_ref().is_some_and(|p| selection.materials.keys()
            .any(|&m| p.program(m).is_some_and(|p| p.needs_texaccum())));
        selection.texaccum = target.filter(|_| needed);
        !needed || selection.texaccum.is_some()
    }

    /// Check bindings on this attachment pair before the guest parts can be suppressed.
    pub fn prepare_selection(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, selection: &WorldSelection,
    ) -> bool {
        self.selection_ready(device, color, depth, selection, PipelineUse::Direct)
    }

    fn selection_ready(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat,
        selection: &WorldSelection, usage: PipelineUse,
    ) -> bool {
        let Some(p) = &mut self.programs else {
            return self.fixed.prepare(device, color, depth, None, None, &wgpu::StencilState::default(), usage);
        };
        let state = &self.stencil.state;
        p.selected_pipelines(device, color, depth, state, &selection.materials, usage)
            && selection.spans.iter().all(|&(batch, _, _)| p.lit_batch_binding(batch, crate::lighting::Lighting::Retail).is_some())
            && (selection.texaccum.is_some() || !selection.materials.keys().any(|&m| p.program(m).is_some_and(|p| p.needs_texaccum())))
            && selection.vertices.iter().all(|&(material, instance)| match instance {
                Some(n) => p.rigid(material, n, color, depth, state).is_some(),
                None => p.cluster(material, color, depth, state).is_some(),
            })
    }

    /// Samples the selection's animated layer-3 materials at `time` (the
    /// render clock, [`crate::layer_pass::render_time`]) into it: its
    /// writes then upload their registers at that clock
    /// ([`crate::program::MaterialProgram::animation`]). An error is how
    /// many refused (the clock did not read); the view is then the D3D
    /// path's.
    pub fn animate_selection(&self, selection: &mut WorldSelection, time: Option<f64>) -> Result<(), usize> {
        self.programs.as_ref().map_or(Ok(()), |p| p.animate(selection, time))
    }

    /// Both arms have bindings for all sampled layer-3 pages.
    pub fn has_relight(&self) -> bool {
        self.programs.as_ref().is_some_and(ProgramSet::has_relight)
    }

    /// Selected draws sampling a reconstructed page whose two arms differ.
    pub fn relit_lightmap_ranges(&self, selection: &WorldSelection) -> usize {
        let Some(programs) = self.programs.as_ref() else { return 0 };
        selection.spans.iter().filter(|(batch, _, _)| programs.changed_lightmap_batch(*batch))
            .map(|(_, first, last)| last - first).sum()
    }

    /// A live change to the geometry, lightmap or materials invalidates an offline bake.
    pub fn discard_relight(&mut self) -> bool {
        self.programs.as_mut().is_some_and(ProgramSet::discard_relight)
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
        if let Some(p) = &mut self.programs {
            p.pipelines(device, color, depth, &self.stencil.state, self.inherited_cull);
        } else {
            self.fixed.prepare(device, color, depth, None, None, &wgpu::StencilState::default(), PipelineUse::Direct);
        }
    }

    /// The same visibility test for guest admission and indirect arguments.
    /// Material programs supply the depth range. The fixed stage uses the camera's range.
    pub fn draw_frustum(vp: &crate::camera::Mat4, programs: bool) -> Frustum {
        if programs { Frustum::sides(vp) } else { Frustum::from_view_projection(vp) }
    }

    /// The frame's view for the whole-world pass ([`GpuWorld::encode`],
    /// next in the same encoder, with the same `viewport`): writes the
    /// uniform and the view the GPU culls against, and records the early
    /// cull ([`crate::cull_gpu`]). The fixed stage's views take the depth
    /// pyramid; a world with material programs culls by the frustum alone
    /// (counted, [`GpuWorld::occlusion_skips`]).
    pub fn prepare(
        &mut self, uploads: &mut gfx::ring::EncoderUploads<'_>, camera: &RenderCamera, viewport: PassViewport,
        depth: DepthRule,
    ) {
        let vp = camera.view_projection();
        let frustum = Self::draw_frustum(&vp, self.has_programs());
        self.write_frame(uploads, camera, depth);
        let occlusion = !self.has_programs();
        self.cull.early(uploads, &vp, &frustum, depth, camera.far, viewport, occlusion);
    }

    /// The last view's cull counts, read back from the GPU (blocking; for
    /// tests and tools).
    pub fn read_cull_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> CullStats {
        self.cull.read_stats(device, queue)
    }

    /// Whether each draw ([`World::draws`]) was drawn in the last view,
    /// read back from the GPU (blocking; for tests and tools).
    pub fn read_drawn(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<bool> {
        self.cull.read_drawn(device, queue)
    }

    /// Views whose late cull ran without the depth pyramid, by why.
    pub fn occlusion_skips(&self) -> OcclusionSkips {
        self.cull.skips()
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
        let spans: Vec<_> = self.world.batches.iter().enumerate().map(|(i, b)| {
            let first = b.first_draw as usize;
            (i, first, first + b.draw_count as usize)
        }).collect();
        self.prepare_pipelines(device, color_format, depth_format);
        self.record_spans(encoder, color, color_format, depth, depth_format, viewport, target_size, &spans, Slots::Early, crate::lighting::Lighting::Retail);
        if self.cull.late(device, encoder, depth, viewport) {
            self.record_spans(encoder, color, color_format, depth, depth_format, viewport, target_size, &spans, Slots::Late, crate::lighting::Lighting::Retail);
        }
    }

    /// All selected draws passed the owner and frustum checks before admission.
    /// Their index ranges are immutable and can be issued directly.
    pub fn encode_selection(
        &mut self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView, depth_format: wgpu::TextureFormat, viewport: PassViewport, target_size: (u32, u32),
        selection: &WorldSelection,
    ) -> usize {
        self.encode_selection_lit(encoder, color, color_format, depth, depth_format, viewport, target_size,
            selection, crate::lighting::Lighting::Retail)
    }

    pub fn encode_selection_lit(
        &mut self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView, depth_format: wgpu::TextureFormat, viewport: PassViewport, target_size: (u32, u32),
        selection: &WorldSelection, lighting: crate::lighting::Lighting,
    ) -> usize {
        self.record_spans(encoder, color, color_format, depth, depth_format, viewport, target_size, &selection.spans, Slots::Selected, lighting)
    }

    /// [`GpuWorld::write_selection`] and [`GpuWorld::encode_selection`] as
    /// packets for the D3D pass already open on these attachments: the
    /// view's rows written ahead of them, the same pipelines, groups,
    /// stencil references and index ranges, adjacent triangle lists under
    /// one binding joined as there. Material programs only (the fixed stage
    /// reads its frame and indirect buffers in draw order). Refused, rather
    /// than leaving batches out, when a program cannot be bound.
    pub fn selection_draws(
        &mut self, color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        selection: &WorldSelection, view: &ViewInputs,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        self.selection_draws_lit(color_format, depth_format, target_size, selection, view, crate::lighting::Lighting::Retail)
    }

    pub fn selection_draws_lit(
        &mut self, color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        selection: &WorldSelection, view: &ViewInputs, lighting: crate::lighting::Lighting,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        self.selection_draws_bound(color_format, depth_format, target_size, selection, view, lighting, None)
    }

    pub fn selection_draws_bound(
        &mut self, color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        selection: &WorldSelection, view: &ViewInputs, lighting: crate::lighting::Lighting,
        accumulation: Option<(&wgpu::Device, &wgpu::TextureView)>,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        self.selection_packets(color_format, depth_format, target_size, selection, view, lighting, accumulation, None)
            .map(|(draws, ..)| draws)
    }

    /// A game view's selected parts culled on the GPU (docs/graphics/
    /// native-world.md, "Cull"): the packets draw the selection's slots of
    /// the guest cull's indirect buffers, one `multi_draw_indexed_indirect`
    /// per run of consecutive slots under one binding, in the selection's
    /// order. [`GuestDispatch::record`] writes the early slots from the
    /// frustum of `camera` (`Frustum::sides`) and the selection as the
    /// start set. With `occlusion` (the depth target the draws test, which
    /// the pyramid must read, and the copy's lens and rectangle) the early
    /// set is what the last view kept, and [`GuestDispatch::record_late`]
    /// tests the rest against the pyramid of the early run's depth (the
    /// programs' [`DepthModel`], the lens's [`ScreenMap`]) for the late
    /// packets. A binding whose order matters
    /// ([`keeps_order`]) has no late packets: the frustum alone culls it,
    /// in the early run. The caller records `record`, the early packets,
    /// `record_late`, then the late packets, in one encoder.
    #[allow(clippy::too_many_arguments)]
    pub fn culled_selection_draws(
        &mut self, uploads: &gfx::ring::SubmissionUploads, device: &wgpu::Device, queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        selection: &WorldSelection, view: &ViewInputs, camera: &RenderCamera, lighting: crate::lighting::Lighting,
        accumulation: Option<(&wgpu::Device, &wgpu::TextureView)>,
        occlusion: Option<(&wgpu::Texture, &RenderCamera, PassViewport, u64)>,
    ) -> Result<CulledSelection, &'static str> {
        let slots = (self.guest_cull.early.clone(), self.guest_cull.late.clone());
        let (draws, late, start) = self.selection_packets(color_format, depth_format, target_size, selection, view,
            lighting, accumulation, Some(slots))?;
        let vp = camera.view_projection();
        // The copy's lens into its rectangle; a scaled copy's sample shift
        // within one guest pixel of the margin.
        let scale = target_size.0 as f32 / view.guest_size.0.max(1) as f32;
        let occlusion = occlusion.map(|(t, lens, viewport, token)| {
            let map = ScreenMap::viewport(&lens.view_projection(), viewport).with_margin(2.0 + scale);
            (DepthModel::program(view), map, t, token)
        });
        let dispatch = self.guest_cull.guest_dispatch(device, queue, uploads, &vp, &Frustum::sides(&vp), &start, occlusion);
        let late = if dispatch.has_late() { late } else { Vec::new() };
        Ok(CulledSelection { draws, late, dispatch })
    }

    /// Views of a game whose guest cull ran without the pyramid, by why.
    pub fn guest_occlusion_skips(&self) -> OcclusionSkips {
        self.guest_cull.skips()
    }

    /// A view's decorators culled on the GPU (docs/graphics/
    /// native-decorators.md, "Cull"): each instance by its placement
    /// group's sphere against `Frustum::sides` of `camera` and, for a
    /// batch whose pipeline draws nothing where hidden (`occludable`,
    /// [`depth_hides`]), against the pyramid this view's layer 3 built on
    /// the target (`occlusion`: the depth target, the copy's lens and
    /// rectangle and the view's token, as [`GpuWorld::culled_layer_draws`]
    /// reads it). The caller records the dispatch, then draws each batch
    /// from its slot over the compacted instances.
    #[allow(clippy::too_many_arguments)]
    pub fn culled_decorators(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads,
        plan: &crate::decorators::Plan, occludable: &[bool], camera: &RenderCamera, view: &ViewInputs,
        target_size: (u32, u32), occlusion: Option<(&wgpu::Texture, &RenderCamera, PassViewport, u64)>,
    ) -> crate::cull_gpu::DecoratorDispatch {
        let scale = target_size.0 as f32 / view.guest_size.0.max(1) as f32;
        let occlusion = occlusion.and_then(|(depth, lens, viewport, token)| {
            let map = ScreenMap::viewport(&lens.view_projection(), viewport).with_margin(2.0 + scale);
            let model = DepthModel::program(view);
            let served = self.guest_cull.served_pyramid(depth, token, &map, &model)?;
            Some((model, map, served))
        });
        let vp = camera.view_projection();
        let cull = self.decorator_cull.get_or_insert_with(|| crate::cull_gpu::DecoratorCull::new(device));
        cull.dispatch(device, queue, uploads, plan, occludable, &vp, &Frustum::sides(&vp), occlusion)
    }

    /// What the decorators' cull could not test, counted.
    pub fn decorator_skips(&self) -> crate::cull_gpu::DecoratorSkips {
        self.decorator_cull.as_ref().map(|c| c.skips).unwrap_or_default()
    }

    /// Later layers' views culled without a game view's pyramid
    /// (`layer_view`).
    pub fn layer_occlusion_skips(&self) -> OcclusionSkips {
        self.layer_cull.skips()
    }

    /// The last later layer's cull counts, read back (blocking): for tests
    /// and tools.
    pub fn read_layer_cull_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> CullStats {
        self.layer_cull.read_stats(device, queue)
    }

    /// Whether each draw was drawn by the last later layer's cull.
    pub fn read_layer_drawn(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<bool> {
        self.layer_cull.read_drawn(device, queue)
    }

    /// The last game view's guest cull counts (`visible` is what its
    /// frustum kept of the selection), read back (blocking): for tests and
    /// tools.
    pub fn read_guest_cull_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> CullStats {
        self.guest_cull.read_stats(device, queue)
    }

    /// Whether each draw was drawn by the last game view's guest cull.
    pub fn read_guest_drawn(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<bool> {
        self.guest_cull.read_drawn(device, queue)
    }

    #[allow(clippy::too_many_arguments)]
    fn selection_packets(
        &mut self, color_format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat, target_size: (u32, u32),
        selection: &WorldSelection, view: &ViewInputs, lighting: crate::lighting::Lighting,
        accumulation: Option<(&wgpu::Device, &wgpu::TextureView)>, culled: Option<(wgpu::Buffer, wgpu::Buffer)>,
    ) -> Result<(gfx::external::ExternalDraws, Vec<gfx::RecordedDraw>, Vec<u32>), &'static str> {
        use crate::program::BatchTextures;
        use gfx::{BoundGroup, RecordedDraw, ScissorRect};
        let Self { programs, packet_pipelines, world, stencil, program_vertices, indices, .. } = self;
        let p = programs.as_ref().ok_or("world programs unavailable")?;
        let has_stencil = depth_format.has_stencil_aspect();
        let scissor = ScissorRect::full(target_size.0, target_size.1);
        // Retired programs' pipelines (a map change) leave with the world;
        // bound what a long session of program swaps could hold.
        if packet_pipelines.len() > 4096 {
            packet_pipelines.clear();
        }
        struct Bound {
            key: gfx::pipeline::PipelineKey,
            pipeline: std::sync::Arc<gfx::compile::PendingPipeline>,
            groups: Vec<BoundGroup>,
            state: gfx::state::RenderState,
            ordered: bool,
        }
        let mut packets = Vec::new();
        let mut late = Vec::new();
        let mut start = vec![0u32; if culled.is_some() { world.draws.len() } else { 0 }];
        let mut ranges = 0;
        let mut bound: Option<((u16, wgpu::BindGroup, Option<u32>, u32, Option<u8>), Bound)> = None;
        let mut live_groups = HashMap::new();
        let mut pending: Option<std::ops::Range<u32>> = None;
        // `range`: indices, or with `culled` the slots of a run.
        let emit = |packets: &mut Vec<RecordedDraw>, late: &mut Vec<RecordedDraw>, b: &Bound, range: std::ops::Range<u32>| {
            let count = range.end - range.start;
            let indexed = |at: u64, len: u64, n: u32| RecordedDraw::prepared_indexed(
                &b.key,
                b.pipeline.clone(),
                vec![(program_vertices.clone(), 0, program_vertices.size())],
                (indices.clone(), at, len),
                n,
                b.groups.clone(),
                &b.state,
                target_size,
                scissor,
            );
            match &culled {
                None => packets.push(indexed(u64::from(range.start) * 4, u64::from(count) * 4, count)),
                Some((early, late_slots)) => {
                    let at = u64::from(range.start) * SLOT_SIZE;
                    packets.push(indexed(0, indices.size(), 0).with_indirect(early.clone(), at, count));
                    if !b.ordered {
                        late.push(indexed(0, indices.size(), 0).with_indirect(late_slots.clone(), at, count));
                    }
                }
            }
        };
        for &(i, first, last) in &selection.spans {
            let batch = &world.batches[i];
            let textures: &wgpu::BindGroup = match p.lit_batch_binding(i, lighting).ok_or("world batch textures unavailable")? {
                BatchTextures::Resident(group) => group,
                BatchTextures::Accumulation { page } => {
                    let (device, target) = accumulation.ok_or("world accumulation target unavailable")?;
                    match live_groups.entry((batch.material, batch.lightmap)) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
                            p.accumulation_group(device, batch.material, page.as_ref(), target)
                                .ok_or("world accumulation binding unavailable")?),
                    }
                }
            };
            let draws = &world.draws[first..last];
            let mut k = 0;
            while k < draws.len() {
                let owner = draws[k].owner;
                let reference = stencil.reference_of(owner);
                let same = |d: &WorldDraw| match owner {
                    Owner::Cluster(_) => matches!(d.owner, Owner::Cluster(_)) && stencil.reference_of(d.owner) == reference,
                    Owner::Instance(_) => d.owner == owner,
                };
                let end = k + draws[k..].iter().take_while(|d| same(d)).count();
                let instance = match owner {
                    Owner::Cluster(_) => None,
                    Owner::Instance(n) => Some(n),
                };
                // A faded placement draws with its program's stipple variant.
                let level = instance.and_then(|n| selection.fade_level(n));
                let binding = (batch.material, textures.clone(), instance, reference, level);
                if bound.as_ref().is_none_or(|(at, _)| *at != binding) {
                    if let (Some(range), Some((_, b))) = (pending.take(), &bound) {
                        emit(&mut packets, &mut late, b, range);
                    }
                    let (pipeline, uniforms, offsets) = match instance {
                        None => {
                            let (pipeline, uniforms) = p.cluster(batch.material, color_format, depth_format, &stencil.state)
                                .ok_or("world program unbound")?;
                            (pipeline, uniforms, Vec::new())
                        }
                        Some(n) => match level {
                            None => {
                                let (pipeline, slots, offset) = p.rigid(batch.material, n, color_format, depth_format, &stencil.state)
                                    .ok_or("world rigid program unbound")?;
                                (pipeline, slots, vec![offset])
                            }
                            Some(level) => {
                                let (pipeline, slots, offsets) = p.rigid_stipple(batch.material, n, level, color_format, depth_format, &stencil.state)
                                    .ok_or("world rigid stipple program unbound")?;
                                (pipeline, slots, offsets.to_vec())
                            }
                        },
                    };
                    let key = p.described(batch.material, instance.is_some(), color_format, depth_format, &stencil.state)
                        .ok_or("world program unbound")?;
                    let pipeline = packet_pipelines.entry(pipeline.clone())
                        .or_insert_with(|| gfx::compile::Pending::ready(Some(pipeline.clone())))
                        .clone();
                    let mut state = gfx::state::RenderState::default();
                    // Set only with a stencil aspect, as the pass sets it.
                    state.stencil.reference = if has_stencil { reference as u8 } else { 0 };
                    let ordered = keeps_order(&key) || p.offsets_depth(batch.material);
                    bound = Some((binding, Bound {
                        ordered,
                        key,
                        pipeline,
                        groups: vec![
                            BoundGroup { index: 0, group: uniforms.clone(), offsets },
                            BoundGroup::new(1, textures.clone()),
                        ],
                        state,
                    }));
                }
                for (j, d) in draws[k..end].iter().enumerate() {
                    let joined = if culled.is_some() {
                        let slot = (first + k + j) as u32;
                        let ordered = bound.as_ref().is_some_and(|(_, b)| b.ordered);
                        let value = if ordered { crate::cull_gpu::ORDERED } else { crate::cull_gpu::OCCLUDABLE };
                        start[slot as usize] = start[slot as usize].max(value);
                        append_slot(&mut pending, slot..slot + 1)
                    } else {
                        append_triangles(&mut pending, d.first_index..d.first_index + d.index_count)
                    };
                    if let Some(previous) = joined {
                        emit(&mut packets, &mut late, &bound.as_ref().expect("bound before its draws").1, previous);
                    }
                }
                ranges += end - k;
                k = end;
            }
        }
        if let (Some(range), Some((_, b))) = (pending, &bound) {
            emit(&mut packets, &mut late, b, range);
        }
        if packets.is_empty() {
            return Err("world selection draws nothing");
        }
        // Conservative: the programs' alpha tests and kill sources discard.
        let writes = p.selected_writes(view, selection);
        Ok((gfx::external::ExternalDraws { ranges, packets, discards: true, writes }, late, start))
    }

    fn record_spans(
        &mut self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView, depth_format: wgpu::TextureFormat, viewport: PassViewport, target_size: (u32, u32),
        spans: &[(usize, usize, usize)], slots: Slots, lighting: crate::lighting::Lighting,
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
        if self.world.draws.is_empty() {
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
            let indirect = match slots {
                Slots::Early => &self.cull.early,
                Slots::Late => &self.cull.late,
                Slots::Selected => {
                    for d in &self.world.draws[first..end] {
                        let next = d.first_index..d.first_index + d.index_count;
                        if let Some(previous) = append_triangles(pending, next) {
                            pass.draw_indexed(previous, 0, 0..1);
                        }
                    }
                    return;
                }
            };
            pass.multi_draw_indexed_indirect(indirect, first as u64 * SLOT_SIZE, (end - first) as u32);
        };
        for &(i, first, last) in spans {
            let b = &self.world.batches[i];
            if let Some(p) = &self.programs {
                let Some(textures) = p.lit_batch_textures(i, lighting) else {
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
                    let Some(pipeline) = self.fixed.get(color_format, depth_format, &wgpu::StencilState::default(), None) else {
                        return 0;
                    };
                    pass.set_pipeline(pipeline);
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

/// One view's draws of `set`'s selected parts in a pass of their own over
/// `color` and `depth` (loaded and kept): [`GpuWorld::encode_fog`] and
/// [`GpuWorld::encode_layer`].
fn encode_set(
    set: &mut FogSet,
    label: &str,
    program_vertices: &wgpu::Buffer,
    indices: &wgpu::Buffer,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    color_format: wgpu::TextureFormat,
    depth: &wgpu::TextureView,
    depth_format: wgpu::TextureFormat,
    target_size: (u32, u32),
    viewport: [f32; 4],
    selected: &[(Owner, u8)],
) -> usize {
    let Some([x, y, w, h]) = viewport_scissor(viewport, target_size) else { return 0 };
    set.pipelines(device, color_format, depth_format);
    let set = &*set;
    let has_stencil = depth_format.has_stencil_aspect();
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    pass.set_scissor_rect(x, y, w, h);
    pass.set_vertex_buffer(0, program_vertices.slice(..));
    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
    set.draw(&mut pass, color_format, depth_format, selected)
}


/// A game view's culled selection ([`GpuWorld::culled_selection_draws`]).
pub struct CulledSelection {
    /// The early run's packets, the view rows written ahead of them.
    pub draws: gfx::external::ExternalDraws,
    /// The late run's packets: every binding's but those that keep their
    /// order; empty when the pyramid does not run.
    pub late: Vec<gfx::RecordedDraw>,
    pub dispatch: GuestDispatch,
}

/// Whether a binding's draws must keep their place in the guest's order,
/// so the frustum alone culls them in the early run: one that blends,
/// writes no depth, tests depth other than less or equal (equal
/// included), or writes stencil when the depth test fails (a culled draw
/// would not).
pub fn keeps_order(key: &gfx::pipeline::PipelineKey) -> bool {
    use wgpu::{CompareFunction as C, StencilOperation as S};
    let Some(ds) = &key.depth_stencil else { return true };
    key.color_target.blend.is_some()
        || ds.depth_write_enabled != Some(true)
        || !matches!(ds.depth_compare, Some(C::Less | C::LessEqual))
        || ds.stencil.write_mask != 0
            && [ds.stencil.front, ds.stencil.back].iter().any(|f| f.depth_fail_op != S::Keep)
}

/// Whether a draw under this pipeline draws nothing at a fragment its
/// depth test hides: a `Less`, `LessEqual` or `Equal` test (blending or
/// not), and no stencil write when the test fails. A later layer's item
/// every pass of which holds this may be culled by a game view's pyramid
/// ([`GpuWorld::culled_layer_draws`]).
pub fn hidden_draws_nothing(key: &gfx::pipeline::PipelineKey) -> bool {
    let Some(ds) = &key.depth_stencil else { return false };
    ds.depth_compare.is_some_and(|compare| depth_hides(compare, &ds.stencil))
}

/// [`hidden_draws_nothing`] of a depth test and stencil state.
pub fn depth_hides(compare: wgpu::CompareFunction, stencil: &wgpu::StencilState) -> bool {
    use wgpu::{CompareFunction as C, StencilOperation as S};
    matches!(compare, C::Less | C::LessEqual | C::Equal)
        && !(stencil.write_mask != 0 && [stencil.front, stencil.back].iter().any(|f| f.depth_fail_op != S::Keep))
}

/// [`append_triangles`] for runs of slots: `next` joins a run it follows.
fn append_slot(pending: &mut Option<std::ops::Range<u32>>, next: std::ops::Range<u32>) -> Option<std::ops::Range<u32>> {
    if let Some(previous) = pending && previous.end == next.start {
        previous.end = next.end;
        return None;
    }
    pending.replace(next)
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
        assert_eq!(selection.materials[&2], MaterialUse { cluster: true, rigid: true, stippled: false });
        assert_eq!(selection.materials[&9], MaterialUse { cluster: false, rigid: true, stippled: false });
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


/// The parts of every owner whose parts all pass `drawable` (`None`: not
/// loaded), in their given order.
fn complete_owners(parts: &[(Owner, u8)], drawable: impl Fn(&(Owner, u8)) -> Option<bool>) -> Vec<(Owner, u8)> {
    let refused: HashSet<Owner> = parts.iter()
        .filter(|part| drawable(part) != Some(true))
        .map(|&(owner, _)| owner)
        .collect();
    parts.iter().copied().filter(|(owner, _)| !refused.contains(owner)).collect()
}

#[cfg(test)]
mod display_parts_tests {
    use super::*;

    #[test]
    fn a_display_owner_with_a_part_it_cannot_draw_is_left_out_whole() {
        let (a, b, c) = (Owner::Cluster(4), Owner::Instance(9), Owner::Cluster(6));
        let parts = [(a, 0), (b, 0), (a, 1), (b, 2), (c, 3)];
        let kept = complete_owners(&parts, |&(owner, part)| match (owner, part) {
            (o, 2) if o == b => Some(false), // samples the accumulation target
            (o, _) if o == c => None,        // not loaded
            _ => Some(true),
        });
        assert_eq!(kept, [(a, 0), (a, 1)]);
        assert!(complete_owners(&[], |_| Some(true)).is_empty());
    }
}

#[cfg(test)]
mod image_storage_tests {
    use super::*;

    #[test]
    fn image_clones_share_accounted_storage_until_edited() {
        let image = Image::solid([10, 20, 30, 255]);
        let original = Arc::downgrade(&image.pixels);
        let mut biased = image.clone();
        assert!(Arc::ptr_eq(&image.pixels, &biased.pixels));
        assert_eq!(image.pixels.bytes.get(), 4);

        biased.edit_rgba(|rgba, _| rgba[0] = 32);
        assert_eq!(image.rgba[0], 10);
        assert_eq!(biased.rgba[0], 32);
        assert_eq!(biased.pixels.bytes.get(), 4);
        drop(image);
        assert!(original.upgrade().is_none());
    }
}
