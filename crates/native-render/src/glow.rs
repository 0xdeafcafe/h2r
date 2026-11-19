//! The glow of `render_screen_effects`' colour pass (`0x000214f0`) drawn
//! natively (docs/graphics/native-renderer.md, stage 5): the down-sample
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
//! into a copy of the view's colour's stand-ins for targets 7 and 8.

use std::collections::HashMap;

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};

use crate::camera::GuestRead;
use crate::screen_gpu::{STRIP_ATTRIBUTES, StripVertex, strip_ranges};

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
    /// Target 7's stand-in.
    Down,
    /// Target 8's stand-in.
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
    pub small: (u32, u32),
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
/// the sizes (`None` when a size is missing, zero, or 7 and 8 differ).
/// The draws are the caller's ([`Plan::draws`]).
pub fn read_frame(mem: &dyn GuestRead) -> Option<Plan> {
    let (window, signs) = read_window(mem)?;
    let view_index = mem.read_u32(VIEW_TARGET)? & 0xffff;
    let view = target_size(mem, view_index as u16 as i16 as i32 as u32)?;
    let small = target_size(mem, TARGET_DOWN)?;
    if target_size(mem, TARGET_OTHER)? != small || [view.0, view.1, small.0, small.1].contains(&0) {
        return None;
    }
    Some(Plan { window, signs, view, small, draws: Vec::new() })
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
    /// `2 / width`, `2 / height` of the target (guest pixels).
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
            Surface::Down | Surface::Other => self.small,
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
    let k = vec2f(f32(i & 1u), f32(i >> 1u));
    let p = mix(glow.corners.xy, glow.corners.zw, k);
    var o: XboxVaryings;
    o.position = vec4f(p.x * glow.dest.x - 1.0, 1.0 - p.y * glow.dest.y, 0.0, 1.0);
    o.d0 = vec4f(0.0);
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
    o.position = vec4f(v.position.x * glow.dest.x - 1.0, 1.0 - v.position.y * glow.dest.y, 0.0, 1.0);
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
    /// `2 / width`, `2 / height` of the target (guest pixels).
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
    pub wgsl: String,
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
    translate(combiner, stages, false)
}

/// [`program`] for a grid's strips ([`GRID_VERTEX`]).
pub fn grid_program(combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
    translate(combiner, stages, true)
}

fn translate(combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4], grid: bool) -> Result<Program, String> {
    let def = PixelShaderDef::from_words(combiner);
    let options = PixelOptions {
        texture_dims: stages.map(|s| s.map(|_| TextureDim::D2)),
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
        wgsl: format!("{}{}", ps.module(), if grid { GRID_VERTEX } else { VERTEX }),
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

/// The format of the stand-ins (targets 7 and 8 are
/// `D3DFMT_LIN_A8R8G8B8`).
pub const SMALL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Compiled {
    module: wgpu::ShaderModule,
    uniforms_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<(wgpu::TextureFormat, Option<Blend>), wgpu::RenderPipeline>,
}

/// What [`GpuGlow::encode`] draws into: the view's colour (a sampled,
/// drawable texture) and its sizes.
pub struct GlowTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    /// Guest pixels and this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
}

/// Quads drawn through combiner blocks: each block translated once by its
/// code ([`program_key`]), each translation compiled once by its WGSL,
/// and the sampler every stage of these passes binds.
#[derive(Default)]
pub struct QuadDrawer {
    programs: HashMap<(ProgramKey, bool), Program>,
    compiled: HashMap<String, Compiled>,
    sampler: Option<wgpu::Sampler>,
}

impl QuadDrawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `combiner`'s program with `stages` bound ([`program`]), its
    /// translation shared with every block of the same code.
    pub fn program(&mut self, combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
        self.cached(combiner, stages, false)
    }

    /// [`Self::program`] for a grid's strips ([`grid_program`]).
    pub fn grid_program(&mut self, combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4]) -> Result<Program, String> {
        self.cached(combiner, stages, true)
    }

    fn cached(&mut self, combiner: &[u32; COMBINER_WORDS], stages: [Option<(u32, u32)>; 4], grid: bool) -> Result<Program, String> {
        let key = (program_key(combiner, stages), grid);
        if let Some(p) = self.programs.get(&key) {
            return Ok(with_constants(p.clone(), combiner));
        }
        let p = translate(combiner, stages, grid)?;
        if self.programs.len() >= 256 {
            self.programs.clear();
        }
        self.programs.insert(key, p.clone());
        Ok(p)
    }

    /// Records one quad of `program` with `quad`'s vertex uniforms into
    /// `dest` (format `format`, blended by `blend`), each sampled stage
    /// `n` reading `stages[n]`, the rest of `dest` kept.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        quad: &Quad,
        dest: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        blend: Option<Blend>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) {
        self.record(device, encoder, program, bytemuck::bytes_of(quad), None, (dest, format, blend), stages);
    }

    /// Records a grid's `strips` (`screen_gpu::StripVertex`, each a
    /// triangle strip) of a [`grid_program`] into `dest` (`frame` its
    /// size), as [`Self::draw`] does a quad.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_grid(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        frame: &GridFrame,
        strips: &[Vec<StripVertex>],
        dest: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        blend: Option<Blend>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) {
        self.record(device, encoder, program, bytemuck::bytes_of(frame), Some(strips), (dest, format, blend), stages);
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        program: &Program,
        vertex_uniform: &[u8],
        strips: Option<&[Vec<StripVertex>]>,
        (dest, format, blend): (&wgpu::TextureView, wgpu::TextureFormat, Option<Blend>),
        stages: [Option<&wgpu::TextureView>; 4],
    ) {
        let sampler = self.sampler.get_or_insert_with(|| device.create_sampler(&sampler_descriptor())).clone();
        let c = self.compiled.entry(program.wgsl.clone()).or_insert_with(|| compile(device, program));
        let (module, layout) = (&c.module, &c.layout);
        c.pipelines.entry((format, blend)).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native glow"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: if program.grid { &STRIP_LAYOUT[..] } else { &[] },
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend.map(Blend::state),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let buffer = |label, bytes: &[u8], usage| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage,
            })
        };
        let all: Vec<StripVertex> = strips.unwrap_or_default().iter().flatten().copied().collect();
        if strips.is_some() && all.is_empty() {
            return;
        }
        let quad = buffer("native glow vertex", vertex_uniform, wgpu::BufferUsages::UNIFORM);
        let pixel = buffer("native glow pixel", &program.pixel.to_bytes(), wgpu::BufferUsages::UNIFORM);
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native glow uniforms"),
            layout: &c.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: quad.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pixel.as_entire_binding() },
            ],
        });
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
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native glow"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dest,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&c.pipelines[&(format, blend)]);
        pass.set_bind_group(0, &uniforms, &[]);
        pass.set_bind_group(1, &textures, &[]);
        match strips {
            None => pass.draw(0..4, 0..1),
            Some(strips) => {
                let vertices = buffer("native glow grid", bytemuck::cast_slice(&all), wgpu::BufferUsages::VERTEX);
                pass.set_vertex_buffer(0, vertices.slice(..));
                for r in strip_ranges(strips) {
                    pass.draw(r, 0..1);
                }
            }
        }
    }
}

