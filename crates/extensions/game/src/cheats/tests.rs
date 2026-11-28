//! The cheats over a fake game: one local player on a biped (the same
//! shape as `skulls_custom`'s tests), game time, and the globals.

use guest_layout::datum::layout as dl;
use guest_layout::testing::FakeGuest;
use guest_layout::view::objects;

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
const HUD: u32 = 0x0068_0000;
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
        HUD,
    ] {
        g.map(a, 0x1000);
    }
    g.put_u32(guest_layout::points::PLAYERS.base, PLAYERS_H)
        .put_u32(guest_layout::points::OBJECTS.base, OBJECTS_H)
        .put_u32(GAME_GLOBALS, GLOBALS)
        .put_u32(GLOBALS + GLOBALS_MODE, 1)
        .put_u16(GLOBALS + GLOBALS_TICK_RATE, 30)
        .put_u32(guest_layout::points::GAME_TIME.base, TIME)
        .put_u16(TIME + TIME_RATE, 30)
        .put_f32(TIME + TIME_TICK_SECONDS, 1.0 / 30.0)
        .put_f32(TIME + TIME_SPEED, 1.0)
        .put_u32(guest_layout::points::SIM_GLOBALS.base, SIM)
        .put_u32(SIM + 8, 1)
        .put_u32(HUD_GLOBALS, HUD)
        .put_u8(HUD + HUD_SHOWN, 1);
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

/// Nothing refused.
const OK: Refusals = Refusals {
    all: None,
    simulation: None,
    clock: None,
};

fn set(c: &mut Cheats, name: &str, v: Value) {
    c.set(&json!({"name": name, "value": v}), &OK, true).unwrap();
}

#[test]
fn god_mode_holds_and_undoes_only_what_it_set() {
    let mut g = game();
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    // The game clears it (a script): set again next tick.
    g.put_u16(BODY + BODY_DAMAGE_FLAGS, 0);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    set(&mut c, "god", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g), 0);
    // A unit the game made invulnerable itself is left so.
    let mut g = game();
    g.put_u16(BODY + BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE | 1);
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "god", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g), CANNOT_TAKE_DAMAGE | 1);
}

#[test]
fn infinite_ammo_tops_the_grenades_and_off_writes_nothing() {
    let mut g = game();
    let mut c = Cheats::default();
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(BODY + UNIT_GRENADES).unwrap(), 1);
    set(&mut c, "infinite_ammo", json!(true));
    c.tick(&mut g, &OK).unwrap();
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
    assert_eq!(speed_fields(30, 0.1), (300, 1.0 / 300.0, 0.1));
    // 30 / 4 is not whole: the nearest rate, and the speed asked for.
    assert_eq!(speed_fields(30, 4.0), (8, 1.0 / 8.0, 4.0));
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
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 60);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.5);
    // A script's ramp writes the speed: held.
    g.put_f32(TIME + TIME_SPEED, 0.9);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.5);
    set(&mut c, "game_speed", json!(1.0));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 30);
    assert_eq!(g.f32(TIME + TIME_TICK_SECONDS).unwrap(), 1.0 / 30.0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    // Then left alone.
    g.put_f32(TIME + TIME_SPEED, 0.9);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 0.9);
    // Only the offered speeds.
    assert!(
        c.set(&json!({"name": "game_speed", "value": 0.0}), &OK, true)
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
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g), 0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    // Multiplayer (split screen) is playing.
    g.put_u32(GLOBALS + GLOBALS_MODE, 2);
    c.tick(&mut g, &OK).unwrap();
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
    assert!(refusal(Some(5), false).unwrap().starts_with("a guest"));
    assert_eq!(refusal(Some(4), false), None, "a networked game's host");
    assert!(
        refusal(Some(1), true)
            .unwrap()
            .starts_with("a lockstep run")
    );
    let mut g = game();
    assert_eq!(sim_mode(&g), Some(1));
    g.put_u32(SIM + 8, 3);
    assert_eq!(sim_mode(&g), Some(3));
    g.put_u32(guest_layout::points::SIM_GLOBALS.base, 0);
    assert_eq!(sim_mode(&g), None);
    // cheat.set refuses; cheat.list says why.
    let mut c = Cheats::default();
    let why = co_op();
    let e = c
        .set(&json!({"name": "god", "value": true}), &why, true)
        .unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(!c.god);
    let l = c.list(&why, true);
    assert_eq!(l["refused"], Value::Null, "not every cheat is refused");
    assert_eq!(l["cheats"][0]["refused"], json!(policy::CO_OP));
    // On when the session turns co-op: off, the flag and speed undone.
    let mut g = game();
    set(&mut c, "god", json!(true));
    set(&mut c, "game_speed", json!(0.5));
    c.tick(&mut g, &OK).unwrap();
    let said = c.tick(&mut g, &why).unwrap();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(!c.any());
    assert_eq!(flags(&g), 0);
    assert_eq!(g.f32(TIME + TIME_SPEED).unwrap(), 1.0);
    assert_eq!(g.u16(TIME + TIME_RATE).unwrap(), 30);
}

