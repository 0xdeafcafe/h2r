//! The runtime over invented guest memory: a multiplayer game whose record
//! names a script, two players, fake engine callees and a local record
//! store in place of the process's `SimExt` blob.

use std::sync::{Arc, Mutex};

use ::rules::vm::{Arg, Call as VmCall};
use guest_layout::entry::{Cx, EntryFuture, GuestCall, Regs, Ret, testing};
use guest_layout::points;
use guest_layout::testing::FakeGuest;
use guest_layout::view::fake;
use guest_layout::view::players::Player;
use variants::ext::{LEN, Script};

use super::publish::walls_due;
use super::*;
use crate::rules::feature::Shared;

/// `role_traits` and the objectives on the host.
mod hosted;
/// The start gate and the label reads.
mod gate;
/// Sounds on the host.
mod audio;
/// Zones, the roster's events and loadouts.
mod world;
/// Tag reads: the gate's tag check and the host's read of a tag's data.
mod tag_reads;
/// A script game on the script index and its engine object.
mod script_index;
/// What each tick costs: the record's copies and writes, the roster's
/// reads, the show state's encodes.
mod per_tick;
/// `next_round` and the round reset.
mod rounds;

const GAME_GLOBALS: u32 = 0x1000_0000;
const GAME_TIME: u32 = 0x1000_2000;
const ENGINE: u32 = 0x1000_4000;
const PLAYERS: u32 = 0x1003_0000;
const TICK: u32 = 10;
const SLAYER: i32 = 2;

/// The `RULES` record, kept here instead of in the process's blob.
#[derive(Clone, Default)]
struct Local(Arc<Mutex<Option<Vec<u8>>>>);

impl Local {
    fn bytes(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap().clone()
    }
    fn put(&self, bytes: Vec<u8>) {
        *self.0.lock().unwrap() = Some(bytes);
    }
}

impl Store for Local {
    fn get(&self) -> Option<Vec<u8>> {
        self.bytes()
    }
    fn same(&self, written: &[u8]) -> Option<bool> {
        self.0.lock().unwrap().as_deref().map(|now| now == written)
    }
    fn set(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        self.put(bytes);
        Ok(())
    }
    fn remove(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

fn slayer() -> &'static Builtin {
    builtins()
        .iter()
        .find(|b| b.engine == SLAYER as u8)
        .expect("the built-in Slayer")
}

fn prefix(id: &Sha256) -> [u8; 8] {
    let mut p = [0; 8];
    p.copy_from_slice(&id.0[..8]);
    p
}

/// A record's block naming the script with `prefix` and `options`.
fn block(prefix: [u8; 8], options: u32) -> [u8; LEN] {
    Ext {
        script: Script {
            mode: Mode::Rules,
            prefix,
            options,
        },
        ..Ext::default()
    }
    .encode()
}

/// A multiplayer game at tick [`TICK`] whose record is engine `engine`
/// with `block`, free for all. Players 0 and 1 are on teams 0 and 1, with
/// no unit. Returns their salted datums.
fn guest(block: [u8; LEN], engine: i32) -> (FakeGuest, [u32; 2]) {
    let mut m = FakeGuest::new();
    m.put_u32(points::GAME_GLOBALS.base, GAME_GLOBALS)
        .put_u32(points::GAME_TIME.base, GAME_TIME)
        .put_u32(points::GAME_ENGINE.base, ENGINE)
        .put_u32(points::PLAYERS.base, PLAYERS)
        // No scenario loaded: the start gate's census reads none.
        .put_u32(points::SCENARIO.base, 0);
    m.map(GAME_GLOBALS, 0x1200).map(GAME_TIME, 0x10).map(ENGINE, 0xc9c);
    m.put_u32(GAME_GLOBALS + 8, MODE_MULTIPLAYER)
        .put_u32(GAME_GLOBALS + guest::VARIANT + guest::VARIANT_ENGINE, engine as u32)
        .put(GAME_GLOBALS + crate::variant_ext::BLOCK, &block)
        .put_u16(GAME_TIME + 2, 30)
        .put_u32(GAME_TIME + 8, TICK);
    let players = fake::datums::<Player>(&mut m, PLAYERS, 16);
    let mut datums = [0; 2];
    for (i, team) in [0, 1].into_iter().enumerate() {
        let (h, p) = fake::push(&mut m, &players);
        p.set_team(&mut m, team).unwrap();
        p.set_unit(&mut m, None).unwrap();
        datums[i] = h.raw();
    }
    (m, datums)
}

fn shared(store: &Local, fetch: Box<Fetch>) -> Arc<Shared> {
    Arc::new(Shared::new(Runtime::new(Box::new(store.clone()), fetch)))
}

/// Runs the tick's async body; the authority check answers `authority`,
/// the score entry true and every other call 0.
fn drive(shared: &Arc<Shared>, m: FakeGuest, work: Work, authority: bool) -> Vec<GuestCall> {
    let s = Arc::clone(shared);
    let entry =
        move |cx: Cx| -> EntryFuture { Box::pin(run_work(cx, Arc::clone(&s), work.clone(), 0)) };
    let run = testing::run(&entry, m, Regs::default(), |c, _| Regs {
        eax: if c.target == points::GAME_ENGINE_AUTHORITATIVE.base {
            u32::from(authority)
        } else {
            u32::from(c.target == points::SCORE_ADD.base)
        },
        ..Default::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    run.calls
}

#[test]
fn every_builtin_compiles_and_only_hostable_ports_are_ported() {
    assert_eq!(slayer().id, ::rules::compile(::rules::builtin::SLAYER).unwrap().id());
    // A ported engine's built-in runs for a no-mode record: this host must
    // deliver every event it answers and make every entry it calls.
    for &e in PORTED {
        let b = builtins().iter().find(|b| b.engine == e).expect("a built-in");
        let (ir, _) = verify::load(&b.blob).unwrap();
        assert_eq!(host_refusal(&ir), None, "engine {e}");
    }
}

#[test]
fn a_record_naming_the_builtin_slayer_starts_it_and_sets_the_record() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(line.contains("Slayer"), "{line}");
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.script.as_deref(), Some("Slayer"));
    assert_eq!(state.hash, Some(slayer().id.to_string()));
    assert_eq!(state.source.as_deref(), Some("builtin"));
    assert_eq!(state.carrier, Some(2));
    assert_eq!(state.options, Some(2));
    assert_eq!(state.authority, None);
    assert_eq!(state.budget.unwrap().limit, TICK_BUDGET);
    assert_eq!(s.lock().hash(), Some(slayer().id.to_string()));
    let record = store.bytes().expect("the RULES record is set at activation");
    assert_eq!(RulesExt::decode(&record).unwrap(), *s.lock().running().unwrap().vm.ext());
}

#[test]
fn a_kill_scores_through_score_add_at_the_next_tick_boundary() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    // Kill points 3 (raw 2 over the option's 1), suicides free.
    let (m, datums) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [Call::Score {
            player: 1,
            delta: 3
        }]
    );
    assert!(s.lock().snapshot_blocker().is_some());
    let work = s.lock().pre_tick(&m).expect("calls and the authority check");
    assert_eq!(
        work,
        Work {
            tick: TICK + 1,
            ask_authority: true,
            calls: vec![Call::Score {
                player: 1,
                delta: 3
            }],
            sight: Vec::new(),
        }
    );
    // The same boundary again does nothing.
    assert_eq!(s.lock().pre_tick(&m), None);
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
    let state = s.lock().state();
    assert!(state.running);
    assert_eq!(state.authority, Some(true));
    assert!(state.errors.is_empty(), "{:?}", state.errors);
    assert_eq!(state.pending, 0);
    assert!(s.lock().snapshot_blocker().is_none());
}

