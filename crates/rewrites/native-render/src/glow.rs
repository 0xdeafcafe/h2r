//! The glow of `render_screen_effects`' colour pass (`0x000214f0`) drawn
//! natively (docs/graphics/native-post-hud.md, stage 5): the down-sample
//! into target 7, the blur ping-ponging 7 and 8, and the composite back
//! over the view's colour, each a screen quad through the stage 3
//! combiner compiler (`shaders::psh`) with the game's own 60-word blocks
//! (`engine::glow`).
//!
//! What a quad is, from the game (Code):
//!
//! - `0x00034a90` draws with the vertex shader the rasterizer globals
//!   name for screen quads (`0x0001c590`), which passes every input
//!   through (`oPos = v0`, `oT0..oT3 = v1..v4`, `oD0`, `oD1 = v5`, `v6`):
//!   the position is in the target's pixels.
//! - `0x00034420` emits one quad (`Begin(7)`, a four-vertex fan), corners `(0, 0)`,
//!   `(0, 1)`, `(1, 1)`, `(1, 0)`, each attribute from `0x00034100`.
//!   With the quad draw's tenth argument clear (all three passes), the
//!   rectangle is Halo's window `0x004b9dd0` (top, left, bottom, right
//!   shorts); the position is its lerp (`0x000350e0`) and so is each
//!   tap's base coordinate, so a fragment's coordinate is its own pixel.
//! - Tap `k` (attribute `k + 1`, while `k` is below the ninth argument)
//!   moves the coordinate by the spread (seventh argument) times a sign
//!   (`[0x0045dbbc]` for `k & 1` in x and `k & 2` in y, `[0x0045dbc4]`
//!   otherwise), then scales it by the stage's bound size over the
//!   target's (`0x00485af4`, `0x00485b04` over `0x0001dd60`): texels of
//!   the source, which the combiner fetches as a linear texture does.
//!
//! So [`Plan::quad`] gives each draw its corners and per-stage offsets
//! and scales, and [`VERTEX`] reproduces the screen shader for them; the
//! pixel side is the block itself ([`program`]). [`GpuGlow`] draws a plan
//! into the live view and blur targets, retaining pixels outside the quads.

use std::collections::HashMap;
use std::sync::Arc;

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};

use crate::camera::GuestRead;
use crate::screen_gpu::{FragmentRaster, PostRaster, RasterPipeline, STRIP_ATTRIBUTES, StripVertex, strip_ranges};

/// A grid's vertex buffer: [`StripVertex`] in order.
const STRIP_LAYOUT: [Option<wgpu::VertexBufferLayout<'static>>; 1] = [Some(wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<StripVertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &STRIP_ATTRIBUTES,
})];

/// Words in a combiner block (`D3DPIXELSHADERDEF`).
pub const COMBINER_WORDS: usize = 60;

/// Halo's window rectangle (shorts: top, left, bottom, right).
pub const WINDOW: u32 = 0x004b_9dd0;
/// The tap signs: `k & 1` (x) or `k & 2` (y) set takes the first.
pub const SIGN_SET: u32 = 0x0045_dbbc;
pub const SIGN_CLEAR: u32 = 0x0045_dbc4;
/// Halo's render-target table (`0x0001dd60`): stride `0x98`, width at
/// `+0x80`, height `+0x84`, surface `+0x88`, a byte at `+0x95` that
/// hides the size.
pub const TARGETS: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;
/// The view's target index (a word).
pub const VIEW_TARGET: u32 = 0x0048_58b8;
/// The pass's two buffers.
pub const TARGET_DOWN: u32 = 7;
pub const TARGET_OTHER: u32 = 8;

/// A surface a draw reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The view's colour target.
    View,
    /// Target 7.
    Down,
    /// Target 8.
    Other,
}

impl Surface {
    /// The surface for Halo's target `index` in this pass.
    pub fn of_target(index: u32, view: u32) -> Option<Surface> {
        match index {
            TARGET_DOWN => Some(Surface::Down),
            TARGET_OTHER => Some(Surface::Other),
            i if i == view => Some(Surface::View),
            _ => None,
        }
    }
}

/// The composite's blend (`0x000222d0`: state 59 = 1, 62 the source
/// factor, 63 the destination's, 74 `D3DBLENDOP_ADD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend {
    /// Source factor `D3DBLEND_INVDESTCOLOR` (`0x307`), else `ONE`.
    pub inverse_dest_colour: bool,
    /// Destination factor `ONE`, else `ZERO`.
    pub keep_dest: bool,
}

impl Blend {
    /// From the D3D values of states 62 and 63; `None` for others.
    pub fn of_states(source: u32, dest: u32) -> Option<Blend> {
        let inverse_dest_colour = match source {
            1 => false,
            0x307 => true,
            _ => return None,
        };
        let keep_dest = match dest {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Blend { inverse_dest_colour, keep_dest })
    }

    /// The same blend on every channel (the NV2A blends alpha with the
    /// colour's factors).
    pub fn state(self) -> wgpu::BlendState {
        let component = wgpu::BlendComponent {
            src_factor: if self.inverse_dest_colour {
                wgpu::BlendFactor::OneMinusDst
            } else {
                wgpu::BlendFactor::One
            },
            dst_factor: if self.keep_dest { wgpu::BlendFactor::One } else { wgpu::BlendFactor::Zero },
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState { color: component, alpha: component }
    }
}

/// One quad of the pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub dest: Surface,
    /// What each stage has bound (`0x00014f60`'s binds so far).
    pub stages: [Option<Surface>; 4],
    pub combiner: [u32; COMBINER_WORDS],
    /// Taps emitted (the quad draw's ninth argument).
    pub taps: u32,
    /// The quad draw's seventh argument.
    pub spread: f32,
    /// `None`: the blend the pass leaves off (the down-sample and blur).
    pub blend: Option<Blend>,
}

/// The glow of one call: where its quads are and what they draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The window rectangle `(left, top, right, bottom)` in pixels.
    pub window: [f32; 4],
    /// `[[SIGN_CLEAR], [SIGN_SET]]`.
    pub signs: [f32; 2],
    /// The view's target size and targets 7 and 8's, in guest pixels.
    pub view: (u32, u32),
    pub down: (u32, u32),
    pub other: (u32, u32),
    pub draws: Vec<Draw>,
}

/// The size `0x0001dd60` gives Halo's target `index`; `None` where it
/// gives none.
pub fn target_size(mem: &dyn GuestRead, index: u32) -> Option<(u32, u32)> {
    let at = TARGETS.checked_add(index.checked_mul(TARGET_STRIDE)?)?;
    let surface = mem.read_u32(at + 0x88)?;
    let hidden = (mem.read_u32(at + 0x94)? >> 8) & 0xff != 0;
    if surface != 0 && !hidden {
        return Some((mem.read_u32(at + 0x80)?, mem.read_u32(at + 0x84)?));
    }
    matches!(index, 0 | 3 | 0x18).then_some((640, 480))
}

