//! Clears, the draw clip rectangle, and render-to-texture targets.
//!
//! * **Clears.** `D3DDevice_Clear` intersects every rectangle with the
//!   viewport (Count = 0 clears the viewport, `0x3fa1a0..0x3fa247`) and
//!   honours the per-channel bits (`0x10..0x80`, which Halo's wrapper
//!   `0x14b60` builds from `EAX` bits 1..4). One whole-target, all-channel
//!   clear goes to [`Backend::clear`]; anything else to
//!   [`Backend::clear_rects`].
//! * **Clip.** Draws carry the area they may write
//!   ([`ResolvedDraw::clip`], [`ResolvedDraw::clip_rects`]): with no
//!   scissors the viewport (SetViewport resets the scissors, `0x3f82e8` ->
//!   `0x3f8760`); with scissors the target narrowed to the union of
//!   inclusive rectangles or to what exclusive ones leave, as disjoint
//!   pieces ([`scissor_region`]), with no viewport clip.
//! * **Frame buffer as a texture.** A texture whose `Data` is a frame
//!   buffer's ([`BACK_BUFFER_DATA`], [`DISPLAY_BUFFER_DATA`], ...) is
//!   sampled from a copy of that buffer ([`Backend::snapshot_surface`]):
//!   the image being drawn while it is the back buffer, the image last
//!   presented while it is the front. Draws share a copy until the device
//!   writes that buffer again, and copies refill a fixed pool of textures
//!   ([`Backend::snapshot_surface_into`], `frame_snaps.rs`). One over the depth buffer
//!   ([`DEPTH_BUFFER_DATA`]) reads the depth/stencil bits as the colour
//!   format it names: the depth buffer is read back as guest bytes
//!   ([`Backend::read_depth`]) once per change, and those bytes are decoded
//!   as the texture, exactly as if the guest memory held them. A backend
//!   that makes such textures itself ([`Backend::depth_alias_texture`]:
//!   `GfxBackend` decodes on the GPU, the render thread on its side) gets
//!   the job instead, with no read-back on the guest's side.
//! * **Render to texture.** A surface whose `Parent` is a texture (a level,
//!   as `0x14850` builds for its two 64x64 targets with the native
//!   `0x3fb7a0`) gets its own host surface: a colour surface, or a
//!   depth/stencil surface when the level is bound as `pNewZStencil`
//!   (`D16`/`D24S8`, swizzled or linear: shadow maps). Lazily (the default, on a
//!   backend that samples surfaces), draws that sample the texture read
//!   that surface in place ([`Backend::surface_texture`]), and its pixels
//!   reach guest memory only when the guest is about to read them: a wait
//!   or busy check on the resource (`BlockOnResource`, `0x3fb710`), a
//!   fence wait, a push-buffer callback, a sample that cannot read the
//!   surface in place, a destroy, `PersistDisplay` or teardown. Guest
//!   writes into a level held on the host are merged in when it is next
//!   used. Eagerly ([`Device::set_rtt_direct`] off), the write-back also
//!   runs when the target changes, at `Swap` and `KickOff`, and before
//!   every sample. A write-back reads the surface back in the level's
//!   format and layout, writes it into the level's guest memory and
//!   reports the write to the device's own tracker so cached uploads of
//!   that texture are refreshed.

use super::*;
use crate::backend::{ChannelMask, GuestLayoutKind, PixelRect, RectClear};

/// A texture level bound as a render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RttTarget<S> {
    /// Host surface drawn into.
    pub surface: S,
    /// The texture the level belongs to (`Parent`).
    pub parent: u32,
    /// Guest virtual address of the level's texels.
    pub data: u32,
    /// Header identity when the surface was made (`Data`, `Format`, `Size`).
    pub identity: [u32; 3],
    /// Level format.
    pub format: D3DFormat,
    /// Level size.
    pub width: u32,
    /// Level size.
    pub height: u32,
    /// Guest layout of the texels.
    pub layout: GuestLayoutKind,
    /// A depth/stencil level (bound as `pNewZStencil`), not a colour one.
    pub depth: bool,
    /// Drawn into since the last write-back.
    pub dirty: bool,
    /// Write-tracker generation when the host surface last matched guest
    /// memory (filled from it, or written back into it).
    pub generation: u64,
    /// Hash of those bytes (`UploadPolicy::Verify` only; 0 otherwise).
    pub hash: u64,
}

impl<S> RttTarget<S> {
    /// Bytes the level occupies in guest memory.
    pub fn len(&self) -> u32 {
        let bpp = self.format.bytes_per_texel().unwrap_or(4);
        match self.layout {
            GuestLayoutKind::Swizzled => self.width * self.height * bpp,
            GuestLayoutKind::Linear { pitch } => pitch * self.height,
        }
    }

    /// Never empty (levels are at least 1x1).
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// A depth alias's header and effective `COLORSIGN` mask. Different stages
/// can sample the same header with different channel signs in one draw.
pub(super) type DepthAliasKey = (u32, u32);

/// A texture header over the depth buffer, decoded from a readback.
pub(crate) struct DepthAlias<T> {
    /// [`Device::depth_generation`] of the bytes it was made from.
    generation: u64,
    /// The header's identity words then (`Data`, `Format`, `Size`).
    identity: [u32; 3],
    info: TextureInfo,
    /// The host texture.
    pub(crate) texture: T,
}

/// A colour surface whose memory is the device's depth buffer: not a
/// texture level, its `Data` the depth buffer's. Halo's rasterizer target 3
/// (`0x14bc0` -> `0x14c21`) is one: `0x14600` builds its header at
/// `0x509370` as a copy of the back buffer's with the depth buffer's `Data`,
/// and target 3 as a texture (`0x14f60`, table `0x15114` -> `0x15020`) is `0x509368`, the
/// `LIN_A8R8G8B8` texture over the same memory. The level loading screen
/// copies its image into target 3 and box-filters it back from `0x509368`
/// every frame (`0x29370`); on the console the filter reads the colour just
/// written, not depth bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DepthColor {
    /// The surface header (its host surface is in `other_surfaces`).
    pub(crate) header: u32,
    /// Its format and size.
    format: D3DFormat,
    width: u32,
    height: u32,
}

/// A frame-buffer copy's size and format: the pool of copies is kept per
/// key (`frame_snaps.rs`).
pub(super) type CopyKey = (u32, u32, D3DFormat);

/// What a bound texture header is, as far as the frame buffers go.
pub(super) enum FrameTexture<T> {
    /// Ordinary guest texture memory.
    No,
    /// A frame buffer, copied for this draw alone (while post-process
    /// draws are held, each packet keeps its own copy).
    Snapshot(TextureInfo, T),
    /// A frame buffer, through the pool slot `Device::frame_snaps` holds
    /// under this id, shared with every draw that samples the surface
    /// before it is written again.
    Shared(TextureInfo, u64),
    /// The depth buffer's bits as this texture's format: the host texture
    /// is `Device::depth_alias[key]`.
    DepthAlias(TextureInfo, DepthAliasKey),
    /// The draw cannot sample it: skip with this reason.
    Skip(String),
}

/// Left, top, right, bottom (right and bottom exclusive).
type Edges = (i64, i64, i64, i64);

/// A draw's clip: the bounding rectangle and, when it is not one
/// rectangle, the pieces.
type ClipArea = (PixelRect, Vec<PixelRect>);

/// A level's current guest bytes and, per byte, whether a guest store
/// owns it over the host surface.
type GuestStores = (Vec<u8>, Vec<bool>);

/// The pieces of `base` (left, top, right, bottom; right and bottom
/// exclusive) that scissor rectangles `rects` keep: inside any of them, or,
/// with `exclusive`, outside all of them. The pieces are disjoint and
/// cover the kept area exactly: each band between consecutive edges is cut
/// into maximal runs, and runs that continue a piece of the band above
/// extend it. No rectangles keeps `base` whole.
pub fn scissor_region(base: Edges, rects: &[Rect], exclusive: bool) -> Vec<PixelRect> {
    let (bx1, by1, bx2, by2) = base;
    if bx1 >= bx2 || by1 >= by2 {
        return Vec::new();
    }
    if rects.is_empty() {
        return vec![PixelRect {
            x: bx1 as u32,
            y: by1 as u32,
            width: (bx2 - bx1) as u32,
            height: (by2 - by1) as u32,
        }];
    }
    let rects: Vec<Edges> = rects
        .iter()
        .map(|r| {
            (
                (r.x1 as i64).clamp(bx1, bx2),
                (r.y1 as i64).clamp(by1, by2),
                (r.x2 as i64).clamp(bx1, bx2),
                (r.y2 as i64).clamp(by1, by2),
            )
        })
        .filter(|r| r.0 < r.2 && r.1 < r.3)
        .collect();
    let edges = |a: i64, b: i64, f: &dyn Fn(&Edges) -> [i64; 2]| {
        let mut e = vec![a, b];
        e.extend(rects.iter().flat_map(f));
        e.sort_unstable();
        e.dedup();
        e
    };
    let xs = edges(bx1, bx2, &|r| [r.0, r.2]);
    let ys = edges(by1, by2, &|r| [r.1, r.3]);
    let mut out: Vec<PixelRect> = Vec::new();
    // Pieces open at the band above: (x1, x2) -> index in `out`.
    let mut open: Vec<((i64, i64), usize)> = Vec::new();
    for band in ys.windows(2) {
        let (y1, y2) = (band[0], band[1]);
        let mut runs: Vec<(i64, i64)> = Vec::new();
        for col in xs.windows(2) {
            let (x1, x2) = (col[0], col[1]);
            let inside = rects
                .iter()
                .any(|r| r.0 <= x1 && x2 <= r.2 && r.1 <= y1 && y2 <= r.3);
            if inside != exclusive {
                match runs.last_mut() {
                    Some(last) if last.1 == x1 => last.1 = x2,
                    _ => runs.push((x1, x2)),
                }
            }
        }
        let mut next = Vec::with_capacity(runs.len());
        for run in runs {
            if let Some(&(_, i)) = open.iter().find(|(r, _)| *r == run) {
                out[i].height = (y2 - out[i].y as i64) as u32;
                next.push((run, i));
            } else {
                out.push(PixelRect {
                    x: run.0 as u32,
                    y: y1 as u32,
                    width: (run.1 - run.0) as u32,
                    height: (y2 - y1) as u32,
                });
                next.push((run, out.len() - 1));
            }
        }
        open = next;
    }
    out
}