/// A co-op session's refusals.
fn co_op() -> Refusals {
    policy::refusals(&policy::Session {
        sim_mode: Some(3),
        ..policy::Session::default()
    })
}

#[test]
fn the_list_is_what_the_menu_reads() {
    let mut c = Cheats::default();
    let l = c.list(&OK, true);
    let names: Vec<&str> = l["cheats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "god",
            "infinite_ammo",
            "bottomless_clip",
            "game_speed",
            "fly",
            "noclip",
            "hide_hud",
            "ai_off"
        ]
    );
    assert_eq!(l["refused"], Value::Null);
    assert_eq!(l["pending"], Value::Null);
    assert!(
        l["cheats"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["refused"].is_null() && c["pending"].is_null())
    );
    let labels: Vec<&str> = l["cheats"][3]["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["0.1x", "0.25x", "0.5x", "1x", "1.5x", "2x", "4x"]);
    assert_eq!(
        l["cheats"][3]["choices"][3],
        json!({"value": 1.0, "label": "1x"})
    );
    assert_eq!(l["cheats"][3]["choices"][0]["value"], json!(0.1));
    // A set answers with the new list; the choice's value is its own, and
    // a client may send it back as it came.
    let r = c
        .set(&json!({"name": "game_speed", "value": 0.25}), &OK, true)
        .unwrap();
    assert_eq!(
        r["cheats"][3]["value"],
        r["cheats"][3]["choices"][1]["value"]
    );
    let tenth = r["cheats"][3]["choices"][0]["value"].clone();
    let r = c
        .set(&json!({"name": "game_speed", "value": tenth}), &OK, true)
        .unwrap();
    assert_eq!(r["cheats"][3]["value"], json!(0.1));
    let r = c
        .set(&json!({"name": "noclip", "value": true}), &OK, true)
        .unwrap();
    assert_eq!(r["cheats"][5]["value"], json!(true));
    assert!(c.set(&json!({"name": "warp", "value": true}), &OK, true).is_err());
    assert!(c.set(&json!({"name": "god", "value": 1}), &OK, true).is_err());
}

fn on_live(tags: &[&str]) -> Refusals {
    policy::refusals(&policy::Session {
        sim_mode: Some(1),
        lockstep: false,
        live: policy::Live::SignedIn(tags.iter().map(|t| t.to_string()).collect()),
    })
}

#[test]
fn on_live_only_the_allowed_gamertag_cheats() {
    let mut c = Cheats::default();
    // afr on Live: everything.
    for name in ["god", "noclip", "hide_hud"] {
        c.set(&json!({"name": name, "value": true}), &on_live(&["AFR"]), true)
            .unwrap();
    }
    let mut g = game();
    c.tick(&mut g, &on_live(&["afr"])).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
    // Someone else signs in mid-session: every cheat off, and undone.
    let other = on_live(&["Chief"]);
    let said = c.tick(&mut g, &other).unwrap();
    let off: Vec<&String> = said.iter().filter(|l| l.contains("switched off")).collect();
    assert_eq!(off.len(), 3, "{said:?}");
    assert!(
        off.iter()
            .all(|l| l.contains("cheats are off on Xbox Live")),
        "{said:?}"
    );
    // Noclip's own line: handed back to the physics.
    assert!(
        said.iter()
            .any(|l| l.starts_with("cheats: noclip off for player 0"))
    );
    assert!(!c.any());
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 1);
    let e = c
        .set(&json!({"name": "hide_hud", "value": true}), &other, true)
        .unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert_eq!(c.list(&other, true)["refused"], json!(other.all.clone().unwrap()));
    // Off Live: anyone.
    c.set(&json!({"name": "god", "value": true}), &OK, true).unwrap();
}