/// What the quads share, read at the call: the window, the tap signs and
/// the sizes (`None` when a size is missing or zero).
/// The draws are the caller's ([`Plan::draws`]).
pub fn read_frame(mem: &dyn GuestRead) -> Option<Plan> {
    let (window, signs) = read_window(mem)?;
    let view_index = mem.read_u32(VIEW_TARGET)? & 0xffff;
    let view = target_size(mem, view_index as u16 as i16 as i32 as u32)?;
    let down = target_size(mem, TARGET_DOWN)?;
    let other = target_size(mem, TARGET_OTHER)?;
    if [view.0, view.1, down.0, down.1, other.0, other.1].contains(&0) {
        return None;
    }
    Some(Plan { window, signs, view, down, other, draws: Vec::new() })
}

/// The window `(left, top, right, bottom)` and the tap signs
/// `[[SIGN_CLEAR], [SIGN_SET]]`, as the quad draw reads them
/// (`0x00034b0a`).
pub fn read_window(mem: &dyn GuestRead) -> Option<([f32; 4], [f32; 2])> {
    let half = |a: u32| -> Option<f32> {
        let w = mem.read_u32(a & !3)?;
        let h = if a & 2 != 0 { w >> 16 } else { w & 0xffff };
        Some(f32::from(h as u16 as i16))
    };
    let window = [half(WINDOW + 2)?, half(WINDOW)?, half(WINDOW + 6)?, half(WINDOW + 4)?];
    let signs = [f32::from_bits(mem.read_u32(SIGN_CLEAR)?), f32::from_bits(mem.read_u32(SIGN_SET)?)];
    Some((window, signs))
}

/// One quad's vertex uniforms ([`VERTEX`]'s `GlowQuad`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    /// The corners `(x0, y0, x1, y1)` in the target's pixels.
    pub corners: [f32; 4],
    /// `2 / width`, `2 / height` of the target (guest pixels), then the
    /// quad's depth (`[0, 1]`, what a [`QuadDepth`] test compares) and the
    /// alpha of `v0` (the quad's diffuse; its colour is 0). The glow's
    /// quads leave both 0.
    pub dest: [f32; 4],
    /// Per stage: the tap's offset in the target's pixels (xy) and the
    /// scale to the source's texels (zw).
    pub taps: [[f32; 4]; 4],
}

impl Plan {
    /// A surface's size in guest pixels.
    pub fn size(&self, s: Surface) -> (u32, u32) {
        match s {
            Surface::View => self.view,
            Surface::Down => self.down,
            Surface::Other => self.other,
        }
    }

    /// `draw`'s vertex uniforms: `0x00034100`'s taps for its stages.
    pub fn quad(&self, draw: &Draw) -> Quad {
        let stages = draw.stages.map(|s| s.map(|s| self.size(s)));
        quad(self.window, self.signs, self.size(draw.dest), stages, draw.taps, draw.spread)
    }
}

/// The vertex uniforms of a quad `0x00034a90` draws with its tenth
/// argument clear into a target `dest` guest pixels: corners the window
/// `(left, top, right, bottom)` (clipped to the target), `taps` taps of
/// `spread` (the signs `[[SIGN_CLEAR], [SIGN_SET]]`, `k & 1` in x and
/// `k & 2` in y), each stage's coordinates scaled by its bound size
/// (`stages[k]`; `None` the target's) over the target's
/// (`0x00034bf7`..`0x00034c27`).
pub fn quad(window: [f32; 4], signs: [f32; 2], dest: (u32, u32), stages: [Option<(u32, u32)>; 4], taps: u32, spread: f32) -> Quad {
    let (dw, dh) = (dest.0 as f32, dest.1 as f32);
    let taps = std::array::from_fn(|k| {
        let s = stages[k].unwrap_or(dest);
        let (sx, sy) = if (k as u32) < taps {
            (signs[usize::from(k & 1 != 0)], signs[usize::from(k & 2 != 0)])
        } else {
            (0.0, 0.0)
        };
        [spread * sx, spread * sy, s.0 as f32 / dw, s.1 as f32 / dh]
    });
    Quad {
        corners: window,
        dest: [2.0 / dw, 2.0 / dh, 0.0, 0.0],
        taps,
    }
}

/// The screen shader for a [`Quad`]: corner `i` of a four-vertex strip,
/// its position in the target's pixels to clip space, and each tap's
/// coordinate in the source's texels (the combiner's `tex_scale` makes
/// them normalised, as for the D3D path's linear textures).
pub const VERTEX: &str = r#"
struct GlowQuad {
    corners: vec4f,
    dest: vec4f,
    taps: array<vec4f, 4>,
}
@group(0) @binding(0) var<uniform> glow: GlowQuad;

fn glow_tap(p: vec2f, t: vec4f) -> vec4f {
    return vec4f((p + t.xy) * t.zw, 0.0, 1.0);
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> XboxVaryings {
    // BL, BR, TL, TR: the strip has the fan's winding and TL–BR diagonal.
    let k = vec2f(f32(i & 1u), f32(1u - (i >> 1u)));
    let p = mix(glow.corners.xy, glow.corners.zw, k);
    var o: XboxVaryings;
    o.position = vec4f(p.x * glow.dest.x - 1.0, 1.0 - p.y * glow.dest.y, glow.dest.z, 1.0);
    o.d0 = vec4f(0.0, 0.0, 0.0, glow.dest.w);
    o.d1 = vec4f(0.0);
    o.b0 = vec4f(0.0);
    o.b1 = vec4f(0.0);
    o.t0 = glow_tap(p, glow.taps[0]);
    o.t1 = glow_tap(p, glow.taps[1]);
    o.t2 = glow_tap(p, glow.taps[2]);
    o.t3 = glow_tap(p, glow.taps[3]);
    o.fog = 1.0;
    return o;
}
"#;

/// The same screen shader for the strips of a grid (`0x00034d90`, a wide
/// convolution's draw; `engine::screen_grid`): the vertex shader
/// `0x0001c590` binds for both draws passes its inputs through, so each
/// vertex is attribute 0 the position in the target's pixels and 1..4
/// the taps in the source's texels (`screen_gpu::StripVertex`), the
/// uniform `2 / width`, `2 / height` of the target. z and w are the
/// depth's, as the quad's are; with the depth test off and w the same at
/// every vertex they change nothing, so the position goes out at z 0 and
/// w 1 as [`VERTEX`]'s does.
pub const GRID_VERTEX: &str = r#"
struct GlowGrid {
    dest: vec4f,
}
@group(0) @binding(0) var<uniform> glow: GlowGrid;

struct GridIn {
    @location(0) position: vec4f,
    @location(1) t0: vec4f,
    @location(2) t1: vec4f,
    @location(3) t2: vec4f,
    @location(4) t3: vec4f,
}

@vertex
fn vs_main(v: GridIn) -> XboxVaryings {
    var o: XboxVaryings;
    o.position = vec4f(v.position.x * glow.dest.x - 1.0, 1.0 - v.position.y * glow.dest.y, glow.dest.z, 1.0);
    o.d0 = vec4f(0.0);
    o.d1 = vec4f(0.0);
    o.b0 = vec4f(0.0);
    o.b1 = vec4f(0.0);
    o.t0 = v.t0;
    o.t1 = v.t1;
    o.t2 = v.t2;
    o.t3 = v.t3;
    o.fog = 1.0;
    return o;
}
"#;

/// A grid's vertex uniform ([`GRID_VERTEX`]'s `GlowGrid`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridFrame {
    /// `2 / width`, `2 / height` of the target (guest pixels), then the
    /// strips' depth (`[0, 1]`, what a [`QuadDepth`] test compares; 0 in
    /// [`GridFrame::of`]).
    pub dest: [f32; 4],
}

