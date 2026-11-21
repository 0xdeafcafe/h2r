//! World-space lines and labels over the finished frame.
use std::sync::Arc;
use std::time::Instant;

pub use gfx::world_lines::ImageSamples;
use gfx::world_lines::{ShownMarks, WorldLine};

use crate::scale::Viewport;

#[derive(Clone, Copy, Debug)]
pub struct MarkCamera {
    /// World-to-clip matrix, column-major, with WGPU depth in 0..w.
    pub view_projection: [[f32; 4]; 4],
    /// Top, left, bottom, right as fractions of the whole guest image.
    pub viewport: [f32; 4],
}

pub type MarkCameras = [Option<MarkCamera>; 4];

/// All sources on an image use its per-view cameras.
#[derive(Debug)]
pub struct ProjectedMarks {
    pub cameras: MarkCameras,
    pub samples: Arc<ImageSamples>,
}

impl ProjectedMarks {
    pub(crate) fn visit(
        &self,
        now: Instant,
        visitor: &mut dyn FnMut(&MarkCameras, WorldLine<'_>),
    ) -> ShownMarks {
        self.samples.visit(&|| now, &mut |line| visitor(&self.cameras, line))
    }
}

/// Presentation metadata, independent of the link plate.
#[derive(Clone, Debug, Default)]
pub enum FrameMarks {
    #[default]
    Raw,
    Guest(Option<Arc<ProjectedMarks>>),
    GuestWithoutWorldView,
    Synthetic(Option<Arc<ProjectedMarks>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkFrameStatus {
    Waiting,
    Ready(u64),
    NoWorldView,
    Raw,
    Loading,
    Synthetic,
}

impl FrameMarks {
    pub(crate) fn marks(&self) -> Option<&ProjectedMarks> {
        match self {
            Self::Guest(sample) | Self::Synthetic(sample) => sample.as_deref(),
            Self::Raw | Self::GuestWithoutWorldView => None,
        }
    }

    pub(crate) fn status(&self, drawn_generation: Option<u64>) -> MarkFrameStatus {
        match self {
            Self::Guest(_) => {
                drawn_generation.map_or(MarkFrameStatus::Waiting, MarkFrameStatus::Ready)
            }
            Self::GuestWithoutWorldView => MarkFrameStatus::NoWorldView,
            Self::Synthetic(_) => MarkFrameStatus::Synthetic,
            Self::Raw => MarkFrameStatus::Raw,
        }
    }
}

fn clip(camera: &MarkCamera, [x, y, z]: [f32; 3]) -> [f32; 4] {
    let m = &camera.view_projection;
    std::array::from_fn(|i| m[0][i] * x + m[1][i] * y + m[2][i] * z + m[3][i])
}

fn planes([x, y, z, w]: [f32; 4]) -> [f32; 6] {
    [w + x, w - x, w + y, w - y, z, w - z]
}

fn pixels(camera: &MarkCamera, area: Viewport, [x, y, _, w]: [f32; 4]) -> Option<[f32; 2]> {
    let [top, left, bottom, right] = camera.viewport;
    if w <= 0.0 || !w.is_finite() || area.is_empty() {
        return None;
    }
    let x = left + (right - left) * (0.5 + 0.5 * (x / w));
    let y = top + (bottom - top) * (0.5 - 0.5 * (y / w));
    Some([area.x as f32 + x * area.width as f32, area.y as f32 + y * area.height as f32])
}

fn project_point(camera: &MarkCamera, area: Viewport, point: [f32; 3]) -> Option<[f32; 2]> {
    let point = clip(camera, point);
    if !planes(point).into_iter().all(|value| value.is_finite() && value >= 0.0) {
        return None;
    }
    pixels(camera, area, point)
}

/// Clip a segment in homogeneous coordinates before dividing by w.
fn project_segment(
    camera: &MarkCamera,
    area: Viewport,
    [a, b]: [[f32; 3]; 2],
) -> Option<[[f32; 2]; 2]> {
    let a = clip(camera, a);
    let b = clip(camera, b);
    let (mut start, mut end) = (0.0_f32, 1.0_f32);
    for (a, b) in planes(a).into_iter().zip(planes(b)) {
        if !a.is_finite() || !b.is_finite() || (a < 0.0 && b < 0.0) {
            return None;
        }
        if a < 0.0 {
            start = start.max(a / (a - b));
        } else if b < 0.0 {
            end = end.min(a / (a - b));
        }
    }
    if start > end {
        return None;
    }
    let at = |t| std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t);
    Some([pixels(camera, area, at(start))?, pixels(camera, area, at(end))?])
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
mod renderer {
    use font8x8::UnicodeFonts;

    use super::*;

    const SHADER: &str = r#"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) stroke: vec2<f32>,
}
@vertex
fn vs(
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) stroke: vec2<f32>,
) -> Out {
    return Out(vec4<f32>(pos, 0.0, 1.0), color, stroke);
}
@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    if in.stroke.y > 0.5 && fract(in.stroke.x / 9.0) > 5.0 / 9.0 {
        discard;
    }
    return in.color;
}
"#;

    #[derive(Default)]
    pub(crate) struct MarkRenderer {
        pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
        vertices: Vec<u8>,
        buffer: Option<wgpu::Buffer>,
    }

