//! The physical arena the tag cache loads into, and how far ours is grown
//! past retail's. One table for the runtime that reserves it, the hook that
//! moves the game's cursors, and the writer's metadata bounds. See
//! docs/formats/map-format.md, "The tag arena".
//!
//! Retail `0x0012b400` stores the arena's cursors (low `0x61000`, high
//! `0x3145000`, at `0x004e6424..`) and calls `XPhysicalAlloc(0x30e4000,
//! 0x61000, 0, PAGE_READWRITE)`. The cache loader (`0x1228f0`) takes the
//! map's `+0x1c` from the low end at `TAG_CACHE_BASE`, the game carves from
//! the high end, and `0x12c1e0` hands every page left between the cursors
//! to one pool (`0x13d8b0`). Growing the arena's high end moves nothing the
//! maps name: every tag address is relative to the fixed low end.

use std::ops::Range;

/// Physical RAM of a retail Xbox.
pub const RETAIL_RAM: u32 = 0x0400_0000;

/// The arena as retail `0x0012b400` reserves it (physical, end exclusive).
pub const RETAIL_ARENA: Range<u32> = 0x0006_1000..0x0314_5000;

/// What our runtime adds to the arena's high end and to guest RAM: 8 MiB.
///
/// Every map's tag growth comes out of the pool that takes the arena's
/// free pages after the load, so growing the arena by this much lets a
/// map's `+0x1c` grow by this much while that pool stays as large as
/// retail's worst case. Guest RAM grows by the same amount, so what lies
/// after the arena (the XBE image, stacks, the kernel's and our own
/// physical allocations) keeps the room it had on a retail console.
/// 8 MiB is 50 times the 164 KiB Waterworks' Firefight kit was over and
/// covers the largest DLC map's excess (Turf, about 380 KB) many times.
pub const GROWTH: u32 = 0x0080_0000;

/// The arena our runtime reserves (physical, end exclusive).
pub const ARENA: Range<u32> = RETAIL_ARENA.start..RETAIL_ARENA.end + GROWTH;

/// Guest RAM in our runtime: retail's plus [`GROWTH`].
pub const RAM: u32 = RETAIL_RAM + GROWTH;

/// The contiguous alias base: physical `p` is visible at `ALIAS + p`.
pub const ALIAS: u32 = 0x8000_0000;

/// Where the loader places the tag index (`ALIAS + ARENA.start`).
pub const TAG_CACHE_BASE: u32 = ALIAS + RETAIL_ARENA.start;

/// The game's arena state at `0x004e6420`: the stack depth, then the
/// arena bounds and the per-depth low and high cursors.
pub mod cursors {
    /// Low bound (`0x61000`).
    pub const LOW_BOUND: u32 = 0x004e_6424;
    /// High bound (`0x3145000` on retail).
    pub const HIGH_BOUND: u32 = 0x004e_6428;
    /// Depth-0 low cursor.
    pub const LOW: u32 = 0x004e_642c;
    /// Depth-0 high cursor; the game carves down from it.
    pub const HIGH: u32 = 0x004e_6440;
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u32 = 0x1000;

    #[test]
    fn the_grown_arena_keeps_its_base_and_fits_page_aligned_in_ram() {
        assert_eq!(ARENA.start, RETAIL_ARENA.start, "tag addresses stay put");
        assert_eq!(TAG_CACHE_BASE, 0x8006_1000);
        for v in [ARENA.start, ARENA.end, GROWTH, RAM] {
            assert_eq!(v % PAGE, 0, "{v:#x} is not page aligned");
        }
        // The XBE image, stacks and kernel pages keep retail's room after it.
        assert_eq!(RAM - ARENA.end, RETAIL_RAM - RETAIL_ARENA.end);
        assert!(ARENA.end <= RAM);
        // No overlap with the window above the alias.
        assert!(u64::from(ALIAS) + u64::from(RAM) <= 0xa000_0000);
        assert_eq!(ARENA.end - ARENA.start, 0x030e_4000 + GROWTH);
    }

    #[test]
    fn every_metadata_bound_grows_by_the_arena_growth() {
        use crate::{CacheType, writer};
        for t in [
            CacheType::Campaign,
            CacheType::Multiplayer,
            CacheType::MainMenu,
            CacheType::Shared,
            CacheType::SinglePlayerShared,
        ] {
            let retail = writer::retail_max_metadata_size(t).unwrap();
            assert_eq!(writer::max_metadata_size(t).unwrap(), retail + GROWTH, "{t:?}");
        }
        assert_eq!(writer::retail_max_metadata_size(CacheType::Multiplayer).unwrap(), 0x0062_c000);
    }

    #[test]
    fn the_cursor_words_follow_the_depth_word() {
        assert_eq!(cursors::LOW_BOUND, 0x004e_6420 + 4);
        assert_eq!(cursors::HIGH_BOUND, cursors::LOW_BOUND + 4);
        assert_eq!(cursors::LOW, cursors::HIGH_BOUND + 4);
        // Five low cursors (depth 0..4), then the high ones.
        assert_eq!(cursors::HIGH, cursors::LOW + 5 * 4);
    }
}
