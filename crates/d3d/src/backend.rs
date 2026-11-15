//! The host side of the device: where surfaces live and draws go.
//!
//! The device resolves every guest-visible thing (state arrays, resource
//! headers, vertex bytes, shader inputs) into a [`ResolvedDraw`] and hands it
//! to a [`Backend`]. [`GfxBackend`](crate::gfx_backend::GfxBackend) renders
//! through `gfx`; [`RecordingBackend`] records calls for CPU-only tests.

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::vertex::{PrimitiveType, VertexLayout};
use gfx::{Clear, D3DFormat};

use gfx::texture::TextureKind;

use crate::shader::ShaderInputs;
use crate::state::{DrawState, StageExtras, StateWords};

/// The render target a draw goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour format (as the guest header describes it).
    pub format: D3DFormat,
    /// Depth/stencil format, if a depth buffer is bound.
    pub depth_format: Option<D3DFormat>,
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
    pub data: Vec<u8>,
    /// Stride in bytes.
    pub stride: u32,
}

/// `D3DVIEWPORT8`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Near depth.
    pub min_z: f32,
    /// Far depth.
    pub max_z: f32,
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
    /// Whether this is the whole `w x h` target.
    pub fn is_whole(&self, w: u32, h: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width == w && self.height == h
    }
}

/// Colour channels a clear writes (`D3DCLEAR_TARGET_R/G/B/A`, Xbox bits
/// `0x10`/`0x20`/`0x40`/`0x80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChannelMask {
    /// Red (`0x10`).
    pub red: bool,
    /// Green (`0x20`).
    pub green: bool,
    /// Blue (`0x40`).
    pub blue: bool,
    /// Alpha (`0x80`).
    pub alpha: bool,
}

impl ChannelMask {
    /// From the `D3DCLEAR` flags.
    pub fn from_flags(flags: u32) -> Self {
        Self {
            red: flags & 0x10 != 0,
            green: flags & 0x20 != 0,
            blue: flags & 0x40 != 0,
            alpha: flags & 0x80 != 0,
        }
    }

    /// Any channel at all.
    pub fn any(&self) -> bool {
        self.red || self.green || self.blue || self.alpha
    }

    /// All four.
    pub fn all(&self) -> bool {
        self.red && self.green && self.blue && self.alpha
    }
}

/// A clear that is not one whole-attachment clear: some rectangles, or
/// only some colour channels. The values are the same as [`Clear`]'s.
#[derive(Clone, Debug, PartialEq)]
pub struct RectClear {
    /// Rectangles, already clipped to the viewport and the target, none
    /// empty.
    pub rects: Vec<PixelRect>,
    /// Colour and the channels to write (`None`: colour untouched).
    pub color: Option<([f32; 4], ChannelMask)>,
    /// Depth (0..1).
    pub depth: Option<f32>,
    /// Stencil.
    pub stencil: Option<u8>,
}

/// How a colour surface's bytes are laid out in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestLayoutKind {
    /// Morton order (a swizzled texture level).
    Swizzled,
    /// Rows `pitch` bytes apart.
    Linear {
        /// Row pitch in bytes.
        pitch: u32,
    },
}

