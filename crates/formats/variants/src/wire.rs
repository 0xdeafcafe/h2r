//! The variant record's wire codec: what the game's encode (`0x7cc50`) and
//! decode (`0x7d520`) put in and take from a bit stream when a variant
//! travels in a parameters update (docs/game/game-options.md, "How a
//! variant reaches other machines"), with the script index's empty type
//! area (docs/game/game-types-without-carriers.md, "No retail engine
//! object"). The layout and the bit stream's rules are read from the image
//! (docs/game/notes/2026-11-24-game-type-engine-reads.md, "GE1 and GE3"
//! and "GE2"); `crates/rewrites/engine/tests/variant_record_differential.rs`
//! checks both directions against the lifted original.
//!
//! The game's encoder has no bounds check on its type-area jump: an index
//! of 5, 6 or out of 1..9 jumps through a zero or a stray entry. Its
//! decoder refuses those. With the script index answered here, a script
//! record encodes with no type area and decodes with a zero one.

use crate::record::{self, CleanUp, ENGINE_AT, RECORD_SIZE, SCRIPT_INDEX, TYPE_AREA};

/// A guest bit stream (the reader `0x1959c0` and the writer `0x195720`
/// share it): `+0` the words, `+4` the size in bytes, `+0x10` the bit
/// position, `+0x14` a byte the name reader sets when a name has no end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bits {
    /// The stream's words, from its `+0` pointer: `size` bytes, rounded up
    /// to whole words.
    pub words: Vec<u32>,
    /// `+4`: the stream's size in bytes.
    pub size: u32,
    /// `+0x10`: the next bit, counted from bit 0 of the first word.
    pub pos: u32,
    /// `+0x14`.
    pub error: bool,
}

/// The stream's offsets.
pub const WORDS_AT: u32 = 0x00;
pub const SIZE_AT: u32 = 0x04;
pub const POS_AT: u32 = 0x10;
pub const ERROR_AT: u32 = 0x14;

impl Bits {
    /// A zeroed stream of `size` bytes at bit 0.
    pub fn new(size: u32) -> Self {
        Self {
            words: vec![0; size.div_ceil(4) as usize],
            size,
            pos: 0,
            error: false,
        }
    }

    /// The bits that fit of `count` at the position: the count, or what
    /// is left when that is less, and 0 or less past the end.
    fn fit(&self, count: u32) -> i32 {
        let left = (self.size.wrapping_shl(3) as i32).wrapping_sub(self.pos as i32);
        left.min(count as i32)
    }

    fn word(&self, i: i32) -> u32 {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.words.get(i))
            .copied()
            .unwrap_or(0)
    }

    fn set_word(&mut self, i: i32, v: u32) {
        if let Some(w) = usize::try_from(i).ok().and_then(|i| self.words.get_mut(i)) {
            *w = v;
        }
    }

    /// `0x1959c0`: `count` bits (1..=32), least significant first. Reads
    /// only what is left in the stream, as zeros above it, and moves the
    /// position by the whole count.
    pub fn read(&mut self, count: u32) -> u32 {
        let n = self.fit(count);
        let pos = self.pos as i32;
        let mut v = 0;
        if n > 0 {
            let (i, j, sh) = (pos >> 5, (pos + n - 1) >> 5, (pos & 31) as u32);
            v = if i == j {
                self.word(i) >> sh
            } else {
                (self.word(i + 1) << (32 - sh)) | (self.word(i) >> sh)
            };
            v &= u32::MAX >> (32 - n as u32);
        }
        self.pos = self.pos.wrapping_add(count);
        v
    }

    /// `0x195720`: the low `count` bits of `value` (1..=32). Writes only
    /// what is left in the stream and moves the position by the whole
    /// count. A write within one word ORs into it; one across two ORs into
    /// the first and stores the second.
    pub fn write(&mut self, value: u32, count: u32) {
        let n = self.fit(count);
        let pos = self.pos as i32;
        if n > 0 {
            let v = value & (u32::MAX >> (32 - n as u32));
            let (i, j, sh) = (pos >> 5, (pos + n - 1) >> 5, (pos & 31) as u32);
            self.set_word(i, self.word(i) | (v << sh));
            if i != j {
                self.set_word(i + 1, v >> (32 - sh));
            }
        }
        self.pos = self.pos.wrapping_add(count);
    }
}

/// How a field is stored in the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    U8,
    U16,
    U32,
}

/// One field on the wire: its offset, its store and its bit count.
type Field = (usize, Store, u32);

