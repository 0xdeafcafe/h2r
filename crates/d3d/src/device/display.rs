//! Display-side entries and the vertical-blank callback.
//!
//! * `0x3f8c70` (flicker filter), `SetStipple` (`0x3f8cc0`, the polygon
//!   stipple pattern at method `0x1480`), `PersistDisplay` (`0x3f8ad0`) and
//!   the resource busy check `0x3fb710`: their guest-visible effects,
//!   without the AV-encoder kernel calls (there is no TV encoder; the value
//!   is kept on the device).
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

/// Offset of the miniport object in the device (`0x12cb4`: `lea eax,
/// [edx+1C28h]`); its first dword is the NV2A register base.
pub const MINIPORT: u32 = 0x1C28;

/// Bytes of a `D3DGAMMARAMP`: 256 red, 256 green, then 256 blue entries.
pub const GAMMA_RAMP_BYTES: usize = 3 * 256;

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
    /// Gamma ramp last loaded (`0x3ff5c6`): 256 red, 256 green, 256 blue.
    pub gamma: Option<Box<[u8; GAMMA_RAMP_BYTES]>>,
    /// Surface `PersistDisplay` kept for the next title.
    pub persisted: Option<S>,
    /// Vertical blanks counted.
    pub vblanks: u32,
    /// A swap happened since the last vblank.
    pub swapped: bool,
    /// The image last presented was drawn over and could not be kept (no
    /// [`Backend::copy_surface`]); cleared by the next `Swap`.
    pub shown_lost: bool,
    /// Vertical blanks each `Swap` produces ([`Device::set_vblanks_per_swap`]).
    pub per_swap: u32,
    /// Vertical blanks counted at a `Swap` and not yet delivered.
    pub pending: Vec<VBlank>,
}

