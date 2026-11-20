//! Offscreen wgpu renderer for the comparison, with per-pass GPU timing.
//!
//! Techniques, each into its own colour and mask targets:
//!
//! - `none`: one shading pass with no shadow test. The floor every other
//!   technique is measured against.
//! - `volume`: depth-fail stencil shadow volumes. Ambient pass (depth and
//!   stencil cleared, depth written), stencil pass (volumes, two-sided
//!   increment/decrement on depth fail, no colour), light pass (additive,
//!   depth equal, stencil equal 0).
//! - `map-hard` and `map-pcf`: a depth map from the light (one fitted
//!   orthographic map for the sun, six cube faces for a point light), then a
//!   shading pass sampling it with one nearest comparison tap, or with nine
//!   bilinear comparison taps (a 3x3 PCF kernel, 4x4 texels in effect).
//!
//! Timing uses timestamp queries at pass boundaries when the adapter has
//! them; otherwise it falls back to wall time per technique (submit and wait),
//! which includes driver overhead and is labelled so.
use crate::geom::{M4, V3};
use crate::volume::Light;
use anyhow::{Context, Result, bail};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SceneVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

/// Flat-shaded vertices (three per triangle, face normal on each).
pub fn flat_vertices(soup: &[[V3; 3]], color: V3) -> Vec<SceneVertex> {
    use crate::geom::{cross, normalize, sub};
    soup.iter()
        .flat_map(|t| {
            let n = normalize(cross(sub(t[1], t[0]), sub(t[2], t[0])));
            t.map(|p| SceneVertex {
                pos: p,
                normal: n,
                color,
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Technique {
    None,
    Volume,
    MapHard,
    MapPcf,
}
impl Technique {
    pub const ALL: [Technique; 4] = [
        Technique::None,
        Technique::Volume,
        Technique::MapHard,
        Technique::MapPcf,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Technique::None => "none",
            Technique::Volume => "volume",
            Technique::MapHard => "map-hard",
            Technique::MapPcf => "map-pcf",
        }
    }
    fn mode(self) -> f32 {
        match self {
            Technique::None | Technique::Volume => 0.0,
            Technique::MapHard => 1.0,
            Technique::MapPcf => 2.0,
        }
    }
}

/// How the light's shadow map is projected. Small enough to copy around;
/// the cube variant's six matrices are the bulk of it.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub enum ShadowProjection {
    Sun {
        matrix: M4,
        size: u32,
        texel_world: f32,
    },
    Cube {
        matrices: [M4; 6],
        size: u32,
        texel_world: f32,
    },
}
impl ShadowProjection {
    pub fn size(&self) -> u32 {
        match *self {
            ShadowProjection::Sun { size, .. } | ShadowProjection::Cube { size, .. } => size,
        }
    }
    fn layers(&self) -> u32 {
        match self {
            ShadowProjection::Sun { .. } => 1,
            ShadowProjection::Cube { .. } => 6,
        }
    }
    fn matrix(&self, i: usize) -> M4 {
        match self {
            ShadowProjection::Sun { matrix, .. } => *matrix,
            ShadowProjection::Cube { matrices, .. } => matrices[i],
        }
    }
    pub fn texel_world(&self) -> f32 {
        match *self {
            ShadowProjection::Sun { texel_world, .. }
            | ShadowProjection::Cube { texel_world, .. } => texel_world,
        }
    }
}

/// One light to render the scene under.
pub struct LightRun<'a> {
    pub light: Light,
    pub color: V3,
    /// Point-light range for falloff; 0 means none.
    pub range: f32,
    /// The volume, expanded (three `vec4` per triangle).
    pub volume: &'a [[f32; 4]],
    pub shadow: ShadowProjection,
    /// World distance shadow-map lookups are pushed along the normal.
    pub normal_offset: f32,
    /// World distance volume caps are pushed away from the light.
    pub volume_offset: f32,
}

/// The camera for the main passes.
pub struct View {
    pub view_proj: M4,
    pub eye: V3,
    pub size: (u32, u32),
}

/// One technique's images and pass times.
pub struct TechniqueOutput {
    pub technique: Technique,
    pub color: Vec<u8>,
    pub mask: Vec<u8>,
    /// Median milliseconds per pass, in pass order.
    pub passes: Vec<(&'static str, f64)>,
}
impl TechniqueOutput {
    pub fn total_ms(&self) -> f64 {
        self.passes.iter().map(|p| p.1).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// Timestamp queries at pass boundaries.
    Gpu,
    /// Wall time around submit-and-wait, per technique.
    Wall,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    view_proj: M4,
    eye: [f32; 4],
    light_pos: [f32; 4],
    light_color: [f32; 4],
    /// x shadow mode, y PCF tap spacing (uv), z normal offset, w volume offset.
    params: [f32; 4],
    ambient: [f32; 4],
    shadow: [M4; 6],
}

const FRAME_STRIDE: u64 = 768; // >= size_of::<Frame>(), 256-aligned
const CASTER_STRIDE: u64 = 256;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const MASK: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
const SHADOW: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_QUERIES: u32 = 64;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.05,
    g: 0.06,
    b: 0.08,
    a: 1.0,
};

const SHADER: &str = r#"
struct Frame {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light_pos: vec4<f32>,
    light_color: vec4<f32>,
    params: vec4<f32>,
    ambient: vec4<f32>,
    shadow: array<mat4x4<f32>, 6>,
};
@group(0) @binding(0) var<uniform> u: Frame;
@group(0) @binding(1) var smap: texture_depth_2d_array;
@group(0) @binding(2) var hard: sampler_comparison;
@group(0) @binding(3) var soft: sampler_comparison;

struct V {
    @builtin(position) @invariant pos: vec4<f32>,
    @location(0) w: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) c: vec3<f32>,
};
@vertex fn vs_scene(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec3<f32>) -> V {
    var o: V;
    o.pos = u.view_proj * vec4<f32>(p, 1.0);
    o.w = p; o.n = n; o.c = c;
    return o;
}

fn point_light() -> bool { return u.light_pos.w > 0.5; }
fn to_light(w: vec3<f32>) -> vec3<f32> {
    if (point_light()) { return u.light_pos.xyz - w; }
    return -u.light_pos.xyz;
}
fn extrude(p: vec3<f32>) -> vec3<f32> {
    if (point_light()) { return normalize(p - u.light_pos.xyz); }
    return normalize(u.light_pos.xyz);
}
fn facing(i: V) -> bool { return dot(i.n, to_light(i.w)) > 0.0; }
fn diffuse(i: V) -> vec3<f32> {
    let l = to_light(i.w);
    let nl = max(dot(i.n, normalize(l)), 0.0);
    var att = 1.0;
    if (point_light() && u.light_color.w > 0.0) {
        let d = length(l) / u.light_color.w;
        att = clamp(1.0 - d * d, 0.0, 1.0);
    }
    return u.light_color.rgb * i.c * nl * att;
}

// Same rule as smap::cube_face_of.
fn face_of(d: vec3<f32>) -> u32 {
    let a = abs(d);
    if (a.x >= a.y && a.x >= a.z) { return select(1u, 0u, d.x >= 0.0); }
    if (a.y >= a.z) { return select(3u, 2u, d.y >= 0.0); }
    return select(5u, 4u, d.z >= 0.0);
}

fn shadow(i: V) -> f32 {
    let mode = u.params.x;
    if (mode < 0.5) { return 1.0; }
    let p = i.w + i.n * u.params.z;
    var layer = 0u;
    if (point_light()) { layer = face_of(p - u.light_pos.xyz); }
    let c = u.shadow[layer] * vec4<f32>(p, 1.0);
    let ndc = c.xyz / c.w;
    if (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) { return 1.0; }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (mode < 1.5) {
        return textureSampleCompareLevel(smap, hard, uv, layer, ndc.z);
    }
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * u.params.y;
            sum += textureSampleCompareLevel(smap, soft, uv + o, layer, ndc.z);
        }
    }
    return sum / 9.0;
}

