//! The 3D viewport: the structure BSP and the placement markers, drawn with
//! wgpu into the panel's own colour and depth textures, which egui then
//! shows as an image.
//!
//! Rendering offscreen (instead of inside egui's render pass) keeps the
//! viewport independent of the shell's surface: egui's pass may have no
//! depth buffer and any MSAA count, and a pipeline must match both. The
//! texture is registered with `egui_wgpu::Renderer` as a native texture.
use crate::geometry::{Camera, Marker, Tri, Vec3, block_color, marker_triangles};
use egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Background colour (linear).
pub const CLEAR: [f64; 3] = [0.02, 0.025, 0.035];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
}

const SHADER: &str = r#"
struct U { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct V { @builtin(position) clip: vec4<f32>, @location(0) n: vec3<f32>, @location(1) c: vec3<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec3<f32>) -> V {
    var o: V;
    o.clip = u.view_proj * vec4<f32>(p, 1.0);
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
            });
        }
    }
    out
}

/// Marker vertices; the selected marker is drawn white and larger.
pub fn marker_vertices(
    markers: &[Marker],
    selected: Option<(crate::BlockKind, usize)>,
) -> Vec<Vertex> {
    let mut out = Vec::new();
    for m in markers {
        let is_sel = selected == Some((m.block, m.index));
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
                });
            }
        }
    }
    out
}

struct Target {
    size: [u32; 2],
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

/// Offscreen renderer for the viewport.
pub struct Viewport {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    world: Option<(wgpu::Buffer, u32)>,
    markers: Option<(wgpu::Buffer, u32)>,
    target: Option<Target>,
    texture_id: Option<egui::TextureId>,
}

impl Viewport {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scenario viewport"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
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
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
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
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
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
        Self {
            pipeline,
            uniforms,
            bind,
            world: None,
            markers: None,
            target: None,
            texture_id: None,
        }
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
    pub fn set_world(&mut self, device: &wgpu::Device, tris: &[Tri]) {
        self.world = Self::upload(device, &world_vertices(tris));
    }
    pub fn set_markers(&mut self, device: &wgpu::Device, v: &[Vertex]) {
        self.markers = Self::upload(device, v);
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
        let make = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scenario viewport"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = make(
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = make(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        self.target = Some(Target {
            size,
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
        });
        true
    }

    /// Draw into the offscreen target (resized to `size`). Returns whether
    /// the target was recreated (its egui registration must be refreshed).
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
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: camera.view_proj(aspect),
            }),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scenario viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: CLEAR[0],
                            g: CLEAR[1],
                            b: CLEAR[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            for (buffer, count) in [&self.world, &self.markers].into_iter().flatten() {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        queue.submit([enc.finish()]);
        resized
    }

    /// The colour texture (for readback in tests).
    pub fn color_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.color)
    }

    /// Render and hand the result to egui as a texture.
    pub fn egui_texture(
        &mut self,
        render: &egui_wgpu::RenderState,
        camera: &Camera,
        size: [u32; 2],
    ) -> egui::TextureId {
        let resized = self.render(&render.device, &render.queue, camera, size);
        let view = &self.target.as_ref().expect("rendered").color_view;
        let mut r = render.renderer.write();
        match self.texture_id {
            Some(id) if resized => {
                r.update_egui_texture_from_wgpu_texture(
                    &render.device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                );
                id
            }
            Some(id) => id,
            None => {
                let id = r.register_native_texture(&render.device, view, wgpu::FilterMode::Linear);
                self.texture_id = Some(id);
                id
            }
        }
    }
}
