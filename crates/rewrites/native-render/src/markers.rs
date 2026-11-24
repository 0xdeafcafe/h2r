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
//! nothing of `game-markers`. The game publishes the frame's [`Batch`]
//! ([`publish`]); the frame graph (`frame_graph::markers`) draws it in each
//! game view after the transparents and before the HUD, against the
//! guest's W-buffer depth ([`WDepth`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// How the guest's depth buffer stores a view's depth: a W-buffer, `floor(w
/// * scale / far) / scale`, nearer smaller (`frame_scene::w_depth`). The
/// pass writes that value for each fragment, so its test meets the world's
/// own. `bias` is how far towards the eye a fragment moves first, in view
/// units, so a face lying on a surface passes the rounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WDepth {
    pub far: f32,
    pub scale: f32,
    pub bias: f32,
}

static PUBLISHED: Mutex<Option<Arc<Batch>>> = Mutex::new(None);
static HAS_PUBLISHED: AtomicBool = AtomicBool::new(false);
static DREW: Mutex<Option<Instant>> = Mutex::new(None);

/// How long after the pass last recorded the window's line pass leaves the
/// lines to it.
pub const DREW_FOR: Duration = Duration::from_millis(250);

/// The batch the frame graph draws from now on, in place of the last; none
/// (or an empty batch) when there are no markers.
pub fn publish(batch: Option<Batch>) {
    let batch = batch.filter(|b| !b.is_empty()).map(Arc::new);
    if batch.is_none() && !HAS_PUBLISHED.swap(false, Ordering::AcqRel) {
        return;
    }
    HAS_PUBLISHED.store(batch.is_some(), Ordering::Release);
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = batch;
}

