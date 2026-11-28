//! A damage part's record, as the `jpt!` arm of `0x17a8a0` builds it.
//!
//! The arm calls the damage data init `0xd6660` on a record in its frame
//! (`[esp+0x34]`), copies the effect's owner and the part's location into
//! it, and calls the area damage `0xd6c80(record, -1)`. The init clears
//! `0x22` dwords and sets the empty fields; the arm's copies then overwrite
//! some of them (the dword at `+0x20` covers the init's `+0x22`). The bytes
//! below are the record `0xd6c80` reads.

/// The record's size: `0xd6660` clears `0x22` dwords.
pub const SIZE: usize = 0x88;

/// The globals the init reads.
pub const INIT_WORD_7C: u32 = 0x0047_d8e0;
/// Read by the init into `+0x22`; the arm's `+0x20` dword overwrites it.
pub const INIT_WORD_22: u32 = 0x0046_86c4;
/// `1.0`, into `+0x54`, `+0x58` and `+0x5c`.
pub const INIT_ONE: u32 = 0x0045_dbc0;

/// Everything the arm's record is built from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The part's damage effect tag (`[part + 0x10]`).
    pub tag: u32,
    /// The effect datum's `+0x4c..+0x58` (its owner) and `+0x1c`, `+0x20`.
    pub owner: [u32; 3],
    pub effect_1c: u32,
    pub effect_20: u32,
    /// The three location words: the position and direction vectors (as
    /// bits) and the scale.
    pub position: [u32; 3],
    pub direction: [u32; 3],
    pub scale: u32,
    /// The init's globals, read when the part is entered.
    pub word_7c: u16,
    pub word_22: u16,
    pub one: u32,
}

/// The record bytes, little-endian, as the guest lays them out.
pub type Record = [u8; SIZE];

fn put32(r: &mut Record, at: usize, v: u32) {
    r[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(r: &mut Record, at: usize, v: u16) {
    r[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// The record `0xd6c80` is called with.
pub fn record(i: &Inputs) -> Record {
    let mut r = [0u8; SIZE];
    // 0xd6660, in its store order.
    put32(&mut r, 0x00, i.tag);
    put16(&mut r, 0x7c, i.word_7c);
    put32(&mut r, 0x0c, u32::MAX);
    put32(&mut r, 0x08, u32::MAX);
    put16(&mut r, 0x10, u16::MAX);
    put32(&mut r, 0x14, u32::MAX);
    put32(&mut r, 0x1c, u32::MAX);
    put16(&mut r, 0x20, u16::MAX);
    put16(&mut r, 0x22, i.word_22);
    put32(&mut r, 0x18, u32::MAX);
    for at in [0x54, 0x58, 0x5c] {
        put32(&mut r, at, i.one);
    }
    put16(&mut r, 0x7e, u16::MAX);
    r[0x84] = 0;
    // The arm (0x17a9a1..0x17aa43), in its store order.
    for (n, w) in i.owner.iter().enumerate() {
        put32(&mut r, 0x08 + 4 * n, *w);
    }
    put32(&mut r, 0x1c, i.effect_1c);
    put32(&mut r, 0x20, i.effect_20);
    for (n, w) in i.position.iter().enumerate() {
        put32(&mut r, 0x30 + 4 * n, *w);
        put32(&mut r, 0x24 + 4 * n, *w);
    }
    for (n, w) in i.direction.iter().enumerate() {
        put32(&mut r, 0x3c + 4 * n, *w);
    }
    put32(&mut r, 0x6c, i.direction[0]);
    put32(&mut r, 0x54, i.scale);
    put32(&mut r, 0x70, i.direction[1]);
    put32(&mut r, 0x74, i.direction[2]);
    r
}

/// The second argument of `0xd6c80`: no object is excepted.
pub const NO_OBJECT: u32 = u32::MAX;

/// Where the arm keeps the record, below the switch's entry `ESP` (its
/// `sub esp, 0x100` and four pushes, then `[esp+0x34]`).
pub const FRAME: u32 = 0x110;
pub const RECORD_BELOW_ENTRY: u32 = FRAME - 0x34;

/// What the arm passes to `0xd6c80`: the record and the excepted object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub record: Record,
    pub except: u32,
}

impl Call {
    pub fn predict(i: &Inputs) -> Call {
        Call { record: record(i), except: NO_OBJECT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> Inputs {
        Inputs {
            tag: 0xe1a2_0031,
            owner: [0x0003_0002, 0x0004_0005, 0xffff_0007],
            effect_1c: 0x1111_2222,
            effect_20: 0x3333_4444,
            position: [1.5f32.to_bits(), (-2.0f32).to_bits(), 7.25f32.to_bits()],
            direction: [0.0f32.to_bits(), (-0.0f32).to_bits(), 0x7fc0_0001],
            scale: 0.75f32.to_bits(),
            word_7c: 0x0102,
            word_22: 0xbeef,
            one: 1.0f32.to_bits(),
        }
    }

    fn u32_at(r: &Record, at: usize) -> u32 {
        u32::from_le_bytes(r[at..at + 4].try_into().unwrap())
    }

    /// The arm's stores over the init's, field by field, as the
    /// disassembly of `0x17a98a..0x17aa4a` and `0xd6660` lay them out.
    #[test]
    fn the_record_is_the_arms() {
        let i = inputs();
        let r = record(&i);
        assert_eq!(u32_at(&r, 0), i.tag);
        assert_eq!([u32_at(&r, 8), u32_at(&r, 0xc), u32_at(&r, 0x10)], i.owner);
        assert_eq!(u32_at(&r, 0x14), u32::MAX);
        assert_eq!(u32_at(&r, 0x18), u32::MAX);
        assert_eq!(u32_at(&r, 0x1c), i.effect_1c);
        // The dword at +0x20 covers the init's +0x22 word.
        assert_eq!(u32_at(&r, 0x20), i.effect_20);
        assert_eq!([u32_at(&r, 0x24), u32_at(&r, 0x28), u32_at(&r, 0x2c)], i.position);
        assert_eq!([u32_at(&r, 0x30), u32_at(&r, 0x34), u32_at(&r, 0x38)], i.position);
        assert_eq!([u32_at(&r, 0x3c), u32_at(&r, 0x40), u32_at(&r, 0x44)], i.direction);
        assert_eq!([u32_at(&r, 0x6c), u32_at(&r, 0x70), u32_at(&r, 0x74)], i.direction);
        assert_eq!(u32_at(&r, 0x54), i.scale);
        assert_eq!([u32_at(&r, 0x58), u32_at(&r, 0x5c)], [i.one, i.one]);
        assert_eq!(&r[0x7c..0x80], &[0x02, 0x01, 0xff, 0xff]);
        // Everything else is the init's zero.
        let written: Vec<std::ops::Range<usize>> =
            vec![0..4, 8..0x48, 0x54..0x60, 0x6c..0x78, 0x7c..0x80];
        for (at, b) in r.iter().enumerate() {
            if !written.iter().any(|w| w.contains(&at)) {
                assert_eq!(*b, 0, "byte {at:#x}");
            }
        }
        // A NaN payload and a negative zero are kept as bits.
        assert_eq!(u32_at(&r, 0x44), 0x7fc0_0001);
        assert_eq!(u32_at(&r, 0x40), 0x8000_0000);
    }
}
