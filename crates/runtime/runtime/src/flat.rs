//! Optional generated-code alias of guest VA space. All topology changes
//! happen under the Guest mutex and C gate. Rust keeps the physical mapping.
use crate::cow::Ram;
use crate::guest::{FastPage, RAM_SIZE};
use std::ptr::NonNull;

mod protection;

const VA_SIZE: usize = 1usize << 32;
const PAGE: usize = 4096;

pub(crate) fn enabled() -> bool {
    protection::enable_stats();
    std::env::var_os("H2_FLAT_RAM").is_some_and(|s| s == "1")
}

/// Bit in the existing C page table. Low bits remain guest permissions.
pub(crate) const FLAT: u32 = 8;
const WINDOW_BITS: u32 = FLAT | 0x30;

// Old images know only FLAT, which must always mean base + guest_va.
fn window_flag(index: usize) -> u32 {
    if index == 0 {
        FLAT
    } else {
        (index as u32) << 4
    }
}

/// Host-only, valid under the generated memory gate.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub(crate) struct View {
    pub(crate) base: *mut u8,
    pub(crate) stores: *mut u8,
    pub(crate) plane_mask: u32,
    // Appended after the original descriptor. Visible only with bit 30.
    pub(crate) cold_stores: *mut u8,
    pub(crate) software_pages: *const u64,
}

struct Reservation {
    ptr: NonNull<u8>,
    len: usize,
}
impl Reservation {
    fn new(len: usize) -> Option<Self> {
        // SAFETY: anonymous reservation. The OS chooses a free range.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return None;
        }
        if let Some(ptr) = NonNull::new(p.cast()) {
            return Some(Self { ptr, len });
        }
        unsafe { libc::munmap(p, len) };
        None
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        // SAFETY: also removes every alias installed inside the reservation.
        unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}
// SAFETY: reservation ownership can move. All ordinary access is gated.
unsafe impl Send for Reservation {}
unsafe impl Sync for Reservation {}

pub(crate) struct Arena {
    // Reservation ownership includes one unmapped host page at either end.
    guest: Reservation,
    page_size: usize,
    windows: usize,
    watch: Option<protection::Mirror>,
    watch_attempted: bool,
}

impl Arena {
    pub(crate) fn new(ram: &Ram, pages: &mut [FastPage]) -> Option<Self> {
        if !ram.can_alias() {
            return None;
        }
        let host = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
        if !matches!(host, 4096 | 16384) || !(ram.as_ptr() as usize).is_multiple_of(host) {
            return None;
        }
        // On 16 KiB hosts, VA and PA may differ by any of four 4 KiB
        // offsets. Reserve address space only; all windows share the RAM.
        let windows = host / PAGE;
        let stride = VA_SIZE + 3 * host;
        let (guest, windows) = if windows > 1 {
            Reservation::new(windows * stride).map(|r| (r, windows))
        } else {
            None
        }
        .or_else(|| Reservation::new(VA_SIZE + 2 * host).map(|r| (r, 1)))?;
        let arena = Self {
            guest,
            page_size: host,
            windows,
            watch: None,
            watch_attempted: std::env::var_os("H2_FLAT_WRITE_WATCH").is_some_and(|v| v == "0"),
        };
        for page in pages.iter_mut() {
            page.permissions &= !WINDOW_BITS;
        }
        for index in 0..windows {
            arena.map_ranges(ram, pages, index, 0, VA_SIZE);
        }
        Some(arena)
    }

