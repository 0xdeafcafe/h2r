//! The cheats over a fake game: one local player on a biped (the same
//! shape as `skulls_custom`'s tests), game time, and the globals.

use engine::datum::layout as dl;
use engine::testing::FakeGuest;
use engine::view::objects;

use super::*;
use crate::skulls_custom::{BODY_PARENT, UNIT_WEAPONS};

const PLAYERS_H: u32 = 0x0060_0000;
const OBJECTS_H: u32 = 0x0061_0000;
const PLAYER_DATA: u32 = 0x0062_0000;
const OBJECT_DATA: u32 = 0x0063_0000;
const BODY: u32 = 0x0064_0000;
const GLOBALS: u32 = 0x0065_0000;
const TIME: u32 = 0x0066_0000;
const SIM: u32 = 0x0067_0000;
const UNIT: u32 = 0xe001_0000;

/// A campaign game at the options' 30 ticks a second.
fn game() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x004c_0000, 0x1_0000)
        .map(0x004e_0000, 0x1_0000)
        .map(0x0051_0000, 0x1_0000);
    for a in [
        PLAYERS_H,
        OBJECTS_H,
        PLAYER_DATA,
        OBJECT_DATA,
        BODY,
        GLOBALS,
        TIME,
        SIM,
    ] {
        g.map(a, 0x1000);
    }
    g.put_u32(engine::points::PLAYERS.base, PLAYERS_H)
        .put_u32(engine::points::OBJECTS.base, OBJECTS_H)
        .put_u32(GAME_GLOBALS, GLOBALS)
        .put_u32(GLOBALS + GLOBALS_MODE, 1)
        .put_u16(GLOBALS + GLOBALS_TICK_RATE, 30)
        .put_u32(engine::points::GAME_TIME.base, TIME)
        .put_u16(TIME + TIME_RATE, 30)
        .put_f32(TIME + TIME_TICK_SECONDS, 1.0 / 30.0)
        .put_f32(TIME + TIME_SPEED, 1.0)
        .put_u32(engine::points::SIM_GLOBALS.base, SIM)
        .put_u32(SIM + 8, 1);
    for (h, size, data) in [
        (PLAYERS_H, 0x21c, PLAYER_DATA),
        (OBJECTS_H, 0xc, OBJECT_DATA),
    ] {
        g.put_u32(h + dl::ELEMENT_SIZE, size)
            .put_u32(h + dl::HIGH_WATER, 1)
            .put_u32(h + dl::DATA, data);
    }
    g.put_u16(PLAYER_DATA, 0xe0ff)
        .put_u16(PLAYER_DATA + 0x28, 0)
        .put_u32(PLAYER_DATA + 0x2c, UNIT);
    g.put_u16(OBJECT_DATA, 0xe001)
        .put_u8(OBJECT_DATA + 3, objects::BIPED)
        .put_u32(OBJECT_DATA + 8, BODY);
    g.put_u32(BODY + BODY_PARENT, u32::MAX)
        .put_u8(BODY + UNIT_GRENADES, 1);
    for i in 0..4 {
        g.put_u32(BODY + UNIT_WEAPONS + 4 * i, u32::MAX);
    }
    g
}

fn flags(g: &FakeGuest) -> u16 {
    g.u16(BODY + BODY_DAMAGE_FLAGS).unwrap()
}

fn set(c: &mut Cheats, name: &str, v: Value) {
    c.set(&json!({"name": name, "value": v}), None).unwrap();
}

#[test]
fn god_mode_holds_and_undoes_only_what_it_set() {
    let mut g = game();
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    // The game clears it (a script): set again next tick.
    g.put_u16(BODY + BODY_DAMAGE_FLAGS, 0);
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    set(&mut c, "god", json!(false));
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g), 0);
    // A unit the game made invulnerable itself is left so.
    let mut g = game();
    g.put_u16(BODY + BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE | 1);
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    c.tick(&mut g, None).unwrap();
    set(&mut c, "god", json!(false));
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g), CANNOT_TAKE_DAMAGE | 1);
}

#[test]
fn infinite_ammo_tops_the_grenades_and_off_writes_nothing() {
    let mut g = game();
    let mut c = Cheats::default();
    c.tick(&mut g, None).unwrap();
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), 1);
    set(&mut c, "infinite_ammo", json!(true));
    c.tick(&mut g, None).unwrap();
    let cap = crate::skulls_custom::GRENADE_CAP as u8;
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), cap);
    assert_eq!(g.u8(BODY + UNIT_GRENADES + 1).unwrap(), cap);
}

#[test]
fn bottomless_clip_fills_the_loaded_rounds() {
    let (w, def, block) = (0x10_0000, 0x20_0000, 0x30_0000);
    let mut g = FakeGuest::new();
    g.map(w, 0x1000).map(def, 0x1000).map(block, 0x1000);
    g.put_u32(def + WEAP_MAGAZINE_COUNT, 1)
        .put_u32(def + WEAP_MAGAZINE_BLOCK, block)
        .put_u16(block + WEAP_LOADED_MAX, 36)
        .put_u16(block + 0xc, 108);
    let mag = w + WEAPON_MAGAZINES;
    g.put_u16(mag + MAGAZINE_LOADED, 5).put_u16(mag + 2, 20);
    assert!(load(&mut g, w, def).unwrap());
    assert_eq!(g.u16(mag + MAGAZINE_LOADED).unwrap(), 36);
    assert_eq!(
        g.u16(mag + 2).unwrap(),
        20,
        "the reserve is infinite ammo's"
    );
    assert!(!load(&mut g, w, def).unwrap(), "full already");
    // An energy weapon: its charge.
    let used = w + crate::skulls_custom::WEAPON_CHARGE_USED;
    g.put_u32(def + WEAP_MAGAZINE_COUNT, 0).put_f32(used, 0.7);
    assert!(load(&mut g, w, def).unwrap());
    assert_eq!(g.f32(used).unwrap(), 0.0);
}

