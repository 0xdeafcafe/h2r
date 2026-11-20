//! Optional generated-code alias of guest VA space. All topology changes
//! happen under the Guest mutex and C gate. Rust keeps the physical mapping.
use crate::cow::Ram;
use crate::guest::{FastPage, RAM_SIZE};
use std::ptr::NonNull;

mod protection;

const VA_SIZE: usize = 1usize << 32;
const PAGE: usize = 4096;

pub(crate) fn enabled() -> bool {
    std::env::var_os("H2_FLAT_RAM").is_some_and(|s| s == "1")
}

/// Bit in the existing C page table. Low bits remain guest permissions.
pub(crate) const FLAT: u32 = 8;

/// Host-only, valid under the generated memory gate.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub(crate) struct View {
    pub(crate) base: *mut u8,
    pub(crate) stores: *mut u8,
    pub(crate) plane_mask: u32,
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
    watch: Option<protection::Mirror>,
    watch_attempted: bool,
}

impl Arena {
    pub(crate) fn new(ram: &Ram, pages: &mut [FastPage]) -> Option<Self> {
        let host = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
        if !matches!(host, 4096 | 16384) || !(ram.as_ptr() as usize).is_multiple_of(host) {
            return None;
        }
        let guest = Reservation::new(VA_SIZE + 2 * host)?;
        let arena = Self {
            guest,
            page_size: host,
            watch: None,
            watch_attempted: std::env::var_os("H2_FLAT_WRITE_WATCH").is_some_and(|v| v == "0"),
        };
        for page in pages.iter_mut() {
            page.permissions &= !FLAT;
        }
        arena.map_ranges(ram, pages, 0, VA_SIZE);
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
        let lo = start as usize & !(self.page_size - 1);
        let hi = (end as usize).next_multiple_of(self.page_size);
        let at = self.guest.ptr.as_ptr().wrapping_add(self.page_size + lo);
        // Restore the original guard reservation (including stale aliases).
        // On failure the caller drops the entire arena before republishing.
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
        for page in &mut pages[lo / PAGE..hi / PAGE] {
            page.permissions &= !FLAT;
        }
        self.map_ranges(ram, pages, lo, hi);
        true
    }

    fn map_ranges(&self, ram: &Ram, pages: &mut [FastPage], start: usize, end: usize) {
        let host = self.page_size;
        // The permissions in the 4 KiB table still guard every access. A
        // 16 KiB VA group can therefore use a shared mapping even if only
        // some of its subpages agree with that mapping. Coalesce consecutive
        // physical host pages into one VM operation, including across Guest
        // Mapping boundaries.
        let backing = |pages: &[FastPage], va: usize| {
            pages[va / PAGE..(va + host) / PAGE]
                .iter()
                .enumerate()
                .find_map(|(i, p)| {
                    let pa = p.physical as usize;
                    (p.permissions & 3 != 0 && pa % host == i * PAGE).then_some(pa & !(host - 1))
                })
        };
        let mut va = start;
        while va < end {
            let Some(physical) = backing(pages, va) else {
                va += host;
                continue;
            };
            let lo = va;
            va += host;
            while va < end && backing(pages, va) == Some(physical + (va - lo)) {
                va += host;
            }
            let hi = va;
            let address = self.guest.ptr.as_ptr().wrapping_add(host + lo);
            // SAFETY: aliases whole host pages inside our own reservation.
            if !unsafe { ram.alias(address, physical, hi - lo) } {
                continue;
            }
            for (i, page) in pages[lo / PAGE..hi / PAGE].iter_mut().enumerate() {
                if page.permissions & 3 != 0 && page.physical as usize == physical + i * PAGE {
                    page.permissions |= FLAT;
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
                .map_or(std::ptr::null_mut(), |w| w.base()),
            plane_mask: (self.page_size / PAGE - 1) as u32,
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
