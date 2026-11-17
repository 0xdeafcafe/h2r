//! The draw path: guest-style draws through the state model.
//!
//! This is the seed of the D3D device replacement: callers describe a draw
//! with a vertex format ([`VertexLayout`]), vertex streams, a
//! [`PrimitiveType`] and a typed [`RenderState`]; the renderer repacks vertex
//! data where wgpu cannot read the guest encoding, translates the state to a
//! cached wgpu pipeline and records the draw.
//!
//! Per draw, on top of the pipeline state:
//!
//! * [`Draw::viewport`] maps clip space to a sub-rectangle of the target and
//!   a depth range (a wgpu viewport). Shaders that already produce window
//!   coordinates (pre-transformed vertices, translated NV2A programs) leave
//!   it `None` and clip with [`Draw::scissor`] instead.
//! * [`Draw::scissor`] clips to a rectangle.
//! * [`Draw::indices`] draws an index list over the streams.
//! * [`Draw::count_samples`] adds the samples the draw writes to a
//!   [`PixelCounter`] (see [`crate::occlusion`]).
//!
//! One shader is built in ([`SHADER_XYZRHW_DIFFUSE`], the fixed-function
//! stand-in for pre-transformed `XYZRHW | DIFFUSE` vertices). It honours the
//! alpha test in [`RenderState`]. Generated shaders plug in through
//! [`Renderer::register_shader`] and [`Draw::shader`]; they do their own
//! alpha test. [`Renderer::clear_rects`] clears rectangles and channel
//! subsets (see [`crate::clear`]).

use std::collections::HashMap;
use std::sync::Arc;

use crate::clear::{Clearer, MaskedClear};
use crate::compile::{Compiler, Pending, PendingPipeline, Priority};
use crate::context::GfxContext;
use crate::occlusion::{COUNT_FORMAT, COUNT_FRAGMENT_WGSL, PixelCounter};
use crate::persistent::{PersistentStats, PersistentVertices};
use crate::pipeline::{
    PipelineCache, PipelineKey, ShaderId, ShaderProgram, TargetFormats, create_render_pipeline,
};
use crate::presenter::RenderTarget;
use crate::repack::{FetchMode, RepackError, RepackPlan};
use crate::ring::UploadRing;
use crate::state::{CompareFunc, RenderState};
use crate::translate::{self, Capabilities};
use crate::vertex::{HostPrimitive, PrimitiveType, VertexDataType, VertexLayout, VertexRegister};

/// The shader id of the built-in `XYZRHW | DIFFUSE` pass-through program.
pub const SHADER_XYZRHW_DIFFUSE: ShaderId = ShaderId(0);

/// Why a draw was rejected.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DrawError {
    /// The built-in shader needs pre-transformed float4 position and a
    /// `D3DCOLOR` diffuse.
    #[error("the built-in shader needs XYZRHW (float4 position) and a D3DCOLOR diffuse")]
    UnsupportedVertexFormat,
    /// The draw names a shader that was never registered.
    #[error("unknown shader {0:?}")]
    UnknownShader(ShaderId),
    /// `streams.len()` differs from the layout's stream count.
    #[error("draw supplies {given} vertex streams but the layout declares {declared}")]
    StreamCount {
        /// Streams supplied.
        given: usize,
        /// Streams declared.
        declared: usize,
    },
    /// Vertex data could not be repacked.
    #[error(transparent)]
    Repack(#[from] RepackError),
    /// An index names a vertex the streams do not hold.
    #[error("index {index} is out of range for {vertices} vertices")]
    IndexOutOfRange {
        /// The index.
        index: u32,
        /// Vertices in the streams.
        vertices: u32,
    },
    /// The viewport is empty, not finite, or not inside the target.
    #[error("viewport {0:?} is not inside the {1}x{2} target")]
    BadViewport(DrawViewport, u32, u32),
    /// A draw counts samples, but no [`PixelCounter`] was given (see
    /// [`Renderer::render_counted`]).
    #[error("draw counts samples, but the pass has no pixel counter")]
    NoPixelCounter,
    /// A counted draw that writes depth or stencil needs to copy the
    /// depth/stencil attachment (see [`crate::occlusion`]), but its texture
    /// lacks `COPY_SRC`. Read-only counted draws replay in place and need
    /// no copy.
    /// [`SurfaceStore`](crate::SurfaceStore) depth surfaces have it.
    #[error("draw counts samples, but the depth/stencil texture cannot be copied (no COPY_SRC)")]
    CountDepthNotCopyable,
}

/// Which attachments a pass clears (D3D `Clear` flags, typed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clear {
    /// Clear colour (`D3DCLEAR_TARGET`).
    pub color: Option<[f32; 4]>,
    /// Clear depth (`D3DCLEAR_ZBUFFER`).
    pub depth: Option<f32>,
    /// Clear stencil (`D3DCLEAR_STENCIL`).
    pub stencil: Option<u8>,
}

impl Clear {
    /// Clear nothing.
    pub const NONE: Self = Self {
        color: None,
        depth: None,
        stencil: None,
    };

    /// Clear everything: `color`, depth 1.0, stencil 0.
    pub fn all(color: [f32; 4]) -> Self {
        Self {
            color: Some(color),
            depth: Some(1.0),
            stencil: Some(0),
        }
    }
}

/// A wgpu viewport for one draw: clip space maps to this rectangle (target
/// pixels, origin top left) and depth to `[min_depth, max_depth]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawViewport {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Depth at clip z = 0.
    pub min_depth: f32,
    /// Depth at clip z = 1.
    pub max_depth: f32,
}

impl DrawViewport {
    /// The whole of a `width` x `height` target, depth 0..1.
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }
    }

    fn valid_in(&self, w: u32, h: u32) -> bool {
        let v = [
            self.x,
            self.y,
            self.width,
            self.height,
            self.min_depth,
            self.max_depth,
        ];
        v.iter().all(|f| f.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.width > 0.0
            && self.height > 0.0
            && self.x + self.width <= w as f32
            && self.y + self.height <= h as f32
            && (0.0..=1.0).contains(&self.min_depth)
            && (0.0..=1.0).contains(&self.max_depth)
            && self.min_depth <= self.max_depth
    }
}

