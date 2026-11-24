//! GE4's acceptance on the fake game
//! (docs/game/game-types-without-carriers.md, "Slices"): a script game on
//! the script index starts with its engine object in the engine table, and
//! a kill reaches `on kill` and scores through `SCORE_ADD` with no engine
//! slot called.

use super::*;
use crate::rules::engine_object::{self, Installed, OVERRIDES, SLOTS};

const OBJECT: u32 = 0x1006_0000;
const SCRIPT: i32 = ::rules::SCRIPT_INDEX as i32;

/// Invented bodies for Slayer's vtable: `0x0070_0000 + slot offset`.
fn slayer_body(offset: u32) -> u32 {
    0x0070_0000 + offset
}

/// The game of [`guest`] on the script index, its engine index at
/// `E+0xc14`, with Slayer's vtable in the image and the engine object put
/// in as the game start does.
fn script_game(block: [u8; LEN]) -> (FakeGuest, [u32; 2]) {
    let (mut m, datums) = guest(block, SCRIPT);
    m.put_u32(ENGINE + engine_object::ENGINE_INDEX, SCRIPT as u32);
    m.map(points::SLAYER_VTABLE.base, 4 * SLOTS)
        .map(points::ENGINE_TABLE.base, 4 * 11)
        .map(OBJECT, engine_object::OBJECT_LEN);
    for i in 0..SLOTS {
        m.put_u32(points::SLAYER_VTABLE.base + 4 * i, slayer_body(4 * i));
    }
    assert_eq!(
        engine_object::install(&mut m, OBJECT),
        Ok(Installed::Written)
    );
    (m, datums)
}

#[test]
fn a_script_game_starts_on_the_script_index_with_its_object() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = script_game(block(prefix(&slayer().id), 2));
    let line = s.lock().start(&m, false);
    assert!(line.contains("Slayer"), "{line}");
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.refused, None);
    // The engine table's script index entry is the object; its kill slot
    // is the shared empty body, never Slayer's scoring.
    let object = m.u32(engine_object::table_slot()).unwrap();
    assert_eq!(object, OBJECT);
    let vtable = m.u32(object).unwrap();
    let kill = m.u32(vtable + 0x78).unwrap();
    assert_eq!(kill, 0x002e_d7f0);
    assert_ne!(kill, points::SLAYER_KILL.base);
    for (offset, body) in OVERRIDES {
        assert_eq!(m.u32(vtable + offset), Ok(body), "+{offset:#x}");
    }
    assert_eq!(
        m.u32(vtable + 0x9c),
        Ok(slayer_body(0x9c)),
        "Slayer's entity type"
    );
}

#[test]
fn a_kill_reaches_on_kill_and_scores_with_no_engine_slot_called() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, datums) = script_game(block(prefix(&slayer().id), 2));
    s.lock().start(&m, false);
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [Call::Score {
            player: 1,
            delta: 3
        }]
    );
    let work = s
        .lock()
        .pre_tick(&m)
        .expect("the score and the authority check");
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::SCORE_ADD.base)
                .eax(3)
                .args([datums[1]]),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    // No call is to a vtable body or to a carrier's kill routine.
    for c in &calls {
        assert!(
            !(0x0070_0000..0x0070_0000 + 4 * SLOTS).contains(&c.target),
            "{c:?}"
        );
        assert!(
            ![points::SLAYER_KILL.base, points::JUGGERNAUT_KILL.base].contains(&c.target),
            "{c:?}"
        );
    }
    let state = s.lock().state();
    assert!(state.errors.is_empty(), "{:?}", state.errors);
}

#[test]
fn a_record_of_the_script_index_with_no_script_runs_nothing() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = script_game([0; LEN]);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(!state.running);
    assert_eq!(state.refused.as_deref(), Some("the record names no script"));
}
