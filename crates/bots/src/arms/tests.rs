use super::*;
use engine::datum::layout as dl;
use engine::points;
use engine::testing::FakeGuest;
use engine::view::Record;

const OBJECTS_HEADER: u32 = 0x0062_0000;
const OBJECTS_DATA: u32 = 0x0062_1000;
const BODIES: u32 = 0x0063_0000;
const TAG_INDEX: u32 = 0x0068_0000;
const TAG_ENTRIES: u32 = 0x0068_1000;
const TAGS: u32 = 0x0069_0000;

fn rounds(loaded: i16, capacity: i16, reserve: i16, reserve_max: i16) -> Ammo {
    Ammo::Rounds {
        loaded,
        capacity,
        reserve,
        reserve_max,
    }
}
fn gun(def: u32, ammo: Ammo) -> Gun {
    Gun { def, ammo }
}
fn pickup(id: u32, x: f32, def: u32, ammo: Ammo) -> Pickup {
    Pickup {
        id,
        position: [x, 0.0, 0.0],
        gun: gun(def, ammo),
    }
}
fn arms(held: Option<Gun>, other: Option<Gun>) -> Arms {
    Arms {
        held,
        other,
        grenades: [0, 0],
    }
}
fn decide(a: &Arms, enemy: Option<f32>, calm: bool, pickups: &[Pickup]) -> Kit {
    plan(
        &Situation {
            arms: a,
            at: [0.0; 3],
            enemy,
            calm,
            pickups,
        },
        |_| false,
    )
}

const BR: u32 = 0xa001_0001;
const PP: u32 = 0xa002_0002;
const SMG: u32 = 0xa003_0003;

#[test]
fn ammo_reads_dry_low_and_reload_worthy() {
    assert!(rounds(0, 36, 0, 108).dry());
    assert!(!rounds(0, 36, 1, 108).dry());
    assert!(rounds(0, 36, 1, 108).empty_magazine());
    assert!(rounds(10, 36, 30, 108).wants_reload());
    assert!(!rounds(10, 36, 0, 108).wants_reload(), "nothing to load");
    assert!(!rounds(30, 36, 30, 108).wants_reload(), "full enough");
    assert!(Ammo::Battery(0.01).dry());
    assert!(!Ammo::Battery(0.5).dry());
    assert!(
        !Ammo::Battery(0.1).wants_reload(),
        "a battery never reloads"
    );
    assert!(rounds(4, 36, 10, 108).low());
    assert!(!rounds(36, 36, 108, 108).low());
    assert!(Ammo::None.dry() && !Ammo::None.gun());
    assert!((rounds(36, 36, 108, 108).fraction() - 1.0).abs() < 1e-6);
}

#[test]
fn a_full_gun_is_fine() {
    let a = arms(Some(gun(BR, rounds(36, 36, 108, 108))), None);
    assert_eq!(decide(&a, Some(5.0), false, &[]), Kit::Fine);
    assert_eq!(decide(&a, None, true, &[]), Kit::Fine);
}

#[test]
fn an_empty_magazine_reloads_even_in_a_fight() {
    let a = arms(Some(gun(BR, rounds(0, 36, 50, 108))), None);
    assert_eq!(decide(&a, Some(5.0), false, &[]), Kit::Reload);
}

#[test]
fn a_low_magazine_reloads_only_when_calm() {
    let a = arms(Some(gun(BR, rounds(6, 36, 50, 108))), None);
    assert_eq!(decide(&a, Some(5.0), false, &[]), Kit::Fine);
    assert_eq!(decide(&a, None, true, &[]), Kit::Reload);
}

#[test]
fn a_dry_gun_switches_to_the_other() {
    let a = arms(
        Some(gun(BR, rounds(0, 36, 0, 108))),
        Some(gun(PP, Ammo::Battery(0.8))),
    );
    assert_eq!(decide(&a, Some(5.0), false, &[]), Kit::Switch);
    // Nothing held at all, something in the other hand.
    let a = arms(None, Some(gun(PP, Ammo::Battery(0.8))));
    assert_eq!(decide(&a, None, true, &[]), Kit::Switch);
}

#[test]
fn both_dry_seeks_the_nearest_useful_pickup() {
    let a = arms(
        Some(gun(BR, rounds(0, 36, 0, 108))),
        Some(gun(PP, Ammo::Battery(0.0))),
    );
    let ps = [
        pickup(1, 20.0, SMG, rounds(60, 60, 120, 240)),
        pickup(2, 6.0, SMG, rounds(0, 60, 0, 240)), // empty: useless
        pickup(3, 9.0, BR, rounds(36, 36, 36, 108)),
        pickup(4, 2.0, 0xa004_0004, Ammo::None), // the flag
    ];
    assert_eq!(decide(&a, None, true, &ps), Kit::Seek(ps[2]));
    // Out of range: nothing to seek, nobody to hit.
    let far = [pickup(1, SEEK_RANGE + 1.0, SMG, rounds(60, 60, 0, 240))];
    assert_eq!(decide(&a, None, true, &far), Kit::Fine);
    // A pickup the bot could not reach is skipped.
    let k = plan(
        &Situation {
            arms: &a,
            at: [0.0; 3],
            enemy: None,
            calm: true,
            pickups: &ps,
        },
        |p| p.id == 3,
    );
    assert_eq!(k, Kit::Seek(ps[0]));
}