impl GridFrame {
    pub fn of(dest: (u32, u32)) -> Self {
        Self { dest: [2.0 / dest.0.max(1) as f32, 2.0 / dest.1.max(1) as f32, 0.0, 0.0] }
    }
}

/// A block compiled for the pass.
#[derive(Clone, Debug)]
pub struct Program {
    /// `vs_main` and `fs_main`.
    pub wgsl: Arc<str>,
    /// What each stage samples (`None`: nothing).
    pub dims: [Option<TextureDim>; 4],
    pub pixel: PixelUniforms,
    /// Drawn as a grid's strips ([`GRID_VERTEX`]), not a quad.
    pub grid: bool,
}

/// `combiner` through the stage 3 compiler with `stages` bound, each a
/// linear texture of that size (`tex_scale` one over it), fog off; or why
/// it cannot be drawn.
pub fn program(combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
    translate(combiner, stages, false, FragmentRaster::default())
}

/// [`program`] for a grid's strips ([`GRID_VERTEX`]).
pub fn grid_program(combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
    translate(combiner, stages, true, FragmentRaster::default())
}

fn translate(
    combiner: &[u32; COMBINER_WORDS],
    stages: [Option<(u32, u32)>; 4],
    grid: bool,
    fragment: FragmentRaster,
) -> Result<Program, String> {
    let def = PixelShaderDef::from_words(combiner);
    let options = PixelOptions {
        texture_dims: stages.map(|s| s.map(|_| TextureDim::D2)),
        alpha_test: fragment.alpha,
        stipple: fragment.stipple,
        ..Default::default()
    };
    let ps = psh::translate(&def, &options);
    if !ps.warnings.is_empty() {
        return Err(format!("combiner: {}", ps.warnings.join("; ")));
    }
    for (n, d) in ps.textures.iter().enumerate() {
        match (d, stages[n]) {
            (None, _) | (Some(TextureDim::D2), Some(_)) => {}
            (Some(_), None) => return Err(format!("stage {n} is sampled with nothing bound")),
            (Some(d), Some(_)) => return Err(format!("stage {n} is fetched as {d:?}")),
        }
    }
    let (c0, c1) = def.embedded_constants();
    let mut pixel = PixelUniforms {
        c0,
        c1,
        fog: psh::fog::Fog::OFF.uniform(),
        ..Default::default()
    };
    for (n, s) in stages.iter().enumerate() {
        if let Some((w, h)) = s {
            pixel.tex_scale[n] = [1.0 / *w as f32, 1.0 / *h as f32, 1.0, 1.0];
        }
    }
    Ok(Program {
        wgsl: format!("{}{}", ps.module(), if grid { GRID_VERTEX } else { VERTEX }).into(),
        dims: ps.textures,
        pixel,
        grid,
    })
}

/// Every draw's program, in order; or the first reason one cannot be.
pub fn programs(plan: &Plan) -> Result<Vec<Program>, String> {
    plan.draws
        .iter()
        .map(|d| program(&d.combiner, d.stages.map(|s| s.map(|s| plan.size(s)))))
        .collect()
}

/// The sampler of every stage the pass binds: the words `0x000214f0`
/// and `0x000211a0` store (address 3, clamp; filters 2, linear; no mip).
pub fn sampler_descriptor() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("native glow"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        lod_max_clamp: 0.0,
        ..Default::default()
    }
}

struct Compiled {
    module: wgpu::ShaderModule,
    uniforms_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<(RasterPipeline, Option<DepthKey>), wgpu::RenderPipeline>,
}

/// A tested quad's depth format and comparison.
type DepthKey = (wgpu::TextureFormat, wgpu::CompareFunction);

/// A depth test for one quad (`QuadDrawer::prepare_tested`): `view` the
/// depth attachment, loaded and kept, never written; the quad's
/// fragments at its depth ([`Quad::dest`]'s z) pass where `compare`
/// holds against the stored depth.
#[derive(Clone, Copy)]
pub struct QuadDepth<'a> {
    pub view: &'a wgpu::TextureView,
    pub compare: wgpu::CompareFunction,
}

/// One attachment of a post pass. Fixed-size blur levels may have a
/// different guest-to-host ratio from the framebuffer they filter.
#[derive(Clone, Copy)]
pub struct PostSurface<'a> {
    pub view: &'a wgpu::TextureView,
    pub guest_size: (u32, u32),
}

impl<'a> From<&'a d3d::ExternalColor> for PostSurface<'a> {
    fn from(color: &'a d3d::ExternalColor) -> Self {
        Self { view: &color.attachment, guest_size: color.guest_size }
    }
}

