//! The owner callbacks of a simulated sound (docs/engine/sound-sim.md,
//! "Owner callbacks"): the finish that clears a unit's dialogue word, and
//! the per-tick lip-sync update.
//!
//! A start's request block names an owner (`+0x50`, when flag `0x08` of its
//! first byte is set) and a callback table (`+0x58`, flag `0x20`). The
//! mixer calls table `+8` every update with the owner, the instance's
//! payload, the tag, the pitch range, the current permutation and the audio
//! time (`0x129593`), and table `+0x10` once when the sound finishes
//! (`0x127360`, reason 4 at `0x1294b7`). The sim holds no instance, so the
//! callbacks that read one are not called with a sim handle:
//!
//! - the update `0x18c6a0` reads no instance; it is called from the sim;
//! - the finishes `0x18c8c0` and `0x18c910` read the instance's tag from the
//!   pool, so their effect on guest memory is made here instead
//!   ([`finish`]), word for word.
//!
//! A table holding any other function is counted, not called.

use engine::{GuestMemory, points};

pub const NONE: u32 = u32::MAX;
/// Table `+8`: the lip-sync update the whitelist allows.
pub const UPDATE_LIP_SYNC: u32 = 0x18c6a0;
/// Table `+0x10`: the unit's dialogue word is cleared.
pub const FINISH_UNIT: u32 = 0x18c8c0;
/// Table `+0x10`: the same, and the dialogue-table slot holding the handle.
pub const FINISH_DIALOGUE: u32 = 0x18c910;
/// The table a marker start (`0x1891d0`) gives its owner (`0x444afc`; the
/// other, `0x444b1c`, holds the same `+8` and `+0x10`).
pub const MARKER_TABLE: u32 = 0x0044_4afc;

const TABLE_UPDATE: u32 = 8;
const TABLE_FINISH: u32 = 0x10;
const REQUEST_OWNER: u32 = 0x50;
const REQUEST_TABLE: u32 = 0x58;
const FLAG_OWNER: u8 = 0x08;
const FLAG_TABLE: u8 = 0x20;
const GESTALT: u32 = 0x0051_ebd4;
const OBJECTS: u32 = 0x004e_0300;
/// `[0x4ed28c] + 0x29`: the dialogue switch the finishes test.
const DIALOGUE_GLOBALS: u32 = 0x004e_d28c;
const DIALOGUE_ENABLED: u32 = 0x29;
/// `[0x4ed288] + 0x128`: sixteen slots of tag, start tick, instance, ...
const DIALOGUE_TABLE: u32 = 0x004e_d288;
const SLOT_BASE: u32 = 0x128;
const SLOT_STRIDE: u32 = 0x10;
const SLOTS: u32 = 16;
/// The unit's dialogue word, at the offset a word at `+0x33e` gives, `+0x38`.
const UNIT_FACE_OFFSET: u32 = 0x33e;
const UNIT_DIALOGUE_WORD: u32 = 0x38;
/// Object types a finish accepts: bits 0 and 1 (`push 3`, `0xbadc0`).
const UNIT_TYPES: u32 = 3;
/// The finish reason of a sound that played out (`push 4`, `0x1294b7`).
pub const FINISH_REASON: u32 = 4;
const TICKS_PER_SECOND: f32 = 30.0;

/// What the sim keeps of a sound's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owned {
    pub owner: u32,
    /// The callback table's address.
    pub table: u32,
    pub tag: u32,
    /// The pitch range, relative to the tag's first.
    pub range: i32,
    /// The permutation record the update is told is playing; 0 for none.
    pub permutation: u32,
    pub start: u32,
}

impl Owned {
    pub const WORDS: usize = 6;

    pub fn words(&self) -> [u32; Self::WORDS] {
        [self.owner, self.table, self.tag, self.range as u32, self.permutation, self.start]
    }

    pub fn from_words(w: [u32; Self::WORDS]) -> Self {
        Owned { owner: w[0], table: w[1], tag: w[2], range: w[3] as i32, permutation: w[4], start: w[5] }
    }
}

fn word(m: &dyn GuestMemory, a: u32) -> Option<u32> {
    m.u32(a).ok()
}

/// The owner a start's request block (`SOUND_IMPULSE_ALLOCATE`'s third
/// argument) names, if it names a callback table.
pub fn read_request(m: &dyn GuestMemory, request: u32, tag: u32, tick: u32) -> Option<Owned> {
    let flags = m.u8(request).ok()?;
    if flags & FLAG_TABLE == 0 {
        return None;
    }
    let table = word(m, request.wrapping_add(REQUEST_TABLE)).filter(|&t| t != 0)?;
    let owner = if flags & FLAG_OWNER != 0 { word(m, request.wrapping_add(REQUEST_OWNER))? } else { NONE };
    Some(with_permutation(m, Owned { owner, table, tag, range: 0, permutation: 0, start: tick }))
}