/// A rectangle in target pixels (origin top left).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScissorRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl ScissorRect {
    /// The whole of a `width` x `height` target.
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// From edges `x1 <= x < x2`, `y1 <= y < y2` (signed, as D3D rects);
    /// negative edges clamp to 0.
    pub fn from_edges(x1: i64, y1: i64, x2: i64, y2: i64) -> Self {
        let c = |v: i64| v.clamp(0, u32::MAX as i64) as u32;
        let (x1, y1, x2, y2) = (c(x1), c(y1), c(x2), c(y2));
        Self {
            x: x1,
            y: y1,
            width: x2.saturating_sub(x1),
            height: y2.saturating_sub(y1),
        }
    }

    /// The part of this rectangle inside `other`.
    pub fn intersect(self, other: Self) -> Self {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x as u64 + self.width as u64).min(other.x as u64 + other.width as u64);
        let y2 = (self.y as u64 + self.height as u64).min(other.y as u64 + other.height as u64);
        Self {
            x: x1,
            y: y1,
            width: x2.saturating_sub(x1 as u64) as u32,
            height: y2.saturating_sub(y1 as u64) as u32,
        }
    }

    /// Whether it covers no pixel.
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether it covers the whole `width` x `height` target.
    pub fn covers(self, width: u32, height: u32) -> bool {
        self.intersect(Self::full(width, height)) == Self::full(width, height)
    }
}

/// One guest vertex stream bound for a draw.
#[derive(Clone, Copy, Debug)]
pub struct VertexStream<'a> {
    /// Guest bytes, starting at the first vertex to draw.
    pub data: &'a [u8],
    /// Guest stride (`SetStreamSource`); `0` uses the layout's declared one.
    pub stride: u32,
}

/// A bind group for one draw: the group index, the group, and its dynamic
/// offsets (empty for a layout without dynamic bindings).
#[derive(Clone, Debug)]
pub struct BoundGroup {
    /// Group index (`@group(n)`).
    pub index: u32,
    /// The group.
    pub group: wgpu::BindGroup,
    /// Dynamic offsets, in binding order.
    pub offsets: Vec<u32>,
}

impl BoundGroup {
    /// A group without dynamic offsets.
    pub fn new(index: u32, group: wgpu::BindGroup) -> Self {
        Self {
            index,
            group,
            offsets: Vec::new(),
        }
    }
}

/// One draw call. Cloning it copies the borrows (a draw repeated with another
/// scissor, for example).
#[derive(Clone)]
pub struct Draw<'a> {
    /// Shader program ([`SHADER_XYZRHW_DIFFUSE`] or a registered one).
    pub shader: ShaderId,
    /// One entry per stream of `layout`, in the layout's order.
    pub streams: &'a [VertexStream<'a>],
    /// Number of vertices in `streams`. Without `indices` this is also the
    /// number of vertices drawn.
    pub vertex_count: u32,
    /// Vertex format.
    pub layout: &'a VertexLayout,
    /// Primitive type, including Xbox-only ones.
    pub primitive: PrimitiveType,
    /// Render state in effect.
    pub state: RenderState,
    /// Extra bind groups for registered shaders. The built-in shader
    /// manages group 0 itself.
    pub bind_groups: &'a [BoundGroup],
    /// Viewport; `None` is the whole target, depth 0..1.
    pub viewport: Option<DrawViewport>,
    /// Scissor rectangle; `None` is the whole target. Clamped to the
    /// target; a draw whose scissor is empty draws nothing.
    pub scissor: Option<ScissorRect>,
    /// Index list into `streams` (each below `vertex_count`), in primitive
    /// order; Xbox-only primitive types are rewritten on top of it. `None`
    /// draws the vertices in order.
    pub indices: Option<&'a [u32]>,
    /// Add the samples this draw writes (those passing the depth and
    /// stencil tests) to the pass's [`PixelCounter`] (see
    /// [`crate::occlusion`]).
    pub count_samples: bool,
    /// The window the built-in shader's pre-transformed positions are in,
    /// when it is not the target: a target rendered at a multiple of the
    /// guest's size keeps guest window coordinates, shifted by
    /// [`window_shift`] so an edge on a window pixel centre covers whole
    /// blocks of host pixels, as it covers whole pixels at 1x
    /// (docs/graphics/high-resolution.md). `None` is the target's size.
    pub window: Option<(u32, u32)>,
    /// Where vertex formats are converted. [`FetchMode::Gpu`] needs a
    /// shader that reads each register as
    /// [`RepackPlan::input_fetch`](crate::repack::RepackPlan::input_fetch)
    /// says (`shaders::vsh::VertexOptions::input_fetch`); [`Draw::new`]
    /// picks [`FetchMode::Cpu`], where every attribute is a float format.
    pub vertex_fetch: FetchMode,
    /// Per stream (by position; missing entries are `None`), an id under
    /// which its bytes never change: the stream's host buffer is kept
    /// across submissions and bound again by later draws of the id instead
    /// of being staged in the upload ring ([`crate::persistent`]). Empty
    /// (the default) stages every stream.
    pub stream_ids: &'a [Option<u64>],
    /// An id under which `indices` never change: the host index list is
    /// kept across submissions like [`stream_ids`](Self::stream_ids)'
    /// buffers. `None` (the default) stages it.
    pub index_id: Option<u64>,
}

impl<'a> Draw<'a> {
    /// A non-indexed draw over the whole target with no extra bind groups.
    pub fn new(
        shader: ShaderId,
        layout: &'a VertexLayout,
        streams: &'a [VertexStream<'a>],
        vertex_count: u32,
        primitive: PrimitiveType,
        state: RenderState,
    ) -> Self {
        Self {
            shader,
            streams,
            vertex_count,
            layout,
            primitive,
            state,
            bind_groups: &[],
            viewport: None,
            scissor: None,
            indices: None,
            count_samples: false,
            window: None,
            vertex_fetch: FetchMode::Cpu,
            stream_ids: &[],
            index_id: None,
        }
    }
}