#[test]
fn a_suicide_costs_a_point_with_the_option_on() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    // Kill points 1, the suicide option (bit 3) on.
    let (m, datums) = guest(block(prefix(&slayer().id), 0b1000), SLAYER);
    s.lock().start(&m, false);
    s.lock().kill(&m, 0, None);
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls[1],
        GuestCall::new(points::SCORE_ADD.base)
            .eax(-1i32 as u32)
            .args([datums[0]])
    );
}

/// One tick's medal award for player `slot`: a bounce off `spot` at
/// chain count `chain`, or a Bounced with none.
fn award(slot: u8, spot: Option<u32>, chain: u32) -> crate::superbounce_chain::Award {
    use hud_kit_core::views::superbounce::Medal;
    crate::superbounce_chain::Award {
        seq: 1,
        slot,
        user: None,
        tick: TICK,
        chain,
        medals: match spot {
            Some(_) => crate::superbounce_chain::bounce_medals(chain).to_vec(),
            None => vec![Medal::Bounced],
        },
        spot,
        strips: spot.map(|_| 1),
    }
}

/// A landing the chain judged a bounce, for player `slot`, off `spot`,
/// launching at `speed` units/s.
fn launch(slot: u8, spot: Option<u32>, speed: f32) -> (u8, crate::superbounce_chain::Physics) {
    (slot, crate::superbounce_chain::Physics::Launch { spot, speed })
}

#[test]
fn a_launch_reaches_on_launch_and_scores_at_the_next_tick_boundary() {
    let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
    assert_eq!(host_refusal(&ir), None, "the host delivers on launch and on grounded");
    let id = ir.id();
    let store = Local::default();
    // An example is compiled in: found by its prefix, never fetched.
    let s = shared(&store, Box::new(no_fetch));
    // Bounce points 2, every penalty off.
    use ::rules::ir::OptionValue::{Bool, Int};
    let bits = ir
        .pack_options(&[
            ("bounce_points".into(), Int(2)),
            ("suicide_costs".into(), Bool(false)),
            ("bounced_costs".into(), Bool(false)),
        ])
        .unwrap();
    let (m, datums) = guest(block(prefix(&id), bits), SLAYER);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.source.as_deref(), Some("builtin"));
    assert_eq!(s.lock().hash(), Some(id.to_string()));
    // Player 1's launch off strip 77 is a bounce; player 0's touchdown
    // is none. A Bounced reaches `on bounced`, which costs nothing with
    // its option off.
    s.lock().physics(
        &m,
        &[
            (0, crate::superbounce_chain::Physics::Touchdown { down: 6.0, up: 0.0 }),
            launch(1, Some(77), 12.0),
        ],
    );
    s.lock().bounces(&m, &[award(0, None, 0)]);
    {
        let rt = s.lock();
        let r = rt.running().unwrap();
        assert_eq!(r.vm.player_var(0, "bounces"), Some(0));
        assert_eq!(r.vm.player_var(1, "bounces"), Some(1));
        assert_eq!(r.vm.player_var(1, "chain"), Some(1));
        assert_eq!(
            r.pending(),
            [Call::Score {
                player: 1,
                delta: 2
            }]
        );
    }
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls[1],
        GuestCall::new(points::SCORE_ADD.base)
            .eax(2)
            .args([datums[1]])
    );
    // The record carries the count.
    let record = RulesExt::decode(&store.bytes().unwrap()).unwrap();
    assert_eq!(&record, s.lock().running().unwrap().vm.ext());
}

/// The script keeps the chain on the host's launches and ends of runs: a
/// second launch with no ground between is a double, a launch over 16
/// units/s a Monster, a Grounded starts the count again, and a Bounced
/// reaches `on bounced`. With Superbounce's default options but the
/// suicide penalty off (so the Bounced penalty is the one that takes the
/// point): a single 1, a double 2, a Monster triple 6, then after the
/// ground a single 1, and a Bounced -1. The chain's own awards reach no
/// handler of this script.
#[test]
fn the_script_chains_the_hosts_launches_and_a_bounced_reaches_on_bounced() {
    use crate::superbounce_chain::Physics::Grounded;
    let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
    let id = ir.id();
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let bits = ir.pack_options(&[("suicide_costs".into(), ::rules::ir::OptionValue::Bool(false))]).unwrap();
    let (m, _) = guest(block(prefix(&id), bits), SLAYER);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    s.lock().physics(&m, &[launch(1, Some(77), 12.0)]);
    s.lock().physics(&m, &[launch(1, None, 11.0)]);
    s.lock().bounces(&m, &[award(1, Some(78), 2)]);
    s.lock().physics(&m, &[launch(1, Some(80), 16.5), (1, Grounded)]);
    s.lock().physics(&m, &[launch(1, Some(77), 12.0)]);
    s.lock().bounces(&m, &[award(0, None, 3)]);
    let rt = s.lock();
    let r = rt.running().unwrap();
    assert_eq!(r.vm.player_var(1, "bounces"), Some(4));
    assert_eq!(r.vm.player_var(1, "chain"), Some(1));
    assert_eq!(r.vm.player_var(1, "flying"), Some(1));
    assert_eq!(
        r.pending(),
        [
            Call::Score { player: 1, delta: 1 },
            Call::Score { player: 1, delta: 2 },
            Call::Score { player: 1, delta: 6 },
            Call::Score { player: 1, delta: 1 },
            Call::Score { player: 0, delta: -1 },
        ]
    );
}