/// The owner of a marker start: the object it starts at.
pub fn marker(m: &dyn GuestMemory, object: u32, tag: u32, tick: u32) -> Owned {
    with_permutation(m, Owned { owner: object, table: MARKER_TABLE, tag, range: 0, permutation: 0, start: tick })
}

/// Chooses the permutation of the tag's first pitch range the update is
/// told is playing: `hash(tag, start) % count`. The engine draws it from
/// the guest's random stream at admission (`0x125260`); the sim does not
/// replay that draw.
fn with_permutation(m: &dyn GuestMemory, mut o: Owned) -> Owned {
    o.permutation = (|| {
        let entries = word(m, points::TAG_ENTRIES.base)?;
        let data = word(m, entries.wrapping_add((o.tag & 0xffff) * 0x10 + 8)).filter(|&d| d != 0)?;
        let first_range = m.u16(data.wrapping_add(8)).ok()? as i16 as i32 as u32;
        if m.u8(data.wrapping_add(0xa)).ok()? == 0 {
            return None;
        }
        let gestalt = word(m, GESTALT)?;
        let ranges = word(m, gestalt.wrapping_add(0x24))?;
        let perms = word(m, gestalt.wrapping_add(0x2c))?;
        let range = ranges.wrapping_add(first_range.wrapping_mul(12));
        let first = m.u16(range.wrapping_add(8)).ok()? as i16 as i32 as u32;
        let count = u32::from(m.u16(range.wrapping_add(10)).ok()?);
        if count == 0 {
            return None;
        }
        let pick = (o.tag.wrapping_mul(0x9e37_79b1) ^ o.start.wrapping_mul(0x85eb_ca6b)) % count;
        Some(perms.wrapping_add(first.wrapping_add(pick).wrapping_mul(16)))
    })()
    .unwrap_or(0);
    o
}

/// The guest call a running owned sound gets this tick, if its table's `+8`
/// is the lip-sync update: `(function, arguments)`.
pub fn update_call(m: &dyn GuestMemory, o: &Owned, tick: u32) -> Option<(u32, [u32; 6])> {
    let f = word(m, o.table.wrapping_add(TABLE_UPDATE)).filter(|&f| f != 0)?;
    if f != UPDATE_LIP_SYNC || o.permutation == 0 {
        return None;
    }
    let seconds = tick.saturating_sub(o.start) as f32 / TICKS_PER_SECOND;
    Some((f, [o.owner, 0, o.tag, o.range as u32, o.permutation, seconds.to_bits()]))
}

/// What a finish did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// The table holds no finish.
    None,
    /// A finish the sim made itself.
    Made,
    /// A finish the sim does not know; it was not called.
    Unsupported,
}

/// The effect of the table's finish on guest memory, for the sound `handle`
/// (`0x18c8c0`, `0x18c910`).
pub fn finish(m: &mut dyn GuestMemory, o: &Owned, handle: u32) -> Finish {
    let Some(f) = word(m, o.table.wrapping_add(TABLE_FINISH)).filter(|&f| f != 0) else {
        return Finish::None;
    };
    match f {
        FINISH_UNIT => {
            clear_dialogue_word(m, o);
            Finish::Made
        }
        FINISH_DIALOGUE => {
            if o.owner != NONE {
                clear_dialogue_word(m, o);
            }
            clear_slots(m, handle);
            Finish::Made
        }
        _ => Finish::Unsupported,
    }
}

/// `0x18c8c0`: with the dialogue switch on and the owner a unit, a unit
/// whose dialogue word is this sound's tag has it cleared (`0xd0dc0`).
fn clear_dialogue_word(m: &mut dyn GuestMemory, o: &Owned) {
    let _ = (|| -> Option<()> {
        let globals = word(m, DIALOGUE_GLOBALS)?;
        if m.u8(globals.wrapping_add(DIALOGUE_ENABLED)).ok()? == 0 {
            return None;
        }
        let object = object_data(m, o.owner, UNIT_TYPES)?;
        let face = m.u16(object.wrapping_add(UNIT_FACE_OFFSET)).ok()? as i16 as i32 as u32;
        let cell = object.wrapping_add(face).wrapping_add(UNIT_DIALOGUE_WORD);
        if word(m, cell)? == o.tag {
            m.set_u32(cell, NONE).ok()?;
        }
        Some(())
    })();
}

