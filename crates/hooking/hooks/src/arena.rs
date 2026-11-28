//! The one reserved range of guest memory that features and the installer's
//! shadow entries are placed in, so private guest pages stop being
//! hand-picked addresses in comments.
//!
//! The range is inside the kernel's HLE reservation
//! (`kernel::memory::HLE_RESERVED`, `0xa0000000..0xa1000000`), which no
//! guest allocator places anything in. The Bink and DirectSound HLE search
//! that reservation upward from its start for unmapped pages, so a range
//! mapped at boot near its top is skipped by them.
//!
//! Allocation is byte-granular below a page: a request smaller than
//! [`PAGE`] is packed, [`ALIGN`]-aligned, into a page shared with other
//! small requests of the same [`Permissions`], so every page has one
//! protection and is mapped once ([`mapping`]). Return shadows (the
//! addresses a chain calls its body through, `crate::install`) are packed
//! [`SHADOW_STRIDE`] apart on pages of their own, so a late observer plan
//! can take free slots on a boot shadow's page
//! (`crate::Plan::attach_observers`). A request of a page or more gets
//! whole pages of its own.

use runtime::guest::Permissions;

/// Start of the feature arena, `0xa0c8_1000`.
pub const ARENA_START: u32 = 0xa0c8_1000;
/// End (exclusive): 3388 KiB, up to the community variants' fixed page at
/// `0xa0fd_0000`.
pub const ARENA_END: u32 = 0xa0fd_0000;
/// Bytes in the arena.
pub const ARENA_SIZE: u32 = ARENA_END - ARENA_START;
/// The mapping granule.
pub const PAGE: u32 = 0x1000;
/// Alignment of a request smaller than a page.
pub const ALIGN: u32 = 0x10;
/// Bytes between two return shadows on a shadow page.
pub const SHADOW_STRIDE: u32 = 4;

/// What a shared page holds: one protection's small requests, or return
/// shadows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Data(Permissions),
    Shadow,
}

/// A shared page with room left.
#[derive(Clone, Debug)]
struct Shared {
    group: Group,
    page: u32,
    used: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// The simulation's allocations, from [`ARENA_START`] up.
    Low,
    /// Presentation-only allocations, from [`ARENA_END`] down, so they leave
    /// the low end's addresses unchanged.
    High,
}

/// A two-ended bump allocator over [`ARENA_START`]..[`ARENA_END`]: whole
/// pages for a page or more, packed shared pages below that (see the module
/// docs). Allocation is pure (an address is handed out); mapping happens at
/// install.
#[derive(Clone, Debug)]
pub struct Arena {
    next: u32,
    end: u32,
    low: Vec<Shared>,
    high: Vec<Shared>,
}

impl Default for Arena {
    fn default() -> Self {
        Arena {
            next: ARENA_START,
            end: ARENA_END,
            low: Vec::new(),
            high: Vec::new(),
        }
    }
}

impl Arena {
    /// `len` bytes with protection `perms` from the low end, or `None` when
    /// full.
    pub fn alloc(&mut self, len: u32, perms: Permissions) -> Option<u32> {
        self.place(End::Low, len, Group::Data(perms))
    }
    /// `len` bytes from the high end, without moving the simulation's
    /// low-end allocations.
    pub fn alloc_high(&mut self, len: u32, perms: Permissions) -> Option<u32> {
        self.place(End::High, len, Group::Data(perms))
    }
    /// A return shadow (executable), from the high end when `high`.
    pub fn alloc_shadow(&mut self, high: bool) -> Option<u32> {
        let end = if high { End::High } else { End::Low };
        self.place(end, SHADOW_STRIDE, Group::Shadow)
    }

    fn place(&mut self, end: End, len: u32, group: Group) -> Option<u32> {
        let len = len.max(1);
        if len >= PAGE {
            return self.pages(end, len);
        }
        let align = match group {
            Group::Data(_) => ALIGN,
            Group::Shadow => SHADOW_STRIDE,
        };
        let size = len.next_multiple_of(align);
        let shared = match end {
            End::Low => &mut self.low,
            End::High => &mut self.high,
        };
        // First fit over the end's shared pages of this group.
        if let Some(s) = shared
            .iter_mut()
            .find(|s| s.group == group && PAGE - s.used >= size)
        {
            let at = s.page + s.used;
            s.used += size;
            return Some(at);
        }
        let page = self.pages(end, PAGE)?;
        let shared = match end {
            End::Low => &mut self.low,
            End::High => &mut self.high,
        };
        shared.push(Shared {
            group,
            page,
            used: size,
        });
        Some(page)
    }

    /// `len` rounded up to whole pages from `end`.
    fn pages(&mut self, end: End, len: u32) -> Option<u32> {
        let len = len.checked_add(PAGE - 1)? & !(PAGE - 1);
        match end {
            End::Low => {
                let at = self.next;
                let top = at.checked_add(len)?;
                if top > self.end {
                    return None;
                }
                self.next = top;
                Some(at)
            }
            End::High => {
                let at = self.end.checked_sub(len)?;
                if at < self.next {
                    return None;
                }
                self.end = at;
                Some(at)
            }
        }
    }

    /// Bytes of pages handed out from both ends.
    pub fn used(&self) -> u32 {
        self.next - ARENA_START + (ARENA_END - self.end)
    }
    /// Bytes of whole pages left between the two ends.
    pub fn free(&self) -> u32 {
        self.end - self.next
    }
}

