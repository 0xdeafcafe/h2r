//! The NV2A register window that device `+0x934` points at.
//!
//! The original CreateDevice stores the NV2A register base (`0xFD000000`)
//! at device `+0x934` (`0x3fc31d`). Game code reads it back after an
//! inlined `KickOff` and reads GPU registers through it: 17 sites, all the
//! same shape (`mov edx, [D3D__pDevice]`, `call 0x3fac30`, `mov eax,
//! [edx+0x934]`, then `0x3240`, `0x3244` and `0x400700`). D3D's own
//! functions read the same registers plus `0x400B10`, but only from code
//! the overrides replace (CreateDevice, `KickOff`, `BlockOnTime`,
//! `MakeRequestedSpace`, a swap callback CreateDevice installs). The
//! ignored test in `tests/round6.rs` finds every site in `$H2_XBE` by
//! decoding it, and checks that the registers it reaches are the ones
//! below.
//!
//! The runtime keeps the real aperture (`0xFD000000..0xFE000000`) free of
//! RAM and has no MMIO handlers: an access there is an unsupported-device
//! fault, by design (`docs/LIFTER.md`). So the window is a documented
//! substitute: the host picks a base ([`DeviceConfig::nv2a_window`]), the
//! two pages holding modelled registers are mapped as ordinary RAM
//! ([`RegisterWindow::pages`]), and the device writes their words at its
//! sync points. A read of any other register page faults (unmapped), which
//! names the register in the fault address.
//!
//! The values follow the device's synchronous model: the push buffer is
//! drained at every D3D entry, so the GPU is idle whenever guest code can
//! look ([`RegisterValues`]). Guest stores into the two pages are found at
//! the next sync, reported as [`crate::Event::RegisterWrite`] and undone.
//! No game site writes them.
//!
//! [`DeviceConfig::nv2a_window`]: crate::DeviceConfig::nv2a_window

use crate::device::GuestRange;
use crate::guest::{GuestError, GuestMemory};

/// Where the original CreateDevice points device `+0x934`.
pub const RETAIL_BASE: u32 = 0xFD00_0000;
/// Size of the NV2A register aperture.
pub const APERTURE: u32 = 0x0100_0000;
/// Page size of the backed pages.
pub const PAGE: u32 = 0x1000;

/// Register offsets from the base.
pub mod reg {
    /// `NV_PFIFO_CACHE1_DMA_PUT`: where the FIFO's DMA puller has been told
    /// to fetch up to.
    pub const PFIFO_CACHE1_DMA_PUT: u32 = 0x3240;
    /// `NV_PFIFO_CACHE1_DMA_GET`: where it has fetched to. Equal to
    /// `DMA_PUT` when the push buffer is drained.
    pub const PFIFO_CACHE1_DMA_GET: u32 = 0x3244;
    /// `NV_PGRAPH_STATUS`: non-zero while the graphics engine is busy.
    pub const PGRAPH_STATUS: u32 = 0x40_0700;
    /// The PGRAPH register D3D's fence word lands in. `D3D_SetFence`
    /// (`0x3fa6a1`) pushes method `0x0310` on subchannel 5 with
    /// `(put << 5) | (time & 0x1F) << 2 | ([device+0x40] & 3)`, where
    /// `put` is the push-buffer address of the fence; `BlockOnTime`
    /// (`0x3fa910`) waits until bits 6:2 match the completion word, and
    /// `MakeRequestedSpace` (`0x3fa8bb`) reads bits 31:7 back as the
    /// address the GPU has fetched to.
    pub const PGRAPH_FENCE: u32 = 0x40_0B10;

    /// Every modelled register.
    pub const ALL: [u32; 4] = [
        PFIFO_CACHE1_DMA_PUT,
        PFIFO_CACHE1_DMA_GET,
        PGRAPH_STATUS,
        PGRAPH_FENCE,
    ];
}

/// Offsets of the backed pages: PFIFO CACHE1 and PGRAPH.
pub const PAGES: [u32; 2] = [0x3000, 0x40_0000];

/// The register values of the synchronous model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterValues {
    /// `DMA_PUT` and `DMA_GET` (always equal: drained).
    pub dma: u32,
    /// `PGRAPH_STATUS` (always 0: idle).
    pub pgraph_status: u32,
    /// The fence word.
    pub fence: u32,
}

