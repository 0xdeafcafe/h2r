//! The loaded tag index and its entries (docs/engine/tag_get_checked.md;
//! `guest_layout::tags` is the ported lookup and keeps the same offsets).

use crate::field::Datum;

crate::layout! {
    /// The tag index header [`crate::global::TAG_INDEX`] points at.
    pub struct TagIndex: 0x1c {
        /// The group records, sorted for the lookup's binary search.
        GROUPS groups @ 0x00: u32,
        GROUP_COUNT group_count @ 0x04: i32,
        /// The loaded scenario's tag.
        SCENARIO scenario @ 0x0c: Datum,
        /// Tag slots: a tag's index must be below it.
        TAG_COUNT tag_count @ 0x18: i32,
    }
}

crate::layout! {
    /// One tag entry, `0x10` bytes, in the array
    /// [`crate::global::TAG_ENTRIES`] points at; slot `i` at `i * 0x10`.
    pub struct TagEntry: 0x10 {
        /// The group's four characters as a little-endian word.
        GROUP group @ 0x0: u32,
        /// The tag's full salted datum: a lookup must match it whole.
        DATUM datum @ 0x4: Datum,
        /// The tag's data; 0 is the guest's "no entry".
        ADDRESS address @ 0x8: u32,
        DATA_SIZE size @ 0xc: u32,
    }
}