/// What [`GpuGlow::prepare`] draws into: the view's colour (a sampled,
/// drawable texture) and its sizes, and the submission's arena its
/// uniforms go in.
pub struct GlowTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub uploads: &'a gfx::ring::SubmissionUploads,
    pub color: PostSurface<'a>,
    /// Resolved inputs of the downsample, including any XRGB alpha fixup.
    pub entry_sources: [Option<&'a wgpu::TextureView>; 4],
    pub intermediates: [Option<PostSurface<'a>>; 2],
}

impl From<Option<Blend>> for PostRaster {
    fn from(blend: Option<Blend>) -> Self {
        Self { blend: blend.map(Blend::state), ..Self::default() }
    }
}

/// Quads drawn through combiner blocks: each block translated once by its
/// code ([`program_key`]), each translation compiled once by its WGSL,
/// and the sampler every stage of these passes binds.
#[derive(Default)]
pub struct QuadDrawer {
    programs: HashMap<(ProgramKey, bool, FragmentRaster), Program>,
    compiled: HashMap<Arc<str>, Compiled>,
    sampler: Option<wgpu::Sampler>,
}

impl QuadDrawer {
    const PROGRAMS: usize = 256;
    const PIPELINES_PER_PROGRAM: usize = 128;

    pub fn new() -> Self {
        Self::default()
    }

    /// `combiner`'s program with `stages` bound ([`program`]), its
    /// translation shared with every block of the same code.
    pub fn program(&mut self, combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
        self.cached(combiner, stages, false, FragmentRaster::default())
    }

    /// [`Self::program`] for a grid's strips ([`grid_program`]).
    pub fn grid_program(&mut self, combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
        self.cached(combiner, stages, true, FragmentRaster::default())
    }

    pub(crate) fn cached(
        &mut self,
        combiner: &[u32; COMBINER_WORDS],
        stages: [Option<(u32, u32)>; 4],
        grid: bool,
        fragment: FragmentRaster,
    ) -> Result<Program, String> {
        let key = (program_key(combiner, stages), grid, fragment);
        if let Some(p) = self.programs.get(&key) {
            return Ok(with_constants(p.clone(), combiner));
        }
        let p = translate(combiner, stages, grid, fragment)?;
        if self.programs.len() >= Self::PROGRAMS {
            self.programs.clear();
        }
        self.programs.insert(key, p.clone());
        Ok(p)
    }

    /// One quad of `program` with `quad`'s vertex uniforms into `dest`
    /// (format `format`, blended by `blend`), each sampled stage `n`
    /// reading `stages[n]`, the rest of `dest` kept: its pipeline and
    /// groups made and its uniforms staged in `uploads`, for
    /// [`QuadCommand::record`].
    pub fn quad(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        program: &Program,
        quad: &Quad,
        dest: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        blend: Option<Blend>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Option<QuadCommand>, &'static str> {
        let texture = dest.texture();
        let surface = PostSurface { view: dest, guest_size: (texture.width(), texture.height()) };
        self.prepare((device, queue, uploads), program, bytemuck::bytes_of(quad), None, (surface, format, blend.into()), stages)
    }

    /// A grid's `strips` (`screen_gpu::StripVertex`, each a triangle
    /// strip) of a [`grid_program`] into `dest` (`frame` its size), as
    /// [`Self::quad`] does a quad; `None` when the strips hold no vertex
    /// (nothing to draw).
    pub fn grid(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &gfx::ring::SubmissionUploads,
        program: &Program,
        frame: &GridFrame,
        strips: &[Vec<StripVertex>],
        dest: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        blend: Option<Blend>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Option<QuadCommand>, &'static str> {
        let texture = dest.texture();
        let surface = PostSurface { view: dest, guest_size: (texture.width(), texture.height()) };
        self.grid_raster((device, queue, uploads), program, frame, strips, (surface, format, blend.into()), stages)
    }

    /// [`Self::grid`] into `dest` drawn under `raster` (a blend constant,
    /// a blend [`Blend`] cannot name), `dest`'s guest size its own.
    pub fn grid_raster(
        &mut self,
        gpu: (&wgpu::Device, &wgpu::Queue, &gfx::ring::SubmissionUploads),
        program: &Program,
        frame: &GridFrame,
        strips: &[Vec<StripVertex>],
        dest: (PostSurface<'_>, wgpu::TextureFormat, PostRaster),
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Option<QuadCommand>, &'static str> {
        self.prepare(gpu, program, bytemuck::bytes_of(frame), Some(strips), dest, stages)
    }

    pub(crate) fn prepare(
        &mut self,
        gpu: (&wgpu::Device, &wgpu::Queue, &gfx::ring::SubmissionUploads),
        program: &Program,
        vertex_uniform: &[u8],
        strips: Option<&[Vec<StripVertex>]>,
        target: (PostSurface<'_>, wgpu::TextureFormat, PostRaster),
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Option<QuadCommand>, &'static str> {
        self.prepare_tested(gpu, program, vertex_uniform, strips, target, None, stages)
    }

    /// [`Self::prepare`] with the quad's fragments tested against `depth`
    /// (`None`: untested, as `prepare`). A [`Quad`] gives the depth in
    /// its `dest` z. Refused when the depth attachment's size is not the
    /// colour's or its format holds no depth.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_tested(
        &mut self,
        (device, queue, uploads): (&wgpu::Device, &wgpu::Queue, &gfx::ring::SubmissionUploads),
        program: &Program,
        vertex_uniform: &[u8],
        strips: Option<&[Vec<StripVertex>]>,
        (dest, format, raster): (PostSurface<'_>, wgpu::TextureFormat, PostRaster),
        depth: Option<QuadDepth<'_>>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<Option<QuadCommand>, &'static str> {
        let depth_key = match depth {
            None => None,
            Some(d) => {
                let (t, c) = (d.view.texture(), dest.view.texture());
                if t.size() != c.size() || t.sample_count() != c.sample_count() || !t.format().has_depth_aspect()
                    || !t.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
                {
                    return Err("quad depth attachment");
                }
                Some((t.format(), d.compare))
            }
        };
        let sampler = self.sampler.get_or_insert_with(|| device.create_sampler(&sampler_descriptor())).clone();
        if self.compiled.len() == Self::PROGRAMS && !self.compiled.contains_key(&program.wgsl) {
            self.compiled.clear();
        }
        let c = self.compiled.entry(Arc::clone(&program.wgsl)).or_insert_with(|| compile(device, program));
        let pipeline_key = (raster.pipeline(format, device.features()), depth_key);
        let texture = dest.view.texture();
        let scissor = raster.scissor(dest.guest_size, (texture.width(), texture.height()))
            .map_err(|_| "invalid post quad scissor")?;
        if c.pipelines.len() == Self::PIPELINES_PER_PROGRAM && !c.pipelines.contains_key(&pipeline_key) {
            c.pipelines.clear();
        }
        let (module, layout) = (&c.module, &c.layout);
        let pipeline = c.pipelines.entry(pipeline_key).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native glow"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: if program.grid { &STRIP_LAYOUT[..] } else { &[] },
                },
                primitive: pipeline_key.0.primitive(),
                depth_stencil: depth_key.map(|(format, compare)| wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: raster.blend,
                        write_mask: raster.writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }).clone();
        let all: Vec<StripVertex> = strips.unwrap_or_default().iter().flatten().copied().collect();
        if strips.is_some() && all.is_empty() {
            return Ok(None);
        }
        let mut pixel = program.pixel;
        raster.pixel_uniforms(&mut pixel, dest.guest_size, (texture.width(), texture.height()));
        let pixel = pixel.to_bytes();
        let run = uploads
            .uniform_run(device, queue, &c.uniforms_layout, [vertex_uniform.len(), pixel.len()], 1, |_, v, p| {
                v.copy_from_slice(vertex_uniform);
                p.copy_from_slice(&pixel);
            })
            .ok_or("native glow uniforms unavailable")?;
        let mut entries = Vec::new();
        for (n, d) in program.dims.iter().enumerate() {
            let (Some(_), Some(view)) = (d, stages[n]) else { continue };
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native glow textures"),
            layout: &c.textures_layout,
            entries: &entries,
        });
        let (vertices, ranges) = match strips {
            None => (None, std::iter::once(0..4).collect()),
            Some(strips) => {
                let bytes: &[u8] = bytemuck::cast_slice(&all);
                let (buffer, offset) = uploads.stage(device, queue, bytes);
                (Some((buffer, offset, bytes.len() as u64)), strip_ranges(strips))
            }
        };
        Ok(Some(QuadCommand {
            blend_constant: raster.blend_constant,
            scissor,
            dest: dest.view.clone(),
            depth: depth.map(|d| d.view.clone()),
            pipeline,
            uniforms: run.group.clone(),
            offsets: run.offsets(0),
            textures,
            vertices,
            ranges,
        }))
    }
}

/// One quad's or grid's render pass ([`QuadDrawer::quad`],
/// [`QuadDrawer::grid`]): its pipeline, groups and bytes made and staged
/// when it was prepared, so recording only issues commands, the same each
/// time.
#[derive(Clone)]
pub struct QuadCommand {
    blend_constant: wgpu::Color,
    scissor: [u32; 4],
    dest: wgpu::TextureView,
    /// A tested quad's depth attachment, loaded and kept.
    depth: Option<wgpu::TextureView>,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroup,
    offsets: [u32; 2],
    textures: wgpu::BindGroup,
    /// A grid's strips in the submission's arena.
    vertices: Option<(wgpu::Buffer, u64, u64)>,
    ranges: Vec<std::ops::Range<u32>>,
}

impl QuadCommand {
    /// Records the pass into `encoder`, loading `dest`'s pixels (and a
    /// tested quad's depth and stencil, kept as they were).
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native glow"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.dest,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: self.depth.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: view.texture().format().has_stencil_aspect().then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_blend_constant(self.blend_constant);
        let [x, y, width, height] = self.scissor;
        pass.set_scissor_rect(x, y, width, height);
        pass.set_bind_group(0, &self.uniforms, &self.offsets);
        pass.set_bind_group(1, &self.textures, &[]);
        if let Some((buffer, offset, size)) = &self.vertices {
            pass.set_vertex_buffer(0, buffer.slice(*offset..*offset + *size));
        }
        for r in &self.ranges {
            pass.draw(r.clone(), 0..1);
        }
    }
}

