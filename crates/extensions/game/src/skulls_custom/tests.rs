use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::testing::FakeGuest;
use settings::Def;

fn store() -> Store {
    let defs: Vec<&'static Def> = SKULLS.iter().collect();
    Store::defaults(&defs)
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn ours_follow_the_engines_fifteen() {
    assert_eq!(OURS, 6);
    assert_eq!(skulls::TOTAL, 21);
    assert_eq!(
        [
            ACROPHOBIA,
            LOW_GRAVITY,
            FALL_DAMAGE,
            UNLIMITED_AMMO,
            UNLIMITED_GRENADES,
            INVINCIBLE
        ],
        [15, 16, 17, 18, 19, 20]
    );
    let labels: Vec<_> = (COUNT..skulls::TOTAL).map(|i| SKULLS[i].label).collect();
    assert_eq!(
        labels,
        [
            "Acrophobia",
            "Low Gravity",
            "Fall Damage",
            "Unlimited Ammo",
            "Unlimited Grenades",
            "Invincible"
        ]
    );
    assert!((COUNT..skulls::TOTAL).all(skulls::is_ours));
    assert!(!skulls::is_ours(14) && !skulls::is_ours(21));
    // Ours are none of retail's, and every one is described and kept.
    for i in COUNT..skulls::TOTAL {
        assert!(!ABOUT[i].legendary, "{}", ABOUT[i].name);
        assert!(!ABOUT[i].effect.is_empty());
        assert_eq!(ABOUT[i].name, SKULLS[i].label);
        assert!(
            crate::setting_defs::ALL
                .iter()
                .any(|d| d.key == SKULLS[i].key)
        );
    }
    // The same numbers on the wire.
    assert_eq!(live_proto::skulls::OURS, COUNT as u8..skulls::TOTAL as u8);
}

#[test]
fn active_sets_keep_only_ours() {
    let a = Active::of(&[0, 12, FALL_DAMAGE, INVINCIBLE, 40]);
    assert_eq!(a.indices(), [FALL_DAMAGE, INVINCIBLE]);
    assert!(a.has(INVINCIBLE) && !a.has(LOW_GRAVITY) && !a.has(12));
    assert_eq!(a.names(), "Fall Damage, Invincible");
    assert_eq!(Active::default().names(), "none");
}

#[test]
fn one_spot_per_skull_on_six_levels() {
    let mut maps: Vec<_> = SPOTS.iter().map(|s| s.map).collect();
    maps.sort();
    maps.dedup();
    assert_eq!(maps.len(), OURS);
    let mut ours: Vec<_> = SPOTS.iter().map(|s| s.skull).collect();
    ours.sort();
    assert_eq!(ours, (COUNT..skulls::TOTAL).collect::<Vec<_>>());
    let level = |map: &str| match map {
        "03a_oldmombasa" => "Outskirts",
        "03b_newmombasa" => "Metropolis",
        "05a_deltaapproach" => "Delta Halo",
        "06a_sentinelwalls" => "Sacred Icon",
        "08a_deltacliffs" => "Uprising",
        "08b_deltacontrol" => "The Great Journey",
        _ => "?",
    };
    for s in &SPOTS {
        assert_eq!(ABOUT[s.skull].level, level(s.map), "{}", s.map);
        assert_eq!(spot_for(s.map), Some(s));
        assert!(
            s.at.iter().all(|v| v.is_finite()) && s.at != [0.0; 3],
            "{} unplaced",
            s.map
        );
        assert!(!s.hint.is_empty());
        assert!(s.bsp >= 0);
    }
    assert_eq!(spot_for("01a_tutorial"), None);
    assert_eq!(spot_for("ascension"), None);
}

#[test]
fn the_skull_tag_is_found_by_name() {
    let rows = [
        (
            0xe100_0001,
            *b"weap",
            "objects\\weapons\\rifle\\battle_rifle\\battle_rifle",
        ),
        (
            0xe100_0002,
            *b"bipd",
            "objects\\weapons\\multiplayer\\ball\\head_sp",
        ),
        (
            0xe100_0003,
            *b"weap",
            "objects\\weapons\\multiplayer\\ball\\head",
        ),
        (
            0xe100_0004,
            *b"weap",
            "Objects\\Weapons\\Multiplayer\\Ball\\Head_SP",
        ),
    ];
    assert_eq!(
        skull_tag(rows.iter().copied()),
        Some(0xe100_0004),
        "head_sp first"
    );
    assert_eq!(
        skull_tag(rows[..3].iter().copied()),
        Some(0xe100_0003),
        "head, as High Charity has"
    );
    assert_eq!(
        skull_tag(rows[..2].iter().copied()),
        None,
        "only a weap counts"
    );
}

#[test]
fn the_skull_is_placed_once_on_its_bsp_until_collected() {
    let s = &SPOTS[0];
    let tag = Some(0xe100_0001);
    assert!(should_spawn(true, s, Some(s.bsp), true, tag, false));
    assert!(
        !should_spawn(false, s, Some(s.bsp), true, tag, false),
        "not a campaign"
    );
    assert!(
        !should_spawn(true, s, Some(s.bsp + 1), true, tag, false),
        "another bsp"
    );
    assert!(!should_spawn(true, s, None, true, tag, false));
    assert!(
        !should_spawn(true, s, Some(s.bsp), false, tag, false),
        "everyone has it"
    );
    assert!(
        !should_spawn(true, s, Some(s.bsp), true, None, false),
        "no tag"
    );
    assert!(
        !should_spawn(true, s, Some(s.bsp), true, tag, true),
        "already there"
    );
}

#[test]
fn a_spot_waits_for_anyone_still_missing_its_skull() {
    let st = store();
    let both = names(&["Chief", "Arbiter"]);
    assert!(uncollected(&st, &both, INVINCIBLE));
    skulls::record_pickup(&st, &names(&["Chief"]), INVINCIBLE);
    assert!(uncollected(&st, &both, INVINCIBLE), "the Arbiter has not");
    assert!(!uncollected(&st, &names(&["Chief"]), INVINCIBLE));
    skulls::record_pickup(&st, &both, INVINCIBLE);
    assert!(!uncollected(&st, &both, INVINCIBLE));
    assert!(!uncollected(&st, &[], INVINCIBLE), "nobody to find it");
}

#[test]
fn ours_are_locked_until_collected_then_toggle_and_start_with_the_campaign() {
    let st = store();
    let chief = names(&["Chief"]);
    assert_eq!(
        skulls::state(&st, "Chief", ACROPHOBIA),
        Some(skulls::State::Locked)
    );
    assert!(skulls::toggle(&st, "Chief", "skulls.acrophobia").is_err());
    let log = skulls::record_pickup(&st, &chief, ACROPHOBIA);
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("Acrophobia for Chief"), "{log:?}");
    skulls::record_pickup(&st, &chief, LOW_GRAVITY);
    assert_eq!(
        skulls::state(&st, "Chief", ACROPHOBIA),
        Some(skulls::State::On)
    );
    assert_eq!(
        skulls::custom_selection(&st, &chief),
        [ACROPHOBIA, LOW_GRAVITY]
    );
    // Retail's byte array never sees ours.
    assert!(skulls::selection(&st, &chief).is_none_or(|set| set.iter().all(|on| !on)));
    assert_eq!(
        skulls::toggle(&st, "Chief", "skulls.acrophobia").unwrap(),
        "skulls: Acrophobia off for Chief"
    );
    assert_eq!(skulls::custom_selection(&st, &chief), [LOW_GRAVITY]);
    assert_eq!(
        skulls::collected(&st, "Chief"),
        [(ACROPHOBIA, false), (LOW_GRAVITY, true)]
    );
}

