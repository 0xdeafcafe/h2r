//! World-space lines and labels over the finished frame.
use std::sync::Arc;
use std::time::Instant;

pub use gfx::world_lines::ImageSamples;
use gfx::world_lines::{Fade, Pulse, ShownMarks, WorldLine};

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
        self.samples
            .visit(&|| now, &mut |line| visitor(&self.cameras, line))
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
    Some([
        area.x as f32 + x * area.width as f32,
        area.y as f32 + y * area.height as f32,
    ])
}

fn project_point(camera: &MarkCamera, area: Viewport, point: [f32; 3]) -> Option<[f32; 2]> {
    let point = clip(camera, point);
    if !planes(point)
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
    {
        return None;
    }
    pixels(camera, area, point)
}

/// Clip a segment in homogeneous coordinates before dividing by w.
fn project_segment(
    camera: &MarkCamera,
    area: Viewport,
    edge: [[f32; 3]; 2],
) -> Option<[[f32; 2]; 2]> {
    project_segment_depth(camera, area, edge).map(|(points, _)| points)
}

/// [`project_segment`] with each end's clip w, the distance along the
/// view axis in world units.
fn project_segment_depth(
    camera: &MarkCamera,
    area: Viewport,
    [a, b]: [[f32; 3]; 2],
) -> Option<([[f32; 2]; 2], [f32; 2])> {
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
    let at = |t| -> [f32; 4] { std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t) };
    let (p, q) = (at(start), at(end));
    Some((
        [pixels(camera, area, p)?, pixels(camera, area, q)?],
        [p[3], q[3]],
    ))
}

/// Pixels of a clip-space point that may lie off screen (the GPU clips).
fn pixels_of(camera: &MarkCamera, area: Viewport, point: [f32; 4]) -> Option<([f32; 2], f32)> {
    Some((pixels(camera, area, point)?, point[3]))
}

/// The pixel scale: style widths are given at 1080p.
fn pixel_scale(output_height: u32) -> f32 {
    output_height as f32 / 1080.0
}

/// Opacity left at `depth` world units from the camera: 1 up to the
/// fade's start, 0 from its end, linear between. No fade, or an empty
/// range, leaves it at 1.
fn fade_factor(fade: Option<Fade>, depth: f32) -> f32 {
    match fade {
        Some(Fade { start, end }) if end > start => {
            (1.0 - (depth - start) / (end - start)).clamp(0.0, 1.0)
        }
        _ => 1.0,
    }
}

/// Brightness at `seconds`: 1 at the top of the cycle, `1 - depth` at the
/// bottom.
fn pulse_factor(pulse: Option<Pulse>, seconds: f32) -> f32 {
    match pulse {
        Some(Pulse { period, depth }) if period > 0.0 => {
            let wave = 0.5 - 0.5 * (std::f32::consts::TAU * seconds / period).cos();
            1.0 - depth.clamp(0.0, 1.0) * wave
        }
        _ => 1.0,
    }
}

/// One corner of a line quad: its pixel and its (along, across) position
/// from the segment's start, along its direction and its left normal.
type Corner = ([f32; 2], [f32; 2]);

/// The quad around a screen segment: `outer` pixels either side and
/// `extend` pixels past each end. None for a non-finite segment.
fn strip_quad([a, b]: [[f32; 2]; 2], outer: f32, extend: f32) -> Option<[Corner; 4]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    if !length.is_finite() || !a.iter().chain(&b).all(|v| v.is_finite()) {
        return None;
    }
    let (ux, uy) = if length > 1e-4 {
        (dx / length, dy / length)
    } else {
        (1.0, 0.0)
    };
    let (nx, ny) = (-uy * outer, ux * outer);
    let start = [a[0] - ux * extend, a[1] - uy * extend];
    let end = [b[0] + ux * extend, b[1] + uy * extend];
    Some([
        ([start[0] + nx, start[1] + ny], [-extend, outer]),
        ([start[0] - nx, start[1] - ny], [-extend, -outer]),
        ([end[0] - nx, end[1] - ny], [length + extend, -outer]),
        ([end[0] + nx, end[1] + ny], [length + extend, outer]),
    ])
}