#[test]
fn both_dry_with_an_enemy_close_melees() {
    let a = arms(Some(gun(BR, rounds(0, 36, 0, 108))), None);
    let ps = [pickup(1, 5.0, SMG, rounds(60, 60, 0, 240))];
    assert_eq!(decide(&a, Some(1.5), false, &ps), Kit::Melee);
    // Further off it goes for the gun.
    assert_eq!(decide(&a, Some(8.0), false, &ps), Kit::Seek(ps[0]));
    // No gun anywhere: melee whatever comes.
    assert_eq!(decide(&a, Some(8.0), false, &[]), Kit::Melee);
}

#[test]
fn low_on_both_tops_up_nearby_when_calm() {
    let a = arms(
        Some(gun(BR, rounds(8, 36, 0, 108))),
        Some(gun(SMG, rounds(3, 60, 0, 240))),
    );
    let near = [pickup(1, 6.0, BR, rounds(36, 36, 0, 108))];
    assert_eq!(decide(&a, None, true, &near), Kit::Seek(near[0]));
    assert_eq!(decide(&a, Some(6.0), false, &near), Kit::Fine);
    let far = [pickup(1, TOP_UP_RANGE + 2.0, BR, rounds(36, 36, 0, 108))];
    assert_eq!(decide(&a, None, true, &far), Kit::Fine);
}

// ---- the traced fields, read from a fake guest ----

/// A guest with these weapon objects: (definition index, parent unit or
/// none, position, magazine (loaded, reserve) or the battery used).
struct World {
    g: FakeGuest,
    unit: u32,
}

const BR_TAG: u32 = TAGS;
const PP_TAG: u32 = TAGS + 0x400;
const FLAG_TAG: u32 = TAGS + 0x800;

fn world() -> World {
    let mut g = FakeGuest::new();
    // Tags: 0 a battle rifle (36 a magazine, 108 reserve), 1 a plasma
    // pistol (a barrel using 0.01 a shot), 2 the flag (neither).
    g.put_u32(points::TAG_INDEX.base, TAG_INDEX)
        .map(TAG_INDEX, 0x40)
        .put_u32(TAG_INDEX + 0x18, 3);
    g.put_u32(points::TAG_ENTRIES.base, TAG_ENTRIES)
        .map(TAG_ENTRIES, 0x100);
    for (i, (datum, addr)) in [(BR, BR_TAG), (PP, PP_TAG), (0xa004_0002, FLAG_TAG)]
        .into_iter()
        .enumerate()
    {
        let e = TAG_ENTRIES + 0x10 * i as u32;
        g.put_u32(e, u32::from_be_bytes(*b"weap"))
            .put_u32(e + 4, datum & 0xffff_0000 | i as u32)
            .put_u32(e + 8, addr);
        g.map(addr, 0x400);
    }
    let br_mags = BR_TAG + 0x300;
    g.put_u32(BR_TAG + WEAP_MAGAZINES, 1)
        .put_u32(BR_TAG + WEAP_MAGAZINES + 4, br_mags)
        .put_u16(br_mags + MAGAZINE_DEF_LOADED_MAX, 36)
        .put_u16(br_mags + MAGAZINE_DEF_RESERVE_MAX, 108);
    let br_barrels = BR_TAG + 0x200;
    g.put_u32(BR_TAG + WEAP_BARRELS, 1)
        .put_u32(BR_TAG + WEAP_BARRELS + 4, br_barrels);
    let pp_barrels = PP_TAG + 0x200;
    g.put_u32(PP_TAG + WEAP_BARRELS, 1)
        .put_u32(PP_TAG + WEAP_BARRELS + 4, pp_barrels)
        .put_f32(pp_barrels + BARREL_DEF_USAGE, 0.01);

    // Objects: 0 the unit, 1 its battle rifle (held), 2 its plasma pistol,
    // 3 a battle rifle on the ground, 4 the flag on the ground.
    g.put_u32(points::OBJECTS.base, OBJECTS_HEADER)
        .map(OBJECTS_HEADER, 0x48)
        .put_u32(OBJECTS_HEADER + dl::ELEMENT_SIZE, 0xc)
        .put_u32(OBJECTS_HEADER + dl::HIGH_WATER, 5)
        .put_u32(OBJECTS_HEADER + dl::DATA, OBJECTS_DATA)
        .map(OBJECTS_DATA, 0xc * 5);
    let unit = DatumHandle::new(0xb000, 0);
    let defs = [0, BR, PP, BR, 0xa004_0002];
    for i in 0..5u32 {
        let h = OBJECTS_DATA + i * 0xc;
        let body = BODIES + i * 0x400;
        g.put_u16(h, 0xb000 + i as u16)
            .put_u8(
                h + 3,
                if i == 0 {
                    objects::BIPED
                } else {
                    objects::WEAPON
                },
            )
            .put_u32(h + 8, body)
            .map(body, 0x400);
        let tag_index = [0, 0, 1, 0, 2][i as usize];
        g.put_u32(body, defs[i as usize] & 0xffff_0000 | tag_index)
            .put_u32(
                body + 0x14,
                if (1..=2).contains(&i) {
                    unit.raw()
                } else {
                    u32::MAX
                },
            )
            .put_f32(body + 0x64, 10.0 * i as f32);
    }
    let w = |i: u32| BODIES + i * 0x400;
    // The held rifle: 7 loaded, 50 reserve. The pistol: 0.25 used.
    g.put_u16(w(1) + WEAPON_MAGAZINES + MAGAZINE_LOADED, 7)
        .put_u16(w(1) + WEAPON_MAGAZINES + MAGAZINE_RESERVE, 50)
        .put_f32(w(2) + WEAPON_AGE, 0.25)
        .put_u16(w(3) + WEAPON_MAGAZINES + MAGAZINE_LOADED, 36)
        .put_u16(w(3) + WEAPON_MAGAZINES + MAGAZINE_RESERVE, 0);
    // The unit: slot 1 held (slot 0 empty), the pistol in slot 2; two
    // frags, one plasma.
    let u = BODIES;
    g.put_u8(u + UNIT_WEAPON_INDEX, 1)
        .put_u32(u + UNIT_WEAPONS, u32::MAX)
        .put_u32(u + UNIT_WEAPONS + 4, DatumHandle::new(0xb001, 1).raw())
        .put_u32(u + UNIT_WEAPONS + 8, DatumHandle::new(0xb002, 2).raw())
        .put_u32(u + UNIT_WEAPONS + 12, u32::MAX)
        .put_u8(u + UNIT_GRENADES, 2)
        .put_u8(u + UNIT_GRENADES + 1, 1);
    World { g, unit: u }
}

