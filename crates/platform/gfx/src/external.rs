//! What a stage hands the renderer to join the frame: the target copy a
//! recording is for, prepared draw packets, encoded commands, the pinned
//! attachments a stage records into, and the plain value types its inputs
//! are made of (cached vertex and index snapshots, texture info, pixel
//! rectangles, the UI fit, the lighting mode). The old `d3d::backend` paths re-export
//! them until that crate goes.

use crate::format::D3DFormat;
use crate::texture::TextureKind;

/// Which physical copy of a guest target a recording draws into: the 1x
/// reference the guest can read back, or the display copy at the render
/// scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetCopy {
    Reference,
    Display,
}

/// Draws prepared with cached pipelines and bindings for an ordered run
/// of a stage on its attachments: appended to the render pass already
/// open on them, without an encoder of their own.
#[cfg(feature = "gpu")]
pub struct ExternalDraws {
    /// Guest ranges represented by the packets (a range may have several
    /// scissors, and adjacent ranges may share a packet).
    pub ranges: usize,
    pub packets: Vec<crate::RecordedDraw>,
    pub discards: bool,
    /// Persistent buffer ranges the packets read, written ahead of them:
    /// at the front of the submission when it holds no other bytes for
    /// them, otherwise in draw order before the run.
    pub writes: Vec<(wgpu::Buffer, u64, Vec<u8>)>,
}

/// Commands a pass issues into the frame's open encoder at commit.
/// Everything they use is made and written at prepare; the closure only
/// records, once on a scratch encoder under a validation scope and once
/// for real.
#[cfg(feature = "gpu")]
pub struct ExternalEncoded {
    /// Guest ranges recorded.
    pub ranges: usize,
    /// GPU draws the commands issue, including any intermediate draws.
    pub draws: usize,
    /// Render passes the commands begin (a submission's passes are bounded).
    pub passes: usize,
    pub commands: Box<dyn Fn(&mut wgpu::CommandEncoder) + Send>,
}

/// The id a vertex or index snapshot gets: unique in the process, so a host
/// copy kept by id never names two snapshots.
pub fn next_snapshot_id() -> u64 {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The viewport of a request in a `texels`-sized target: `[x, y, w, h]`.
/// `rect` is `(x, y, width, height)` in the guest pixels of a target whose
/// guest size is `surface_size`.
pub fn external_viewport(rect: (u32, u32, u32, u32), surface_size: (u32, u32), texels: (u32, u32)) -> [f32; 4] {
    let Some(scale) = crate::raster::RenderScale::between(surface_size, texels) else {
        return [0.0; 4];
    };
    let (x, y, w, h) = rect;
    scale.rect(x, y, w, h).map(|n| n as f32)
}

/// The largest render scale: 6x of 640x480 is 3840x2880.
pub const MAX_SCALE: u32 = 6;

/// The view's lighting (docs/graphics/e3-lighting-setting.md), chosen at
/// launch by `H2_LIGHTING` (`native_render::lighting`); no setting holds it. Nothing
/// of it reaches the guest: the reference draws retail whatever this is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lighting {
    /// The game's own: shadow maps at the guest's size, sampled as the
    /// NV2A does (2x2 percentage-closer filtering when the stage filters).
    #[default]
    Retail,
    /// E3: shadow maps at `shadow_scale` times their
    /// size in the view, compared with one tap.
    E3,
    /// The privately reconstructed BSP lightmap with its selected lamp on.
    RelightOn,
    /// The privately reconstructed BSP lightmap with its selected lamp off.
    RelightOff,
}

impl Lighting {
    pub fn is_relight(self) -> bool {
        matches!(self, Self::RelightOn | Self::RelightOff)
    }

    /// The two startup-only arms that need private BSP resources.
    pub fn relight_from_name(name: &str) -> Option<Self> {
        if name.trim().eq_ignore_ascii_case("relight-on") {
            Some(Self::RelightOn)
        } else if name.trim().eq_ignore_ascii_case("relight-off") {
            Some(Self::RelightOff)
        } else {
            None
        }
    }
}