#[test]
fn a_machine_that_is_not_the_authority_drops_the_script_and_its_record() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    s.lock().kill(&m, 0, Some(1));
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
    assert!(!state.running);
    assert!(
        state.refused.as_deref().unwrap().contains("not the game engine's authority"),
        "{state:?}"
    );
    assert_eq!(store.bytes(), None);
}

#[test]
fn an_unknown_hash_is_refused_by_name_and_the_game_plays_its_own_rules() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block([9; 8], 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(line.contains("the record engine's own rules"), "{line}");
    let state = s.lock().state();
    assert!(!state.running);
    let why = state.refused.unwrap();
    assert!(why.contains("0909090909090909"), "{why}");
    assert!(why.contains("no Live client to fetch it"), "{why}");
    assert_eq!(state.hash.as_deref(), Some("0909090909090909"));
    assert_eq!(store.bytes(), None);
    assert_eq!(s.lock().pre_tick(&m), None);
    assert_eq!(s.lock().hash(), None);

    // System Link runs only the scripts on this machine.
    s.lock().start(&m, true);
    let why = s.lock().state().refused.unwrap();
    assert!(why.contains("System Link"), "{why}");
}

#[test]
fn a_script_the_host_cannot_run_is_refused_by_the_entry_or_event_it_names() {
    for (src, want) in [
        (
            "rules \"Teams\" 1\ntype slayer\nrequires player.team\n\non kill(victim, killer) { set_team(victim, team 1) }\n",
            "the script calls set_team, whose engine entry is untraced",
        ),
        (
            "rules \"Picker\" 1\ntype slayer\n\non pickup(p, o) { score(p, 1) }\n",
            "the script answers on pickup, which this host runtime does not deliver yet",
        ),
    ] {
        let ir = ::rules::compile(src).unwrap();
        let blob = ir.encode();
        let id = ir.id();
        let store = Local::default();
        let s = shared(
            &store,
            Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
        );
        let (m, _) = guest(block(prefix(&id), 0), SLAYER);
        s.lock().start(&m, false);
        let state = s.lock().state();
        assert_eq!(state.refused.as_deref(), Some(want));
        assert_eq!(state.hash, Some(id.to_string()));
        assert_eq!(store.bytes(), None);
    }
}

#[test]
fn a_ranked_game_refuses_a_script_that_grants_edits_by_the_capability() {
    let src = "rules \"Builder\" 1\ntype slayer\nrequires forge.move\n\non kill(victim, killer) { if killer != none { score(killer, 1) } }\n";
    let ir = ::rules::compile(src).unwrap();
    let blob = ir.encode();
    let id = ir.id();
    let store = Local::default();
    let s = shared(&store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (m, _) = guest(block(prefix(&id), 0), SLAYER);
    s.lock().set_context(Context::Ranked);
    s.lock().start(&m, false);
    let why = s.lock().state().refused.unwrap();
    assert!(
        why.starts_with("capability forge.move is not allowed in a ranked game"),
        "{why}"
    );
    assert_eq!(store.bytes(), None);

    // A custom game takes the header; what stops it there is the host.
    s.lock().set_context(Context::Custom);
    s.lock().start(&m, false);
    let why = s.lock().state().refused.unwrap_or_default();
    assert!(!why.contains("not allowed in a"), "{why}");
}

#[test]
fn a_script_runs_on_another_supported_record_and_reports_its_actual_index() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    for index in [1, i32::from(::rules::SCRIPT_INDEX)] {
        let (m, _) = guest(block(prefix(&slayer().id), 0), index);
        let line = s.lock().start(&m, false);
        let state = s.lock().state();
        assert!(state.running, "{line}");
        assert_eq!(state.carrier, Some(index as u8));
    }
    let (m, _) = guest(block(prefix(&slayer().id), 0), 5);
    assert!(s.lock().start(&m, false).contains("unsupported engine index 5"));
}

#[test]
fn a_record_with_no_mode_runs_nothing_while_no_engine_is_ported() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest([0; LEN], SLAYER);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(!state.running);
    assert_eq!(state.refused.as_deref(), Some("the record names no script"));
    assert_eq!(store.bytes(), None);
}

#[test]
fn examples_are_found_by_prefix_and_never_by_engine() {
    for e in examples() {
        assert!(std::ptr::eq(compiled_in(prefix(&e.id)).unwrap(), e));
        assert!(builtins().iter().all(|b| b.id != e.id));
    }
    assert_eq!(examples().len(), ::rules::builtin::examples().len());
    assert!(std::ptr::eq(compiled_in(prefix(&slayer().id)).unwrap(), slayer()));
    assert!(compiled_in([9; 8]).is_none());
}

#[test]
fn the_record_is_removed_at_the_game_end() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    assert!(store.bytes().is_some());
    s.lock().ended();
    assert_eq!(store.bytes(), None);
    assert_eq!(
        s.lock().state().refused.as_deref(),
        Some("the game ended")
    );
    // A new game clears a stale record before it starts.
    store.put(vec![1, 2, 3]);
    let (m, _) = guest([0; LEN], SLAYER);
    s.lock().start(&m, false);
    assert_eq!(store.bytes(), None);
}