/// A fully resolved draw: nothing in it refers to guest memory.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedDraw {
    /// Entry point that issued it (for events).
    pub entry: &'static str,
    /// Primitive type.
    pub primitive: PrimitiveType,
    /// Vertices in `streams`.
    pub vertex_count: u32,
    /// Vertex format.
    pub layout: VertexLayout,
    /// One per stream of `layout`.
    pub streams: Vec<StreamBytes>,
    /// Indexed draws: indices into `streams`, each below `vertex_count`, in
    /// primitive order. `None` draws the vertices in order.
    pub indices: Option<Vec<u32>>,
    /// Translated render and stage state.
    pub state: DrawState,
    /// The raw state words the translation came from: everything the guest
    /// set, including what `DrawState` does not model (combiner registers,
    /// deferred states, ...).
    pub raw: Box<StateWords>,
    /// Stage state outside `TextureStageState` (alpha kill, colour sign,
    /// bump environment, colour key).
    pub extras: [StageExtras; 4],
    /// The render target.
    pub target: TargetInfo,
    /// Bound textures (dimensions and format per stage).
    pub texture_info: [Option<TextureInfo>; 4],
    /// Shader inputs.
    pub shader: ShaderInputs,
    /// Current viewport.
    pub viewport: Viewport,
    /// Where the draw may write: the viewport intersected with the
    /// (inclusive) scissor rectangles and the target. On the Xbox,
    /// `SetViewport` resets the scissors to the viewport rectangle
    /// (`SetScissors(0, 0, 0)` at `0x3f82e8`), so pre-transformed geometry is
    /// clipped to it. `None`: the scissors keep nothing. With several
    /// inclusive scissor rectangles, or exclusive ones, this is the bounding
    /// box of the area and [`clip_rects`](Self::clip_rects) holds its pieces.
    pub clip: Option<PixelRect>,
    /// The writable area as disjoint rectangles inside `clip`, when it is
    /// not one rectangle: the union of several inclusive scissor
    /// rectangles, or what exclusive ones leave of the viewport. Empty when
    /// `clip` alone describes it. A backend draws once per piece (disjoint,
    /// so blending touches every pixel once).
    pub clip_rects: Vec<PixelRect>,
    /// NV2A register state when the draw was issued (viewport offset and
    /// scale, depth clip range, `SET_CONTROL0`; see [`crate::hw`]). The
    /// viewport transform of programmable and pass-through shaders lives
    /// here (and in constants 58/59), not in `viewport`.
    pub hw: crate::hw::HwState,
    /// Header addresses of the bound textures (identity for the backend's
    /// own bookkeeping; the resolved host textures are passed separately).
    pub textures: [Option<u32>; 4],
}

/// Predicts a backend's [`Backend::draw`] outcomes without running it, so
/// a backend that renders on another thread (`crate::threaded`) answers
/// each draw exactly when it is issued: skipped where the backend would
/// skip it. Made by [`Backend::judge`] before the backend moves.
pub trait DrawJudge: Send {
    /// Whether a texture uploaded from `texture` has a view the shaders
    /// sample.
    fn upload_view(&self, texture: &DecodedTexture) -> bool;
    /// The same for a texture over a colour surface of `format` (a
    /// snapshot, or the surface sampled in place).
    fn surface_view(&self, format: D3DFormat) -> bool;
    /// What [`Backend::draw`] answers for `draw`: `views[n]` is `None`
    /// with no texture on stage `n`, else whether it has a view.
    fn draw(&mut self, draw: &ResolvedDraw, views: [Option<bool>; 4]) -> DrawOutcome;
}

/// Result of [`Backend::draw`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawOutcome {
    /// Rendered.
    Drawn,
    /// Not rendered; the device records `Event::DrawSkipped` with the reason.
    Skipped(String),
    /// The backend would render this wrongly (state it cannot express):
    /// the device stops with `D3dError::Unimplemented`.
    Unsupported(String),
}

/// Host rendering for the device.
/// `Send` (with its handles) so a device can sit behind the runtime's
/// `Send + Sync` override handlers.
pub trait Backend: Send {
    /// Handle of a colour or depth/stencil surface.
    type Surface: Copy + Eq + std::fmt::Debug + Send;
    /// A host texture made from guest texture memory.
    type Texture: Send;

