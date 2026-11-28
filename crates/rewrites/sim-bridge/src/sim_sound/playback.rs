//! A sound's custom playback row and its effect, read from tag data
//! (docs/engine/sound-instances.md, "The start layer").
//!
//! | Function | Answers |
//! |---|---|
//! | `0x2194a0(eax = definition)` | [`bank_row`]: the definition's bank row (`+0xd`, gestalt `+0x34`, `0x34` bytes a row), 0 for bank `0xff` |
//! | `0x18d170(ecx = tag)` | [`default_row`]: a `snd!` tag's bank row, else 0 |
//! | `0x18d090(ecx = tag, edx = custom)` | [`playback_row`]: the row `custom` names, by its top two bits |
//! | `0x21dd30(edi = tag, custom, &effect)` | [`effect_of`]: the row's effect (row `+0x2c` a count, `+0x30` the effect), when it has components |
//!
//! | `custom >> 30` | Row |
//! |---|---|
//! | `custom` is -1 | the tag's bank row ([`default_row`]) |
//! | 0 | a `snd!` tag's bank row, with no -1 test of the tag |
//! | 1 | the globals tag's list (`[0x4e034c] + 0xc4`, its `+0xc` when `+0xc0` is set) |
//! | 2 | the scenario's list (`[0x4e0350] + 0x324`) |
//! | 3 | none |
//!
//! A list's row is its definition's `+4` plus `0x38 * (custom & 0x3fffffff) + 4`.

use engine::memory::{GuestMemory, MemFault};
use engine::points::{GLOBALS_TAG, SCENARIO, TAG_ENTRIES};

const NONE: u32 = u32::MAX;
/// `[0x51ebd4]`: the sound gestalt.
pub const GESTALT: u32 = 0x0051_ebd4;
/// `snd!` as the tag entry's group word.
pub const SOUND_GROUP: u32 = 0x736e_6421;
const BANK: u32 = 0x34;
const LIST_ROW: u32 = 0x38;

/// A tag's definition: `[0x4e3b44] + 16 * (tag & 0xffff) + 8`.
pub fn definition(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    let entries = m.u32(TAG_ENTRIES.base)?;
    m.u32(entries.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))
}

/// The definition of a `snd!` tag, `None` for another group. The group
/// test indexes by the sign-extended low word, as `movsx` does.
fn sound_definition(m: &dyn GuestMemory, tag: u32) -> Result<Option<u32>, MemFault> {
    let entries = m.u32(TAG_ENTRIES.base)?;
    let group = entries.wrapping_add((i32::from(tag as u16 as i16) << 4) as u32);
    if m.u32(group)? != SOUND_GROUP {
        return Ok(None);
    }
    definition(m, tag).map(Some)
}

/// `0x2194a0(eax = definition)`: the bank row, 0 for bank `0xff`.
pub fn bank_row(m: &dyn GuestMemory, definition: u32) -> Result<u32, MemFault> {
    let bank = m.u8(definition.wrapping_add(0xd))?;
    if bank == 0xff {
        return Ok(0);
    }
    let banks = m.u32(m.u32(GESTALT)?.wrapping_add(0x34))?;
    Ok(banks.wrapping_add((i32::from(bank as i8) * BANK as i32) as u32))
}

/// `0x18d170(ecx = tag)`: a `snd!` tag's bank row, else 0.
pub fn default_row(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    if tag == NONE {
        return Ok(0);
    }
    match sound_definition(m, tag)? {
        Some(def) => bank_row(m, def),
        None => Ok(0),
    }
}

/// A list tag's row `custom` names (`0x18d0f8`).
fn list_row(m: &dyn GuestMemory, list: u32, custom: u32) -> Result<u32, MemFault> {
    if list == NONE {
        return Ok(0);
    }
    let rows = m.u32(definition(m, list)?.wrapping_add(4))?;
    Ok(rows.wrapping_add((custom & 0x3fff_ffff).wrapping_mul(LIST_ROW)).wrapping_add(4))
}

/// `0x18d090(ecx = tag, edx = custom) -> eax`: the custom playback row, 0
/// for none.
pub fn playback_row(m: &dyn GuestMemory, tag: u32, custom: u32) -> Result<u32, MemFault> {
    if custom == NONE {
        return default_row(m, tag);
    }
    match custom >> 30 {
        0 => match sound_definition(m, tag)? {
            Some(def) => bank_row(m, def),
            None => Ok(0),
        },
        1 => {
            let globals = m.u32(GLOBALS_TAG.base)?;
            if m.u32(globals.wrapping_add(0xc0))? == 0 {
                return Ok(0);
            }
            let reference = m.u32(globals.wrapping_add(0xc4))?;
            if reference == 0 {
                return Ok(0);
            }
            list_row(m, m.u32(reference.wrapping_add(0xc))?, custom)
        }
        2 => list_row(m, m.u32(m.u32(SCENARIO.base)?.wrapping_add(0x324))?, custom),
        _ => Ok(0),
    }
}