#[test]
fn a_reverted_record_resumes_the_vm_from_it() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    let at_start = store.bytes().unwrap();
    s.lock().pre_tick(&m);
    let after = store.bytes().unwrap();
    assert_ne!(after, at_start, "the tick's start is state");
    assert_eq!(s.lock().state().budget.unwrap().tick, TICK + 1);
    // A checkpoint revert puts the start's record back.
    store.put(at_start.clone());
    s.lock().post_tick(&m, &[]);
    assert_eq!(s.lock().state().budget.unwrap().tick, 0);
    assert_eq!(store.bytes(), Some(at_start));
    // A full restore clears it: the script stops, by name.
    *store.0.lock().unwrap() = None;
    let mut m = m;
    m.put_u32(GAME_TIME + 8, TICK + 1);
    s.lock().post_tick(&m, &[]);
    let why = s.lock().state().refused.unwrap();
    assert!(why.contains("RULES record was cleared"), "{why}");
}

#[test]
fn the_game_reads_the_roster_the_statborg_and_the_record() {
    let (mut m, _) = guest([0; LEN], SLAYER);
    m.put_u16(ENGINE + 0x308 + 0x1c, (-4i16) as u16);
    m.put_u32(GAME_GLOBALS + guest::VARIANT + guest::VARIANT_SCORE_LIMIT, 25);
    let mut calls = Vec::new();
    let mut g = GuestGame::new(&m, &mut calls);
    assert_eq!(g.players(), [0, 1]);
    assert_eq!(g.teams(), [0, 1]);
    let entry = |name: &str| caps::entry_named(name).unwrap();
    let p1 = vec![Arg::Player(Some(1))];
    let read = |g: &mut GuestGame<'_>, name: &str, args: Vec<Arg>| {
        g.read(&VmCall {
            entry: entry(name),
            args,
        })
    };
    assert_eq!(read(&mut g, "player_score", p1.clone()), -4);
    assert_eq!(read(&mut g, "team_of", p1.clone()), 1);
    assert_eq!(
        read(&mut g, "team_of", vec![Arg::Player(None)]),
        ::rules::ty::NONE
    );
    assert_eq!(
        read(&mut g, "round_time_left", Vec::new()),
        guest::NO_TIME_LIMIT
    );
    assert_eq!(g.variant("team_play"), 0);
    assert_eq!(g.variant("score_limit"), 25);
    g.act(&VmCall {
        entry: entry("score"),
        args: vec![Arg::Player(None), Arg::Int(1)],
    });
    g.act(&VmCall {
        entry: entry("end_round"),
        args: vec![Arg::Team(Some(0))],
    });
    assert_eq!(calls, [Call::End], "a call through none does nothing");
}

#[test]
fn the_game_answers_airborne_and_primed_from_the_chains_bodies() {
    use crate::superbounce_chain::Body;
    let (m, _) = guest([0; LEN], SLAYER);
    let mut calls = Vec::new();
    let bodies = [
        Body { slot: 0, airborne: true, crouching: false, primed: false },
        Body { slot: 1, airborne: false, crouching: false, primed: true },
    ];
    let mut g = GuestGame::new(&m, &mut calls).with_bodies(&bodies);
    let read = |g: &mut GuestGame<'_>, name: &str, p: Option<u8>| {
        g.read(&VmCall {
            entry: caps::entry_named(name).unwrap(),
            args: vec![Arg::Player(p)],
        })
    };
    assert_eq!(read(&mut g, "airborne", Some(0)), 1);
    assert_eq!(read(&mut g, "primed", Some(0)), 0);
    assert_eq!(read(&mut g, "airborne", Some(1)), 0);
    assert_eq!(read(&mut g, "primed", Some(1)), 1);
    assert_eq!(read(&mut g, "primed", Some(5)), 0, "no body, no read");
    assert_eq!(read(&mut g, "primed", None), 0, "through none reads false");
    let mut calls = Vec::new();
    let mut none = GuestGame::new(&m, &mut calls);
    assert_eq!(read(&mut none, "primed", Some(1)), 0, "no step, no bodies");
}

// ---- scripts from the game-variant store and Live ----

use crate::game_variants::GameScripts;
use crate::game_variants::store::{Origin, Store as VariantStore};
use live_hle::game_variants::LiveVariant;

const POINTS: &str = "rules \"Double Points\" 1\ntype slayer\n\non kill(victim, killer) { if killer != none { score(killer, 2) } }\n";

type Answer = Box<dyn Fn(&str, std::time::Duration) -> Result<Vec<u8>, String> + Send + Sync>;

fn answer(
    f: impl Fn(&str, std::time::Duration) -> Result<Vec<u8>, String> + Send + Sync + 'static,
) -> Answer {
    Box::new(f)
}

/// A Slayer variant carrying `blob`, as a format 2 bundle.
fn stored_bundle(blob: Vec<u8>) -> variants::portable::Bundle {
    let mut v = variants::GameVariant::blank(variants::GameType::Slayer);
    v.set_name("Double Points").unwrap();
    variants::portable::port(&v, None, None, variants::portable::PortScript::Ir(blob)).unwrap()
}

/// A Live variant whose record names `id` and whose IR is not here.
fn pending(id: &Sha256) -> LiveVariant {
    let mut v = variants::GameVariant::blank(variants::GameType::Slayer);
    v.set_name("Double Points").unwrap();
    let mut e = v.ext();
    e.script = Script {
        mode: Mode::Rules,
        prefix: prefix(id),
        options: 0,
    };
    v.set_ext(&e).unwrap();
    LiveVariant {
        id: "v1".into(),
        name: "Double Points".into(),
        description: String::new(),
        author: live_hle::game_variants::Author {
            xuid: "9000000000000001".into(),
            gamertag: "Alex".into(),
        },
        engine: 2,
        game_type: "slayer".into(),
        record_hex: v.bytes().iter().map(|b| format!("{b:02x}")).collect(),
        script_hash: Some(id.to_string()),
        script_name: Some("Double Points".into()),
        visibility: "public".into(),
        created: String::new(),
    }
}

fn scripts_over(vs: VariantStore, fetch: Answer) -> Arc<GameScripts> {
    Arc::new(GameScripts::new(Arc::new(Mutex::new(vs)), None, fetch))
}

