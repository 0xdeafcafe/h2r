//! Spare clips over a fake game: one spawner state, one weapon object and
//! its definition. All values are invented.
use super::*;
use guest_layout::datum::layout;
use guest_layout::points;
use guest_layout::tags;
use guest_layout::testing::FakeGuest;

const ENGINE: u32 = 0x0080_0000;
const OBJECT_ARRAY: u32 = 0x0086_0000;
const OBJECT_ROWS: u32 = 0x0087_0000;
const OBJECT_BITS: u32 = 0x0088_0000;
const WEAPON_BODY: u32 = 0x0089_0000;
const CRATE_BODY: u32 = 0x008a_0000;
const TAG_HEADER: u32 = 0x0060_0000;
const TAG_ROWS: u32 = 0x0062_0000;
const WEAP_TAG: u32 = 0x0063_0000;
const MAGAZINE: u32 = 0x0063_1000;
const WEAP_DATUM: u32 = 0xe000_0000;

/// Entry 0 has a weapon out (an 8-round clip, at most 40 in reserve, 16
/// now), entry 1 a crate, entry 2 nothing.
fn game() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.put_u32(global::GAME_ENGINE.base, ENGINE)
        .map(ENGINE + STATE, 3 * STATE_STRIDE)
        .put_u32(ENGINE + STATE, 0x0002_0000)
        .put_u32(ENGINE + STATE + STATE_STRIDE, 0x0003_0001)
        .put_u32(ENGINE + STATE + 2 * STATE_STRIDE, u32::MAX)
        .put_u32(points::OBJECTS.base, OBJECT_ARRAY)
        .put_u32(OBJECT_ARRAY + layout::ELEMENT_SIZE, 0xc)
        .put_u32(OBJECT_ARRAY + layout::HIGH_WATER, 2)
        .put_u32(OBJECT_ARRAY + layout::DATA, OBJECT_ROWS)
        .put_u32(OBJECT_ARRAY + layout::BITMAP, OBJECT_BITS)
        .put_u32(OBJECT_BITS, 0b11)
        .put_u16(OBJECT_ROWS, 2)
        .put_u8(OBJECT_ROWS + 3, objects::WEAPON)
        .put_u32(OBJECT_ROWS + 8, WEAPON_BODY)
        .put_u16(OBJECT_ROWS + 0xc, 3)
        .put_u8(OBJECT_ROWS + 0xc + 3, 6)
        .put_u32(OBJECT_ROWS + 0xc + 8, CRATE_BODY)
        .map(WEAPON_BODY, Weapon::SIZE)
        .put_u32(WEAPON_BODY, WEAP_DATUM)
        .put_u16(WEAPON_BODY + Weapon::MAGAZINE_RESERVE.offset, 16)
        .map(CRATE_BODY, 0x10c)
        .put_u32(CRATE_BODY, WEAP_DATUM)
        .put_u32(tags::TAG_INDEX_GLOBAL, TAG_HEADER)
        .put_u32(TAG_HEADER + tags::index_layout::TAG_COUNT, 1)
        .put_u32(tags::TAG_ENTRIES_GLOBAL, TAG_ROWS)
        .put_u32(TAG_ROWS + tags::entry_layout::GROUP, WEAP)
        .put_u32(TAG_ROWS + tags::entry_layout::DATUM, WEAP_DATUM)
        .put_u32(TAG_ROWS + tags::entry_layout::ADDRESS, WEAP_TAG)
        .map(WEAP_TAG, 0x300)
        .put_u32(WEAP_TAG + MAGAZINES, 1)
        .put_u32(WEAP_TAG + MAGAZINES + 4, MAGAZINE)
        .map(MAGAZINE, MAGAZINE_LEN)
        .put_u16(MAGAZINE + CLIP, 8)
        .put_u16(MAGAZINE + RESERVE_MAX, 40);
    g
}

fn reserve_of(g: &FakeGuest) -> u16 {
    g.u16(WEAPON_BODY + Weapon::MAGAZINE_RESERVE.offset).unwrap()
}

#[test]
fn a_spawners_weapon_takes_its_clips_capped_at_the_weapons_most() {
    let mut g = game();
    let state = state(&g).unwrap().expect("a game engine");
    assert_eq!(state, ENGINE + STATE);
    assert_eq!(fill(&mut g, state, 0, 2).unwrap(), Some(16));
    assert_eq!(reserve_of(&g), 16);
    assert_eq!(fill(&mut g, state, 0, 0).unwrap(), Some(0));
    assert_eq!(reserve_of(&g), 0, "no spare clips is an empty reserve");
    assert_eq!(fill(&mut g, state, 0, 10).unwrap(), Some(40), "capped");
    assert_eq!(reserve_of(&g), 40);
}

#[test]
fn an_entry_with_nothing_out_or_no_weapon_is_left_alone() {
    let mut g = game();
    let state = state(&g).unwrap().unwrap();
    let crate_before = g.u16(CRATE_BODY + Weapon::MAGAZINE_RESERVE.offset).ok();
    assert_eq!(fill(&mut g, state, 1, 3).unwrap(), None, "a crate is no weapon");
    assert_eq!(g.u16(CRATE_BODY + Weapon::MAGAZINE_RESERVE.offset).ok(), crate_before);
    assert_eq!(fill(&mut g, state, 2, 3).unwrap(), None, "nothing out");
    assert_eq!(reserve_of(&g), 16);
}

#[test]
fn the_table_holds_clips_by_entry_until_cleared() {
    // The table is the process's; this test is its only writer.
    set(7, Some(3));
    set(8, Some(1));
    set(8, None);
    assert_eq!((get(7), get(8)), (Some(3), None));
    clear();
    assert_eq!(get(7), None);
}
