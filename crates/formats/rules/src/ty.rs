//! Value types shared by the language, the IR and the VM.
//!
//! Every run-time value is an `i32`. A type says what the number means and
//! which numbers are valid; [`Ty::valid`] is the one place that says so.

/// Players the VM addresses (roster slots 0..16).
pub const MAX_PLAYERS: usize = 16;
/// Teams the VM addresses (indices 0..8).
pub const MAX_TEAMS: usize = 8;
/// Kit squad slots (`firefight::director::SQUAD_SLOTS`).
pub const MAX_SQUADS: usize = 16;
/// Members a label allows at most (`LabelDef::max`), and so the most a
/// labelled set visits. A label whose `max` is this states no limit.
pub const MAX_LABELLED: usize = 256;
/// Objective records the engine holds at most (`E+0xc18`, bots.md 3.7).
pub const MAX_OBJECTIVES: usize = 8;
/// Enum types a script declares at most.
pub const MAX_ENUMS: usize = 32;
/// Variants of one enum at most.
pub const MAX_ENUM_VARIANTS: usize = 64;

/// A value type.
///
/// `player`, `team`, `object` and `tag` may be `none`, stored as -1.
/// `timer` is a slot type only: it is read and written as an `int` of ticks
/// left, and 0 is stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ty {
    /// 32-bit, saturating.
    Int,
    /// 16.16 fixed point.
    Fixed,
    Bool,
    Player,
    Team,
    /// An object handle the game hands out; never a datum.
    Object,
    /// A kit squad slot, 0..16.
    Squad,
    Timer,
    /// A kit class name, as a string-table index (-1 for a class the
    /// script never names).
    Class,
    /// A tag path, as a tag-table index.
    Tag,
    /// A Forge label, as a label-table index.
    Label,
    /// A message, as a string-table index.
    Text,
    /// A script-declared enum, by its index in the enum table.
    Enum(u8),
}

impl Ty {
    /// The binary code of the type.
    pub fn code(self) -> u8 {
        match self {
            Ty::Int => 0,
            Ty::Fixed => 1,
            Ty::Bool => 2,
            Ty::Player => 3,
            Ty::Team => 4,
            Ty::Object => 5,
            Ty::Squad => 6,
            Ty::Timer => 7,
            Ty::Class => 8,
            Ty::Tag => 9,
            Ty::Label => 10,
            Ty::Text => 11,
            Ty::Enum(n) => 0x40u8.saturating_add(n),
        }
    }

    pub fn from_code(code: u8) -> Option<Ty> {
        Some(match code {
            0 => Ty::Int,
            1 => Ty::Fixed,
            2 => Ty::Bool,
            3 => Ty::Player,
            4 => Ty::Team,
            5 => Ty::Object,
            6 => Ty::Squad,
            7 => Ty::Timer,
            8 => Ty::Class,
            9 => Ty::Tag,
            10 => Ty::Label,
            11 => Ty::Text,
            c if (0x40..0x40 + MAX_ENUMS as u8).contains(&c) => Ty::Enum(c - 0x40),
            _ => return None,
        })
    }

    /// Whether the type has a `none` value.
    pub fn nullable(self) -> bool {
        matches!(self, Ty::Player | Ty::Team | Ty::Object | Ty::Tag)
    }

    /// The value a register or slot of this type starts at.
    pub fn zero(self) -> i32 {
        if self.nullable() { NONE } else { 0 }
    }

    /// The register type a slot of this type is read into.
    pub fn register(self) -> Ty {
        if self == Ty::Timer { Ty::Int } else { self }
    }

    /// Whether `value` is a value of this type, given the blob's table
    /// sizes.
    pub fn valid(self, value: i32, sizes: &Sizes) -> bool {
        let below = |n: usize| value >= 0 && (value as usize) < n;
        match self {
            Ty::Int | Ty::Fixed => true,
            Ty::Bool => value == 0 || value == 1,
            Ty::Player => value == NONE || below(MAX_PLAYERS),
            Ty::Team => value == NONE || below(MAX_TEAMS),
            Ty::Object => true,
            Ty::Squad => below(MAX_SQUADS),
            Ty::Timer => value >= 0,
            Ty::Class => value == NONE || below(sizes.strings),
            Ty::Tag => value == NONE || below(sizes.tags),
            Ty::Label => below(sizes.labels),
            Ty::Text => below(sizes.strings),
            Ty::Enum(e) => sizes
                .enum_variants
                .get(e as usize)
                .is_some_and(|&n| below(n)),
        }
    }

    /// The name the language spells the type with.
    pub fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Fixed => "fixed",
            Ty::Bool => "bool",
            Ty::Player => "player",
            Ty::Team => "team",
            Ty::Object => "object",
            Ty::Squad => "squad",
            Ty::Timer => "timer",
            Ty::Class => "Class",
            Ty::Tag => "tag",
            Ty::Label => "label",
            Ty::Text => "text",
            Ty::Enum(_) => "enum",
        }
    }
}