struct Out { @location(0) color: vec4<f32>, @location(1) mask: vec4<f32> };
fn mask_of(i: V, lit: f32) -> vec4<f32> {
    if (!facing(i)) { return vec4<f32>(0.0); }
    return vec4<f32>(mix(128.0, 255.0, lit) / 255.0);
}
@fragment fn fs_ambient(i: V) -> Out {
    return Out(vec4<f32>(u.ambient.rgb * i.c, 1.0), mask_of(i, 0.0));
}
@fragment fn fs_light(i: V) -> Out {
    return Out(vec4<f32>(diffuse(i), 1.0), mask_of(i, 1.0));
}
@fragment fn fs_shade(i: V) -> Out {
    let s = shadow(i);
    return Out(vec4<f32>(u.ambient.rgb * i.c + diffuse(i) * s, 1.0), mask_of(i, s));
}

@vertex fn vs_volume(@location(0) p: vec4<f32>) -> @builtin(position) @invariant vec4<f32> {
    if (p.w > 0.5) {
        // Push the caps a hair away from the light so a lit face never
        // z-fights its own cap.
        return u.view_proj * vec4<f32>(p.xyz + extrude(p.xyz) * u.params.w, 1.0);
    }
    return u.view_proj * p;
}
@fragment fn fs_volume() -> Out { return Out(vec4<f32>(0.0), vec4<f32>(0.0)); }
"#;

