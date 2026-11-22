//! The one reserved range of guest memory that features and the installer's
//! shadow entries are placed in, so private guest pages stop being
//! hand-picked addresses in comments.
//!
//! The range is inside the kernel's HLE reservation
//! (`kernel::memory::HLE_RESERVED`, `0xa0000000..0xa1000000`), which no
//! guest allocator places anything in. The Bink and DirectSound HLE search
//! that reservation upward from its start for unmapped pages, so a range
//! mapped at boot near its top is skipped by them.

/// Start of the feature arena: the page after the D3D scratch page
/// (`boot::d3d_glue::SCRATCH_BASE`, `0xa0e8_0000`), which ends the push
/// buffer's range.
pub const ARENA_START: u32 = 0xa0e8_1000;
/// End (exclusive): 1340 KiB, up to the community variants' fixed page at
/// `0xa0fd_0000`. 832 KiB filled once map packs, Halo 1, Halo 1 AI and
/// mods all placed their pages.
pub const ARENA_END: u32 = 0xa0fd_0000;
const PAGE: u32 = 0x1000;

/// A two-ended bump allocator over [`ARENA_START`]..[`ARENA_END`], page-granular.
/// Allocation is pure (an address is handed out); mapping happens at
/// install.
#[derive(Clone, Debug)]
pub struct Arena {
    next: u32,
    end: u32,
}

impl Default for Arena {
    fn default() -> Self {
        Arena {
            next: ARENA_START,
            end: ARENA_END,
        }
    }
}

impl Arena {
    /// `len` bytes rounded up to whole pages, or `None` when full.
    pub fn alloc(&mut self, len: u32) -> Option<u32> {
        let len = len.max(1).checked_add(PAGE - 1)? & !(PAGE - 1);
        let at = self.next;
        let end = at.checked_add(len)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(at)
    }
    /// Reserve a page without moving the simulation's low-end allocations.
    pub(crate) fn alloc_high(&mut self, len: u32) -> Option<u32> {
        let len = len.max(1).checked_add(PAGE - 1)? & !(PAGE - 1);
        let at = self.end.checked_sub(len)?;
        if at < self.next {
            return None;
        }
        self.end = at;
        Some(at)
    }

    /// Bytes handed out from both ends.
    pub fn used(&self) -> u32 {
        self.next - ARENA_START + (ARENA_END - self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_pages_leave_low_addresses_unchanged() {
        let mut arena = Arena::default();
        assert_eq!(arena.alloc_high(8), Some(ARENA_END - PAGE));
        assert_eq!(arena.alloc(4), Some(ARENA_START));
        assert_eq!(arena.alloc_high(0x1001), Some(ARENA_END - 3 * PAGE));
        assert_eq!(arena.used(), 4 * PAGE);
        assert_eq!(arena.alloc_high(ARENA_END - ARENA_START - 3 * PAGE), None);
        assert_eq!(
            arena.alloc(ARENA_END - ARENA_START - 4 * PAGE),
            Some(ARENA_START + PAGE)
        );
        assert_eq!(arena.alloc_high(1), None);
    }

    #[test]
    fn allocations_are_whole_pages_and_stop_at_the_end() {
        let mut a = Arena::default();
        assert_eq!(a.alloc(4), Some(ARENA_START));
        assert_eq!(a.alloc(0x1001), Some(ARENA_START + 0x1000));
        assert_eq!(a.used(), 0x3000);
        assert_eq!(a.alloc(ARENA_END - ARENA_START), None);
        assert_eq!(
            a.alloc(ARENA_END - ARENA_START - 0x3000),
            Some(ARENA_START + 0x3000)
        );
        assert_eq!(a.alloc(1), None);
    }
}