impl RegisterValues {
    /// The values for a device whose push buffer has been executed up to
    /// `drained` (a guest address), whose last fence `time` was issued with
    /// the push buffer at `fence_at`, and whose device `+0x40` holds
    /// `counter`.
    ///
    /// `DMA_PUT`/`GET` are in D3D's own encoding of a push-buffer position,
    /// `address & 0x0FFFFFFF` (what `0x3fad07` writes to the channel's put
    /// register), not a DMA offset.
    pub fn synchronous(drained: u32, time: u32, fence_at: u32, counter: u32) -> Self {
        Self {
            dma: drained & 0x0FFF_FFFF,
            pgraph_status: 0,
            fence: fence_word(time, fence_at, counter),
        }
    }

    fn get(&self, offset: u32) -> u32 {
        match offset {
            reg::PFIFO_CACHE1_DMA_PUT | reg::PFIFO_CACHE1_DMA_GET => self.dma,
            reg::PGRAPH_STATUS => self.pgraph_status,
            reg::PGRAPH_FENCE => self.fence,
            _ => 0,
        }
    }
}

/// The word `D3D_SetFence` pushes for fence `time` at push-buffer address
/// `at` (see [`reg::PGRAPH_FENCE`]).
pub fn fence_word(time: u32, at: u32, counter: u32) -> u32 {
    ((at & 0x07FF_FFFC) << 5) | ((time & 0x1F) << 2) | (counter & 3)
}

/// A guest store into the window, found at a sync point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterWrite {
    /// Register offset from the base.
    pub offset: u32,
    /// The word found there.
    pub value: u32,
}

/// The window: its base and the words the device last wrote to it.
#[derive(Clone, Debug)]
pub struct RegisterWindow {
    base: u32,
    shadow: [Vec<u8>; 2],
}

impl RegisterWindow {
    /// A window at `base`, which must be page-aligned and leave room for
    /// the whole aperture below 4 GB. Pages read as zero until the first
    /// [`RegisterWindow::sync`].
    pub fn new(base: u32) -> Result<Self, String> {
        if !base.is_multiple_of(PAGE) || base as u64 + APERTURE as u64 > 1 << 32 {
            return Err(format!(
                "NV2A window base {base:#010x} is not page-aligned below 4 GB - 16 MB"
            ));
        }
        Ok(Self {
            base,
            shadow: [vec![0; PAGE as usize], vec![0; PAGE as usize]],
        })
    }

    /// The base the device stores at `+0x934`.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// The guest ranges to map read/write (see
    /// [`crate::runtime_glue::map_device_memory`]).
    pub fn pages(&self) -> [GuestRange; 2] {
        PAGES.map(|p| GuestRange {
            addr: self.base + p,
            len: PAGE,
        })
    }