/// The runtime as the game composes it (`Runtime::in_game`), over `scripts`.
fn composed(store: &Local, scripts: &Arc<GameScripts>) -> Arc<Shared> {
    let (l, f) = (Arc::clone(scripts), Arc::clone(scripts));
    Arc::new(Shared::new(
        Runtime::new(Box::new(store.clone()), Box::new(move |p: [u8; 8]| f.fetched(p)))
            .with_local(Box::new(move |p: [u8; 8]| l.local(p))),
    ))
}

#[test]
fn a_stored_script_runs_with_no_live_client_and_on_system_link() {
    let ir = ::rules::compile(POINTS).unwrap();
    let (blob, id) = (ir.encode(), ir.id());
    let mut vs = VariantStore::in_memory();
    assert_eq!(vs.add(Origin::Imported, stored_bundle(blob.clone())), Ok(true));
    let scripts = scripts_over(vs, answer(|_, _| panic!("a stored script is never fetched")));
    for system_link in [false, true] {
        let store = Local::default();
        let s = composed(&store, &scripts);
        let (m, _) = guest(block(prefix(&id), 0), SLAYER);
        assert_eq!(crate::rules::feature::to_fetch(&m, &scripts), None);
        let line = s.lock().start(&m, system_link);
        assert!(line.contains("Double Points"), "{line}");
        let state = s.lock().state();
        assert!(state.running, "{state:?}");
        assert_eq!(state.source.as_deref(), Some("local"));
        assert_eq!(state.hash, Some(id.to_string()));
    }
}

#[test]
fn a_live_variants_script_is_fetched_by_its_full_hash_verified_and_run() {
    let ir = ::rules::compile(POINTS).unwrap();
    let (blob, id) = (ir.encode(), ir.id());
    let mut vs = VariantStore::in_memory();
    vs.add_pending(pending(&id)).unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let served = blob.clone();
    let scripts = scripts_over(
        vs,
        answer(move |h, _| {
            seen.lock().unwrap().push(h.to_owned());
            Ok(served.clone())
        }),
    );
    let store = Local::default();
    let s = composed(&store, &scripts);
    let (m, _) = guest(block(prefix(&id), 0), SLAYER);
    // The start hook fetches first, with no guest memory held.
    assert_eq!(crate::rules::feature::to_fetch(&m, &scripts), Some(prefix(&id)));
    scripts.prefetch(prefix(&id), crate::game_variants::START_WAIT);
    assert_eq!(*asked.lock().unwrap(), [id.to_string()]);
    assert_eq!(crate::rules::feature::to_fetch(&m, &scripts), None);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.hash, Some(id.to_string()));
    // Kept: the next start needs no fetch.
    assert_eq!(scripts.local(prefix(&id)), Some(blob));
}

#[test]
fn each_fetch_failure_is_refused_by_name() {
    let id = ::rules::compile(POINTS).unwrap().id();
    // A blob that hashes to what its bundle names and does not verify.
    let junk = vec![0xee; 40];
    let junk_id = Sha256::of(&junk);
    let cases: Vec<(Sha256, Answer, &str)> = vec![
        (
            id,
            answer(|_, _| Err("the Live server did not answer within 1500 ms".into())),
            "did not answer within 1500 ms",
        ),
        (
            id,
            answer(|_, _| Err("the Live server has no such script".into())),
            "has no such script",
        ),
        (id, answer(|_, _| Ok(vec![1, 2, 3])), "hashes to"),
        (
            junk_id,
            answer(move |_, _| Ok(junk.clone())),
            "the blob does not verify",
        ),
    ];
    for (named, answer, want) in cases {
        let mut vs = VariantStore::in_memory();
        vs.add_pending(pending(&named)).unwrap();
        let scripts = scripts_over(vs, answer);
        let store = Local::default();
        let s = composed(&store, &scripts);
        let (m, _) = guest(block(prefix(&named), 0), SLAYER);
        scripts.prefetch(prefix(&named), crate::game_variants::START_WAIT);
        s.lock().start(&m, false);
        let state = s.lock().state();
        assert!(!state.running);
        let why = state.refused.unwrap();
        assert!(why.contains(want), "{want}: {why}");
        assert_eq!(store.bytes(), None);
    }
    // A prefix no bundle names a full hash for.
    let scripts = scripts_over(
        VariantStore::in_memory(),
        answer(|_, _| panic!("nothing to fetch by")),
    );
    let s = composed(&Local::default(), &scripts);
    let (m, _) = guest(block([7; 8], 0), SLAYER);
    scripts.prefetch([7; 8], crate::game_variants::START_WAIT);
    s.lock().start(&m, false);
    let why = s.lock().state().refused.unwrap();
    assert!(why.contains("full hash is not known"), "{why}");
}

#[test]
fn an_in_place_swap_keeps_the_state_and_the_next_kill_runs_the_new_handler() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 2), SLAYER);
    s.lock().start(&m, false);
    let before = s.lock().running().unwrap().vm.ext().clone();
    let src = ::rules::builtin::SLAYER.replacen(
        "score(killer, kill_points)",
        "score(killer, kill_points * 10)",
        1,
    );
    let ir = ::rules::compile(&src).unwrap();
    let id = ir.id();
    assert_eq!(s.lock().swap(ir), Ok(id));
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.hash, Some(id.to_string()));
    assert_eq!(state.options, Some(2), "the record's option bits are kept");
    assert_eq!(*s.lock().running().unwrap().vm.ext(), before);
    assert_eq!(
        RulesExt::decode(&store.bytes().unwrap()).unwrap(),
        before,
        "the record is written as it was"
    );
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [Call::Score {
            player: 1,
            delta: 30
        }]
    );
    // Another state shape keeps the old program.
    let shaped = format!("{src}\nglobal var kills: int = 0\n");
    let why = s.lock().swap(::rules::compile(&shaped).unwrap()).unwrap_err();
    assert!(why.contains("global variables"), "{why}");
    assert_eq!(s.lock().state().hash, Some(id.to_string()));
}

/// A script whose kill handler orders the killer to follow the victim
/// (docs/game/rules-world-api.md, "Bots").
const BOT_ORDERS: &str = "rules \"Bot Orders\" 1\ntype slayer\nrequires bots.orders\n\non kill(victim, killer) { if killer != none { bot_follow(killer, victim) } }\n";