/// The most arrowheads along one edge.
const MAX_CHEVRONS: usize = 16;

/// How many arrowheads fit along an edge of `length` world units and the
/// gap between them: at least one, centred.
fn chevron_layout(length: f32, spacing: f32) -> (usize, f32) {
    if !(length.is_finite() && length > 0.0) {
        return (0, 0.0);
    }
    let fit = if spacing > 0.0 {
        (length / spacing) as usize
    } else {
        1
    };
    let count = fit.clamp(1, MAX_CHEVRONS);
    (count, length / count as f32)
}

/// Where an arrowhead is in its flow at `seconds`: how far back from its
/// resting place along the direction (zero or negative) and how opaque.
/// No speed keeps it at rest, opaque.
fn chevron_flow(seconds: f32, speed: f32, travel: f32) -> (f32, f32) {
    if speed <= 0.0 || travel <= 0.0 {
        return (0.0, 1.0);
    }
    let phase = (seconds * speed / travel).fract();
    (
        -(1.0 - phase) * travel,
        (std::f32::consts::PI * phase).sin(),
    )
}

/// The three points of an arrowhead lying flat at `at`: tip, left wing,
/// right wing. `direction` is flattened to the XY plane; None when it has
/// no length there.
fn chevron_points(at: [f32; 3], direction: [f32; 3], size: f32) -> Option<[[f32; 3]; 3]> {
    let len = direction[0].hypot(direction[1]);
    if !(len > 1e-6 && len.is_finite()) {
        return None;
    }
    let (dx, dy) = (direction[0] / len, direction[1] / len);
    let tip = [at[0] + dx * size * 0.5, at[1] + dy * size * 0.5, at[2]];
    let back = [at[0] - dx * size * 0.5, at[1] - dy * size * 0.5];
    let wing = |sign: f32| {
        [
            back[0] - dy * sign * size * 0.7,
            back[1] + dx * sign * size * 0.7,
            at[2],
        ]
    };
    Some([tip, wing(1.0), wing(-1.0)])
}

/// The floor strip along an edge: its four corners, `width` world units
/// across, flat to the XY plane. None for an edge that is a point or
/// straight up.
fn ribbon_corners([a, b]: [[f32; 3]; 2], width: f32) -> Option<[[f32; 3]; 4]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if !(len > 1e-6 && len.is_finite() && width > 0.0) {
        return None;
    }
    let (ox, oy) = (-dy / len * width * 0.5, dx / len * width * 0.5);
    Some([
        [a[0] + ox, a[1] + oy, a[2]],
        [a[0] - ox, a[1] - oy, a[2]],
        [b[0] - ox, b[1] - oy, b[2]],
        [b[0] + ox, b[1] + oy, b[2]],
    ])
}

/// Clips a convex clip-space polygon against the near plane (z >= 0),
/// before the divide by w. At most one vertex more than it had.
fn clip_near(polygon: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (da, db) = (a[2], b[2]);
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t));
        }
    }
    out
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
mod renderer {
    use font8x8::UnicodeFonts;

    use gfx::world_lines::Chevrons;

    use super::*;

    /// Floats in one vertex: position, colour, along/across, shape.
    const FLOATS: usize = 12;
    const STRIDE: u64 = (FLOATS * 4) as u64;
    /// A shape's half-width for a flat fill: the whole quad is core.
    const FILL: f32 = 1.0e4;