#[test]
fn a_pickup_is_the_skull_in_a_local_units_hands() {
    let skull = 0xe200_0010;
    let units = vec![
        (0xe300_0001, vec![0xe200_0001]),
        (0xe300_0002, vec![0xe200_0002, skull]),
    ];
    assert_eq!(held_by(skull, None, &units), Some(1), "in a weapon slot");
    assert_eq!(
        held_by(skull, Some(0xe300_0001), &units),
        Some(0),
        "attached"
    );
    assert_eq!(held_by(skull, None, &units[..1]), None);
    assert_eq!(
        held_by(skull, Some(0xe300_0099), &units[..1]),
        None,
        "someone else's"
    );
    assert_eq!(held_by(skull, None, &[]), None);
}

#[test]
fn low_gravity_halves_what_the_game_sets_and_gives_it_back() {
    let mut g = Gravity::default();
    assert_eq!(gravity_step(1.0, 4.0, &mut g), None);
    assert_eq!(gravity_step(GRAVITY_SCALE, 4.0, &mut g), Some(2.0));
    assert_eq!(
        gravity_step(GRAVITY_SCALE, 2.0, &mut g),
        None,
        "ours already"
    );
    // A script sets it (physics_set_gravity): scaled in turn.
    assert_eq!(gravity_step(GRAVITY_SCALE, 8.0, &mut g), Some(4.0));
    assert_eq!(gravity_step(1.0, 4.0, &mut g), Some(8.0), "the game's back");
    assert_eq!(gravity_step(1.0, 8.0, &mut g), None);
    // Turned off after the game changed it again: left alone.
    gravity_step(GRAVITY_SCALE, 4.0, &mut g);
    assert_eq!(gravity_step(1.0, 3.0, &mut g), None);
}