/// [`Draw::window`]'s shift for a window `window` pixels across on a
/// target `target` pixels across, in window pixels: `(2 s - 1) / (4 s)`
/// for `s = target / window`, 0 when they are equal.
///
/// At 1x a pixel is sampled at its centre, and D3D-convention geometry puts
/// edges (and a 1:1 copy's texel corners) exactly there, so the tie rules
/// decide. With the shift the `s x s` host pixels of a guest pixel sample
/// it at offsets `(k + 1/4) / s` from its centre, `k = 0..s`: all inside
/// `[0, 1)`, none on the tie. An edge on a guest pixel centre then covers
/// whole blocks, as it covers whole pixels at 1x (top-left rule included),
/// and a 1:1 point-sampled copy reads the texel 1x reads.
pub fn window_shift(window: u32, target: u32) -> f32 {
    if window == 0 || window >= target {
        return 0.0;
    }
    let s = target as f32 / window as f32;
    (2.0 * s - 1.0) / (4.0 * s)
}

/// Bytes of the built-in shader's uniform struct.
const BUILTIN_UNIFORM_SIZE: u64 = 32;
/// Initial size of the vertex/index upload ring.
const VERTEX_RING: u64 = 4 << 20;
/// Initial size of the built-in uniform ring.
const UNIFORM_RING: u64 = 1 << 20;

/// The built-in shader's alpha-test code for a compare function (0 = test
/// off); matches `alpha_passes` in `xyzrhw_diffuse.wgsl`.
fn builtin_alpha_func(rs: &RenderState) -> f32 {
    if !rs.alpha_test_enable {
        return 0.0;
    }
    match rs.alpha_func {
        CompareFunc::Always => 0.0,
        CompareFunc::Never => 1.0,
        CompareFunc::Less => 2.0,
        CompareFunc::Equal => 3.0,
        CompareFunc::LessEqual => 4.0,
        CompareFunc::Greater => 5.0,
        CompareFunc::NotEqual => 6.0,
        CompareFunc::GreaterEqual => 7.0,
    }
}

/// Draws guest primitives through the state model.
pub struct Renderer {
    shaders: HashMap<ShaderId, ShaderProgram>,
    builtin_bgl: wgpu::BindGroupLayout,
    cache: PipelineCache<Arc<PendingPipeline>>,
    clearer: Clearer,
    /// Builds pipelines off this thread.
    compiler: Arc<Compiler>,
    /// Keys of pipelines draws asked for that were not in the cache (and
    /// whether it was the count pipeline), oldest first, until
    /// [`take_new_keys`](Renderer::take_new_keys).
    new_keys: Vec<(PipelineKey, bool)>,
    caps: Capabilities,
    z_max: f32,
    /// Vertex and index bytes of recorded draws.
    vertices: UploadRing,
    /// Built-in shader uniforms of recorded draws.
    uniforms: UploadRing,
    /// The built-in uniform group on `uniforms`' current buffer, by ring
    /// generation.
    builtin_group: Option<(u64, wgpu::BindGroup)>,
    uniform_align: u64,
    /// Count-pass pipelines (see [`crate::occlusion`]), keyed by the
    /// draw's own key.
    count_cache: PipelineCache<Arc<PendingPipeline>>,
    count_fragment: Option<wgpu::ShaderModule>,
    /// What registered vertex programs are built for ([`Renderer::vertex_fetch`]).
    vertex_fetch: FetchMode,
    /// Reused buffer for streams that are realigned or converted.
    scratch: Vec<u8>,
    /// Host buffers of streams with an id ([`Draw::stream_ids`]).
    persistent: PersistentVertices,
}

/// One draw, validated, with its pipeline built and its bytes staged in the
/// renderer's upload rings: ready for [`Renderer::encode_pass`] on a target
/// with the size and formats it was recorded for.
pub struct RecordedDraw {
    /// Built on a compile worker if it was not cached; waited for (or built)
    /// when the draw is encoded.
    pipeline: Arc<PendingPipeline>,
    vertex_buffers: Vec<(wgpu::Buffer, u64, u64)>,
    indices: Option<(wgpu::Buffer, u64, u64)>,
    vertices: u32,
    builtin: Option<(wgpu::BindGroup, u32)>,
    bind_groups: Vec<BoundGroup>,
    viewport: DrawViewport,
    scissor: ScissorRect,
    blend_constant: wgpu::Color,
    stencil_ref: u32,
    /// The count-pass pipeline of a draw that counts its samples, and
    /// whether it writes depth or stencil (the replay then needs a scratch
    /// copy of the attachment).
    count: Option<(Arc<PendingPipeline>, bool)>,
    /// [`encode_pass`](Renderer::encode_pass) opens a new render pass for
    /// this draw.
    new_pass: bool,
    /// The pipeline's shader and fixed-function state, for
    /// [`describe`](Self::describe).
    state: Box<(
        ShaderId,
        wgpu::PrimitiveState,
        Option<wgpu::DepthStencilState>,
        wgpu::ColorTargetState,
    )>,
}

impl RecordedDraw {
    /// Whether the draw adds its samples to the pass's [`PixelCounter`].
    pub fn counts_samples(&self) -> bool {
        self.count.is_some()
    }

    /// Makes [`Renderer::encode_pass`] start a new render pass (on the same
    /// encoder) at this draw instead of continuing the previous draw's.
    pub fn start_pass(&mut self) {
        self.new_pass = true;
    }

    /// A summary of what the draw binds (vertex and index ranges, group
    /// offsets, viewport, scissor, stencil reference, blend constant) and of
    /// its pipeline (shader, primitive, depth and stencil, blend and write
    /// mask), for diagnostics.
    pub fn describe(&self) -> String {
        let (shader, primitive, depth_stencil, color) = &*self.state;
        format!(
            "vb={:?} idx={:?} n={} builtin={} groups={:?} vp={:?} sc={:?} sref={} bc={:?}\n      shader={shader:?} primitive={primitive:?}\n      depth_stencil={depth_stencil:?}\n      color={color:?}",
            self.vertex_buffers
                .iter()
                .map(|(b, o, l)| (b.size(), *o, *l))
                .collect::<Vec<_>>(),
            self.indices.as_ref().map(|(b, o, l)| (b.size(), *o, *l)),
            self.vertices,
            self.builtin.is_some(),
            self.bind_groups
                .iter()
                .map(|g| (g.index, g.offsets.clone()))
                .collect::<Vec<_>>(),
            self.viewport,
            self.scissor,
            self.stencil_ref,
            self.blend_constant,
        )
    }

    /// The colour channels the draw writes.
    pub fn color_writes(&self) -> wgpu::ColorWrites {
        self.state.3.write_mask
    }