const CASTER_SHADER: &str = r#"
struct Caster { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> caster: Caster;
@vertex fn vs_caster(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return caster.view_proj * vec4<f32>(p, 1.0);
}
"#;

pub struct Lab {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub clock: Clock,
    pub adapter_name: String,
    pub max_texture: u32,
    period_ns: f32,
    main_layout: wgpu::BindGroupLayout,
    caster_layout: wgpu::BindGroupLayout,
    ambient: wgpu::RenderPipeline,
    stencil: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    shade: wgpu::RenderPipeline,
    caster: wgpu::RenderPipeline,
    hard: wgpu::Sampler,
    soft: wgpu::Sampler,
    scene: wgpu::Buffer,
    scene_count: u32,
    casters: std::ops::Range<u32>,
}

/// A colour attachment, cleared to `clear` or loaded.
fn pass_color(
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: match clear {
                Some(c) => wgpu::LoadOp::Clear(c),
                None => wgpu::LoadOp::Load,
            },
            store: wgpu::StoreOp::Store,
        },
    })
}

struct Targets {
    color: wgpu::Texture,
    mask: wgpu::Texture,
    color_view: wgpu::TextureView,
    mask_view: wgpu::TextureView,
}

impl Lab {
    /// A headless device, with timestamp queries when the adapter has them.
    pub fn new() -> Result<Self> {
        pollster::block_on(Self::new_async())
    }
    async fn new_async() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no GPU adapter")?;
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let limits = adapter.limits();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("shadowlab"),
                required_features: features,
                required_limits: limits.clone(),
                ..Default::default()
            })
            .await?;
        let clock = if features.contains(wgpu::Features::TIMESTAMP_QUERY) {
            Clock::Gpu
        } else {
            Clock::Wall
        };
        let period_ns = queue.get_timestamp_period();
        let info = adapter.get_info();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadowlab"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let caster_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadowlab caster"),
            source: wgpu::ShaderSource::Wgsl(CASTER_SHADER.into()),
        });
        let main_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadowlab main"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadowlab caster"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let main_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&main_layout)],
            immediate_size: 0,
        });
        let caster_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&caster_layout)],
            immediate_size: 0,
        });
        let scene_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
        let scene_buf = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SceneVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &scene_attrs,
        };
        let volume_attrs = wgpu::vertex_attr_array![0 => Float32x4];
        let volume_buf = wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &volume_attrs,
        };
        let caster_attrs = wgpu::vertex_attr_array![0 => Float32x3];
        let caster_buf = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SceneVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &caster_attrs,
        };
        let keep = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Keep,
        };
        let stencil_off = wgpu::StencilState {
            front: keep,
            back: keep,
            read_mask: 0xff,
            write_mask: 0xff,
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let targets = |blend: Option<wgpu::BlendState>, write: wgpu::ColorWrites| {
            [
                Some(wgpu::ColorTargetState {
                    format: COLOR,
                    blend,
                    write_mask: write,
                }),
                Some(wgpu::ColorTargetState {
                    format: MASK,
                    blend: None,
                    write_mask: write,
                }),
            ]
        };
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buf: &wgpu::VertexBufferLayout<'_>,
                    cull: Option<wgpu::Face>,
                    depth: wgpu::DepthStencilState,
                    color: [Option<wgpu::ColorTargetState>; 2]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&main_pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(buf.clone())],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &color,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let depth = |write: bool, compare: wgpu::CompareFunction, stencil: wgpu::StencilState| {
            wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil,
                bias: Default::default(),
            }
        };
        let all = wgpu::ColorWrites::ALL;
        // Scene passes draw both faces: level and model meshes are not all
        // closed, and the shading uses the face normal either way.
        let ambient = make(
            "ambient",
            "vs_scene",
            "fs_ambient",
            &scene_buf,
            None,
            depth(true, wgpu::CompareFunction::Less, stencil_off.clone()),
            targets(None, all),
        );
        let shade = make(
            "shade",
            "vs_scene",
            "fs_shade",
            &scene_buf,
            None,
            depth(true, wgpu::CompareFunction::Less, stencil_off.clone()),
            targets(None, all),
        );
        let face = |op| wgpu::StencilFaceState {
            depth_fail_op: op,
            ..keep
        };
        let stencil = make(
            "volume stencil",
            "vs_volume",
            "fs_volume",
            &volume_buf,
            None,
            depth(
                false,
                wgpu::CompareFunction::Less,
                wgpu::StencilState {
                    front: face(wgpu::StencilOperation::DecrementWrap),
                    back: face(wgpu::StencilOperation::IncrementWrap),
                    read_mask: 0xff,
                    write_mask: 0xff,
                },
            ),
            targets(None, wgpu::ColorWrites::empty()),
        );
        let equal = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Equal,
            ..keep
        };
        let light = make(
            "volume light",
            "vs_scene",
            "fs_light",
            &scene_buf,
            None,
            depth(
                false,
                wgpu::CompareFunction::Equal,
                wgpu::StencilState {
                    front: equal,
                    back: equal,
                    read_mask: 0xff,
                    write_mask: 0,
                },
            ),
            [
                Some(wgpu::ColorTargetState {
                    format: COLOR,
                    blend: Some(additive),
                    write_mask: all,
                }),
                Some(wgpu::ColorTargetState {
                    format: MASK,
                    blend: None,
                    write_mask: all,
                }),
            ],
        );
        let caster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow map"),
            layout: Some(&caster_pl),
            vertex: wgpu::VertexState {
                module: &caster_module,
                entry_point: Some("vs_caster"),
                compilation_options: Default::default(),
                buffers: &[Some(caster_buf)],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let sampler = |filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            })
        };
        let hard = sampler(wgpu::FilterMode::Nearest);
        let soft = sampler(wgpu::FilterMode::Linear);
        let scene = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            clock,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            max_texture: limits.max_texture_dimension_2d,
            period_ns,
            main_layout,
            caster_layout,
            ambient,
            stencil,
            light,
            shade,
            caster,
            hard,
            soft,
            scene,
            scene_count: 0,
            casters: 0..0,
        })
    }

    /// Upload the scene: receivers first, then casters. The shadow maps draw
    /// the casters, or everything when `receivers_cast`.
    pub fn set_scene(
        &mut self,
        receivers: &[SceneVertex],
        casters: &[SceneVertex],
        receivers_cast: bool,
    ) {
        let mut all = receivers.to_vec();
        all.extend_from_slice(casters);
        if all.is_empty() {
            all.push(SceneVertex {
                pos: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                color: [0.0; 3],
            });
        }
        self.scene = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene"),
                contents: bytemuck::cast_slice(&all),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.scene_count = (receivers.len() + casters.len()) as u32;
        let first = if receivers_cast {
            0
        } else {
            receivers.len() as u32
        };
        self.casters = first..self.scene_count;
    }

    fn targets(&self, size: (u32, u32)) -> Targets {
        let make = |format, label| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let color = make(COLOR, "color");
        let mask = make(MASK, "mask");
        Targets {
            color_view: color.create_view(&Default::default()),
            mask_view: mask.create_view(&Default::default()),
            color,
            mask,
        }
    }

    /// Render every technique under one light, `frames` times, and return
    /// each technique's last images and median pass times.
    pub fn run(&self, view: &View, l: &LightRun<'_>, frames: u32) -> Result<Vec<TechniqueOutput>> {
        let d = &self.device;
        let size = view.size;
        if size.0 > self.max_texture || size.1 > self.max_texture {
            bail!(
                "{}x{} exceeds the device's {} texture limit",
                size.0,
                size.1,
                self.max_texture
            );
        }
        let smap_size = l.shadow.size();
        if smap_size > self.max_texture {
            bail!(
                "shadow map {smap_size} exceeds the device's {} limit",
                self.max_texture
            );
        }
        let ds = d
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth stencil"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let layers = l.shadow.layers();
        let shadow_tex = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: smap_size,
                height: smap_size,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_all = shadow_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            array_layer_count: Some(layers),
            ..Default::default()
        });
        let shadow_layers: Vec<wgpu::TextureView> = (0..layers)
            .map(|i| {
                shadow_tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        // Uniforms: one frame slot per technique, one caster slot per layer.
        let mut shadow = [crate::geom::IDENTITY; 6];
        for (i, m) in shadow.iter_mut().enumerate().take(layers as usize) {
            *m = l.shadow.matrix(i);
        }
        let (light_pos, range) = match l.light {
            Light::Directional { dir } => {
                let n = crate::geom::normalize(dir);
                ([n[0], n[1], n[2], 0.0], 0.0)
            }
            Light::Point { pos } => ([pos[0], pos[1], pos[2], 1.0], l.range),
        };
        let mut frame_bytes = vec![0u8; FRAME_STRIDE as usize * Technique::ALL.len()];
        for (i, t) in Technique::ALL.iter().enumerate() {
            let f = Frame {
                view_proj: view.view_proj,
                eye: [view.eye[0], view.eye[1], view.eye[2], 1.0],
                light_pos,
                light_color: [l.color[0], l.color[1], l.color[2], range],
                params: [
                    t.mode(),
                    1.0 / smap_size as f32,
                    l.normal_offset,
                    l.volume_offset,
                ],
                ambient: [0.22, 0.24, 0.3, 0.0],
                shadow,
            };
            let at = i * FRAME_STRIDE as usize;
            frame_bytes[at..at + std::mem::size_of::<Frame>()]
                .copy_from_slice(bytemuck::bytes_of(&f));
        }
        let frame_ub = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("frame"),
            contents: &frame_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut caster_bytes = vec![0u8; CASTER_STRIDE as usize * 6];
        for (i, m) in shadow.iter().enumerate() {
            let at = i * CASTER_STRIDE as usize;
            caster_bytes[at..at + 64].copy_from_slice(bytemuck::bytes_of(m));
        }
        let caster_ub = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("casters"),
            contents: &caster_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let main_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.main_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &frame_ub,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Frame>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_all),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.hard),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.soft),
                },
            ],
        });
        let caster_bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &caster_ub,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });
        let volume_vb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("volume"),
            contents: if l.volume.is_empty() {
                &[0u8; 16]
            } else {
                bytemuck::cast_slice(l.volume)
            },
            usage: wgpu::BufferUsages::VERTEX,
        });
        let volume_count = l.volume.len() as u32;
        let targets: Vec<Targets> = Technique::ALL.iter().map(|_| self.targets(size)).collect();

        let queries = (self.clock == Clock::Gpu).then(|| {
            let set = d.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_QUERIES,
            });
            let resolve = d.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(MAX_QUERIES) * 8,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = d.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(MAX_QUERIES) * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            (set, resolve, read)
        });

        // samples[technique][pass] = per-frame milliseconds.
        let mut names: Vec<Vec<&'static str>> = vec![Vec::new(); Technique::ALL.len()];
        let mut samples: Vec<Vec<Vec<f64>>> = vec![Vec::new(); Technique::ALL.len()];
        for _ in 0..frames.max(1) {
            let mut slot = 0u32;
            let mut enc = d.create_command_encoder(&Default::default());
            let mut frame_names: Vec<Vec<(&'static str, u32)>> =
                vec![Vec::new(); Technique::ALL.len()];
            for (ti, &t) in Technique::ALL.iter().enumerate() {
                let wall = std::time::Instant::now();
                if self.clock == Clock::Wall {
                    enc = d.create_command_encoder(&Default::default());
                }
                let offset = (ti as u64 * FRAME_STRIDE) as u32;
                let tg = &targets[ti];
                let mut ts = |name: &'static str| {
                    let q = queries
                        .as_ref()
                        .map(|(set, _, _)| wgpu::RenderPassTimestampWrites {
                            query_set: set,
                            beginning_of_pass_write_index: Some(slot),
                            end_of_pass_write_index: Some(slot + 1),
                        });
                    frame_names[ti].push((name, slot));
                    slot += 2;
                    q
                };
                let main_pass = |enc: &mut wgpu::CommandEncoder,
                                 label: &'static str,
                                 clear: bool,
                                 stamp: Option<wgpu::RenderPassTimestampWrites<'_>>|
                 -> wgpu::RenderPass<'static> {
                    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(label),
                        color_attachments: &[
                            pass_color(&tg.color_view, clear.then_some(BACKGROUND)),
                            // The mask clears to "faces away": not compared.
                            pass_color(&tg.mask_view, clear.then_some(wgpu::Color::TRANSPARENT)),
                        ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &ds,
                            depth_ops: Some(wgpu::Operations {
                                load: if clear {
                                    wgpu::LoadOp::Clear(1.0)
                                } else {
                                    wgpu::LoadOp::Load
                                },
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: Some(wgpu::Operations {
                                load: if clear {
                                    wgpu::LoadOp::Clear(0)
                                } else {
                                    wgpu::LoadOp::Load
                                },
                                store: wgpu::StoreOp::Store,
                            }),
                        }),
                        timestamp_writes: stamp,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime()
                };
                let draw_scene = |p: &mut wgpu::RenderPass<'_>, pipe: &wgpu::RenderPipeline| {
                    p.set_pipeline(pipe);
                    p.set_bind_group(0, &main_bg, &[offset]);
                    p.set_vertex_buffer(0, self.scene.slice(..));
                    p.draw(0..self.scene_count, 0..1);
                };
                if matches!(t, Technique::MapHard | Technique::MapPcf) {
                    for (layer, lv) in shadow_layers.iter().enumerate() {
                        let stamp = ts("map");
                        let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("shadow map"),
                            color_attachments: &[],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: lv,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: stamp,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        p.set_pipeline(&self.caster);
                        p.set_bind_group(0, &caster_bg, &[(layer as u64 * CASTER_STRIDE) as u32]);
                        p.set_vertex_buffer(0, self.scene.slice(..));
                        p.draw(self.casters.clone(), 0..1);
                    }
                }
                match t {
                    Technique::None | Technique::MapHard | Technique::MapPcf => {
                        let stamp = ts("shade");
                        let mut p = main_pass(&mut enc, "shade", true, stamp);
                        draw_scene(&mut p, &self.shade);
                    }
                    Technique::Volume => {
                        let stamp = ts("ambient");
                        let mut p = main_pass(&mut enc, "ambient", true, stamp);
                        draw_scene(&mut p, &self.ambient);
                        drop(p);
                        let stamp = ts("stencil");
                        let mut p = main_pass(&mut enc, "stencil", false, stamp);
                        p.set_pipeline(&self.stencil);
                        p.set_bind_group(0, &main_bg, &[offset]);
                        p.set_vertex_buffer(0, volume_vb.slice(..));
                        p.draw(0..volume_count, 0..1);
                        drop(p);
                        let stamp = ts("light");
                        let mut p = main_pass(&mut enc, "light", false, stamp);
                        p.set_stencil_reference(0);
                        draw_scene(&mut p, &self.light);
                    }
                }
                if self.clock == Clock::Wall {
                    let done =
                        std::mem::replace(&mut enc, d.create_command_encoder(&Default::default()));
                    self.queue.submit([done.finish()]);
                    d.poll(wgpu::PollType::wait_indefinitely())?;
                    let ms = wall.elapsed().as_secs_f64() * 1e3;
                    record(
                        &mut names[ti],
                        &mut samples[ti],
                        &[("technique (wall)", ms)],
                    );
                }
            }
            if let Some((set, resolve, read)) = &queries {
                enc.resolve_query_set(set, 0..slot, resolve, 0);
                enc.copy_buffer_to_buffer(resolve, 0, read, 0, u64::from(slot) * 8);
                self.queue.submit([enc.finish()]);
                let s = read.slice(..u64::from(slot) * 8);
                s.map_async(wgpu::MapMode::Read, |r| r.expect("map timestamps"));
                d.poll(wgpu::PollType::wait_indefinitely())?;
                let stamps: Vec<u64> = s
                    .get_mapped_range()
                    .expect("mapped timestamps")
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| u64::from_le_bytes(*b))
                    .collect();
                read.unmap();
                for (ti, list) in frame_names.iter().enumerate() {
                    let mut per: Vec<(&'static str, f64)> = Vec::new();
                    for &(name, q) in list {
                        let ticks = stamps[q as usize + 1].saturating_sub(stamps[q as usize]);
                        let ms = ticks as f64 * f64::from(self.period_ns) / 1e6;
                        // Cube faces add up into one "map" pass.
                        match per.iter_mut().find(|p| p.0 == name) {
                            Some(p) => p.1 += ms,
                            None => per.push((name, ms)),
                        }
                    }
                    record(&mut names[ti], &mut samples[ti], &per);
                }
            }
        }
        let mut out = Vec::new();
        for (ti, &t) in Technique::ALL.iter().enumerate() {
            let tg = &targets[ti];
            out.push(TechniqueOutput {
                technique: t,
                color: read_texture(d, &self.queue, &tg.color, 4, size)?,
                mask: read_texture(d, &self.queue, &tg.mask, 1, size)?,
                passes: names[ti]
                    .iter()
                    .zip(&samples[ti])
                    .map(|(n, s)| (*n, median(s)))
                    .collect(),
            });
        }
        Ok(out)
    }
}