#[test]
fn a_variants_gravity_composes_with_low_gravity_on_physics_memory() {
    // Synthetic physics constants: the pointer, then gravity first.
    const PC: u32 = 0x8100_0000;
    let mut f = FakeGuest::new();
    f.map(PHYSICS_CONSTANTS & !0xfff, 0x1000).map(PC, 0x1000);
    f.set_u32(PHYSICS_CONSTANTS, PC).unwrap();
    f.set_f32(PC, 4.0).unwrap();
    let mut g = Gravity::default();
    let tick = |f: &mut FakeGuest, scale: f32, g: &mut Gravity| {
        let pc = f.u32(PHYSICS_CONSTANTS).unwrap();
        let now = f.f32(pc).unwrap();
        if let Some(v) = gravity_step(scale, now, g) {
            f.set_f32(pc, v).unwrap();
        }
        f.f32(pc).unwrap()
    };
    // A 50% variant alone, then Low Gravity on top: a quarter.
    assert_eq!(tick(&mut f, 0.5, &mut g), 2.0);
    assert_eq!(tick(&mut f, 0.5, &mut g), 2.0, "held, not halved again");
    assert_eq!(tick(&mut f, 0.5 * GRAVITY_SCALE, &mut g), 1.0);
    // The skull off again: the variant's half of the game's value.
    assert_eq!(tick(&mut f, 0.5, &mut g), 2.0);
    // A 200% variant.
    assert_eq!(tick(&mut f, 2.0, &mut g), 8.0);
    // Nothing: the game's own back.
    assert_eq!(tick(&mut f, 1.0, &mut g), 4.0);
    assert_eq!(g, Gravity::default());
}

#[test]
fn flight_follows_the_view_and_crouch_drops() {
    assert_eq!(fly(-5.0, 1.0, 0.0), Some(FLY_SPEED), "look up: climb");
    assert_eq!(fly(-5.0, 0.0, 0.0), Some(0.0), "level: hover");
    assert_eq!(
        fly(-5.0, -0.5, 0.0),
        Some(-0.5 * FLY_SPEED),
        "look down: sink slowly"
    );
    assert_eq!(fly(6.0, 1.0, 0.0), None, "already rising faster (a jump)");
    assert_eq!(fly(-5.0, 1.0, 1.0), None, "crouched: drop");
    assert_eq!(fly(f32::NAN, 1.0, 0.0), None);
}

#[test]
fn hard_landings_hurt_shield_first() {
    assert_eq!(fall_damage(FALL_SAFE), 0.0);
    assert_eq!(fall_damage(2.0), 0.0);
    assert!((fall_damage(FALL_SAFE + 2.0) - 2.0 * FALL_DAMAGE_PER_SPEED).abs() < 1e-6);
    assert_eq!(fall_damage(f32::INFINITY), 0.0);
    assert_eq!(take(1.0, 1.0, 0.5), (0.5, 1.0, false));
    assert_eq!(take(0.25, 1.0, 0.5), (0.0, 0.75, false));
    assert_eq!(take(0.0, 0.5, 2.0), (0.0, 0.0, true));
    assert_eq!(
        landed(
            Fall {
                airborne: true,
                vz: -7.0
            },
            false
        ),
        Some(7.0)
    );
    assert_eq!(
        landed(
            Fall {
                airborne: true,
                vz: -7.0
            },
            true
        ),
        None,
        "still falling"
    );
    assert_eq!(
        landed(
            Fall {
                airborne: false,
                vz: -7.0
            },
            false
        ),
        None
    );
    assert_eq!(
        landed(
            Fall {
                airborne: true,
                vz: 1.0
            },
            false
        ),
        None,
        "bumped a ceiling"
    );
    assert_eq!(topped([0, 2]), [GRENADE_CAP, GRENADE_CAP]);
    assert_eq!(topped([6, 4]), [6, 4], "more than the cap is kept");
}