    /// Whether [`encode_pass`](Renderer::encode_pass) opens a new render
    /// pass for this draw.
    pub fn starts_pass(&self) -> bool {
        self.new_pass
    }

    /// Sets or clears [`start_pass`](Self::start_pass).
    pub fn set_start_pass(&mut self, on: bool) {
        self.new_pass = on;
    }
}

/// `D3DZ_MAX` for a 24-bit integer depth buffer: the range of
/// pre-transformed z.
pub const D3DZ_MAX_24: f32 = 16_777_215.0;
/// `D3DZ_MAX` for a 16-bit integer depth buffer.
pub const D3DZ_MAX_16: f32 = 65_535.0;
/// `D3DZ_MAX` for the Xbox 16-bit float depth format (4-bit exponent,
/// 12-bit mantissa): its largest value, `(2 - 2^-12) * 2^8`.
pub const D3DZ_MAX_F16: f32 = 511.9375;
/// `D3DZ_MAX` for the Xbox 24-bit float depth format (8-bit exponent,
/// 16-bit mantissa): its largest value, `(2 - 2^-16) * 2^127`.
pub const D3DZ_MAX_F24: f32 = 3.402_797_7e38;

/// The full scale of window-space z for a depth buffer format
/// (`D3DZ_MAX_*`): pre-transformed and vertex-program z is divided by it
/// to get the host depth in 0..1. No depth buffer counts as D24S8.
pub fn d3dz_max(depth: Option<crate::D3DFormat>) -> f32 {
    use crate::D3DFormat as F;
    match depth {
        Some(F::D16 | F::LinD16) => D3DZ_MAX_16,
        Some(F::F16 | F::LinF16) => D3DZ_MAX_F16,
        Some(F::F24S8 | F::LinF24S8) => D3DZ_MAX_F24,
        _ => D3DZ_MAX_24,
    }
}

/// The built-in uniform group over `buffer`, one struct at a dynamic
/// offset.
fn builtin_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("builtin draw uniforms"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(BUILTIN_UNIFORM_SIZE),
            }),
        }],
    })
}