    /// A change to any 4 KiB guest page can invalidate a whole host alias.
    pub(crate) fn refresh(
        &mut self,
        ram: &Ram,
        pages: &mut [FastPage],
        start: u64,
        end: u64,
    ) -> bool {
        for index in 0..self.windows {
            let (lo, hi) = self.host_range(index, start as usize, end as usize);
            let at = self.window_address(index, lo);
            // Replace every old alias in the affected host pages with guards.
            // This also covers siblings outside the changed guest range.
            if unsafe {
                libc::mmap(
                    at.cast(),
                    hi - lo,
                    libc::PROT_NONE,
                    libc::MAP_PRIVATE | libc::MAP_FIXED | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            } == libc::MAP_FAILED
            {
                return false;
            }
            let range = self.guest_pages(index, lo, hi);
            for page in &mut pages[range] {
                if page.permissions & WINDOW_BITS == window_flag(index) {
                    page.permissions &= !WINDOW_BITS;
                }
            }
            self.map_ranges(ram, pages, index, start as usize, end as usize);
        }
        true
    }

    // Window i holds the same guest addresses shifted by i guest pages.
    // Each stride leaves room for the leading guard, shifted last page,
    // and a trailing guard. Keep in sync with h2_flat_span in h2fastmem.h.
    fn window_address(&self, index: usize, offset: usize) -> *mut u8 {
        let stride = VA_SIZE + 3 * self.page_size;
        self.guest
            .ptr
            .as_ptr()
            .wrapping_add(self.page_size + index * stride + offset)
    }

    fn host_range(&self, index: usize, start: usize, end: usize) -> (usize, usize) {
        let shift = index * PAGE;
        (
            (start + shift) & !(self.page_size - 1),
            (end + shift).next_multiple_of(self.page_size),
        )
    }

    fn guest_pages(&self, index: usize, lo: usize, hi: usize) -> std::ops::Range<usize> {
        let shift = index * PAGE;
        lo.saturating_sub(shift) / PAGE..hi.saturating_sub(shift).min(VA_SIZE) / PAGE
    }

    fn map_ranges(
        &self,
        ram: &Ram,
        pages: &mut [FastPage],
        index: usize,
        start: usize,
        end: usize,
    ) {
        let host = self.page_size;
        let shift = index * PAGE;
        let (mut pos, end) = self.host_range(index, start, end);
        let backing = |pages: &[FastPage], pos: usize| {
            self.guest_pages(index, pos, pos + host).find_map(|vpn| {
                let p = pages[vpn];
                let pa = p.physical as usize;
                let slot = (vpn * PAGE + shift) & (host - 1);
                (p.permissions & 3 != 0 && pa & (host - 1) == slot).then_some(pa & !(host - 1))
            })
        };
        // Each virtual page is congruent in just one window. Coalesce
        // consecutive physical host pages into one shared mapping. A split
        // group can have candidates in different physical host pages; only
        // certify bytes actually represented by the chosen mapping.
        while pos < end {
            let Some(physical) = backing(pages, pos) else {
                pos += host;
                continue;
            };
            let lo = pos;
            pos += host;
            while pos < end && backing(pages, pos) == Some(physical + (pos - lo)) {
                pos += host;
            }
            let at = self.window_address(index, lo);
            // SAFETY: this host-page-aligned range is inside our reservation.
            if !unsafe { ram.alias(at, physical, pos - lo) } {
                continue;
            }
            for vpn in self.guest_pages(index, lo, pos) {
                let page = &mut pages[vpn];
                if page.permissions & 3 != 0
                    && page.physical as usize == physical + (vpn * PAGE + shift - lo)
                {
                    page.permissions = (page.permissions & !WINDOW_BITS) | window_flag(index);
                }
            }
        }
    }

    pub(crate) fn view(&mut self, ram: &Ram, watch: &crate::watch::WriteWatch) -> View {
        if watch.active() && !self.watch_attempted {
            self.watch_attempted = true;
            self.watch = protection::Mirror::new(ram, self.page_size, watch);
        }
        View {
            base: self.guest.ptr.as_ptr().wrapping_add(self.page_size),
            stores: self
                .watch
                .as_ref()
                .filter(|w| !w.has_hot_pages())
                .map_or(std::ptr::null_mut(), |w| w.base()),
            plane_mask: (self.page_size / PAGE - 1) as u32,
            cold_stores: self
                .watch
                .as_ref()
                .map_or(std::ptr::null_mut(), |w| w.base()),
            software_pages: self
                .watch
                .as_ref()
                .map_or(std::ptr::null(), |w| w.software_pages()),
        }
    }
    pub(crate) fn publish(&mut self, watch: &mut crate::watch::WriteWatch) {
        if let Some(mirror) = &mut self.watch {
            mirror.publish(watch);
        }
    }
    pub(crate) fn rearm(&mut self, watch: &crate::watch::WriteWatch) {
        if self.watch.as_mut().is_some_and(|m| !m.rearm(watch)) {
            self.watch = None;
        }
    }
}
