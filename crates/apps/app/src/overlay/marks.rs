//! World-space lines and labels over the finished frame.
use std::sync::Arc;
use std::time::Instant;

pub use gfx::world_lines::ImageSamples;
use gfx::world_lines::{Fade, HudMark, LineStyle, Pulse, ShownMarks, TextLayout, WorldLine};

use crate::scale::Viewport;

#[derive(Clone, Copy, Debug)]
pub struct MarkCamera {
    /// World-to-clip matrix, column-major, with WGPU depth in 0..w.
    pub view_projection: [[f32; 4]; 4],
    /// Top, left, bottom, right as fractions of the whole guest image.
    pub viewport: [f32; 4],
}

pub type MarkCameras = [Option<MarkCamera>; 4];

/// Which camera a frame's marks are projected with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkCameraSource {
    /// The genuine image's own cameras: its tick's.
    Tick,
    /// An in-between frame the host made at `fraction`. `shifted`: some
    /// view was drawn with another camera than the marks' image, and the
    /// cameras here were moved onto it (`gfx::present_camera`).
    Made { fraction: f32, shifted: bool },
}

/// All sources on an image use its per-view cameras.
#[derive(Debug)]
pub struct ProjectedMarks {
    pub cameras: MarkCameras,
    pub samples: Arc<ImageSamples>,
    pub source: MarkCameraSource,
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

    /// The samples' HUD-space marks ([`HudMark`]), after the world lines.
    pub(crate) fn visit_hud(&self, now: Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
        self.samples.visit_hud(&|| now, visitor);
    }
}

/// The guest image is 640 by 480 guest pixels however large it is drawn.
const GUEST_SIZE: [f32; 2] = [640.0, 480.0];

/// The window pixel of a HUD-space point (guest pixels of the 640 by 480
/// image) in the image's `area`.
fn hud_pixel(area: Viewport, [x, y]: [f32; 2]) -> [f32; 2] {
    [
        area.x as f32 + x * area.width as f32 / GUEST_SIZE[0],
        area.y as f32 + y * area.height as f32 / GUEST_SIZE[1],
    ]
}