#[test]
fn a_co_op_session_keeps_the_hud_cheat_and_drops_the_rest() {
    let mut g = game();
    let mut c = Cheats::default();
    set(&mut c, "noclip", json!(true));
    set(&mut c, "hide_hud", json!(true));
    c.tick(&mut g, &OK).unwrap();
    let said = c.tick(&mut g, &co_op()).unwrap();
    assert_eq!(said.len(), 2, "switched off, then handed back: {said:?}");
    assert!(said[0].starts_with("cheats: noclip switched off: a co-op session"));
    assert!(said[1].starts_with("cheats: noclip off for player 0"));
    assert!(c.hide_hud && !c.noclip);
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
}

// ---- AI off ----

const AI: u32 = 0x0069_0000;

/// [`game`] with the AI globals, AI on.
fn with_ai() -> FakeGuest {
    let mut g = game();
    g.map(0x004f_0000, 0x1_0000).map(AI, 0x1000);
    g.put_u32(guest_layout::points::AI_GLOBALS.base, AI)
        .put_u8(AI + AI_ON, 1)
        .put_u8(AI + 1, 1);
    g
}

#[test]
fn ai_off_holds_the_on_byte_and_gives_back_the_games_value() {
    let mut g = with_ai();
    let mut c = Cheats::default();
    set(&mut c, "ai_off", json!(true));
    assert!(c.any());
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
    assert_eq!(g.u8(AI + 1).unwrap(), 1, "initialised is the game's");
    // A script switches AI on (or Firefight does): off again.
    g.put_u8(AI + AI_ON, 1);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
    assert!(snapshot_live(&Mutex::new(std::mem::take(&mut c))).is_some());
    let mut c = Cheats::default();
    set(&mut c, "ai_off", json!(true));
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "ai_off", json!(false));
    c.tick(&mut g, &OK).unwrap();
    // AI was already off (our hold) when this one began: stays off.
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
    // From a fresh game with AI on: back on after, then left alone.
    let mut g = with_ai();
    let mut c = Cheats::default();
    set(&mut c, "ai_off", json!(true));
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "ai_off", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 1);
    assert!(!c.busy());
    g.put_u8(AI + AI_ON, 0);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
}

#[test]
fn ai_off_waits_for_the_globals_and_a_co_op_session_refuses_it() {
    // No AI globals yet (a map loading): nothing written, nothing held.
    let mut g = with_ai();
    g.put_u32(guest_layout::points::AI_GLOBALS.base, 0);
    let mut c = Cheats::default();
    set(&mut c, "ai_off", json!(true));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 1);
    g.put_u32(guest_layout::points::AI_GLOBALS.base, AI);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
    // It changes the simulation: a co-op session switches it off, undone.
    let said = c.tick(&mut g, &co_op()).unwrap();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(!c.ai_off);
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 1);
    assert_eq!(
        c.set(&json!({"name": "ai_off", "value": true}), &co_op(), true)
            .unwrap_err()
            .code,
        code::UNAVAILABLE
    );
}

// ---- at the main menu ----