/// `0x18c910`: the dialogue-table slots holding this instance are freed.
fn clear_slots(m: &mut dyn GuestMemory, handle: u32) {
    let _ = (|| -> Option<()> {
        let table = word(m, DIALOGUE_TABLE)?;
        for i in 0..SLOTS {
            let slot = table.wrapping_add(SLOT_BASE + i * SLOT_STRIDE);
            if word(m, slot.wrapping_add(8))? == handle {
                m.set_u32(slot, NONE).ok()?;
                m.set_u32(slot.wrapping_add(8), NONE).ok()?;
                return Some(());
            }
        }
        Some(())
    })();
}

/// `0xbadc0`: the data of the object `datum` names, if its type is in `mask`.
fn object_data(m: &dyn GuestMemory, datum: u32, mask: u32) -> Option<u32> {
    if datum == NONE {
        return None;
    }
    let objects = word(m, OBJECTS)?;
    let index = datum & 0xffff;
    if index >= word(m, objects.wrapping_add(0x38))? {
        return None;
    }
    let elem = word(m, objects.wrapping_add(0x24))?.wrapping_mul(index).wrapping_add(word(m, objects.wrapping_add(0x44))?);
    let salt = m.u16(elem).ok()? as i16 as i32;
    if salt == 0 || salt != (datum as i32 >> 16) {
        return None;
    }
    let kind = m.u8(elem.wrapping_add(3)).ok()?;
    if kind >= 32 || mask & (1 << kind) == 0 {
        return None;
    }
    word(m, elem.wrapping_add(8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const ENTRIES: u32 = 0x0060_0000;
    const TAGDATA: u32 = 0x0060_1000;
    const GEST: u32 = 0x0060_2000;
    const RANGES: u32 = 0x0060_3000;
    const PERMS: u32 = 0x0060_4000;
    const REQUEST: u32 = 0x0060_5000;
    const TABLE: u32 = 0x0060_6000;
    const OBJ_HDR: u32 = 0x0060_7000;
    const OBJ_DATA: u32 = 0x0060_8000;
    const GLOBALS: u32 = 0x0060_9000;
    const DIALOGUE: u32 = 0x0060_a000;
    const UNIT: u32 = 0x0060_b000;
    const TAG: u32 = 0xe1e1_0003;
    const OWNER: u32 = 0x1234_0002;

    fn guest(table_finish: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0051_0000, 0x1_0000).map(0x004e_0000, 0x1_0000).map(0x0060_0000, 0x1_0000);
        g.put_u32(points::TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 3 * 0x10 + 8, TAGDATA);
        // The tag: first pitch range 1, one range.
        g.put_u16(TAGDATA + 8, 1).put_u8(TAGDATA + 0xa, 1);
        g.put_u32(GESTALT, GEST).put_u32(GEST + 0x24, RANGES).put_u32(GEST + 0x2c, PERMS);
        // Range 1: first permutation 5, 4 permutations.
        g.put_u16(RANGES + 12 + 8, 5).put_u16(RANGES + 12 + 10, 4);
        // The request: flags owner|table, owner, table.
        g.put_u8(REQUEST, FLAG_OWNER | FLAG_TABLE).put_u32(REQUEST + 0x50, OWNER).put_u32(REQUEST + 0x58, TABLE);
        g.put_u32(TABLE + 8, UPDATE_LIP_SYNC).put_u32(TABLE + 0x10, table_finish);
        // Objects: element size 12, 4 slots; slot 2 holds the unit, salt 0x1234, type 0.
        g.put_u32(OBJECTS, OBJ_HDR).put_u32(OBJ_HDR + 0x38, 4).put_u32(OBJ_HDR + 0x24, 12).put_u32(OBJ_HDR + 0x44, OBJ_DATA);
        g.put_u16(OBJ_DATA + 24, 0x1234).put_u8(OBJ_DATA + 24 + 3, 0).put_u32(OBJ_DATA + 24 + 8, UNIT);
        // The unit: its face block at +0x40, whose dialogue word holds the tag.
        g.put_u16(UNIT + 0x33e, 0x40).put_u32(UNIT + 0x40 + 0x38, TAG);
        g.put_u32(DIALOGUE_GLOBALS, GLOBALS).put_u8(GLOBALS + 0x29, 1);
        g.put_u32(DIALOGUE_TABLE, DIALOGUE);
        g
    }

    #[test]
    fn a_request_block_names_its_owner_table_and_permutation() {
        let g = guest(FINISH_UNIT);
        let o = read_request(&g, REQUEST, TAG, 40).unwrap();
        assert_eq!((o.owner, o.table, o.tag, o.range, o.start), (OWNER, TABLE, TAG, 0, 40));
        // Permutation 5..9 of the permutation array, 16 bytes each.
        assert!((PERMS + 5 * 16..PERMS + 9 * 16).contains(&o.permutation) && o.permutation % 16 == 0);
        // The same start picks the same one.
        assert_eq!(read_request(&g, REQUEST, TAG, 40), Some(o));
        // No table flag, no owner.
        let mut g2 = guest(FINISH_UNIT);
        g2.put_u8(REQUEST, FLAG_OWNER);
        assert_eq!(read_request(&g2, REQUEST, TAG, 40), None);
        // No owner flag: the owner is none but the table stays.
        g2.put_u8(REQUEST, FLAG_TABLE);
        assert_eq!(read_request(&g2, REQUEST, TAG, 40).unwrap().owner, NONE);
    }

    #[test]
    fn the_update_is_the_mixers_six_arguments_with_the_elapsed_seconds() {
        let g = guest(FINISH_UNIT);
        let o = read_request(&g, REQUEST, TAG, 40).unwrap();
        let (f, args) = update_call(&g, &o, 55).unwrap();
        assert_eq!(f, UPDATE_LIP_SYNC);
        assert_eq!(args, [OWNER, 0, TAG, 0, o.permutation, 0.5f32.to_bits()]);
        // A table with another update is not called.
        let mut g2 = guest(FINISH_UNIT);
        g2.put_u32(TABLE + 8, 0x1234);
        assert_eq!(update_call(&g2, &o, 55), None);
        // Nor is a sound with no permutation to name.
        let none = Owned { permutation: 0, ..o };
        assert_eq!(update_call(&g, &none, 55), None);
    }

    #[test]
    fn the_unit_finish_clears_the_dialogue_word_only_for_this_tag() {
        let mut g = guest(FINISH_UNIT);
        let o = read_request(&g, REQUEST, TAG, 40).unwrap();
        // The owner's slot is 2 and its salt 0x1234.
        assert_eq!(finish(&mut g, &o, 0x7000_0001), Finish::Made);
        assert_eq!(g.u32(UNIT + 0x40 + 0x38), Ok(NONE));
        // A unit speaking another line keeps it.
        let mut g = guest(FINISH_UNIT);
        g.put_u32(UNIT + 0x40 + 0x38, TAG + 1);
        finish(&mut g, &o, 0x7000_0001);
        assert_eq!(g.u32(UNIT + 0x40 + 0x38), Ok(TAG + 1));
        // The dialogue switch off leaves it too.
        let mut g = guest(FINISH_UNIT);
        g.put_u8(GLOBALS + 0x29, 0);
        finish(&mut g, &o, 0x7000_0001);
        assert_eq!(g.u32(UNIT + 0x40 + 0x38), Ok(TAG));
    }

    #[test]
    fn the_dialogue_finish_also_frees_the_slot_holding_the_handle() {
        let mut g = guest(FINISH_DIALOGUE);
        let slot = |i: u32| DIALOGUE + SLOT_BASE + i * SLOT_STRIDE;
        g.put_u32(slot(3), TAG).put_u32(slot(3) + 8, 0x7000_0001);
        g.put_u32(slot(4), TAG).put_u32(slot(4) + 8, 0x7000_0002);
        let o = read_request(&g, REQUEST, TAG, 40).unwrap();
        assert_eq!(finish(&mut g, &o, 0x7000_0001), Finish::Made);
        assert_eq!((g.u32(slot(3)), g.u32(slot(3) + 8)), (Ok(NONE), Ok(NONE)));
        assert_eq!((g.u32(slot(4)), g.u32(slot(4) + 8)), (Ok(TAG), Ok(0x7000_0002)));
        assert_eq!(g.u32(UNIT + 0x40 + 0x38), Ok(NONE));
    }

    #[test]
    fn a_finish_the_sim_does_not_know_is_counted_not_called() {
        let mut g = guest(0x0011_2233);
        let o = read_request(&g, REQUEST, TAG, 40).unwrap();
        assert_eq!(finish(&mut g, &o, 1), Finish::Unsupported);
        assert_eq!(g.u32(UNIT + 0x40 + 0x38), Ok(TAG));
        g.put_u32(TABLE + 0x10, 0);
        assert_eq!(finish(&mut g, &o, 1), Finish::None);
    }
}