/// A runtime over the record of a game playing `src`, with the script
/// served by the fetch.
fn playing(store: &Local, src: &str) -> (Arc<Shared>, FakeGuest) {
    let ir = ::rules::compile(src).unwrap();
    let blob = ir.encode();
    let id = ir.id();
    let s = shared(store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (m, _) = guest(block(prefix(&id), 0), SLAYER);
    (s, m)
}

#[test]
fn a_bot_order_is_made_at_the_next_tick_boundary() {
    let store = Local::default();
    let (s, m) = playing(&store, BOT_ORDERS);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    s.lock().kill(&m, 0, Some(1));
    // Queued: the order waits in the VM's queue for the boundary.
    assert_eq!(s.lock().running().unwrap().pending(), []);
    let work = s.lock().pre_tick(&m).unwrap();
    assert_eq!(
        work.calls,
        [Call::BotOrder {
            player: 1,
            order: Some(bots::brain::Order::Follow(0)),
        }]
    );
    let calls = drive(&s, m, work, true);
    // The order reaches no engine entry: it delivers to the phantom
    // registry, which has no phantom in this game, so it is refused.
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    let state = s.lock().state();
    assert!(state.running);
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.errors[0].call, "bot_follow");
    assert!(state.errors[0].error.contains("player 1 is no bot"), "{:?}", state.errors);
}

#[test]
fn a_bot_order_changes_no_rulesext_byte() {
    // One script: the kill makes the order with a killer, none without.
    // The runs share one IR, so the record's rng seed (`vm::seed`, from
    // the IR's hash) matches; only the order differs.
    let run = |killer: Option<u8>| {
        let store = Local::default();
        let (s, m) = playing(&store, BOT_ORDERS);
        s.lock().start(&m, false);
        s.lock().kill(&m, 0, killer);
        let work = s.lock().pre_tick(&m).unwrap();
        drive(&s, m, work, true);
        store.bytes().unwrap()
    };
    assert_eq!(
        run(Some(1)),
        run(None),
        "an order changes no RULES record byte"
    );
}

#[test]
fn a_film_makes_no_bot_order() {
    let store = Local::default();
    let ir = ::rules::compile(BOT_ORDERS).unwrap();
    let blob = ir.encode();
    let s = Arc::new(Shared::new(
        Runtime::new(
            Box::new(store.clone()),
            Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
        )
        .with_film(true),
    ));
    let (m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    // The re-run VM makes the call; the playback runtime ignores it
    // (docs/game/rules-world-api.md, "Films and Theater").
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(s.lock().running().unwrap().pending(), []);
    let work = s.lock().pre_tick(&m).unwrap();
    assert_eq!(work.calls, []);
}

// ---- a `uses` script (docs/game/rules-variants.md, "A script that keeps
// its carrier's rules") ----

/// A settings-only Slayer script: its game runs the built-in Slayer.
const CARRIED: &str = "rules \"SWAT\" 1\ntype slayer\nuses slayer\nabout \"One headshot kills.\"\nsettings {\n    score_limit = 50\n}\n";

/// A settings-only Slayer script that asks for the superbounce mode.
const CARRIED_BOUNCES: &str = "rules \"Bounce Slayer\" 1\ntype slayer\nrequires hud.superbounce\nuses slayer\nsettings {\n    score_limit = 25\n}\n";

/// A runtime whose fetch answers `src`'s blob, started on a game of
/// `engine` whose record names it, with team play on and a score limit of
/// 50 in the record (what its settings made it). Returns the runtime, the
/// game, the players' datums, the start's line and the script's hash.
fn carried(store: &Local, src: &str, engine: i32) -> (Arc<Shared>, FakeGuest, [u32; 2], String, Sha256) {
    let ir = ::rules::compile(src).unwrap();
    assert!(ir.header.uses.is_some());
    let (blob, id) = (ir.encode(), ir.id());
    let s = shared(store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (mut m, datums) = guest(block(prefix(&id), 0), engine);
    let v = GAME_GLOBALS + guest::VARIANT;
    m.put_u32(v + guest::VARIANT_FLAGS, guest::TEAM_PLAY)
        .put_u32(v + guest::VARIANT_SCORE_LIMIT, 50);
    let line = s.lock().start(&m, false);
    (s, m, datums, line, id)
}

#[test]
fn a_carrier_scored_slayer_script_runs_the_builtin_slayer_under_its_record() {
    let store = Local::default();
    let (s, m, datums, line, id) = carried(&store, CARRIED, SLAYER);
    assert!(line.starts_with("rules: Slayer ("), "{line}");
    assert!(line.ends_with(&format!(", for SWAT ({id}) from live, which uses it")), "{line}");
    // The built-in runs, with its own default options: the carried script
    // has none, so the record's option bits are not read.
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.refused, None);
    assert_eq!(state.scoring, Some(wire::Scoring::Script));
    assert_eq!(state.script.as_deref(), Some("Slayer"));
    assert_eq!(state.hash, Some(slayer().id.to_string()));
    assert_eq!(state.source.as_deref(), Some("builtin"));
    assert_eq!(state.carrier, Some(SLAYER as u8));
    let defaults = ::rules::compile(::rules::builtin::SLAYER).unwrap().default_options();
    assert_eq!(state.options, Some(defaults));
    // The Live session names the built-in, which every guest has.
    assert_eq!(s.lock().hash(), Some(slayer().id.to_string()));
    assert_eq!(
        s.lock().carried(),
        Some(&Carried {
            hash: id,
            source: Source::Live,
            name: "SWAT".into(),
        })
    );
    let record = store.bytes().expect("the RULES record is set at activation");
    assert_eq!(RulesExt::decode(&record).unwrap(), *s.lock().running().unwrap().vm.ext());
    // The record is the variant's own: the VM reads its team play and its
    // score limit, which the runtime leaves as they are.
    {
        let mut calls = Vec::new();
        let g = GuestGame::new(&m, &mut calls);
        assert_eq!(g.variant("team_play"), 1);
        assert_eq!(g.variant("score_limit"), 50);
    }
    // A kill across teams scores the built-in's kill points through the
    // VM, at the next tick boundary.
    s.lock().kill(&m, 0, Some(1));
    assert_eq!(
        s.lock().running().unwrap().pending(),
        [Call::Score {
            player: 1,
            delta: 1
        }]
    );
    let work = s.lock().pre_tick(&m).expect("the call and the authority check");
    assert!(work.ask_authority);
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls[1],
        GuestCall::new(points::SCORE_ADD.base)
            .eax(1)
            .args([datums[1]])
    );
}

