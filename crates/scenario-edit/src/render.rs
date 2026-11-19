//! The 3D viewport: the structure BSP and the placement markers, drawn with
//! wgpu into the viewport's own colour and depth textures, which the UI
//! (Dear ImGui) then shows as an image.
//!
//! Rendering offscreen (instead of inside the UI's render pass) keeps the
//! viewport independent of the shell's surface: the UI pass may have no
//! depth buffer and any MSAA count, and a pipeline must match both. No UI
//! toolkit is used here; `h2-editor` registers [`Viewport::ui_view`]
//! with `dear-imgui-wgpu`.
use crate::geometry::{Camera, Marker, Tri, Vec3, block_color, cross, marker_triangles, normalize};
use crate::scene::{Blend, Scene, SceneVertex};
use std::ops::Range;
use wgpu::util::DeviceExt;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// [`COLOR_FORMAT`] without the sRGB decode: [`Viewport::ui_view`].
pub const UI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Background colour (linear).
pub const CLEAR: [f64; 3] = [0.02, 0.025, 0.035];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// The glyph's centre: the vertex is pushed out from it as the glyph
    /// grows with distance ([`crate::geometry::glyph_scale`]). Equal to
    /// `position` for geometry that keeps its size.
    pub center: [f32; 3],
}

/// One placed copy of a mesh: model-space axes scaled, the translation,
/// and a tint (rgb multiplies; alpha mixes in the selection highlight).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub x: [f32; 4],
    pub y: [f32; 4],
    pub z: [f32; 4],
    pub t: [f32; 4],
    pub tint: [f32; 4],
}

impl Instance {
    pub const IDENTITY: Self = Self {
        x: [1.0, 0.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0, 0.0],
        z: [0.0, 0.0, 1.0, 0.0],
        t: [0.0, 0.0, 0.0, 1.0],
        tint: [1.0, 1.0, 1.0, 0.0],
    };

