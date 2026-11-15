//! Salted datum handles.
//!
//! Every datum array (and the tag index) names its elements with a 32-bit
//! handle: the element's salt in the high 16 bits and its absolute index in
//! the low 16. A slot gets a new salt each time it is allocated, so a handle
//! kept past a delete stops matching. `0xffffffff` means "no datum".

use std::fmt;

/// A salted datum handle, exactly as the guest stores it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DatumHandle(pub u32);

impl DatumHandle {
    /// The guest's "none" value. Lookups return it on failure.
    pub const NONE: DatumHandle = DatumHandle(u32::MAX);

    /// `(salt << 16) | index`, as every constructor in the engine builds it.
    ///
    /// The index is OR-ed in at full width, matching the guest's `or eax,
    /// index`: an index of `0x10000` or more (impossible for a sane array)
    /// would overlap the salt bits rather than be truncated.
    pub const fn new(salt: u16, index: u32) -> Self {
        DatumHandle(((salt as u32) << 16) | index)
    }

    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    /// Low 16 bits: the absolute element index.
    pub const fn index(self) -> u16 {
        self.0 as u16
    }

    /// High 16 bits: the salt the element must carry.
    pub const fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }

    /// Raw value for writing back into guest registers or memory.
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// `None` for [`DatumHandle::NONE`], so Rust callers get an `Option`
    /// instead of a sentinel.
    pub const fn from_raw(v: u32) -> Option<Self> {
        if v == u32::MAX {
            None
        } else {
            Some(DatumHandle(v))
        }
    }
}

impl fmt::Debug for DatumHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            write!(f, "DatumHandle(NONE)")
        } else {
            write!(f, "DatumHandle({:#06x}:{})", self.salt(), self.index())
        }
    }
}

/// Raw guest form of an optional handle.
pub const fn raw_or_none(h: Option<DatumHandle>) -> u32 {
    match h {
        Some(h) => h.0,
        None => u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_round_trip() {
        let h = DatumHandle::new(0x8123, 7);
        assert_eq!(h.raw(), 0x8123_0007);
        assert_eq!((h.salt(), h.index()), (0x8123, 7));
        assert_eq!(DatumHandle::from_raw(u32::MAX), None);
        assert_eq!(raw_or_none(None), u32::MAX);
    }
}