    /// Creates a surface (`depth` selects depth/stencil).
    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String>;
    /// Destroys a surface made by [`create_surface`](Self::create_surface).
    /// The handle must not be used again.
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String>;
    /// Uploads a decoded guest texture.
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String>;
    /// Whether BC formats pass through (decides the decode path).
    fn bc_supported(&self) -> bool;
    /// Clears the whole of the given attachments.
    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String>;
    /// Draws into `color` (+ `depth`).
    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String>;
    /// Shows `surface` (`Swap`). `flags` are the guest's `D3DSWAP_*` flags.
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String>;
    /// Presents with the current DAC ramp. Backends without a display may ignore it.
    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        _gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present(surface, flags)
    }
    /// Clears rectangles of the attachments, or only some colour channels.
    /// `Unsupported` stops the device (the default: no backend support).
    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        let _ = (color, depth);
        Ok(DrawOutcome::Unsupported(format!(
            "partial clear ({} rectangle(s), channels {:?}): the backend has no clear_rects",
            clear.rects.len(),
            clear.color.map(|c| c.1)
        )))
    }
    /// Reads a colour surface back as guest bytes: `format` texels of a
    /// `width x height` level laid out as `layout` (render-to-texture
    /// write-back). The default cannot.
    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let _ = (surface, width, height, layout);
        Err(format!(
            "the backend cannot read surfaces back ({format:?})"
        ))
    }
    /// Reads a depth/stencil surface back as guest bytes: `format` (`D16`
    /// or `D24S8`, as `z << 8 | stencil`) texels of a `width x height`
    /// image laid out as `layout`. For a render-to-texture depth level's
    /// write-back, and for textures that alias the depth buffer's memory.
    /// `Ok(None)`: the backend cannot (the default); a write-back then
    /// stops and an aliasing texture skips its draw.
    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        let _ = (surface, format, width, height, layout);
        Ok(None)
    }
    /// Copies the whole of colour surface `src` into `dst` (same format and
    /// size). `Ok(false)`: the backend cannot (the default).
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        let _ = (src, dst);
        Ok(false)
    }
    /// Fills a colour or depth/stencil surface from guest texels: `desc` is
    /// the level as the guest lays it out (format, size, and the pitch of a
    /// linear level), so a render-to-texture target starts with the level's
    /// current texels (`D16`/`D24S8` for a depth level).
    /// `Ok(false)`: the backend cannot, and the surface keeps whatever it
    /// was created with (the default).
    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let _ = (surface, desc, bytes);
        Ok(false)
    }
    /// A texture holding colour surface `surface` as it is now (a copy:
    /// later drawing into the surface does not reach it). For a texture
    /// header whose memory is a surface that has no guest copy, such as the
    /// back buffer read through a texture that aliases it. With `opaque`
    /// the copy's alpha is one (an `X8` texture over the surface).
    /// `Ok(None)`: the backend cannot (the default), and the draw is
    /// skipped.
    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        let _ = (surface, opaque);
        Ok(None)
    }
    /// `CLEAR_REPORT_VALUE` (zpass): the sample count restarts at zero.
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// `GET_REPORT`: a ticket for the samples counted since the last
    /// [`clear_pixel_count`](Self::clear_pixel_count), by every draw whose
    /// [`ResolvedDraw::hw`] had `zpass_count_enable` set (samples that
    /// passed the depth and stencil tests, as the NV2A counts them). It need
    /// not wait for the GPU; [`read_pixel_count`](Self::read_pixel_count)
    /// does. `Ok(None)`: the backend cannot count (the default), and the
    /// device stops.
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        Ok(None)
    }
    /// The count behind a ticket from
    /// [`report_pixel_count`](Self::report_pixel_count), waiting for the
    /// GPU if it has to. The ticket is spent.
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        Err(format!("the backend cannot count pixels (ticket {ticket})"))
    }
    /// The count behind a ticket if the GPU has already written it,
    /// without waiting; `Some` spends the ticket and is the value
    /// [`read_pixel_count`](Self::read_pixel_count) would give. `Ok(None)`:
    /// not back yet, or the backend cannot tell (the default); the ticket
    /// stays live.
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        let _ = ticket;
        Ok(None)
    }
    /// Forgets a ticket that will never be read.
    fn drop_pixel_count(&mut self, ticket: u64) {
        let _ = ticket;
    }
    /// A colour surface that is also a texture, for a render-to-texture
    /// level. The default is [`create_surface`](Self::create_surface).
    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        self.create_surface(format, width, height, false)
    }
    /// Whether [`surface_texture`](Self::surface_texture) works. A device
    /// on a backend that can sample its render-to-texture surfaces keeps
    /// their pixels on the host and writes them back to guest memory only
    /// when the guest is about to read them (`docs/d3d.md`).
    fn can_sample_surfaces(&self) -> bool {
        false
    }
    /// A texture that samples [`create_texture_target`](Self::create_texture_target)
    /// surface `surface` itself, not a copy: draws recorded later see what
    /// was drawn into it before them. Sampled exactly as a texture uploaded
    /// from the surface's read-back texels would be (formats whose texels
    /// read back unchanged only). `Ok(None)`: the backend cannot (the
    /// default).
    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let _ = surface;
        Ok(None)
    }
    /// Whether [`depth_level_texture`](Self::depth_level_texture) answers
    /// for `D24S8` depth surfaces. With it, lazy render-to-texture keeps
    /// depth levels on the host too (`docs/d3d.md`).
    fn can_sample_depth_surfaces(&self) -> bool {
        false
    }
    /// A texture with the texels a depth texture over depth/stencil
    /// surface `surface` decodes to once the surface is written back as
    /// `D24S8` ([`read_depth`](Self::read_depth)): its raw bits
    /// ([`gfx::convert::depth_channels`]), one 2D level of the surface's
    /// size, as of the work queued so far, without reading back.
    /// `Ok(None)`: the backend cannot (the default).
    fn depth_level_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let _ = surface;
        Ok(None)
    }
    /// A [`DrawJudge`] that answers this backend's draw outcomes on the
    /// device's side. `None` (the default): they cannot be predicted.
    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        None
    }
    /// Whether [`depth_alias_texture`](Self::depth_alias_texture) makes
    /// depth-alias textures (default false).
    fn defers_depth_aliases(&self) -> bool {
        false
    }
    /// A texture over the depth buffer's memory, made on the backend's side:
    /// the guest words [`read_depth`](Self::read_depth) gives for `depth`
    /// (`format`, `width x height`, rows `pitch` bytes) as the device's
    /// depth generation `generation` left them, from byte `offset` on,
    /// decoded as `desc` with `options` and uploaded. The bytes never reach
    /// guest memory, so a backend that renders on another thread can make
    /// it there without the guest waiting (`threaded`). `Ok(None)`: the
    /// device reads the depth back and decodes it itself (the default).
    #[allow(clippy::too_many_arguments)]
    fn depth_alias_texture(
        &mut self,
        depth: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        let _ = (depth, format, width, height, pitch, generation, offset, desc, options);
        Ok(None)
    }
}

