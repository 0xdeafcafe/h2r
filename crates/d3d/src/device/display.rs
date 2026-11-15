//! Display-side entries and the vertical-blank callback.
//!
//! * `0x3f8c70` (flicker filter), `0x3f8cc0` (the 32-word block at
//!   `0x1480`), `PersistDisplay` (`0x3f8ad0`) and the resource busy check
//!   `0x3fb710`: their guest-visible effects, without the AV-encoder kernel
//!   calls (there is no TV encoder; the value is kept on the device).
//! * The vertical blank: Halo stores a callback at device `+0x1DB8`
//!   (`0x12b093`, the inlined `SetVerticalBlankCallback`). On the Xbox the
//!   GPU interrupt calls it once per vblank with a `D3DVBLANKDATA`
//!   `{VBlank, Swap, Flags}`; Halo's (`0x12b2a0` -> `0x14280`) counts them in
//!   the 64-bit `0x485AB0`, which its frame wait `0x12b2e0` spins on. The
//!   device counts vblanks and builds the record ([`Device::vblank`]); the
//!   runtime glue ([`crate::vblank`]) calls the guest function.

use super::*;

/// `D3DVBLANK_SWAPDONE`: a swap completed since the previous vblank.
pub const VBLANK_SWAPDONE: u32 = 1;

/// One vertical blank to deliver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VBlank {
    /// Guest callback (`+0x1DB8`).
    pub callback: u32,
    /// `D3DVBLANKDATA`: `{VBlank count, Swap count, Flags}`.
    pub record: [u32; 3],
}

/// Display state the device keeps.
#[derive(Clone, Debug)]
pub(crate) struct Display<S> {
    /// Last flicker-filter value sent (`0x3f8c70`).
    pub flicker: Option<u32>,
    /// Surface `PersistDisplay` kept for the next title.
    pub persisted: Option<S>,
    /// Vertical blanks counted.
    pub vblanks: u32,
    /// A swap happened since the last vblank.
    pub swapped: bool,
    /// Vertical blanks each `Swap` produces ([`Device::set_vblanks_per_swap`]).
    pub per_swap: u32,
    /// Vertical blanks counted at a `Swap` and not yet delivered.
    pub pending: Vec<VBlank>,
}

impl<S> Default for Display<S> {
    fn default() -> Self {
        Self {
            flicker: None,
            persisted: None,
            vblanks: 0,
            swapped: false,
            per_swap: 0,
            pending: Vec::new(),
        }
    }
}

impl<B: Backend> Device<B> {
    /// `0x3f8c70` (`ESI` = filter): unless the filter was already sent with
    /// the same on/off value (`[0x40863C]` set and `[0x408630] == (filter !=
    /// 0)`), the original sends `AvSendTVEncoderOption(+0x1C28, 14, filter,
    /// 0)` and stores `[0x408630] = filter != 0`, `[0x40863C] = 1`. Here the
    /// globals change the same way and the value is kept
    /// ([`Device::flicker_filter`]); no encoder exists to program.
    pub fn set_flicker_filter(
        &mut self,
        mem: &mut dyn GuestMemory,
        filter: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3D display/flicker filter (0x3f8c70)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let (value, sent) = self.cfg.layout.flicker_filter;
        let on = (filter != 0) as u32;
        if mem.read_u32(sent)? != 0 && mem.read_u32(value)? == on {
            return Ok(());
        }
        self.display.flicker = Some(filter);
        mem.write_u32(sent, 1)?;
        mem.write_u32(value, on)?;
        Ok(())
    }

    /// The flicker filter last sent, if any.
    pub fn flicker_filter(&self) -> Option<u32> {
        self.display.flicker
    }

    /// `0x3f8cc0` (stdcall `pBlock`, `ret 4`): copies 32 dwords to device
    /// `+0x197C` unless device flag `0x10`, and pushes them as method
    /// `0x1480` (`0x801480`, 33 words).
    pub fn set_ramp_block(
        &mut self,
        mem: &mut dyn GuestMemory,
        block: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3D gamma/display ramp (0x3f8cc0)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let dev = self.cfg.layout.device;
        let put = self.reserve(mem)?;
        let words = mem.read_words(block, 32)?;
        if mem.read_u32(dev + field::FLAGS)? & 0x10 == 0 {
            Self::write_words(mem, dev + field::RAMP, &words)?;
        }
        let mut out = vec![0x0080_0000 | crate::hw::method::BLOCK_1480];
        out.extend_from_slice(&words);
        let pb = self.cfg.push_buffer;
        if put < pb.addr || put as u64 + 4 * out.len() as u64 > pb.addr as u64 + pb.len as u64 {
            return Err(D3dError::invalid(
                E,
                format!("put {put:#010x} outside the push buffer"),
            ));
        }
        Self::write_words(mem, put, &out)?;
        mem.write_u32(dev + field::PUT, put + 4 * out.len() as u32)?;
        Ok(())
    }