/// The fields after the name and the description, in wire order. The
/// counts are the bits of each field's limit in the clean-up.
const COMMON: &[Field] = &[
    (0x48, Store::U32, 15),
    (0x4c, Store::U32, 3),
    (0x50, Store::U32, 16),
    (0x54, Store::U32, 16),
    (0x58, Store::U32, 2),
    (0x74, Store::U32, 5),
    (0x78, Store::U32, 5),
    (0x7c, Store::U32, 16),
    (0x80, Store::U32, 16),
    (0x84, Store::U32, 16),
    (0x88, Store::U32, 2),
    (0xa4, Store::U32, 2),
    (0xa8, Store::U32, 2),
    (0xac, Store::U32, 16),
    (0xb4, Store::U32, 4),
    (0xcc, Store::U8, 2),
    (0xcd, Store::U8, 3),
    (0xce, Store::U8, 3),
    (0xcf, Store::U8, 3),
    (0xd0, Store::U8, 3),
    (0xd1, Store::U8, 3),
    (0xd2, Store::U8, 3),
    (0xd3, Store::U8, 3),
    (0xd4, Store::U8, 5),
    (0xd5, Store::U8, 2),
    (0xd6, Store::U8, 5),
    (0xd7, Store::U8, 5),
];

const CTF: &[Field] = &[
    (0xf0, Store::U32, 8),
    (0xf4, Store::U32, 16),
    (0xf8, Store::U32, 2),
    (0xfc, Store::U32, 1),
    (0x100, Store::U32, 2),
    (0x104, Store::U32, 2),
];
const ASSAULT_HEAD: &[Field] = &[(0x108, Store::U16, 16), (0x10a, Store::U16, 16)];
const SLAYER: &[Field] = &[(0xf0, Store::U32, 3)];
const ODDBALL: &[Field] = &[
    (0xf0, Store::U32, 3),
    (0xf4, Store::U16, 2),
    (0xf6, Store::U16, 1),
    (0xf8, Store::U16, 2),
    (0xfa, Store::U16, 2),
];
const KING: &[Field] = &[(0xf0, Store::U32, 5), (0xf4, Store::U16, 16)];
const JUGGERNAUT: &[Field] = &[(0xf0, Store::U32, 7), (0xf4, Store::U16, 2)];
const TERRITORIES: &[Field] = &[
    (0xf0, Store::U16, 4),
    (0xf2, Store::U16, 16),
    (0xf4, Store::U16, 16),
];

/// The type area's fields for `engine`, in wire order: the decode's jump
/// table at `0x7da38`, the encode's at `0x7d4f8`. `None` for an index the
/// game has no type area for (5, 6 and anything out of 1..9); the script
/// index has an empty one, answered by the caller.
fn type_area(engine: i32) -> Option<Vec<Field>> {
    Some(match engine {
        1 => CTF.to_vec(),
        2 => SLAYER.to_vec(),
        3 => ODDBALL.to_vec(),
        4 => KING.to_vec(),
        7 => JUGGERNAUT.to_vec(),
        8 => TERRITORIES.to_vec(),
        9 => [ASSAULT_HEAD, CTF].concat(),
        _ => return None,
    })
}

/// The fields after the description that `engine`'s record carries, in
/// wire order, as `(offset, bytes, bits)`: the common ones, then the type
/// area's (none for the script index). `None` for an index with no type
/// area.
pub fn fields(engine: i32) -> Option<Vec<(usize, usize, u32)>> {
    let area = if engine == SCRIPT_INDEX {
        Vec::new()
    } else {
        type_area(engine)?
    };
    Some(
        COMMON
            .iter()
            .chain(&area)
            .map(|&(at, s, bits)| {
                let bytes = match s {
                    Store::U8 => 1,
                    Store::U16 => 2,
                    Store::U32 => 4,
                };
                (at, bytes, bits)
            })
            .collect(),
    )
}

fn load(r: &[u8; RECORD_SIZE], (at, store, _): Field) -> u32 {
    match store {
        Store::U8 => u32::from(r[at]),
        Store::U16 => u32::from(u16::from_le_bytes([r[at], r[at + 1]])),
        Store::U32 => u32::from_le_bytes(r[at..at + 4].try_into().unwrap()),
    }
}

fn store(r: &mut [u8; RECORD_SIZE], (at, store, _): Field, v: u32) {
    match store {
        Store::U8 => r[at] = v as u8,
        Store::U16 => r[at..at + 2].copy_from_slice(&(v as u16).to_le_bytes()),
        Store::U32 => r[at..at + 4].copy_from_slice(&v.to_le_bytes()),
    }
}

