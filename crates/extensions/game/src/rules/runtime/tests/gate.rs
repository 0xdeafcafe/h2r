//! The host's start gate (docs/game/rules-start-gate.md): over invented
//! guest memory, a script that names a label its map lacks blocks the game
//! with every problem, no VM and one `end_round` at the first boundary; a
//! map that has it starts; a film skips the check; a Forge session refuses
//! without ending; and the script reads the labelled things.

use ::rules::ir::{LabelDef, LabelKind, label_flags};
use forge_doc::BlockKind;

use super::*;

const SCNR: u32 = 0x1006_0000;
const FLAGS_AT: u32 = SCNR + 0x400;

/// [`guest`] with a scenario whose netgame flags are `flags` (type, team).
fn mapped(block: [u8; LEN], engine: i32, flags: &[(u16, u16)]) -> FakeGuest {
    let (mut m, _) = guest(block, engine);
    m.put_u32(points::SCENARIO.base, SCNR);
    m.map(SCNR, 0x1000);
    m.put_u32(SCNR + engine::objectives::NETGAME_FLAGS, flags.len() as u32)
        .put_u32(SCNR + engine::objectives::NETGAME_FLAGS + 4, FLAGS_AT);
    for (i, &(kind, team)) in flags.iter().enumerate() {
        let f = FLAGS_AT + i as u32 * engine::objectives::NETGAME_FLAG_STRIDE;
        m.put_u16(f + engine::objectives::FLAG_TYPE, kind)
            .put_u16(f + engine::objectives::FLAG_TEAM, team);
    }
    m
}

/// A Slayer script that scores a kill and declares one zone, `text`, which
/// a map must have exactly one of.
fn needing(text: &str) -> Ir {
    let mut ir = ::rules::compile(POINTS).unwrap();
    ir.labels.push(LabelDef {
        text: text.into(),
        kind: LabelKind::Zone,
        min: 1,
        max: 1,
        flags: label_flags::SINGLE,
        lo: 0,
        hi: 0,
    });
    ir
}

/// A runtime whose fetch answers `ir`, and the record's block naming it.
fn serving(store: &Local, ir: &Ir) -> (Arc<Shared>, [u8; LEN]) {
    let blob = ir.encode();
    let id = ir.id();
    let s = shared(store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    (s, block(prefix(&id), 0))
}

const TERRITORY: u16 = engine::objectives::kind::TERRITORY;

#[test]
fn a_map_that_lacks_a_label_blocks_the_game_and_ends_it_in_its_first_tick() {
    let store = Local::default();
    let ir = needing("goal");
    let (s, record) = serving(&store, &ir);
    let m = mapped(record, SLAYER, &[(TERRITORY, 0)]);
    let line = s.lock().start(&m, false);
    assert!(line.starts_with("rules: blocked, the game does not start: "), "{line}");
    let want = "the map has no zone labelled \"goal\"; Double Points needs exactly 1";
    assert!(line.contains(want), "{line}");
    // No VM, no record, no hash on a Live session.
    assert!(s.lock().running().is_none());
    assert_eq!(store.bytes(), None);
    assert_eq!(s.lock().hash(), None);
    assert_eq!(s.lock().blocked(), Some(&[want.to_string()][..]));
    let state = s.lock().state();
    assert!(!state.running);
    assert_eq!(state.blocked, [want]);
    assert_eq!(state.hash, Some(ir.id().to_string()));
    assert!(state.refused.unwrap().contains("the game was ended"));
    assert_eq!(state.labelled.len(), 1, "the map's territory flag");
    assert_eq!(state.labelled[0].label, "territory");
    assert!(!state.labelled[0].declared);
    // A kill runs nothing.
    s.lock().kill(&m, 0, Some(1));
    // The first boundary ends the game, on the authority, once.
    let work = s.lock().pre_tick(&m).expect("the game's end");
    assert_eq!(
        work,
        Work {
            tick: TICK + 1,
            ask_authority: true,
            calls: vec![Call::End],
        }
    );
    assert_eq!(s.lock().pre_tick(&m), None, "one end_round");
    let calls = drive(&s, m, work, true);
    assert_eq!(calls[0], GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base));
    assert!(calls.iter().all(|c| c.target != points::SCORE_ADD.base));
    // The block holds after the game's end, for the lobby's verdict.
    s.lock().ended();
    assert_eq!(s.lock().state().blocked, [want]);
}

#[test]
fn a_machine_that_is_not_the_authority_ends_no_blocked_game() {
    let store = Local::default();
    let (s, record) = serving(&store, &needing("goal"));
    let m = mapped(record, SLAYER, &[]);
    s.lock().start(&m, false);
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, false);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    let state = s.lock().state();
    assert!(state.blocked.is_empty());
    assert!(state.refused.unwrap().contains("not the game engine's authority"));
}

#[test]
fn a_map_that_has_the_label_starts_the_script() {
    let store = Local::default();
    let (s, record) = serving(&store, &needing("territory"));
    let m = mapped(record, SLAYER, &[(TERRITORY, 1)]);
    let line = s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(state.running, "{line}");
    assert!(state.blocked.is_empty());
    assert_eq!(state.labelled.len(), 1);
    assert!(state.labelled[0].declared);
    assert_eq!(state.labelled[0].team, Some(1));
}

