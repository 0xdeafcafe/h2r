//! A small wgpu pipeline for flat/normal-shaded model display, drawing into
//! any `gfx::RenderTarget` (window or offscreen).
use crate::mesh::{GpuVertex, Mesh};
use gfx::{GfxContext, RenderTarget};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct U { mvp: mat4x4<f32>, eye: vec4<f32>, mode: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct V { @builtin(position) pos: vec4<f32>, @location(0) n: vec3<f32>, @location(1) c: vec3<f32>, @location(2) w: vec3<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec3<f32>) -> V {
    var o: V;
    o.pos = u.mvp * vec4<f32>(p, 1.0);
    o.n = n; o.c = c; o.w = p;
    return o;
}
@fragment fn fs(i: V) -> @location(0) vec4<f32> {
    // Face normal from screen-space derivatives: independent of the decoded
    // vertex normals, so it also checks winding/shape on its own.
    let face = normalize(cross(dpdx(i.w), dpdy(i.w)));
    let view = normalize(u.eye.xyz - i.w);
    if (u.mode.x > 1.5) {
        return vec4<f32>(normalize(i.n) * 0.5 + 0.5, 1.0);
    }
    var n = face;
    if (u.mode.x > 0.5) { n = normalize(i.n); }
    let d = abs(dot(n, view));
    let key = max(dot(n, normalize(vec3<f32>(0.4, 0.3, 0.85))), 0.0);
    let l = 0.25 + 0.55 * d + 0.25 * key;
    return vec4<f32>(i.c * l, 1.0);
}
"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shade {
    /// Flat (face normal) lighting tinted per material.
    Flat,
    /// Lighting from the decoded vertex normals.
    Smooth,
    /// Decoded vertex normal as colour.
    Normals,
}

pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}
impl Orbit {
    pub fn frame(mesh: &Mesh) -> Self {
        let c = [0, 1, 2].map(|k| 0.5 * (mesh.min[k] + mesh.max[k]));
        let r = (0..3)
            .map(|k| (mesh.max[k] - mesh.min[k]).powi(2))
            .sum::<f32>()
            .sqrt()
            * 0.5;
        Self {
            yaw: 35f32.to_radians(),
            pitch: 20f32.to_radians(),
            distance: (r.max(1e-3)) * 2.6,
            target: c,
        }
    }
    pub fn eye(&self) -> [f32; 3] {
        // Halo is Z-up; yaw rotates about Z.
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        [
            self.target[0] + self.distance * cp * cy,
            self.target[1] + self.distance * cp * sy,
            self.target[2] + self.distance * sp,
        ]
    }
    pub fn matrix(&self, aspect: f32) -> [[f32; 4]; 4] {
        let view = look_at(self.eye(), self.target, [0.0, 0.0, 1.0]);
        let near = self.distance * 0.01;
        let proj = perspective(45f32.to_radians(), aspect, near, self.distance * 10.0);
        mul(proj, view)
    }
}

type M = [[f32; 4]; 4]; // column-major: m[col][row]
fn mul(a: M, b: M) -> M {
    let mut o = [[0.0; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    o
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn look_at(eye: [f32; 3], at: [f32; 3], up: [f32; 3]) -> M {
    let f = norm(sub(at, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}
/// Right-handed perspective with wgpu's 0..1 depth.
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    eye: [f32; 4],
    mode: [f32; 4],
}

pub struct ModelRenderer {
    pipeline: wgpu::RenderPipeline,
    wire_pipeline: Option<wgpu::RenderPipeline>,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
}

impl ModelRenderer {
    pub fn new(ctx: &GfxContext, format: wgpu::TextureFormat, mesh: &Mesh) -> Self {
        let d = &ctx.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewer shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
        let make = |mode: wgpu::PolygonMode| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("viewer pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GpuVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    polygon_mode: mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: gfx::presenter::DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make(wgpu::PolygonMode::Fill);
        let wire_pipeline = d
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE)
            .then(|| make(wgpu::PolygonMode::Line));
        let uniforms = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        // Empty meshes still need non-empty buffers.
        let dummy = [GpuVertex {
            position: [0.0; 3],
            normal: [0.0; 3],
            color: [0.0; 3],
        }];
        let verts: &[GpuVertex] = if mesh.vertices.is_empty() {
            &dummy
        } else {
            &mesh.vertices
        };
        let idx: &[u32] = if mesh.indices.is_empty() {
            &[0, 0, 0]
        } else {
            &mesh.indices
        };
        let vb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model vertices"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model indices"),
            contents: bytemuck::cast_slice(idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipeline,
            wire_pipeline,
            uniforms,
            bind,
            vb,
            ib,
            count: mesh.indices.len() as u32,
        }
    }

    pub fn draw(
        &self,
        ctx: &GfxContext,
        target: &RenderTarget<'_>,
        orbit: &Orbit,
        shade: Shade,
        wireframe: bool,
    ) {
        let aspect = target.size.0 as f32 / target.size.1.max(1) as f32;
        let e = orbit.eye();
        let u = Uniforms {
            mvp: orbit.matrix(aspect),
            eye: [e[0], e[1], e[2], 1.0],
            mode: [
                match shade {
                    Shade::Flat => 0.0,
                    Shade::Smooth => 1.0,
                    Shade::Normals => 2.0,
                },
                0.0,
                0.0,
                0.0,
            ],
        };
        ctx.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.09,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: target.depth.map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let pipeline = match (&self.wire_pipeline, wireframe) {
                (Some(w), true) => w,
                _ => &self.pipeline,
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.vb.slice(..));
            pass.set_index_buffer(self.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.count, 0, 0..1);
        }
        ctx.queue.submit([enc.finish()]);
    }
}