    /// Triangles, one buffer, one draw. Every vertex carries its own
    /// coverage shape: `shape` is (core half-width, outer half-width,
    /// dash period, glow opacity), all in pixels, and `local` the
    /// pixel's (along, across) place on its line.
    const SHADER: &str = r#"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) shape: vec4<f32>,
}
@vertex
fn vs(
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) shape: vec4<f32>,
) -> Out {
    return Out(vec4<f32>(pos, 0.0, 1.0), color, local, shape);
}
@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    if in.shape.z > 0.0 && fract(in.local.x / in.shape.z) > 5.0 / 9.0 {
        discard;
    }
    let d = abs(in.local.y);
    let core = in.shape.x;
    let outer = in.shape.y;
    let edge = 1.0 - smoothstep(core - 0.75, core + 0.75, d);
    var halo = 0.0;
    if outer > core + 0.75 {
        let t = 1.0 - smoothstep(core, outer, d);
        halo = in.shape.w * t * t;
    }
    let alpha = in.color.a * (edge + (1.0 - edge) * halo);
    return vec4<f32>(in.color.rgb, alpha);
}
"#;

    /// How one stroke is drawn, in pixels.
    struct Pen {
        color: [f32; 3],
        alpha: f32,
        fade: Option<Fade>,
        half: f32,
        outer: f32,
        glow_alpha: f32,
        dash: f32,
    }

    #[derive(Default)]
    pub(crate) struct MarkRenderer {
        pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
        vertices: Vec<u8>,
        buffer: Option<wgpu::Buffer>,
        seconds: f32,
        started: Option<Instant>,
    }

    impl MarkRenderer {
        /// Starts a frame; animations read the time since the first one.
        pub(crate) fn begin(&mut self) {
            self.vertices.clear();
            self.seconds = self
                .started
                .get_or_insert_with(Instant::now)
                .elapsed()
                .as_secs_f32();
        }

        pub(crate) fn push(
            &mut self,
            mark: &WorldLine<'_>,
            cameras: &MarkCameras,
            area: Viewport,
            size: (u32, u32),
        ) {
            let style = mark.style;
            let scale = pixel_scale(size.1);
            let alpha = (style.alpha * pulse_factor(style.pulse, self.seconds)).clamp(0.0, 1.0);
            let half = (style.width * scale * 0.5).max(0.5);
            let glow = style.glow.map_or(0.0, |g| g.width.max(0.0) * scale);
            let pen = Pen {
                color: mark.color,
                alpha,
                fade: style.fade,
                half,
                outer: (half + 0.75).max(half + glow),
                glow_alpha: style.glow.map_or(0.0, |g| g.alpha),
                dash: if mark.dashed {
                    9.0 * scale.max(1.0)
                } else {
                    0.0
                },
            };
            for camera in cameras.iter().flatten() {
                for &edge in mark.edges {
                    if let Some(ribbon) = style.ribbon {
                        self.ribbon(
                            camera,
                            area,
                            size,
                            edge,
                            ribbon.width,
                            alpha * ribbon.alpha,
                            &pen,
                        );
                    }
                    self.stroke(camera, area, size, edge, &pen);
                    if let Some(chevrons) = style.chevrons {
                        self.chevrons(camera, area, size, edge, chevrons, &pen);
                    }
                }
                if let Some(label) = mark.label {
                    let point = clip(camera, mark.anchor);
                    let fade = fade_factor(style.fade, point[3]);
                    if fade > 0.0
                        && let Some(point) = project_point(camera, area, mark.anchor)
                    {
                        let color = [mark.color[0], mark.color[1], mark.color[2], fade];
                        self.label(label, point, color, size, camera, area);
                    }
                }
            }
        }

        fn vertex(
            &mut self,
            [x, y]: [f32; 2],
            color: [f32; 4],
            local: [f32; 2],
            shape: [f32; 4],
            size: (u32, u32),
        ) {
            let pos = [2.0 * x / size.0 as f32 - 1.0, 1.0 - 2.0 * y / size.1 as f32];
            for value in pos.into_iter().chain(color).chain(local).chain(shape) {
                self.vertices.extend(value.to_ne_bytes());
            }
        }

        /// Two triangles from a quad's corners, in order around it.
        fn quad(&mut self, corners: [(Corner, [f32; 4], [f32; 4]); 4], size: (u32, u32)) {
            for i in [0, 1, 2, 0, 2, 3] {
                let ((pos, local), color, shape) = corners[i];
                self.vertex(pos, color, local, shape, size);
            }
        }

        /// One world edge as an anti-aliased quad with its halo.
        fn stroke(
            &mut self,
            camera: &MarkCamera,
            area: Viewport,
            size: (u32, u32),
            edge: [[f32; 3]; 2],
            pen: &Pen,
        ) {
            let Some((line, depth)) = project_segment_depth(camera, area, edge) else {
                return;
            };
            let Some(quad) = strip_quad(line, pen.outer, pen.half) else {
                return;
            };
            let shape = [pen.half, pen.outer, pen.dash, pen.glow_alpha];
            let color = |depth| {
                let a = pen.alpha * fade_factor(pen.fade, depth);
                [pen.color[0], pen.color[1], pen.color[2], a]
            };
            // Corners 0, 1 sit at the start; 2, 3 at the end.
            let colors = [
                color(depth[0]),
                color(depth[0]),
                color(depth[1]),
                color(depth[1]),
            ];
            if colors[0][3] <= 0.0 && colors[2][3] <= 0.0 {
                return;
            }
            self.quad(std::array::from_fn(|i| (quad[i], colors[i], shape)), size);
        }

        /// The floor strip along an edge: a flat fill, near-plane clipped.
        #[allow(clippy::too_many_arguments)]
        fn ribbon(
            &mut self,
            camera: &MarkCamera,
            area: Viewport,
            size: (u32, u32),
            edge: [[f32; 3]; 2],
            width: f32,
            alpha: f32,
            pen: &Pen,
        ) {
            let Some(corners) = ribbon_corners(edge, width) else {
                return;
            };
            let clipped = clip_near(&corners.map(|c| clip(camera, c)));
            let points: Vec<([f32; 2], f32)> = clipped
                .into_iter()
                .filter_map(|p| pixels_of(camera, area, p))
                .collect();
            if points.len() < 3 {
                return;
            }
            let shape = [FILL, FILL, 0.0, 0.0];
            for i in 1..points.len() - 1 {
                for (pos, depth) in [points[0], points[i], points[i + 1]] {
                    let a = alpha * fade_factor(pen.fade, depth);
                    let color = [pen.color[0], pen.color[1], pen.color[2], a];
                    self.vertex(pos, color, [0.0, 0.0], shape, size);
                }
            }
        }

        /// Arrowheads along an edge, flowing along their direction.
        #[allow(clippy::too_many_arguments)]
        fn chevrons(
            &mut self,
            camera: &MarkCamera,
            area: Viewport,
            size: (u32, u32),
            [a, b]: [[f32; 3]; 2],
            chevrons: Chevrons,
            pen: &Pen,
        ) {
            let delta = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
            let (count, step) = chevron_layout(length, chevrons.spacing);
            let flat = chevrons.direction[0].hypot(chevrons.direction[1]);
            if count == 0 || flat <= 1e-6 {
                return;
            }
            let travel = chevrons.size * 4.0;
            let (back, weight) = chevron_flow(self.seconds, chevrons.speed, travel);
            let heading = [chevrons.direction[0] / flat, chevrons.direction[1] / flat];
            let stroke = Pen {
                half: (pen.half * 0.8).max(0.75),
                outer: (pen.half * 0.8).max(0.75) + 0.75,
                glow_alpha: 0.0,
                dash: 0.0,
                alpha: pen.alpha * weight,
                color: pen.color,
                fade: pen.fade,
            };
            for i in 0..count {
                let s = (i as f32 + 0.5) * step / length;
                let at = [
                    a[0] + delta[0] * s + heading[0] * back,
                    a[1] + delta[1] * s + heading[1] * back,
                    a[2] + delta[2] * s,
                ];
                let Some([tip, left, right]) =
                    chevron_points(at, chevrons.direction, chevrons.size)
                else {
                    return;
                };
                self.stroke(camera, area, size, [left, tip], &stroke);
                self.stroke(camera, area, size, [right, tip], &stroke);
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
                let Some(glyph) = font8x8::BASIC_FONTS
                    .get(c)
                    .or_else(|| font8x8::LATIN_FONTS.get(c))
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
                        // A one-pixel-high run, covering whole pixels.
                        let top = y + row as f32;
                        let (x0, x1) = (x + first as f32, x + col as f32);
                        let shape = [FILL, FILL, 0.0, 0.0];
                        let corner = |pos| ((pos, [0.0, 0.0]), color, shape);
                        self.quad(
                            [
                                corner([x0, top]),
                                corner([x0, top + 1.0]),
                                corner([x1, top + 1.0]),
                                corner([x1, top]),
                            ],
                            size,
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
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x2, 1 => Float32x4, 2 => Float32x2, 3 => Float32x4
                        ],
                    })],
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
                    topology: wgpu::PrimitiveTopology::TriangleList,
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
            if self
                .buffer
                .as_ref()
                .is_none_or(|buffer| buffer.size() < bytes)
            {
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
            let count = (bytes / STRIDE) as u32;
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

    const AREA: Viewport = Viewport {
        x: 40,
        y: 10,
        width: 200,
        height: 100,
    };

    #[test]
    fn marks_project_straight_ahead_to_the_viewport_centre() {
        assert_eq!(
            project_point(&CAMERA, AREA, [0.0, 0.0, -3.0]),
            Some([190.0, 60.0])
        );
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

    #[test]
    fn project_segment_depth_reports_w_at_the_clipped_ends() {
        let (points, depth) =
            project_segment_depth(&CAMERA, AREA, [[0.5, 0.0, 0.0], [0.5, 0.0, -2.0]]).unwrap();
        assert_eq!(points, [[215.0, 60.0], [202.5, 60.0]]);
        // Clipped at the near plane (w = 1), the far end is at w = 2.
        assert!((depth[0] - 1.0).abs() < 1e-4 && (depth[1] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn pixel_scale_is_relative_to_1080p() {
        assert_eq!(pixel_scale(1080), 1.0);
        assert_eq!(pixel_scale(2160), 2.0);
        assert_eq!(pixel_scale(540), 0.5);
    }

    #[test]
    fn fade_runs_from_full_to_nothing_and_off_without_a_range() {
        let fade = Some(Fade {
            start: 25.0,
            end: 45.0,
        });
        assert_eq!(fade_factor(fade, 10.0), 1.0);
        assert_eq!(fade_factor(fade, 25.0), 1.0);
        assert_eq!(fade_factor(fade, 35.0), 0.5);
        assert_eq!(fade_factor(fade, 45.0), 0.0);
        assert_eq!(fade_factor(fade, 90.0), 0.0);
        assert_eq!(fade_factor(None, 90.0), 1.0);
        assert_eq!(
            fade_factor(
                Some(Fade {
                    start: 5.0,
                    end: 5.0
                }),
                90.0
            ),
            1.0
        );
    }

    #[test]
    fn pulse_is_full_at_the_cycle_start_and_dips_by_its_depth_half_way() {
        let pulse = Some(Pulse {
            period: 2.0,
            depth: 0.4,
        });
        assert!((pulse_factor(pulse, 0.0) - 1.0).abs() < 1e-6);
        assert!((pulse_factor(pulse, 1.0) - 0.6).abs() < 1e-6);
        assert!((pulse_factor(pulse, 2.0) - 1.0).abs() < 1e-6);
        assert_eq!(pulse_factor(None, 1.0), 1.0);
        assert_eq!(
            pulse_factor(
                Some(Pulse {
                    period: 0.0,
                    depth: 1.0
                }),
                1.0
            ),
            1.0
        );
    }

    #[test]
    fn a_strip_quad_is_the_segment_widened_and_extended() {
        let quad = strip_quad([[10.0, 20.0], [30.0, 20.0]], 3.0, 1.0).unwrap();
        let pos: Vec<_> = quad.iter().map(|c| c.0).collect();
        // The left normal of +x is +y (down the screen, y grows down).
        assert_eq!(pos, [[9.0, 23.0], [9.0, 17.0], [31.0, 17.0], [31.0, 23.0]]);
        let local: Vec<_> = quad.iter().map(|c| c.1).collect();
        assert_eq!(
            local,
            [[-1.0, 3.0], [-1.0, -3.0], [21.0, -3.0], [21.0, 3.0]]
        );
    }

    #[test]
    fn a_strip_quad_survives_a_point_and_rejects_non_finite_input() {
        let quad = strip_quad([[5.0, 5.0], [5.0, 5.0]], 2.0, 0.0).unwrap();
        assert!(quad.iter().all(|c| c.0.iter().all(|v| v.is_finite())));
        assert!(strip_quad([[f32::NAN, 0.0], [1.0, 1.0]], 2.0, 0.0).is_none());
    }

    #[test]
    fn chevrons_fit_at_least_one_and_at_most_the_cap() {
        assert_eq!(chevron_layout(2.0, 0.5), (4, 0.5));
        assert_eq!(chevron_layout(0.2, 0.5), (1, 0.2));
        assert_eq!(
            chevron_layout(100.0, 0.5),
            (MAX_CHEVRONS, 100.0 / MAX_CHEVRONS as f32)
        );
        assert_eq!(chevron_layout(0.0, 0.5).0, 0);
        assert_eq!(chevron_layout(f32::NAN, 0.5).0, 0);
        assert_eq!(chevron_layout(3.0, 0.0), (1, 3.0));
    }

    #[test]
    fn chevrons_at_rest_are_opaque_and_in_flow_fade_in_and_out() {
        assert_eq!(chevron_flow(3.0, 0.0, 1.0), (0.0, 1.0));
        let (back, weight) = chevron_flow(0.0, 1.0, 2.0);
        assert_eq!((back, weight), (-2.0, 0.0));
        let (back, weight) = chevron_flow(1.0, 1.0, 2.0);
        assert!((back + 1.0).abs() < 1e-6 && (weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_chevron_points_along_its_direction_with_wings_behind() {
        let [tip, left, right] = chevron_points([0.0, 0.0, 1.0], [0.0, 2.0, 5.0], 1.0).unwrap();
        assert_eq!(tip, [0.0, 0.5, 1.0]);
        assert!(left[1] < 0.0 && right[1] < 0.0);
        assert!((left[0] + right[0]).abs() < 1e-6 && left[0] != right[0]);
        assert!(chevron_points([0.0; 3], [0.0, 0.0, 1.0], 1.0).is_none());
    }

    #[test]
    fn a_ribbon_is_a_floor_strip_across_the_edge() {
        let corners = ribbon_corners([[1.0, 0.0, 2.0], [1.0, 4.0, 2.0]], 0.2).unwrap();
        for c in corners {
            assert_eq!(c[2], 2.0);
            assert!((c[0] - 1.0).abs() - 0.1 < 1e-6);
        }
        assert!(ribbon_corners([[0.0, 0.0, 0.0], [0.0, 0.0, 3.0]], 0.2).is_none());
        assert!(ribbon_corners([[0.0; 3], [1.0, 0.0, 0.0]], 0.0).is_none());
    }

    #[test]
    fn clip_near_cuts_a_polygon_at_z_zero() {
        let square = [
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 0.0, 1.0, 1.0],
            [1.0, 1.0, -1.0, 1.0],
            [0.0, 1.0, -1.0, 1.0],
        ];
        let out = clip_near(&square);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|p| p[2] >= -1e-6));
        // Fully in front is unchanged; fully behind is empty.
        assert_eq!(
            clip_near(
                &square[..2]
                    .iter()
                    .map(|p| *p)
                    .chain([[0.5, 1.0, 1.0, 1.0]])
                    .collect::<Vec<_>>()
            )
            .len(),
            3
        );
        let behind = square.map(|p| [p[0], p[1], -1.0, p[3]]);
        assert!(clip_near(&behind).is_empty());
    }
}