#[test]
fn a_film_skips_the_check() {
    let store = Local::default();
    let ir = needing("goal");
    let blob = ir.encode();
    let s = Arc::new(Shared::new(
        Runtime::new(
            Box::new(store.clone()),
            Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
        )
        .with_film(true),
    ));
    let m = mapped(block(prefix(&ir.id()), 0), SLAYER, &[]);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
}

#[test]
fn a_forge_session_refuses_the_script_without_ending_the_game() {
    let store = Local::default();
    let (s, record) = serving(&store, &needing("goal"));
    let m = mapped(record, SLAYER, &[]);
    s.lock().set_forge_session(true);
    let line = s.lock().start(&m, false);
    assert!(line.starts_with("rules: blocked in a Forge session"), "{line}");
    let state = s.lock().state();
    assert!(!state.running);
    assert_eq!(state.blocked.len(), 1);
    assert!(state.refused.unwrap().contains("Forge session"));
    assert_eq!(s.lock().pre_tick(&m), None, "the game is not ended");
    assert_eq!(store.bytes(), None);
}

#[test]
fn a_census_that_cannot_be_read_blocks_a_script_with_labels() {
    let store = Local::default();
    let ir = needing("goal");
    let blob = ir.encode();
    let s = Arc::new(Shared::new(
        Runtime::new(
            Box::new(store.clone()),
            Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
        )
        .with_census(Box::new(|_: &dyn GuestMemory| Err("no scenario".to_string()))),
    ));
    let m = mapped(block(prefix(&ir.id()), 0), SLAYER, &[]);
    s.lock().start(&m, false);
    assert_eq!(
        s.lock().state().blocked,
        ["the map's labels could not be read: no scenario"]
    );
    // A script with no labels reads no census it needs.
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
}

#[test]
fn a_swap_to_a_script_the_map_lacks_a_label_for_is_refused() {
    let store = Local::default();
    let (s, record) = serving(&store, &needing("territory"));
    let m = mapped(record, SLAYER, &[(TERRITORY, 0)]);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    let why = s.lock().swap(needing("goal")).unwrap_err();
    assert!(why.contains("no zone labelled \"goal\""), "{why}");
    assert!(s.lock().state().running, "the old program keeps running");
}

#[test]
fn the_script_reads_the_labelled_things() {
    // Two territories (teams 0 and 1 by designator) and a hill corner of
    // hill 2; designator 1 plays as game team 3.
    let mut m = mapped(
        [0; LEN],
        SLAYER,
        &[
            (TERRITORY, 0),
            (TERRITORY, 1),
            (engine::objectives::kind::HILL + 2, 8),
        ],
    );
    let teams = ENGINE + engine::objectives::DESIGNATOR_TEAMS;
    m.put_u16(teams, 0).put_u16(teams + 2, 3);
    let things = census::things(&m).unwrap();
    let mut ir = needing("hill_corner");
    ir.labels[0] = LabelDef::objects("hill_corner");
    ir.labels.push(LabelDef::objects("territory"));
    let table = Table::new(&things, &ir);
    let mut calls = Vec::new();
    let mut g = GuestGame::new(&m, &mut calls).with_table(&table);
    let territories = g.labelled("territory");
    assert_eq!(territories.len(), 2);
    let flags = BlockKind::NetgameFlags as u32;
    assert_eq!(territories[0].placement, flags << 16);
    assert_eq!(territories[1].placement, flags << 16 | 1);
    let corner = g.labelled("hill_corner");
    assert_eq!(corner.len(), 1);
    let read = |g: &mut GuestGame<'_>, name: &str, o: u32| {
        g.read(&VmCall {
            entry: caps::entry_named(name).unwrap(),
            args: vec![Arg::Object(Some(o))],
        })
    };
    let (t0, t1, c) = (territories[0].object, territories[1].object, corner[0].object);
    assert_eq!(read(&mut g, "label_number", c), 2, "the hill's number");
    assert_eq!(read(&mut g, "label_number", t0), 0, "unnumbered");
    assert_eq!(read(&mut g, "label_of", c), 0);
    assert_eq!(read(&mut g, "label_of", t1), 1);
    assert_eq!(read(&mut g, "zone_id", t1), (flags << 16 | 1) as i32);
    assert_eq!(read(&mut g, "object_team", t0), 0);
    assert_eq!(read(&mut g, "object_team", t1), 3, "through the designator");
    assert_eq!(read(&mut g, "object_team", c), ::rules::ty::NONE, "neutral");
    // Not a labelled thing.
    assert_eq!(read(&mut g, "label_of", 77), ::rules::ty::NONE);
    assert_eq!(read(&mut g, "zone_id", 77), 0);
    assert!(g.labelled("goal").is_empty());
}

#[test]
fn label_reads_labelled_sets_and_zone_tests_are_hostable() {
    for name in ["label_number", "label_of", "zone_id", "object_team", "inside", "player_of"] {
        let e = caps::entry_named(name).unwrap();
        assert!(e.status.hostable(), "{name}");
        assert!(READS.contains(&e.id), "{name}");
    }
    // `ticks_held` is the VM's, over the host's zone test.
    assert!(caps::entry_named("ticks_held").unwrap().status.hostable());
    let loadout = caps::entry_named("set_loadout").unwrap();
    assert!(loadout.status.hostable());
    assert!(ACTIONS.contains(&loadout.id));
    // A script with labels is no longer refused for them.
    assert_eq!(host_refusal(&needing("goal")), None);
}