/// A bound texture, as its guest header describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureInfo {
    /// Texture header address.
    pub header: u32,
    /// Palette header (P8 only, else 0).
    pub palette: u32,
    /// Format.
    pub format: D3DFormat,
    /// 2D, cube or volume.
    pub kind: TextureKind,
    /// Base width.
    pub width: u32,
    /// Base height.
    pub height: u32,
    /// Base depth.
    pub depth: u32,
    /// Mip levels.
    pub levels: u32,
}


/// One vertex stream, already read out of guest memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamBytes {
    /// Bytes, first vertex first.
    pub data: StreamData,
    /// Stride in bytes.
    pub stride: u32,
}

/// A stream's bytes: a copy made for this draw, or a shared snapshot from
/// the device's vertex cache. Both read as
/// `[u8]`, and compare by their bytes.
#[derive(Clone)]
pub enum StreamData {
    /// Copied for this draw (the buffer goes back to the device's pool).
    Owned(Vec<u8>),
    /// Guest bytes no store has touched since they were read: the same
    /// snapshot for every draw of the range until it is written.
    Cached(CachedBytes),
}

/// A vertex cache snapshot. `id` names these exact bytes: a refill of the
/// same guest range gets a new one, so a backend may keep a host copy (a
/// GPU buffer) per id and never check it again.
#[derive(Clone, Debug)]
pub struct CachedBytes {
    /// Unique per snapshot, never reused.
    pub id: u64,
    /// The bytes.
    pub bytes: std::sync::Arc<[u8]>,
}

impl CachedBytes {
    /// An immutable derived stream with its own GPU cache identity.
    pub fn derived(bytes: Vec<u8>) -> Self {
        Self { id: next_snapshot_id(), bytes: bytes.into() }
    }
}

impl StreamData {
    /// The cache snapshot's id, if this is one.
    pub fn cache_id(&self) -> Option<u64> {
        match self {
            Self::Owned(_) => None,
            Self::Cached(c) => Some(c.id),
        }
    }

    /// Heap capacity a pool would keep (`0` for a shared snapshot).
    pub fn capacity(&self) -> usize {
        match self {
            Self::Owned(v) => v.capacity(),
            Self::Cached(_) => 0,
        }
    }

    /// The copied buffer back, for reuse; `None` for a shared snapshot.
    pub fn into_owned_buffer(self) -> Option<Vec<u8>> {
        match self {
            Self::Owned(v) => Some(v),
            Self::Cached(_) => None,
        }
    }
}

impl Default for StreamData {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl std::ops::Deref for StreamData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(v) => v,
            Self::Cached(c) => &c.bytes,
        }
    }
}