    /// A placement's transform, highlighted when selected.
    pub fn of(m: &Marker, selected: bool) -> Self {
        let s = m.scale;
        let [f, l, u] = m.axes;
        let col = |v: Vec3| [v[0] * s, v[1] * s, v[2] * s, 0.0];
        Self {
            x: col(f),
            y: col(l),
            z: col(u),
            t: [m.position[0], m.position[1], m.position[2], 1.0],
            tint: if selected {
                [1.0, 1.0, 1.0, 0.45]
            } else {
                [1.0, 1.0, 1.0, 0.0]
            },
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    /// The camera's right, up and forward axes, and (tan of half the
    /// vertical field of view, aspect, 0, 0): the sky's view rays.
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    lens: [f32; 4],
}

/// A sky behind a map with art: a gradient from the zenith to a pale
/// horizon, darker below it, along each pixel's view ray.
const SKY_SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>, eye: vec4<f32>,
    right: vec4<f32>, up: vec4<f32>, forward: vec4<f32>, lens: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
struct V { @builtin(position) clip: vec4<f32>, @location(0) ndc: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: V;
    o.clip = vec4<f32>(p, 1.0, 1.0);
    o.ndc = p;
    return o;
}
@fragment fn fs(v: V) -> @location(0) vec4<f32> {
    let t = u.lens.x;
    let d = normalize(u.forward.xyz + u.right.xyz * v.ndc.x * t * u.lens.y + u.up.xyz * v.ndc.y * t);
    let zenith = vec3<f32>(0.10, 0.20, 0.42);
    let horizon = vec3<f32>(0.52, 0.60, 0.70);
    let ground = vec3<f32>(0.16, 0.17, 0.19);
    var c: vec3<f32>;
    if (d.z >= 0.0) {
        c = mix(horizon, zenith, pow(d.z, 0.6));
    } else {
        c = mix(horizon, ground, pow(-d.z, 0.4));
    }
    return vec4<f32>(c, 1.0);
}
"#;

const SHADER: &str = r#"
struct U { view_proj: mat4x4<f32>, eye: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct V { @builtin(position) clip: vec4<f32>, @location(0) n: vec3<f32>, @location(1) c: vec3<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec3<f32>, @location(3) center: vec3<f32>) -> V {
    var o: V;
    let s = max(1.0, length(center - u.eye.xyz) / GLYPH_DISTANCE);
    o.clip = u.view_proj * vec4<f32>(center + (p - center) * s, 1.0);
    o.n = n;
    o.c = c;
    return o;
}
@fragment fn fs(v: V) -> @location(0) vec4<f32> {
    let l = normalize(vec3<f32>(0.4, 0.3, 0.85));
    let d = abs(dot(normalize(v.n), l));
    return vec4<f32>(v.c * (0.3 + 0.7 * d), 1.0);
}
"#;

/// Textured meshes: placed per instance, lit by one sun and the sky.
const SCENE_SHADER: &str = r#"
struct U { view_proj: mat4x4<f32>, eye: vec4<f32> };
struct P { lift: f32, lit: f32, pad0: f32, pad1: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var lm: texture_2d<f32>;
@group(1) @binding(3) var<uniform> p: P;
struct V {
    @builtin(position) clip: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) luv: vec2<f32>,
};
@vertex fn vs(
    @location(0) pos: vec3<f32>, @location(1) n: vec3<f32>, @location(2) uv: vec2<f32>,
    @location(8) luv: vec2<f32>,
    @location(3) x: vec4<f32>, @location(4) y: vec4<f32>, @location(5) z: vec4<f32>,
    @location(6) t: vec4<f32>, @location(7) tint: vec4<f32>,
) -> V {
    var o: V;
    let m = mat4x4<f32>(x, y, z, t);
    o.clip = u.view_proj * m * vec4<f32>(pos, 1.0);
    o.n = (m * vec4<f32>(n, 0.0)).xyz;
    o.uv = uv;
    o.luv = luv;
    o.tint = tint;
    return o;
}
// Roll off highlights (snow, sand) instead of clipping them.
fn roll(lin: vec3<f32>) -> vec3<f32> {
    return lin / (vec3<f32>(1.0) + max(lin - vec3<f32>(0.7), vec3<f32>(0.0)));
}
fn lit(v: V, albedo: vec3<f32>) -> vec3<f32> {
    var light: vec3<f32>;
    if (p.lit > 0.5) {
        // The baked lightmap, as the game draws it: albedo × light × 2
        // in gamma space (LIGHTMAP_SCALE).
        // A faint fill light keeps the few truly unlit texels from
        // going pure black.
        light = textureSample(lm, samp, v.luv).rgb * LIGHTMAP_SCALE + vec3<f32>(LIGHTMAP_FILL);
    } else {
        // One sun and the sky; dark base maps are lifted (`p.lift`)
        // because the game would draw them under detail maps and light.
        let n = normalize(v.n + vec3<f32>(0.0, 0.0, 1e-5));
        let sun = normalize(vec3<f32>(0.45, 0.3, 0.84));
        let d = max(dot(n, sun), 0.0);
        let sky = 0.5 + 0.5 * n.z;
        light = vec3<f32>(1.4 * (0.36 + 0.14 * sky + 0.5 * d) * p.lift);
    }
    let c = roll(albedo * light * v.tint.rgb);
    return mix(c, vec3<f32>(1.0, 0.78, 0.25), v.tint.a);
}
@fragment fn fs_opaque(v: V) -> @location(0) vec4<f32> {
    return vec4<f32>(lit(v, textureSample(tex, samp, v.uv).rgb), 1.0);
}
@fragment fn fs_mask(v: V) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv);
    if (c.a < 0.5) { discard; }
    return vec4<f32>(lit(v, c.rgb), 1.0);
}
@fragment fn fs_translucent(v: V) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv);
    return vec4<f32>(c.rgb * (0.6 + 0.4 * v.tint.rgb), 0.3);
}
// The sky: its base maps as they are, unlit (the sky is its own light).
@fragment fn fs_sky(v: V) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, v.uv).rgb, 1.0);
}
@fragment fn fs_sky_blend(v: V) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, v.uv);
}
"#;

