//! The runtime over invented guest memory: a multiplayer game whose record
//! names a script, two players, fake engine callees and a local record
//! store in place of the process's `SimExt` blob.

use std::sync::{Arc, Mutex};

use engine::entry::{EntryFuture, Regs, testing};
use engine::testing::FakeGuest;
use engine::view::fake;
use engine::view::players::Player;
use variants::ext::{LEN, Script};

use super::*;
use crate::rules::feature::Shared;

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
        .put_u32(points::PLAYERS.base, PLAYERS);
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
        move |cx: Cx| -> EntryFuture { Box::pin(run_work(cx, Arc::clone(&s), work.clone())) };
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
fn every_builtin_compiles_and_nothing_is_ported_yet() {
    assert_eq!(slayer().id, ::rules::compile(::rules::builtin::SLAYER).unwrap().id());
    assert!(PORTED.is_empty(), "Slayer stays lifted until its port passes parity");
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

/// One tick's medal award for player `slot`: a bounce off `spot`, or a
/// Bounced with none.
fn award(slot: u8, spot: Option<u32>) -> crate::superbounce_chain::Award {
    use hud_kit_core::huds::superbounce::Medal;
    crate::superbounce_chain::Award {
        seq: 1,
        slot,
        user: None,
        tick: TICK,
        chain: 1,
        medals: vec![if spot.is_some() { Medal::Superbounce } else { Medal::Bounced }],
        spot,
    }
}

#[test]
fn a_bounce_reaches_on_bounce_and_scores_at_the_next_tick_boundary() {
    let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
    assert_eq!(host_refusal(&ir), None, "the host delivers on bounce");
    let (blob, id) = (ir.encode(), ir.id());
    let store = Local::default();
    let s = shared(
        &store,
        Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())),
    );
    // Bounce points 2 (raw 2 at bit 4); kill points 1, suicides free.
    let (m, datums) = guest(block(prefix(&id), 2 << 4), SLAYER);
    s.lock().start(&m, false);
    let state = s.lock().state();
    assert!(state.running, "{state:?}");
    // A Bounced is no bounce; the bounce off strip 77 is.
    s.lock().bounces(&m, &[award(0, None), award(1, Some(77))]);
    {
        let rt = s.lock();
        let r = rt.running().unwrap();
        assert_eq!(r.vm.player_var(0, "bounces"), Some(0));
        assert_eq!(r.vm.player_var(1, "bounces"), Some(1));
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
    assert!(line.contains("the carrier engine's own rules"), "{line}");
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
            "rules \"Teams\" 1\ncarrier slayer\nrequires player.team\n\non kill(victim, killer) { set_team(victim, team 1) }\n",
            "the script calls set_team, whose engine entry is untraced",
        ),
        (
            "rules \"Spawner\" 1\ncarrier slayer\n\non spawn(p) { score(p, 1) }\n",
            "the script answers on spawn, which this host runtime does not deliver yet",
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
    let src = "rules \"Builder\" 1\ncarrier slayer\nrequires forge.move\n\non kill(victim, killer) { score(killer, 1) }\n";
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
fn a_script_on_another_engine_is_refused() {
    let store = Local::default();
    let s = shared(&store, Box::new(no_fetch));
    let (m, _) = guest(block(prefix(&slayer().id), 0), 1);
    s.lock().start(&m, false);
    let why = s.lock().state().refused.unwrap();
    assert!(why.contains("rides engine 2"), "{why}");
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
    s.lock().post_tick(&m);
    assert_eq!(s.lock().state().budget.unwrap().tick, 0);
    assert_eq!(store.bytes(), Some(at_start));
    // A full restore clears it: the script stops, by name.
    *store.0.lock().unwrap() = None;
    let mut m = m;
    m.put_u32(GAME_TIME + 8, TICK + 1);
    s.lock().post_tick(&m);
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

// ---- scripts from the game-variant store and Live ----

use crate::game_variants::GameScripts;
use crate::game_variants::store::{Origin, Store as VariantStore};
use live_hle::game_variants::LiveVariant;

const POINTS: &str = "rules \"Double Points\" 1\ncarrier slayer\n\non kill(victim, killer) { score(killer, 2) }\n";

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