impl Renderer {
    /// Creates the renderer and its built-in shader.
    pub fn new(ctx: &GfxContext) -> Self {
        let module = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xyzrhw_diffuse"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../shaders/xyzrhw_diffuse.wgsl").into(),
                ),
            });
        let bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("builtin draw uniforms"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(BUILTIN_UNIFORM_SIZE),
                    },
                    count: None,
                }],
            });
        let layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xyzrhw layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
        let mut shaders = HashMap::new();
        shaders.insert(
            SHADER_XYZRHW_DIFFUSE,
            ShaderProgram {
                module,
                layout,
                vs_entry: "vs_main".into(),
                fs_entry: "fs_main".into(),
                count_entry: None,
            },
        );
        Self {
            shaders,
            builtin_bgl: bgl,
            cache: PipelineCache::new(),
            clearer: Clearer::new(&ctx.device),
            compiler: Compiler::global().clone(),
            new_keys: Vec::new(),
            caps: ctx.caps,
            z_max: D3DZ_MAX_24,
            vertices: UploadRing::new(
                &ctx.device,
                "draw vertices",
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
                VERTEX_RING,
            ),
            uniforms: UploadRing::new(
                &ctx.device,
                "builtin draw uniforms",
                wgpu::BufferUsages::UNIFORM,
                UNIFORM_RING,
            ),
            builtin_group: None,
            uniform_align: (ctx.device.limits().min_uniform_buffer_offset_alignment as u64)
                .max(4)
                .next_power_of_two(),
            count_cache: PipelineCache::new(),
            count_fragment: None,
            vertex_fetch: FetchMode::from_env(),
            scratch: Vec::new(),
            persistent: PersistentVertices::new(PersistentVertices::budget_from_env()),
        }
    }

    /// The vertex fetch mode a shader provider should build programs for
    /// and set as [`Draw::vertex_fetch`]: [`FetchMode::from_env`] unless
    /// [`set_vertex_fetch`](Self::set_vertex_fetch) changed it. The renderer
    /// itself follows each draw's own field.
    pub fn vertex_fetch(&self) -> FetchMode {
        self.vertex_fetch
    }

    /// Overrides [`vertex_fetch`](Self::vertex_fetch) (tests and benches
    /// comparing the two paths).
    pub fn set_vertex_fetch(&mut self, mode: FetchMode) {
        self.vertex_fetch = mode;
    }

    /// Sets the range of pre-transformed (`XYZRHW`) z values for the
    /// built-in shader: [`d3dz_max`] of the depth format (default
    /// 16777215, D24S8). Applies to draws recorded after it.
    pub fn set_pretransformed_z_max(&mut self, z_max: f32) {
        self.z_max = z_max.max(1.0);
    }

    /// Registers a generated shader under `id`. Returns `false` (and does
    /// nothing) for the built-in id. Re-registering an id replaces the
    /// program and drops all cached pipelines.
    pub fn register_shader(&mut self, id: ShaderId, program: ShaderProgram) -> bool {
        if id == SHADER_XYZRHW_DIFFUSE {
            return false;
        }
        if self.shaders.insert(id, program).is_some() {
            self.cache.clear();
            self.count_cache.clear();
        }
        true
    }

    /// Whether `id` names a registered (or the built-in) shader.
    pub fn has_shader(&self, id: ShaderId) -> bool {
        self.shaders.contains_key(&id)
    }

    /// The pipeline cache (for stats).
    pub fn cache(&self) -> &PipelineCache<Arc<PendingPipeline>> {
        &self.cache
    }

    /// Builds pipelines on `compiler`'s workers instead of the global one
    /// (`Compiler::new(0)`: on the first draw that encodes them).
    pub fn set_compiler(&mut self, compiler: Arc<Compiler>) {
        self.compiler = compiler;
    }

    /// The keys of pipelines draws needed that were not cached (`true`:
    /// the draw's count pipeline) since the last call, oldest first: the
    /// ones worth [`prewarm`](Self::prewarm)ing next time.
    pub fn take_new_keys(&mut self) -> Vec<(PipelineKey, bool)> {
        std::mem::take(&mut self.new_keys)
    }

    /// Starts building the pipeline for `key` (its count pipeline with
    /// `count`) on a compile worker, behind any a draw is waiting for, so a
    /// later draw that needs it finds it built. Returns `false` if it is
    /// already cached or `key.shader` is not registered.
    pub fn prewarm(&mut self, ctx: &GfxContext, key: &PipelineKey, count: bool) -> bool {
        let Some(program) = self.shaders.get(&key.shader) else {
            return false;
        };
        let cache = if count {
            &mut self.count_cache
        } else {
            &mut self.cache
        };
        if cache.get(key).is_some() {
            return false;
        }
        let pending = if count {
            let fragment = count_fragment(&mut self.count_fragment, &ctx.device);
            pending_count(&ctx.device, key, program, &fragment)
        } else {
            pending_pipeline(&ctx.device, key, program)
        };
        self.compiler.submit(&pending, Priority::Background);
        cache.insert(key.clone(), pending);
        true
    }

    /// Whether the built-in shader can draw `layout` (pre-transformed float4
    /// position and a `D3DCOLOR` diffuse).
    pub fn builtin_supports(layout: &VertexLayout) -> bool {
        let has = |reg: VertexRegister, ty: VertexDataType| {
            layout
                .streams
                .iter()
                .flat_map(|s| &s.attributes)
                .any(|a| a.register == reg && a.data_type == ty)
        };
        layout.pretransformed
            && has(VertexRegister::POSITION, VertexDataType::Float4)
            && has(VertexRegister::DIFFUSE, VertexDataType::D3dColor)
    }

    /// Clears `rects` of `target` (clamped to it), only the attachments and
    /// colour channels `clear` names, in one pass recorded on `encoder`.
    /// Rectangles that cover the whole target with every channel use load
    /// operations; anything else draws through a scissor.
    pub fn clear_rects(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: MaskedClear,
        rects: &[ScissorRect],
    ) {
        self.clearer.clear(ctx, encoder, target, clear, rects);
    }

    /// Records one render pass containing `draws`, applying `clear` first.
    /// The draws' bytes are written to the queue before it returns, so the
    /// pass is ready for the caller's next submit.
    pub fn render(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: Clear,
        draws: &[Draw<'_>],
    ) -> Result<(), DrawError> {
        self.render_counted(ctx, encoder, target, clear, draws, None)
    }

    /// [`render`](Self::render) with a pixel counter: each draw with
    /// [`Draw::count_samples`] adds the samples it writes to `counter`
    /// (see [`crate::occlusion`]; a draw scissored to nothing adds zero).
    /// The counted draws get a count pass and a compute pass of their own
    /// before they are drawn, so `draws` may be split over several render
    /// passes; the clear applies once, first.
    pub fn render_counted(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: Clear,
        draws: &[Draw<'_>],
        counter: Option<&mut PixelCounter>,
    ) -> Result<(), DrawError> {
        if counter.is_none() && draws.iter().any(|d| d.count_samples) {
            return Err(DrawError::NoPixelCounter);
        }
        let mut recorded = Vec::with_capacity(draws.len());
        for d in draws {
            recorded.push(self.record(ctx, target, d)?);
        }
        self.flush_uploads(&ctx.queue);
        Self::encode_pass(ctx, encoder, target, clear, &recorded, counter).map(|_| ())
    }

    /// Validates `d` for `target`'s size and formats, builds (or finds) its
    /// pipeline and stages its vertices, indices and built-in uniforms.
    /// The staged bytes reach the GPU at [`flush_uploads`](Self::flush_uploads),
    /// which must come before the submit of the pass the draw is encoded in.
    pub fn record(
        &mut self,
        ctx: &GfxContext,
        target: &RenderTarget<'_>,
        d: &Draw<'_>,
    ) -> Result<RecordedDraw, DrawError> {
        let (w, h) = target.size;
        let formats = TargetFormats {
            color: target.color_format,
            depth: target.depth_format,
            samples: target.samples,
        };
        if !self.shaders.contains_key(&d.shader) {
            return Err(DrawError::UnknownShader(d.shader));
        }
        if d.shader == SHADER_XYZRHW_DIFFUSE && !Self::builtin_supports(d.layout) {
            return Err(DrawError::UnsupportedVertexFormat);
        }
        if d.streams.len() != d.layout.streams.len() {
            return Err(DrawError::StreamCount {
                given: d.streams.len(),
                declared: d.layout.streams.len(),
            });
        }
        let viewport = d.viewport.unwrap_or(DrawViewport::full(w, h));
        if !viewport.valid_in(w, h) {
            return Err(DrawError::BadViewport(viewport, w, h));
        }
        let full = ScissorRect::full(w, h);
        let scissor = d.scissor.map_or(full, |s| s.intersect(full));
        if let Some(idx) = d.indices
            && let Some(&bad) = idx.iter().find(|&&i| i >= d.vertex_count)
        {
            return Err(DrawError::IndexOutOfRange {
                index: bad,
                vertices: d.vertex_count,
            });
        }
        let strides: Vec<u32> = d.streams.iter().map(|s| s.stride).collect();
        // The built-in shader reads floats only.
        let mode = if d.shader == SHADER_XYZRHW_DIFFUSE {
            FetchMode::Cpu
        } else {
            d.vertex_fetch
        };
        let plan = RepackPlan::with_mode(d.layout, &strides, mode);
        for (sp, vs) in plan.streams.iter().zip(d.streams) {
            sp.check(vs.data, d.vertex_count)?;
        }
        let mut key =
            PipelineKey::from_plan(d.shader, &plan, d.primitive, &d.state, formats, self.caps);
        // Only a counted draw that writes depth or stencil copies the
        // attachment (a read-only one replays in place).
        if d.count_samples
            && key.depth_stencil.as_ref().is_some_and(writes_depth_stencil)
            && target
                .depth
                .is_some_and(|v| !v.texture().usage().contains(wgpu::TextureUsages::COPY_SRC))
        {
            return Err(DrawError::CountDepthNotCopyable);
        }
        let drawn = d.indices.map_or(d.vertex_count, |i| i.len() as u32);
        // A kept index list (`Draw::index_id`) is bound as it is.
        let kept_id = d.index_id.filter(|_| d.indices.is_some());
        let kept = kept_id.and_then(|id| self.persistent.cached_indices(id, d.primitive));
        let list: Option<Vec<u32>> = match (kept.is_some(), d.primitive.to_host(drawn), d.indices) {
            (true, ..) => None,
            (false, HostPrimitive::Native(_), None) => None,
            (false, HostPrimitive::Native(_), Some(user)) => Some(user.to_vec()),
            (false, HostPrimitive::Indexed(_, conv), None) => Some(conv),
            (false, HostPrimitive::Indexed(_, conv), Some(user)) => {
                Some(conv.iter().map(|&c| user[c as usize]).collect())
            }
        };
        // An indexed strip needs the index format in the pipeline (wgpu
        // validates it; restart never triggers, indices are below
        // `vertex_count`).
        if (list.is_some() || kept.is_some())
            && matches!(
                key.primitive.topology,
                wgpu::PrimitiveTopology::TriangleStrip | wgpu::PrimitiveTopology::LineStrip
            )
        {
            key.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
        }
        // A miss starts the compile on a worker; the draw waits for it only
        // when it is encoded (`encode_pass`).
        let program = &self.shaders[&d.shader];
        let compiler = &self.compiler;
        let new_keys = &mut self.new_keys;
        let pipeline = self
            .cache
            .get_or_insert_with(&key, |k| {
                new_keys.push((k.clone(), false));
                let p = pending_pipeline(&ctx.device, k, program);
                compiler.submit(&p, Priority::Draw);
                p
            })
            .clone();
        let count = d.count_samples.then(|| {
            let fragment = count_fragment(&mut self.count_fragment, &ctx.device);
            let pipeline = self
                .count_cache
                .get_or_insert_with(&key, |k| {
                    new_keys.push((k.clone(), true));
                    let p = pending_count(&ctx.device, k, program, &fragment);
                    compiler.submit(&p, Priority::Draw);
                    p
                })
                .clone();
            (
                pipeline,
                key.depth_stencil.as_ref().is_some_and(writes_depth_stencil),
            )
        });

        // Each stream is written straight into the upload ring's staging
        // memory: guest bytes for a raw stream, realigned or converted
        // vertices otherwise. An empty stream takes 4 bytes: slices are
        // never empty.
        let scratch = &mut self.scratch;
        let persistent = &mut self.persistent;
        let vertex_buffers = plan
            .streams
            .iter()
            .zip(d.streams)
            .enumerate()
            .map(|(i, (sp, vs))| {
                if let Some(&Some(id)) = d.stream_ids.get(i) {
                    let (buf, len) = persistent.get(ctx, id, sp, vs.data, d.vertex_count, scratch);
                    return (buf, 0, len);
                }
                let len = sp.host_bytes(d.vertex_count);
                let (buf, off) = self
                    .vertices
                    .push_with(&ctx.device, &ctx.queue, len, 4, |out| {
                        sp.write_into(vs.data, d.vertex_count, out, scratch)
                    });
                (buf, off, (len as u64).max(4))
            })
            .collect();
        let (vertices, indices) = match kept {
            Some((buf, len, count)) => (count, Some((buf, 0, len))),
            None => {
                let vertices = list.as_ref().map_or(drawn, |l| l.len() as u32);
                let indices = list.map(|idx| match kept_id {
                    Some(id) => {
                        let (buf, len) = self.persistent.put_indices(ctx, id, d.primitive, &idx);
                        (buf, 0, len)
                    }
                    None => {
                        let bytes: &[u8] = bytemuck::cast_slice(&idx);
                        let len = (bytes.len() as u64).max(4);
                        let (buf, off) = self.vertices.push(&ctx.device, &ctx.queue, bytes, 4);
                        (buf, off, len)
                    }
                });
                (vertices, indices)
            }
        };
        let builtin = (d.shader == SHADER_XYZRHW_DIFFUSE).then(|| {
            let (ww, wh) = d.window.unwrap_or((w, h));
            let words = [
                ww as f32,
                wh as f32,
                self.z_max,
                builtin_alpha_func(&d.state),
                d.state.alpha_ref as f32,
                window_shift(ww, w),
                window_shift(wh, h),
                0.0,
            ];
            let (buf, off) = self.uniforms.push(
                &ctx.device,
                &ctx.queue,
                bytemuck::cast_slice(&words),
                self.uniform_align,
            );
            let generation = self.uniforms.generation();
            let group = match &self.builtin_group {
                Some((g, group)) if *g == generation => group.clone(),
                _ => {
                    let group = builtin_group(&ctx.device, &self.builtin_bgl, &buf);
                    self.builtin_group = Some((generation, group.clone()));
                    group
                }
            };
            (group, off as u32)
        });
        Ok(RecordedDraw {
            pipeline,
            vertex_buffers,
            indices,
            vertices,
            builtin,
            bind_groups: d.bind_groups.to_vec(),
            viewport,
            scissor,
            blend_constant: translate::blend_constant(&d.state),
            stencil_ref: d.state.stencil.reference as u32,
            count,
            new_pass: false,
            state: Box::new((d.shader, key.primitive, key.depth_stencil, key.color_target)),
        })
    }

    /// Writes the bytes staged by [`record`](Self::record) since the last
    /// flush to the queue.
    pub fn flush_uploads(&mut self, queue: &wgpu::Queue) {
        self.vertices.flush(queue);
        self.uniforms.flush(queue);
    }

    /// Counters of the persistent vertex buffers ([`Draw::stream_ids`]).
    pub fn persistent_stats(&self) -> PersistentStats {
        self.persistent.stats()
    }

    /// Lets the upload rings reuse their space: call after submitting every
    /// pass recorded so far (and after [`flush_uploads`](Self::flush_uploads)).
    /// Without it the rings move to a new buffer whenever one fills up.
    pub fn submitted(&mut self) {
        self.vertices.reset();
        self.uniforms.reset();
        self.persistent.submitted();
    }

    /// Encodes `draws` (recorded for a target of this size and these
    /// formats) on `encoder`, applying `clear` first, and returns how many
    /// render passes that took: one render pass, or a
    /// run of passes split before each draw marked with
    /// [`RecordedDraw::start_pass`] and before each counted draw, which gets
    /// its count and sum passes first (as
    /// [`render_counted`](Self::render_counted)).
    pub fn encode_pass(
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: Clear,
        draws: &[RecordedDraw],
        mut counter: Option<&mut PixelCounter>,
    ) -> Result<usize, DrawError> {
        if counter.is_none() && draws.iter().any(RecordedDraw::counts_samples) {
            return Err(DrawError::NoPixelCounter);
        }
        wait_for_pipelines(draws);
        let has_stencil = target.depth_format.is_some_and(|f| f.has_stencil_aspect());
        let color_view = target.color;
        let attachment = {
            let t = color_view.texture();
            (t.width(), t.height())
        };
        // The clear goes with the first pass that has the attachment.
        let mut color_clear = clear.color;
        let mut depth_clear = clear.depth;
        let mut stencil_clear = clear.stencil;
        let depth_ops = |depth: Option<f32>, stencil: Option<u8>| {
            target
                .depth
                .map(|view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: depth.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: has_stencil.then_some(wgpu::Operations {
                        load: stencil.map_or(wgpu::LoadOp::Load, |s| wgpu::LoadOp::Clear(s as u32)),
                        store: wgpu::StoreOp::Store,
                    }),
                })
        };
        // Runs of draws, each starting at the start or at a counted draw.
        let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
        for (i, p) in draws.iter().enumerate() {
            match runs.last_mut() {
                Some(r) if !p.counts_samples() && !p.new_pass => r.end = i + 1,
                _ => runs.push(i..i + 1),
            }
        }
        if runs.is_empty() {
            runs.push(0..0);
        }
        let passes = runs.len();
        for run in runs {
            if let Some(p) = draws.get(run.start)
                && let Some((count, writes)) = &p.count
            {
                let counter = counter.as_deref_mut().expect("checked above");
                let samples = target.samples;
                let count_view = counter.target(ctx, attachment, samples).clone();
                // A replay that writes depth or stencil runs on a copy (see
                // crate::occlusion); a pending clear is applied to the copy
                // and stays pending for the real pass. One that writes
                // neither replays on the attachment itself, which then
                // takes the pending clear.
                let depth_copy = target
                    .depth
                    .filter(|_| *writes)
                    .map(|v| counter.depth_copy(ctx, encoder, v.texture()).clone());
                let in_place = depth_copy.is_none() && target.depth.is_some();
                let (count_depth, count_stencil) = if in_place {
                    (depth_clear.take(), stencil_clear.take())
                } else {
                    (depth_clear, stencil_clear)
                };
                let depth_store = if in_place {
                    wgpu::StoreOp::Store
                } else {
                    wgpu::StoreOp::Discard
                };
                {
                    crate::churn::pass();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("gfx count pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &count_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: depth_copy.as_ref().or(target.depth).map(
                            |view| wgpu::RenderPassDepthStencilAttachment {
                                view,
                                depth_ops: Some(wgpu::Operations {
                                    load: count_depth
                                        .map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                                    store: depth_store,
                                }),
                                stencil_ops: has_stencil.then_some(wgpu::Operations {
                                    load: count_stencil.map_or(wgpu::LoadOp::Load, |s| {
                                        wgpu::LoadOp::Clear(s as u32)
                                    }),
                                    store: depth_store,
                                }),
                            },
                        ),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    if let Some(count) = count.get() {
                        PassState::default().draw(&mut pass, p, count);
                    }
                }
                if !p.scissor.is_empty() {
                    let s = p.scissor;
                    counter.sum(
                        ctx,
                        encoder,
                        attachment,
                        samples,
                        [s.x, s.y, s.width, s.height],
                    );
                }
            }
            let color_load = match color_clear.take() {
                Some([r, g, b, a]) => wgpu::LoadOp::Clear(wgpu::Color {
                    r: r as f64,
                    g: g as f64,
                    b: b as f64,
                    a: a as f64,
                }),
                None => wgpu::LoadOp::Load,
            };
            crate::churn::pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gfx pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: color_load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: depth_ops(depth_clear.take(), stencil_clear.take()),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut state = PassState::default();
            for p in &draws[run] {
                if let Some(pipeline) = p.pipeline.get() {
                    state.draw(&mut pass, p, pipeline);
                }
            }
        }
        Ok(passes)
    }
}

