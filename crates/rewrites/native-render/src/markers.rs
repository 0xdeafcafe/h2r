//! The marker skin (docs/game/gametype-markers.md, "The game"): the
//! translucent faces of the game type markers and their X-ray lines, drawn
//! over the world's colour with its depth.
//!
//! The faces are alpha blended and depth tested, never depth written, so a
//! marker behind a nearer wall shows no skin. A line in front of the depth
//! draws at its alpha; a line behind it draws at [`XRAY_ALPHA`] of that, so
//! an author finds a spawn through a wall. With the native renderer off
//! ([`plan`]) this pass does not run and the window's world-line pass draws
//! the lines alone.
//!
//! The pass takes plain vertices and a view-projection matrix; it knows
//! nothing of `game-markers`.

/// What a line behind the world's depth keeps of its alpha.
pub const XRAY_ALPHA: f32 = 1.0 / 3.0;

/// One vertex of a face or a line: a world point and a straight RGBA colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub colour: [f32; 4],
}

/// The faces and lines of a view's markers, as the pass uploads them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    /// Three vertices a face.
    pub tris: Vec<Vertex>,
    /// Two vertices a line.
    pub lines: Vec<Vertex>,
}

impl Batch {
    pub fn push_tri(&mut self, points: [[f32; 3]; 3], colour: [f32; 4]) {
        self.tris.extend(points.map(|position| Vertex { position, colour }));
    }

    pub fn push_line(&mut self, a: [f32; 3], b: [f32; 3], colour: [f32; 4]) {
        self.lines.extend([a, b].map(|position| Vertex { position, colour }));
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty() && self.lines.is_empty()
    }
}

/// What the marker pass draws for a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The translucent faces.
    pub skin: bool,
    /// The lines, with their X-ray look behind the world.
    pub lines: bool,
}

/// `impl.render.native` on: the pass draws the skin and the lines. Off: it
/// draws nothing and the world-line pass draws the lines alone, at full
/// alpha with no depth.
pub fn plan(native: bool, batch: &Batch) -> Plan {
    Plan {
        skin: native && !batch.tris.is_empty(),
        lines: native && !batch.lines.is_empty(),
    }
}

#[cfg(feature = "gpu")]
pub use gpu::{Pass, Targets};

#[cfg(feature = "gpu")]
mod gpu {
    use super::{Batch, Vertex, XRAY_ALPHA};

    const SHADER: &str = "
struct U { m: mat4x4<f32>, scale: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VO { @builtin(position) p: vec4<f32>, @location(0) c: vec4<f32> };
@vertex fn vs_main(@location(0) pos: vec3<f32>, @location(1) col: vec4<f32>) -> VO {
    var o: VO;
    o.p = u.m * vec4<f32>(pos, 1.0);
    o.c = vec4<f32>(col.rgb, col.a * u.scale.x);
    return o;
}
@fragment fn fs_main(i: VO) -> @location(0) vec4<f32> { return i.c; }
";

    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Uniform {
        matrix: [f32; 16],
        scale: [f32; 4],
    }

    /// The attachments the pass draws over. It loads both and writes only
    /// colour.
    pub struct Targets<'a> {
        pub color: &'a wgpu::TextureView,
        pub depth: &'a wgpu::TextureView,
    }

    /// The marker pass: the skin, the lines in front of the depth and the
    /// lines behind it.
    pub struct Pass {
        layout: wgpu::BindGroupLayout,
        skin: wgpu::RenderPipeline,
        front: wgpu::RenderPipeline,
        behind: wgpu::RenderPipeline,
    }

    impl Pass {
        /// `reversed_z` is a depth buffer where nearer is greater.
        pub fn new(
            device: &wgpu::Device,
            color_format: wgpu::TextureFormat,
            depth_format: wgpu::TextureFormat,
            reversed_z: bool,
        ) -> Self {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("markers"),
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
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("markers"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("markers"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let (near, far) = if reversed_z {
                (wgpu::CompareFunction::GreaterEqual, wgpu::CompareFunction::Less)
            } else {
                (wgpu::CompareFunction::LessEqual, wgpu::CompareFunction::Greater)
            };
            let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
            let make = |label, topology, compare| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology,
                        // A face is seen from both sides.
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: depth_format,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(compare),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: color_format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
            Self {
                skin: make("marker skin", wgpu::PrimitiveTopology::TriangleList, near),
                front: make("marker lines", wgpu::PrimitiveTopology::LineList, near),
                behind: make("marker x-ray", wgpu::PrimitiveTopology::LineList, far),
                layout,
            }
        }

        /// Records the pass: the skin, the lines in front, then the lines
        /// behind at [`XRAY_ALPHA`]. `matrix` is column-major, world to clip.
        pub fn encode(
            &self,
            device: &wgpu::Device,
            encoder: &mut wgpu::CommandEncoder,
            targets: &Targets,
            matrix: [[f32; 4]; 4],
            batch: &Batch,
        ) {
            if batch.is_empty() {
                return;
            }
            let buffer = |label: &str, usage, bytes: &[u8]| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: bytes.len() as u64,
                    usage,
                    mapped_at_creation: true,
                });
                buffer.slice(..).get_mapped_range_mut().copy_from_slice(bytes);
                buffer.unmap();
                buffer
            };
            let group = |scale: f32| {
                let uniform = Uniform {
                    matrix: std::array::from_fn(|i| matrix[i / 4][i % 4]),
                    scale: [scale, 0., 0., 0.],
                };
                let buffer = buffer("marker uniform", wgpu::BufferUsages::UNIFORM, bytemuck::bytes_of(&uniform));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("markers"),
                    layout: &self.layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                })
            };
            let (full, xray) = (group(1.0), group(XRAY_ALPHA));
            let tris = (!batch.tris.is_empty())
                .then(|| buffer("marker skin", wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&batch.tris)));
            let lines = (!batch.lines.is_empty())
                .then(|| buffer("marker lines", wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&batch.lines)));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("markers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: targets.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(tris) = &tris {
                pass.set_pipeline(&self.skin);
                pass.set_bind_group(0, &full, &[]);
                pass.set_vertex_buffer(0, tris.slice(..));
                pass.draw(0..batch.tris.len() as u32, 0..1);
            }
            if let Some(lines) = &lines {
                pass.set_vertex_buffer(0, lines.slice(..));
                pass.set_pipeline(&self.front);
                pass.set_bind_group(0, &full, &[]);
                pass.draw(0..batch.lines.len() as u32, 0..1);
                pass.set_pipeline(&self.behind);
                pass.set_bind_group(0, &xray, &[]);
                pass.draw(0..batch.lines.len() as u32, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_counts_three_vertices_a_face_and_two_a_line() {
        let mut batch = Batch::default();
        assert!(batch.is_empty());
        batch.push_tri([[0.; 3], [1., 0., 0.], [0., 1., 0.]], [1.; 4]);
        batch.push_line([0.; 3], [1.; 3], [1.; 4]);
        assert_eq!((batch.tris.len(), batch.lines.len()), (3, 2));
    }

    #[test]
    fn with_the_native_renderer_off_nothing_draws_here() {
        let mut batch = Batch::default();
        batch.push_tri([[0.; 3]; 3], [1.; 4]);
        batch.push_line([0.; 3], [1.; 3], [1.; 4]);
        assert_eq!(plan(false, &batch), Plan { skin: false, lines: false });
        assert_eq!(plan(true, &batch), Plan { skin: true, lines: true });
        assert_eq!(plan(true, &Batch::default()), Plan { skin: false, lines: false });
    }
}
