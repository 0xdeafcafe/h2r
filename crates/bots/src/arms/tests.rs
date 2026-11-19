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
    Gun {
        def,
        ammo,
        ..Default::default()
    }
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
            breather: calm,
            by_range: false,
            pickups,
        },
        |_| false,
    )
}
/// As [`decide`], at Normal skill and up: picks the weapon by range; and
/// `breather` apart from `calm`.
fn decide_by_range(
    a: &Arms,
    enemy: Option<f32>,
    calm: bool,
    breather: bool,
    pickups: &[Pickup],
) -> Kit {
    plan(
        &Situation {
            arms: a,
            at: [0.0; 3],
            enemy,
            calm,
            breather: breather || calm,
            by_range: true,
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
            breather: true,
            by_range: false,
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

// ---- handling: roles, ranges, and the choices they drive ----

fn handling(trigger: Trigger, zoom: i16, pellets: i16, shot_speed: f32) -> Profile {
    Profile {
        trigger,
        zoom,
        pellets,
        shot_speed,
        ..Profile::default()
    }
}
fn sniper() -> Profile {
    handling(Trigger::Single, 2, 1, 900.0)
}
fn rifle() -> Profile {
    handling(Trigger::Single, 1, 1, 400.0)
}
fn shotgun() -> Profile {
    handling(Trigger::Single, 0, 15, 300.0)
}
fn smg() -> Profile {
    handling(Trigger::Automatic, 0, 1, 300.0)
}
fn rockets() -> Profile {
    handling(Trigger::Single, 1, 1, 30.0)
}
fn sword() -> Profile {
    Profile {
        trigger: Trigger::Absent,
        cone_range: 2.0,
        ..Profile::default()
    }
}
fn with(def: u32, profile: Profile, ammo: Ammo) -> Gun {
    Gun { def, ammo, profile }
}
fn full() -> Ammo {
    rounds(36, 36, 108, 108)
}
const SNIPER: u32 = 0xa005_0005;
const SHOTGUN: u32 = 0xa006_0006;
const ROCKETS: u32 = 0xa007_0007;
const SWORD: u32 = 0xa008_0008;

#[test]
fn roles_come_from_the_handling() {
    assert_eq!(sniper().role(), Role::Sniper);
    assert_eq!(rifle().role(), Role::Precise);
    assert_eq!(shotgun().role(), Role::Scatter);
    assert_eq!(smg().role(), Role::Spray);
    assert_eq!(rockets().role(), Role::Blast);
    assert_eq!(sword().role(), Role::Blade);
    assert_eq!(
        handling(Trigger::Charge, 0, 1, 60.0).role(),
        Role::Spray,
        "plasma pistol"
    );
    // The flag: no trigger and no aim assist. Not read: weapon-blind.
    assert_eq!(handling(Trigger::Absent, 0, 0, 0.0).role(), Role::Unknown);
    assert_eq!(Profile::default().role(), Role::Unknown);
    assert_eq!(Trigger::from_behaviour(0), Trigger::Automatic);
    for b in [1, 4, 5] {
        assert_eq!(Trigger::from_behaviour(b), Trigger::Single);
    }
    assert_eq!(Trigger::from_behaviour(2), Trigger::Charge);
    // Bands nest the way the weapons do.
    assert!(Role::Scatter.band().1 < Role::Precise.band().1);
    assert!(Role::Precise.band().1 < Role::Sniper.band().1);
    assert!(Role::Blast.fire_range().0 > 0.0, "never at its own feet");
    assert!(Role::Sniper.power() && Role::Blast.power() && !Role::Precise.power());
}

#[test]
fn the_preferred_range_is_the_held_guns_band() {
    let a = arms(
        Some(with(SNIPER, sniper(), full())),
        Some(with(SMG, smg(), full())),
    );
    assert_eq!(preferred_range(&a), Role::Sniper.band());
    assert_eq!(preferred_range(&a.held.unwrap()), (12.0, 60.0));
    // An empty magazine is soon reloaded: still the sniper's band...
    let a = arms(
        Some(with(SNIPER, sniper(), rounds(0, 4, 8, 24))),
        Some(with(SMG, smg(), full())),
    );
    assert_eq!(preferred_range(&a), Role::Sniper.band());
    // ...but a dry one is switched away from: the other's.
    let a = arms(
        Some(with(SNIPER, sniper(), rounds(0, 4, 0, 24))),
        Some(with(SMG, smg(), full())),
    );
    assert_eq!(preferred_range(&a), Role::Spray.band());
    // Nothing to fire: arm's length, as the melee decision says.
    let a = arms(Some(with(SNIPER, sniper(), rounds(0, 4, 0, 24))), None);
    assert_eq!(preferred_range(&a), MELEE_BAND);
    assert_eq!(preferred_range(&Kit::Melee), MELEE_BAND);
    assert_eq!(preferred_range(&Kit::Fine), Role::Unknown.band());
}

#[test]
fn by_range_it_switches_to_the_gun_that_suits_the_distance() {
    let a = arms(
        Some(with(SHOTGUN, shotgun(), full())),
        Some(with(BR, rifle(), full())),
    );
    assert_eq!(
        decide_by_range(&a, Some(20.0), false, false, &[]),
        Kit::Switch
    );
    assert_eq!(
        decide(&a, Some(20.0), false, &[]),
        Kit::Fine,
        "Easy keeps it"
    );
    assert_eq!(decide_by_range(&a, Some(2.0), false, false, &[]), Kit::Fine);
    // The rifle in hand and the enemy in its face: the shotgun...
    let b = arms(
        Some(with(BR, rifle(), full())),
        Some(with(SHOTGUN, shotgun(), full())),
    );
    assert_eq!(
        decide_by_range(&b, Some(2.0), false, false, &[]),
        Kit::Switch
    );
    // ...and no flip-flop at a band's edge: 4.5 is just past the shotgun's
    // band and just short of the rifle's, neither clearly wrong.
    assert_eq!(decide_by_range(&a, Some(4.5), false, false, &[]), Kit::Fine);
    assert_eq!(decide_by_range(&b, Some(4.5), false, false, &[]), Kit::Fine);
    // An empty other gun is no choice.
    let c = arms(
        Some(with(SHOTGUN, shotgun(), full())),
        Some(with(BR, rifle(), rounds(0, 36, 0, 108))),
    );
    assert_eq!(
        decide_by_range(&c, Some(20.0), false, false, &[]),
        Kit::Fine
    );
}

#[test]
fn rockets_are_put_away_close_up() {
    let a = arms(
        Some(with(ROCKETS, rockets(), full())),
        Some(with(SMG, smg(), full())),
    );
    assert_eq!(
        decide_by_range(&a, Some(1.5), false, false, &[]),
        Kit::Switch
    );
    assert_eq!(
        decide_by_range(&a, Some(12.0), false, false, &[]),
        Kit::Fine
    );
}

#[test]
fn an_empty_magazine_in_a_fight_switches_when_that_is_faster() {
    let a = arms(
        Some(with(BR, rifle(), rounds(0, 36, 50, 108))),
        Some(with(SMG, smg(), full())),
    );
    // The SMG is good at 5: draw it rather than reload.
    assert_eq!(
        decide_by_range(&a, Some(5.0), false, false, &[]),
        Kit::Switch
    );
    // At 20 it is not: reload.
    assert_eq!(
        decide_by_range(&a, Some(20.0), false, false, &[]),
        Kit::Reload
    );
    // Nobody about: reload.
    assert_eq!(decide_by_range(&a, None, true, true, &[]), Kit::Reload);
    // Easy always reloads.
    assert_eq!(decide(&a, Some(5.0), false, &[]), Kit::Reload);
}

#[test]
fn a_lull_reloads_a_half_empty_magazine() {
    let a = arms(Some(gun(BR, rounds(15, 36, 50, 108))), None);
    assert_eq!(decide_by_range(&a, None, false, true, &[]), Kit::Reload);
    assert_eq!(
        decide_by_range(&a, None, false, false, &[]),
        Kit::Fine,
        "still under fire"
    );
    assert_eq!(decide_by_range(&a, Some(5.0), false, false, &[]), Kit::Fine);
    // Nearly full: no reload.
    let b = arms(Some(gun(BR, rounds(30, 36, 50, 108))), None);
    assert_eq!(decide_by_range(&b, None, false, true, &[]), Kit::Fine);
    // Nothing to load: no reload.
    let c = arms(Some(gun(BR, rounds(15, 36, 0, 108))), None);
    assert_eq!(decide_by_range(&c, None, false, true, &[]), Kit::Fine);
}

#[test]
fn an_enemy_out_of_reach_is_time_to_reload() {
    let a = arms(Some(with(SMG, smg(), rounds(10, 60, 100, 240))), None);
    assert_eq!(
        decide_by_range(&a, Some(30.0), false, false, &[]),
        Kit::Reload
    );
    assert_eq!(decide_by_range(&a, Some(5.0), false, false, &[]), Kit::Fine);
}

#[test]
fn a_power_weapon_nearby_is_taken_and_the_worse_gun_dropped() {
    let rifle_held = arms(
        Some(with(BR, rifle(), full())),
        Some(with(SMG, smg(), full())),
    );
    let ps = [pickup(1, 15.0, SNIPER, rounds(4, 4, 8, 24))];
    let ps = [Pickup {
        gun: with(SNIPER, sniper(), ps[0].gun.ammo),
        ..ps[0]
    }];
    // The rifle is the better of the two: switch to the SMG, which the
    // sniper rifle then replaces.
    assert_eq!(
        decide_by_range(&rifle_held, None, true, true, &ps),
        Kit::Switch
    );
    let smg_held = arms(
        Some(with(SMG, smg(), full())),
        Some(with(BR, rifle(), full())),
    );
    assert_eq!(
        decide_by_range(&smg_held, None, true, true, &ps),
        Kit::Seek(ps[0])
    );
    // Not in a fight.
    assert_eq!(
        decide_by_range(&smg_held, Some(8.0), false, false, &ps),
        Kit::Fine
    );
    // Not with a power weapon already.
    let armed = arms(
        Some(with(SHOTGUN, shotgun(), full())),
        Some(with(BR, rifle(), full())),
    );
    assert_eq!(decide_by_range(&armed, None, true, true, &ps), Kit::Fine);
    // Not further than POWER_RANGE.
    let far = [Pickup {
        position: [POWER_RANGE + 1.0, 0.0, 0.0],
        ..ps[0]
    }];
    assert_eq!(
        decide_by_range(&smg_held, None, true, true, &far),
        Kit::Fine
    );
}

#[test]
fn dry_it_goes_for_the_better_gun_a_little_further_off() {
    let a = arms(Some(gun(BR, rounds(0, 36, 0, 108))), None);
    let ps = [
        Pickup {
            id: 1,
            position: [5.0, 0.0, 0.0],
            gun: with(SMG, smg(), rounds(60, 60, 0, 240)),
        },
        Pickup {
            id: 2,
            position: [9.0, 0.0, 0.0],
            gun: with(SNIPER, sniper(), rounds(4, 4, 8, 24)),
        },
    ];
    assert_eq!(decide(&a, None, true, &ps), Kit::Seek(ps[1]));
}

#[test]
fn a_blade_is_kept_close_up_and_put_away_for_a_far_enemy() {
    let a = arms(
        Some(with(SWORD, sword(), Ammo::None)),
        Some(with(BR, rifle(), full())),
    );
    // Not "dry": it is a sword.
    assert_eq!(decide_by_range(&a, Some(1.0), false, false, &[]), Kit::Fine);
    assert_eq!(
        decide_by_range(&a, Some(20.0), false, false, &[]),
        Kit::Switch
    );
    assert_eq!(
        decide(&a, Some(20.0), false, &[]),
        Kit::Fine,
        "Easy keeps it"
    );
    assert!(a.held.unwrap().ready());
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
    let held = a.held.expect("the rifle");
    assert_eq!(held.def, def_raw(BR, 0));
    assert_eq!(held.ammo, rounds(7, 36, 50, 108));
    // No triggers, no aim assist in this fake tag: weapon-blind.
    assert_eq!(held.role(), Role::Unknown);
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

#[test]
fn the_handling_reads_from_the_weapon_and_projectile_tags() {
    let mut w = world();
    let g = &mut w.g;
    // A fourth tag, a projectile at 400 a second.
    const PROJ_TAG: u32 = TAGS + 0xc00;
    const PROJ_DATUM: u32 = 0xa009_0003;
    g.put_u32(TAG_INDEX + 0x18, 4);
    let e = TAG_ENTRIES + 0x30;
    g.put_u32(e, u32::from_be_bytes(*b"proj"))
        .put_u32(e + 4, PROJ_DATUM)
        .put_u32(e + 8, PROJ_TAG)
        .map(PROJ_TAG, 0x400)
        .put_f32(PROJ_TAG + PROJ_INITIAL_VELOCITY, 400.0);
    // The rifle: one zoom level, one trigger firing once per pull, aim
    // assist to 20, a barrel of 6 rounds a second firing that projectile.
    let triggers = BR_TAG + 0x380;
    let barrel = BR_TAG + 0x200;
    g.put_u16(BR_TAG + WEAP_ZOOM_LEVELS, 1)
        .put_f32(BR_TAG + WEAP_CONE_RANGE, 20.0)
        .put_f32(BR_TAG + WEAP_PULL_RANGE, 30.0)
        .put_u32(BR_TAG + WEAP_TRIGGERS, 1)
        .put_u32(BR_TAG + WEAP_TRIGGERS + 4, triggers)
        .put_u16(triggers + TRIGGER_DEF_BEHAVIOUR, 1)
        .put_f32(barrel + BARREL_DEF_RATE, 6.0)
        .put_u16(barrel + BARREL_DEF_PROJECTILES, 1)
        .put_u32(barrel + BARREL_DEF_PROJECTILE, u32::from_be_bytes(*b"proj"))
        .put_u32(barrel + BARREL_DEF_PROJECTILE + 4, PROJ_DATUM);
    let a = read(&w.g, w.unit).unwrap();
    let p = a.held.expect("the rifle").profile;
    assert_eq!(
        p,
        Profile {
            trigger: Trigger::Single,
            zoom: 1,
            cone_range: 20.0,
            pull_range: 30.0,
            rate: 6.0,
            pellets: 1,
            shot_speed: 400.0,
        }
    );
    assert_eq!(p.role(), Role::Precise);
    // The pistol has no triggers block in this fake: no trigger, and no
    // aim assist, so weapon-blind (not a blade).
    let pp = a.other.expect("the pistol").profile;
    assert_eq!(pp.trigger, Trigger::Absent);
    assert_eq!(pp.role(), Role::Unknown);
    // A projectile reference to another group leaves the speed unknown.
    w.g.put_u32(TAG_ENTRIES + 0x30, u32::from_be_bytes(*b"weap"));
    let a = read(&w.g, w.unit).unwrap();
    assert_eq!(a.held.unwrap().profile.shot_speed, 0.0);
}

#[test]
fn decisions_read_only_the_held_magazines_reload_window() {
    let mut w = world();
    let mag = BODIES + 0x400 + WEAPON_MAGAZINES;
    assert!(!held_empty_magazine(&w.g, w.unit).unwrap());
    w.g.put_u16(mag + MAGAZINE_LOADED, 0);
    assert!(held_empty_magazine(&w.g, w.unit).unwrap());
    w.g.put_u16(mag + MAGAZINE_RESERVE, 0);
    assert!(!held_empty_magazine(&w.g, w.unit).unwrap(), "entirely dry");
    w.g.put_u16(mag + MAGAZINE_RESERVE, 10)
        .put_u8(w.unit + UNIT_WEAPON_INDEX, 2);
    assert!(
        !held_empty_magazine(&w.g, w.unit).unwrap(),
        "other gun holstered"
    );
}

#[test]
fn decisions_ground_scan_matches_live_items_to_their_spawn_and_clock() {
    use crate::decisions::supplies::Supply;
    const SCENARIO: u32 = 0x006a_0000;
    const SITES: u32 = SCENARIO + 0x1000;
    const COLLECTION: u32 = TAGS + 0xc00;
    const COLLECTION_ID: u32 = 0xd210_0003;
    let mut w = world();
    let g = &mut w.g;
    // An ordinary two-zoom gun, and a piece of equipment whose clock comes
    // from its item collection. The gun has a per-placement override.
    g.put_u16(BR_TAG + WEAP_ZOOM_LEVELS, 2)
        .put_u32(BR_TAG + WEAP_TRIGGERS, 1)
        .put_u32(BR_TAG + WEAP_TRIGGERS + 4, BR_TAG + 0x380)
        .put_u16(BR_TAG + 0x380 + TRIGGER_DEF_BEHAVIOUR, 1)
        .put_u8(OBJECTS_DATA + 4 * 0xc + 3, crate::guest::intel::EQUIPMENT)
        .put_u32(TAG_ENTRIES + 0x20, u32::from_be_bytes(*b"eqip"))
        .put_u16(FLAG_TAG + 0x12c, 2)
        .put_u8(BODIES + 3 * 0x400 + 0xaf, 0)
        .put_u8(BODIES + 4 * 0x400 + 0xaf, 1)
        .put_u32(points::SCENARIO.base, SCENARIO)
        .put_u32(SCENARIO + 0x120, 2)
        .put_u32(SCENARIO + 0x124, SITES)
        .put_f32(SITES + 0x40, 29.5)
        .put_u16(SITES + 0x0e, 13)
        .put_f32(SITES + 0x90 + 0x40, 40.0)
        .put_u32(SITES + 0x90 + 0x5c, COLLECTION_ID)
        .put_u32(TAG_INDEX + 0x18, 4)
        .put_u32(TAG_ENTRIES + 0x30, u32::from_be_bytes(*b"itmc"))
        .put_u32(TAG_ENTRIES + 0x34, COLLECTION_ID)
        .put_u32(TAG_ENTRIES + 0x38, COLLECTION)
        .put_u16(COLLECTION + 8, 31);
    let gun = Supply {
        site: 0,
        object: DatumHandle::new(0xb003, 3).raw(),
        spawn: [29.5, 0.0, 0.0],
        at: [30.0, 0.0, 0.0],
        respawn: Some(13 * 30),
        powerup: false,
    };
    let eq = Supply {
        site: 1,
        object: DatumHandle::new(0xb004, 4).raw(),
        spawn: [40.0, 0.0, 0.0],
        at: [40.0, 0.0, 0.0],
        respawn: Some(31 * 30),
        powerup: true,
    };
    for kind in [2, 3] {
        g.put_u16(FLAG_TAG + 0x12c, kind);
        let sample = ground(g).unwrap();
        assert_eq!(sample.weapons.len(), 1);
        assert_eq!(sample.supplies.unwrap(), [gun, eq], "equipment type {kind}");
    }
    // A grenade is equipment too. A dropped gun far from its placement
    // is still an ordinary pickup and cannot establish a spawn point.
    g.put_u16(FLAG_TAG + 0x12c, 6)
        .put_f32(BODIES + 3 * 0x400 + 0x64, 10.0);
    let sample = ground(g).unwrap();
    assert_eq!(sample.weapons.len(), 1);
    assert!(sample.supplies.unwrap().is_empty());

    // An unreadable equipment definition makes this an unknown supply
    // sample. It cannot hide the readable rifle or start a phantom clock.
    g.put_u32(TAG_ENTRIES + 0x28, 0x00dd_0000);
    let sample = ground(g).unwrap();
    assert!(sample.supplies.is_none());
    assert_eq!(sample.weapons.len(), 1);
}
