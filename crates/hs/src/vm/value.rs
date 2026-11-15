//! The typed value model: 32-bit value cells plus the type that reads them.
//!
//! Retail HaloScript keeps every value in a four-byte slot (thread locals,
//! global cells, builtin argument arrays). The *type* decides how many of the
//! slot's bytes are meaningful and how they are read; the evaluator never
//! tags a slot. [`Cell`] is that slot and [`Value`] is a typed reading of it.
//! Writes that only touch the low bytes (an 8-bit boolean, a 16-bit index)
//! keep the slot's remaining bytes, exactly as the guest's narrow stores do.

/// A HaloScript type index (0..62), as stored in node `+4`, builtin
/// descriptors and definition records. The XBE's type-name table supplies
/// the display names; the constants here name only the roles the evaluator
/// itself depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HsType(pub u16);

impl HsType {
    pub const UNPARSED: Self = Self(0);
    pub const SPECIAL_FORM: Self = Self(1);
    pub const FUNCTION_NAME: Self = Self(2);
    pub const PASSTHROUGH: Self = Self(3);
    pub const VOID: Self = Self(4);
    pub const BOOLEAN: Self = Self(5);
    pub const REAL: Self = Self(6);
    pub const SHORT: Self = Self(7);
    pub const LONG: Self = Self(8);
    pub const STRING: Self = Self(9);
    pub const SCRIPT: Self = Self(10);
    pub const STRING_ID: Self = Self(11);
    pub const AI: Self = Self(19);
    pub const OBJECT_LIST: Self = Self(31);
    /// First of the six enumerations (44..=49) converted to real as `n + 1`.
    pub const FIRST_ENUM: Self = Self(44);
    pub const LAST_ENUM: Self = Self(49);
    /// Object types 50..=55: object, unit, vehicle, weapon, device, scenery.
    pub const OBJECT: Self = Self(50);
    pub const UNIT: Self = Self(51);
    pub const LAST_OBJECT: Self = Self(55);
    /// Object-name types 56..=61, one per object type above.
    pub const FIRST_OBJECT_NAME: Self = Self(56);
    pub const LAST_OBJECT_NAME: Self = Self(61);
    pub const COUNT: u16 = 62;

    pub fn index(self) -> usize {
        self.0 as usize
    }
    pub fn is_object(self) -> bool {
        (Self::OBJECT.0..=Self::LAST_OBJECT.0).contains(&self.0)
    }
    pub fn is_object_name(self) -> bool {
        (Self::FIRST_OBJECT_NAME.0..=Self::LAST_OBJECT_NAME.0).contains(&self.0)
    }
    pub fn is_enum(self) -> bool {
        (Self::FIRST_ENUM.0..=Self::LAST_ENUM.0).contains(&self.0)
    }

    /// Bytes of the slot the engine reads and compares for this type.
    ///
    /// Recovered from the external-global refresh/write-back switches
    /// (`0x0020a2e0`, `0x0020a490`), whose case lists give each type's store
    /// width, and checked at load against the per-type size table that
    /// `=`/`!=` (`0x002084c0`) hands to `repe cmpsb` (`VmImage::from_xbe`).
    /// Types 0..=4 have no storage.
    pub fn size(self) -> u8 {
        match self.0 {
            0..=4 => 0,
            5 => 1,
            7 | 10 | 13..=17 | 20..=27 | 30 | 44..=49 | 56..=61 => 2,
            _ => 4,
        }
    }

    /// The value an *unbound* external global reads as: the refresh at
    /// `0x0020a2e0` stores 0 (boolean/real/short/long/string_id), the empty
    /// string (string), `0xffff` (16-bit indices) or `-1` (32-bit handles)
    /// when the descriptor's value pointer is null.
    pub fn unbound_external(self) -> u32 {
        match self.0 {
            5..=8 | 11 => 0,
            9 => Cell::EMPTY_STRING.0,
            _ if self.size() == 2 => 0xffff,
            _ => u32::MAX,
        }
    }
}

/// One four-byte value slot. Readers look at `HsType::size` low bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Cell(pub u32);

impl Cell {
    pub const ZERO: Self = Self(0);
    pub const NONE: Self = Self(u32::MAX);
    /// Host encoding of the engine's empty-string constant (the pointer the
    /// refresh stores for a null external string). See [`StringRef`].
    pub const EMPTY_STRING: Self = Self(StringRef::HOST_BIT);