impl AsRef<[u8]> for StreamData {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl From<Vec<u8>> for StreamData {
    fn from(v: Vec<u8>) -> Self {
        Self::Owned(v)
    }
}

impl PartialEq for StreamData {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for StreamData {}

impl PartialEq<Vec<u8>> for StreamData {
    fn eq(&self, other: &Vec<u8>) -> bool {
        **self == **other
    }
}

impl PartialEq<[u8]> for StreamData {
    fn eq(&self, other: &[u8]) -> bool {
        **self == *other
    }
}

impl std::fmt::Debug for StreamData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same output either way, so dumps of a draw do not depend on
        // whether its bytes came from the cache.
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// A draw's index list, rebased onto its vertex window: built for this
/// draw, or a shared snapshot from the device's index cache. Reads as
/// `[u32]` and compares by its values.
#[derive(Clone)]
pub enum IndexData {
    /// Built for this draw.
    Owned(Vec<u32>),
    /// A guest index list no store has touched since it was read.
    Cached(CachedIndices),
}

/// An index cache snapshot: the list's smallest and largest index and the
/// list rebased by the smallest. `id` names these exact values (as for
/// [`CachedBytes`]).
#[derive(Clone, Debug)]
pub struct CachedIndices {
    /// Unique per snapshot, never reused.
    pub id: u64,
    /// Smallest guest index.
    pub lo: u16,
    /// Largest guest index.
    pub hi: u16,
    /// Each guest index minus `lo`.
    pub rebased: std::sync::Arc<[u32]>,
}

impl CachedIndices {
    /// An immutable derived index list with its own GPU cache identity:
    /// `rebased` is each index minus `lo`, the largest index `hi`.
    pub fn derived(lo: u16, hi: u16, rebased: Vec<u32>) -> Self {
        Self { id: next_snapshot_id(), lo, hi, rebased: rebased.into() }
    }
}

impl IndexData {
    /// The cache snapshot's id, if this is one.
    pub fn cache_id(&self) -> Option<u64> {
        match self {
            Self::Owned(_) => None,
            Self::Cached(c) => Some(c.id),
        }
    }
}

impl std::ops::Deref for IndexData {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        match self {
            Self::Owned(v) => v,
            Self::Cached(c) => &c.rebased,
        }
    }
}

impl From<Vec<u32>> for IndexData {
    fn from(v: Vec<u32>) -> Self {
        Self::Owned(v)
    }
}

impl PartialEq for IndexData {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for IndexData {}

impl std::fmt::Debug for IndexData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}


/// A rectangle in target pixels, `[x, x + width) x [y, y + height)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PixelRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl PixelRect {
    /// Reproject a perspective draw's guest scissor around its viewport
    /// centre. A bound already clipped to the viewport remains there: its
    /// original projected edge may have been outside the retail frame.
    pub fn projected_x(self, viewport: (u32, u32, u32, u32), factor: f32) -> Self {
        if factor == 1.0 || self.width == 0 { return self; }
        let (left, _, width, _) = viewport;
        let right = left.saturating_add(width);
        let centre = f64::from(left) + f64::from(width) * 0.5;
        let edge = |x| centre + (f64::from(x) - centre) * f64::from(factor);
        let end = self.x.saturating_add(self.width);
        let x = if self.x <= left { left } else { (edge(self.x).floor() as u32).clamp(left, right) };
        let end = if end >= right { right } else { (edge(end).ceil() as u32).clamp(left, right) };
        Self { x, width: end.saturating_sub(x), ..self }
    }

    /// [`PixelRect::projected_x`] on the vertical axis, for a `factor` above
    /// 1 (the vertical lens grows by its inverse).
    pub fn projected_y(self, viewport: (u32, u32, u32, u32), factor: f32) -> Self {
        if factor == 1.0 || self.height == 0 { return self; }
        let (_, top, _, height) = viewport;
        let bottom = top.saturating_add(height);
        let centre = f64::from(top) + f64::from(height) * 0.5;
        let edge = |y| centre + (f64::from(y) - centre) / f64::from(factor);
        let end = self.y.saturating_add(self.height);
        let y = if self.y <= top { top } else { (edge(self.y).floor() as u32).clamp(top, bottom) };
        let end = if end >= bottom { bottom } else { (edge(end).ceil() as u32).clamp(top, bottom) };
        Self { y, height: end.saturating_sub(y), ..self }
    }

    /// Whether this is the whole `w x h` target.
    pub fn is_whole(&self, w: u32, h: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width == w && self.height == h
    }
}

/// Affine placement in the guest window, captured while a UI or projected
/// sprite producer owns the draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiFit {
    /// x' = x * sx + ox, y' = y * sy + oy, before the host raster shift.
    pub transform: [f32; 4],
    // A flare moves in its viewport. UI widgets carry a local scissor.
    fit_scissor: bool,
    /// Input register carrying a second copy of the guest window coordinates.
    pub screen_input: Option<u8>,
}

impl UiFit {
    pub const IDENTITY: Self = Self { transform: [1.0, 1.0, 0.0, 0.0], fit_scissor: true, screen_input: None };

    /// A fit from a placement in guest window units
    /// (`render_bridge::ui_layout`'s rule): x' = x*sx + ox, y' = y*sy + oy,
    /// scissors fitted with it.
    pub fn from_transform(transform: [f32; 4]) -> Self {
        Self { transform, fit_scissor: true, screen_input: None }
    }

    /// Fit guest pixels uniformly on the output, about a pivot in guest
    /// window coordinates. Each player's viewport has its own pivot.
    pub fn about(output: (u32, u32), pivot: [f32; 2]) -> Option<Self> {
        Self::from_pixel_ratio(canvas_fit::stretch_ratio(output)?.recip(), pivot)
    }