fn engine_of(r: &[u8; RECORD_SIZE]) -> i32 {
    i32::from_le_bytes(r[ENGINE_AT..ENGINE_AT + 4].try_into().unwrap())
}

/// The name's units on the wire (32, from `+4`).
const NAME_UNITS: usize = record::NAME_UNITS;

/// Why a record has no encoding: the game's encoder would jump through a
/// zero or stray entry of its type-area table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("engine {0} has no type area on the wire")]
pub struct NoEncoding(pub i32);

/// `0x7cc50` for `r` into `bits`, and the script index with no type area.
/// Every value is cut to its bit count, as the game's writer does.
pub fn encode(r: &[u8; RECORD_SIZE], bits: &mut Bits) -> Result<(), NoEncoding> {
    let engine = engine_of(r);
    let area = match engine {
        0 => None,
        SCRIPT_INDEX => Some(Vec::new()),
        e => Some(type_area(e).ok_or(NoEncoding(e))?),
    };
    bits.write(engine as u32, 4);
    let Some(area) = area else {
        return Ok(());
    };
    bits.write(load(r, (0, Store::U16, 1)), 1);
    for k in 0..NAME_UNITS {
        let u = load(r, (record::NAME_AT + 2 * k, Store::U16, 16));
        bits.write(u, 16);
        if u == 0 {
            break;
        }
    }
    bits.write((r[record::DESCRIPTION_AT] as i8 as i32 + 1) as u32, 7);
    for &f in COMMON.iter().chain(&area) {
        bits.write(load(r, f), f.2);
    }
    Ok(())
}

/// What the decode answers (AL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// AL 1: the record decoded and the clean-up keeps it (or the index
    /// was 0, which zeroes the record).
    Kept,
    /// AL 0: the clean-up would change it, or the index has no type area.
    Refused,
}

/// `0x7d520` into `out` from `bits`, and the script index with a zero
/// type area. It writes the fields it reads and nothing else; the clean-up
/// at the end runs on a copy (for the script index, [`record::clean_up`]).
pub fn decode(out: &mut [u8; RECORD_SIZE], bits: &mut Bits) -> Decoded {
    let engine = bits.read(4);
    out[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&engine.to_le_bytes());
    if engine == 0 {
        out.fill(0);
        return Decoded::Kept;
    }
    store(out, (0, Store::U16, 1), bits.read(1));
    let mut ended = false;
    for k in 0..NAME_UNITS {
        let u = bits.read(16);
        store(out, (record::NAME_AT + 2 * k, Store::U16, 16), u);
        if u == 0 {
            ended = true;
            break;
        }
    }
    if !ended {
        store(
            out,
            (record::NAME_AT + 2 * (NAME_UNITS - 1), Store::U16, 16),
            0,
        );
        bits.error = true;
    }
    out[record::DESCRIPTION_AT] = (bits.read(7) as u8).wrapping_sub(1);
    for &f in COMMON {
        store(out, f, bits.read(f.2));
    }
    let engine = engine as i32;
    if engine == SCRIPT_INDEX {
        out[TYPE_AREA].fill(0);
    } else {
        let Some(area) = type_area(engine) else {
            return Decoded::Refused;
        };
        for f in area {
            store(out, f, bits.read(f.2));
        }
    }
    let mut copy = *out;
    let kept = match record::clean_up(&mut copy) {
        CleanUp::Clean => true,
        CleanUp::TypeAreaCleared | CleanUp::ResetAsSlayer => false,
        CleanUp::Retail => {
            let v = record::GameVariant::from_bytes(&copy).unwrap();
            record::check(&v).is_empty()
        }
    };
    if kept {
        Decoded::Kept
    } else {
        Decoded::Refused
    }
}