#[test]
fn speed_fields_follow_the_games_helper() {
    assert_eq!(speed_fields(30, 1.0), (30, 1.0 / 30.0, 1.0));
    // Half speed: twice the ticks, each half as long, at half speed.
    assert_eq!(speed_fields(30, 0.5), (60, 1.0 / 60.0, 0.5));
    assert_eq!(speed_fields(30, 2.0), (15, 1.0 / 15.0, 2.0));
    assert_eq!(speed_fields(30, 0.25), (120, 1.0 / 120.0, 0.25));
    assert_eq!(speed_fields(30, 1.5), (20, 1.0 / 20.0, 1.5));
    // The game time a real second runs: rate * speed * seconds per tick.
    for s in SPEEDS {
        let (r, secs, sp) = speed_fields(30, s);
        assert!((f32::from(r) * sp * secs - s).abs() < 1e-6, "{s}");
    }
}

#[test]
fn game_speed_holds_and_is_put_back_once() {
    let mut g = game();
    let mut c = Cheats::default();
    set(&mut c, "game_speed", json!(0.5));
    c.tick(&mut g, None).unwrap();
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 60);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.5);
    // A script's ramp writes the speed: held.
    g.put_f32(TIME + TIME_SPEED, 0.9);
    c.tick(&mut g, None).unwrap();
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.5);
    set(&mut c, "game_speed", json!(1.0));
    c.tick(&mut g, None).unwrap();
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 30);
    assert_eq!(g.f32(TIME + TIME_TICK_SECONDS).unwrap(), 1.0 / 30.0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    // Then left alone.
    g.put_f32(TIME + TIME_SPEED, 0.9);
    c.tick(&mut g, None).unwrap();
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.9);
    // Only the offered speeds.
    assert!(
        c.set(&json!({"name": "game_speed", "value": 0.0}), None)
            .is_err()
    );
}

#[test]
fn nothing_happens_in_the_menus() {
    let mut g = game();
    g.put_u32(GLOBALS + GLOBALS_MODE, 3);
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    set(&mut c, "game_speed", json!(2.0));
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g), 0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    // Multiplayer (split screen) is playing.
    g.put_u32(GLOBALS + GLOBALS_MODE, 2);
    c.tick(&mut g, None).unwrap();
    assert_eq!(flags(&g), CANNOT_TAKE_DAMAGE);
}

#[test]
fn co_op_and_lockstep_refuse_and_switch_them_off() {
    assert_eq!(refusal(Some(0), false), None);
    assert_eq!(refusal(Some(1), false), None);
    assert_eq!(refusal(None, false), None);
    for m in [2, 3] {
        assert!(
            refusal(Some(m), false)
                .unwrap()
                .starts_with("a co-op session")
        );
    }
    for m in [4, 5] {
        assert!(
            refusal(Some(m), false)
                .unwrap()
                .starts_with("a networked session")
        );
    }
    assert!(
        refusal(Some(1), true)
            .unwrap()
            .starts_with("a lockstep run")
    );
    let mut g = game();
    assert_eq!(sim_mode(&g), Some(1));
    g.put_u32(SIM + 8, 3);
    assert_eq!(sim_mode(&g), Some(3));
    g.put_u32(engine::points::SIM_GLOBALS.base, 0);
    assert_eq!(sim_mode(&g), None);
    // cheat.set refuses; cheat.list says why.
    let mut c = Cheats::default();
    let why = refusal(Some(3), false);
    let e = c
        .set(&json!({"name": "god", "value": true}), why)
        .unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(!c.god);
    assert_eq!(c.list(why)["refused"], json!(why.unwrap()));
    // On when the session turns co-op: off, the flag and speed undone.
    let mut g = game();
    set(&mut c, "god", json!(true));
    set(&mut c, "game_speed", json!(0.5));
    c.tick(&mut g, None).unwrap();
    let said = c.tick(&mut g, why).unwrap();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(!c.any());
    assert_eq!(flags(&g), 0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 30);
}

#[test]
fn the_list_is_what_the_menu_reads() {
    let mut c = Cheats::default();
    let l = c.list(None);
    let names: Vec<&str> = l["cheats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["god", "infinite_ammo", "bottomless_clip", "game_speed"]
    );
    assert_eq!(l["refused"], Value::Null);
    assert_eq!(
        l["cheats"][3]["choices"][2],
        json!({"value": 1.0, "label": "1x"})
    );
    // A set answers with the new list; the choice's value is its own.
    let r = c
        .set(&json!({"name": "game_speed", "value": 0.25}), None)
        .unwrap();
    assert_eq!(
        r["cheats"][3]["value"],
        r["cheats"][3]["choices"][0]["value"]
    );
    assert!(
        c.set(&json!({"name": "noclip", "value": true}), None)
            .is_err()
    );
    assert!(c.set(&json!({"name": "god", "value": 1}), None).is_err());
}