#[test]
fn a_carrier_whose_builtin_this_host_cannot_run_is_refused_by_name() {
    let store = Local::default();
    // Oddball's built-in answers on pickup and drop, which this host does
    // not deliver yet.
    let (s, m, _, line, id) = carried(&store, ::rules::builtin::WHEEEEEE, 3);
    assert!(line.starts_with("rules: refused"), "{line}");
    let state = s.lock().state();
    assert!(!state.running, "{state:?}");
    let why = state.refused.unwrap();
    assert!(
        why.starts_with(
            "Wheeeeee! uses the built-in Oddball script, which this host cannot run yet: "
        ),
        "{why}"
    );
    // Refused before the gate: the map's labels end no game here.
    assert!(state.blocked.is_empty());
    assert_eq!(state.hash, Some(id.to_string()));
    assert_eq!(s.lock().carried(), None);
    assert_eq!(s.lock().hash(), None);
    assert_eq!(store.bytes(), None);
    assert_eq!(s.lock().pre_tick(&m), None);
}

/// Every listed `uses` script runs the built-in it uses or is refused in
/// words that name it; none is blocked or left to the engine in silence.
/// The nine on Slayer all run.
#[test]
fn every_listed_carrier_script_runs_its_builtin_or_is_refused_by_name() {
    let mut slayers = Vec::new();
    for b in ::rules::builtin::LISTED {
        let ir = ::rules::compile(b.source).unwrap();
        if ir.header.uses.is_none() {
            continue;
        }
        let name = ir.header.name.clone();
        let store = Local::default();
        // A listed script is compiled in: found by its prefix.
        let s = shared(&store, Box::new(no_fetch));
        let (mut m, _) = guest(block(prefix(&ir.id()), 0), i32::from(b.engine));
        // A map with a hill, for the built-in that reads one.
        let (scnr, flags_at) = (0x1006_0000, 0x1006_0400);
        m.put_u32(points::SCENARIO.base, scnr);
        m.map(scnr, 0x1000);
        m.put_u32(scnr + engine::objectives::NETGAME_FLAGS, 1)
            .put_u32(scnr + engine::objectives::NETGAME_FLAGS + 4, flags_at)
            .put_u16(flags_at + engine::objectives::FLAG_TYPE, 11);
        let line = s.lock().start(&m, false);
        let state = s.lock().state();
        assert!(state.blocked.is_empty(), "{name}: {state:?}");
        if state.running {
            let builtin = builtins().iter().find(|x| x.engine == b.engine).unwrap();
            assert_eq!(state.hash, Some(builtin.id.to_string()), "{name}");
            assert_eq!(state.scoring, Some(wire::Scoring::Script), "{name}");
            let carried = s.lock().carried().cloned().expect("the carried script");
            assert_eq!((carried.name.as_str(), carried.source), (name.as_str(), Source::Builtin));
            assert_eq!(carried.hash, ir.id());
        } else {
            let why = state.refused.unwrap();
            assert!(
                why.starts_with(&format!("{name} uses the built-in ")),
                "{name}: {why}"
            );
            assert!(line.starts_with("rules: refused"), "{line}");
            assert_eq!(state.hash, Some(ir.id().to_string()));
            assert_eq!(store.bytes(), None);
        }
        if b.engine == SLAYER as u8 {
            assert!(state.running, "{name} runs the built-in Slayer: {line}");
            slayers.push(name);
        }
    }
    assert_eq!(
        slayers,
        [
            "Zombies",
            "Jousting",
            "SWAT",
            "Duel",
            "Tower of Power",
            "Cops and Robbers",
            "Bounce Practice",
            "Glitch Hunt",
            "Bumper Cars",
        ]
    );
}

#[test]
fn a_carried_game_takes_no_carrier_program_in_place_and_ends_with_the_game() {
    let store = Local::default();
    let (s, _m, _, _, _) = carried(&store, CARRIED, SLAYER);
    // A program with no handlers would leave the game with no scorer.
    let why = s.lock().swap(::rules::compile(CARRIED).unwrap()).unwrap_err();
    assert!(why.contains("uses line"), "{why}");
    assert_eq!(s.lock().state().script.as_deref(), Some("Slayer"));
    s.lock().ended();
    let state = s.lock().state();
    assert!(!state.running);
    assert_eq!(state.refused.as_deref(), Some("the game ended"));
    assert_eq!(state.scoring, None);
    assert_eq!(s.lock().carried(), None);
    assert_eq!(store.bytes(), None);
}

#[test]
fn its_requires_still_turn_the_mode_on() {
    let ir = ::rules::compile(CARRIED_BOUNCES).unwrap();
    assert!(crate::rules::header::asks_superbounce(&ir));
    // The mode is read from the record's script reference, so the blob is
    // on this machine (the game-variant store), as a stored variant's is.
    crate::game_variants::store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .save_script(ir.encode())
        .unwrap();
    let store = Local::default();
    let (s, _m, _, _, id) = carried(&store, CARRIED_BOUNCES, SLAYER);
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    assert_eq!(state.script.as_deref(), Some("Slayer"));
    assert_eq!(s.lock().carried().map(|c| c.name.as_str()), Some("Bounce Slayer"));
    let ext = Ext::in_effect(&block(prefix(&id), 0));
    assert!(crate::rules::header::script_asks_superbounce(ext.script));
    assert!(crate::variant_ext::decide(ext, None, false).superbounce());
    // A carrier script without the capability asks for nothing.
    let plain = ::rules::compile(CARRIED).unwrap();
    assert!(!crate::rules::header::asks_superbounce(&plain));
}

/// Juggernaut's built-in on the host: the role is dealt as each round
/// starts (`on round_start`: the first from the start, each later one from
/// the engine's round reset) and passed on when its holder quits (`on
/// quit`, then `on tick`), through the engine's role setter.
mod round_start_and_quit {
    use super::*;
    use crate::rules::roles;