impl<S> Default for Display<S> {
    fn default() -> Self {
        Self {
            flicker: None,
            gamma: None,
            persisted: None,
            vblanks: 0,
            swapped: false,
            shown_lost: false,
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

    /// `0x3ff5c6`, the miniport's gamma-ramp load (`EAX` = miniport, device
    /// `+0x1C28`; `EDX` = a `D3DGAMMARAMP`; `ret`). Halo's inlined
    /// `SetGammaRamp` (`0x12c71..0x12d08`, in the brightness update) builds
    /// the ramp from the brightness setting, and when it differs from the
    /// current one copies it into the miniport (`+0x1DC + 0x300 * n`,
    /// selecting `n` at `+0x7E4`) and calls this. The original writes the
    /// 768 entries to the DAC palette (PRAMDAC `0x6813C8`/`0x6813C9`
    /// through the register base at `[EAX]`), red, green and blue per
    /// index. The ramp is kept ([`Device::gamma_ramp`]) and passed to
    /// the backend on each present.
    pub fn load_gamma_ramp(
        &mut self,
        mem: &mut dyn GuestMemory,
        miniport: u32,
        ramp: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3D miniport gamma ramp (0x3ff5c6)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let expected = self.cfg.layout.device + MINIPORT;
        if miniport != expected {
            return Err(D3dError::invalid(
                E,
                format!("miniport {miniport:#010x} is not device + 0x1C28 ({expected:#010x})"),
            ));
        }
        let mut bytes = Box::new([0u8; GAMMA_RAMP_BYTES]);
        mem.read(ramp, &mut bytes[..])?;
        self.display.gamma = Some(bytes);
        Ok(())
    }

    /// The gamma ramp last loaded (`0x3ff5c6`): `[red; 256]`, `[green;
    /// 256]`, `[blue; 256]`, each entry the output for that input level.
    pub fn gamma_ramp(&self) -> Option<&[u8; GAMMA_RAMP_BYTES]> {
        self.display.gamma.as_deref()
    }

    /// `D3DDevice_SetStipple` (`0x3f8cc0`, stdcall `pPattern`, `ret 4`):
    /// copies 32 dwords to device `+0x197C` unless device flag `0x10`, and
    /// pushes them as `NV097_SET_STIPPLE_PATTERN` (`0x801480`, 33 words).
    /// The pattern lands in [`crate::hw::HwState::stipple_pattern`] when the
    /// push buffer drains ([`Device::stipple_pattern`]).
    ///
    /// This is the polygon stipple (a 32x32 screen-door mask applied to
    /// rasterised pixels while `D3DRS_STIPPLEENABLE` is set), not a gamma
    /// ramp: all nine of Halo's calls (`0x2dd86`, `0x2df56`, `0x3531c`, ...)
    /// pass one of 17 dither patterns from `0x487288`, chosen by `0x1c290`
    /// from an opacity.
    pub fn set_stipple(&mut self, mem: &mut dyn GuestMemory, pattern: u32) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_SetStipple";
        let block = pattern;
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        let dev = self.cfg.layout.device;
        let put = self.reserve(mem)?;
        let words = mem.read_words(block, 32)?;
        if mem.read_u32(dev + field::FLAGS)? & 0x10 == 0 {
            Self::write_words(mem, dev + field::STIPPLE, &words)?;
        }
        let mut out = vec![0x0080_0000 | crate::hw::method::STIPPLE_PATTERN];
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
    /// `AvSetSavedDataAddress`; here the image last presented is copied
    /// into a surface of its own ([`Device::persisted_surface`]). A backend
    /// without [`Backend::copy_surface`] stops.
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
        self.flush_render_targets_for(mem, WritebackCause::PersistDisplay)?;
        // An independent copy of what is on screen: the swap ring goes with
        // the device at teardown, the persisted image must not.
        if self.display.shown_lost {
            return Err(D3dError::unimplemented(
                E,
                "the image last presented was drawn over: the backend cannot copy surfaces to keep it",
            ));
        }
        let sw = self.swap.as_ref().expect("created");
        let (src, format, (w, h)) = (sw.shown, sw.format, sw.size);
        let copy = self
            .backend
            .create_surface(format, w, h, false)
            .map_err(D3dError::Backend)?;
        if !self
            .backend
            .copy_surface(src, copy)
            .map_err(D3dError::Backend)?
        {
            self.backend
                .destroy_surface(copy)
                .map_err(D3dError::Backend)?;
            return Err(D3dError::unimplemented(
                E,
                "the backend cannot copy surfaces, so the displayed image cannot be kept",
            ));
        }
        if let Some(old) = self.display.persisted.replace(copy) {
            self.backend
                .destroy_surface(old)
                .map_err(D3dError::Backend)?;
        }
        mem.write_u32(self.cfg.layout.persist_word, 0)?;
        mem.write_u32(self.cfg.layout.flicker_filter.1, 0)?;
        self.display.flicker = None;
        Ok(0)
    }

    /// The surface `PersistDisplay` kept: a copy of the image last
    /// presented, owned by the device until the next `PersistDisplay` and
    /// kept through teardown (it is the next title's boot image).
    pub fn persisted_surface(&self) -> Option<B::Surface> {
        self.display.persisted
    }

    /// The polygon stipple pattern the push buffer last set
    /// (`SET_STIPPLE_PATTERN`, from [`Device::set_stipple`] or inline game
    /// code). Not applied yet: a backend that wants it reads it here or from
    /// [`crate::backend::ResolvedDraw::hw`], and masks pixels while
    /// `D3DRS_STIPPLEENABLE` is set.
    pub fn stipple_pattern(&self) -> &[u32; 32] {
        &self.hw.stipple_pattern
    }

    /// `D3DResource_IsBusy`, inlined as `0x3fb710` (`ECX` = resource;
    /// returns `EAX` = 0 or 1; `ret`). Called before the CPU touches a
    /// resource, e.g. from `0x1db90`/`0x1de50` while render resources are
    /// set up. As compiled:
    ///
    /// 1. The lock bits. Busy while the resource holds an internal (binding)
    ///    reference, `Common & 0x780000`. For a surface (`Common & 0x70000
    ///    == 0x50000`) with a parent texture (`+0x14`), busy if either holds
    ///    one, and the parent is tested from here on (`0x3fb729..0x3fb743`);
    ///    a surface whose own parent holds one is busy too (`0x3fb74e`).
    /// 2. The fence. `Lock` (`+8`) is the GPU time the resource was last
    ///    used at; 0 means never. With `GPU_TIME = [device + 0x2C]` and
    ///    `completed = [[device + 0x30]]`, busy while `GPU_TIME - Lock <
    ///    GPU_TIME - completed` (unsigned, `0x3fb770..0x3fb782`), i.e. the
    ///    use is newer than the last completed fence.
    /// 3. Otherwise not busy, and `Lock` of the resource tested (the parent,
    ///    for a surface level) is cleared (`0x3fb78d`).
    ///
    /// The fence test runs against this device's GPU model: the queued push
    /// buffer is executed first, and the device executes everything
    /// synchronously, so the completion time is the last fence handed out
    /// ([`Device::gpu_completed`]). A use stamped no later than that has
    /// passed; only a `Lock` newer than every fence the device issued
    /// would be busy, and the device never stamps one. Render-to-texture
    /// targets are written back first, since the caller is about to touch
    /// the texels: lazily only those of `resource`.
    pub fn resource_busy(
        &mut self,
        mem: &mut dyn GuestMemory,
        resource: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "D3DResource_IsBusy (0x3fb710)";
        self.live(E)?;
        self.sync_push_buffer(mem)?;
        if self.rtt_lazy() {
            self.flush_for_texture(mem, resource, WritebackCause::Busy)?;
        } else {
            self.flush_render_targets_for(mem, WritebackCause::Busy)?;
        }
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
        let lock = mem.read_u32(at(r, 8)?)?;
        if lock != 0 {
            let gpu_time = mem.read_u32(self.cfg.layout.device + field::GPU_TIME)?;
            let completed = self.gpu_completed();
            if gpu_time.wrapping_sub(lock) < gpu_time.wrapping_sub(completed) {
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

    /// Counts `n` vertical blanks that passed without a delivery of their
    /// own (a clocked source dropped them from a debt too old to deliver,
    /// [`crate::vblank::clocked_vblanks`]): `VBlank` in the next record
    /// still counts them, as the hardware counter would.
    pub fn skip_vblanks(&mut self, n: u32) {
        self.display.vblanks = self.display.vblanks.wrapping_add(n);
        self.stats.vblanks += n as u64;
    }

    /// The vertical-blank callback registered at `+0x1DB8`, if the device
    /// exists and one is set. Counts nothing.
    pub fn vblank_callback(&self, mem: &dyn GuestMemory) -> Result<Option<u32>, D3dError> {
        if !self.created {
            return Ok(None);
        }
        let callback = mem.read_u32(self.cfg.layout.device + field::VBLANK_CALLBACK)?;
        Ok((callback != 0).then_some(callback))
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
        Ok(self.vblank_callback(mem)?.map(|callback| VBlank {
            callback,
            record: [self.display.vblanks, self.swap_count, flags],
        }))
    }
}