/// Checks the actual attachment for this copy of the guest render target.
pub(crate) fn live_colour(view: &wgpu::TextureView) -> bool {
    let t = view.texture();
    t.dimension() == wgpu::TextureDimension::D2
        && t.depth_or_array_layers() == 1
        && t.sample_count() == 1
        && t.mip_level_count() == 1
        && !t.format().is_depth_stencil_format()
        && t.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
}

pub(crate) fn sampleable(device: &wgpu::Device, view: &wgpu::TextureView) -> bool {
    let t = view.texture();
    t.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
        && gfx::program::filterable(t.format(), device.features())
}

/// The GPU pipeline cache. Target views come from each live call.
#[derive(Default)]
pub struct GpuGlow {
    quads: QuadDrawer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    ViewSize((u32, u32), (u32, u32)),
    RasterCount,
    Attachment(Surface),
    Source(Surface),
    Program(String),
    Record(&'static str),
}

/// A draw's program's key: its block without its constants (uniforms,
/// not code: words 10..26 and 43, 44) and what its stages bind.
type ProgramKey = ([u32; COMBINER_WORDS], [Option<(u32, u32)>; 4]);

fn program_key(combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> ProgramKey {
    let mut code = *combiner;
    for w in (10..26).chain(43..45) {
        code[w] = 0;
    }
    (code, stages)
}

/// `combiner`'s program from the cache's one for its code, with its own
/// constants.
fn with_constants(mut p: Program, combiner: &[u32; COMBINER_WORDS]) -> Program {
    let (c0, c1) = PixelShaderDef::from_words(combiner).embedded_constants();
    p.pixel.c0 = c0;
    p.pixel.c1 = c1;
    p
}

impl GpuGlow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every quad into the exact colour attachment the D3D draw used,
    /// prepared ([`GlowCommands::record`] records them). Destination
    /// format and existing pixels come from that view. Refused before
    /// anything is recorded.
    pub fn prepare(&mut self, t: GlowTarget<'_>, plan: &Plan) -> Result<GlowCommands, Refused> {
        self.prepare_raster(t, plan, None)
    }

    /// Prepare the plan with the raster state captured from each matched quad.
    pub fn prepare_raster(&mut self, t: GlowTarget<'_>, plan: &Plan, rasters: Option<&[PostRaster]>) -> Result<GlowCommands, Refused> {
        if rasters.is_some_and(|states| states.len() != plan.draws.len()) {
            return Err(Refused::RasterCount);
        }
        if t.color.guest_size != plan.view {
            return Err(Refused::ViewSize(t.color.guest_size, plan.view));
        }
        let view_of = |surface| match surface {
            Surface::View => Some(t.color),
            Surface::Down => t.intermediates[0],
            Surface::Other => t.intermediates[1],
        };
        let mut prepared = Vec::with_capacity(plan.draws.len());
        for (index, draw) in plan.draws.iter().enumerate() {
            let dest = view_of(draw.dest).ok_or(Refused::Attachment(draw.dest))?;
            if dest.guest_size != plan.size(draw.dest) || !live_colour(dest.view) {
                return Err(Refused::Attachment(draw.dest));
            }
            let sizes = draw.stages.map(|s| s.map(|s| plan.size(s)));
            let raster = rasters.map_or_else(|| draw.blend.into(), |states| states[index]);
            let program = self.quads.cached(&draw.combiner, sizes, false, raster.fragment())
                .map_err(Refused::Program)?;
            let mut stages = [None; 4];
            for (n, stage) in stages.iter_mut().enumerate() {
                if program.dims[n].is_none() {
                    continue;
                }
                let Some(surface) = draw.stages[n] else { return Err(Refused::Program(format!("stage {n} has no source"))) };
                let entry_view = index == 0 && surface == Surface::View;
                let slot = view_of(surface).ok_or(Refused::Source(surface))?;
                let view = if entry_view { t.entry_sources[n] } else { Some(slot.view) }
                    .ok_or(Refused::Source(surface))?;
                let source = view.texture();
                if slot.guest_size != plan.size(surface)
                    || dest.view.texture() == source
                    || source.size() != slot.view.texture().size()
                    || source.sample_count() != 1
                    || (!entry_view && !live_colour(view))
                    || !sampleable(t.device, view)
                {
                    return Err(Refused::Source(surface));
                }
                *stage = Some(view);
            }
            prepared.push((dest, program, stages, raster));
        }
        let mut quads = Vec::with_capacity(prepared.len());
        for (draw, (dest, program, stages, raster)) in plan.draws.iter().zip(prepared) {
            let format = dest.view.texture().format();
            let quad = self.quads.prepare(
                (t.device, t.queue, t.uploads), &program, bytemuck::bytes_of(&plan.quad(draw)),
                None, (dest, format, raster), stages,
            );
            quads.extend(quad.map_err(Refused::Record)?);
        }
        Ok(GlowCommands { quads })
    }

    /// [`Self::prepare`], recorded into `encoder`.
    pub fn encode(&mut self, t: GlowTarget<'_>, encoder: &mut wgpu::CommandEncoder, plan: &Plan) -> Result<(), Refused> {
        self.prepare(t, plan)?.record(encoder);
        Ok(())
    }
}

/// A plan's quads ([`GpuGlow::prepare`]), one render pass each.
#[derive(Clone)]
pub struct GlowCommands {
    quads: Vec<QuadCommand>,
}

impl GlowCommands {
    /// The render passes [`Self::record`] begins.
    pub fn passes(&self) -> usize {
        self.quads.len()
    }

