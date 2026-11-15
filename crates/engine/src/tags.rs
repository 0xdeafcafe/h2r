//! Checked tag lookup over the published cache index (`0x122c10`,
//! `0x122d00`). See `docs/engine/tag_get_checked.md`.
//!
//! The cache loader (`0x1228f0`) publishes two guest pointers that most of
//! the engine reads directly: the tag-index header and the tag-entry table.
//! These functions read them exactly as the original does; nothing here
//! owns or copies the index.

use std::fmt;

use crate::handle::DatumHandle;
use crate::memory::{GuestMemory, MemFault};

type R<T> = Result<T, MemFault>;

/// Global holding the guest address of the tag-index header.
pub const TAG_INDEX_GLOBAL: u32 = 0x0054_7f00;
/// Global holding the guest address of the 16-byte tag-entry table. The
/// checked lookup uses this, not the index header's own `+8` pointer.
pub const TAG_ENTRIES_GLOBAL: u32 = 0x004e_3b44;

/// Tag-index header fields read here (`docs/map-format.md`).
pub mod index_layout {
    /// Guest pointer to the 12-byte group records, sorted for bsearch.
    pub const GROUPS: u32 = 0x00;
    /// `i32` group record count.
    pub const GROUP_COUNT: u32 = 0x04;
    /// `i32` tag slot count. The lookup's bound.
    pub const TAG_COUNT: u32 = 0x18;
}

/// A tag entry: `+0` group, `+4` full salted datum, `+8` metadata address,
/// `+0xc` metadata size.
pub mod entry_layout {
    pub const GROUP: u32 = 0x0;
    pub const DATUM: u32 = 0x4;
    pub const ADDRESS: u32 = 0x8;
    pub const SIZE: u32 = 0xc;
    pub const STRIDE: u32 = 0x10;
}

/// A group record: the group and its parent and grandparent (`0xffffffff`
/// when absent).
pub mod group_layout {
    pub const GROUP: u32 = 0x0;
    pub const PARENT: u32 = 0x4;
    pub const GRANDPARENT: u32 = 0x8;
    pub const STRIDE: u32 = 0xc;
}

/// A tag group FourCC, stored as the guest stores it: the numeric value of
/// the multi-character constant, so `'scnr'` is `0x73636e72`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupTag(pub u32);

impl GroupTag {
    /// `0xffffffff`: no group. Also the "absent ancestor" marker.
    pub const NONE: GroupTag = GroupTag(u32::MAX);

    /// From the four characters as written (`b"scnr"`).
    pub const fn from_chars(c: [u8; 4]) -> Self {
        GroupTag(u32::from_be_bytes(c))
    }
}

impl fmt::Debug for GroupTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0.to_be_bytes();
        if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            write!(
                f,
                "GroupTag('{}')",
                b.iter().map(|&c| c as char).collect::<String>()
            )
        } else {
            write!(f, "GroupTag({:#010x})", self.0)
        }
    }
}

/// `tag_group_find` (`0x122d00`): the address of the group record for
/// `group`, or `None` (guest 0) when no index is published or the group is
/// absent.
///
/// The search is the guest's own bsearch (`0x13ddd0`) with comparator
/// `0x122cf0`, reproduced probe for probe: the comparator returns the
/// *wrapping* difference `key - record.group` as a signed 32-bit value, so
/// ordering follows the sign of that difference, not unsigned order. On an
/// unsorted table the probe sequence decides the result, so it must match.
pub fn tag_group_find(m: &dyn GuestMemory, group: GroupTag) -> R<Option<u32>> {
    match tag_group_find_bounded(m, group, u64::MAX)? {
        Bounded::Done(r) => Ok(r),
        // u64::MAX probes is longer than any run; treat it as the hang it is.
        Bounded::OutOfBudget => unreachable!("bsearch exceeded u64::MAX probes"),
    }
}

/// Result of a search run with a probe budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bounded<T> {
    Done(T),
    /// The budget ran out with the search still going. With a negative
    /// group count the guest search never terminates unless a probe
    /// matches, so this is how a test observes the hang.
    OutOfBudget,
}

/// [`tag_group_find`] with at most `max_probes` comparator calls.
pub fn tag_group_find_bounded(
    m: &dyn GuestMemory,
    group: GroupTag,
    max_probes: u64,
) -> R<Bounded<Option<u32>>> {
    let index = m.u32(TAG_INDEX_GLOBAL)?;
    if index == 0 {
        return Ok(Bounded::Done(None));
    }
    let base = m.u32(index.wrapping_add(index_layout::GROUPS))?;
    let count = m.i32(index.wrapping_add(index_layout::GROUP_COUNT))?;
    let found = match bsearch_groups(m, group, base, count, max_probes)? {
        Bounded::OutOfBudget => return Ok(Bounded::OutOfBudget),
        Bounded::Done(None) => return Ok(Bounded::Done(None)),
        Bounded::Done(Some(i)) => i,
    };
    // The guest maps the hit back to an index and re-derives the address
    // from a fresh read of the index pointer.
    let index = m.u32(TAG_INDEX_GLOBAL)?;
    let base = m.u32(index.wrapping_add(index_layout::GROUPS))?;
    Ok(Bounded::Done(Some(base.wrapping_add(
        (found as u32).wrapping_mul(group_layout::STRIDE),
    ))))
}