#[test]
fn cheats_set_at_the_menu_say_they_wait_then_take_hold_in_a_game() {
    let mut g = with_ai();
    g.put_u32(GLOBALS + GLOBALS_MODE, 3);
    assert!(!playing(&g));
    let mut c = Cheats::default();
    let l = c.list(&OK, playing(&g));
    assert_eq!(l["pending"], json!(PENDING), "said before anything is on");
    assert!(l["cheats"][0]["pending"].is_null(), "god is off");
    let r = c
        .set(&json!({"name": "god", "value": true}), &OK, playing(&g))
        .unwrap();
    assert_eq!(r["cheats"][0]["value"], json!(true));
    assert_eq!(r["cheats"][0]["pending"], json!(PENDING));
    assert!(r["cheats"][1]["pending"].is_null(), "ammo is off");
    set(&mut c, "ai_off", json!(true));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g), 0);
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 1, "the menu's AI is left alone");
    // A game starts: the cheats hold, and nothing is pending.
    g.put_u32(GLOBALS + GLOBALS_MODE, 1);
    assert!(playing(&g));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g), CANNOT_TAKE_DAMAGE);
    assert_eq!(g.u8(AI + AI_ON).unwrap(), 0);
    let l = c.list(&OK, playing(&g));
    assert!(l["pending"].is_null() && l["cheats"][0]["pending"].is_null());
    // A refused cheat is refused, not pending.
    let l = c.list(&co_op(), false);
    assert_eq!(l["cheats"][0]["refused"], json!(policy::CO_OP));
    assert!(l["cheats"][0]["pending"].is_null());
    assert_eq!(l["cheats"][6]["pending"], Value::Null, "hide_hud is off");
}

// ---- hide HUD ----

#[test]
fn hide_hud_holds_the_flag_and_gives_back_the_games_value() {
    let mut g = game();
    let mut c = Cheats::default();
    set(&mut c, "hide_hud", json!(true));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
    // A script shows it: hidden again.
    g.put_u8(HUD + HUD_SHOWN, 1);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
    set(&mut c, "hide_hud", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 1);
    // Then left alone.
    g.put_u8(HUD + HUD_SHOWN, 0);
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
    // Hidden by the game already (a cutscene): stays hidden after.
    let mut c = Cheats::default();
    set(&mut c, "hide_hud", json!(true));
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "hide_hud", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(g.u8(HUD + HUD_SHOWN).unwrap(), 0);
    assert!(!c.busy());
}

// ---- fly and noclip ----

use crate::skulls_custom::{BIPED_MODE, BODY_VELOCITY, MODE_AIRBORNE, MODE_GROUND};
use flight::{
    BODY_POSITION, CONTROL_CROUCH, CONTROL_JUMP, CRUISE, Controls, TAKEOFF_LIFT, UNIT_AIMING,
    UNIT_CONTROL_FLAGS, UNIT_THROTTLE, VERTICAL,
};

const DT: f32 = 1.0 / 30.0;

fn put3(g: &mut FakeGuest, at: u32, v: [f32; 3]) {
    for (i, c) in v.iter().enumerate() {
        g.put_f32(at + 4 * i as u32, *c);
    }
}

fn pos(g: &FakeGuest) -> [f32; 3] {
    g.vec3(BODY + BODY_POSITION).unwrap()
}

fn vel(g: &FakeGuest) -> [f32; 3] {
    g.vec3(BODY + BODY_VELOCITY).unwrap()
}

/// The player's hands: the stick, the view, and crouch and jump.
fn hands(g: &mut FakeGuest, stick: [f32; 2], aim: [f32; 3], crouch: bool, jump: bool) {
    let flags = (if crouch { CONTROL_CROUCH } else { 0 }) | (if jump { CONTROL_JUMP } else { 0 });
    g.put_u32(BODY + UNIT_CONTROL_FLAGS, flags);
    put3(g, BODY + UNIT_THROTTLE, [stick[0], stick[1], 0.0]);
    put3(g, BODY + UNIT_AIMING, aim);
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
}

/// A biped standing at (10, 20, 5), looking along +x.
fn standing() -> FakeGuest {
    let mut g = game();
    put3(&mut g, BODY + BODY_POSITION, [10.0, 20.0, 5.0]);
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, false);
    g
}

