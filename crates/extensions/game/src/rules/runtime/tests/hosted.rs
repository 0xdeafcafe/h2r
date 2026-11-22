//! What a script game changes in the carrier engine on the host: its own
//! scoring stands down while a script runs on the authority
//! (`rules::scoring`), `role_traits` sets Juggernaut's role through the
//! engine (`rules::roles`), and the objective set, its carriers and
//! `reset_objective` read and drive the engine's registry
//! (`rules::objectives`).

use super::*;
use crate::rules::{objectives, roles, scoring};

const JUGGERNAUT: i32 = 7;
const ODDBALL: i32 = 3;
const JUGGERNAUT_GLOBALS: u32 = 0x1005_0000;
const ODDBALL_GLOBALS: u32 = 0x1005_1000;
/// An objective object's salted datum: the salt is the high 16 bits and
/// has bit 15 set, so the datum is at or above 2^31 like every real one.
const BALL: u32 = 0xE1A3_0012;

/// A runtime whose fetch answers `src`'s blob, and the blob's hash.
fn hosting(store: &Local, src: &str) -> (Arc<Shared>, Sha256) {
    let ir = ::rules::compile(src).unwrap();
    let blob = ir.encode();
    let id = ir.id();
    let s = shared(store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    (s, id)
}

/// The game of [`guest`] with the Juggernaut globals and its role mask.
fn juggernaut_guest(block: [u8; LEN], mask: u16) -> (FakeGuest, [u32; 2]) {
    let (mut m, datums) = guest(block, JUGGERNAUT);
    m.put_u32(points::JUGGERNAUT_GLOBALS.base, JUGGERNAUT_GLOBALS);
    m.map(JUGGERNAUT_GLOBALS, 0x10);
    m.put_u16(JUGGERNAUT_GLOBALS + roles::ROLE_MASK, mask);
    (m, datums)
}

/// An Oddball game at game tick `tick` with one ball, [`BALL`], registered
/// as ball 0 and carried by `carrier` (a salted player datum), its spawn
/// flag 4.
fn oddball_guest(block: [u8; LEN], tick: u32, carrier: Option<u32>) -> (FakeGuest, [u32; 2]) {
    let (mut m, datums) = guest(block, ODDBALL);
    m.put_u32(GAME_TIME + 8, tick);
    m.put_u16(GAME_GLOBALS + objectives::BALL_COUNT, 1);
    m.put_u32(points::ODDBALL_GLOBALS.base, ODDBALL_GLOBALS);
    m.map(ODDBALL_GLOBALS, 0x40);
    m.put_u32(ODDBALL_GLOBALS + objectives::ODDBALL_SPAWNS, 4);
    let record = ENGINE + engine::objectives::OBJECTIVES;
    m.put_u32(ENGINE + engine::objectives::OBJECTIVE_COUNT, 1);
    m.put_u32(record + engine::objectives::RECORD_OBJECT, BALL);
    m.put_u16(record + engine::objectives::RECORD_TEAM, 0);
    m.put_u32(record + engine::objectives::RECORD_UNIT, u32::MAX);
    m.put_u32(record + engine::objectives::RECORD_PLAYER, carrier.unwrap_or(u32::MAX));
    (m, datums)
}

#[test]
fn the_carrier_scores_until_the_engine_names_this_machine_its_authority() {
    let site = scoring::CARRIER_SCORE_SITES[0].0 + scoring::CALL_LEN;
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    assert!(!scoring::script_scores(&s.lock()), "no game");
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    assert!(
        !scoring::script_scores(&s.lock()),
        "the engine keeps its rules until the first boundary asks for the authority"
    );
    assert!(!scoring::drops_score(&s.lock(), site));
    let work = s.lock().pre_tick(&m).unwrap();
    drive(&s, m, work, true);
    assert!(scoring::script_scores(&s.lock()));
    assert!(scoring::drops_score(&s.lock(), site));
    // The script's own calls come from no carrier site.
    assert!(!scoring::drops_score(&s.lock(), 0x0012_3456));
    s.lock().ended();
    assert!(!scoring::script_scores(&s.lock()), "the game ended");
}

#[test]
fn a_guest_and_a_refused_script_leave_the_carriers_scoring_on() {
    let site = scoring::CARRIER_SCORE_SITES[3].0 + scoring::CALL_LEN;
    // Another machine is the authority: the script is dropped here.
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    let work = s.lock().pre_tick(&m).unwrap();
    drive(&s, m, work, false);
    assert!(!scoring::script_scores(&s.lock()));
    assert!(!scoring::drops_score(&s.lock(), site));
    // A refused script plays the carrier's own rules.
    let (m, _) = guest(block([9; 8], 0), SLAYER);
    s.lock().start(&m, false);
    assert!(s.lock().state().refused.is_some());
    assert!(!scoring::script_scores(&s.lock()));
}

const CROWN: &str = "rules \"Crown\" 1\ncarrier juggernaut\n\non kill(victim, killer) {\n    if killer != none and killer != victim { role_traits(killer, true) }\n    role_traits(victim, false)\n}\n";

#[test]
fn role_traits_sets_the_juggernaut_role_through_the_engine_at_the_boundary() {
    let store = Local::default();
    let (s, id) = hosting(&store, CROWN);
    // Player 0 is the Juggernaut.
    let (m, datums) = juggernaut_guest(block(prefix(&id), 0), 0b01);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [
            Call::RoleTraits {
                player: 1,
                on: true
            },
            Call::RoleTraits {
                player: 0,
                on: false
            },
        ]
    );
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                .eax(datums[1])
                .ecx(1),
            GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                .eax(datums[0])
                .ecx(0),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    assert!(s.lock().state().errors.is_empty());
}