/// How much brighter than its stored value a lightmap texel lights. The
/// game modulates the base map by the lightmap doubled, in gamma space
/// (D3D8, no sRGB); in linear light that is a factor of 2^2.2.
pub const LIGHTMAP_SCALE: f32 = 4.595;
/// Light added to every lightmapped surface, so none is pure black in the
/// editor. Small: lightmaps are not dark once the viewport reaches the
/// window intact ([`Viewport::ui_view`]); 0.22 had been compensating for
/// a window that showed linear values as encoded ones.
pub const LIGHTMAP_FILL: f32 = 0.05;

/// BSP triangles as vertices, shaded by slope: floors light, walls darker.
pub fn world_vertices(tris: &[Tri]) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(tris.len() * 3);
    for t in tris {
        let up = t.n[2].abs();
        let color = [0.35 + 0.25 * up, 0.37 + 0.25 * up, 0.4 + 0.2 * up];
        for p in t.p {
            out.push(Vertex {
                position: p,
                normal: t.n,
                color,
                center: p,
            });
        }
    }
    out
}

/// The opacity of [`Viewport::set_overlay`]'s geometry.
pub const OVERLAY_ALPHA: f64 = 0.35;

/// Triangles in one flat colour, lit by slope, for the overlay (a
/// conversion's source drawn over the converted map).
pub fn overlay_vertices(tris: &[Tri], color: [f32; 3]) -> Vec<Vertex> {
    let mut out = world_vertices(tris);
    for v in &mut out {
        let shade = 0.6 + 0.4 * v.normal[2].abs();
        v.color = color.map(|c| c * shade);
    }
    out
}

/// Marker vertices; the selected marker is drawn white and larger.
pub fn marker_vertices(markers: &[Marker], selected: &[(crate::BlockKind, usize)]) -> Vec<Vertex> {
    glyph_vertices(markers, selected, |_| false)
}

/// Glyph vertices for the placements `modelled` says have no model, and
/// for the selection whatever it is (white and larger).
pub fn glyph_vertices(
    markers: &[Marker],
    selected: &[(crate::BlockKind, usize)],
    modelled: impl Fn(&Marker) -> bool,
) -> Vec<Vertex> {
    let mut out = Vec::new();
    for m in markers {
        let is_sel = selected.contains(&(m.block, m.index));
        if !is_sel && modelled(m) {
            continue;
        }
        let mut m = *m;
        if is_sel {
            m.axes = m.axes.map(|a| crate::geometry::scale(a, 1.6));
        }
        let color: Vec3 = if is_sel {
            [1.0; 3]
        } else {
            block_color(m.block)
        };
        for t in marker_triangles(&m) {
            let n = crate::geometry::normalize(crate::geometry::cross(
                crate::geometry::sub(t[1], t[0]),
                crate::geometry::sub(t[2], t[0]),
            ));
            for p in t {
                out.push(Vertex {
                    position: p,
                    normal: n,
                    color,
                    center: m.position,
                });
            }
        }
    }
    out
}

/// Each model's placements as instances, grouped by model: the instances
/// and, per model, its range in them.
pub fn placement_instances(
    scene: &Scene,
    markers: &[Marker],
    selected: &[(crate::BlockKind, usize)],
) -> (Vec<Instance>, Vec<(usize, Range<u32>)>) {
    let mut placed: Vec<(usize, Instance)> = markers
        .iter()
        .filter_map(|m| {
            let model = scene.model_of(m)?;
            Some((
                model,
                Instance::of(m, selected.contains(&(m.block, m.index))),
            ))
        })
        .collect();
    placed.sort_by_key(|(model, _)| *model);
    let mut ranges: Vec<(usize, Range<u32>)> = Vec::new();
    for (i, (model, _)) in placed.iter().enumerate() {
        let i = i as u32;
        match ranges.last_mut() {
            Some((m, r)) if m == model => r.end = i + 1,
            _ => ranges.push((*model, i..i + 1)),
        }
    }
    (placed.into_iter().map(|(_, i)| i).collect(), ranges)
}

struct Target {
    size: [u32; 2],
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    /// The same pixels viewed as plain `Rgba8Unorm`: [`Viewport::ui_view`].
    ui_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

/// A mesh on the GPU: one vertex and index buffer, drawn batch by batch.
struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Index range, its textures and parameters, blend.
    batches: Vec<(Range<u32>, wgpu::BindGroup, Blend)>,
}

