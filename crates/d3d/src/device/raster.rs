//! Clears, the draw clip rectangle, and render-to-texture targets.
//!
//! * **Clears.** `D3DDevice_Clear` intersects every rectangle with the
//!   viewport (Count = 0 clears the viewport, `0x3fa1a0..0x3fa247`) and
//!   honours the per-channel bits (`0x10..0x80`, which Halo's wrapper
//!   `0x14b60` builds from `EAX` bits 1..4). One whole-target, all-channel
//!   clear goes to [`Backend::clear`]; anything else to
//!   [`Backend::clear_rects`].
//! * **Clip.** Draws carry the rectangle they may write
//!   ([`ResolvedDraw::clip`]): the viewport (SetViewport resets the window
//!   clip to it, `0x3f82e8` -> `0x3f8760`) narrowed by inclusive scissors.
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

impl<B: Backend> Device<B> {
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
        if let Some(t) = self.rtt.get(&header)
            && t.identity == id
        {
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
            .create_surface(desc.format, desc.width, desc.height, false)
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
        };
        // The level's current texels, so draws that do not cover it all
        // leave the rest as the guest had it. A backend that cannot
        // (`Ok(false)`) leaves the surface as created.
        let bytes = mem.read_vec(target.data, target.len() as usize)?;
        self.backend
            .write_surface(
                surface,
                target.format,
                target.width,
                target.height,
                layout,
                &bytes,
            )
            .map_err(D3dError::Backend)?;
        self.rtt.insert(header, target);
        Ok(surface)
    }

    /// Marks the current colour target drawn into, if it is a texture level.
    pub(super) fn touch_target(&mut self) {
        if let Some(t) = self.rtt.get_mut(&self.render_target) {
            t.dirty = true;
        }
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
        if let Some(t) = self.rtt.get_mut(&header) {
            t.dirty = false;
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

    /// The colour target is about to change.
    pub(super) fn leave_target(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let h = self.render_target;
        self.write_back(mem, h)
    }

    /// The rectangle a draw may write ([`ResolvedDraw::clip`]).
    pub(super) fn draw_clip(&self, mem: &dyn GuestMemory) -> Result<Option<PixelRect>, D3dError> {
        let (w, h) = self.target_size(mem)?;
        let v = self.viewport;
        let mut x1 = v.x.min(w) as i64;
        let mut y1 = v.y.min(h) as i64;
        let mut x2 = (v.x as i64 + v.width as i64).min(w as i64);
        let mut y2 = (v.y as i64 + v.height as i64).min(h as i64);
        // Inclusive scissors: a rectangle covering the target changes
        // nothing; otherwise there is exactly one (`check_raster` stops on
        // more), and the draw is clipped to it.
        if let Some(r) = self
            .scissors
            .first()
            .filter(|_| !self.scissors.iter().any(|r| r.covers(w, h)))
        {
            x1 = x1.max(r.x1 as i64);
            y1 = y1.max(r.y1 as i64);
            x2 = x2.min(r.x2 as i64);
            y2 = y2.min(r.y2 as i64);
        }
        if x1 >= x2 || y1 >= y2 {
            return Ok(None);
        }
        Ok(Some(PixelRect {
            x: x1 as u32,
            y: y1 as u32,
            width: (x2 - x1) as u32,
            height: (y2 - y1) as u32,
        }))
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
        self.touch_target();
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