#[test]
fn unlimited_ammo_fills_the_reserve_not_the_magazine() {
    let (w, def, block) = (0x10_0000, 0x20_0000, 0x30_0000);
    let mut g = FakeGuest::new();
    g.map(w, 0x1000).map(def, 0x1000).map(block, 0x1000);
    g.put_u32(def + WEAP_MAGAZINE_COUNT, 2)
        .put_u32(def + WEAP_MAGAZINE_BLOCK, block)
        .put_u16(block + WEAP_RESERVE_MAX, 108)
        .put_u16(block + WEAP_MAGAZINE_STRIDE + WEAP_RESERVE_MAX, 8);
    let mag = |i: u32| w + WEAPON_MAGAZINES + i * MAGAZINE_STRIDE;
    g.put_u16(mag(0) + MAGAZINE_RESERVE, 12)
        .put_u16(mag(0) + 4, 3)
        .put_u16(mag(1) + MAGAZINE_RESERVE, 8);
    assert!(refill(&mut g, w, def).unwrap());
    assert_eq!(g.u16(mag(0) + MAGAZINE_RESERVE).unwrap(), 108);
    assert_eq!(
        g.u16(mag(0) + 4).unwrap(),
        3,
        "the loaded rounds: still a reload"
    );
    assert_eq!(g.u16(mag(1) + MAGAZINE_RESERVE).unwrap(), 8);
    assert!(!refill(&mut g, w, def).unwrap(), "full already");
    // An energy weapon: no magazines, its charge kept full.
    g.put_u32(def + WEAP_MAGAZINE_COUNT, 0)
        .put_f32(w + WEAPON_CHARGE_USED, 0.4);
    assert!(refill(&mut g, w, def).unwrap());
    assert_eq!(g.f32(w + WEAPON_CHARGE_USED).unwrap(), 0.0);
}

// ---- a fake game: one local player on a biped ----

const PLAYERS_H: u32 = 0x0060_0000;
const OBJECTS_H: u32 = 0x0061_0000;
const PLAYER_DATA: u32 = 0x0062_0000;
const OBJECT_DATA: u32 = 0x0063_0000;
const BODY: u32 = 0x0064_0000;
const UNIT: u32 = 0xe001_0000; // salt 0xe001, slot 0

fn game() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x004e_0000, 0x1_0000);
    for a in [PLAYERS_H, OBJECTS_H, PLAYER_DATA, OBJECT_DATA, BODY] {
        g.map(a, 0x1000);
    }
    g.put_u32(guest_layout::points::PLAYERS.base, PLAYERS_H)
        .put_u32(guest_layout::points::OBJECTS.base, OBJECTS_H);
    for (h, size, data) in [
        (PLAYERS_H, 0x21c, PLAYER_DATA),
        (OBJECTS_H, 0xc, OBJECT_DATA),
    ] {
        g.put_u32(h + dl::ELEMENT_SIZE, size)
            .put_u32(h + dl::HIGH_WATER, 1)
            .put_u32(h + dl::DATA, data);
    }
    // Player 0: local user 0 on unit UNIT.
    g.put_u16(PLAYER_DATA, 0xe0ff)
        .put_u16(PLAYER_DATA + 0x28, 0)
        .put_u32(PLAYER_DATA + 0x2c, UNIT);
    // Object 0: a biped.
    g.put_u16(OBJECT_DATA, 0xe001)
        .put_u8(OBJECT_DATA + 3, objects::BIPED)
        .put_u32(OBJECT_DATA + 8, BODY);
    g.put_u32(BODY + BODY_PARENT, u32::MAX)
        .put_f32(BODY + BODY_HEALTH, 1.0)
        .put_f32(BODY + BODY_SHIELD, 1.0)
        .put_u8(BODY + UNIT_GRENADES, 1)
        .put_u8(BODY + BIPED_MODE, MODE_GROUND);
    for i in 0..4 {
        g.put_u32(BODY + UNIT_WEAPONS + 4 * i, u32::MAX);
    }
    g
}

fn tick(g: &mut FakeGuest, a: Active, falls: &mut HashMap<u32, Fall>) -> Effects {
    let units = local_units(g).unwrap();
    apply_units(g, a, &units, falls).unwrap()
}

#[test]
fn the_local_players_units_are_found() {
    let mut g = game();
    assert_eq!(
        local_units(&g).unwrap(),
        [LocalUnit {
            user: 0,
            unit: UNIT,
            body: BODY,
            kind: objects::BIPED
        }]
    );
    // Not local (a remote player): none.
    g.put_u16(PLAYER_DATA + 0x28, 0xffff);
    assert!(local_units(&g).unwrap().is_empty());
    // No players array yet (the menus): none, no fault.
    g.put_u32(guest_layout::points::PLAYERS.base, 0);
    assert!(local_units(&g).unwrap().is_empty());
}

