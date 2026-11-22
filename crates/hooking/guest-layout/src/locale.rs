//! Localized string lookup by packed StringID (`0x1a02d0`, `0x1a0310`).
//! See `docs/engine/locale_string_id_find.md`.
//!
//! A locale table is one of nine 0x1c-byte records at `matg + 0x188 +
//! language * 0x1c`. `locale_tables_load` (`0x1a01e0`) fills in the two
//! resident pointers and the loaded flag. A `unic` tag selects a range of
//! entries per language; lookups scan that range for a full 32-bit
//! StringID match.

use std::fmt;

use crate::memory::{GuestMemory, MemFault};

type R<T> = Result<T, MemFault>;

/// Guest address of a NUL byte in `.rdata`, returned for a missing string.
/// Callers copy from it unconditionally, so a miss reads as "".
pub const EMPTY_STRING: u32 = 0x0043_e5e1;

/// Locale-record fields (`docs/formats/map-format.md`, "LocaleDescriptor").
pub mod layout {
    /// Resident pointer to the index: `count` 8-byte entries.
    pub const INDEX: u32 = 0x00;
    /// Resident pointer to the string blob.
    pub const BLOB: u32 = 0x04;
    /// `i32` entry count.
    pub const COUNT: u32 = 0x08;
    /// `i32` blob size.
    pub const BLOB_SIZE: u32 = 0x0c;
    /// File offset of the index.
    pub const INDEX_OFFSET: u32 = 0x10;
    /// File offset of the blob.
    pub const BLOB_OFFSET: u32 = 0x14;
    /// `u8`: set to 1 once both are resident.
    pub const LOADED: u32 = 0x18;
    /// Record stride within `matg`.
    pub const STRIDE: u32 = 0x1c;
    /// First record's offset within `matg`.
    pub const MATG_OFFSET: u32 = 0x188;
}

/// An index entry: `+0` StringID, `+4` offset into the blob.
pub mod entry_layout {
    pub const STRING_ID: u32 = 0;
    pub const OFFSET: u32 = 4;
    pub const STRIDE: u32 = 8;
}

/// A packed StringID: string length in the high byte, identifier in the
/// low 24 bits. Lookups compare all 32 bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

impl StringId {
    pub const fn length(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub const fn index(self) -> u32 {
        self.0 & 0x00ff_ffff
    }
}

impl fmt::Debug for StringId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StringId(len {}, {:#08x})", self.length(), self.index())
    }
}

/// A locale record at a guest address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocaleTable {
    pub addr: u32,
}

impl LocaleTable {
    pub const fn at(addr: u32) -> Self {
        LocaleTable { addr }
    }

    /// The record for `language` inside the globals tag at `matg`.
    pub const fn in_globals(matg: u32, language: u32) -> Self {
        LocaleTable {
            addr: matg
                .wrapping_add(layout::MATG_OFFSET)
                .wrapping_add(language.wrapping_mul(layout::STRIDE)),
        }
    }

    /// The first entry in `start .. start + count` whose StringID equals
    /// `key`. Indices outside `0 ..= table count - 1` are skipped, which is
    /// how the inlined bounds check behaves in both guest scanners.
    /// `start + count` wraps, and both bounds are signed, as in the guest.
    fn scan(&self, m: &dyn GuestMemory, start: i32, count: i32, key: StringId) -> R<Option<i32>> {
        let end = start.wrapping_add(count);
        let mut i = start;
        while i < end {
            // The guest clamps i into [0, count - 1] and only compares when
            // the clamp left it unchanged. `count - 1` wraps, so a count of
            // i32::MIN accepts every non-negative index.
            let in_range = i >= 0
                && i <= m
                    .i32(self.addr.wrapping_add(layout::COUNT))?
                    .wrapping_sub(1);
            if in_range {
                let index = m.u32(self.addr.wrapping_add(layout::INDEX))?;
                let e = index.wrapping_add((i as u32).wrapping_mul(entry_layout::STRIDE));
                if m.u32(e.wrapping_add(entry_layout::STRING_ID))? == key.0 {
                    return Ok(Some(i));
                }
            }
            i += 1;
        }
        Ok(None)
    }

    /// `locale_string_id_find` (`0x1a02d0`): the guest address of the string
    /// for `key` among entries `start .. start + count`, or
    /// [`EMPTY_STRING`] when it is not there.
    ///
    /// It does not check the loaded flag; its callers do.
    pub fn find(&self, m: &dyn GuestMemory, start: i32, count: i32, key: StringId) -> R<u32> {
        match self.scan(m, start, count, key)? {
            Some(i) => {
                let index = m.u32(self.addr.wrapping_add(layout::INDEX))?;
                let e = index.wrapping_add((i as u32).wrapping_mul(entry_layout::STRIDE));
                let offset = m.u32(e.wrapping_add(entry_layout::OFFSET))?;
                let blob = m.u32(self.addr.wrapping_add(layout::BLOB))?;
                Ok(offset.wrapping_add(blob))
            }
            None => Ok(EMPTY_STRING),
        }
    }

    /// `locale_string_id_exists` (`0x1a0310`, assigned name): whether the
    /// table is loaded and has `key` among entries `start .. start + count`.
    pub fn contains(&self, m: &dyn GuestMemory, start: i32, count: i32, key: StringId) -> R<bool> {
        Ok(self.is_loaded(m)? && self.contains_loaded(m, start, count, key)?)
    }

    /// The loaded flag, the first thing `0x1a0310` reads.
    pub fn is_loaded(&self, m: &dyn GuestMemory) -> R<bool> {
        Ok(m.u8(self.addr.wrapping_add(layout::LOADED))? != 0)
    }

    /// [`contains`](Self::contains) after its loaded check, for callers
    /// that have already made it (the adapter reads the stack count in
    /// between, as the original does).
    pub fn contains_loaded(
        &self,
        m: &dyn GuestMemory,
        start: i32,
        count: i32,
        key: StringId,
    ) -> R<bool> {
        Ok(self.scan(m, start, count, key)?.is_some())
    }
}