/// `0x13ddd0` specialised to 12-byte group records: returns the element
/// index, or `None` for the guest's `-1` (including a genuine hit at index
/// -1, which a negative count can produce).
///
/// A negative count never reaches zero under the arithmetic shift, so a
/// corrupt index makes this loop until a probe matches, exactly as the
/// guest would; `max_probes` bounds that.
fn bsearch_groups(
    m: &dyn GuestMemory,
    key: GroupTag,
    base: u32,
    count: i32,
    max_probes: u64,
) -> R<Bounded<Option<i32>>> {
    const SIZE: i32 = group_layout::STRIDE as i32;
    let mut lo = base;
    let mut n = count;
    let mut probes = 0u64;
    while n != 0 {
        if probes == max_probes {
            return Ok(Bounded::OutOfBudget);
        }
        probes += 1;
        let mid = lo.wrapping_add(((n >> 1).wrapping_mul(SIZE)) as u32);
        let r = key.0.wrapping_sub(m.u32(mid)?) as i32;
        if r == 0 {
            let i = (mid.wrapping_sub(base) as i32).wrapping_div(SIZE);
            return Ok(Bounded::Done((i != -1).then_some(i)));
        }
        if r > 0 {
            lo = mid.wrapping_add(SIZE as u32);
            n = n.wrapping_sub(1);
        }
        n >>= 1;
    }
    Ok(Bounded::Done(None))
}

/// `tag_get_checked` (`0x122c10`): the metadata address of tag `tag` if
/// its entry carries exactly that datum and its group is `expected` or has
/// `expected` as parent or grandparent. `None` (guest 0) otherwise.
///
/// - The slot is the **sign-extended** low 16 bits; a negative slot fails.
///   (Inline consumers mask instead.) The bound is the index's tag count.
/// - The whole 32-bit datum must match the entry's, so stale salts fail.
/// - An entry whose address is 0 is rejected after that comparison (the
///   guest's null-entry sentinel), even if its datum matches.
/// - **Quirk:** if the entry's group has no record in the group table,
///   the guest reads the "record" at address 0. That read is reproduced, so
///   it faults wherever guest page 0 is unmapped, as it would on hardware.
/// - **Quirk:** `expected = NONE` matches any group with an absent parent or
///   grandparent, because absent ancestors are stored as `0xffffffff`.
pub fn tag_get_checked(
    m: &dyn GuestMemory,
    tag: DatumHandle,
    expected: GroupTag,
) -> R<Option<u32>> {
    let Some(entry) = tag_entry_checked(m, tag)? else {
        return Ok(None);
    };
    let group = GroupTag(m.u32(entry.wrapping_add(entry_layout::GROUP))?);
    let record = tag_group_find(m, group)?.unwrap_or(0);
    let accepted = m.u32(record.wrapping_add(group_layout::GROUP))? == expected.0
        || m.u32(record.wrapping_add(group_layout::PARENT))? == expected.0
        || m.u32(record.wrapping_add(group_layout::GRANDPARENT))? == expected.0;
    if !accepted {
        return Ok(None);
    }
    m.u32(entry.wrapping_add(entry_layout::ADDRESS)).map(Some)
}

/// The bounds-and-salt half of `tag_get_checked`: the entry address for
/// `tag`, if its slot is in range and the entry's datum equals `tag`.
pub fn tag_entry_checked(m: &dyn GuestMemory, tag: DatumHandle) -> R<Option<u32>> {
    let slot = tag.index() as i16 as i32;
    if slot < 0 {
        return Ok(None);
    }
    let index = m.u32(TAG_INDEX_GLOBAL)?;
    if slot >= m.i32(index.wrapping_add(index_layout::TAG_COUNT))? {
        return Ok(None);
    }
    let entries = m.u32(TAG_ENTRIES_GLOBAL)?;
    let entry = ((slot as u32) << 4).wrapping_add(entries);
    if m.u32(entry.wrapping_add(entry_layout::DATUM))? != tag.raw() {
        return Ok(None);
    }
    // The guest keeps "no entry" as a null pointer and tests it after the
    // datum comparison, so an entry that lands on address 0 is rejected
    // even when its datum matches.
    Ok((entry != 0).then_some(entry))
}