#[test]
fn flight_flies_where_you_look() {
    let c = |stick: [f32; 2], aim: [f32; 3], crouch: bool, jump: bool| Controls {
        crouch,
        jump,
        throttle: [stick[0], stick[1], 0.0],
        aim,
    };
    // Forward along the view, level.
    let v = flight::velocity(&c([1.0, 0.0], [1.0, 0.0, 0.0], false, false), 1.0);
    assert!(close(v, [CRUISE, 0.0, 0.0]), "{v:?}");
    // Looking up and pushing forward climbs along the view.
    let h = 0.5f32.sqrt();
    let v = flight::velocity(&c([1.0, 0.0], [h, 0.0, h], false, false), 1.0);
    assert!(close(v, [CRUISE * h, 0.0, CRUISE * h]), "{v:?}");
    // Strafing is level and to the left of the heading (+y for +x).
    let v = flight::velocity(&c([0.0, 1.0], [h, 0.0, h], false, false), 1.0);
    assert!(close(v, [0.0, CRUISE, 0.0]), "{v:?}");
    // Jump climbs, crouch sinks, both cancel.
    let up = flight::velocity(&c([0.0, 0.0], [1.0, 0.0, 0.0], false, true), 1.0);
    assert!(close(up, [0.0, 0.0, VERTICAL]));
    let down = flight::velocity(&c([0.0, 0.0], [1.0, 0.0, 0.0], true, false), 1.0);
    assert!(close(down, [0.0, 0.0, -VERTICAL]));
    let none = flight::velocity(&c([0.0, 0.0], [1.0, 0.0, 0.0], true, true), 1.0);
    assert!(close(none, [0.0; 3]));
    // Garbage in: a sane answer (no view: along +x; the stick clamped).
    let v = flight::velocity(&c([f32::NAN, 9.0], [0.0, 0.0, 0.0], false, false), 1.0);
    assert!(close(v, [0.0, CRUISE, 0.0]), "{v:?}");
    // The boost: nothing for half a second, then up to four times.
    assert_eq!(flight::boost(0.0), 1.0);
    assert_eq!(flight::boost(0.5), 1.0);
    assert_eq!(flight::boost(1.0), 2.5);
    assert_eq!(flight::boost(1.5), flight::BOOST);
    assert_eq!(flight::boost(10.0), flight::BOOST);
}

#[test]
fn noclip_holds_its_own_position_through_the_physics() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "noclip", json!(true));
    // On: hovers where it was, the velocity gone, shielded.
    put3(&mut g, BODY + BODY_VELOCITY, [0.0, 0.0, -1.0]);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(pos(&g), [10.0, 20.0, 5.0]));
    assert!(close(vel(&g), [0.0; 3]));
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    // Forward: moves along the view.
    hands(&mut g, [1.0, 0.0], [1.0, 0.0, 0.0], false, false);
    c.tick(&mut g, &OK).unwrap();
    assert!(
        close(pos(&g), [10.0 + CRUISE * DT, 20.0, 5.0]),
        "{:?}",
        pos(&g)
    );
    // A wall: the physics put the body back. Ours wins; on we go.
    put3(&mut g, BODY + BODY_POSITION, [10.0, 20.0, 5.0]);
    put3(&mut g, BODY + BODY_VELOCITY, [-3.0, 0.0, 0.0]);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(pos(&g), [10.0 + 2.0 * CRUISE * DT, 20.0, 5.0]));
    assert!(close(vel(&g), [0.0; 3]));
    // Jump climbs, crouch sinks, without taking off first.
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, true);
    c.tick(&mut g, &OK).unwrap();
    assert!((pos(&g)[2] - (5.0 + VERTICAL * DT)).abs() < 1e-4);
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], true, false);
    c.tick(&mut g, &OK).unwrap();
    c.tick(&mut g, &OK).unwrap();
    assert!((pos(&g)[2] - (5.0 - VERTICAL * DT)).abs() < 1e-4);
}