/// `0x21dd30(edi = tag, custom, &effect) -> al`: the effect of the row
/// `custom` names, or with no custom the tag's own, when it has a count
/// (`+0x2c`) and components (effect `+8`).
pub fn effect_of(m: &dyn GuestMemory, tag: u32, custom: u32) -> Result<Option<u32>, MemFault> {
    let row = playback_row(m, tag, custom)?;
    let default = default_row(m, tag)?;
    let found = (row != 0 && m.u32(row.wrapping_add(0x2c))? != 0)
        || (custom == NONE && default != 0 && m.u32(default.wrapping_add(0x2c))? != 0);
    // The count and effect are read from the custom row in both cases:
    // with no custom it is the default row.
    if !found || m.u32(row.wrapping_add(0x2c))? as i32 <= 0 {
        return Ok(None);
    }
    let effect = m.u32(row.wrapping_add(0x30))?;
    if effect == 0 || m.u32(effect.wrapping_add(8))? as i32 <= 0 {
        return Ok(None);
    }
    Ok(Some(effect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const ENTRIES: u32 = 0x0060_0000;
    const DEF: u32 = 0x0060_1000;
    const GESTALT_AT: u32 = 0x0060_2000;
    const BANKS: u32 = 0x0060_3000;
    const LIST_DEF: u32 = 0x0060_4000;
    const LIST_ROWS: u32 = 0x0060_5000;
    const SCENARIO_AT: u32 = 0x0060_6000;
    const GLOBALS_AT: u32 = 0x0060_7000;
    const EFFECT: u32 = 0x0060_8000;
    const TAG: u32 = 0xe100_0001;
    const LIST: u32 = 0xe200_0002;

    /// Tag 1 a `snd!` with bank 2; tag 2 a list both the scenario and the
    /// globals name.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 16, SOUND_GROUP).put_u32(ENTRIES + 16 + 8, DEF);
        g.put_u32(ENTRIES + 32 + 8, LIST_DEF).put_u32(LIST_DEF + 4, LIST_ROWS);
        g.put_u8(DEF + 0xd, 2).put_u32(GESTALT, GESTALT_AT).put_u32(GESTALT_AT + 0x34, BANKS);
        g.put_u32(SCENARIO.base, SCENARIO_AT).put_u32(SCENARIO_AT + 0x324, LIST);
        g.put_u32(GLOBALS_TAG.base, GLOBALS_AT).put_u32(GLOBALS_AT + 0xc0, 1).put_u32(GLOBALS_AT + 0xc4, GLOBALS_AT + 0x100);
        g.put_u32(GLOBALS_AT + 0x10c, LIST);
        g.map(EFFECT, 0x100).map(BANKS, 0x1000).map(LIST_ROWS, 0x1000);
        g
    }

    fn with_effect(g: &mut FakeGuest, row: u32, components: u32) {
        g.put_u32(row + 0x2c, 1).put_u32(row + 0x30, EFFECT).put_u32(EFFECT + 8, components);
    }

    #[test]
    fn the_rows_a_custom_word_names() {
        let mut g = guest();
        let bank = BANKS + 2 * BANK;
        assert_eq!(playback_row(&g, TAG, NONE), Ok(bank));
        assert_eq!(playback_row(&g, TAG, 5), Ok(bank), "kind 0 is the tag's bank row");
        assert_eq!(playback_row(&g, TAG, (1 << 30) | 3), Ok(LIST_ROWS + 3 * LIST_ROW + 4));
        assert_eq!(playback_row(&g, TAG, (2 << 30) | 4), Ok(LIST_ROWS + 4 * LIST_ROW + 4));
        assert_eq!(playback_row(&g, TAG, 3 << 30), Ok(0));
        assert_eq!(playback_row(&g, NONE, NONE), Ok(0));
        g.put_u32(GLOBALS_AT + 0xc0, 0);
        assert_eq!(playback_row(&g, TAG, 1 << 30), Ok(0), "no globals reference");
        g.put_u8(DEF + 0xd, 0xff);
        assert_eq!(default_row(&g, TAG), Ok(0), "bank 0xff");
    }

    #[test]
    fn a_tag_of_another_group_has_no_bank_row() {
        let mut g = guest();
        g.put_u32(ENTRIES + 16, 0x6c73_6e64);
        assert_eq!(default_row(&g, TAG), Ok(0));
        assert_eq!(playback_row(&g, TAG, 0), Ok(0));
    }

    #[test]
    fn the_effect_needs_a_count_and_components() {
        let mut g = guest();
        let bank = BANKS + 2 * BANK;
        assert_eq!(effect_of(&g, TAG, NONE), Ok(None), "no count");
        with_effect(&mut g, bank, 0);
        assert_eq!(effect_of(&g, TAG, NONE), Ok(None), "no components");
        with_effect(&mut g, bank, 2);
        assert_eq!(effect_of(&g, TAG, NONE), Ok(Some(EFFECT)));
        let custom = (2 << 30) | 1;
        assert_eq!(effect_of(&g, TAG, custom), Ok(None), "a custom row does not fall back to the tag's");
        with_effect(&mut g, LIST_ROWS + LIST_ROW + 4, 1);
        assert_eq!(effect_of(&g, NONE, custom), Ok(Some(EFFECT)), "a tag start has no sound tag");
    }

    #[test]
    fn a_negative_count_has_no_effect() {
        let mut g = guest();
        let bank = BANKS + 2 * BANK;
        with_effect(&mut g, bank, 1);
        g.put_u32(bank + 0x2c, u32::MAX);
        assert_eq!(effect_of(&g, TAG, NONE), Ok(None));
    }
}