#[test]
fn a_player_already_in_the_asked_role_is_left_alone() {
    let store = Local::default();
    let (s, id) = hosting(&store, CROWN);
    // Player 1 already holds the role and player 0 does not.
    let (m, _) = juggernaut_guest(block(prefix(&id), 0), 0b10);
    s.lock().start(&m, false);
    s.lock().kill(&m, 0, Some(1));
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    assert!(s.lock().state().errors.is_empty());
}

#[test]
fn role_traits_without_the_juggernaut_globals_is_an_engine_refusal() {
    let (mut m, _) = guest([0; LEN], JUGGERNAUT);
    m.put_u32(points::JUGGERNAUT_GLOBALS.base, 0);
    let why = roles::plan(&m, 1, true).unwrap_err().to_string();
    assert!(why.contains("Juggernaut globals are not set up"), "{why}");
    let (m, _) = juggernaut_guest([0; LEN], 0);
    assert_eq!(roles::plan(&m, 1, false).unwrap(), roles::Plan::Already);
    let why = roles::plan(&m, 5, true).unwrap_err().to_string();
    assert!(why.contains("no player in slot 5"), "{why}");
}

#[test]
fn role_traits_on_another_carrier_is_refused_by_the_carrier() {
    let src = CROWN.replace("carrier juggernaut", "carrier slayer");
    let store = Local::default();
    let (s, id) = hosting(&store, &src);
    let (m, _) = guest(block(prefix(&id), 0), SLAYER);
    s.lock().start(&m, false);
    assert_eq!(
        s.lock().state().refused.as_deref(),
        Some("the script calls role_traits, which a host makes only on juggernaut, and the script rides slayer")
    );
}

const RETURN: &str = "rules \"Return\" 1\ncarrier oddball\n\non tick {\n    for each objectives b {\n        if b.carrier == none { reset_objective(b) }\n    }\n}\n";