#[test]
fn noclip_off_drops_you_here_and_rescues_a_stuck_body() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "noclip", json!(true));
    c.tick(&mut g, &OK).unwrap();
    // Fly up into a wall.
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, true);
    for _ in 0..30 {
        c.tick(&mut g, &OK).unwrap();
    }
    let inside = pos(&g);
    assert!((inside[2] - (5.0 + VERTICAL)).abs() < 1e-3, "{inside:?}");
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, false);
    set(&mut c, "noclip", json!(false));
    // Off: the body is left where it is, still shielded.
    let said = c.tick(&mut g, &OK).unwrap();
    assert!(
        said[0].starts_with("cheats: noclip off for player 0"),
        "{said:?}"
    );
    assert!(close(pos(&g), inside));
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    // The physics can't move it (inside the wall): put back on the floor
    // it took off from after half a second.
    let mut rescued = None;
    for i in 0..30 {
        let said = c.tick(&mut g, &OK).unwrap();
        if !said.is_empty() {
            rescued = Some((i, said));
            break;
        }
    }
    let (i, said) = rescued.expect("rescued");
    assert!((13..=16).contains(&i), "after about 0.5 s, not tick {i}");
    assert!(
        said[0].contains("put back at (10.00, 20.00, 5.00)"),
        "{said:?}"
    );
    assert!(close(pos(&g), [10.0, 20.0, 5.0]));
    assert!(close(vel(&g), [0.0; 3]));
    // Once only, and the shield goes when the watch ends.
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    for _ in 0..100 {
        assert!(c.tick(&mut g, &OK).unwrap().is_empty());
    }
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, 0);
    assert!(!c.busy());
}

#[test]
fn noclip_off_in_the_open_falls_and_a_long_fall_is_caught() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "noclip", json!(true));
    c.tick(&mut g, &OK).unwrap();
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    set(&mut c, "noclip", json!(false));
    c.tick(&mut g, &OK).unwrap();
    // Falling: moving, so not stuck; nothing done for a short drop.
    let mut z = 5.0;
    for _ in 0..10 {
        z -= 0.5;
        put3(&mut g, BODY + BODY_POSITION, [10.0, 20.0, z]);
        assert!(c.tick(&mut g, &OK).unwrap().is_empty());
    }
    // Out of the world: caught 20 units down.
    let mut caught = false;
    for _ in 0..40 {
        z -= 1.0;
        put3(&mut g, BODY + BODY_POSITION, [10.0, 20.0, z]);
        if !c.tick(&mut g, &OK).unwrap().is_empty() {
            caught = true;
            break;
        }
    }
    assert!(caught);
    assert!(z < 5.0 - flight::FALL_RESCUE && z > 5.0 - flight::FALL_RESCUE - 1.5);
    assert!(close(pos(&g), [10.0, 20.0, 5.0]));
}

#[test]
fn fly_takes_off_on_crouch_flies_and_lands_with_crouch() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "fly", json!(true));
    // Standing with nothing pressed: nothing happens.
    c.tick(&mut g, &OK).unwrap();
    assert!(close(pos(&g), [10.0, 20.0, 5.0]));
    assert!(close(vel(&g), [0.0; 3]));
    // A crouch press takes off: a lift off the floor and a climb.
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], true, false);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(pos(&g), [10.0, 20.0, 5.0 + TAKEOFF_LIFT]));
    assert!(close(vel(&g), [0.0, 0.0, VERTICAL]));
    // In the air: the velocity is the controls'. The physics moves the
    // body (collision is the game's); fly never writes the position.
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    hands(&mut g, [1.0, 0.0], [1.0, 0.0, 0.0], false, false);
    put3(&mut g, BODY + BODY_POSITION, [11.0, 20.0, 7.0]);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(vel(&g), [CRUISE, 0.0, 0.0]));
    assert!(close(pos(&g), [11.0, 20.0, 7.0]));
    // Nothing pressed: a hover (gravity's pull cancelled each tick).
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, false);
    put3(&mut g, BODY + BODY_VELOCITY, [0.0, 0.0, -0.5]);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(vel(&g), [0.0; 3]));
    // Not shielded by flying itself.
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, 0);
    // Crouch sinks to the floor; touching it with crouch held lands.
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], true, false);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(vel(&g), [0.0, 0.0, -VERTICAL]));
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    put3(&mut g, BODY + BODY_VELOCITY, [0.0; 3]);
    c.tick(&mut g, &OK).unwrap();
    // Landed: walking is the game's again, and crouch still held does not
    // take off.
    hands(&mut g, [1.0, 0.0], [1.0, 0.0, 0.0], true, false);
    put3(&mut g, BODY + BODY_VELOCITY, [1.0, 0.0, 0.0]);
    for _ in 0..3 {
        c.tick(&mut g, &OK).unwrap();
    }
    assert!(close(vel(&g), [1.0, 0.0, 0.0]));
    // A fresh press does.
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, false);
    c.tick(&mut g, &OK).unwrap();
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], true, false);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(vel(&g), [0.0, 0.0, VERTICAL]));
}

