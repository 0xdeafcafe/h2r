//! A datum array's header: the pools objects, players and most game
//! state live in (docs/engine/datum_new.md; `guest_layout::datum` is the ported
//! allocator and keeps the full header there).

crate::layout! {
    /// The header a datum array global points at (`0x4c` bytes). Slot `i`
    /// is at `data + i * element_size`, live when bit `i` of the bitmap
    /// is set.
    pub struct DatumArray: 0x4c {
        /// Element capacity.
        MAXIMUM_COUNT maximum_count @ 0x20: i32,
        /// Bytes per element.
        ELEMENT_SIZE element_size @ 0x24: i32,
        /// One past the highest slot in use: iteration stops here.
        HIGH_WATER high_water @ 0x38: i32,
        /// Live elements.
        ACTIVE_COUNT active_count @ 0x3c: i32,
        /// Element storage.
        DATA data @ 0x44: u32,
        /// Occupancy bitmap, one bit per slot, in 32-bit words.
        BITMAP bitmap @ 0x48: u32,
    }
}
