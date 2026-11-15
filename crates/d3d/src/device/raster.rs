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
//! * **Frame buffer as a texture.** A texture whose `Data` is the back
//!   buffer's ([`BACK_BUFFER_DATA`]) is sampled from a copy of the back
//!   buffer ([`Backend::snapshot_surface`]); one over the depth buffer
//!   ([`DEPTH_BUFFER_DATA`]) skips the draw.
//! * **Render to texture.** A surface whose `Parent` is a texture (a level,
//!   as `0x14850` builds for its two 64x64 targets with the native
//!   `0x3fb7a0`) gets its own host surface. When the guest can next observe
//!   the texels (the target changes, `Swap`, `KickOff`/`BlockOn*`, the
//!   busy check `0x3fb710`, or a draw samples that texture), the device
//!   reads the surface back in the level's format and layout and writes it
//!   into the level's guest memory, then reports the write to its own
//!   tracker so cached uploads of that texture are refreshed.

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

/// What a bound texture header is, as far as the frame buffers go.
pub(super) enum FrameTexture<T> {
    /// Ordinary guest texture memory.
    No,
    /// The back buffer, copied for this draw.
    Snapshot(TextureInfo, T),
    /// The draw cannot sample it: skip with this reason.
    Skip(String),
}

/// Left, top, right, bottom (right and bottom exclusive).
type Edges = (i64, i64, i64, i64);