impl<B: Backend> Device<B> {
    /// Colour surface `header` (`h`, `desc`; not a texture level) got its
    /// host surface: notes whether its memory is the depth buffer's
    /// ([`DepthColor`]).
    pub(super) fn note_depth_color(
        &mut self,
        header: u32,
        h: &SurfaceHeader,
        desc: &TextureDesc,
        depth: bool,
    ) {
        let map = self.cfg.address_map;
        let over_depth = !depth
            && self.depth.is_some()
            && map.to_virtual(h.pixels.resource.data) == map.to_virtual(DEPTH_BUFFER_DATA);
        if over_depth {
            if self.depth_color.is_some_and(|c| c.header != header) {
                self.depth_color_newer = false;
            }
            self.depth_color = Some(DepthColor {
                header,
                format: desc.format,
                width: desc.width,
                height: desc.height,
            });
        } else if self.depth_color.is_some_and(|c| c.header == header) {
            self.depth_color = None;
            self.depth_color_newer = false;
        }
    }

    /// Whether texture `header` reads one of the device's own frame
    /// buffers (a frame buffer's `Data`, or [`DEPTH_BUFFER_DATA`]),
    /// and if so how a draw samples it. The frame buffer is found by its
    /// own `Data`, not by which header names it now: over the back buffer
    /// the draw reads the image being drawn, over the display surface the
    /// image last presented. A stage the draw does not read (`sampled`
    /// false) is not copied; the caller leaves it unbound.
    pub(super) fn frame_texture(
        &mut self,
        mem: &dyn GuestMemory,
        header: u32,
        sampled: bool,
        color_sign: u32,
    ) -> Result<FrameTexture<B::Texture>, D3dError> {
        let pc = PixelContainer::read(mem, header)?;
        if pc.resource.kind() != ResourceType::Texture {
            return Ok(FrameTexture::No);
        }
        let map = self.cfg.address_map;
        let data = map.to_virtual(pc.resource.data);
        let within = |base: u32| data.wrapping_sub(map.to_virtual(base)) < FRAME_DATA_SPAN;
        let Some(sw) = self.swap.as_ref() else {
            return Ok(FrameTexture::No);
        };
        if self.depth.is_some() && within(DEPTH_BUFFER_DATA) {
            let offset = data.wrapping_sub(map.to_virtual(DEPTH_BUFFER_DATA));
            return self.depth_texture(header, &pc, offset, sampled, color_sign);
        }
        let Some(&(base, buffer)) = sw.buffers.iter().find(|b| within(b.0)) else {
            return Ok(FrameTexture::No);
        };
        // The front after a COPY swap that nothing has drawn over since is
        // still in back0 (`Device::materialize_display_copy`).
        let front = buffer == sw.front;
        let (source, which) = if buffer == sw.back[0] {
            (buffer, "the back buffer")
        } else if front && sw.copy_pending {
            (sw.back[0], "the display surface")
        } else if front {
            (buffer, "the display surface")
        } else {
            (buffer, "the second back buffer")
        };
        if front && self.display.shown_lost {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} reads {which}, which was drawn over: the backend cannot copy surfaces to keep it"
            )));
        }
        let (bw, bh) = sw.size;
        let desc = match crate::texdesc::from_header(pc.format, pc.size) {
            Ok(d) => d,
            Err(e) => {
                return Ok(FrameTexture::Skip(format!(
                    "texture {header:#010x} over {which}: {e}"
                )));
            }
        };
        let exact = data == map.to_virtual(base);
        let format_ok = matches!(desc.format, D3DFormat::LinA8R8G8B8 | D3DFormat::LinX8R8G8B8)
            && sw.format.bytes_per_texel() == Some(4);
        if !exact
            || !format_ok
            || desc.kind != gfx::texture::TextureKind::D2
            || (desc.width, desc.height) != (bw, bh)
        {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over {which} ({:?} {}x{} at +{:#x}) is not the whole {bw}x{bh} {:?} image as a linear 32-bit texture",
                desc.format,
                desc.width,
                desc.height,
                data.wrapping_sub(map.to_virtual(base)),
                sw.format
            )));
        }
        if !sampled {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over {which} on a stage the draw does not read"
            )));
        }
        // X8: the texel's alpha reads as one, whatever the buffer holds.
        let opaque = desc.format == D3DFormat::LinX8R8G8B8;
        let previous = source != sw.back[0];
        let info = TextureInfo {
            header,
            palette: 0,
            format: desc.format,
            kind: desc.kind,
            width: desc.width,
            height: desc.height,
            depth: desc.depth,
            levels: desc.levels,
        };
        let key = (bw, bh, sw.format);
        Ok(self.frame_copy(source, key, opaque, info, previous)?.unwrap_or_else(|| {
            FrameTexture::Skip(format!(
                "texture {header:#010x} reads {which} and the backend cannot copy surfaces into textures"
            ))
        }))
    }

    /// A draw samples colour surface `source` (size and format `key`) as
    /// `info`: a copy of it as it is now, alpha one with `opaque`.
    /// The copy is shared with later draws until the device writes
    /// `source` again (`frame_snaps.rs`), so N sampling draws with no
    /// write in between make one copy. A fresh copy goes into the pool:
    /// a GPU copy into a slot's texture ([`Backend::snapshot_surface_into`])
    /// whenever a slot of `key` can be refilled, a new texture
    /// ([`Backend::snapshot_surface`]) only while the pool for `key` is
    /// filling, so the textures held stay at the pool's size however many
    /// draws sample the frame. While post-process draws are held (swallowing) the writes
    /// they will make are not issued yet, so each held draw gets a copy of
    /// its own, for the post call. `previous`: the copy is of the image
    /// last presented (`Stats::previous_frame_snapshots`). `None`: the
    /// backend cannot copy surfaces.
    fn frame_copy(
        &mut self,
        source: B::Surface,
        key: CopyKey,
        opaque: bool,
        info: TextureInfo,
        previous: bool,
    ) -> Result<Option<FrameTexture<B::Texture>>, D3dError> {
        let shared = self.swallowing.is_none();
        if shared && let Some(id) = self.frame_snaps.find(source, opaque) {
            self.stats.frame_snapshot_reuses += 1;
            return Ok(Some(FrameTexture::Shared(info, id)));
        }
        let refill = if shared { self.frame_snaps.slot_for(key) } else { None };
        let slot = match refill {
            Some(id) => self.frame_snaps.texture_mut(id).map(|t| (id, t)),
            None => None,
        };
        let refilled = match slot {
            Some((id, t)) => self
                .backend
                .snapshot_surface_into(source, opaque, t)
                .map_err(D3dError::Backend)?
                .then_some(id),
            None => None,
        };
        let id = match refilled {
            Some(id) => {
                self.frame_snaps.filled(id, source, opaque);
                id
            }
            None => {
                let Some(t) = self
                    .backend
                    .snapshot_surface(source, opaque)
                    .map_err(D3dError::Backend)?
                else {
                    return Ok(None);
                };
                self.stats.frame_copy_textures += 1;
                if !shared {
                    self.count_frame_copy(previous);
                    return Ok(Some(FrameTexture::Snapshot(info, t)));
                }
                match refill {
                    // The backend could not refill the slot: the new copy
                    // takes its place.
                    Some(id) => {
                        self.frame_snaps.replace(id, source, opaque, t);
                        id
                    }
                    None => self.frame_snaps.insert(key, source, opaque, t),
                }
            }
        };
        self.count_frame_copy(previous);
        Ok(Some(FrameTexture::Shared(info, id)))
    }

    fn count_frame_copy(&mut self, previous: bool) {
        self.stats.frame_snapshots += 1;
        if previous {
            self.stats.previous_frame_snapshots += 1;
        }
    }

    /// Texture `header` (`pc`) reads the depth buffer's memory from byte
    /// `offset`: its texels are the depth/stencil words as the guest would
    /// see them in memory (`D24S8`: `z << 8 | stencil` per pixel, rows of
    /// `width * 4` bytes), decoded as the texture's own format. Halo's
    /// `0x509368` (`LIN_A8R8G8B8`) therefore samples the depth's top byte as
    /// alpha, its middle and low bytes as red and green and the stencil as
    /// blue, and `0x50936C` (`LIN_A8B8G8R8`) the same bytes with red and
    /// blue swapped. The depth buffer is read back once per change
    /// (`depth_generation`), and each header/sign mask decoded once per
    /// readback. Colour channels are biased as for ordinary signed textures;
    /// depth-format aliases keep raw words for shadow comparison.
    /// The draw sees the buffer as it was before the draw, as for the back
    /// buffer.
    fn depth_texture(
        &mut self,
        header: u32,
        pc: &PixelContainer,
        offset: u32,
        sampled: bool,
        color_sign: u32,
    ) -> Result<FrameTexture<B::Texture>, D3dError> {
        if !sampled {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over the depth buffer on a stage the draw does not read"
            )));
        }
        let skip = |why: String| {
            Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over the depth buffer: {why}"
            )))
        };
        let desc = match crate::texdesc::from_header(pc.format, pc.size) {
            Ok(d) => d,
            Err(e) => return skip(e.to_string()),
        };
        let total = match crate::texdesc::total_size(&desc) {
            Ok(l) => l as u64,
            Err(e) => return skip(e.to_string()),
        };
        let (Some(depth), Some(p)) = (self.depth, self.params) else {
            return Ok(FrameTexture::No);
        };
        if self.depth_color_newer
            && let Some(c) = self.depth_color
            && let Some(&(_, surface)) = self.other_surfaces.get(&c.header)
        {
            return self.depth_color_texture(header, pc, offset, color_sign, c, surface);
        }
        let dfmt = D3DFormat::from_raw(p.auto_depth_stencil_format)
            .map(linear_twin)
            .unwrap_or(D3DFormat::LinD24S8);
        let bpp = dfmt.bytes_per_texel().unwrap_or(4);
        let pitch = p.width * bpp;
        let len = pitch as u64 * p.height as u64;
        if offset as u64 + total > len {
            return skip(format!(
                "{:?} {}x{} at +{offset:#x} ({total} bytes) runs past the {len}-byte {dfmt:?} buffer",
                desc.format, desc.width, desc.height
            ));
        }
        let info = TextureInfo {
            header,
            palette: 0,
            format: desc.format,
            kind: desc.kind,
            width: desc.width,
            height: desc.height,
            depth: desc.depth,
            levels: desc.levels,
        };
        let generation = self.depth_generation;
        let identity = pc.identity();
        let key = (header, texture_color_sign(desc.format, color_sign));
        if let Some(a) = self.depth_alias.get(&key)
            && a.generation == generation
            && a.identity == identity
        {
            return Ok(FrameTexture::DepthAlias(a.info, key));
        }
        let options = DecodeOptions {
            bc_supported: self.backend.bc_supported(),
            ..DecodeOptions::default()
        }
        .with_color_sign(key.1);
        // A backend that makes the texture on its side (a render thread)
        // gets the whole job; whether the bytes decode depends only on the
        // description, so that is checked here once per description, on
        // zeros, and the draw skips exactly as below.
        if self.backend.defers_depth_aliases() {
            if let Err(e) = self.check_alias_decode(&desc, key.1, options, total as usize) {
                return skip(e);
            }
            if self.depth_read_generation != Some(generation) {
                self.depth_read_generation = Some(generation);
                self.stats.depth_reads += 1;
            }
            let texture = self
                .backend
                .depth_alias_texture(depth, dfmt, p.width, p.height, pitch, generation, offset as usize, &desc, options)
                .map_err(D3dError::Backend)?
                .ok_or_else(|| D3dError::Backend("depth_alias_texture: deferred backend answered None".into()))?;
            self.stats.depth_alias_uploads += 1;
            self.depth_alias.insert(key, DepthAlias { generation, identity, info, texture });
            return Ok(FrameTexture::DepthAlias(info, key));
        }
        if self
            .depth_copy
            .as_ref()
            .is_none_or(|(g, _)| *g != generation)
        {
            let layout = GuestLayoutKind::Linear { pitch };
            let Some(bytes) = self
                .backend
                .read_depth(depth, dfmt, p.width, p.height, layout)
                .map_err(D3dError::Backend)?
            else {
                return skip("the backend cannot read depth back".into());
            };
            if (bytes.len() as u64) < len {
                return Err(D3dError::Backend(format!(
                    "read_depth returned {} bytes for a {len}-byte depth buffer",
                    bytes.len()
                )));
            }
            self.stats.depth_reads += 1;
            self.depth_copy = Some((generation, bytes));
        }
        let bytes = &self.depth_copy.as_ref().expect("read above").1;
        let decoded = match decode_texture(&desc, &bytes[offset as usize..], None, options) {
            Ok(d) => d,
            Err(e) => return skip(e.to_string()),
        };
        let texture = self
            .backend
            .upload_texture(&decoded)
            .map_err(D3dError::Backend)?;
        self.stats.depth_alias_uploads += 1;
        self.depth_alias.insert(
            key,
            DepthAlias {
                generation,
                identity,
                info,
                texture,
            },
        );
        Ok(FrameTexture::DepthAlias(info, key))
    }

    fn check_alias_decode(&mut self, desc: &gfx::TextureDesc, sign: u32, options: DecodeOptions, len: usize) -> Result<(), String> {
        if let Some((_, _, verdict)) = self.depth_alias_decodes.iter().find(|(d, s, _)| d == desc && *s == sign) {
            return verdict.clone();
        }
        let verdict = decode_texture(desc, &vec![0; len], None, options).map(|_| ()).map_err(|e| e.to_string());
        self.depth_alias_decodes.push((desc.clone(), sign, verdict.clone()));
        verdict
    }

    /// Texture `header` (`pc`) over the depth buffer's memory from byte
    /// `offset`, when a colour surface over that memory ([`DepthColor`])
    /// was drawn into after the depth buffer: the memory holds the colour
    /// surface's pixels, so the texture reads them. The whole surface as a
    /// linear 32-bit texture of its size with no `COLORSIGN` (Halo's
    /// `0x509368` over target 3) is a copy of the surface
    /// ([`Backend::snapshot_surface`]); anything else decodes the surface's
    /// bytes read back as guest memory would hold them, once per change.
    fn depth_color_texture(
        &mut self,
        header: u32,
        pc: &PixelContainer,
        offset: u32,
        color_sign: u32,
        c: DepthColor,
        surface: B::Surface,
    ) -> Result<FrameTexture<B::Texture>, D3dError> {
        let skip = |why: String| {
            Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over the colour surface {:#010x} in the depth buffer's memory: {why}",
                c.header
            )))
        };
        let desc = match crate::texdesc::from_header(pc.format, pc.size) {
            Ok(d) => d,
            Err(e) => return skip(e.to_string()),
        };
        let info = TextureInfo {
            header,
            palette: 0,
            format: desc.format,
            kind: desc.kind,
            width: desc.width,
            height: desc.height,
            depth: desc.depth,
            levels: desc.levels,
        };
        let sign = texture_color_sign(desc.format, color_sign);
        let word = |f: D3DFormat| matches!(f, D3DFormat::LinA8R8G8B8 | D3DFormat::LinX8R8G8B8);
        if offset == 0
            && sign == 0
            && word(desc.format)
            && word(c.format)
            && desc.kind == gfx::texture::TextureKind::D2
            && desc.levels == 1
            && (desc.width, desc.height) == (c.width, c.height)
            && desc.pitch.max(desc.width * 4) == c.width * 4
        {
            let opaque = desc.format == D3DFormat::LinX8R8G8B8;
            let key = (c.width, c.height, c.format);
            return match self.frame_copy(surface, key, opaque, info, false)? {
                Some(t) => Ok(t),
                None => skip("the backend cannot copy surfaces into textures".into()),
            };
        }
        let generation = self.depth_generation;
        let identity = pc.identity();
        let key = (header, sign);
        if let Some(a) = self.depth_alias.get(&key)
            && a.generation == generation
            && a.identity == identity
        {
            return Ok(FrameTexture::DepthAlias(a.info, key));
        }
        let Some(bpp) = c.format.bytes_per_texel() else {
            return skip(format!("{:?} has no texel size", c.format));
        };
        let pitch = c.width * bpp;
        let len = pitch as u64 * c.height as u64;
        let total = match crate::texdesc::total_size(&desc) {
            Ok(l) => l as u64,
            Err(e) => return skip(e.to_string()),
        };
        if offset as u64 + total > len {
            return skip(format!(
                "{:?} {}x{} at +{offset:#x} ({total} bytes) runs past the {len}-byte {:?} surface",
                desc.format, desc.width, desc.height, c.format
            ));
        }
        let options = DecodeOptions {
            bc_supported: self.backend.bc_supported(),
            ..DecodeOptions::default()
        }
        .with_color_sign(sign);
        let texture = if self.backend.handles_surface_aliases() {
            if let Err(e) = self.check_alias_decode(&desc, sign, options, total as usize) {
                return skip(e);
            }
            self.backend.depth_alias_texture(surface, c.format, c.width, c.height, pitch, generation,
                offset as usize, &desc, options).map_err(D3dError::Backend)?
                .ok_or_else(|| D3dError::Backend("missing colour alias texture".into()))?
        } else {
            let bytes = self
                .backend
                .read_surface(
                    surface,
                    c.format,
                    c.width,
                    c.height,
                    GuestLayoutKind::Linear { pitch },
                )
                .map_err(D3dError::Backend)?;
            if (bytes.len() as u64) < len {
                return Err(D3dError::Backend(format!(
                    "read_surface returned {} bytes for a {len}-byte surface",
                    bytes.len()
                )));
            }
            let decoded = match decode_texture(&desc, &bytes[offset as usize..], None, options) {
                Ok(d) => d,
                Err(e) => return skip(e.to_string()),
            };
            self
                .backend
                .upload_texture(&decoded)
                .map_err(D3dError::Backend)?
        };
        self.stats.depth_alias_uploads += 1;
        self.depth_alias.insert(
            key,
            DepthAlias {
                generation,
                identity,
                info,
                texture,
            },
        );
        Ok(FrameTexture::DepthAlias(info, key))
    }

    /// The host surface for a texture-level render target (made on first
    /// use; remade if the header's identity changed).
    pub(super) fn rtt_surface(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        header: u32,
        h: &SurfaceHeader,
        depth: bool,
    ) -> Result<B::Surface, D3dError> {
        let id = h.pixels.identity();
        if let Some(t) = self.rtt.get(&header).copied()
            && t.identity == id
        {
            if t.depth != depth {
                return Err(D3dError::unimplemented(
                    entry,
                    format!(
                        "level {header:#010x} of texture {:#010x} is bound as {} after being a {} target",
                        h.parent,
                        if depth { "depth/stencil" } else { "colour" },
                        if t.depth { "depth/stencil" } else { "colour" },
                    ),
                ));
            }
            self.write_back_overlapping(mem, header)?;
            self.refresh_rtt(mem, header)?;
            return Ok(t.surface);
        }
        let desc = crate::texdesc::from_header(h.pixels.format, h.pixels.size)
            .map_err(|e| D3dError::unimplemented(entry, format!("surface header: {e}")))?;
        if depth != desc.format.is_depth() {
            return Err(D3dError::invalid(
                entry,
                format!(
                    "level {header:#010x} of texture {:#010x} has format {:?} but is bound as a {} target",
                    h.parent,
                    desc.format,
                    if depth { "depth/stencil" } else { "colour" }
                ),
            ));
        }
        if depth
            && !matches!(
                desc.format,
                D3DFormat::D24S8 | D3DFormat::LinD24S8 | D3DFormat::D16 | D3DFormat::LinD16
            )
        {
            // Float depth stores a non-linear value the host depth surface
            // does not hold; its guest bits cannot be filled or written back.
            return Err(D3dError::unimplemented(
                entry,
                format!(
                    "depth/stencil level {header:#010x} of texture {:#010x} has float depth format {:?}",
                    h.parent, desc.format
                ),
            ));
        }
        let layout = match desc.format.layout() {
            gfx::format::Layout::Swizzled => GuestLayoutKind::Swizzled,
            gfx::format::Layout::Linear => GuestLayoutKind::Linear {
                pitch: desc
                    .pitch
                    .max(desc.width * desc.format.bytes_per_texel().unwrap_or(4)),
            },
            gfx::format::Layout::Compressed => {
                return Err(D3dError::unimplemented(
                    entry,
                    format!(
                        "render target {header:#010x} has compressed format {:?}",
                        desc.format
                    ),
                ));
            }
        };
        // The header was rewritten: its old surface goes. Eagerly, content
        // not yet written back cannot exist here through D3D (the
        // write-back runs when the target changes), so it is an error;
        // lazily, the old level's pixels go to its memory first.
        if let Some(old) = self.rtt.get(&header).copied() {
            if old.dirty {
                if !self.rtt_lazy() {
                    return Err(D3dError::invalid(
                        entry,
                        format!(
                            "render target {header:#010x} was rewritten while it held unwritten pixels"
                        ),
                    ));
                }
                self.write_back(mem, header, WritebackCause::Rewritten)?;
            }
            self.rtt.remove(&header);
            self.rtt_epoch += 1;
            self.mirror_stale.remove(&header);
            self.rtt_synced.remove(&header);
            self.rtt_stores.remove(&header);
            self.depth_level_tex.remove(&header);
            self.drop_surface(old.surface)?;
        }
        let role = self.role_of(header, h.parent);
        let surface = self.make_target(role, desc.format, desc.width, desc.height, depth, !depth)?;
        let target = RttTarget {
            surface,
            parent: h.parent,
            data: self.cfg.address_map.to_virtual(h.pixels.resource.data),
            identity: id,
            format: desc.format,
            width: desc.width,
            height: desc.height,
            layout,
            depth,
            dirty: false,
            generation: 0,
            hash: 0,
        };
        self.tracker.watch(target.data, target.len());
        self.rtt.insert(header, target);
        self.rtt_epoch += 1;
        self.write_back_overlapping(mem, header)?;
        // The level's current texels, so draws that do not cover it all
        // leave the rest as the guest had it.
        self.fill_level(mem, header)?;
        Ok(surface)
    }

    /// Whether render-to-texture pixels stay on the host until the guest
    /// is about to read them, and draws sample the surfaces directly: on
    /// ([`Device::set_rtt_direct`]) and the backend can
    /// ([`Backend::can_sample_surfaces`]).
    pub(super) fn rtt_lazy(&self) -> bool {
        self.rtt_direct && self.backend.can_sample_surfaces()
    }

    /// Whether render-to-texture levels are sampled in place ([`Self::rtt_lazy`]):
    /// what an external pass sampling one ([`crate::backend::ExternalPass::sampled`])
    /// needs to be given it.
    pub fn samples_levels_in_place(&self) -> bool {
        self.rtt_lazy()
    }

    /// Lazy render-to-texture (the default): draws sample a level's host
    /// surface in place and its pixels reach guest memory only when the
    /// guest is about to read them (`docs/graphics/d3d.md`). Off: every target
    /// change, `Swap`, `KickOff` and sample writes back, as before. Lazy
    /// needs a backend that samples surfaces
    /// ([`Backend::can_sample_surfaces`]); without one it stays off.
    pub fn set_rtt_direct(&mut self, on: bool) {
        self.rtt_direct = on;
    }

    /// Lazy depth levels (on by default): with lazy render-to-texture and a backend that samples depth
    /// surfaces ([`Backend::can_sample_depth_surfaces`]), a depth level is
    /// written back only where a colour level is, not at every binding
    /// change, and draws sample it through [`Backend::depth_level_texture`].
    pub fn set_lazy_depth(&mut self, on: bool) {
        self.lazy_depth = on;
    }

    /// Whether a lazy level left as a target is read from guest memory
    /// every time, not only when its pages were written since its match
    /// was made (the default). Tests compare the two.
    pub fn set_leave_read(&mut self, on: bool) {
        self.leave_read = on;
    }

    /// Writes back the other levels drawn into whose memory overlaps
    /// `header`'s, before `header`'s surface is filled from or matched
    /// against that memory (two surface headers over one level).
    fn write_back_overlapping(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<(), D3dError> {
        let Some(t) = self.rtt.get(&header).copied() else {
            return Ok(());
        };
        let (start, end) = (t.data as u64, t.data as u64 + t.len() as u64);
        let others: Vec<u32> = self
            .rtt
            .iter()
            .filter(|(h, o)| {
                **h != header
                    && o.dirty
                    && (o.data as u64) < end
                    && start < o.data as u64 + o.len() as u64
            })
            .map(|(h, _)| *h)
            .collect();
        if self.park_overlapping(mem, header, &others)? {
            return Ok(());
        }
        for h in others {
            self.write_back(mem, h, WritebackCause::Overlap)?;
        }
        Ok(())
    }

    /// What the guest stored into a level since its host surface and
    /// guest memory last matched, as the level's current guest bytes and,
    /// per byte, whether the guest's store owns it; `None` if nothing.
    /// Owned: every byte of a store reported exactly
    /// ([`Device::note_write`]), whatever its value, and every byte that
    /// differs from the match (a page-granular report,
    /// [`Device::note_written_pages`], or a change the upload policy
    /// found). A page report that left the bytes as they were (the runtime
    /// reports the device's own write-backs again) only moves the match
    /// forward.
    pub(super) fn rtt_guest_stores(
        &mut self,
        mem: &dyn GuestMemory,
        header: u32,
    ) -> Result<Option<GuestStores>, D3dError> {
        let t = *self.rtt.get(&header).expect("cached");
        let len = t.len();
        let stores = self.rtt_stores.get(&header).cloned().unwrap_or_default();
        let written = self.tracker.dirty_since(t.data, len, t.generation);
        if self.cfg.upload == UploadPolicy::Tracked && !written && stores.is_empty() {
            return Ok(None);
        }
        let bytes = mem.read_vec(t.data, len as usize)?;
        let synced = self.rtt_synced.get(&header).expect("filled when made");
        let mut owned: Vec<bool> = bytes.iter().zip(synced).map(|(b, s)| b != s).collect();
        let changed = owned.contains(&true);
        for (off, n) in stores {
            owned[off as usize..(off + n) as usize].fill(true);
        }
        if !owned.contains(&true) {
            let generation = self.tracker.generation();
            self.rtt.get_mut(&header).expect("cached").generation = generation;
            return Ok(None);
        }
        if changed && !written && self.cfg.upload == UploadPolicy::Verify {
            self.events.push(Event::MissedInvalidation {
                header,
                addr: t.data,
                len,
            });
        }
        Ok(Some((bytes, owned)))
    }

    /// An exact guest store (`addr`, `len` bytes) into the levels it
    /// overlaps that hold pixels only on the host (lazily, neither active
    /// colour nor depth/stencil target, whose write-back overwrites the
    /// whole level as the eager one at the target change does).
    pub(super) fn note_rtt_store(&mut self, addr: u32, len: u32) {
        if self.rtt.is_empty() || !self.rtt_lazy() {
            return;
        }
        let (start, end) = (addr as u64, addr as u64 + len as u64);
        for (h, t) in &self.rtt {
            let (a, b) = (t.data as u64, t.data as u64 + t.len() as u64);
            if !t.dirty
                || *h == self.render_target
                || *h == self.depth_stencil
                || end <= a
                || b <= start
            {
                continue;
            }
            let (lo, hi) = (start.max(a), end.min(b));
            self.rtt_stores
                .entry(*h)
                .or_default()
                .push(((lo - a) as u32, (hi - lo) as u32));
        }
    }

    /// A draw samples texture `header`. When its memory is a
    /// render-to-texture level the backend can sample in place (lazy
    /// render-to-texture, the whole single-level `A8R8G8B8` texture over
    /// the level, no `COLORSIGN`, not the target being drawn), the level's
    /// own surface: no write-back, no upload. `None`: the texture goes
    /// through the write-back and the texture cache as before.
    pub(super) fn rtt_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        color_sign: u32,
    ) -> Result<Option<(TextureInfo, B::Texture)>, D3dError> {
        let Some((info, surface)) = self.rtt_level(mem, header, color_sign)? else {
            return Ok(None);
        };
        let Some(texture) = self
            .backend
            .surface_texture(surface)
            .map_err(D3dError::Backend)?
        else {
            return Ok(None);
        };
        self.stats.rtt_direct_samples += 1;
        Ok(Some((info, texture)))
    }

    /// A feedback bind needs an immutable image. Headers that reinterpret a
    /// single drawn level can also read its GPU pixels, with guest addressing.
    pub(super) fn rtt_alias_texture(
        &mut self, mem: &mut dyn GuestMemory, header: u32, color_sign: u32,
    ) -> Result<Option<(TextureInfo, B::Texture)>, D3dError> {
        if self.rtt.is_empty() || !self.backend.handles_surface_aliases() || !self.rtt_lazy() {
            return Ok(None);
        }
        let pc = PixelContainer::read(mem, header)?;
        if pc.resource.kind() != ResourceType::Texture { return Ok(None); }
        let data = self.cfg.address_map.to_virtual(pc.resource.data);
        let Some((&h, &t)) = self.rtt.iter().find(|(_, t)| t.dirty && data.wrapping_sub(t.data) < t.len()) else {
            return Ok(None);
        };
        let Ok(desc) = crate::texdesc::from_header(pc.format, pc.size) else { return Ok(None) };
        let Ok(len) = crate::texdesc::total_size(&desc) else { return Ok(None) };
        let offset = (data - t.data) as usize;
        if len > t.len() as usize - offset || self.dirty_level_overlap(h, &t) {
            return Ok(None); // multiple owners have to merge their bytes
        }
        let sign = texture_color_sign(desc.format, color_sign);
        let options = DecodeOptions { bc_supported: self.backend.bc_supported(), ..Default::default() }.with_color_sign(sign);
        if self.check_alias_decode(&desc, sign, options, len).is_err() {
            return Ok(None); // e.g. P8 also requires the guest's palette
        }
        if h != self.render_target && h != self.depth_stencil {
            self.refresh_rtt(mem, h)?;
        }
        let info = TextureInfo { header, palette: 0, format: desc.format, kind: desc.kind,
            width: desc.width, height: desc.height, depth: desc.depth, levels: desc.levels };
        let same = offset == 0 && sign == 0 && desc.kind == gfx::texture::TextureKind::D2 && desc.levels == 1
            && desc.format == t.format && (desc.width, desc.height) == (t.width, t.height)
            && match t.layout {
                GuestLayoutKind::Linear { pitch } => pitch == desc.pitch,
                GuestLayoutKind::Swizzled => true,
            };
        let texture = if same && !t.depth && gfx::format::WordFormat::of(t.format).is_some() {
            self.backend.snapshot_surface(t.surface, t.format.texel() == gfx::format::TexelFormat::X8R8G8B8)
        } else {
            let pitch = match t.layout { GuestLayoutKind::Linear { pitch } => pitch, GuestLayoutKind::Swizzled => 0 };
            self.backend.depth_alias_texture(t.surface, t.format, t.width, t.height, pitch,
                self.depth_generation, offset, &desc, options)
        }.map_err(D3dError::Backend)?;
        Ok(texture.map(|texture| {
            self.stats.rtt_direct_samples += 1;
            (info, texture)
        }))
    }

    /// Select the live level a sample can bind without decoding guest pixels.
    /// Clean overlapping levels do not participate in its write-back.
    pub(super) fn direct_color_level(
        &self,
        data: u32,
        desc: &TextureDesc,
        color_sign: u32,
    ) -> Option<(u32, RttTarget<B::Surface>)> {
        if !self.rtt_lazy()
            || color_sign & COLOR_SIGN_MASK != 0
            || desc.kind != gfx::texture::TextureKind::D2
            || desc.levels != 1
            || !matches!(desc.format, D3DFormat::A8R8G8B8 | D3DFormat::LinA8R8G8B8)
        {
            return None;
        }
        let layout = match desc.format.layout() {
            gfx::format::Layout::Swizzled => GuestLayoutKind::Swizzled,
            _ => GuestLayoutKind::Linear { pitch: desc.pitch.max(desc.width * 4) },
        };
        // Prefer a drawn level when two headers name the same texels; ties
        // go to the lower header. The active target needs a copy instead.
        let (&header, &level) = self.rtt.iter().filter(|(_, t)| {
            !t.depth && t.data == data && t.format == desc.format
                && (t.width, t.height) == (desc.width, desc.height) && t.layout == layout
        }).max_by_key(|&(header, t)| (t.dirty, std::cmp::Reverse(*header)))?;
        (header != self.render_target).then_some((header, level))
    }

    pub(super) fn dirty_level_overlap(&self, header: u32, level: &RttTarget<B::Surface>) -> bool {
        let (start, end) = (u64::from(level.data), u64::from(level.data) + u64::from(level.len()));
        self.rtt.iter().any(|(&other, t)| {
            other != header && t.dirty && u64::from(t.data) < end
                && start < u64::from(t.data) + u64::from(t.len())
        })
    }

    /// The render-to-texture level texture `header` samples in place, as
    /// [`Self::rtt_texture`] finds it (its write-back and refresh done),
    /// and the level's host surface: what a draw, or an external pass
    /// ([`crate::backend::ExternalPass::sampled`]), binding it now reads.
    pub(super) fn rtt_level(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        color_sign: u32,
    ) -> Result<Option<(TextureInfo, B::Surface)>, D3dError> {
        if self.rtt.is_empty() || !self.rtt_lazy() || color_sign & COLOR_SIGN_MASK != 0 {
            return Ok(None);
        }
        let pc = PixelContainer::read(mem, header)?;
        if pc.resource.kind() != ResourceType::Texture {
            return Ok(None);
        }
        let Ok(desc) = crate::texdesc::from_header(pc.format, pc.size) else {
            return Ok(None);
        };
        let data = self.cfg.address_map.to_virtual(pc.resource.data);
        let Some((h, t)) = self.direct_color_level(data, &desc, color_sign) else {
            return Ok(None);
        };
        // Another level drawn into over part of these bytes (an offset
        // alias) goes to memory first, and this one takes it up there.
        self.write_back_overlapping(mem, h)?;
        self.refresh_rtt(mem, h)?;
        Ok(Some((
            TextureInfo {
                header,
                palette: 0,
                format: desc.format,
                kind: desc.kind,
                width: desc.width,
                height: desc.height,
                depth: desc.depth,
                levels: desc.levels,
            },
            t.surface,
        )))
    }

    /// Copies `bytes` (the level's guest texels) into its host surface and
    /// records that they match. A backend that cannot (`Ok(false)`) leaves
    /// the surface as it was.
    pub(super) fn fill_rtt(&mut self, header: u32, bytes: Vec<u8>) -> Result<(), D3dError> {
        let t = *self.rtt.get(&header).expect("cached");
        let mut desc = TextureDesc::new(
            t.format,
            gfx::texture::TextureKind::D2,
            t.width,
            t.height,
            1,
            1,
        );
        if let GuestLayoutKind::Linear { pitch } = t.layout {
            desc.pitch = pitch;
        }
        self.backend
            .write_surface(t.surface, &desc, &bytes)
            .map_err(D3dError::Backend)?;
        self.frame_snaps.written(t.surface);
        self.stats.rtt_uploads += 1;
        let hash = if self.cfg.upload == UploadPolicy::Verify {
            hash_bytes(&bytes)
        } else {
            0
        };
        let generation = self.tracker.generation();
        let t = self.rtt.get_mut(&header).expect("cached");
        t.generation = generation;
        t.hash = hash;
        self.rtt_synced.insert(header, bytes);
        self.rtt_stores.remove(&header);
        self.depth_level_tex.remove(&header);
        Ok(())
    }

    /// A cached level is bound again: if the guest changed its texels since
    /// the host surface last matched them, fill it again. Content is checked
    /// apart from the header identity, by the upload policy, as for textures:
    /// `Always` refills every time, `Tracked` when the tracker saw a write,
    /// `Verify` when the hash differs (an unreported write is
    /// `Event::MissedInvalidation`). A level drawn into and not yet written
    /// back holds newer pixels than memory and is kept; lazily, guest
    /// bytes changed under it since are merged in ([`Device::write_back`]).
    fn refresh_rtt(&mut self, mem: &mut dyn GuestMemory, header: u32) -> Result<(), D3dError> {
        let t = *self.rtt.get(&header).expect("cached");
        if t.dirty {
            if self.rtt_lazy() && self.rtt_guest_stores(mem, header)?.is_some() {
                self.write_back(mem, header, WritebackCause::GuestStores)?;
            }
            return Ok(());
        }
        // The mirror took newer bytes over it since it was filled.
        if self.mirror_stale.contains(&header) {
            return self.fill_level(mem, header);
        }
        let len = t.len();
        let written = self.tracker.dirty_since(t.data, len, t.generation);
        match self.cfg.upload {
            UploadPolicy::Tracked if !written => return Ok(()),
            UploadPolicy::Verify => {
                let bytes = mem.read_vec(t.data, len as usize)?;
                if hash_bytes(&bytes) == t.hash {
                    return Ok(());
                }
                if !written {
                    self.events.push(Event::MissedInvalidation {
                        header,
                        addr: t.data,
                        len,
                    });
                }
                return self.fill_level(mem, header);
            }
            _ => {}
        }
        self.fill_level(mem, header)
    }

    /// A draw or clear has written the current colour target and, with
    /// `writes_depth`, possibly the depth/stencil surface: marks texture
    /// levels dirty and notes a change of the device's depth buffer
    /// (textures over it are read again). Called only once the backend has
    /// drawn: a skipped draw dirties nothing.
    pub(super) fn mark_written(&mut self, writes_depth: bool) {
        self.mark_color_written(self.render_target);
        if writes_depth { self.mark_depth_written(); }
    }

    pub(super) fn mark_color_written(&mut self, header: u32) {
        if let Some(t) = self.rtt.get_mut(&header) {
            self.rtt_epoch += u64::from(!t.dirty);
            t.dirty = true;
            self.mirror_supersede(header);
        }
        // The colour surface over the depth buffer's memory: that memory
        // now holds its pixels (textures over it made before are stale).
        if self
            .depth_color
            .is_some_and(|c| c.header == header)
        {
            self.depth_color_newer = true;
            self.depth_generation += 1;
        }
    }

    pub(super) fn mark_depth_written(&mut self) {
        self.mark_depth_header_written(self.depth_stencil);
    }

    pub(super) fn mark_depth_header_written(&mut self, header: u32) {
        if header != 0 {
            if let Some(t) = self.rtt.get_mut(&header) {
                self.rtt_epoch += u64::from(!t.dirty);
                t.dirty = true;
                self.depth_level_tex.remove(&header);
                self.mirror_supersede(header);
            }
            if header == self.depth_header {
                self.depth_generation += 1;
                self.depth_color_newer = false;
            }
        }
    }

    /// The deferred half of a `COPY` swap: before back0 is drawn over, its
    /// presented contents go to the front, which then holds the displayed
    /// image (what `PersistDisplay` keeps). A backend without
    /// [`Backend::copy_surface`] cannot keep it; `PersistDisplay` then stops.
    pub(super) fn materialize_display_copy(&mut self, color: B::Surface) -> Result<(), D3dError> {
        let Some(sw) = self.swap.as_mut() else {
            return Ok(());
        };
        // Owed by a `COPY` swap: made before either end is written, back0
        // (its source) or the display buffer (its destination, which a
        // title can bind as a target through the display header).
        if !sw.copy_pending || (sw.back[0] != color && sw.front != color) {
            return Ok(());
        }
        let (src, dst) = (sw.back[0], sw.front);
        sw.copy_pending = false;
        self.frame_snaps.written(dst);
        if self
            .backend
            .copy_surface(src, dst)
            .map_err(D3dError::Backend)?
        {
            self.present_cameras.copy(src, dst);
            let sw = self.swap.as_mut().expect("checked");
            if sw.shown == src {
                sw.shown = dst;
            }
        } else {
            self.present_cameras.invalidate(dst);
            self.display.shown_lost = true;
        }
        Ok(())
    }

    /// Writes one render-to-texture target back into guest memory if it
    /// was drawn into. Lazily, the guest may have stored into the level
    /// since its pixels went to the host only: the bytes its stores own
    /// ([`rtt_guest_stores`](Self::rtt_guest_stores)) win, and the surface
    /// is filled with the merged level.
    pub(super) fn write_back(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        cause: WritebackCause,
    ) -> Result<(), D3dError> {
        let Some(t) = self.rtt.get(&header).copied() else {
            return Ok(());
        };
        if !t.dirty {
            return Ok(());
        }
        self.stats.writeback_causes.count(cause);
        if writeback_trace() {
            eprintln!(
                "d3d write-back: {cause:?} level {header:#010x} (texture {:#010x}) {:?} {}x{} {} at vblank {}",
                t.parent,
                t.format,
                t.width,
                t.height,
                if t.depth { "depth" } else { "colour" },
                self.stats.vblanks,
            );
        }
        let mut bytes = if t.depth {
            self.backend
                .read_depth(t.surface, t.format, t.width, t.height, t.layout)
                .map_err(D3dError::Backend)?
                .ok_or_else(|| {
                    D3dError::Backend(format!(
                        "the backend cannot read depth level {header:#010x} ({:?}) back",
                        t.format
                    ))
                })?
        } else {
            self.backend
                .read_surface(t.surface, t.format, t.width, t.height, t.layout)
                .map_err(D3dError::Backend)?
        };
        let len = t.len() as usize;
        if bytes.len() < len {
            return Err(D3dError::Backend(format!(
                "read_surface returned {} bytes for a {len}-byte level",
                bytes.len()
            )));
        }
        bytes.truncate(len);
        let merged = match self.rtt_lazy() {
            true => self.rtt_guest_stores(mem, header)?,
            false => None,
        };
        if let Some((guest, owned)) = &merged {
            for (i, b) in bytes.iter_mut().enumerate() {
                if owned[i] {
                    *b = guest[i];
                }
            }
        }
        mem.write(t.data, &bytes)?;
        self.rtt_stores.remove(&header);
        self.tracker.note_write(t.data, len as u32);
        self.stats.rtt_writebacks += 1;
        let hash = if self.cfg.upload == UploadPolicy::Verify {
            hash_bytes(&bytes)
        } else {
            0
        };
        let generation = self.tracker.generation();
        if let Some(t) = self.rtt.get_mut(&header) {
            t.dirty = false;
            t.generation = generation;
            t.hash = hash;
        }
        if merged.is_some() {
            self.fill_rtt(header, bytes)?;
        } else {
            self.rtt_synced.insert(header, bytes);
        }
        Ok(())
    }

    /// A point where, eagerly, the guest could look at memory (`Swap`,
    /// `KickOff`): writes back every dirty target unless render-to-texture
    /// is lazy, where only a wait or busy check on the resource, a generic
    /// wait (`BlockOnTime`), a push-buffer callback, `PersistDisplay`,
    /// teardown, or a sample that cannot read the surface in place does.
    pub(super) fn flush_at_sync(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        if self.rtt_lazy() {
            return Ok(());
        }
        self.flush_render_targets_for(mem, WritebackCause::Sync)
    }

    /// Writes back every dirty render-to-texture target (the guest is about
    /// to look at memory: `BlockOnTime`, push-buffer callbacks, teardown,
    /// and eagerly also `Swap`, `KickOff`, `BlockOnResource` and the busy
    /// check).
    pub fn flush_render_targets(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        self.flush_render_targets_for(mem, WritebackCause::Host)
    }

    /// [`flush_render_targets`](Self::flush_render_targets), counted under `cause`.
    pub(super) fn flush_render_targets_for(
        &mut self,
        mem: &mut dyn GuestMemory,
        cause: WritebackCause,
    ) -> Result<(), D3dError> {
        let headers: Vec<u32> = self
            .rtt
            .iter()
            .filter(|(_, t)| t.dirty)
            .map(|(h, _)| *h)
            .collect();
        for h in headers {
            self.write_back(mem, h, cause)?;
        }
        self.mirror_flush_all(mem)
    }

    /// Writes back the targets that are `texture`, or levels of it (by
    /// `Parent`, or by texel address): before it is sampled through guest
    /// memory, or, lazily, before the guest waits on it or checks it is
    /// busy (`texture` is then any resource).
    pub(super) fn flush_for_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        texture: u32,
        cause: WritebackCause,
    ) -> Result<(), D3dError> {
        if self.rtt.is_empty() && self.mirror.is_none() {
            return Ok(());
        }
        // Nothing has changed for this texture since the last call: no
        // level to write back, the same range to flush. (A 12-byte header
        // at the end of mapped memory is not remembered.)
        let words = crate::guest::read_array::<5>(mem, texture).ok();
        if let Some(&(w, epoch, lo, hi)) = self.texture_flushed.get(&texture)
            && Some(w) == words
            && epoch == self.rtt_epoch
        {
            return self.mirror_flush_range(mem, lo, hi);
        }
        let (start, end) = self.resource_span(mem, texture)?;
        let headers: Vec<u32> = self
            .rtt
            .iter()
            .filter(|(h, t)| {
                t.dirty
                    && (**h == texture
                        || t.parent == texture
                        || (start < t.data as u64 + t.len() as u64 && (t.data as u64) < end))
            })
            .map(|(h, _)| *h)
            .collect();
        for h in headers {
            self.write_back(mem, h, cause)?;
        }
        // The mirror's bytes over the resource (and over the levels just
        // written back, as those were).
        let mut lo = start;
        let mut hi = end;
        for t in self.rtt.values() {
            if t.parent == texture {
                lo = lo.min(t.data as u64);
                hi = hi.max(t.data as u64 + t.len() as u64);
            }
        }
        // Written back, the levels are clean: the answer holds until a
        // target changes.
        if let Some(words) = words {
            self.texture_flushed.insert(texture, (words, self.rtt_epoch, lo, hi));
        }
        self.mirror_flush_range(mem, lo, hi)
    }

    /// The guest memory `resource` reads: a texture's or surface's whole
    /// layout from `Data`, any other resource's first byte.
    pub(super) fn resource_span(&self, mem: &dyn GuestMemory, resource: u32) -> Result<(u64, u64), D3dError> {
        let header = ResourceHeader::read(mem, resource)?;
        let start = self.cfg.address_map.to_virtual(header.data) as u64;
        let len = match header.kind() {
            ResourceType::Texture | ResourceType::Surface => {
                let pc = PixelContainer::read(mem, resource)?;
                crate::texdesc::from_header(pc.format, pc.size)
                    .ok()
                    .and_then(|d| crate::texdesc::total_size(&d).ok())
                    .map_or(1, |l| l as u64)
            }
            _ => 1,
        };
        Ok((start, start + len.max(1)))
    }

    /// Teardown: writes back every dirty level, then destroys the host
    /// surfaces and empties the cache.
    pub(super) fn release_render_targets(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<(), D3dError> {
        self.flush_render_targets_for(mem, WritebackCause::Teardown)?;
        self.mirror_stale.clear();
        self.rtt_synced.clear();
        self.rtt_stores.clear();
        self.depth_level_tex.clear();
        self.rtt_epoch += 1;
        for (_, t) in self.rtt.drain().collect::<Vec<_>>() {
            self.drop_surface(t.surface)?;
        }
        Ok(())
    }

    /// The colour target is about to change: eagerly, its pixels go to
    /// guest memory; lazily they stay on the host.
    pub(super) fn leave_target(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let h = self.render_target;
        if !self.rtt_lazy() {
            return self.write_back(mem, h, WritebackCause::LeaveTarget);
        }
        // Eagerly the write-back here would overwrite every guest store
        // made while the level was the target; lazily the host pixels win
        // over the same stores when the level is written back. Guest
        // bytes as they are now become the match, so only stores after
        // this count as the guest's.
        self.resync_on_leave(mem, h)
    }

    /// Lazily, level `h` stops being a target and keeps its pixels on the
    /// host: if drawn into, its guest bytes as they are now become the
    /// match. When the match it holds already is those bytes (tracked
    /// uploads, no exact store and no reported write to its pages since
    /// the match was made), the match only moves forward and the level is
    /// not read again: that read was a whole level's copy at every target
    /// change. It trusts the tracker as
    /// [`rtt_guest_stores`](Self::rtt_guest_stores) does for the same
    /// question. [`Device::set_leave_read`] reads the level every time.
    fn resync_on_leave(&mut self, mem: &dyn GuestMemory, h: u32) -> Result<(), D3dError> {
        let Some(t) = self.rtt.get(&h).copied() else {
            return Ok(());
        };
        if !t.dirty {
            return Ok(());
        }
        let len = t.len() as usize;
        let unchanged = !self.leave_read
            && self.cfg.upload == UploadPolicy::Tracked
            && self.rtt_synced.get(&h).is_some_and(|b| b.len() == len)
            && self.rtt_stores.get(&h).is_none_or(Vec::is_empty)
            && !self.tracker.dirty_since(t.data, t.len(), t.generation);
        if unchanged {
            self.stats.rtt_leave_reads_skipped += 1;
        } else if let Some(bytes) = self.rtt_synced.get_mut(&h).filter(|b| b.len() == len) {
            // The old match's buffer is refilled in place, not freed and
            // made again (a read that faults copies nothing).
            mem.read(t.data, bytes)?;
        } else {
            let bytes = mem.read_vec(t.data, len)?;
            self.rtt_synced.insert(h, bytes);
        }
        self.rtt_stores.remove(&h);
        let generation = self.tracker.generation();
        self.rtt.get_mut(&h).expect("cached").generation = generation;
        Ok(())
    }

    /// Whether depth levels stay on the host until the guest is about to
    /// read them, as colour levels do lazily, and draws sample them through
    /// [`Backend::depth_level_texture`]: lazy render-to-texture, a backend
    /// that can ([`Backend::can_sample_depth_surfaces`]), and not turned off
    /// ([`Device::set_lazy_depth`]).
    pub(super) fn rtt_lazy_depth(&self) -> bool {
        self.lazy_depth && self.rtt_lazy() && self.backend.can_sample_depth_surfaces()
    }

    /// Lazily, depth level `h` stops being the depth/stencil target and
    /// keeps its pixels on the host: as for a colour level
    /// ([`leave_target`](Self::leave_target)), the guest bytes as they are
    /// now become the match, so only stores after this count as the
    /// guest's when it is written back.
    pub(super) fn leave_depth(&mut self, mem: &mut dyn GuestMemory, h: u32) -> Result<(), D3dError> {
        self.resync_on_leave(mem, h)
    }

    /// A draw samples texture `header`. When its memory is a dirty depth
    /// level kept on the host (lazy depth, [`rtt_lazy_depth`](Self::rtt_lazy_depth)):
    /// a `D24S8` level under a whole single-level 2D texture of the same
    /// format, size and layout, with no `COLORSIGN`. The backend makes the
    /// texture from the surface ([`Backend::depth_level_texture`]: the same
    /// texels an upload of the written-back level gives), once per time the
    /// level is drawn into. Answers the texture's info and the level, whose
    /// texture is in `depth_level_tex`. `None`: through the write-back and
    /// the texture cache as before.
    pub(super) fn rtt_depth_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        color_sign: u32,
    ) -> Result<Option<(TextureInfo, u32)>, D3dError> {
        if self.rtt.is_empty() || !self.rtt_lazy_depth() || color_sign & COLOR_SIGN_MASK != 0 {
            return Ok(None);
        }
        let pc = PixelContainer::read(mem, header)?;
        if pc.resource.kind() != ResourceType::Texture {
            return Ok(None);
        }
        let Ok(desc) = crate::texdesc::from_header(pc.format, pc.size) else {
            return Ok(None);
        };
        if desc.kind != gfx::texture::TextureKind::D2
            || desc.levels != 1
            || !matches!(desc.format, D3DFormat::D24S8 | D3DFormat::LinD24S8)
        {
            return Ok(None);
        }
        let data = self.cfg.address_map.to_virtual(pc.resource.data);
        let Some((h, t)) = self
            .rtt
            .iter()
            .find(|(_, t)| t.depth && t.dirty && t.data == data)
            .map(|(h, t)| (*h, *t))
        else {
            return Ok(None);
        };
        let pitch = desc.pitch.max(desc.width * 4);
        let layout = match desc.format.layout() {
            gfx::format::Layout::Swizzled => GuestLayoutKind::Swizzled,
            _ => GuestLayoutKind::Linear { pitch },
        };
        if t.format != desc.format || (t.width, t.height) != (desc.width, desc.height) || t.layout != layout {
            return Ok(None);
        }
        // Another level over these bytes goes to memory first, and guest
        // stores into this one are merged (which writes it back: then it
        // is clean and goes through guest memory below).
        self.write_back_overlapping(mem, h)?;
        self.refresh_rtt(mem, h)?;
        if !self.rtt.get(&h).is_some_and(|t| t.dirty) {
            return Ok(None);
        }
        if !self.depth_level_tex.contains_key(&h) {
            let Some(texture) = self
                .backend
                .depth_level_texture(t.surface)
                .map_err(D3dError::Backend)?
            else {
                return Ok(None);
            };
            self.stats.depth_level_textures += 1;
            self.depth_level_tex.insert(h, texture);
        }
        self.stats.rtt_direct_samples += 1;
        Ok(Some((
            TextureInfo {
                header,
                palette: 0,
                format: desc.format,
                kind: desc.kind,
                width: desc.width,
                height: desc.height,
                depth: desc.depth,
                levels: desc.levels,
            },
            h,
        )))
    }

    /// The area a draw may write: its bounding rectangle
    /// ([`ResolvedDraw::clip`]) and, when it is not one rectangle, its
    /// disjoint pieces ([`ResolvedDraw::clip_rects`]). `None`: nothing.
    pub(super) fn draw_clip(&self, mem: &dyn GuestMemory) -> Result<Option<ClipArea>, D3dError> {
        let (w, h) = self.target_size(mem)?;
        // The surface clip as `SetScissors` sets it (`0x3f8760`): the
        // viewport with no rectangles (`0x3f87a7`), the rectangle itself
        // for one inclusive rectangle (`0x3f886c`), and otherwise the whole
        // target, cut by the window clip (`0x3f8928`). The viewport is no
        // clip of its own in the last two.
        let target = (0, 0, w as i64, h as i64);
        let (base, windows): (Edges, &[Rect]) = match self.scissors.as_slice() {
            [] => {
                let v = self.viewport;
                let vp = (
                    v.x.min(w) as i64,
                    v.y.min(h) as i64,
                    (v.x as i64 + v.width as i64).min(w as i64),
                    (v.y as i64 + v.height as i64).min(h as i64),
                );
                (vp, &[])
            }
            [one] if !self.scissor_exclusive => (target, std::slice::from_ref(one)),
            all => (target, all),
        };
        let mut pieces = scissor_region(base, windows, self.scissor_exclusive);
        if pieces.len() <= 1 {
            return Ok(pieces.pop().map(|p| (p, Vec::new())));
        }
        let x1 = pieces.iter().map(|p| p.x).min().expect("several");
        let y1 = pieces.iter().map(|p| p.y).min().expect("several");
        let x2 = pieces.iter().map(|p| p.x + p.width).max().expect("several");
        let y2 = pieces
            .iter()
            .map(|p| p.y + p.height)
            .max()
            .expect("several");
        let bound = PixelRect {
            x: x1,
            y: y1,
            width: x2 - x1,
            height: y2 - y1,
        };
        Ok(Some((bound, pieces)))
    }

    /// The projected, guest-rounded rectangle after SetScissors. A
    /// disjoint window would need several scissored draws.
    pub fn light_scissor(&self, mem: &dyn GuestMemory) -> Result<[u32; 4], &'static str> {
        match self.draw_clip(mem).map_err(|_| "unreadable light scissor")? {
            None => Ok([0; 4]),
            Some((r, pieces)) if pieces.is_empty() => Ok([r.x, r.y, r.width, r.height]),
            Some(_) => Err("light uses disjoint window clips"),
        }
    }

    /// `D3DDevice_Clear` (`0x3f9fc0`, stdcall `Count, pRects, Flags, Color,
    /// Z, Stencil`).
    pub fn clear(
        &mut self,
        mem: &mut dyn GuestMemory,
        count: u32,
        rects: u32,
        flags: u32,
        color: u32,
        z: f32,
        stencil: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_Clear";
        self.live(E)?;
        self.sync_residual_entry(mem, EntryKind::Clear)?;
        let (w, h) = self.target_size(mem)?;
        let v = self.viewport;
        let mask = ChannelMask::from_flags(flags);
        if self.residual.enabled
            && count == 0
            && w != 0
            && h != 0
            && v.x == 0
            && v.y == 0
            && v.width >= w
            && v.height >= h
            && (mask.all() || !mask.any())
        {
            return self.native_attachment_clear(mem, E, flags, mask, color, z, stencil);
        }
        let vp = Rect {
            x1: v.x as i32,
            y1: v.y as i32,
            x2: (v.x as i64 + v.width as i64).min(i32::MAX as i64) as i32,
            y2: (v.y as i64 + v.height as i64).min(i32::MAX as i64) as i32,
        };
        if count > MAX_CLEAR_RECTS {
            return Err(D3dError::invalid(E, format!("{count} rectangles")));
        }
        let mut rect_list = Vec::with_capacity(count as usize);
        for i in 0..count {
            let [x1, y1, x2, y2] = crate::guest::read_array::<4>(mem, at(rects, 16 * i as u64)?)?;
            rect_list.push(Rect {
                x1: x1 as i32,
                y1: y1 as i32,
                x2: x2 as i32,
                y2: y2 as i32,
            });
        }
        let bounds = (
            i64::from(vp.x1.max(0)),
            i64::from(vp.y1.max(0)),
            i64::from(vp.x2).min(i64::from(w)),
            i64::from(vp.y2).min(i64::from(h)),
        );
        // The union planner partitions both axes at rectangle edges. Bound
        // its grid on long guest lists and keep the linear per-rect path.
        const MAX_UNION_RECTS: u32 = 8;
        let native = self.residual.enabled && count <= MAX_UNION_RECTS;
        // A tiled depth clear must stay scissored: the depth attachment
        // can extend beyond the colour target, including on its scaled copy.
        let writes_zs = flags & (clear_flags::ZBUFFER | clear_flags::STENCIL) != 0;
        let pixels = if native && (count == 0 || count > 1 && !writes_zs) {
            scissor_region(bounds, &rect_list, false)
        } else {
            if count == 0 {
                rect_list.push(vp);
            }
            rect_list.iter().filter_map(|r| {
                let x1 = i64::from(r.x1).max(bounds.0);
                let y1 = i64::from(r.y1).max(bounds.1);
                let x2 = i64::from(r.x2).min(bounds.2);
                let y2 = i64::from(r.y2).min(bounds.3);
                (x1 < x2 && y1 < y2).then(|| PixelRect {
                    x: x1 as u32, y: y1 as u32,
                    width: (x2 - x1) as u32, height: (y2 - y1) as u32,
                })
            }).collect()
        };
        if self.residual.enabled && !native {
            self.stats.clear_refused_rect_count += 1;
        }
        if pixels.is_empty() {
            return Ok(());
        }
        let (c, d) = self.current_targets(mem, E)?;
        let depth = (flags & clear_flags::ZBUFFER != 0 && d.is_some()).then_some(z);
        let stencil = (flags & clear_flags::STENCIL != 0 && d.is_some()).then_some(stencil as u8);
        if !mask.any() && depth.is_none() && stencil.is_none() {
            return Ok(());
        }
        self.materialize_display_copy(c)?;
        let writes_depth = depth.is_some() || stencil.is_some();
        let whole = pixels.iter().any(|r| r.is_whole(w, h));
        if whole && (mask.all() || !mask.any()) {
            let clear = Clear {
                color: mask.all().then(|| d3dcolor_to_rgba(color)),
                depth,
                stencil,
            };
            self.backend.clear(c, d, clear).map_err(D3dError::Backend)?;
            if native {
                self.stats.native_rect_clears += 1;
            }
            if clear.color.is_some() {
                self.color_surface_written(c);
            }
            self.mark_written(writes_depth);
            return Ok(());
        }
        let rc = RectClear {
            rects: if whole {
                vec![PixelRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                }]
            } else {
                pixels
            },
            color: mask.any().then(|| (d3dcolor_to_rgba(color), mask)),
            depth,
            stencil,
        };
        self.stats.partial_clears += 1;
        match self
            .backend
            .clear_rects(c, d, &rc)
            .map_err(D3dError::Backend)?
        {
            DrawOutcome::Drawn => {
                if native {
                    self.stats.native_rect_clears += 1;
                }
                if rc.color.is_some() {
                    self.color_surface_written(c);
                }
                self.mark_written(writes_depth);
                Ok(())
            }
            DrawOutcome::Skipped(reason) => {
                self.events.push(Event::DrawSkipped { entry: E, reason });
                Ok(())
            }
            DrawOutcome::Unsupported(detail) => Err(D3dError::Unimplemented { entry: E, detail }),
        }
    }
}