    pub fn from_bool(b: bool) -> Self {
        Self(b as u32)
    }
    pub fn from_real(r: f32) -> Self {
        Self(r.to_bits())
    }
    pub fn from_short(s: i16) -> Self {
        Self(s as u16 as u32)
    }
    pub fn from_long(l: i32) -> Self {
        Self(l as u32)
    }
    pub fn boolean(self) -> bool {
        self.0 as u8 != 0
    }
    pub fn real(self) -> f32 {
        f32::from_bits(self.0)
    }
    pub fn short(self) -> i16 {
        self.0 as u16 as i16
    }
    pub fn long(self) -> i32 {
        self.0 as i32
    }
    /// Store only `ty.size()` low bytes, keeping the rest of the slot, the
    /// way the guest's byte/word/dword stores into an existing slot do.
    pub fn store(self, ty: HsType, value: Cell) -> Cell {
        let mask = match ty.size() {
            1 => 0xff,
            2 => 0xffff,
            _ => u32::MAX,
        };
        Cell((self.0 & !mask) | (value.0 & mask))
    }
    /// Compare the bytes `=` compares for `ty` (`repe cmpsb` over the size).
    pub fn bytes_equal(self, other: Cell, ty: HsType) -> bool {
        let n = ty.size() as usize;
        self.0.to_le_bytes()[..n] == other.0.to_le_bytes()[..n]
    }
}

/// A string slot. Guest string values are pointers; equality compares the
/// pointers, not the text. The VM preserves that by encoding a script string
/// as its offset in the scenario's string blob (which is what the load-time
/// fixup adds to the blob base) and engine-supplied text as an index into the
/// VM's host-string table with bit 31 set. Index 0 is the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringRef {
    Script(u32),
    Host(u32),
}
impl StringRef {
    pub const HOST_BIT: u32 = 0x8000_0000;
    pub fn from_cell(c: Cell) -> Self {
        if c.0 & Self::HOST_BIT != 0 {
            Self::Host(c.0 & !Self::HOST_BIT)
        } else {
            Self::Script(c.0)
        }
    }
    pub fn cell(self) -> Cell {
        match self {
            Self::Script(o) => Cell(o),
            Self::Host(i) => Cell(i | Self::HOST_BIT),
        }
    }
}

/// A typed reading of a [`Cell`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Void,
    Boolean(bool),
    Real(f32),
    Short(i16),
    Long(i32),
    String(StringRef),
    StringId(u32),
    /// 16-bit scenario block index or enumeration value; -1 is none.
    Index {
        ty: HsType,
        index: i16,
    },
    /// 32-bit datum handle, tag reference or packed value; -1 is none.
    Handle {
        ty: HsType,
        bits: u32,
    },
    /// Types with no storage (unparsed, special form, function name,
    /// passthrough): the raw slot.
    Raw {
        ty: HsType,
        bits: u32,
    },
}
impl Value {
    pub fn decode(ty: HsType, c: Cell) -> Self {
        match ty {
            HsType::VOID => Self::Void,
            HsType::BOOLEAN => Self::Boolean(c.boolean()),
            HsType::REAL => Self::Real(c.real()),
            HsType::SHORT => Self::Short(c.short()),
            HsType::LONG => Self::Long(c.long()),
            HsType::STRING => Self::String(StringRef::from_cell(c)),
            HsType::STRING_ID => Self::StringId(c.0),
            _ => match ty.size() {
                2 => Self::Index {
                    ty,
                    index: c.short(),
                },
                4 => Self::Handle { ty, bits: c.0 },
                _ => Self::Raw { ty, bits: c.0 },
            },
        }
    }
    pub fn cell(self) -> Cell {
        match self {
            Self::Void => Cell::ZERO,
            Self::Boolean(b) => Cell::from_bool(b),
            Self::Real(r) => Cell::from_real(r),
            Self::Short(s) => Cell::from_short(s),
            Self::Long(l) => Cell::from_long(l),
            Self::String(s) => s.cell(),
            Self::StringId(i) => Cell(i),
            Self::Index { index, .. } => Cell::from_short(index),
            Self::Handle { bits, .. } | Self::Raw { bits, .. } => Cell(bits),
        }
    }
}