    /// Guest pixel width/height divided by displayed pixel width/height.
    /// For anamorphic Xbox artwork the numerator already includes its lens.
    pub fn from_pixel_ratio(ratio: f32, pivot: [f32; 2]) -> Option<Self> {
        canvas_fit::fit_about(ratio, pivot).map(Self::from_transform)
    }

    /// A CPU-projected sprite or query retains its player's scissor.
    pub fn projected_sprite(scale: f32, offset: f32) -> Self {
        Self { transform: [scale, 1.0, offset, 0.0], fit_scissor: false, screen_input: None }
    }

    /// Also fit this vertex register before the guest shader uses it for a lookup.
    pub fn with_screen_input(self, register: u8) -> Self {
        Self { screen_input: Some(register), ..self }
    }

    pub fn applies_to_frame_sample(self) -> bool {
        !self.fit_scissor
    }

    /// Fit a scissor into the box. An edge on the canvas boundary (0 or the
    /// 640x480 far side) of a shrunk axis stays on the display boundary, so a
    /// clip that is the whole canvas spans the whole display; interior edges
    /// are fitted.
    pub fn map_rect(&self, r: PixelRect) -> PixelRect {
        if !self.fit_scissor { return r; }
        let [sx, sy, ox, oy] = self.transform;
        let axis = |s: f32, o: f32, size: u32, start: u32, len: u32| {
            let end = start.saturating_add(len);
            let edge = |e: u32| {
                if s < 1.0 && (e == 0 || e == size) { e } else { (e as f32 * s + o).round() as u32 }
            };
            let lo = edge(start);
            (lo, edge(end).saturating_sub(lo))
        };
        let (x, width) = axis(sx, ox, 640, r.x, r.width);
        let (y, height) = axis(sy, oy, 480, r.y, r.height);
        PixelRect { x, y, width, height }
    }
}


#[cfg(feature = "gpu")]
/// A render-to-texture level an [`ExternalPass`] samples
/// (`ExternalTarget::sampled`).
#[derive(Clone, Debug)]
pub struct SampledLevel {
    pub view: wgpu::TextureView,
    /// Its size in this copy's texels.
    pub size: (u32, u32),
}


#[cfg(feature = "gpu")]
pub struct ExternalColor {
    pub guest_size: (u32, u32),
    pub attachment: wgpu::TextureView,
    pub sample: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
}


/// GPU handles pinned for a complete prepare/record transaction.
#[cfg(feature = "gpu")]
pub struct ExternalAttachments {
    pub targets: (crate::surface::SurfaceId, crate::surface::SurfaceId),
    pub uploads: crate::ring::SubmissionUploads,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub color: wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub depth: wgpu::TextureView,
    pub depth_format: wgpu::TextureFormat,
    pub size: (u32, u32),
    pub sampled: Vec<Option<SampledLevel>>,
    pub slots: Vec<Option<ExternalColor>>,
    pub stages: [Option<wgpu::TextureView>; 4],
    /// The active scaled policy requires just the display recording.
    pub single_display: bool,
    /// Lightmap arm permitted for this physical target. An isolated
    /// display copy or a private capture may select reconstructed pages.
    pub lighting: Lighting,
    pub copy: TargetCopy,
}



/// How many vertex constants a view's block holds: the per-view constants
/// the game uploads before a view's draws (docs/graphics/native-world.md,
/// "What a world pass reads").
pub const VIEW_CONSTANTS: usize = 18;

/// Where and how a stage runs, in guest terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExternalPassRequest {
    /// The viewport `(x, y, width, height)` in guest pixels of targets
    /// whose guest size is `surface_size`; a scaled copy scales it.
    pub rect: (u32, u32, u32, u32),
    pub surface_size: (u32, u32),
    pub z_scale: f32,
    pub display_projection_factor: f32,
}

impl ExternalPassRequest {
    pub fn projection_factor(&self, copy: TargetCopy) -> f32 {
        if copy == TargetCopy::Display { self.display_projection_factor } else { 1.0 }
    }

    pub fn view_constants(&self, copy: TargetCopy, mut c: [[f32; 4]; VIEW_CONSTANTS]) -> [[f32; 4]; VIEW_CONSTANTS] {
        crate::guest_program::widen_projection_rows(&mut c[..4], self.projection_factor(copy));
        c
    }
}