/// What [`RecordingBackend`] saw.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    /// `create_surface`.
    Surface {
        /// Handle.
        id: u32,
        /// Format.
        format: D3DFormat,
        /// Size.
        size: (u32, u32),
        /// Depth surface?
        depth: bool,
    },
    /// `upload_texture`.
    Upload {
        /// Handle.
        id: u32,
        /// Size.
        size: (u32, u32),
    },
    /// `clear`.
    Clear {
        /// Target.
        color: u32,
        /// What.
        clear: Clear,
    },
    /// `draw`.
    Draw {
        /// Target.
        color: u32,
        /// The draw.
        draw: Box<ResolvedDraw>,
        /// Texture handles per stage.
        textures: [Option<u32>; 4],
    },
    /// `destroy_surface`.
    DestroySurface {
        /// Handle.
        id: u32,
    },
    /// `clear_rects`.
    ClearRects {
        /// Target.
        color: u32,
        /// What.
        clear: RectClear,
    },
    /// `read_surface`.
    ReadSurface {
        /// Surface.
        surface: u32,
        /// Bytes returned.
        len: usize,
    },
    /// `read_depth`.
    ReadDepth {
        /// Surface.
        surface: u32,
        /// Format asked for.
        format: D3DFormat,
        /// Bytes returned.
        len: usize,
    },
    /// `copy_surface`.
    CopySurface {
        /// Source.
        src: u32,
        /// Destination.
        dst: u32,
    },
    /// `surface_texture`.
    SurfaceTexture {
        /// Surface sampled.
        surface: u32,
        /// The texture handle returned.
        id: u32,
    },
    /// `depth_level_texture`.
    DepthLevelTexture {
        /// Depth surface converted.
        surface: u32,
        /// The texture handle returned.
        id: u32,
    },
    /// `snapshot_surface`.
    Snapshot {
        /// Surface copied.
        surface: u32,
        /// Alpha forced to one.
        opaque: bool,
        /// The texture handle returned.
        id: u32,
    },
    /// `write_surface`.
    WriteSurface {
        /// Surface.
        surface: u32,
        /// The bytes written.
        bytes: Vec<u8>,
    },
    /// `present`.
    Present {
        /// Surface shown.
        surface: u32,
        /// Flags.
        flags: u32,
    },
    /// `clear_pixel_count`.
    ClearPixelCount,
    /// `report_pixel_count`.
    ReportPixelCount {
        /// The ticket handed out.
        ticket: u64,
        /// The count it stands for.
        value: u64,
    },
    /// `read_pixel_count`.
    ReadPixelCount {
        /// The ticket.
        ticket: u64,
    },
}