#[test]
fn invincible_and_grenades_hold_every_tick() {
    let mut g = game();
    let mut falls = HashMap::new();
    tick(
        &mut g,
        Active::of(&[INVINCIBLE, UNLIMITED_GRENADES]),
        &mut falls,
    );
    assert_eq!(
        g.u16(BODY + BODY_DAMAGE_FLAGS).unwrap() & CANNOT_TAKE_DAMAGE,
        CANNOT_TAKE_DAMAGE
    );
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), GRENADE_CAP as u8);
    assert_eq!(g.u8(BODY + UNIT_GRENADES + 1).unwrap(), GRENADE_CAP as u8);
    // Off: nothing written.
    let mut g = game();
    tick(&mut g, Active::default(), &mut falls);
    assert_eq!(g.u16(BODY + BODY_DAMAGE_FLAGS).unwrap(), 0);
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), 1);
}

#[test]
fn acrophobia_lifts_an_airborne_biped_only() {
    let mut g = game();
    let mut falls = HashMap::new();
    let vz = BODY + BODY_VELOCITY + 8;
    g.put_f32(vz, -3.0).put_f32(BODY + UNIT_AIM + 8, 0.5);
    tick(&mut g, Active::of(&[ACROPHOBIA]), &mut falls);
    assert_eq!(g.f32(vz).unwrap(), -3.0, "on the ground");
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    tick(&mut g, Active::of(&[ACROPHOBIA]), &mut falls);
    assert_eq!(g.f32(vz).unwrap(), 0.5 * FLY_SPEED);
    g.put_f32(vz, -3.0).put_f32(BODY + BIPED_CROUCH, 1.0);
    tick(&mut g, Active::of(&[ACROPHOBIA]), &mut falls);
    assert_eq!(g.f32(vz).unwrap(), -3.0, "crouched");
}

#[test]
fn fall_damage_on_landing_and_a_kill_when_drained() {
    let mut g = game();
    let mut falls = HashMap::new();
    let a = Active::of(&[FALL_DAMAGE]);
    let vz = BODY + BODY_VELOCITY + 8;
    // A small hop: nothing.
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE).put_f32(vz, -2.0);
    tick(&mut g, a, &mut falls);
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND).put_f32(vz, 0.0);
    assert!(tick(&mut g, a, &mut falls).lines.is_empty());
    assert_eq!(g.f32(BODY + BODY_SHIELD).unwrap(), 1.0);
    // A drop landing at 8 wu/s: 1.5 off, the shield then half the health.
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE).put_f32(vz, -8.0);
    tick(&mut g, a, &mut falls);
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND).put_f32(vz, 0.0);
    let fx = tick(&mut g, a, &mut falls);
    assert_eq!(fx.lines.len(), 1);
    assert!(
        fx.lines[0].starts_with("skulls: Fall Damage: player 0 landed at 8.00"),
        "{fx:?}"
    );
    assert_eq!(g.f32(BODY + BODY_SHIELD).unwrap(), 0.0);
    assert!((g.f32(BODY + BODY_HEALTH).unwrap() - 0.5).abs() < 1e-5);
    assert!(fx.kills.is_empty());
    // Another: drained, killed as unit_kill does.
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE).put_f32(vz, -8.0);
    tick(&mut g, a, &mut falls);
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    let fx = tick(&mut g, a, &mut falls);
    assert_eq!(fx.kills, [UNIT]);
    assert_eq!(g.u16(BODY + BODY_DAMAGE_FLAGS).unwrap() & KILL, KILL);
    // With Invincible as well: no harm.
    let mut g = game();
    let a = Active::of(&[FALL_DAMAGE, INVINCIBLE]);
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE)
        .put_f32(vz, -20.0);
    tick(&mut g, a, &mut falls);
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    assert!(tick(&mut g, a, &mut falls).lines.is_empty());
    assert_eq!(g.f32(BODY + BODY_HEALTH).unwrap(), 1.0);
}