/// The state an open render pass has had set, so a draw only sets what
/// differs from the draw before it. A pipeline switch keeps bind groups wgpu
/// has already validated against a compatible layout, so groups are
/// compared by value.
#[derive(Default)]
struct PassState<'a> {
    last: Option<(&'a RecordedDraw, &'a wgpu::RenderPipeline)>,
    groups: [Option<(&'a wgpu::BindGroup, &'a [u32])>; 4],
}

impl<'a> PassState<'a> {
    /// Encodes `p` with `pipeline` (its own, or its count pipeline).
    fn draw(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        p: &'a RecordedDraw,
        pipeline: &'a wgpu::RenderPipeline,
    ) {
        // The attachments may be larger than the logical target (a depth
        // surface bigger than the colour surface): the viewport and scissor
        // were checked against, and clamped to, the logical size.
        if p.scissor.is_empty() {
            return;
        }
        let last = self.last.map(|(l, _)| l);
        let v = p.viewport;
        if last.is_none_or(|l| l.viewport != v) {
            pass.set_viewport(v.x, v.y, v.width, v.height, v.min_depth, v.max_depth);
        }
        let s = p.scissor;
        if last.is_none_or(|l| l.scissor != s) {
            pass.set_scissor_rect(s.x, s.y, s.width, s.height);
        }
        if self.last.is_none_or(|(_, l)| l != pipeline) {
            pass.set_pipeline(pipeline);
        }
        let builtin = p
            .builtin
            .as_ref()
            .map(|(group, offset)| (0, group, std::slice::from_ref(offset)));
        let extra = p
            .bind_groups
            .iter()
            .map(|g| (g.index, &g.group, g.offsets.as_slice()));
        for (index, group, offsets) in builtin.into_iter().chain(extra) {
            match self.groups.get_mut(index as usize) {
                Some(Some((g, o))) if *g == group && *o == offsets => {}
                slot => {
                    pass.set_bind_group(index, group, offsets);
                    if let Some(slot) = slot {
                        *slot = Some((group, offsets));
                    }
                }
            }
        }
        if last.is_none_or(|l| l.blend_constant != p.blend_constant) {
            pass.set_blend_constant(p.blend_constant);
        }
        if last.is_none_or(|l| l.stencil_ref != p.stencil_ref) {
            pass.set_stencil_reference(p.stencil_ref);
        }
        // Every draw stages its own vertices and indices.
        for (i, (buf, off, len)) in p.vertex_buffers.iter().enumerate() {
            pass.set_vertex_buffer(i as u32, buf.slice(*off..*off + *len));
        }
        match &p.indices {
            None => pass.draw(0..p.vertices, 0..1),
            Some((buf, off, len)) => {
                pass.set_index_buffer(buf.slice(*off..*off + *len), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..p.vertices, 0, 0..1);
            }
        }
        self.last = Some((p, pipeline));
    }
}