/// Per-batch shading parameters (`P` in the scene shader).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BatchParams {
    lift: f32,
    lit: f32,
    pad: [f32; 2],
}

/// A [`Scene`] uploaded: the world and the models, each batch bound to its
/// textures.
struct GpuScene {
    world: Option<GpuMesh>,
    sky: Option<GpuMesh>,
    models: Vec<Option<GpuMesh>>,
    instances: Option<wgpu::Buffer>,
    ranges: Vec<(usize, Range<u32>)>,
}

/// What uploading a mesh needs: the scene's texture views and lifts.
struct MeshContext<'a> {
    device: &'a wgpu::Device,
    layout: &'a wgpu::BindGroupLayout,
    sampler: &'a wgpu::Sampler,
    /// For textures whose shader clamps them ([`crate::scene::Scene::clamped`]).
    clamp_sampler: &'a wgpu::Sampler,
    clamped: &'a std::collections::HashSet<usize>,
    views: &'a [wgpu::TextureView],
    lifts: &'a [f32],
    grey: &'a wgpu::TextureView,
    white: &'a wgpu::TextureView,
}

impl MeshContext<'_> {
    fn bind(&self, b: &crate::scene::Batch) -> wgpu::BindGroup {
        let base = b.texture.and_then(|t| self.views.get(t));
        let lightmap = b.lightmap.and_then(|t| self.views.get(t));
        let params = BatchParams {
            lift: b
                .texture
                .and_then(|t| self.lifts.get(t).copied())
                .unwrap_or(1.0),
            lit: if lightmap.is_some() { 1.0 } else { 0.0 },
            pad: [0.0; 2],
        };
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scenario viewport batch"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(base.unwrap_or(self.grey)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(
                        if b.texture.is_some_and(|t| self.clamped.contains(&t)) {
                            self.clamp_sampler
                        } else {
                            self.sampler
                        },
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lightmap.unwrap_or(self.white)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn mesh(&self, mesh: &crate::scene::Mesh) -> Option<GpuMesh> {
        let mut vertices: Vec<SceneVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut batches = Vec::new();
        // Solid first, then cut-outs, then see-through.
        let mut order: Vec<&crate::scene::Batch> = mesh.batches.iter().collect();
        order.sort_by_key(|b| b.blend);
        for b in order {
            let base = vertices.len() as u32;
            let start = indices.len() as u32;
            vertices.extend_from_slice(&b.vertices);
            indices.extend(b.indices.iter().map(|i| i + base));
            batches.push((start..indices.len() as u32, self.bind(b), b.blend));
        }
        if indices.is_empty() {
            return None;
        }
        let buffer = |contents: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scenario viewport mesh"),
                    contents,
                    usage,
                })
        };
        Some(GpuMesh {
            vertices: buffer(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
            indices: buffer(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX),
            batches,
        })
    }
}

/// Offscreen renderer for the viewport.
pub struct Viewport {
    pipeline: wgpu::RenderPipeline,
    /// [`Self::pipeline`] blended at [`OVERLAY_ALPHA`] over everything.
    ghost: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    /// The sky model: solid, then see-through batches.
    sky_opaque: wgpu::RenderPipeline,
    sky_blend: wgpu::RenderPipeline,
    /// The sky's camera ([`Camera::sky_view_proj`]) and its bind group.
    sky_uniforms: wgpu::Buffer,
    sky_bind: wgpu::BindGroup,
    opaque: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    /// The texture of untextured batches and the lightmap of unlit ones
    /// (made with the first scene).
    grey: Option<(wgpu::TextureView, wgpu::TextureView)>,
    identity: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    world: Option<(wgpu::Buffer, u32)>,
    markers: Option<(wgpu::Buffer, u32)>,
    overlay: Option<(wgpu::Buffer, u32)>,
    scene: Option<GpuScene>,
    target: Option<Target>,
}