    const JUGGERNAUT: u8 = 7;
    const JUGGERNAUT_GLOBALS: u32 = 0x1005_0000;

    fn juggernaut() -> &'static Builtin {
        builtins()
            .iter()
            .find(|b| b.engine == JUGGERNAUT)
            .expect("the built-in Juggernaut")
    }

    /// A Juggernaut game whose record carries `block`, no one holding the
    /// role.
    fn game(block: [u8; LEN]) -> (FakeGuest, [u32; 2]) {
        let (mut m, datums) = guest(block, i32::from(JUGGERNAUT));
        m.put_u32(points::JUGGERNAUT_GLOBALS.base, JUGGERNAUT_GLOBALS);
        m.map(JUGGERNAUT_GLOBALS, 0x10);
        m.put_u16(JUGGERNAUT_GLOBALS + roles::ROLE_MASK, 0);
        (m, datums)
    }

    /// Takes player `slot` off the roster as the engine does before its
    /// player-left notice (player `+2` bit 1).
    fn leave(m: &mut FakeGuest, slot: u8) {
        let players = guest_layout::view::players::array(&*m).unwrap().unwrap();
        let p = players.get_absolute(&*m, i32::from(slot)).unwrap().unwrap();
        p.set_flags(m, 2).unwrap();
    }

    /// The script's Juggernaut, by its global (`ty::NONE` for none).
    fn holder(s: &Arc<Shared>) -> i32 {
        s.lock().running().unwrap().vm.global("juggernaut").unwrap()
    }

    fn pending(s: &Arc<Shared>) -> Vec<Call> {
        s.lock().running().unwrap().pending().to_vec()
    }

    fn role(player: u8, on: bool) -> Call {
        Call::RoleTraits { player, on }
    }

    /// Starts the script and makes its first boundary: the first round
    /// deals the role, and the engine's role setter gives it. Returns the
    /// slot it went to.
    fn started(s: &Arc<Shared>, m: &FakeGuest, datums: [u32; 2]) -> u8 {
        let line = s.lock().start(m, false);
        assert!(s.lock().state().running, "{line}");
        let j = holder(s);
        assert!(j == 0 || j == 1, "the first round deals the role: {j}");
        let j = j as u8;
        assert_eq!(pending(s), [role(j, true)]);
        let work = s.lock().pre_tick(m).unwrap();
        let calls = drive(s, m.clone(), work, true);
        assert_eq!(
            calls,
            [
                GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
                GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                    .eax(datums[j as usize])
                    .ecx(1),
                GuestCall::new(points::GAME_TICK.base),
            ]
        );
        assert!(s.lock().state().errors.is_empty());
        j
    }

    #[test]
    fn the_builtin_needs_nothing_this_host_lacks_and_runs_for_a_record_with_no_mode() {
        let (ir, _) = verify::load(&juggernaut().blob).unwrap();
        assert_eq!(host_refusal(&ir), None);
        assert!(PORTED.contains(&JUGGERNAUT));
        let store = Local::default();
        let s = shared(&store, Box::new(no_fetch));
        let (m, datums) = game([0; LEN]);
        started(&s, &m, datums);
        let state = s.lock().state();
        assert_eq!(state.script.as_deref(), Some("Juggernaut"));
        assert_eq!(state.source.as_deref(), Some("builtin"));
        assert_eq!(state.hash, Some(juggernaut().id.to_string()));
    }

    #[test]
    fn each_later_round_deals_the_role_again() {
        let store = Local::default();
        let s = shared(&store, Box::new(no_fetch));
        let (m, datums) = game(block(prefix(&juggernaut().id), 0));
        started(&s, &m, datums);
        // A new Juggernaut takes the role from the last one; the same one
        // is asked for the role it holds, which the host leaves alone.
        for _ in 0..6 {
            let before = holder(&s) as u8;
            let n = pending(&s).len();
            s.lock().round_start(&m);
            let now = holder(&s);
            assert!(now == 0 || now == 1, "{now}");
            let now = now as u8;
            let want = if now == before {
                vec![role(now, true)]
            } else {
                vec![role(before, false), role(now, true)]
            };
            assert_eq!(pending(&s)[n..], want[..]);
        }
        assert!(s.lock().state().errors.is_empty());
    }

    #[test]
    fn another_players_quit_leaves_the_juggernaut_alone() {
        let store = Local::default();
        let s = shared(&store, Box::new(no_fetch));
        let (mut m, datums) = game(block(prefix(&juggernaut().id), 0));
        let j = started(&s, &m, datums);
        let other = 1 - j;
        leave(&mut m, other);
        s.lock().quit(&m, other);
        s.lock().post_tick(&m, &[]);
        assert_eq!(holder(&s), i32::from(j));
        assert!(pending(&s).is_empty());
    }

    #[test]
    fn the_juggernauts_quit_passes_the_role_at_the_next_tick() {
        let store = Local::default();
        let s = shared(&store, Box::new(no_fetch));
        let (mut m, datums) = game(block(prefix(&juggernaut().id), 0));
        let j = started(&s, &m, datums);
        let heir = 1 - j;
        // The engine took the leaver off the roster (its own slot `+0x1c`
        // clears the leaver's role bit); the script lets the role go.
        leave(&mut m, j);
        s.lock().quit(&m, j);
        assert_eq!(holder(&s), ::rules::ty::NONE);
        assert!(pending(&s).is_empty(), "{:?}", pending(&s));
        // `on tick` names one from those left.
        s.lock().post_tick(&m, &[]);
        assert_eq!(holder(&s), i32::from(heir));
        assert_eq!(pending(&s), [role(heir, true)]);
        // The next boundary gives it through the role setter.
        m.put_u32(GAME_TIME + 8, TICK + 1);
        let work = s.lock().pre_tick(&m).unwrap();
        assert!(!work.ask_authority);
        let calls = drive(&s, m, work, true);
        assert_eq!(
            calls,
            [
                GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                    .eax(datums[heir as usize])
                    .ecx(1),
                GuestCall::new(points::GAME_TICK.base),
            ]
        );
        assert!(s.lock().state().errors.is_empty());
    }
}