/// Whether a draw with `ds` can change the depth/stencil attachment.
fn writes_depth_stencil(ds: &wgpu::DepthStencilState) -> bool {
    let face_writes = |f: &wgpu::StencilFaceState| {
        [f.fail_op, f.depth_fail_op, f.pass_op]
            .iter()
            .any(|op| *op != wgpu::StencilOperation::Keep)
    };
    ds.depth_write_enabled == Some(true)
        || (ds.stencil.write_mask != 0
            && ds.format.has_stencil_aspect()
            && (face_writes(&ds.stencil.front) || face_writes(&ds.stencil.back)))
}

/// A count-pass pipeline for `key` (see [`crate::occlusion`]): the draw's
/// vertex stage, primitive state and depth/stencil state (writes included;
/// the pass runs on a scratch copy of the attachment), into a
/// [`COUNT_FORMAT`] target that adds 1 per fragment.
fn create_count_pipeline(
    device: &wgpu::Device,
    key: &PipelineKey,
    program: &ShaderProgram,
    fragment: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = key
        .vertex_buffers
        .iter()
        .map(|b| Some(b.as_wgpu()))
        .collect();
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let targets = [Some(wgpu::ColorTargetState {
        format: COUNT_FORMAT,
        blend: Some(wgpu::BlendState {
            color: add,
            alpha: add,
        }),
        write_mask: wgpu::ColorWrites::RED,
    })];
    let depth_stencil = key.depth_stencil.clone();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gfx count pass"),
        layout: Some(&program.layout),
        vertex: wgpu::VertexState {
            module: &program.module,
            entry_point: Some(&program.vs_entry),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: key.primitive,
        depth_stencil,
        multisample: wgpu::MultisampleState {
            count: key.samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: if program.count_entry.is_some() {
                &program.module
            } else {
                fragment
            },
            entry_point: Some(program.count_entry.as_deref().unwrap_or("fs_count")),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A draw pipeline for `key` to be built on a worker (or its first user).
fn pending_pipeline(
    device: &wgpu::Device,
    key: &PipelineKey,
    program: &ShaderProgram,
) -> Arc<PendingPipeline> {
    let (device, key, program) = (device.clone(), key.clone(), program.clone());
    Pending::new(move || {
        validated(&device, || {
            create_render_pipeline(&device, &key, &program, "xbox draw")
        })
    })
}

/// Runs `build` inside a validation error scope of its own, on a compile
/// worker or the drawing thread alike (scopes are per thread). A pipeline
/// that fails validation is reported once and comes back as `None`, so its
/// draws are skipped: an invalid pipeline reaching `set_pipeline` panics
/// inside wgpu and took the guest thread down with it.
fn validated(
    device: &wgpu::Device,
    build: impl FnOnce() -> wgpu::RenderPipeline,
) -> Option<wgpu::RenderPipeline> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let p = build();
    if let Some(e) = pollster::block_on(scope.pop()) {
        eprintln!("gfx: pipeline compile (its draws are skipped): {e}");
        return None;
    }
    Some(p)
}

/// The count pipeline for `key`, likewise.
fn pending_count(
    device: &wgpu::Device,
    key: &PipelineKey,
    program: &ShaderProgram,
    fragment: &wgpu::ShaderModule,
) -> Arc<PendingPipeline> {
    let (device, key, program, fragment) = (
        device.clone(),
        key.clone(),
        program.clone(),
        fragment.clone(),
    );
    Pending::new(move || {
        validated(&device, || {
            create_count_pipeline(&device, &key, &program, &fragment)
        })
    })
}

/// The count pass's fragment module, made on first use.
fn count_fragment(
    slot: &mut Option<wgpu::ShaderModule>,
    device: &wgpu::Device,
) -> wgpu::ShaderModule {
    slot.get_or_insert_with(|| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx count fragment"),
            source: wgpu::ShaderSource::Wgsl(COUNT_FRAGMENT_WGSL.into()),
        })
    })
    .clone()
}