/// Records every call; surfaces and textures are numbers.
#[derive(Default)]
pub struct RecordingBackend {
    /// Calls in order.
    pub calls: Vec<Recorded>,
    next: u32,
    /// The byte `read_surface` returns.
    pub readback_fill: u8,
    /// The DAC ramp passed at the most recent present.
    pub present_gamma: Option<Box<[u8; 768]>>,
    /// What `read_depth` returns (repeated or cut to the image's length);
    /// `None` returns `readback_fill` bytes.
    pub depth_readback: Option<Vec<u8>>,
    /// Samples counted since the last `clear_pixel_count`: each counted
    /// draw adds the area of its clip rectangle (every pixel it may touch
    /// passes; nothing is rasterised here).
    pub pixel_count: u64,
    /// Reports not yet read, by ticket.
    pub pixel_reports: std::collections::HashMap<u64, u64>,
    next_ticket: u64,
    /// `copy_surface` answers `Ok(false)` (a backend that cannot copy).
    pub no_copies: bool,
    /// Whether it claims to sample surfaces directly
    /// ([`Backend::can_sample_surfaces`]; default false).
    pub sample_surfaces: bool,
    /// Whether it answers [`Backend::depth_level_texture`] (lazy depth
    /// levels); off by default.
    pub sample_depth: bool,
}