impl Viewport {
    pub fn new(device: &wgpu::Device) -> Self {
        let glyph = format!("{:.1}", crate::geometry::GLYPH_NATIVE_DISTANCE);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scenario viewport"),
            source: wgpu::ShaderSource::Wgsl(SHADER.replace("GLYPH_DISTANCE", &glyph).into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let depth = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scenario viewport"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth(true),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let ghost = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scenario viewport overlay"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // Seen through everything: it is there to compare against.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Constant,
                            dst_factor: wgpu::BlendFactor::OneMinusConstant,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Textured meshes.
        let scene_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scenario viewport scene"),
            source: wgpu::ShaderSource::Wgsl(
                SCENE_SHADER
                    .replace("LIGHTMAP_SCALE", &format!("{LIGHTMAP_SCALE:.3}"))
                    .replace("LIGHTMAP_FILL", &format!("{LIGHTMAP_FILL:.3}"))
                    .into(),
            ),
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scenario viewport texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl), Some(&texture_layout)],
            immediate_size: 0,
        });
        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 8 => Float32x2];
        let instance_attrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        // Solid surfaces are one-sided as in the game (front faces wind
        // counter-clockwise: their normals agree with the vertex normals),
        // so the camera sees into a level through the backs of the walls
        // and cliffs around it. Cut-outs and see-through surfaces are
        // drawn from both sides.
        let scene_pipeline = |entry: &str, blend: Option<wgpu::BlendState>, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scenario viewport scene"),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &scene_module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<SceneVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &vertex_attrs,
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Instance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &instance_attrs,
                        }),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: (entry == "fs_opaque").then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: depth(write),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &scene_module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sky_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scenario viewport sky"),
            source: wgpu::ShaderSource::Wgsl(SKY_SHADER.into()),
        });
        let sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scenario viewport sky"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &sky_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &sky_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sky_opaque = scene_pipeline("fs_sky", None, true);
        let sky_blend = scene_pipeline(
            "fs_sky_blend",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        let opaque = scene_pipeline("fs_opaque", None, true);
        let mask = scene_pipeline("fs_mask", None, true);
        let translucent = scene_pipeline(
            "fs_translucent",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scenario viewport"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        // The sky's horizon gradients are drawn clamped (their template
        // says so): repeated, their solid edge covers the whole dome.
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scenario viewport clamped"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let sky_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scenario viewport sky camera"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scenario viewport sky camera"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sky_uniforms.as_entire_binding(),
            }],
        });
        let identity = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scenario viewport identity"),
            contents: bytemuck::bytes_of(&Instance::IDENTITY),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            pipeline,
            ghost,
            sky,
            sky_opaque,
            sky_blend,
            sky_uniforms,
            sky_bind,
            opaque,
            mask,
            translucent,
            texture_layout,
            sampler,
            clamp_sampler,
            grey: None,
            identity,
            uniforms,
            bind,
            world: None,
            markers: None,
            overlay: None,
            scene: None,
            target: None,
        }
    }

    /// A texture with its mip chain.
    fn texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: crate::scene::Image,
    ) -> wgpu::TextureView {
        let chain = image.mip_chain();
        let size = wgpu::Extent3d {
            width: chain[0].width,
            height: chain[0].height,
            depth_or_array_layers: 1,
        };
        let desc = wgpu::TextureDescriptor {
            label: Some("scenario viewport texture"),
            size,
            mip_level_count: chain.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let texture = device.create_texture(&desc);
        for (level, img) in chain.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(img.width * 4),
                    rows_per_image: Some(img.height),
                },
                wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        texture.create_view(&Default::default())
    }

    fn upload(device: &wgpu::Device, v: &[Vertex]) -> Option<(wgpu::Buffer, u32)> {
        (!v.is_empty()).then(|| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            });
            (buffer, v.len() as u32)
        })
    }
    /// Draw the world as grey slope-shaded triangles (no scene).
    pub fn set_world(&mut self, device: &wgpu::Device, tris: &[Tri]) {
        self.world = Self::upload(device, &world_vertices(tris));
        self.scene = None;
    }
    pub fn set_markers(&mut self, device: &wgpu::Device, v: &[Vertex]) {
        self.markers = Self::upload(device, v);
    }
    /// Geometry drawn see-through over everything (empty: none).
    pub fn set_overlay(&mut self, device: &wgpu::Device, v: &[Vertex]) {
        self.overlay = Self::upload(device, v);
    }

    /// Upload a map's textured world and models (replacing the grey world
    /// and any earlier scene). Placements are set with
    /// [`Self::set_placements`].
    pub fn set_scene(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene) {
        let views: Vec<wgpu::TextureView> = scene
            .textures
            .iter()
            .map(|img| Self::texture(device, queue, img.clone()))
            .collect();
        let pixel = |rgba: [u8; 4]| crate::scene::Image {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
        };
        let (grey, white) = self
            .grey
            .get_or_insert_with(|| {
                (
                    Self::texture(device, queue, pixel([150, 152, 158, 255])),
                    Self::texture(device, queue, pixel([255; 4])),
                )
            })
            .clone();
        let ctx = MeshContext {
            device,
            layout: &self.texture_layout,
            sampler: &self.sampler,
            clamp_sampler: &self.clamp_sampler,
            clamped: &scene.clamped,
            views: &views,
            lifts: &scene.lifts,
            grey: &grey,
            white: &white,
        };
        self.world = None;
        self.scene = Some(GpuScene {
            world: ctx.mesh(&scene.world),
            sky: scene.sky.as_ref().and_then(|s| ctx.mesh(&s.mesh)),
            models: scene.models.iter().map(|m| ctx.mesh(m)).collect(),
            instances: None,
            ranges: Vec::new(),
        });
    }

    /// The placed models and the glyphs of placements without one.
    pub fn set_placements(
        &mut self,
        device: &wgpu::Device,
        scene: &Scene,
        markers: &[Marker],
        selected: &[(crate::BlockKind, usize)],
    ) {
        self.markers = Self::upload(
            device,
            &glyph_vertices(markers, selected, |m| scene.model_of(m).is_some()),
        );
        if let Some(gpu) = &mut self.scene {
            let (instances, ranges) = placement_instances(scene, markers, selected);
            gpu.instances = (!instances.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scenario viewport placements"),
                    contents: bytemuck::cast_slice(&instances),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            gpu.ranges = ranges;
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make = |format, usage, view_formats: &[wgpu::TextureFormat]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scenario viewport"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let color = make(
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            &[UI_FORMAT],
        );
        let depth = make(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]);
        self.target = Some(Target {
            size,
            color_view: color.create_view(&Default::default()),
            ui_view: color.create_view(&wgpu::TextureViewDescriptor {
                format: Some(UI_FORMAT),
                ..Default::default()
            }),
            depth_view: depth.create_view(&Default::default()),
            color,
        });
        true
    }

    /// Draw into the offscreen target (resized to `size`). Returns whether
    /// the target was recreated (its UI texture registration must be
    /// refreshed with the new [`Self::color_view`]).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        size: [u32; 2],
    ) -> bool {
        let resized = self.ensure_target(device, size);
        let target = self.target.as_ref().expect("just ensured");
        let aspect = target.size[0] as f32 / target.size[1] as f32;
        let f = camera.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let w = |v: Vec3| [v[0], v[1], v[2], 0.0];
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: camera.view_proj(aspect),
                eye: {
                    let e = camera.eye();
                    [e[0], e[1], e[2], 1.0]
                },
                right: w(r),
                up: w(cross(r, f)),
                forward: w(f),
                lens: [(camera.fov_y * 0.5).tan(), aspect, 0.0, 0.0],
            }),
        );
        queue.write_buffer(
            &self.sky_uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: camera.sky_view_proj(aspect),
                eye: [0.0, 0.0, 0.0, 1.0],
                right: w(r),
                up: w(cross(r, f)),
                forward: w(f),
                lens: [(camera.fov_y * 0.5).tan(), aspect, 0.0, 0.0],
            }),
        );
        let background = wgpu::LoadOp::Clear(wgpu::Color {
            r: CLEAR[0],
            g: CLEAR[1],
            b: CLEAR[2],
            a: 1.0,
        });
        let color_attachment = |load| {
            [Some(wgpu::RenderPassColorAttachment {
                view: &target.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })]
        };
        let depth_attachment = || {
            Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            })
        };
        let mut enc = device.create_command_encoder(&Default::default());
        // The sky first, in a pass of its own: its depth is its own (the
        // model at infinity) and is cleared again for the level.
        let sky = self.scene.as_ref().and_then(|s| s.sky.as_ref());
        if let Some(sky) = sky {
            let color = color_attachment(background);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scenario viewport sky"),
                color_attachments: &color,
                depth_stencil_attachment: depth_attachment(),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.sky);
            pass.draw(0..3, 0..1);
            pass.set_bind_group(0, &self.sky_bind, &[]);
            pass.set_vertex_buffer(0, sky.vertices.slice(..));
            pass.set_vertex_buffer(1, self.identity.slice(..));
            pass.set_index_buffer(sky.indices.slice(..), wgpu::IndexFormat::Uint32);
            // Batches come solid first, see-through last.
            for (indices, bind, blend) in &sky.batches {
                pass.set_pipeline(match blend {
                    Blend::Translucent => &self.sky_blend,
                    Blend::Opaque | Blend::Mask => &self.sky_opaque,
                });
                pass.set_bind_group(1, bind, &[]);
                pass.draw_indexed(indices.clone(), 0, 0..1);
            }
        }
        {
            let color = color_attachment(if sky.is_some() {
                wgpu::LoadOp::Load
            } else {
                background
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scenario viewport"),
                color_attachments: &color,
                depth_stencil_attachment: depth_attachment(),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            if self.scene.is_some() && sky.is_none() {
                pass.set_pipeline(&self.sky);
                pass.draw(0..3, 0..1);
            }
            // Solid and cut-out scene batches, then the glyphs, then the
            // see-through batches over them.
            self.draw_scene(&mut pass, false);
            pass.set_pipeline(&self.pipeline);
            for (buffer, count) in [&self.world, &self.markers].into_iter().flatten() {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            self.draw_scene(&mut pass, true);
            if let Some((buffer, count)) = &self.overlay {
                pass.set_pipeline(&self.ghost);
                let a = OVERLAY_ALPHA;
                pass.set_blend_constant(wgpu::Color {
                    r: a,
                    g: a,
                    b: a,
                    a,
                });
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        queue.submit([enc.finish()]);
        resized
    }

    /// The scene's batches: solid and cut-out ones, or the see-through ones.
    fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>, translucent: bool) {
        let Some(scene) = &self.scene else {
            return;
        };
        let mut draw = |mesh: &GpuMesh, instances: &wgpu::Buffer, range: Range<u32>| {
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, instances.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (indices, bind, blend) in &mesh.batches {
                if (*blend == Blend::Translucent) != translucent {
                    continue;
                }
                pass.set_pipeline(match blend {
                    Blend::Opaque => &self.opaque,
                    Blend::Mask => &self.mask,
                    Blend::Translucent => &self.translucent,
                });
                pass.set_bind_group(1, bind, &[]);
                pass.draw_indexed(indices.clone(), 0, range.clone());
            }
        };
        if let Some(world) = &scene.world {
            draw(world, &self.identity, 0..1);
        }
        if let Some(instances) = &scene.instances {
            for (model, range) in &scene.ranges {
                if let Some(Some(mesh)) = scene.models.get(*model) {
                    draw(mesh, instances, range.clone());
                }
            }
        }
    }

    /// The colour texture (for readback in tests).
    pub fn color_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.color)
    }

    /// The colour target's view (sRGB: sampling it decodes to linear).
    pub fn color_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.color_view)
    }

    /// The colour target viewed as [`UI_FORMAT`], to register with a UI
    /// renderer that treats sampled texels as sRGB-encoded (dear imgui: its
    /// shader raises them to the power 2.2 before an sRGB framebuffer
    /// encodes them again). Through [`Self::color_view`] it would get the
    /// linear values instead, and draw every surface as if its linear value
    /// were its encoded one: a linear 0.05 (a dim wall, sRGB 63) shows as 13.
    pub fn ui_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.ui_view)
    }

    /// Whether the scene uploaded has a sky model to draw.
    pub fn has_sky(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| s.sky.is_some())
    }

    /// The current target size in pixels.
    pub fn size(&self) -> Option<[u32; 2]> {
        self.target.as_ref().map(|t| t.size)
    }
}
