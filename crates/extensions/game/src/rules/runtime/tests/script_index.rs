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

#[test]
fn each_ready_round_posts_the_record_scripts_line_on_the_host() {
    // CTF's category and line on an index-6 record running Slayer's handlers.
    let source = CARRIED.replace("type slayer", "type ctf");
    let store = Local::default();
    let (s, mut m, _, line, _) = carried(&store, &source, SCRIPT);
    assert!(s.lock().state().running, "{line}");

    // The roster exists while its machines are still joining.
    let work = s.lock().pre_tick(&m).unwrap();
    assert!(work.calls.is_empty());
    drive(&s, m.clone(), work, true);
    let players = engine::view::players::array(&m).unwrap().unwrap();
    for slot in 0..2 {
        players.get_absolute(&m, slot).unwrap().unwrap().set_flags(&mut m, 1).unwrap();
    }

    const SP: u32 = 0x3400_0100;
    m.map(SP - 0x100, 0x100);
    for round in 0..2 {
        if round > 0 {
            s.lock().round_start(&m);
        }
        m.put_u32(GAME_TIME + 8, TICK + 1 + 2 * round);
        let work = s.lock().pre_tick(&m).expect("the ready round's line");
        assert_eq!(work.calls, [Call::StartLine(Type::Ctf)]);
        let shared = s.clone();
        let entry = move |cx: Cx| -> EntryFuture {
            Box::pin(run_work(cx, Arc::clone(&shared), work.clone()))
        };
        let run = testing::run(&entry, m, Regs { esp: SP, ..Regs::default() }, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        let event = SP - 0x24;
        assert_eq!(run.calls, [
            GuestCall::new(points::GAME_EVENT_NEW.base)
                .eax(event).ecx(3).edx(0).reserve(0x24),
            GuestCall::new(points::GAME_EVENT_POST.base).eax(event).reserve(0x24),
            GuestCall::new(points::GAME_TICK.base),
        ]);
        m = run.guest;
        m.put_u32(GAME_TIME + 8, TICK + 2 + 2 * round);
        assert!(s.lock().pre_tick(&m).is_none(), "the round already spoke");
    }

    let (client, mut m, _, _, _) = carried(&Local::default(), &source, SCRIPT);
    let players = engine::view::players::array(&m).unwrap().unwrap();
    for slot in 0..2 {
        players.get_absolute(&m, slot).unwrap().unwrap().set_flags(&mut m, 1).unwrap();
    }
    let work = client.lock().pre_tick(&m).unwrap();
    assert_eq!(drive(&client, m, work, false), [
        GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
        GuestCall::new(points::GAME_TICK.base),
    ]);
}

/// A record whose options sit at [`OPTIONS`] in a multiplayer game, as the
/// start hands them to `GAME_ACTIVATE`.
const OPTIONS: u32 = 0x1007_0000;

fn options_with(record: [u8; variants::record::RECORD_SIZE]) -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(OPTIONS, 0x300)
        .put_u32(OPTIONS, MODE_MULTIPLAYER)
        .put(OPTIONS + crate::variant_ext::OPTIONS_VARIANT, &record);
    m
}

fn record_at(m: &FakeGuest) -> [u8; variants::record::RECORD_SIZE] {
    let mut raw = [0; variants::record::RECORD_SIZE];
    m.read(OPTIONS + crate::variant_ext::OPTIONS_VARIANT, &mut raw).unwrap();
    raw
}

fn on_script_index(m: &mut FakeGuest) -> [u8; LEN] {
    let jugg = crate::rules::record::no_mode_juggernaut();
    assert_eq!(crate::rules::record::move_onto_script_index(m, OPTIONS, jugg), Ok(true));
    let raw = record_at(m);
    let v = variants::GameVariant::from_bytes(&raw).unwrap();
    assert_eq!(v.game_type(), variants::GameType::Script);
    assert!(variants::record::check(&v).is_empty());
    raw[variants::ext::AT..variants::ext::AT + LEN].try_into().unwrap()
}

#[test]
fn a_saved_slayer_record_starts_on_the_script_index_and_scores_once() {
    let mut raw = variants::GameVariant::stock(variants::GameType::Slayer).unwrap().to_bytes();
    let block = block(prefix(&slayer().id), 2);
    raw[variants::ext::AT..variants::ext::AT + LEN].copy_from_slice(&block);
    let mut options = options_with(raw);
    let moved = on_script_index(&mut options);
    assert_eq!(moved, block, "the script reference stays");

    // The game starts the script index's engine: the kill is the script's
    // one score, and no carrier's kill routine is called.
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, datums) = script_game(moved);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    s.lock().kill(&m, 0, Some(1));
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    let scores: Vec<_> = calls.iter().filter(|c| c.target == points::SCORE_ADD.base).collect();
    assert_eq!(scores, [&GuestCall::new(points::SCORE_ADD.base).eax(3).args([datums[1]])]);
    assert!(calls.iter().all(|c| ![points::SLAYER_KILL.base, points::JUGGERNAUT_KILL.base].contains(&c.target)));
}

#[test]
fn a_no_mode_juggernaut_record_starts_its_built_in_on_the_script_index_and_scores_at_most_once() {
    let raw = variants::GameVariant::stock(variants::GameType::Juggernaut).unwrap().to_bytes();
    let mut options = options_with(raw);
    let moved = on_script_index(&mut options);
    let ported = builtins().iter().find(|b| b.engine == 7).unwrap();
    let script = variants::ext::Ext::in_effect(&moved).script;
    assert_eq!((script.mode, script.prefix), (variants::ext::Mode::Rules, prefix(&ported.id)));

    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = script_game(moved);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.script.as_deref(), Some("Juggernaut"));
    s.lock().kill(&m, 0, Some(1));
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    // The kill scores at most the script's one point; no retail kill,
    // tick or role routine runs beside it.
    let scores = calls.iter().filter(|c| c.target == points::SCORE_ADD.base).count();
    assert!(scores <= 1, "{calls:?}");
    for target in [
        points::SLAYER_KILL.base,
        points::JUGGERNAUT_KILL.base,
        points::JUGGERNAUT_TICK.base,
        points::JUGGERNAUT_ROLE_SET.base,
    ] {
        assert!(calls.iter().all(|c| c.target != target), "{target:#x}: {calls:?}");
    }
}

#[test]
fn only_a_multiplayer_games_options_move_and_a_retail_record_stays() {
    use crate::rules::record::{move_onto_script_index, no_mode_juggernaut};
    let slayer_raw = variants::GameVariant::stock(variants::GameType::Slayer).unwrap().to_bytes();
    let mut options = options_with(slayer_raw);
    assert_eq!(move_onto_script_index(&mut options, OPTIONS, None), Ok(false));
    assert_eq!(record_at(&options), slayer_raw, "no script: retail Slayer");
    let mut jugg = options_with(variants::GameVariant::stock(variants::GameType::Juggernaut).unwrap().to_bytes());
    let was = record_at(&jugg);
    jugg.put_u32(OPTIONS, 1);
    assert_eq!(move_onto_script_index(&mut jugg, OPTIONS, no_mode_juggernaut()), Ok(false));
    assert_eq!(record_at(&jugg), was, "a campaign start");
}