#[test]
fn fly_off_in_the_air_drops_you_shielded_for_the_fall() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "fly", json!(true));
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], true, false);
    c.tick(&mut g, &OK).unwrap();
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    hands(&mut g, [0.0, 0.0], [1.0, 0.0, 0.0], false, false);
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "fly", json!(false));
    put3(&mut g, BODY + BODY_VELOCITY, [0.0, 0.0, -2.0]);
    c.tick(&mut g, &OK).unwrap();
    // The game's fall, untouched, shielded.
    assert!(close(vel(&g), [0.0, 0.0, -2.0]));
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    // Hanging still in the air is not "stuck" for fly: no rescue.
    for _ in 0..100 {
        assert!(c.tick(&mut g, &OK).unwrap().is_empty());
    }
    assert!(close(pos(&g), [10.0, 20.0, 5.0 + TAKEOFF_LIFT]));
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, 0);
    assert!(!c.busy());
}

#[test]
fn noclip_off_with_fly_on_keeps_you_flying() {
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "fly", json!(true));
    set(&mut c, "noclip", json!(true));
    c.tick(&mut g, &OK).unwrap();
    g.put_u8(BODY + BIPED_MODE, MODE_AIRBORNE);
    set(&mut c, "noclip", json!(false));
    hands(&mut g, [1.0, 0.0], [1.0, 0.0, 0.0], false, false);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(vel(&g), [CRUISE, 0.0, 0.0]));
}

#[test]
fn flight_leaves_a_unit_in_a_vehicle_alone() {
    let mut g = standing();
    g.put_u32(BODY + BODY_PARENT, 0xe002_0000);
    let mut c = Cheats::default();
    set(&mut c, "noclip", json!(true));
    hands(&mut g, [1.0, 0.0], [1.0, 0.0, 0.0], false, true);
    put3(&mut g, BODY + BODY_VELOCITY, [3.0, 0.0, 0.0]);
    c.tick(&mut g, &OK).unwrap();
    assert!(close(pos(&g), [10.0, 20.0, 5.0]));
    assert!(close(vel(&g), [3.0, 0.0, 0.0]));
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, 0);
}

#[test]
fn the_noclip_shield_and_god_share_the_flag() {
    // God off while noclip is on: the flag stays until both are done.
    let mut g = standing();
    let mut c = Cheats::default();
    set(&mut c, "god", json!(true));
    set(&mut c, "noclip", json!(true));
    c.tick(&mut g, &OK).unwrap();
    set(&mut c, "god", json!(false));
    c.tick(&mut g, &OK).unwrap();
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, CANNOT_TAKE_DAMAGE);
    set(&mut c, "noclip", json!(false));
    g.put_u8(BODY + BIPED_MODE, MODE_GROUND);
    for _ in 0..120 {
        c.tick(&mut g, &OK).unwrap();
    }
    assert_eq!(flags(&g) & CANNOT_TAKE_DAMAGE, 0);
}

#[test]
fn snapshots_wait_while_a_cheat_is_on_or_left_to_undo() {
    let c = Mutex::new(Cheats::default());
    assert_eq!(snapshot_live(&c), None);
    lock(&c).god = true;
    assert_eq!(
        snapshot_live(&c).as_deref(),
        Some("a cheat is on or has something to undo")
    );
    // Off, but the unit we flagged is still ours to clear.
    let mut g = standing();
    set(&mut lock(&c), "god", json!(true));
    lock(&c).tick(&mut g, &OK).unwrap();
    set(&mut lock(&c), "god", json!(false));
    assert!(snapshot_live(&c).is_some());
    lock(&c).tick(&mut g, &OK).unwrap();
    assert_eq!(snapshot_live(&c), None);
}