/// Makes sure every pipeline `draws` use is built, before a pass borrows
/// them. A pipeline that is not is a compile on the game thread's critical
/// path: it is counted ([`timeline::phase::Counter::Pipelines`]) and timed
/// (`pipeline wait`), and built here if no worker has started it.
fn wait_for_pipelines(draws: &[RecordedDraw]) {
    let pending = || {
        draws
            .iter()
            .flat_map(|d| std::iter::once(&d.pipeline).chain(d.count.as_ref().map(|c| &c.0)))
            .filter(|p| !p.is_ready())
    };
    if pending().next().is_none() {
        return;
    }
    let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "pipeline wait");
    let mut n = 0;
    for p in pending() {
        if !p.is_ready() {
            n += 1;
            p.get();
        }
    }
    timeline::phase::count(timeline::phase::Counter::Pipelines, n);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scissor_maths() {
        let full = ScissorRect::full(64, 48);
        assert!(full.covers(64, 48));
        assert!(!ScissorRect::from_edges(0, 0, 63, 48).covers(64, 48));
        assert!(ScissorRect::from_edges(-5, -5, 100, 100).covers(64, 48));
        let r = ScissorRect::from_edges(10, 20, 30, 25).intersect(full);
        assert_eq!(
            r,
            ScissorRect {
                x: 10,
                y: 20,
                width: 20,
                height: 5
            }
        );
        assert!(
            ScissorRect::from_edges(70, 0, 80, 10)
                .intersect(full)
                .is_empty()
        );
        assert!(ScissorRect::from_edges(30, 0, 10, 10).is_empty());
    }

    #[test]
    fn viewport_bounds() {
        assert!(DrawViewport::full(64, 48).valid_in(64, 48));
        let mut v = DrawViewport::full(64, 48);
        v.x = 1.0;
        assert!(!v.valid_in(64, 48));
        v.width = 63.0;
        assert!(v.valid_in(64, 48));
        v.min_depth = 0.9;
        v.max_depth = 0.1;
        assert!(!v.valid_in(64, 48));
    }
}