fn def_raw(def: u32, index: u32) -> u32 {
    def & 0xffff_0000 | index
}

#[test]
fn the_unit_reads_its_held_gun_the_other_and_grenades() {
    let w = world();
    let a = read(&w.g, w.unit).unwrap();
    assert_eq!(
        a.held,
        Some(Gun {
            def: def_raw(BR, 0),
            ammo: rounds(7, 36, 50, 108),
        })
    );
    let other = a.other.expect("the pistol");
    assert_eq!(other.def, def_raw(PP, 1));
    match other.ammo {
        Ammo::Battery(b) => assert!((b - 0.75).abs() < 1e-6, "{b}"),
        a => panic!("{a:?}"),
    }
    assert_eq!(a.grenades, [2, 1]);
    assert!(a.has_grenades());
    assert!(a.carries(def_raw(BR, 0)));
}

#[test]
fn a_held_slot_out_of_range_holds_nothing() {
    let mut w = world();
    w.g.put_u8(w.unit + UNIT_WEAPON_INDEX, 0xff);
    let a = read(&w.g, w.unit).unwrap();
    assert_eq!(a.held, None);
    assert!(a.other.is_some());
}

#[test]
fn pickups_are_the_weapons_nobody_holds() {
    let w = world();
    let ps = pickups(&w.g).unwrap();
    assert_eq!(ps.len(), 2, "{ps:?}");
    let rifle = ps.iter().find(|p| p.gun.def == def_raw(BR, 0)).unwrap();
    assert_eq!(rifle.position, [30.0, 0.0, 0.0]);
    assert_eq!(rifle.gun.ammo, rounds(36, 36, 0, 108));
    assert!(rifle.useful());
    let flag = ps.iter().find(|p| p.gun.def != def_raw(BR, 0)).unwrap();
    assert_eq!(flag.gun.ammo, Ammo::None, "no magazine, no battery");
    assert!(!flag.useful());
}

#[test]
fn a_stale_definition_reads_as_no_gun() {
    let mut w = world();
    // The rifle's definition datum with the wrong salt.
    let body = objects::Body::at(BODIES + 0x400);
    w.g.put_u32(body.addr(), 0x1234_0000);
    assert!(
        super::gun(&w.g, DatumHandle::new(0xb001, 1))
            .unwrap()
            .is_none()
    );
}