/// `none` for the nullable types.
pub const NONE: i32 = -1;

/// The object datum a register value holds: all 32 bits are the handle
/// (a salted handle has bit 31 set, so it reads negative), and `NONE`
/// (0xFFFF_FFFF) is no object.
pub fn object_id(v: i32) -> Option<u32> {
    if v == NONE { None } else { Some(v as u32) }
}

/// The register value for an object datum. The datum 0xFFFF_FFFF is
/// `none`.
pub fn object_value(id: u32) -> i32 {
    id as i32
}

/// The table sizes a value's validity depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub strings: usize,
    pub tags: usize,
    pub labels: usize,
    /// Variant count of each declared enum, by enum index.
    pub enum_variants: Vec<usize>,
}

/// 16.16 fixed point helpers. All saturate.
pub mod fixed {
    pub const ONE: i32 = 1 << 16;

    pub fn from_int(v: i32) -> i32 {
        ((v as i64) << 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    /// Truncates toward negative infinity.
    pub fn to_int(v: i32) -> i32 {
        v >> 16
    }

    pub fn mul(a: i32, b: i32) -> i32 {
        (((a as i64) * (b as i64)) >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    /// Division by zero gives 0.
    pub fn div(a: i32, b: i32) -> i32 {
        if b == 0 {
            return 0;
        }
        (((a as i64) << 16) / (b as i64)).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    /// The length of a vector of three 16.16 components, as 16.16: the
    /// square root of the sum of the raw squares, taken whole in 128 bits
    /// so no square overflows, and saturated to the largest fixed value.
    pub fn length(x: i32, y: i32, z: i32) -> i32 {
        let sq = |v: i32| i128::from(v) * i128::from(v);
        let sum = (sq(x) + sq(y) + sq(z)) as u128;
        isqrt(sum).min(i32::MAX as u128) as i32
    }

    /// The floor of the square root, by Newton's steps down from a power of
    /// two above it.
    fn isqrt(n: u128) -> u128 {
        if n < 2 {
            return n;
        }
        let bits = 128 - n.leading_zeros();
        let mut x = 1u128 << bits.div_ceil(2);
        loop {
            let y = (x + n / x) / 2;
            if y >= x {
                return x;
            }
            x = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vectors_length_is_exact_and_never_overflows() {
        // 3-4-5 in world units.
        assert_eq!(fixed::length(fixed::from_int(3), fixed::from_int(4), 0), fixed::from_int(5));
        assert_eq!(fixed::length(0, 0, 0), 0);
        assert_eq!(fixed::length(-fixed::ONE, 0, 0), fixed::ONE);
        // 300 units on one axis: its square (90,000) is far past the 16.16
        // range, and the length is still 300.
        assert_eq!(fixed::length(fixed::from_int(300), 0, 0), fixed::from_int(300));
        assert_eq!(fixed::length(i32::MAX, i32::MAX, i32::MAX), i32::MAX);
        // The floor: 1 raw unit on two axes is 1.41 raw units.
        assert_eq!(fixed::length(1, 1, 0), 1);
    }

    #[test]
    fn codes_round_trip() {
        let all = [
            Ty::Int,
            Ty::Fixed,
            Ty::Bool,
            Ty::Player,
            Ty::Team,
            Ty::Object,
            Ty::Squad,
            Ty::Timer,
            Ty::Class,
            Ty::Tag,
            Ty::Label,
            Ty::Text,
            Ty::Enum(0),
            Ty::Enum(31),
        ];
        for ty in all {
            assert_eq!(Ty::from_code(ty.code()), Some(ty));
        }
        assert_eq!(Ty::from_code(12), None);
        assert_eq!(Ty::from_code(0x40 + 32), None);
    }

    #[test]
    fn fixed_saturates_and_divides_by_zero_to_zero() {
        assert_eq!(fixed::mul(fixed::from_int(3), fixed::ONE / 2), fixed::ONE * 3 / 2);
        assert_eq!(fixed::div(fixed::ONE, 0), 0);
        assert_eq!(fixed::from_int(i32::MAX), i32::MAX);
        assert_eq!(fixed::to_int(fixed::from_int(-7)), -7);
    }

    #[test]
    fn validity_follows_the_tables() {
        let sizes = Sizes {
            strings: 2,
            tags: 1,
            labels: 1,
            enum_variants: vec![3],
        };
        assert!(Ty::Player.valid(NONE, &sizes));
        assert!(!Ty::Player.valid(16, &sizes));
        assert!(Ty::Tag.valid(0, &sizes));
        assert!(!Ty::Tag.valid(1, &sizes));
        assert!(!Ty::Label.valid(NONE, &sizes));
        assert!(Ty::Enum(0).valid(2, &sizes));
        assert!(!Ty::Enum(0).valid(3, &sizes));
        assert!(!Ty::Enum(1).valid(0, &sizes));
        assert!(!Ty::Bool.valid(2, &sizes));
    }
}