    impl MarkRenderer {
        pub(crate) fn begin(&mut self) {
            self.vertices.clear();
        }

        pub(crate) fn push(
            &mut self,
            mark: &WorldLine<'_>,
            cameras: &MarkCameras,
            area: Viewport,
            size: (u32, u32),
        ) {
            let color = [mark.color[0], mark.color[1], mark.color[2], 1.0];
            for camera in cameras.iter().flatten() {
                for &edge in mark.edges {
                    if let Some(line) = project_segment(camera, area, edge) {
                        self.segment(line, color, size, mark.dashed);
                    }
                }
                if let Some(label) = mark.label
                    && let Some(point) = project_point(camera, area, mark.anchor)
                {
                    self.label(label, point, color, size, camera, area);
                }
            }
        }

        fn segment(
            &mut self,
            [a, b]: [[f32; 2]; 2],
            color: [f32; 4],
            size: (u32, u32),
            dashed: bool,
        ) {
            let length = if dashed {
                (b[0] - a[0]).hypot(b[1] - a[1])
            } else {
                0.0
            };
            for ([x, y], distance) in [(a, 0.0), (b, length)] {
                let pos = [2.0 * x / size.0 as f32 - 1.0, 1.0 - 2.0 * y / size.1 as f32];
                let stroke = [distance, if dashed { 1.0 } else { 0.0 }];
                for value in pos.into_iter().chain(color).chain(stroke) {
                    self.vertices.extend(value.to_ne_bytes());
                }
            }
        }

        fn label(
            &mut self,
            label: &str,
            [x, y]: [f32; 2],
            color: [f32; 4],
            size: (u32, u32),
            camera: &MarkCamera,
            area: Viewport,
        ) {
            let [_, _, bottom, right] = camera.viewport;
            let right = area.x as f32 + right * area.width as f32;
            let bottom = area.y as f32 + bottom * area.height as f32;
            let x = x.ceil() + 5.0;
            let y = y.ceil() + 5.0;
            if y + 8.0 > bottom {
                return;
            }
            for (i, c) in crate::overlay::overlay_text(label).chars().enumerate() {
                let x = x + i as f32 * 8.0;
                if x + 8.0 > right {
                    break;
                }
                let Some(glyph) = font8x8::BASIC_FONTS.get(c).or_else(|| font8x8::LATIN_FONTS.get(c))
                else {
                    continue;
                };
                for (row, bits) in glyph.into_iter().enumerate() {
                    let mut col = 0;
                    while col < 8 {
                        if bits & (1 << col) == 0 {
                            col += 1;
                            continue;
                        }
                        let first = col;
                        while col < 8 && bits & (1 << col) != 0 {
                            col += 1;
                        }
                        let y = y + row as f32 + 0.5;
                        self.segment(
                            [[x + first as f32, y], [x + col as f32, y]],
                            color,
                            size,
                            false,
                        );
                    }
                }
            }
        }

        fn pipeline(
            &mut self,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
        ) -> wgpu::RenderPipeline {
            if let Some((_, pipeline)) = self.pipelines.iter().find(|(fmt, _)| *fmt == format) {
                return pipeline.clone();
            }
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("editor marks"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("editor marks"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x2, 1 => Float32x4, 2 => Float32x2
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            self.pipelines.push((format, pipeline.clone()));
            pipeline
        }

        pub(crate) fn draw(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            encoder: &mut wgpu::CommandEncoder,
            dst: &wgpu::TextureView,
            format: wgpu::TextureFormat,
        ) {
            if self.vertices.is_empty() {
                return;
            }
            let bytes = self.vertices.len() as u64;
            if self.buffer.as_ref().is_none_or(|buffer| buffer.size() < bytes) {
                self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("editor mark vertices"),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let pipeline = self.pipeline(device, format);
            let buffer = self.buffer.as_ref().expect("editor vertex buffer");
            queue.write_buffer(buffer, 0, &self.vertices);
            let count = (bytes / 32) as u32;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("editor marks"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
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
            pass.set_pipeline(&pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..count, 0..1);
        }
    }
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) use renderer::MarkRenderer;

#[cfg(test)]
mod tests {
    use super::*;

    // Camera at the origin facing -z, near 1, far 10, 90-degree square lens.
    const CAMERA: MarkCamera = MarkCamera {
        view_projection: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -10.0 / 9.0, -1.0],
            [0.0, 0.0, -10.0 / 9.0, 0.0],
        ],
        viewport: [0.0, 0.5, 1.0, 1.0],
    };

    const AREA: Viewport = Viewport { x: 40, y: 10, width: 200, height: 100 };

    #[test]
    fn marks_project_straight_ahead_to_the_viewport_centre() {
        assert_eq!(project_point(&CAMERA, AREA, [0.0, 0.0, -3.0]), Some([190.0, 60.0]));
    }

    #[test]
    fn marks_clip_segments_at_the_near_plane() {
        let points = project_segment(&CAMERA, AREA, [[0.5, 0.0, 0.0], [0.5, 0.0, -2.0]]).unwrap();
        assert_eq!(points, [[215.0, 60.0], [202.5, 60.0]]);
    }

    #[test]
    fn marks_drop_segments_behind_the_camera() {
        assert!(project_segment(&CAMERA, AREA, [[0.0, 0.0, 1.0], [0.0, 1.0, 2.0]]).is_none());
    }
}
