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
    let players = guest_layout::view::players::array(&m).unwrap().unwrap();
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
            Box::pin(run_work(cx, Arc::clone(&shared), work.clone(), 0))
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
    let players = guest_layout::view::players::array(&m).unwrap().unwrap();
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

use crate::rules::record::Swap;

fn on_script_index(m: &mut FakeGuest) -> [u8; LEN] {
    assert_eq!(crate::rules::record::move_onto_script_index(m, OPTIONS), Ok(Swap::Moved));
    let raw = record_at(m);
    let v = variants::GameVariant::from_bytes(&raw).unwrap();
    assert_eq!(v.game_type(), variants::GameType::Script);
    assert!(variants::record::check(&v).is_empty());
    raw[variants::ext::AT..variants::ext::AT + LEN].try_into().unwrap()
}

#[test]
fn pressing_forge_launches_an_unscored_editor_from_custom_games_setup() {
    use crate::{custom_games::{Menu, Row}, lobby_tabs::{Tab, switch}};
    let menu = Menu::ALL;
    let press = menu.row_kind(menu.row_of(Row::Forge).unwrap());
    assert_eq!(press.retail_press(), Row::CustomGames.retail_press());
    switch::set_ours(&switch::plan(Tab::CustomGames, press.mode().unwrap()), None).unwrap();
    let stock = variants::GameVariant::stock(variants::GameType::Slayer).unwrap();
    let mut options = options_with(stock.to_bytes());
    assert_eq!(crate::rules::record::move_onto_script_index(&mut options, OPTIONS), Ok(Swap::Moved));
    let editor = variants::GameVariant::from_bytes(&record_at(&options)).unwrap();
    assert_eq!(editor.game_type(), variants::GameType::Script);
    assert_eq!(editor.name(), "Forge");
    assert_eq!(editor.get(variants::SettingId(1)), Some(0));
    assert_eq!(editor.get(variants::SettingId(10)), Some(0));
    assert!(crate::lobby_start::forge_session());

    // A subsequent Custom Games press starts the very same lobby record as Slayer.
    switch::set_ours(&switch::plan(Tab::Forge, Row::CustomGames.mode().unwrap()), None).unwrap();
    let mut options = options_with(stock.to_bytes());
    assert_eq!(crate::rules::record::move_onto_script_index(&mut options, OPTIONS), Ok(Swap::Stayed));
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
fn only_a_multiplayer_games_options_move_and_a_retail_slayer_record_stays() {
    use crate::rules::record::move_onto_script_index;
    let slayer_raw = variants::GameVariant::stock(variants::GameType::Slayer).unwrap().to_bytes();
    let mut options = options_with(slayer_raw);
    assert_eq!(move_onto_script_index(&mut options, OPTIONS), Ok(Swap::Stayed));
    assert_eq!(record_at(&options), slayer_raw, "no script: retail Slayer");
    let mut jugg = options_with(variants::GameVariant::stock(variants::GameType::Juggernaut).unwrap().to_bytes());
    let was = record_at(&jugg);
    jugg.put_u32(OPTIONS, 1);
    assert_eq!(move_onto_script_index(&mut jugg, OPTIONS), Ok(Swap::Stayed));
    assert_eq!(record_at(&jugg), was, "a campaign start");
}

fn retail(ty: variants::GameType, rows: &[(&str, i32)]) -> variants::GameVariant {
    let mut v = variants::GameVariant::stock(ty).unwrap();
    for &(name, value) in rows {
        let (_, row) = ::rules::settings::named(name).unwrap();
        v.set(variants::SettingId(row.setting(ty.engine() as u8).unwrap()), value).unwrap();
    }
    v
}

#[test]
fn every_no_script_retail_record_starts_as_its_built_in_with_the_mapped_options() {
    use variants::GameType::{Assault, Ctf, Juggernaut, King, Oddball, Territories};
    let cases = [
        (Ctf, vec![("ctf_reset_time", 45)], "reset_time", 45),
        (Assault, vec![("assault_arm_time", 7)], "arm_time", 7),
        (Oddball, vec![("oddball_ball_count", 3)], "ball_count", 3),
        (King, vec![("king_moving_hill", 30)], "move_time", 30),
        (Territories, vec![("territories_contest_time", 9)], "contest_time", 9),
        (Juggernaut, vec![], "", 0),
    ];
    for (ty, rows, option, want) in cases {
        let was = retail(ty, &rows);
        let mut options = options_with(was.to_bytes());
        let moved = on_script_index(&mut options);
        let script = variants::ext::Ext::in_effect(&moved).script;
        let src = ::rules::builtin::builtin(ty.engine() as u8).unwrap();
        let ir = ::rules::compile(src).unwrap();
        assert_eq!(script.mode, variants::ext::Mode::Rules, "{ty:?}");
        assert_eq!(script.prefix[..], ir.id().0[..8], "{ty:?}");
        if !option.is_empty() {
            assert_eq!(ir.option_value(script.options, option), Some(want), "{ty:?}");
        }
        // The common settings stay.
        assert_eq!(&record_at(&options)[0x4c..0xe0], &was.to_bytes()[0x4c..0xe0], "{ty:?}");
    }
}

#[test]
fn a_record_naming_its_own_built_in_keeps_its_type_area_options() {
    let mut named = retail(variants::GameType::Ctf, &[("ctf_reset_time", 45)]);
    assert!(variants::portable::attach_builtin(&mut named).unwrap());
    let mut options = options_with(named.to_bytes());
    let moved = on_script_index(&mut options);
    let ir = ::rules::compile(::rules::builtin::builtin(1).unwrap()).unwrap();
    let script = variants::ext::Ext::in_effect(&moved).script;
    assert_eq!(ir.option_value(script.options, "reset_time"), Some(45));
}

#[test]
fn a_script_record_on_a_retail_index_other_than_slayers_moves_and_scores_once() {
    for ty in [
        variants::GameType::Ctf,
        variants::GameType::Oddball,
        variants::GameType::King,
        variants::GameType::Territories,
        variants::GameType::Assault,
        variants::GameType::Juggernaut,
    ] {
        let mut raw = variants::GameVariant::stock(ty).unwrap().to_bytes();
        let block = block(prefix(&slayer().id), 2);
        raw[variants::ext::AT..variants::ext::AT + LEN].copy_from_slice(&block);
        let mut options = options_with(raw);
        assert_eq!(on_script_index(&mut options), block, "{ty:?}: the script reference stays");
        let v = variants::GameVariant::from_bytes(&record_at(&options)).unwrap();
        assert!(v.bytes()[variants::record::TYPE_AREA].iter().all(|&b| b == 0), "{ty:?}");
    }
}

#[test]
fn a_retail_record_that_cannot_map_stays_and_the_reason_is_named() {
    let raw = retail(variants::GameType::Ctf, &[("ctf_reset_time", 301)]).to_bytes();
    let mut options = options_with(raw);
    let swap = crate::rules::record::move_onto_script_index(&mut options, OPTIONS);
    assert!(matches!(swap, Ok(Swap::Refused(_))), "{swap:?}");
    assert_eq!(record_at(&options), raw);
}

#[test]
fn the_listed_siege_record_is_on_slayers_index_in_the_lobby_and_moves_at_the_start() {
    // Siege is a team script: its record stays on Slayer's index, which the
    // retail pregame lobby builds its Game Setup rows, Quick Options and
    // team player list from (the script index has no engine object before
    // a game starts). The game's start moves it onto the script index with
    // the script reference, the options and the common settings (team
    // play, one life, no starting weapon) whole and the type area dropped.
    let siege = crate::community_variants::listed_named("Siege").unwrap();
    assert!(siege.record.team_play());
    let was = siege.record.to_bytes();
    let engine = |raw: &[u8]| {
        i32::from_le_bytes(raw[variants::record::ENGINE_AT..variants::record::ENGINE_AT + 4].try_into().unwrap())
    };
    assert_eq!(siege.record.game_type(), variants::GameType::Slayer);
    assert_ne!(engine(&was), SCRIPT);
    let mut options = options_with(was);
    let moved = on_script_index(&mut options);
    assert_eq!(moved[..], was[variants::ext::AT..variants::ext::AT + LEN], "the script reference and options stay");
    let started = record_at(&options);
    assert_eq!(engine(&started), SCRIPT);
    assert_eq!(&started[0x4c..0xe0], &was[0x4c..0xe0], "the common settings stay");
    let v = variants::GameVariant::from_bytes(&started).unwrap();
    assert!(v.bytes()[variants::record::TYPE_AREA].iter().all(|&b| b == 0));
}