fn record(
    names: &mut Vec<&'static str>,
    samples: &mut Vec<Vec<f64>>,
    frame: &[(&'static str, f64)],
) {
    for &(name, ms) in frame {
        match names.iter().position(|n| *n == name) {
            Some(i) => samples[i].push(ms),
            None => {
                names.push(name);
                samples.push(vec![ms]);
            }
        }
    }
}

pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let m = s.len() / 2;
    if s.len() % 2 == 1 {
        s[m]
    } else {
        0.5 * (s[m - 1] + s[m])
    }
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bpp: u32,
    (w, h): (u32, u32),
) -> Result<Vec<u8>> {
    let unpadded = w * bpp;
    let padded = unpadded.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let data = slice.get_mapped_range().expect("mapped range");
    let mut out = Vec::with_capacity((unpadded * h) as usize);
    for row in data.chunks(padded as usize).take(h as usize) {
        out.extend_from_slice(&row[..unpadded as usize]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_fits_its_slot() {
        assert!(std::mem::size_of::<Frame>() as u64 <= FRAME_STRIDE);
        assert_eq!(FRAME_STRIDE % 256, 0);
    }

    #[test]
    fn median_of_odd_and_even_lists() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn flat_vertices_carry_the_face_normal() {
        let v = flat_vertices(&[[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]], [1.0; 3]);
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|x| x.normal == [0.0, 0.0, 1.0]));
    }
}