#[test]
fn the_reveal_line_names_the_spot_and_the_distance() {
    let s = Spot {
        skull: INVINCIBLE,
        map: "08b_deltacontrol",
        bsp: 1,
        at: [3.0, 4.0, 0.0],
        hint: "up high",
    };
    assert_eq!(
        reveal_line(&s, "placed", &[(0, [0.0, 0.0, 0.0])]),
        "skulls reveal: Invincible on 08b_deltacontrol (bsp 1) at (3.00, 4.00, 0.00) [up high]: placed; player 0 5.0 wu away"
    );
    assert!(reveal_line(&s, "at boot", &[]).ends_with("at boot; no local player"));
}

#[test]
fn the_found_message_is_ours() {
    for (i, d) in SKULLS.iter().enumerate().skip(COUNT) {
        assert_eq!(
            crate::skulls_menu::found_text(i),
            format!("Skull found: {}", d.label)
        );
    }
}

/// A variant's Unlimited Grenades tops up every player's unit, a remote
/// player's too (the host's counts are the game's), and leaves a unit with
/// more alone.
#[test]
fn a_variants_unlimited_grenades_reach_every_players_unit() {
    let mut g = game();
    g.put_u16(PLAYER_DATA + 0x28, 0xffff); // a remote player
    assert!(local_units(&g).unwrap().is_empty());
    assert_eq!(player_units(&g, false).unwrap().len(), 1);
    assert_eq!(variant_grenades(&mut g).unwrap(), 1);
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), GRENADE_CAP as u8);
    assert_eq!(g.u8(BODY + UNIT_GRENADES + 1).unwrap(), GRENADE_CAP as u8);
    assert_eq!(variant_grenades(&mut g).unwrap(), 0, "already full");
    g.put_u8(BODY + UNIT_GRENADES, 7);
    assert_eq!(variant_grenades(&mut g).unwrap(), 0);
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), 7, "more is kept");
    // No players array (the menus): nothing, no fault.
    g.put_u32(guest_layout::points::PLAYERS.base, 0);
    assert_eq!(variant_grenades(&mut g).unwrap(), 0);
}

#[test]
fn a_snapshot_carries_the_runs_skull_state() {
    let a = Custom::new(Arc::new(store()), None, None, false);
    let saved_campaign = CAMPAIGN.load(Ordering::SeqCst);
    CAMPAIGN.store(Active::of(&[LOW_GRAVITY]).0, Ordering::SeqCst);
    a.said.store(0b101, Ordering::SeqCst);
    a.found
        .store(Active::of(&[FALL_DAMAGE]).0, Ordering::SeqCst);
    a.ticks.store(812, Ordering::SeqCst);
    *a.level.lock().unwrap() = Level {
        key: Some(("03a_oldmombasa".into(), 0xe174_0000)),
        spot: Some(&SPOTS[2]),
        tag: Some(0xe1b2_0042),
        object: None,
        uncollected: true,
        queued: false,
    };
    *a.gravity.lock().unwrap() = Gravity {
        written: Some(2.5),
        game: Some(5.0),
    };
    a.falls.lock().unwrap().insert(
        0xe2a0_0003,
        Fall {
            airborne: true,
            vz: -7.25,
        },
    );
    a.falls.lock().unwrap().insert(
        0xe2a0_0001,
        Fall {
            airborne: false,
            vz: 0.0,
        },
    );
    assert_eq!(a.snapshot_live(), None);
    let bytes = a.save_state();

    let b = Custom::new(Arc::new(store()), None, None, false);
    CAMPAIGN.store(0, Ordering::SeqCst);
    b.load_state(&bytes).unwrap();
    assert_eq!(campaign(), Active::of(&[LOW_GRAVITY]));
    assert_eq!(b.found.load(Ordering::SeqCst), Active::of(&[FALL_DAMAGE]).0);
    assert_eq!(b.ticks.load(Ordering::SeqCst), 812);
    {
        let level = b.level.lock().unwrap();
        assert!(std::ptr::eq(level.spot.unwrap(), &SPOTS[2]));
        assert_eq!(level.key, Some(("03a_oldmombasa".into(), 0xe174_0000)));
        assert!(level.uncollected);
    }
    assert_eq!(b.falls.lock().unwrap()[&0xe2a0_0003].vz, -7.25);
    assert_eq!(b.save_state(), bytes, "the same state again");
    // Damaged bytes change nothing.
    assert!(b.load_state(&bytes[..bytes.len() - 1]).is_err());
    assert_eq!(b.save_state(), bytes);

    // Queued guest calls hold a snapshot off.
    b.work.lock().unwrap().push(Work::Wake { datum: 1 });
    assert!(b.snapshot_live().unwrap().contains("queued"));
    CAMPAIGN.store(saved_campaign, Ordering::SeqCst);
}
