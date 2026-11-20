//! Type casting rules: the `0x004f5770` conversion table and its guards.
//!
//! Both the atom path (`0x0020a290`) and the return path (`0x00209ae0`)
//! convert a value from a *source* type to the node's expected type
//! (`+4`) with the same guard:
//!
//! - no conversion when the types match, the source is passthrough (3), or
//!   the destination is an object-name type (56..=61);
//! - an object destination (50..=55) converts only from an object-name
//!   source (through the object-name table, `0x000bb760`); any other source
//!   passes the raw handle through, so `unit` -> `object` is free;
//! - everything else indexes the 62x62 function table at `0x004f5770`
//!   (`[dest * 62 + source]`), filled by `0x0020a1a0`. Unfilled entries are
//!   null: the retail engine would call address 0. Here that is
//!   [`Cast::Invalid`].
//!
//! Each table entry's body is a few instructions, reproduced bit for bit,
//! including the slot bytes the narrow stores leave behind.
use super::value::{Cell, HsType};

/// What a conversion needs, after the guard and table lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cast {
    /// No conversion: the slot passes unchanged.
    Identity,
    /// A pure function of the slot bits.
    Pure(PureCast),
    /// Object-name index to object handle (`0x000bb760`).
    ObjectFromName,
    /// Object handle to a new one-element object list (`0x0020a170`).
    ObjectListFromObject,
    /// Object-name index to object, then to a list (`0x0020a130`).
    ObjectListFromName,
    /// AI reference to an object list of its actors (`0x00272ea0`).
    ObjectListFromAi,
    /// Null table entry: the engine would call address 0.
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureCast {
    /// Any value to void: `xor eax,eax` (`0x0011ca70`).
    Zero,
    /// Real or long to boolean, `0x0020a000`: `test eax,eax; sete al`. Note
    /// the sense: the boolean is **true when the source bits are zero**.
    /// The slot's upper three bytes keep the source bytes.
    BooleanFromDwordIsZero,
    /// Short to boolean, `0x0020a020`: `cmp word,0; sete al`.
    BooleanFromShortIsZero,
    /// String to boolean, `0x0020a040`: true when the string is empty.
    /// Needs the text, so the VM resolves it; the upper bytes become the
    /// low bytes of the length, as `sub eax,edx` leaves them.
    BooleanFromStringIsEmpty,
    /// Short to real, `0x0020a070`: `movsx; cvtsi2ss`.
    RealFromShort,
    /// Long to real, `0x0020a090`: `cvtsi2ss`.
    RealFromLong,
    /// Enumeration (44..=49) to real, `0x0020a0b0`: `movsx; inc; cvtsi2ss`.
    RealFromEnumPlusOne,
    /// Real to short, `0x0020a0d0`: `cvttss2si`, low word stored.
    ShortFromReal,
    /// Long to short, `0x0020a110`: the low word is rewritten unchanged.
    ShortFromLong,
    /// Real to long, `0x0020a0f0`: `cvttss2si`.
    LongFromReal,
    /// Short to long, `0x0020a100`: `movsx`.
    LongFromShort,
}

/// Classify a `source` -> `dest` conversion exactly as the guard and the
/// table initializer do.
pub fn classify(source: HsType, dest: HsType) -> Cast {
    if source == dest || source == HsType::PASSTHROUGH || dest.is_object_name() {
        return Cast::Identity;
    }
    if dest.is_object() {
        return if source.is_object_name() {
            Cast::ObjectFromName
        } else {
            Cast::Identity
        };
    }
    use PureCast::*;
    let pure = |p| Cast::Pure(p);
    match (dest.0, source.0) {
        (4, 5..=61) => pure(Zero),
        (5, 6) | (5, 8) => pure(BooleanFromDwordIsZero),
        (5, 7) => pure(BooleanFromShortIsZero),
        (5, 9) => pure(BooleanFromStringIsEmpty),
        (6, 7) => pure(RealFromShort),
        (6, 8) => pure(RealFromLong),
        (6, 44..=49) => pure(RealFromEnumPlusOne),
        (7, 6) => pure(ShortFromReal),
        (7, 8) => pure(ShortFromLong),
        (8, 6) => pure(LongFromReal),
        (8, 7) => pure(LongFromShort),
        (31, 50..=55) => Cast::ObjectListFromObject,
        (31, 56..=61) => Cast::ObjectListFromName,
        (31, 19) => Cast::ObjectListFromAi,
        _ => Cast::Invalid,
    }
}

/// `cvttss2si` / `fistp` result for an unrepresentable value: the x86
/// "integer indefinite".
const INDEFINITE: i32 = i32::MIN;

/// SSE `cvttss2si`: truncate toward zero; NaN and out-of-range give
/// `0x80000000` (Rust's `as` would saturate instead).
pub fn cvttss2si(r: f32) -> i32 {
    if r.is_nan() || r >= 2147483648.0 || r < -2147483648.0 {
        INDEFINITE
    } else {
        r as i32
    }
}

/// x87 `fistp` of a single under the default control word: round to
/// nearest, ties to even; unrepresentable values give `0x80000000`.
pub fn fistp(r: f32) -> i32 {
    let r = f64::from(r).round_ties_even();
    if r.is_nan() || r >= 2147483648.0 || r < -2147483648.0 {
        INDEFINITE
    } else {
        r as i32
    }
}

/// Apply a pure cast. `string_len` is only consulted for strings.
pub fn apply(cast: PureCast, c: Cell, string_len: impl FnOnce() -> usize) -> Cell {
    use PureCast::*;
    let low_byte = |orig: u32, b: bool| Cell((orig & !0xff) | b as u32);
    let low_word = |orig: u32, w: u16| Cell((orig & !0xffff) | u32::from(w));
    match cast {
        Zero => Cell::ZERO,
        BooleanFromDwordIsZero => low_byte(c.0, c.0 == 0),
        BooleanFromShortIsZero => low_byte(c.0, c.0 as u16 == 0),
        BooleanFromStringIsEmpty => {
            let n = string_len() as u32;
            low_byte(n, n == 0)
        }
        RealFromShort => Cell::from_real(f32::from(c.short())),
        RealFromLong => Cell::from_real(c.long() as f32),
        RealFromEnumPlusOne => Cell::from_real((i32::from(c.short()) + 1) as f32),
        ShortFromReal => low_word(c.0, cvttss2si(c.real()) as u16),
        ShortFromLong => c,
        LongFromReal => Cell::from_long(cvttss2si(c.real())),
        LongFromShort => Cell::from_long(i32::from(c.short())),
    }
}