/// The engine index at the stream's position, without moving it: what the
/// decode reads first.
pub fn peek_engine(bits: &Bits) -> u32 {
    bits.clone().read(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{GameType, GameVariant};

    #[test]
    fn bits_go_least_significant_first_and_across_words() {
        let mut b = Bits::new(8);
        b.write(0b101, 3);
        b.write(0xffff_ffff, 30);
        assert_eq!(b.pos, 33);
        assert_eq!(b.words, [0xffff_fffd, 1]);
        b.pos = 0;
        assert_eq!(b.read(3), 0b101);
        assert_eq!(b.read(30), 0x3fff_ffff);
        // At the end: what is left, and the position moves by the whole
        // count; past it, nothing is written.
        b.pos = 60;
        assert_eq!(b.read(8), 0);
        assert_eq!(b.pos, 68);
        b.write(0xff, 8);
        assert_eq!(b.words, [0xffff_fffd, 1]);
    }

    #[test]
    fn a_write_across_words_stores_the_second() {
        let mut b = Bits::new(8);
        b.words = [0, 0xdead_beef].to_vec();
        b.pos = 30;
        b.write(0b1111, 4);
        assert_eq!(b.words, [0xc000_0000, 0b11]);
    }

    fn script_record() -> [u8; RECORD_SIZE] {
        let mut b = GameVariant::stock(GameType::Slayer).unwrap().to_bytes();
        b[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&SCRIPT_INDEX.to_le_bytes());
        b[TYPE_AREA].fill(0);
        b
    }

    /// The bytes the wire carries of `r`: everything the decode writes.
    fn carried(r: &[u8; RECORD_SIZE]) -> Vec<(usize, u8)> {
        let mut out = Vec::new();
        let mut zero = [0u8; RECORD_SIZE];
        let mut ones = [0xffu8; RECORD_SIZE];
        let mut bits = Bits::new(0x200);
        encode(r, &mut bits).unwrap();
        for buf in [&mut zero, &mut ones] {
            bits.pos = 0;
            decode(buf, &mut bits.clone());
        }
        for i in 0..RECORD_SIZE {
            if zero[i] == ones[i] {
                out.push((i, zero[i]));
            }
        }
        out
    }

    #[test]
    fn a_script_record_round_trips_with_no_type_area() {
        let r = script_record();
        let mut bits = Bits::new(0x200);
        encode(&r, &mut bits).unwrap();
        let mut slayer = r;
        slayer[ENGINE_AT] = 2;
        let mut as_slayer = Bits::new(0x200);
        encode(&slayer, &mut as_slayer).unwrap();
        assert_eq!(as_slayer.pos, bits.pos + 3, "Slayer's 3 type-area bits");

        // The decode writes only what it reads, so the clean-up that ends it
        // sees the buffer's other bytes (`+0xb0` must be 0): start from a
        // valid record whose type area holds junk.
        let mut out = r;
        out[TYPE_AREA].fill(0x55);
        bits.pos = 0;
        assert_eq!(decode(&mut out, &mut bits), Decoded::Kept);
        assert_eq!(out[TYPE_AREA], [0; 0x1c], "the type area is zero");
        for (i, x) in carried(&r) {
            assert_eq!(out[i], x, "{i:#x}");
            assert_eq!(r[i], x, "{i:#x}");
        }
        assert_eq!(
            peek_engine(&{
                let mut b = Bits::new(4);
                b.write(6, 4);
                b.pos = 0;
                b
            }),
            6
        );
    }

    #[test]
    fn indexes_with_no_type_area_have_no_encoding_and_fail_the_decode() {
        for e in [5, 10, -1] {
            let mut r = script_record();
            r[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&i32::to_le_bytes(e));
            assert_eq!(encode(&r, &mut Bits::new(0x200)), Err(NoEncoding(e)));
        }
        let mut bits = Bits::new(0x200);
        bits.write(10, 4);
        bits.pos = 0;
        assert_eq!(decode(&mut [0; RECORD_SIZE], &mut bits), Decoded::Refused);
        let mut bits = Bits::new(0x200);
        bits.write(0, 4);
        bits.pos = 0;
        let mut out = [7u8; RECORD_SIZE];
        assert_eq!(decode(&mut out, &mut bits), Decoded::Kept);
        assert_eq!(out, [0; RECORD_SIZE]);
    }

    #[test]
    fn every_stock_record_round_trips() {
        for ty in GameType::ALL {
            let r = GameVariant::stock(ty).unwrap().to_bytes();
            let mut bits = Bits::new(0x200);
            encode(&r, &mut bits).unwrap();
            bits.pos = 0;
            let mut out = [0u8; RECORD_SIZE];
            assert_eq!(decode(&mut out, &mut bits), Decoded::Kept, "{ty:?}");
            assert_eq!(out, r, "{ty:?}: the stock records carry no spare byte");
        }
    }

    #[test]
    fn a_name_with_no_end_is_cut_and_marked() {
        let mut r = script_record();
        for k in 0..NAME_UNITS {
            r[record::NAME_AT + 2 * k] = b'a';
        }
        let mut bits = Bits::new(0x200);
        encode(&r, &mut bits).unwrap();
        bits.pos = 0;
        let mut out = [0u8; RECORD_SIZE];
        decode(&mut out, &mut bits);
        assert!(bits.error);
        assert_eq!(out[0x42..0x44], [0, 0]);
        assert_eq!(out[0x40], b'a');
    }
}