/// A draw's clip: the bounding rectangle and, when it is not one
/// rectangle, the pieces.
type ClipArea = (PixelRect, Vec<PixelRect>);

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
    /// Whether texture `header` reads one of the device's own frame
    /// buffers ([`BACK_BUFFER_DATA`], [`DEPTH_BUFFER_DATA`]), and if so how
    /// a draw samples it. A stage the draw does not read (`sampled` false)
    /// is not copied; the caller leaves it unbound.
    pub(super) fn frame_texture(
        &mut self,
        mem: &dyn GuestMemory,
        header: u32,
        sampled: bool,
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
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} reads the depth buffer's memory (format word {:#x}): depth bits sampled as colour are not modelled",
                pc.format
            )));
        }
        if !within(BACK_BUFFER_DATA) {
            return Ok(FrameTexture::No);
        }
        let (back, (bw, bh)) = (sw.back[0], sw.size);
        let desc = match TextureDesc::from_header(pc.format, pc.size) {
            Ok(d) => d,
            Err(e) => {
                return Ok(FrameTexture::Skip(format!(
                    "texture {header:#010x} over the back buffer: {e}"
                )));
            }
        };
        let exact = data == map.to_virtual(BACK_BUFFER_DATA);
        let format_ok = matches!(desc.format, D3DFormat::LinA8R8G8B8 | D3DFormat::LinX8R8G8B8)
            && sw.format.bytes_per_texel() == Some(4);
        if !exact
            || !format_ok
            || desc.kind != gfx::texture::TextureKind::D2
            || (desc.width, desc.height) != (bw, bh)
        {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over the back buffer ({:?} {}x{} at +{:#x}) is not the whole {bw}x{bh} {:?} image as a linear 32-bit texture",
                desc.format,
                desc.width,
                desc.height,
                data.wrapping_sub(map.to_virtual(BACK_BUFFER_DATA)),
                sw.format
            )));
        }
        if !sampled {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} over the back buffer on a stage the draw does not read"
            )));
        }
        // X8: the texel's alpha reads as one, whatever the buffer holds.
        let opaque = desc.format == D3DFormat::LinX8R8G8B8;
        let Some(t) = self
            .backend
            .snapshot_surface(back, opaque)
            .map_err(D3dError::Backend)?
        else {
            return Ok(FrameTexture::Skip(format!(
                "texture {header:#010x} reads the back buffer and the backend cannot copy surfaces into textures"
            )));
        };
        self.stats.frame_snapshots += 1;
        Ok(FrameTexture::Snapshot(
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
            t,
        ))
    }

    /// The host surface for a texture-level render target (made on first
    /// use; remade if the header's identity changed).
    pub(super) fn rtt_surface(
        &mut self,
        mem: &dyn GuestMemory,
        entry: &'static str,
        header: u32,
        h: &SurfaceHeader,
        depth: bool,
    ) -> Result<B::Surface, D3dError> {
        // Checked before the cache: a level already used as a colour target
        // must still stop when bound as depth.
        if depth {
            return Err(D3dError::unimplemented(
                entry,
                format!(
                    "depth/stencil surface {header:#010x} is a level of texture {:#010x} (depth render-to-texture)",
                    h.parent
                ),
            ));
        }
        let id = h.pixels.identity();
        if let Some(t) = self.rtt.get(&header).copied()
            && t.identity == id
        {
            self.refresh_rtt(mem, header, t)?;
            return Ok(t.surface);
        }
        let desc = TextureDesc::from_header(h.pixels.format, h.pixels.size)
            .map_err(|e| D3dError::unimplemented(entry, format!("surface header: {e}")))?;
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
        // The header was rewritten: its old surface goes. Content not yet
        // written back would be lost, which cannot happen through D3D (the
        // write-back runs when the target changes), so it is an error.
        if let Some(old) = self.rtt.get(&header).copied() {
            if old.dirty {
                return Err(D3dError::invalid(
                    entry,
                    format!(
                        "render target {header:#010x} was rewritten while it held unwritten pixels"
                    ),
                ));
            }
            self.rtt.remove(&header);
            self.backend
                .destroy_surface(old.surface)
                .map_err(D3dError::Backend)?;
        }
        let surface = self
            .backend
            .create_texture_target(desc.format, desc.width, desc.height)
            .map_err(D3dError::Backend)?;
        let target = RttTarget {
            surface,
            parent: h.parent,
            data: self.cfg.address_map.to_virtual(h.pixels.resource.data),
            identity: id,
            format: desc.format,
            width: desc.width,
            height: desc.height,
            layout,
            dirty: false,
            generation: 0,
            hash: 0,
        };
        self.tracker.watch(target.data, target.len());
        self.rtt.insert(header, target);
        // The level's current texels, so draws that do not cover it all
        // leave the rest as the guest had it.
        let bytes = mem.read_vec(target.data, target.len() as usize)?;
        self.fill_rtt(header, &bytes)?;
        Ok(surface)
    }

    /// Copies `bytes` (the level's guest texels) into its host surface and
    /// records that they match. A backend that cannot (`Ok(false)`) leaves
    /// the surface as it was.
    fn fill_rtt(&mut self, header: u32, bytes: &[u8]) -> Result<(), D3dError> {
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
            .write_surface(t.surface, &desc, bytes)
            .map_err(D3dError::Backend)?;
        self.stats.rtt_uploads += 1;
        let hash = if self.cfg.upload == UploadPolicy::Verify {
            hash_bytes(bytes)
        } else {
            0
        };
        let generation = self.tracker.generation();
        let t = self.rtt.get_mut(&header).expect("cached");
        t.generation = generation;
        t.hash = hash;
        Ok(())
    }

    /// A cached level is bound again: if the guest changed its texels since
    /// the host surface last matched them, fill it again. Content is checked
    /// apart from the header identity, by the upload policy, as for textures:
    /// `Always` refills every time, `Tracked` when the tracker saw a write,
    /// `Verify` when the hash differs (an unreported write is
    /// `Event::MissedInvalidation`). A level drawn into and not yet written
    /// back holds newer pixels than memory and is kept.
    fn refresh_rtt(
        &mut self,
        mem: &dyn GuestMemory,
        header: u32,
        t: RttTarget<B::Surface>,
    ) -> Result<(), D3dError> {
        if t.dirty {
            return Ok(());
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
                return self.fill_rtt(header, &bytes);
            }
            _ => {}
        }
        let bytes = mem.read_vec(t.data, len as usize)?;
        self.fill_rtt(header, &bytes)
    }

    /// About to write into `color`, the current colour target: marks a
    /// texture level dirty, and preserves the presented image if `color` is
    /// the back buffer a `COPY` swap showed.
    pub(super) fn touch_target(&mut self, color: B::Surface) -> Result<(), D3dError> {
        if let Some(t) = self.rtt.get_mut(&self.render_target) {
            t.dirty = true;
        }
        self.before_target_write(color)
    }

    /// The deferred half of a `COPY` swap: before back0 is drawn over, its
    /// presented contents go to the front, which then holds the displayed
    /// image (what `PersistDisplay` keeps). A backend without
    /// [`Backend::copy_surface`] cannot keep it; `PersistDisplay` then stops.
    pub(super) fn before_target_write(&mut self, color: B::Surface) -> Result<(), D3dError> {
        let Some(sw) = self.swap.as_mut() else {
            return Ok(());
        };
        if !sw.shown_in_back || sw.shown != color {
            return Ok(());
        }
        let (src, dst) = (sw.shown, sw.front);
        sw.shown_in_back = false;
        if self
            .backend
            .copy_surface(src, dst)
            .map_err(D3dError::Backend)?
        {
            self.swap.as_mut().expect("checked").shown = dst;
        } else {
            self.display.shown_lost = true;
        }
        Ok(())
    }

    /// Writes one render-to-texture target back into guest memory if it
    /// was drawn into.
    pub(super) fn write_back(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<(), D3dError> {
        let Some(t) = self.rtt.get(&header).copied() else {
            return Ok(());
        };
        if !t.dirty {
            return Ok(());
        }
        let bytes = self
            .backend
            .read_surface(t.surface, t.format, t.width, t.height, t.layout)
            .map_err(D3dError::Backend)?;
        let len = t.len() as usize;
        if bytes.len() < len {
            return Err(D3dError::Backend(format!(
                "read_surface returned {} bytes for a {len}-byte level",
                bytes.len()
            )));
        }
        mem.write(t.data, &bytes[..len])?;
        self.tracker.note_write(t.data, len as u32);
        self.stats.rtt_writebacks += 1;
        let hash = if self.cfg.upload == UploadPolicy::Verify {
            hash_bytes(&bytes[..len])
        } else {
            0
        };
        let generation = self.tracker.generation();
        if let Some(t) = self.rtt.get_mut(&header) {
            t.dirty = false;
            t.generation = generation;
            t.hash = hash;
        }
        Ok(())
    }

    /// Writes back every dirty render-to-texture target (the guest is about
    /// to look at memory: `Swap`, `KickOff`, `BlockOn*`, the busy check).
    pub fn flush_render_targets(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let headers: Vec<u32> = self
            .rtt
            .iter()
            .filter(|(_, t)| t.dirty)
            .map(|(h, _)| *h)
            .collect();
        for h in headers {
            self.write_back(mem, h)?;
        }
        Ok(())
    }

    /// Writes back the targets that are levels of `texture` (by `Parent`, or
    /// by texel address), before it is sampled.
    pub(super) fn flush_for_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        texture: u32,
    ) -> Result<(), D3dError> {
        if self.rtt.is_empty() {
            return Ok(());
        }
        let data = self
            .cfg
            .address_map
            .to_virtual(mem.read_u32(at(texture, 4)?)?);
        let headers: Vec<u32> = self
            .rtt
            .iter()
            .filter(|(_, t)| t.dirty && (t.parent == texture || t.data == data))
            .map(|(h, _)| *h)
            .collect();
        for h in headers {
            self.write_back(mem, h)?;
        }
        Ok(())
    }

    /// Teardown: writes back every dirty level, then destroys the host
    /// surfaces and empties the cache.
    pub(super) fn release_render_targets(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<(), D3dError> {
        self.flush_render_targets(mem)?;
        for (_, t) in self.rtt.drain().collect::<Vec<_>>() {
            self.backend
                .destroy_surface(t.surface)
                .map_err(D3dError::Backend)?;
        }
        Ok(())
    }

    /// The colour target is about to change.
    pub(super) fn leave_target(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let h = self.render_target;
        self.write_back(mem, h)
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

    /// `D3DDevice_Clear` (`0x3f9fc0`, stdcall `Count, pRects, Flags, Color,
    /// Z, Stencil`).
    #[allow(clippy::too_many_arguments)] // the D3D signature
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
        self.sync_push_buffer(mem)?;
        let (w, h) = self.target_size(mem)?;
        let v = self.viewport;
        let vp = Rect {
            x1: v.x as i32,
            y1: v.y as i32,
            x2: (v.x as i64 + v.width as i64).min(i32::MAX as i64) as i32,
            y2: (v.y as i64 + v.height as i64).min(i32::MAX as i64) as i32,
        };
        let mut effective = Vec::new();
        if count == 0 {
            effective.push(vp);
        } else {
            if count > MAX_CLEAR_RECTS {
                return Err(D3dError::invalid(E, format!("{count} rectangles")));
            }
            for i in 0..count {
                let r = mem.read_words(at(rects, 16 * i as u64)?, 4)?;
                effective.push(Rect {
                    x1: (r[0] as i32).max(vp.x1),
                    y1: (r[1] as i32).max(vp.y1),
                    x2: (r[2] as i32).min(vp.x2),
                    y2: (r[3] as i32).min(vp.y2),
                });
            }
        }
        // Clip to the target as well; drop what is left empty.
        let pixels: Vec<PixelRect> = effective
            .iter()
            .filter_map(|r| {
                let x1 = r.x1.max(0) as i64;
                let y1 = r.y1.max(0) as i64;
                let x2 = (r.x2 as i64).min(w as i64);
                let y2 = (r.y2 as i64).min(h as i64);
                (x1 < x2 && y1 < y2).then(|| PixelRect {
                    x: x1 as u32,
                    y: y1 as u32,
                    width: (x2 - x1) as u32,
                    height: (y2 - y1) as u32,
                })
            })
            .collect();
        if pixels.is_empty() {
            return Ok(());
        }
        let mask = ChannelMask::from_flags(flags);
        let (c, d) = self.current_targets(mem, E)?;
        let depth = (flags & clear_flags::ZBUFFER != 0 && d.is_some()).then_some(z);
        let stencil = (flags & clear_flags::STENCIL != 0 && d.is_some()).then_some(stencil as u8);
        if !mask.any() && depth.is_none() && stencil.is_none() {
            return Ok(());
        }
        self.touch_target(c)?;
        let whole = pixels.iter().any(|r| r.is_whole(w, h));
        if whole && (mask.all() || !mask.any()) {
            let clear = Clear {
                color: mask.all().then(|| d3dcolor_to_rgba(color)),
                depth,
                stencil,
            };
            return self.backend.clear(c, d, clear).map_err(D3dError::Backend);
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
            DrawOutcome::Drawn => Ok(()),
            DrawOutcome::Skipped(reason) => {
                self.events.push(Event::DrawSkipped { entry: E, reason });
                Ok(())
            }
            DrawOutcome::Unsupported(detail) => Err(D3dError::Unimplemented { entry: E, detail }),
        }
    }
}