/// Window pixels in one guest pixel of height: what HUD widths and font
/// pixels scale by.
fn hud_scale(area: Viewport) -> f32 {
    area.height as f32 / GUEST_SIZE[1]
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

    /// What the `render.marks` probe calls this frame.
    pub(crate) fn probe_kind(&self) -> gfx::world_lines::probe::Kind {
        use gfx::world_lines::probe::Kind;
        match self {
            Self::Guest(_) => Kind::Guest,
            Self::GuestWithoutWorldView => Kind::GuestWithoutWorldView,
            Self::Synthetic(_) => Kind::Synthetic,
            Self::Raw => Kind::Raw,
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

/// Where the window drew a frame: one [`gfx::world_lines::probe::Presented`]
/// for the `render.marks` probe, while it is armed. `anchors` are the
/// groups' anchors in window pixels through the first view's camera.
pub(crate) fn record_probe(
    marks: Option<&FrameMarks>,
    loading: bool,
    anchors: Vec<Option<[f32; 2]>>,
    at: Instant,
) {
    use gfx::world_lines::probe::{Kind, PRESENTED, Presented};
    if !PRESENTED.armed() {
        return;
    }
    let (fraction, shifted) = match marks.and_then(FrameMarks::marks).map(|m| m.source) {
        Some(MarkCameraSource::Made { fraction, shifted }) => (Some(fraction), shifted),
        _ => (None, false),
    };
    let kind = match marks {
        _ if loading => Kind::Loading,
        Some(m) => m.probe_kind(),
        None => Kind::Raw,
    };
    PRESENTED.record(Presented {
        seq: 0,
        at,
        kind,
        fraction,
        shifted,
        anchors,
    });
}

/// `anchor` in window pixels through the first usable view of `cameras`;
/// `None` off screen or behind it (the `render.marks` probe).
pub(crate) fn anchor_pixels(
    cameras: &MarkCameras,
    area: Viewport,
    anchor: [f32; 3],
) -> Option<[f32; 2]> {
    project_point(cameras.iter().flatten().next()?, area, anchor)
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

/// Clip a segment in homogeneous coordinates before dividing by w, with
/// each end's clip w, the distance along the view axis in world units.
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

/// The size of one font pixel of a label: its style's scale, and for a
/// centred (prompt) label also the output height over 1080p, at least 1.
fn label_glyph_scale(style: &LineStyle, pixel_scale: f32) -> f32 {
    let scale = style.label_scale.max(0.25);
    if style.label_centered {
        scale * pixel_scale.max(1.0)
    } else {
        scale
    }
}

/// The top-left pixel of a label of `chars` glyphs at `point`: just below
/// and right of it, or centred on it.
fn label_origin(point: [f32; 2], chars: usize, glyph: f32, centered: bool) -> [f32; 2] {
    text_origin(point, [chars as f32 * 8.0 * glyph, 8.0 * glyph], centered)
}

/// The top-left pixel of a label `size` pixels wide and tall at `point`:
/// just below and right of it, or centred on it.
fn text_origin([x, y]: [f32; 2], [width, height]: [f32; 2], centered: bool) -> [f32; 2] {
    if centered {
        [x - width * 0.5, y - height * 0.5]
    } else {
        [x.ceil() + 5.0, y.ceil() + 5.0]
    }
}

/// The HUD is drawn for this many lines: a disc font's pixel is a HUD
/// pixel at `label_scale` [`HUD_FACE_SCALE`].
const HUD_LINES: f32 = 480.0;
/// The `label_scale` at which a HUD-font label is the HUD's own size.
const HUD_FACE_SCALE: f32 = 2.0;

/// Output pixels in one font pixel of a HUD-font label: the output height
/// over the HUD's, times the style's scale over [`HUD_FACE_SCALE`].
fn hud_font_pixel(style: &LineStyle, output_height: u32) -> f32 {
    style.label_scale.max(0.25) / HUD_FACE_SCALE * output_height as f32 / HUD_LINES
}

/// One glyph of a HUD-font label on screen: its pixel rectangle (left,
/// top, right, bottom), its atlas rectangle in 0..1, and whether it keeps
/// its own colours.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GlyphQuad {
    rect: [f32; 4],
    uv: [f32; 4],
    coloured: bool,
}

/// The quads of a laid-out label with its top-left at `origin` (rounded
/// to a whole pixel) and `pixel` output pixels a font pixel, for an atlas
/// `atlas` texels in size. Glyphs whose left edge is past `right` are
/// dropped.
fn glyph_quads(
    layout: &TextLayout,
    atlas: [u32; 2],
    origin: [f32; 2],
    pixel: f32,
    right: f32,
) -> Vec<GlyphQuad> {
    let (ox, oy) = (origin[0].round(), origin[1].round());
    let (aw, ah) = (atlas[0].max(1) as f32, atlas[1].max(1) as f32);
    layout
        .glyphs
        .iter()
        .filter_map(|p| {
            let g = &p.glyph;
            let left = ox + p.x as f32 * pixel;
            if left > right {
                return None;
            }
            let top = oy + p.y as f32 * pixel;
            let [tx, ty] = [g.atlas[0] as f32, g.atlas[1] as f32];
            Some(GlyphQuad {
                rect: [
                    left,
                    top,
                    left + f32::from(g.width) * pixel,
                    top + f32::from(g.height) * pixel,
                ],
                uv: [
                    tx / aw,
                    ty / ah,
                    (tx + f32::from(g.width)) / aw,
                    (ty + f32::from(g.height)) / ah,
                ],
                coloured: g.coloured,
            })
        })
        .collect()
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

    use gfx::world_lines::{Chevrons, LabelFace, LabelFont};

    use super::*;

    /// Floats in one vertex: position, colour, along/across, shape.
    const FLOATS: usize = 12;
    const STRIDE: u64 = (FLOATS * 4) as u64;
    /// A shape's half-width for a flat fill: the whole quad is core.
    const FILL: f32 = 1.0e4;

    /// A shape's core half-width that marks a textured glyph quad: its
    /// `local` is then the atlas coordinate.
    const GLYPH: f32 = -1.0;

    /// Triangles, one buffer, one draw. Every vertex carries its own
    /// coverage shape: `shape` is (core half-width, outer half-width,
    /// dash period, glow opacity), all in pixels, and `local` the
    /// pixel's (along, across) place on its line. A HUD-font glyph has a
    /// negative core and samples the font atlas at `local`, its colour
    /// times the vertex's.
    const SHADER: &str = r#"
@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
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
    if in.shape.x < 0.0 {
        let t = textureSampleLevel(atlas, atlas_sampler, in.local, 0.0);
        return vec4<f32>(in.color.rgb * t.rgb, in.color.a * t.a);
    }
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

    /// The bind group layout, pipeline layout and sampler of the font
    /// atlas, and a blank atlas for frames without a HUD font.
    struct Binding {
        bgl: wgpu::BindGroupLayout,
        layout: wgpu::PipelineLayout,
        sampler: wgpu::Sampler,
        blank: wgpu::BindGroup,
    }

    /// The uploaded atlas of one [`LabelFont`] generation.
    struct Atlas {
        generation: u64,
        bind: wgpu::BindGroup,
    }

    #[derive(Default)]
    pub(crate) struct MarkRenderer {
        pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
        vertices: Vec<u8>,
        buffer: Option<wgpu::Buffer>,
        seconds: f32,
        started: Option<Instant>,
        /// The HUD font for this frame's [`LabelFace::Hud`] labels.
        font: Option<Arc<LabelFont>>,
        binding: Option<Binding>,
        atlas: Option<Atlas>,
    }

    fn atlas_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (width, height): (u32, u32),
        rgba: &[u8],
    ) -> wgpu::TextureView {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mark font atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        texture.create_view(&Default::default())
    }

    fn atlas_bind(
        device: &wgpu::Device,
        binding: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mark font atlas"),
            layout: binding,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
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
            self.font = gfx::world_lines::hud_font();
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
                    if style.stroke {
                        self.stroke(camera, area, size, edge, &pen);
                    }
                    if let Some(chevrons) = style.chevrons {
                        self.chevrons(camera, area, size, edge, chevrons, &pen);
                    }
                }
                if let Some(label) = mark.label {
                    let point = clip(camera, mark.anchor);
                    let fade = fade_factor(style.fade, point[3]);
                    if fade > 0.0
                        && let Some(point) = project_point(camera, area, mark.anchor)
                        && !(style.label_face == LabelFace::Hud
                            && gfx::world_lines::label_under_hud([
                                (point[0] - area.x as f32) / area.width as f32,
                                (point[1] - area.y as f32) / area.height as f32,
                            ]))
                    {
                        let a = style.alpha.clamp(0.0, 1.0)
                            * fade
                            * pulse_factor(style.pulse, self.seconds);
                        let color = [mark.color[0], mark.color[1], mark.color[2], a];
                        // A HUD-face label without a HUD font loaded is drawn in
                        // the 8x8 font; its source notes that (LabelFace::Hud).
                        let font = match style.label_face {
                            LabelFace::Hud => self.font.clone(),
                            LabelFace::Debug => None,
                        };
                        if let Some(font) = font {
                            let pixel = hud_font_pixel(&style, size.1);
                            self.hud_label(
                                &font,
                                label,
                                point,
                                color,
                                pixel,
                                style.label_centered,
                                size,
                                camera,
                                area,
                            );
                        } else {
                            let glyph = label_glyph_scale(&style, scale);
                            self.label(label, point, color, glyph, style.label_centered, size, camera, area);
                        }
                    }
                }
            }
        }

        /// One HUD-space mark ([`HudMark`]) in the image's `area`: its
        /// guest pixels scaled with the image, as the guest's own HUD is.
        pub(crate) fn push_hud(&mut self, mark: &HudMark<'_>, area: Viewport, size: (u32, u32)) {
            let scale = hud_scale(area);
            match *mark {
                HudMark::Path {
                    points,
                    closed,
                    width,
                    dashed,
                    color,
                    alpha,
                } => {
                    if points.len() < 2 {
                        return;
                    }
                    let half = (width * scale * 0.5).max(0.5);
                    let pen = Pen {
                        color,
                        alpha: alpha.clamp(0.0, 1.0),
                        fade: None,
                        half,
                        outer: half + 0.75,
                        glow_alpha: 0.0,
                        dash: if dashed { 6.0 * scale.max(1.0) } else { 0.0 },
                    };
                    let px: Vec<[f32; 2]> = points.iter().map(|&p| hud_pixel(area, p)).collect();
                    let n = px.len();
                    let segments = if closed { n } else { n - 1 };
                    let mut along = 0.0;
                    for i in 0..segments {
                        let (a, b) = (px[i], px[(i + 1) % n]);
                        self.stroke_pixels([a, b], along, &pen, size);
                        along += (b[0] - a[0]).hypot(b[1] - a[1]);
                    }
                }
                HudMark::Fill {
                    points,
                    color,
                    alpha,
                } => {
                    if points.len() < 3 {
                        return;
                    }
                    let c = [color[0], color[1], color[2], alpha.clamp(0.0, 1.0)];
                    let shape = [FILL, FILL, 0.0, 0.0];
                    let first = hud_pixel(area, points[0]);
                    for w in points[1..].windows(2) {
                        for p in [first, hud_pixel(area, w[0]), hud_pixel(area, w[1])] {
                            self.vertex(p, c, [0.0, 0.0], shape, size);
                        }
                    }
                }
                HudMark::Text {
                    at,
                    text,
                    color,
                    alpha,
                    pixel,
                    centered,
                } => {
                    let point = hud_pixel(area, at);
                    let c = [color[0], color[1], color[2], alpha.clamp(0.0, 1.0)];
                    let px = pixel * scale;
                    // The whole image: text is clipped at its right and bottom.
                    let whole = MarkCamera {
                        view_projection: [[0.0; 4]; 4],
                        viewport: [0.0, 0.0, 1.0, 1.0],
                    };
                    if let Some(font) = self.font.clone() {
                        self.hud_label(&font, text, point, c, px, centered, size, &whole, area);
                    } else {
                        self.label(text, point, c, px, centered, size, &whole, area);
                    }
                }
            }
        }

        /// A window-pixel segment as an anti-aliased quad, its dashes
        /// counted from `along` pixels into its path.
        fn stroke_pixels(&mut self, line: [[f32; 2]; 2], along: f32, pen: &Pen, size: (u32, u32)) {
            let Some(quad) = strip_quad(line, pen.outer, pen.half) else {
                return;
            };
            let shape = [pen.half, pen.outer, pen.dash, pen.glow_alpha];
            let color = [pen.color[0], pen.color[1], pen.color[2], pen.alpha];
            self.quad(
                std::array::from_fn(|i| {
                    let (pos, local) = quad[i];
                    ((pos, [local[0] + along, local[1]]), color, shape)
                }),
                size,
            );
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
            point: [f32; 2],
            color: [f32; 4],
            glyph: f32,
            centered: bool,
            size: (u32, u32),
            camera: &MarkCamera,
            area: Viewport,
        ) {
            let [_, _, bottom, right] = camera.viewport;
            let right = area.x as f32 + right * area.width as f32;
            let bottom = area.y as f32 + bottom * area.height as f32;
            let text = crate::overlay::overlay_text(label);
            let [x, y] = label_origin(point, text.chars().count(), glyph, centered);
            if y + 8.0 * glyph > bottom {
                return;
            }
            if centered {
                // A dark drop shadow keeps big text readable on any floor.
                let shadow = [0.0, 0.0, 0.0, color[3] * 0.7];
                let off = glyph.max(1.0);
                self.glyphs(&text, [x + off, y + off], shadow, glyph, right, size);
            }
            self.glyphs(&text, [x, y], color, glyph, right, size);
        }

        /// A label in the disc's HUD font: one textured quad a glyph, with a
        /// drop shadow when centred. Button glyphs keep their colours.
        fn hud_label(
            &mut self,
            font: &LabelFont,
            label: &str,
            point: [f32; 2],
            color: [f32; 4],
            pixel: f32,
            centered: bool,
            size: (u32, u32),
            camera: &MarkCamera,
            area: Viewport,
        ) {
            let [_, _, bottom, right] = camera.viewport;
            let right = area.x as f32 + right * area.width as f32;
            let bottom = area.y as f32 + bottom * area.height as f32;
            let layout = font.layout(label);
            let box_size = [
                layout.width as f32 * pixel,
                font.line_height() as f32 * pixel,
            ];
            let [x, y] = text_origin(point, box_size, centered);
            if y + box_size[1] > bottom {
                return;
            }
            let mut glyphs = |at: [f32; 2], color: [f32; 4], shadow: bool| {
                for q in glyph_quads(&layout, font.size, at, pixel, right) {
                    let tint = if q.coloured && !shadow {
                        [1.0, 1.0, 1.0, color[3]]
                    } else {
                        color
                    };
                    let shape = [GLYPH, 0.0, 0.0, 0.0];
                    let [l, t, r, b] = q.rect;
                    let [u0, v0, u1, v1] = q.uv;
                    let corner = |pos, uv| ((pos, uv), tint, shape);
                    self.quad(
                        [
                            corner([l, t], [u0, v0]),
                            corner([l, b], [u0, v1]),
                            corner([r, b], [u1, v1]),
                            corner([r, t], [u1, v0]),
                        ],
                        size,
                    );
                }
            };
            if centered {
                let off = pixel.max(1.0);
                glyphs([x + off, y + off], [0.0, 0.0, 0.0, color[3] * 0.7], true);
            }
            glyphs([x, y], color, false);
        }

        /// The text's lit font pixels as runs of `glyph`-pixel squares.
        fn glyphs(
            &mut self,
            text: &str,
            [x, y]: [f32; 2],
            color: [f32; 4],
            glyph: f32,
            right: f32,
            size: (u32, u32),
        ) {
            for (i, c) in text.chars().enumerate() {
                let x = x + i as f32 * 8.0 * glyph;
                if x + 8.0 * glyph > right {
                    break;
                }
                let Some(font) = font8x8::BASIC_FONTS
                    .get(c)
                    .or_else(|| font8x8::LATIN_FONTS.get(c))
                else {
                    continue;
                };
                for (row, bits) in font.into_iter().enumerate() {
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
                        // One font row of a run, snapped to whole pixels.
                        let top = (y + row as f32 * glyph).round();
                        let bottom = (y + (row + 1) as f32 * glyph).round();
                        let x0 = (x + first as f32 * glyph).round();
                        let x1 = (x + col as f32 * glyph).round();
                        let shape = [FILL, FILL, 0.0, 0.0];
                        let corner = |pos| ((pos, [0.0, 0.0]), color, shape);
                        self.quad(
                            [
                                corner([x0, top]),
                                corner([x0, bottom]),
                                corner([x1, bottom]),
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
            let layout = &self.binding(device).layout;
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("editor marks"),
                layout: Some(layout),
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

        /// The atlas binding, made on first use with a 1x1 blank atlas.
        fn binding(&mut self, device: &wgpu::Device) -> &Binding {
            self.binding.get_or_insert_with(|| {
                let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("mark font atlas"),
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
                    ],
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("editor marks"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                });
                // Linear: labels are scaled to the output, rarely by a whole number.
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("mark font atlas"),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                });
                // wgpu clears a new texture: the blank atlas is transparent.
                let blank = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("mark blank atlas"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let blank = atlas_bind(device, &bgl, &sampler, &blank.create_view(&Default::default()));
                Binding {
                    bgl,
                    layout,
                    sampler,
                    blank,
                }
            })
        }

        /// The bind group for this frame: the HUD font's atlas, uploaded
        /// when its generation changes, or the blank one.
        fn bind_group(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::BindGroup {
            self.binding(device);
            let Some(font) = self.font.clone() else {
                return self.binding.as_ref().expect("atlas binding").blank.clone();
            };
            if self.atlas.as_ref().is_none_or(|a| a.generation != font.generation) {
                let view = atlas_texture(device, queue, (font.size[0], font.size[1]), &font.rgba);
                let binding = self.binding.as_ref().expect("atlas binding");
                self.atlas = Some(Atlas {
                    generation: font.generation,
                    bind: atlas_bind(device, &binding.bgl, &binding.sampler, &view),
                });
            }
            self.atlas.as_ref().expect("uploaded atlas").bind.clone()
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
            let bind = self.bind_group(device, queue);
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
            pass.set_bind_group(0, &bind, &[]);
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
    fn the_probe_reads_anchors_through_the_first_view_and_names_the_frame() {
        let cameras = [None, Some(CAMERA), None, None];
        assert_eq!(
            anchor_pixels(&cameras, AREA, [0.0, 0.0, -3.0]),
            Some([190.0, 60.0])
        );
        assert_eq!(anchor_pixels(&cameras, AREA, [0.0, 0.0, 3.0]), None, "behind");
        assert_eq!(anchor_pixels(&[None; 4], AREA, [0.0, 0.0, -3.0]), None);
        use gfx::world_lines::probe::Kind;
        assert_eq!(FrameMarks::Synthetic(None).probe_kind(), Kind::Synthetic);
        assert_eq!(FrameMarks::Guest(None).probe_kind(), Kind::Guest);
        assert_eq!(FrameMarks::Raw.probe_kind(), Kind::Raw);
        assert_eq!(
            FrameMarks::GuestWithoutWorldView.probe_kind().name(),
            "no_world_view"
        );
    }

    #[test]
    fn marks_drop_segments_behind_the_camera() {
        assert!(
            project_segment_depth(&CAMERA, AREA, [[0.0, 0.0, 1.0], [0.0, 1.0, 2.0]]).is_none()
        );
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
    fn hud_points_scale_with_the_guest_image() {
        // The image drawn at 1280 by 960 from (40, 10).
        let area = Viewport {
            x: 40,
            y: 10,
            width: 1280,
            height: 960,
        };
        assert_eq!(hud_pixel(area, [0.0, 0.0]), [40.0, 10.0]);
        assert_eq!(hud_pixel(area, [640.0, 480.0]), [1320.0, 970.0]);
        assert_eq!(hud_pixel(area, [88.5, 401.5]), [217.0, 813.0]);
        assert_eq!(hud_scale(area), 2.0);
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
                    .copied()
                    .chain([[0.5, 1.0, 1.0, 1.0]])
                    .collect::<Vec<_>>()
            )
            .len(),
            3
        );
        let behind = square.map(|p| [p[0], p[1], -1.0, p[3]]);
        assert!(clip_near(&behind).is_empty());
    }

    #[test]
    fn labels_are_centred_or_hang_off_their_anchor() {
        // Six 8-pixel glyphs at 2x are 96 wide and 16 tall: centred on the anchor.
        assert_eq!(label_origin([100.0, 50.0], 6, 2.0, true), [52.0, 42.0]);
        assert_eq!(label_origin([100.2, 50.0], 6, 1.0, false), [106.0, 55.0]);
    }

    #[test]
    fn a_hud_label_box_is_centred_or_hangs_off_its_anchor() {
        assert_eq!(text_origin([100.0, 50.0], [40.0, 20.0], true), [80.0, 40.0]);
        assert_eq!(text_origin([100.2, 50.0], [40.0, 20.0], false), [106.0, 55.0]);
    }

    #[test]
    fn a_hud_font_pixel_is_a_hud_pixel_at_scale_two() {
        let at = |scale| LineStyle {
            label_scale: scale,
            label_face: gfx::world_lines::LabelFace::Hud,
            ..LineStyle::default()
        };
        // 480 lines to 1080: 2.25 output pixels a HUD pixel.
        assert_eq!(hud_font_pixel(&at(2.0), 1080), 2.25);
        assert_eq!(hud_font_pixel(&at(1.0), 960), 1.0);
        assert_eq!(hud_font_pixel(&at(3.0), 480), 1.5);
        // A scale of nothing still draws something.
        assert!(hud_font_pixel(&at(0.0), 1080) > 0.0);
    }

    fn hud_font() -> gfx::world_lines::LabelFont {
        use gfx::world_lines::{GlyphPixels, LabelFont};
        let glyph = |unit: u16, width: u16, coloured: bool| GlyphPixels {
            unit,
            advance: width as i16 + 1,
            x_offset: 0,
            y_offset: 12,
            width,
            height: 12,
            rgba: vec![255; usize::from(width) * 12 * 4],
            coloured,
        };
        LabelFont::build(
            "test",
            12,
            4,
            vec![glyph(u16::from(b'H'), 8, false), glyph(0xe102, 16, true)],
            Vec::new(),
        )
    }

    #[test]
    fn glyph_quads_place_each_glyph_at_its_pen_scaled_with_its_atlas_box() {
        let font = hud_font();
        let layout = font.layout("H\u{e102}");
        let quads = glyph_quads(&layout, font.size, [10.4, 20.6], 2.0, 1000.0);
        assert_eq!(quads.len(), 2);
        // Rounded to (10, 21); H is 8 by 12 font pixels at 2 pixels each.
        assert_eq!(quads[0].rect, [10.0, 21.0, 26.0, 45.0]);
        // The button glyph follows H's advance of 9, and keeps its colours.
        assert_eq!(quads[1].rect, [28.0, 21.0, 60.0, 45.0]);
        assert!(quads[1].coloured && !quads[0].coloured);
        for (q, unit) in quads.iter().zip([u16::from(b'H'), 0xe102]) {
            let g = font.glyph(unit).unwrap();
            let [w, h] = [font.size[0] as f32, font.size[1] as f32];
            assert_eq!(q.uv[0] * w, g.atlas[0] as f32);
            assert_eq!(q.uv[1] * h, g.atlas[1] as f32);
            assert!(((q.uv[2] - q.uv[0]) * w - f32::from(g.width)).abs() < 1e-3);
            assert!(((q.uv[3] - q.uv[1]) * h - f32::from(g.height)).abs() < 1e-3);
        }
        // A glyph that starts past the viewport's right edge is dropped.
        assert_eq!(glyph_quads(&layout, font.size, [0.0, 0.0], 2.0, 10.0).len(), 1);
    }

    #[test]
    fn a_centred_label_scales_with_the_output_a_plain_one_does_not() {
        let plain = LineStyle { label_scale: 2.0, ..LineStyle::default() };
        let centred = LineStyle { label_centered: true, ..plain };
        assert_eq!(label_glyph_scale(&plain, 2.0), 2.0);
        assert_eq!(label_glyph_scale(&centred, 2.0), 4.0);
        assert_eq!(label_glyph_scale(&centred, 0.5), 2.0);
        assert_eq!(label_glyph_scale(&LineStyle::default(), 1.0), 1.0);
    }
}