/// The GPU side: the quads ([`QuadDrawer`]) and the stand-ins for
/// targets 7 and 8 (by size).
#[derive(Default)]
pub struct GpuGlow {
    quads: QuadDrawer,
    small: Option<((u32, u32), [wgpu::TextureView; 2])>,
}

/// Why [`GpuGlow::encode`] drew nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The view's colour cannot be sampled.
    ColourNotSampled,
    /// The view's guest size is not the plan's.
    ViewSize((u32, u32), (u32, u32)),
    /// A block does not compile.
    Program(String),
}

/// Whether [`GpuGlow::encode`] would draw `plan` into `color` (guest size
/// `guest_size`), recording nothing: the target can be sampled, is the
/// plan's size, and every block compiles.
pub fn check(color: &wgpu::TextureView, guest_size: (u32, u32), plan: &Plan) -> Result<(), Refused> {
    check_target(color, guest_size, plan)?;
    programs(plan).map(|_| ()).map_err(Refused::Program)
}

fn check_target(color: &wgpu::TextureView, guest_size: (u32, u32), plan: &Plan) -> Result<(), Refused> {
    if !color.texture().usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        return Err(Refused::ColourNotSampled);
    }
    if guest_size != plan.view {
        return Err(Refused::ViewSize(guest_size, plan.view));
    }
    Ok(())
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

    /// Records `plan`'s draws in order. The stand-ins are the guest sizes
    /// times this copy's scale. Nothing is recorded when it refuses.
    pub fn encode(&mut self, mut t: GlowTarget<'_>, plan: &Plan) -> Result<(), Refused> {
        check_target(t.color, t.guest_size, plan)?;
        let mut programs = Vec::with_capacity(plan.draws.len());
        for d in &plan.draws {
            let stages = d.stages.map(|s| s.map(|s| plan.size(s)));
            programs.push(self.quads.program(&d.combiner, stages).map_err(Refused::Program)?);
        }
        for (draw, program) in plan.draws.iter().zip(&programs) {
            let reads_dest = (0..4).any(|n| program.dims[n].is_some() && draw.stages[n] == Some(draw.dest));
            if reads_dest {
                return Err(Refused::Program("a quad samples the surface it draws into".into()));
            }
        }
        let scale = |n: u32, of: u32, by: u32| ((u64::from(n) * u64::from(of)).div_ceil(u64::from(by.max(1))) as u32).max(1);
        let small = (
            scale(plan.small.0, t.size.0, t.guest_size.0),
            scale(plan.small.1, t.size.1, t.guest_size.1),
        );
        let device = t.device;
        if self.small.as_ref().is_none_or(|(s, _)| *s != small) {
            let make = |label| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width: small.0, height: small.1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: SMALL_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            self.small = Some((small, [make("native glow 7"), make("native glow 8")]));
        }
        let Some((_, [down, other])) = &self.small else { return Ok(()) };
        let view_of = |s: Surface| match s {
            Surface::View => t.color,
            Surface::Down => down,
            Surface::Other => other,
        };
        for (draw, program) in plan.draws.iter().zip(&programs) {
            let format = if draw.dest == Surface::View { t.color_format } else { SMALL_FORMAT };
            let stages = draw.stages.map(|s| s.map(view_of));
            self.quads.draw(device, &mut *t.encoder, program, &plan.quad(draw), view_of(draw.dest), format, draw.blend, stages);
        }
        Ok(())
    }
}

fn uniforms_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native glow uniforms"),
        entries: &[
            crate::program::uniform_entry(0, wgpu::ShaderStages::VERTEX),
            crate::program::uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

fn compile(device: &wgpu::Device, program: &Program) -> Compiled {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native glow"),
        source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
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
            small: (160, 120),
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
        assert_eq!(p.small, (160, 120));
        m.insert(TARGETS + TARGET_OTHER * TARGET_STRIDE + 0x80, 80);
        assert_eq!(read_frame(&Words(m)), None, "7 and 8 differ");
    }
}
