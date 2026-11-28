//! The loaded map's tag index, as the game holds it in guest memory
//! (docs/engine/tag_get_checked.md, docs/formats/map-format.md "Index,
//! root tags and address translation").
//!
//! * `TAG_INDEX` (`0x00547f00`) holds the address of the 0x20-byte index
//!   header; its `+0x0c` is the scenario's datum and `+0x18` the slot count.
//! * `TAG_ENTRIES` (`0x004e3b44`) holds the address of the 16-byte entry
//!   table the engine reads tags through: group at `+0`, full datum at
//!   `+4`, metadata address at `+8`, metadata size at `+0xc`.
//!
//! The metadata is where the cache loader put it, at the virtual addresses
//! its own pointers use, so a tag's blocks are followed by their stored
//! addresses with no translation.

use map::FourCC;
use runtime::Guest;

/// Index header: the scenario's datum.
pub const HEADER_SCENARIO: u32 = 0x0c;
/// Index header: the number of tag slots.
pub const HEADER_COUNT: u32 = 0x18;
/// Bytes per entry.
pub const ENTRY_SIZE: u32 = 16;

/// Where the index is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagIndex {
    pub header: u32,
    pub entries: u32,
    pub count: u32,
    /// The loaded scenario's datum: changes with the map, so it tells an
    /// edit made on another map.
    pub scenario: u32,
}

/// One tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub slot: u32,
    pub group: FourCC,
    pub datum: u32,
    pub address: u32,
    pub size: u32,
}

impl Entry {
    pub(crate) fn contains(&self, addr: u32, len: u32) -> bool {
        let end = u64::from(addr) + u64::from(len);
        self.address <= addr
            && end <= u64::from(self.address) + u64::from(self.size)
            && end <= 1u64 << 32
    }
}

fn fault(what: &str, e: runtime::GuestFault) -> String {
    format!("{what}: guest fault at {:#010x}", e.addr)
}

/// The index, or why there is none (no map loaded yet).
pub fn index(g: &Guest) -> Result<TagIndex, String> {
    let header = g
        .u32(guest_layout::points::TAG_INDEX.base)
        .map_err(|e| fault("tag index pointer", e))?;
    let entries = g
        .u32(guest_layout::points::TAG_ENTRIES.base)
        .map_err(|e| fault("tag entry pointer", e))?;
    if header == 0 || entries == 0 {
        return Err("no tag index: no map is loaded".into());
    }
    let count = g
        .u32(header + HEADER_COUNT)
        .map_err(|e| fault("tag index header", e))?;
    if count > 0x1_0000 {
        return Err(format!("tag index has {count} slots: not an index"));
    }
    let scenario = g
        .u32(header + HEADER_SCENARIO)
        .map_err(|e| fault("tag index header", e))?;
    Ok(TagIndex {
        header,
        entries,
        count,
        scenario,
    })
}

/// The tag in `slot`, `None` for an empty slot.
pub fn entry(g: &Guest, idx: &TagIndex, slot: u32) -> Result<Option<Entry>, String> {
    let at = idx.entries + slot * ENTRY_SIZE;
    let mut b = [0u8; 16];
    g.read_bytes(at, &mut b, 1)
        .map_err(|e| fault("tag entry", e))?;
    let w = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let (group, datum, address, size) = (w(0), w(4), w(8), w(12));
    if group == u32::MAX || datum == u32::MAX || address == 0 {
        return Ok(None);
    }
    Ok(Some(Entry {
        slot,
        group: FourCC::from_u32(group),
        datum,
        address,
        size,
    }))
}

/// The tag whose full datum is `datum` (a stale salt is not it).
pub fn by_datum(g: &Guest, idx: &TagIndex, datum: u32) -> Result<Entry, String> {
    let slot = datum & 0xffff;
    if slot >= idx.count {
        return Err(format!("no tag {datum:#010x}: slot past the index"));
    }
    match entry(g, idx, slot)? {
        Some(e) if e.datum == datum => Ok(e),
        _ => Err(format!("no tag {datum:#010x} in the loaded map")),
    }
}

/// Every tag.
pub fn all(g: &Guest, idx: &TagIndex) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for slot in 0..idx.count {
        if let Some(e) = entry(g, idx, slot)? {
            out.push(e);
        }
    }
    Ok(out)
}