    /// Reports guest stores since the last sync (any word of the two pages
    /// that differs from what the device wrote), then writes `values` and
    /// undoes those stores.
    pub fn sync(
        &mut self,
        mem: &mut dyn GuestMemory,
        values: &RegisterValues,
    ) -> Result<Vec<RegisterWrite>, GuestError> {
        let mut writes = Vec::new();
        for (page, shadow) in PAGES.iter().zip(self.shadow.iter_mut()) {
            let addr = self.base + page;
            let now = mem.read_vec(addr, PAGE as usize)?;
            let mut want = shadow.clone();
            for r in reg::ALL.iter().filter(|r| *r & !(PAGE - 1) == *page) {
                let o = (r - page) as usize;
                want[o..o + 4].copy_from_slice(&values.get(*r).to_le_bytes());
            }
            for o in (0..PAGE as usize).step_by(4) {
                let found = &now[o..o + 4];
                if found != &shadow[o..o + 4] {
                    writes.push(RegisterWrite {
                        offset: page + o as u32,
                        value: u32::from_le_bytes(found.try_into().expect("4 bytes")),
                    });
                }
                if found != &want[o..o + 4] {
                    mem.write(addr + o as u32, &want[o..o + 4])?;
                }
            }
            *shadow = want;
        }
        Ok(writes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::SparseMemory;

    fn window() -> (RegisterWindow, SparseMemory) {
        let w = RegisterWindow::new(0x1000_0000).unwrap();
        let mut m = SparseMemory::new();
        for p in w.pages() {
            m.map(p.addr, p.len);
        }
        (w, m)
    }

    #[test]
    fn sync_writes_the_model_words() {
        let (mut w, mut m) = window();
        let v = RegisterValues::synchronous(0x8012_3458, 7, 0x8012_3000, 2);
        assert!(w.sync(&mut m, &v).unwrap().is_empty());
        let r = |o| m.read_u32(0x1000_0000 + o).unwrap();
        assert_eq!(r(reg::PFIFO_CACHE1_DMA_PUT), 0x0012_3458);
        assert_eq!(r(reg::PFIFO_CACHE1_DMA_GET), 0x0012_3458);
        assert_eq!(r(reg::PGRAPH_STATUS), 0);
        let fence = r(reg::PGRAPH_FENCE);
        // BlockOnTime's test: bits 6:2 against completion << 2.
        assert_eq!((fence ^ (7 << 2)) & 0x7C, 0);
        // MakeRequestedSpace's decode of the fetch address.
        assert_eq!((fence >> 5) & 0x07FF_FFFC | 0x8000_0000, 0x8012_3000);
        assert_eq!(fence & 3, 2);
    }

    #[test]
    fn fence_words_keep_five_time_bits_and_27_address_bits() {
        // Time: bits 4:0 of the fence time, at bits 6:2; 32 wraps to 0.
        for (time, field) in [(15, 15), (16, 16), (31, 31), (32, 0), (0x45, 5)] {
            assert_eq!(fence_word(time, 0, 0), field << 2, "time {time}");
        }
        // The tag is the low two bits of device +0x40, every value.
        for counter in 0..8 {
            assert_eq!(fence_word(0, 0, counter), counter & 3);
        }
        // Address: bits 26:2, at bits 31:7. Bit 26 reaches bit 31; bit 27
        // and the low two bits are dropped.
        assert_eq!(fence_word(0, 0x0400_0000, 0), 0x8000_0000);
        assert_eq!(fence_word(0, 0x0800_0003, 0), 0);
        assert_eq!(fence_word(0, 0x8765_4324, 0), 0x0765_4324 << 5);
        assert_eq!(fence_word(31, 0x07FF_FFFC, 3), 0xFFFF_FFFF);
    }

    #[test]
    fn dma_positions_keep_28_address_bits() {
        for (drained, dma) in [
            (0x8800_0000, 0x0800_0000),
            (0x8812_3458, 0x0812_3458),
            (0x0FFF_FFFC, 0x0FFF_FFFC),
            (0xF000_0004, 0x0000_0004),
        ] {
            let v = RegisterValues::synchronous(drained, 0, 0, 0);
            assert_eq!(v.dma, dma, "{drained:#x}");
            assert_eq!(v.get(reg::PFIFO_CACHE1_DMA_PUT), dma);
            assert_eq!(v.get(reg::PFIFO_CACHE1_DMA_GET), dma);
        }
    }

    #[test]
    fn guest_stores_are_reported_and_undone() {
        let (mut w, mut m) = window();
        let v = RegisterValues::synchronous(0x100, 0, 0x100, 0);
        w.sync(&mut m, &v).unwrap();
        m.write_u32(0x1000_0000 + reg::PGRAPH_STATUS, 1).unwrap();
        m.write_u32(0x1000_0000 + 0x3010, 0xABCD).unwrap();
        let writes = w.sync(&mut m, &v).unwrap();
        assert_eq!(
            writes,
            vec![
                RegisterWrite {
                    offset: 0x3010,
                    value: 0xABCD
                },
                RegisterWrite {
                    offset: reg::PGRAPH_STATUS,
                    value: 1
                },
            ]
        );
        assert_eq!(m.read_u32(0x1000_0000 + reg::PGRAPH_STATUS).unwrap(), 0);
        assert_eq!(m.read_u32(0x1000_0000 + 0x3010).unwrap(), 0);
        assert!(w.sync(&mut m, &v).unwrap().is_empty());
    }

    #[test]
    fn bases_must_be_aligned_and_fit() {
        assert!(RegisterWindow::new(0x1000_0001).is_err());
        assert!(RegisterWindow::new(0xFF80_0000).is_err());
        assert!(RegisterWindow::new(RETAIL_BASE).is_ok());
        // Every modelled register lies in a backed page.
        for r in reg::ALL {
            assert!(PAGES.contains(&(r & !(PAGE - 1))), "{r:#x}");
        }
    }
}