/// The batch to draw, if the game published one.
pub fn published() -> Option<Arc<Batch>> {
    if !HAS_PUBLISHED.load(Ordering::Acquire) {
        return None;
    }
    PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The frame graph recorded the pass for a view.
pub fn note_drawn() {
    *DREW.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// Whether the pass recorded within [`DREW_FOR`].
pub fn drawn_recently() -> bool {
    DREW.lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|at| at.elapsed() < DREW_FOR)
}

/// Whether the window's world-line pass leaves the marker lines out: the
/// native renderer is on and this pass drew them a moment ago, so a line is
/// never drawn twice. Anywhere the pass does not run (the switch off, a
/// frame the graph does not own) the window draws the lines alone.
pub fn window_skips_lines(native: bool, drew_recently: bool) -> bool {
    native && drew_recently
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
pub use gpu::{Pass, Prepared, Targets};

#[cfg(feature = "gpu")]
mod gpu {
    use super::{Batch, Vertex, WDepth, XRAY_ALPHA};

    const SHADER: &str = "
struct U { m: mat4x4<f32>, scale: vec4<f32>, depth: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VO { @builtin(position) p: vec4<f32>, @location(0) c: vec4<f32>, @location(1) w: f32 };
struct FO { @location(0) c: vec4<f32>, @builtin(frag_depth) d: f32 };
@vertex fn vs_main(@location(0) pos: vec3<f32>, @location(1) col: vec4<f32>) -> VO {
    var o: VO;
    o.p = u.m * vec4<f32>(pos, 1.0);
    o.c = vec4<f32>(col.rgb, col.a * u.scale.x);
    o.w = o.p.w;
    return o;
}
@fragment fn fs_main(i: VO) -> FO {
    var o: FO;
    o.c = i.c;
    if (u.depth.x > 0.0) {
        o.d = floor(max(i.w - u.depth.z, 0.0) * u.depth.x) / u.depth.y;
    } else {
        o.d = i.p.z;
    }
    return o;
}
";

    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Uniform {
        matrix: [f32; 16],
        scale: [f32; 4],
        depth: [f32; 4],
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
        stencil: bool,
    }

    /// A batch uploaded for one view: everything `record` needs, made at
    /// prepare so recording is only commands (it can run more than once).
    pub struct Prepared {
        skin: wgpu::RenderPipeline,
        front: wgpu::RenderPipeline,
        behind: wgpu::RenderPipeline,
        full: wgpu::BindGroup,
        xray: wgpu::BindGroup,
        tris: Option<(wgpu::Buffer, u32)>,
        lines: Option<(wgpu::Buffer, u32)>,
        stencil: bool,
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                stencil: depth_format.has_stencil_aspect(),
            }
        }

        /// Uploads `batch` for a view whose world to clip is `matrix`
        /// (column-major). `w` is the guest's W-buffer, or none for a depth
        /// buffer holding the clip depth. None for an empty batch.
        pub fn prepare(
            &self,
            device: &wgpu::Device,
            matrix: [[f32; 4]; 4],
            batch: &Batch,
            w: Option<WDepth>,
        ) -> Option<Prepared> {
            if batch.is_empty() {
                return None;
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
            let depth = w.map_or([0.0; 4], |w| [w.scale / w.far, w.scale, w.bias, 0.0]);
            let group = |scale: f32| {
                let uniform = Uniform {
                    matrix: std::array::from_fn(|i| matrix[i / 4][i % 4]),
                    scale: [scale, 0., 0., 0.],
                    depth,
                };
                let buffer = buffer("marker uniform", wgpu::BufferUsages::UNIFORM, bytemuck::bytes_of(&uniform));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("markers"),
                    layout: &self.layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                })
            };
            let vertices = |label: &str, v: &[Vertex]| {
                (!v.is_empty()).then(|| {
                    (buffer(label, wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(v)), v.len() as u32)
                })
            };
            Some(Prepared {
                skin: self.skin.clone(),
                front: self.front.clone(),
                behind: self.behind.clone(),
                full: group(1.0),
                xray: group(XRAY_ALPHA),
                tris: vertices("marker skin", &batch.tris),
                lines: vertices("marker lines", &batch.lines),
                stencil: self.stencil,
            })
        }

        /// Records the pass: the skin, the lines in front, then the lines
        /// behind at [`XRAY_ALPHA`], over a depth buffer holding the clip
        /// depth. `matrix` is column-major, world to clip.
        pub fn encode(
            &self,
            device: &wgpu::Device,
            encoder: &mut wgpu::CommandEncoder,
            targets: &Targets,
            matrix: [[f32; 4]; 4],
            batch: &Batch,
        ) {
            if let Some(prepared) = self.prepare(device, matrix, batch, None) {
                prepared.record(encoder, targets, None);
            }
        }
    }

    impl Prepared {
        /// GPU draws `record` issues.
        pub fn draws(&self) -> usize {
            usize::from(self.tris.is_some()) + 2 * usize::from(self.lines.is_some())
        }

        /// Records the pass on `targets`, inside `viewport` (`x, y, width,
        /// height` in target pixels) when given.
        pub fn record(&self, encoder: &mut wgpu::CommandEncoder, targets: &Targets, viewport: Option<[f32; 4]>) {
            fn kept<V>() -> wgpu::Operations<V> {
                wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("markers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: targets.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: kept(),
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: targets.depth,
                    depth_ops: Some(kept()),
                    stencil_ops: self.stencil.then(kept::<u32>),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some([x, y, w, h]) = viewport {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
            }
            if let Some((tris, count)) = &self.tris {
                pass.set_pipeline(&self.skin);
                pass.set_bind_group(0, &self.full, &[]);
                pass.set_vertex_buffer(0, tris.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some((lines, count)) = &self.lines {
                pass.set_vertex_buffer(0, lines.slice(..));
                pass.set_pipeline(&self.front);
                pass.set_bind_group(0, &self.full, &[]);
                pass.draw(0..*count, 0..1);
                pass.set_pipeline(&self.behind);
                pass.set_bind_group(0, &self.xray, &[]);
                pass.draw(0..*count, 0..1);
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

    #[test]
    fn the_window_leaves_the_lines_only_to_a_native_pass_that_drew() {
        assert!(window_skips_lines(true, true));
        assert!(!window_skips_lines(true, false), "the graph did not draw: the lines stay");
        assert!(!window_skips_lines(false, true), "native off: lines only");
        assert!(!window_skips_lines(false, false));
    }

    #[test]
    fn a_published_batch_is_read_back_and_an_empty_one_clears_it() {
        let mut batch = Batch::default();
        batch.push_line([0.; 3], [1.; 3], [1.; 4]);
        publish(Some(batch.clone()));
        assert_eq!(published().as_deref(), Some(&batch));
        publish(Some(Batch::default()));
        assert!(published().is_none());
        publish(Some(batch));
        publish(None);
        assert!(published().is_none());
    }
}