impl RecordingBackend {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Surfaces created and not destroyed.
    pub fn live_surfaces(&self) -> usize {
        let created = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::Surface { .. }))
            .count();
        let destroyed = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::DestroySurface { .. }))
            .count();
        created - destroyed
    }

    /// Number of texture uploads so far.
    pub fn uploads(&self) -> usize {
        self.calls
            .iter()
            .filter(|c| matches!(c, Recorded::Upload { .. }))
            .count()
    }

    /// The recorded draws.
    pub fn draws(&self) -> Vec<&ResolvedDraw> {
        self.calls
            .iter()
            .filter_map(|c| {
                if let Recorded::Draw { draw, .. } = c {
                    Some(&**draw)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// [`RecordingBackend`] draws everything.
struct DrawsEverything;

impl DrawJudge for DrawsEverything {
    fn upload_view(&self, _: &DecodedTexture) -> bool {
        true
    }
    fn surface_view(&self, _: D3DFormat) -> bool {
        true
    }
    fn draw(&mut self, _: &ResolvedDraw, _: [Option<bool>; 4]) -> DrawOutcome {
        DrawOutcome::Drawn
    }
}

impl Backend for RecordingBackend {
    type Surface = u32;
    type Texture = u32;

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        Some(Box::new(DrawsEverything))
    }

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Surface {
            id: self.next,
            format,
            size: (width, height),
            depth,
        });
        Ok(self.next)
    }
    fn destroy_surface(&mut self, id: u32) -> Result<(), String> {
        let live = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::Surface { id: i, .. } if *i == id));
        let gone = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id: i } if *i == id));
        if !live || gone {
            return Err(format!("surface {id} is not live"));
        }
        self.calls.push(Recorded::DestroySurface { id });
        Ok(())
    }
    fn upload_texture(&mut self, t: &DecodedTexture) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Upload {
            id: self.next,
            size: (t.desc.width, t.desc.height),
        });
        Ok(self.next)
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, color: u32, _depth: Option<u32>, clear: Clear) -> Result<(), String> {
        self.calls.push(Recorded::Clear { color, clear });
        Ok(())
    }
    fn draw(
        &mut self,
        color: u32,
        _depth: Option<u32>,
        draw: &ResolvedDraw,
        textures: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        if draw.hw.zpass_count_enable
            && let Some(c) = draw.clip
        {
            // The writable area: the clip, or its pieces when it has them.
            let area = |r: &PixelRect| r.width as u64 * r.height as u64;
            self.pixel_count += if draw.clip_rects.is_empty() {
                area(&c)
            } else {
                draw.clip_rects.iter().map(area).sum()
            };
        }
        self.calls.push(Recorded::Draw {
            color,
            draw: Box::new(draw.clone()),
            textures: textures.map(|t| t.copied()),
        });
        Ok(DrawOutcome::Drawn)
    }
    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        self.calls.push(Recorded::Present { surface, flags });
        Ok(())
    }
    fn present_with_gamma(
        &mut self,
        surface: u32,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_gamma = gamma.map(|ramp| Box::new(*ramp));
        self.present(surface, flags)
    }
    fn clear_rects(
        &mut self,
        color: u32,
        _depth: Option<u32>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.calls.push(Recorded::ClearRects {
            color,
            clear: clear.clone(),
        });
        Ok(DrawOutcome::Drawn)
    }
    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        if self.no_copies {
            return Ok(false);
        }
        self.calls.push(Recorded::CopySurface { src, dst });
        Ok(true)
    }
    fn can_sample_surfaces(&self) -> bool {
        self.sample_surfaces
    }
    fn surface_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        if !self.sample_surfaces {
            return Ok(None);
        }
        self.next += 1;
        self.calls.push(Recorded::SurfaceTexture {
            surface,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn can_sample_depth_surfaces(&self) -> bool {
        self.sample_depth
    }
    fn depth_level_texture(&mut self, surface: u32) -> Result<Option<u32>, String> {
        if !self.sample_depth {
            return Ok(None);
        }
        self.next += 1;
        self.calls.push(Recorded::DepthLevelTexture {
            surface,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn snapshot_surface(&mut self, surface: u32, opaque: bool) -> Result<Option<u32>, String> {
        self.next += 1;
        self.calls.push(Recorded::Snapshot {
            surface,
            opaque,
            id: self.next,
        });
        Ok(Some(self.next))
    }
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.pixel_count = 0;
        self.calls.push(Recorded::ClearPixelCount);
        Ok(())
    }
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        self.pixel_reports.insert(ticket, self.pixel_count);
        self.calls.push(Recorded::ReportPixelCount {
            ticket,
            value: self.pixel_count,
        });
        Ok(Some(ticket))
    }
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.calls.push(Recorded::ReadPixelCount { ticket });
        self.pixel_reports
            .remove(&ticket)
            .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))
    }
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.pixel_reports.remove(&ticket);
    }
    /// Records the bytes.
    fn write_surface(
        &mut self,
        surface: u32,
        _desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.calls.push(Recorded::WriteSurface {
            surface,
            bytes: bytes.to_vec(),
        });
        Ok(true)
    }
    /// Returns `depth_readback` (cycled to the image's length) or
    /// `readback_fill` bytes.
    fn read_depth(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        let bpp = format
            .bytes_per_texel()
            .ok_or_else(|| format!("{format:?} has no texel size"))?;
        let len = match layout {
            GuestLayoutKind::Swizzled => (width * height * bpp) as usize,
            GuestLayoutKind::Linear { pitch } => (pitch * height) as usize,
        };
        self.calls.push(Recorded::ReadDepth {
            surface,
            format,
            len,
        });
        Ok(Some(match &self.depth_readback {
            Some(b) if !b.is_empty() => b.iter().copied().cycle().take(len).collect(),
            _ => vec![self.readback_fill; len],
        }))
    }
    /// Returns `readback_fill` repeated to the level's size.
    fn read_surface(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let bpp = format
            .bytes_per_texel()
            .ok_or_else(|| format!("{format:?} has no texel size"))?;
        let len = match layout {
            GuestLayoutKind::Swizzled => (width * height * bpp) as usize,
            GuestLayoutKind::Linear { pitch } => (pitch * height) as usize,
        };
        self.calls.push(Recorded::ReadSurface { surface, len });
        Ok(vec![self.readback_fill; len])
    }
}