#[test]
fn reset_objective_returns_an_oddball_through_the_engines_ball_return() {
    let store = Local::default();
    let (s, id) = hosting(&store, RETURN);
    let (m, _) = oddball_guest(block(prefix(&id), 0), TICK, None);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let work = s.lock().pre_tick(&m).unwrap();
    drive(&s, m, work, true);
    // The tick finds the ball on the ground and queues its return.
    let (m, _) = oddball_guest(block(prefix(&id), 0), TICK + 1, None);
    s.lock().post_tick(&m);
    let work = s.lock().pre_tick(&m).expect("the queued return");
    assert_eq!(work.calls, [Call::ResetObjective(BALL)]);
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::ODDBALL_BALL_RETURN.base).eax(0),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    assert!(s.lock().state().errors.is_empty(), "{:?}", s.lock().state().errors);
}

#[test]
fn a_carried_salted_objective_is_visited_and_its_carrier_read() {
    let store = Local::default();
    let (s, id) = hosting(&store, RETURN);
    let (mut m, datums) = oddball_guest(block(prefix(&id), 0), TICK, None);
    m.put_u32(
        ENGINE + engine::objectives::OBJECTIVES + engine::objectives::RECORD_PLAYER,
        datums[0],
    );
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    let work = s.lock().pre_tick(&m).unwrap();
    drive(&s, m, work, true);
    let (mut m, datums) = oddball_guest(block(prefix(&id), 0), TICK + 1, None);
    m.put_u32(
        ENGINE + engine::objectives::OBJECTIVES + engine::objectives::RECORD_PLAYER,
        datums[0],
    );
    s.lock().post_tick(&m);
    // The carried ball was visited, its carrier read as a player, and no
    // return was queued.
    let queued = s.lock().pre_tick(&m).map(|w| w.calls);
    assert!(queued.as_ref().is_none_or(|c| c.is_empty()), "a carried ball is not returned: {queued:?}");
    assert!(s.lock().state().errors.is_empty(), "{:?}", s.lock().state().errors);
}

#[test]
fn reset_objective_names_why_a_ball_cannot_return() {
    let (m, _) = oddball_guest([0; LEN], TICK, None);
    assert_eq!(objectives::ball_of(&m, BALL).unwrap(), 0);
    let why = objectives::ball_of(&m, 0x0003_0006).unwrap_err().to_string();
    assert!(why.contains("not a registered objective"), "{why}");
    let mut m = m;
    m.put_u32(ODDBALL_GLOBALS + objectives::ODDBALL_SPAWNS, u32::MAX);
    let why = objectives::ball_of(&m, BALL).unwrap_err().to_string();
    assert!(why.contains("ball 0 has no spawn flag"), "{why}");
    m.put_u16(GAME_GLOBALS + objectives::BALL_COUNT, 0);
    let why = objectives::ball_of(&m, BALL).unwrap_err().to_string();
    assert!(why.contains("the variant has 0 balls"), "{why}");
    let (m, _) = guest([0; LEN], SLAYER);
    let why = objectives::ball_of(&m, BALL).unwrap_err().to_string();
    assert!(why.contains("only on Oddball"), "{why}");
}

#[test]
fn the_game_lists_the_objectives_and_who_carries_them() {
    let (m, _) = guest([0; LEN], ODDBALL);
    let (carried, datums) = oddball_guest([0; LEN], TICK, None);
    let mut carried = carried;
    carried.put_u32(
        ENGINE + engine::objectives::OBJECTIVES + engine::objectives::RECORD_PLAYER,
        datums[1],
    );
    let mut calls = Vec::new();
    let carrier = |g: &mut GuestGame<'_>, o: Option<u32>| {
        g.read(&VmCall {
            entry: caps::entry_named("carrier_of").unwrap(),
            args: vec![Arg::Object(o)],
        })
    };
    let mut g = GuestGame::new(&m, &mut calls);
    assert!(g.objectives().is_empty(), "no objective registered");
    assert_eq!(carrier(&mut g, Some(BALL)), ::rules::ty::NONE);
    let mut g = GuestGame::new(&carried, &mut calls);
    assert_eq!(g.objectives(), [BALL]);
    assert_eq!(carrier(&mut g, Some(BALL)), 1);
    assert_eq!(carrier(&mut g, Some(0x0003_0006)), ::rules::ty::NONE);
    assert_eq!(carrier(&mut g, None), ::rules::ty::NONE);
}