    /// Records the quads in order: no resource is made or written.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        for quad in &self.quads {
            quad.record(encoder);
        }
    }
}

/// The vertex and pixel uniforms, at dynamic offsets into the
/// submission's arena (`SubmissionUploads::uniform_run`).
fn uniforms_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native glow uniforms"),
        entries: &[entry(0, wgpu::ShaderStages::VERTEX), entry(1, wgpu::ShaderStages::FRAGMENT)],
    })
}

fn compile(device: &wgpu::Device, program: &Program) -> Compiled {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native glow"),
        source: wgpu::ShaderSource::Wgsl(program.wgsl.as_ref().into()),
    });
    let mut entries = Vec::new();
    for (n, d) in program.dims.iter().enumerate() {
        if d.is_none() {
            continue;
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 * n as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 * n as u32 + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native glow textures"),
        entries: &entries,
    });
    let uniforms_layout = uniforms_layout(device);
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native glow"),
        bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
        immediate_size: 0,
    });
    Compiled {
        module,
        uniforms_layout,
        layout,
        textures_layout,
        pipelines: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Words(HashMap<u32, u32>);

    impl GuestRead for Words {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    fn plan() -> Plan {
        Plan {
            window: [0.0, 0.0, 640.0, 480.0],
            signs: [-0.5, 0.5],
            view: (640, 480),
            down: (160, 120),
            other: (160, 120),
            draws: Vec::new(),
        }
    }

    fn draw(dest: Surface, source: Surface, taps: u32, spread: f32) -> Draw {
        Draw {
            dest,
            stages: [Some(source); 4],
            combiner: [0; COMBINER_WORDS],
            taps,
            spread,
            blend: None,
        }
    }

    #[test]
    fn the_down_sample_reads_a_four_by_four_box() {
        let q = plan().quad(&draw(Surface::Down, Surface::View, 4, 0.5));
        assert_eq!(q.corners, [0.0, 0.0, 640.0, 480.0], "the window: the target's pixels, clipped to it");
        assert_eq!(q.dest, [2.0 / 160.0, 2.0 / 120.0, 0.0, 0.0]);
        let offsets: Vec<[f32; 2]> = q.taps.iter().map(|t| [t[0], t[1]]).collect();
        assert_eq!(offsets, vec![[-0.25, -0.25], [0.25, -0.25], [-0.25, 0.25], [0.25, 0.25]], "k & 1 in x, k & 2 in y");
        assert!(q.taps.iter().all(|t| t[2] == 4.0 && t[3] == 4.0), "the view over target 7");
        // Pixel 10's centre: taps at 4 * (10.5 -+ 0.25) = 41 and 43, each
        // half way between two texel centres, so the four bilinear taps
        // average texels 40..43 on each axis.
        let at = |t: [f32; 4]| (10.5 + t[0]) * t[2];
        assert_eq!(at(q.taps[0]), 41.0);
        assert_eq!(at(q.taps[1]), 43.0);
    }

    #[test]
    fn the_composite_takes_one_tap_back_up_to_the_view() {
        let mut d = draw(Surface::View, Surface::Other, 1, 0.0);
        d.stages = [Some(Surface::Down), Some(Surface::Other), Some(Surface::Other), Some(Surface::Other)];
        let q = plan().quad(&d);
        assert_eq!(q.dest, [2.0 / 640.0, 2.0 / 480.0, 0.0, 0.0]);
        assert_eq!(q.taps[0], [0.0, 0.0, 0.25, 0.25]);
        assert!(q.taps[1..].iter().all(|t| t[0] == 0.0 && t[1] == 0.0), "no taps past the count");
    }

    /// A quad into any target (a screen effect's convolution): its stages'
    /// sizes over the target's.
    #[test]
    fn a_quad_into_any_target_scales_by_its_stages() {
        let q = quad([0.0, 0.0, 640.0, 480.0], [-0.5, 0.5], (320, 240), [Some((640, 480)), None, None, None], 1, 2.0);
        assert_eq!(q.corners, [0.0, 0.0, 640.0, 480.0]);
        assert_eq!(q.dest, [2.0 / 320.0, 2.0 / 240.0, 0.0, 0.0]);
        assert_eq!(q.taps[0], [-1.0, -1.0, 2.0, 2.0]);
        assert_eq!(q.taps[1], [0.0, 0.0, 1.0, 1.0], "past the taps, nothing bound: the target's own size");
        let d = draw(Surface::Down, Surface::View, 4, 0.5);
        assert_eq!(plan().quad(&d), quad(plan().window, plan().signs, (160, 120), [Some((640, 480)); 4], 4, 0.5));
    }

    /// A block's constants are uniforms: blocks differing only there share
    /// one translation, each with its own constants.
    #[test]
    fn a_program_is_cached_by_its_code() {
        let mut a = [0u32; COMBINER_WORDS];
        a[53] = 1; // one combiner stage
        let mut b = a;
        for (w, v) in [(10, 0xff10_2030), (25, 0x8000_0001), (43, 0x0102_0304), (44, 0xffff_ffff)] {
            b[w] = v;
        }
        assert_eq!(program_key(&a, [None; 4]), program_key(&b, [None; 4]));
        let mut c = a;
        c[53] = 2;
        assert_ne!(program_key(&a, [None; 4]), program_key(&c, [None; 4]), "code is the key");
        let (pa, pb) = (program(&a, [None; 4]).unwrap(), program(&b, [None; 4]).unwrap());
        assert_eq!(pa.wgsl, pb.wgsl, "constants are not code");
        let from_cache = with_constants(pa, &b);
        assert_eq!(from_cache.pixel.to_bytes(), pb.pixel.to_bytes());
    }

    #[test]
    fn blends_follow_the_states() {
        assert_eq!(Blend::of_states(1, 1), Some(Blend { inverse_dest_colour: false, keep_dest: true }));
        assert_eq!(Blend::of_states(0x307, 0), Some(Blend { inverse_dest_colour: true, keep_dest: false }));
        assert_eq!(Blend::of_states(0x302, 1), None);
        let s = Blend::of_states(0x307, 1).unwrap().state();
        assert_eq!(s.color.src_factor, wgpu::BlendFactor::OneMinusDst);
        assert_eq!(s.color.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(s.alpha, s.color, "alpha blends as the colour");
    }

    fn words(def: &PixelShaderDef) -> [u32; COMBINER_WORDS] {
        let b = def.to_bytes();
        std::array::from_fn(|i| u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]))
    }

    #[test]
    fn glow_frame_copy_samples_the_same_scene_at_full_and_three_quarter_raster() {
        use psh::bits::{CHAN_ALPHA, REG_T0, final_efg, inputs};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let copy = words(&PixelShaderDef {
            texture_modes: 1,
            final_inputs_abcd: inputs(0, 0, 0, REG_T0),
            final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
            ..Default::default()
        });
        let plan = Plan {
            window: [0.0, 0.0, 16.0, 12.0],
            signs: [0.0; 2],
            view: (16, 12),
            down: (4, 3),
            other: (2, 2),
            draws: [
                (Surface::Down, Surface::View),
                (Surface::View, Surface::Down),
            ].map(|(dest, source)| Draw {
                dest, stages: [Some(source), None, None, None], combiner: copy,
                taps: 1, spread: 0.0, blend: None,
            }).to_vec(),
        };
        for view_size in [(64, 36), (48, 27)] {
            let view = gfx::OffscreenTarget::new(&ctx.device, view_size, format);
            let small = gfx::OffscreenTarget::new(&ctx.device, plan.down, format);
            let source = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("opaque framebuffer sample"),
                size: view.texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            // Every cell has a distinct source colour at the same UV on both grids.
            let source_pixels: Vec<u8> = (0..view_size.1).flat_map(|y| (0..view_size.0).flat_map(move |x| {
                let col = x * 4 / view_size.0;
                let row = y * 3 / view_size.1;
                [48 * col as u8, 80 * row as u8, 192, 255]
            })).collect();
            ctx.queue.write_texture(
                source.as_image_copy(), &source_pixels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * view_size.0), rows_per_image: Some(view_size.1) },
                source.size(),
            );
            let sample = source.create_view(&Default::default());
            let arena = gfx::ring::UploadPool::default().begin();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            let mut glow = GpuGlow::new();
            let target = GlowTarget {
                device: &ctx.device, queue: &ctx.queue, uploads: &arena,
                color: PostSurface { view: view.target().color, guest_size: plan.view }, entry_sources: [None; 4],
                intermediates: [Some(PostSurface { view: small.target().color, guest_size: plan.down }), None],
            };
            assert_eq!(glow.encode(target, &mut encoder, &plan), Err(Refused::Source(Surface::View)));
            glow.encode(GlowTarget {
                device: &ctx.device, queue: &ctx.queue, uploads: &arena,
                color: PostSurface { view: view.target().color, guest_size: plan.view }, entry_sources: [Some(&sample), None, None, None],
                intermediates: [Some(PostSurface { view: small.target().color, guest_size: plan.down }), None],
            }, &mut encoder, &plan).unwrap();
            arena.flush(&ctx.queue);
            ctx.queue.submit([encoder.finish()]);
            // The convolution must see the same cells at either render scale.
            let expected: Vec<u8> = (0..3).flat_map(|row| (0..4).flat_map(move |col| {
                [48 * col, 80 * row, 192, 255]
            })).collect();
            assert_eq!(small.read_pixels(&ctx), expected);
            // The composite linearly filters those four columns and three rows
            // back across the whole raster. Check every pixel, including both edges.
            for (i, pixel) in view.read_pixels(&ctx).chunks_exact(4).enumerate() {
                let x = i as u32 % view_size.0;
                let y = i as u32 / view_size.0;
                let u = (x as f32 + 0.5) / view_size.0 as f32;
                let v = (y as f32 + 0.5) / view_size.1 as f32;
                let r = (48.0 * (u * 4.0 - 0.5).clamp(0.0, 3.0)).round() as u8;
                let g = (80.0 * (v * 3.0 - 0.5).clamp(0.0, 2.0)).round() as u8;
                assert!(pixel[0].abs_diff(r) <= 1 && pixel[1].abs_diff(g) <= 1, "{view_size:?} at ({x}, {y}): {pixel:?}");
                assert_eq!(pixel[2..], [192, 255]);
            }
        }
    }

    /// The prepared quads ([`GpuGlow::prepare`]) recorded into a frame's
    /// open encoder, after a dry run on a scratch encoder that is thrown
    /// away (the backend's validation), draw what the quads recorded on
    /// an encoder of their own draw: the down-sample, a blended quad
    /// between the intermediates and the composite.
    #[test]
    fn prepared_glow_recorded_twice_draws_what_its_own_encoder_draws() {
        use psh::bits::{CHAN_ALPHA, REG_T0, final_efg, inputs};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let copy = words(&PixelShaderDef {
            texture_modes: 1,
            final_inputs_abcd: inputs(0, 0, 0, REG_T0),
            final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
            ..Default::default()
        });
        let plan = Plan {
            window: [0.0, 0.0, 8.0, 8.0],
            signs: [-0.5, 0.5],
            view: (8, 8),
            down: (4, 4),
            other: (4, 4),
            draws: vec![
                Draw { dest: Surface::Down, stages: [Some(Surface::View), None, None, None], combiner: copy, taps: 1, spread: 0.0, blend: None },
                Draw {
                    dest: Surface::Other, stages: [Some(Surface::Down), None, None, None], combiner: copy, taps: 1, spread: 0.5,
                    blend: Some(Blend { inverse_dest_colour: false, keep_dest: true }),
                },
                Draw { dest: Surface::View, stages: [Some(Surface::Other), None, None, None], combiner: copy, taps: 1, spread: 0.0, blend: None },
            ],
        };
        let texture = |size: (u32, u32), usage| ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glow target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let fill = |t: &wgpu::Texture, pixels: &[u8]| ctx.queue.write_texture(
            t.as_image_copy(), pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * t.width()), rows_per_image: Some(t.height()) },
            t.size(),
        );
        let read = |t: &wgpu::Texture| gfx::readback::read_texture(&ctx, t, wgpu::TextureAspect::All, 4, (t.width(), t.height()));
        let pixels: Vec<u8> = (0..64u8).flat_map(|i| [i * 4, 255 - i * 3, i * 2 + 7, 255]).collect();
        let run = |prepared: bool, composite: Option<PostRaster>| {
            let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let (view, down, other) = (texture((8, 8), attachment), texture((4, 4), attachment), texture((4, 4), attachment));
            fill(&view, &pixels);
            fill(&other, &[10, 20, 30, 40].repeat(16));
            // The downsample's entry reads a copy of the view.
            let entry = texture((8, 8), wgpu::TextureUsages::empty());
            fill(&entry, &pixels);
            let views = [&view, &down, &other, &entry].map(|t| t.create_view(&Default::default()));
            let arena = gfx::ring::UploadPool::default().begin();
            let target = GlowTarget {
                device: &ctx.device, queue: &ctx.queue, uploads: &arena,
                color: PostSurface { view: &views[0], guest_size: plan.view }, entry_sources: [Some(&views[3]), None, None, None],
                intermediates: [(1, plan.down), (2, plan.other)].map(|(i, guest_size)| {
                    Some(PostSurface { view: &views[i], guest_size })
                }),
            };
            let mut glow = GpuGlow::new();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            if prepared {
                let mut rasters: Vec<_> = plan.draws.iter().map(|draw| PostRaster::from(draw.blend)).collect();
                if let Some(composite) = composite {
                    rasters[2] = composite;
                }
                let commands = glow.prepare_raster(target, &plan, Some(&rasters)).unwrap();
                assert_eq!(commands.passes(), 3);
                let mut scratch = ctx.device.create_command_encoder(&Default::default());
                commands.record(&mut scratch);
                drop(scratch.finish());
                commands.record(&mut encoder);
            } else {
                glow.encode(target, &mut encoder, &plan).unwrap();
            }
            arena.flush(&ctx.queue);
            ctx.queue.submit([encoder.finish()]);
            (read(&view), read(&down), read(&other))
        };
        let (own, prepared) = (run(false, None), run(true, None));
        let clip = Some([2, 1, 4, 5]);
        let raster = PostRaster { writes: wgpu::ColorWrites::RED, clip, ..Default::default() };
        let masked = run(true, Some(raster));
        for (i, ((actual, full), original)) in masked.0.chunks_exact(4)
            .zip(prepared.0.chunks_exact(4)).zip(pixels.chunks_exact(4)).enumerate()
        {
            let inside = (2..6).contains(&(i % 8)) && (1..6).contains(&(i / 8));
            assert_eq!(actual[0], if inside { full[0] } else { original[0] });
            assert_eq!(actual[1..], original[1..]);
        }
        let cull = |side| run(true, Some(PostRaster { cull: Some(side), ..raster })).0;
        let front = cull(wgpu::Face::Front);
        let back = cull(wgpu::Face::Back);
        assert!(
            (front == pixels && back == masked.0) || (back == pixels && front == masked.0),
            "the two triangles have the same facing"
        );
        let rejected = run(true, Some(PostRaster {
            alpha_test: Some((psh::AlphaFunc::Never, 0)),
            ..raster
        }));
        assert_eq!(rejected.0, pixels, "the live alpha test rejects the composite");
        let stippled = run(true, Some(PostRaster { stipple: Some([0; 32]), ..raster }));
        assert_eq!(stippled.0, pixels, "the live stipple pattern rejects the composite");
        let open_mask = run(true, Some(PostRaster { stipple: Some([u32::MAX; 32]), ..raster }));
        assert_eq!(open_mask.0, masked.0, "stipple uniforms can change without a new pipeline");
        assert_eq!(own, prepared);
        assert_ne!(&own.0[..4], &pixels[..4], "the composite drew over the view");
    }

    #[test]
    fn a_block_compiles_into_a_valid_module_with_linear_stages() {
        // Stage 0 PROJECT2D.
        let def = PixelShaderDef { texture_modes: 1, ..Default::default() };
        let p = program(&words(&def), [Some((160, 120)), None, None, None]).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(p.wgsl.contains("fn vs_main") && p.wgsl.contains("fn fs_main"));
        assert_eq!(p.dims[0], Some(TextureDim::D2));
        assert_eq!(p.pixel.tex_scale[0], [1.0 / 160.0, 1.0 / 120.0, 1.0, 1.0], "texel coordinates");
        assert_eq!(p.pixel.fog, psh::fog::Fog::OFF.uniform());
        let e = program(&words(&def), [None; 4]).unwrap_err();
        assert!(e.contains("nothing bound"), "{e}");
    }

    /// A wide convolution's grid: the same pixel side, the screen shader
    /// taking each vertex's position and taps from its buffer.
    #[test]
    fn a_grid_compiles_with_its_vertices_as_inputs() {
        let def = PixelShaderDef { texture_modes: 1, ..Default::default() };
        let stages = [Some((640, 480)), None, None, None];
        let (q, g) = (program(&words(&def), stages).unwrap(), grid_program(&words(&def), stages).unwrap());
        shaders::validate(&g.wgsl).expect("valid WGSL");
        assert!(g.grid && !q.grid);
        assert!(g.wgsl.contains("@location(4) t3") && !q.wgsl.contains("GridIn"));
        assert_eq!(g.pixel.to_bytes(), q.pixel.to_bytes(), "the pixel side is the quad's");
        assert_eq!(g.dims, q.dims);
        assert_eq!(GridFrame::of((320, 240)).dest, [2.0 / 320.0, 2.0 / 240.0, 0.0, 0.0]);
        assert_eq!(std::mem::size_of::<StripVertex>(), 5 * 16, "five vec4f, locations 0..4");
        let mut drawer = QuadDrawer::new();
        let (dq, dg) = (drawer.program(&words(&def), stages).unwrap(), drawer.grid_program(&words(&def), stages).unwrap());
        assert!(dg.grid && !dq.grid, "cached apart");
    }

    #[test]
    fn the_frame_reads_the_window_signs_and_sizes() {
        let mut m = HashMap::new();
        // Top 0, left 0, bottom 480, right 640.
        m.insert(WINDOW, 0);
        m.insert(WINDOW + 4, 480 | (640 << 16));
        m.insert(SIGN_CLEAR, (-0.5f32).to_bits());
        m.insert(SIGN_SET, 0.5f32.to_bits());
        m.insert(VIEW_TARGET, 0);
        for t in [TARGET_DOWN, TARGET_OTHER] {
            let at = TARGETS + t * TARGET_STRIDE;
            m.insert(at + 0x80, 160);
            m.insert(at + 0x84, 120);
            m.insert(at + 0x88, 0x0100_0000);
            m.insert(at + 0x94, 0);
        }
        m.insert(TARGETS + 0x88, 0);
        m.insert(TARGETS + 0x94, 0);
        let p = read_frame(&Words(m.clone())).expect("reads");
        assert_eq!(p.window, [0.0, 0.0, 640.0, 480.0]);
        assert_eq!(p.signs, [-0.5, 0.5]);
        assert_eq!(p.view, (640, 480), "target 0 without a surface: 0x0001dd60's default");
        assert_eq!(p.down, (160, 120));
        m.insert(TARGETS + TARGET_OTHER * TARGET_STRIDE + 0x80, 80);
        let p = read_frame(&Words(m.clone())).unwrap();
        let quad = p.quad(&draw(Surface::Other, Surface::Down, 4, 0.5));
        assert_eq!(quad.dest[..2], [2.0 / 80.0, 2.0 / 120.0]);
        assert_eq!(quad.taps[0][2..], [2.0, 1.0]);
        m.insert(TARGETS + TARGET_OTHER * TARGET_STRIDE + 0x80, 0);
        assert!(read_frame(&Words(m)).is_none());
    }
}