    /// `D3DDevice_PersistDisplay` (`0x3f8ad0`): keeps the displayed surface
    /// for the next title. Returns `E_FAIL` (`0x80004005`) with no device
    /// reference (`+0x938` = 0, `0x3f8afd`); otherwise finishes queued work,
    /// clears `[0x408640]` and `[0x40863C]` (`0x3f8bf2`/`0x3f8bf7`, so the
    /// flicker filter is sent again) and returns 0. The original copies the
    /// front buffer into persistent contiguous memory and registers it with
    /// `AvSetSavedDataAddress`; here the host front surface is kept
    /// ([`Device::persisted_surface`]).
    pub fn persist_display(&mut self, mem: &mut dyn GuestMemory) -> Result<u32, D3dError> {
        const E: &str = "D3DDevice_PersistDisplay";
        let dev = self.cfg.layout.device;
        if !self.created || mem.read_u32(dev + field::REF_COUNT)? == 0 {
            return Ok(0x8000_4005);
        }
        self.sync_push_buffer(mem)?;
        if self.immediate.is_open() {
            return Err(D3dError::invalid(E, "PersistDisplay inside Begin/End"));
        }
        self.flush_render_targets(mem)?;
        self.display.persisted = self.swap.as_ref().map(|s| s.front);
        mem.write_u32(self.cfg.layout.persist_word, 0)?;
        mem.write_u32(self.cfg.layout.flicker_filter.1, 0)?;
        self.display.flicker = None;
        Ok(0)
    }

    /// The surface `PersistDisplay` kept.
    pub fn persisted_surface(&self) -> Option<B::Surface> {
        self.display.persisted
    }

    /// The busy check `0x3fb710` (`ECX` = resource; returns `EAX`). Busy (1)
    /// while the resource, or for a surface its parent texture, holds an
    /// internal (binding) reference (`Common & 0x780000`); otherwise the GPU
    /// time test `GPU_TIME - Lock >= GPU_TIME - completed`
    /// (`0x3fb770..0x3fb782`). The host GPU is synchronous, so every fence has
    /// passed: not busy, and like the original the `Lock` word of the
    /// resource tested (the parent for a surface level) is cleared
    /// (`0x3fb78d`). Render-to-texture targets are written back first, since
    /// the caller is about to touch the texels.
    pub fn resource_busy(
        &mut self,
        mem: &mut dyn GuestMemory,
        resource: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "D3D surface helper (0x3fb710)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        self.flush_render_targets(mem)?;
        let refs = common::INTREFCOUNT_MASK;
        let is_surface = |c: u32| c & common::TYPE_MASK == 0x5_0000;
        let mut r = resource;
        let c = mem.read_u32(r)?;
        if is_surface(c) {
            let parent = mem.read_u32(at(r, 0x14)?)?;
            if parent != 0 {
                if c & refs != 0 || mem.read_u32(parent)? & refs != 0 {
                    return Ok(1);
                }
                r = parent;
            }
        }
        let c = mem.read_u32(r)?;
        if c & refs != 0 {
            return Ok(1);
        }
        if is_surface(c) {
            let parent = mem.read_u32(at(r, 0x14)?)?;
            if parent != 0 && mem.read_u32(parent)? & refs != 0 {
                return Ok(1);
            }
        }
        mem.write_u32(at(r, 8)?, 0)?;
        Ok(0)
    }

    /// How many vertical blanks each `Swap` produces (default 0: none).
    /// They are counted at the end of `Swap` ([`Device::vblank`]) and queued
    /// for the runtime glue, which calls the guest callback for each when the
    /// `Swap` override returns ([`crate::vblank`]).
    pub fn set_vblanks_per_swap(&mut self, n: u32) {
        self.display.per_swap = n;
    }

    /// The vertical blanks queued by `Swap`, oldest first.
    pub fn take_pending_vblanks(&mut self) -> Vec<VBlank> {
        std::mem::take(&mut self.display.pending)
    }

    /// Called at the end of `Swap`: counts [`Device::set_vblanks_per_swap`]
    /// vblanks and queues those with a callback to deliver.
    pub(super) fn vblanks_after_swap(&mut self, mem: &dyn GuestMemory) -> Result<(), D3dError> {
        for _ in 0..self.display.per_swap {
            if let Some(v) = self.vblank(mem)? {
                self.display.pending.push(v);
            }
        }
        Ok(())
    }

    /// Counts one vertical blank and returns what to deliver: the callback
    /// at `+0x1DB8` with `{VBlank, Swap, Flags}` (`Flags` =
    /// [`VBLANK_SWAPDONE`] for the first vblank after a `Swap`), or `None`
    /// when no callback is registered (the count still advances).
    pub fn vblank(&mut self, mem: &dyn GuestMemory) -> Result<Option<VBlank>, D3dError> {
        self.display.vblanks = self.display.vblanks.wrapping_add(1);
        self.stats.vblanks += 1;
        let flags = if std::mem::take(&mut self.display.swapped) {
            VBLANK_SWAPDONE
        } else {
            0
        };
        if !self.created {
            return Ok(None);
        }
        let callback = mem.read_u32(self.cfg.layout.device + field::VBLANK_CALLBACK)?;
        Ok((callback != 0).then_some(VBlank {
            callback,
            record: [self.display.vblanks, self.swap_count, flags],
        }))
    }
}