/// The pages an allocation at `addr` of `len` bytes lives on, as
/// `(start, len)`: its own whole pages for a page or more, the shared page
/// holding it otherwise. Install maps each distinct range once.
pub fn mapping(addr: u32, len: u32) -> (u32, u32) {
    if len >= PAGE {
        (addr, len.next_multiple_of(PAGE))
    } else {
        (addr & !(PAGE - 1), PAGE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RW: Permissions = Permissions::RW;
    const RX: Permissions = Permissions::RX;

    #[test]
    fn small_requests_share_a_page_aligned() {
        let mut a = Arena::default();
        assert_eq!(a.alloc(8, RW), Some(ARENA_START));
        assert_eq!(a.alloc(12, RW), Some(ARENA_START + 0x10));
        assert_eq!(a.alloc(0x11, RW), Some(ARENA_START + 0x20));
        assert_eq!(a.alloc(0, RW), Some(ARENA_START + 0x40));
        assert_eq!(a.used(), PAGE);
        for at in [ARENA_START + 0x10, ARENA_START + 0x40] {
            assert_eq!(at % ALIGN, 0);
        }
    }

    #[test]
    fn a_full_shared_page_opens_the_next_and_gaps_are_filled_first() {
        let mut a = Arena::default();
        assert_eq!(a.alloc(0xff0, RW), Some(ARENA_START));
        // Does not fit in the 0x10 left: a new page.
        assert_eq!(a.alloc(0x20, RW), Some(ARENA_START + PAGE));
        // Fits in the first page's tail.
        assert_eq!(a.alloc(0x10, RW), Some(ARENA_START + 0xff0));
        assert_eq!(a.used(), 2 * PAGE);
    }

    #[test]
    fn each_shared_page_has_one_protection() {
        let mut a = Arena::default();
        let rw = a.alloc(8, RW).unwrap();
        let rx = a.alloc(8, RX).unwrap();
        let rw2 = a.alloc(8, RW).unwrap();
        let shadow = a.alloc_shadow(false).unwrap();
        assert_eq!(rw2, rw + ALIGN);
        assert_ne!(mapping(rw, 8).0, mapping(rx, 8).0);
        assert_ne!(mapping(shadow, SHADOW_STRIDE).0, mapping(rw, 8).0);
        assert_ne!(mapping(shadow, SHADOW_STRIDE).0, mapping(rx, 8).0);
        assert_eq!(a.used(), 3 * PAGE);
    }

    #[test]
    fn shadows_pack_four_bytes_apart_on_their_own_pages() {
        let mut a = Arena::default();
        let first = a.alloc_shadow(false).unwrap();
        for i in 1..PAGE / SHADOW_STRIDE {
            assert_eq!(a.alloc_shadow(false), Some(first + i * SHADOW_STRIDE));
        }
        assert_eq!(a.alloc_shadow(false), Some(first + PAGE));
        assert_eq!(a.used(), 2 * PAGE);
    }

    #[test]
    fn a_page_or_more_gets_whole_pages_of_its_own() {
        let mut a = Arena::default();
        assert_eq!(a.alloc(8, RW), Some(ARENA_START));
        assert_eq!(a.alloc(PAGE, RW), Some(ARENA_START + PAGE));
        assert_eq!(a.alloc(PAGE + 1, RW), Some(ARENA_START + 2 * PAGE));
        // The shared page still takes small requests.
        assert_eq!(a.alloc(8, RW), Some(ARENA_START + ALIGN));
        assert_eq!(a.used(), 4 * PAGE);
        assert_eq!(mapping(ARENA_START + 2 * PAGE, PAGE + 1), (ARENA_START + 2 * PAGE, 2 * PAGE));
        assert_eq!(mapping(ARENA_START + ALIGN, 8), (ARENA_START, PAGE));
    }

    #[test]
    fn presentation_pages_leave_low_addresses_unchanged() {
        let mut arena = Arena::default();
        assert_eq!(arena.alloc_high(8, RW), Some(ARENA_END - PAGE));
        assert_eq!(arena.alloc(4, RW), Some(ARENA_START));
        // Small high requests share the high page, not the low one.
        assert_eq!(arena.alloc_high(8, RW), Some(ARENA_END - PAGE + ALIGN));
        assert_eq!(arena.alloc_shadow(true), Some(ARENA_END - 2 * PAGE));
        assert_eq!(arena.alloc_high(0x1001, RW), Some(ARENA_END - 4 * PAGE));
        assert_eq!(arena.used(), 5 * PAGE);
        assert_eq!(arena.alloc_high(ARENA_SIZE - 4 * PAGE, RW), None);
        assert_eq!(
            arena.alloc(ARENA_SIZE - 5 * PAGE, RW),
            Some(ARENA_START + PAGE)
        );
        assert_eq!(arena.free(), 0);
        assert_eq!(arena.alloc_high(1, RX), None);
        // Room on an existing shared page is still handed out.
        assert_eq!(arena.alloc(4, RW), Some(ARENA_START + ALIGN));
    }

    #[test]
    fn allocation_stops_at_the_end() {
        let mut a = Arena::default();
        assert_eq!(a.alloc(4, RW), Some(ARENA_START));
        assert_eq!(a.alloc(0x1001, RW), Some(ARENA_START + PAGE));
        assert_eq!(a.used(), 3 * PAGE);
        assert_eq!(a.alloc(ARENA_SIZE, RW), None);
        assert_eq!(a.alloc(ARENA_SIZE - 3 * PAGE, RW), Some(ARENA_START + 3 * PAGE));
        assert_eq!(a.alloc(8, RX), None);
        assert_eq!(a.alloc_shadow(false), None);
        assert_eq!(a.alloc(u32::MAX, RW), None);
    }
}
