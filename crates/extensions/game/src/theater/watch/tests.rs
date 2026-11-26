use super::*;
use crate::level_boot::{MODE_CAMPAIGN, MODE_MAINMENU, MODE_MULTIPLAYER, OPT_PATH, OPT_PATH_LEN};
use crate::theater::actions::at_update;
use crate::theater::game_start::tests::{Temp, block, game};
use crate::theater::tests::{arena, host_facts, options};
use agent::sim::calls::Call;
use film::file::FilmActions;
use runtime::guest::Permissions;
use serde_json::json;
use settings::Value as Setting;
use std::sync::Arc;

/// Where the synthetic engine builds its update, and the machine's state.
const UPDATE: u32 = 0xc2_0000;
const STATE: u32 = 0x80_0000;
const ACTION: usize = 0x5c;
const SETTING: &str = "graphics.single_copy";

const FIRST: &str = "scenarios\\solo\\03a_oldmombasa\\03a_oldmombasa";
const SECOND: &str = "scenarios\\multi\\zanzibar\\zanzibar";

/// An options block that names its scenario, as `level_boot::Request::recorded`
/// reads one.
fn named(mode: u32, salt: u8, scenario: &str) -> Vec<u8> {
    let mut options = block(mode, salt);
    let at = OPT_PATH as usize;
    options[at..at + OPT_PATH_LEN as usize].fill(0);
    options[at..at + scenario.len()].copy_from_slice(scenario.as_bytes());
    options
}

fn machine(options: &[u8], words: [u32; 2]) -> runtime::Guest {
    let mut g = game(options, words);
    g.map_zeroed(UPDATE, 0x5000, Permissions::RW).unwrap();
    g
}

/// The engine builds the tick's update: one player, an action of its own.
fn engine_update(g: &mut runtime::Guest, n: u64) {
    let mut action = [0x11u8; ACTION];
    action[0] = n as u8;
    g.set_u32(UPDATE + 8, 1).unwrap();
    g.write_bytes(UPDATE + 12, &action, 1).unwrap();
}

/// The synthetic game: its state is a function of every update it has taken.
fn step(g: &mut runtime::Guest) {
    let mask = g.u32(UPDATE + 8).unwrap();
    let first = g.u8(UPDATE + 12).unwrap();
    let state = g.u32(STATE).unwrap();
    g.set_u32(
        STATE,
        state
            .wrapping_mul(31)
            .wrapping_add(u32::from(first))
            .wrapping_add(mask * 7)
            .wrapping_add(1),
    )
    .unwrap();
}

/// A tick of a game, as the engine runs it: the update is built, the hook
/// sees it, the game takes it, the tick ends. False once the session
/// stopped counting.
fn tick(session: &Arc<Session>, g: &mut runtime::Guest, n: u64) -> bool {
    {
        let state = session.lock();
        if state.complete || state.failure.is_some() {
            return false;
        }
    }
    engine_update(g, n);
    {
        let state = session.lock();
        at_update(session, state, g, Ok(UPDATE));
    }
    step(g);
    session.checkpoint(g);
    true
}

fn recorder_of(dir: &Temp, name: &str, xbe: &[u8]) -> Arc<Session> {
    let facts = Facts { light: false, player_features: true, ..host_facts() };
    let session = Session::open(Some(dir.0.join(name)), None, facts, xbe, &options(&dir.0))
        .unwrap()
        .unwrap();
    let store = Arc::new(settings::Store::defaults(&[&crate::setting_defs::SINGLE_COPY]));
    assert!(session.video_menu.set(store).is_ok());
    session
}

fn store(session: &Session) -> Arc<settings::Store> {
    session.video_menu.get().unwrap().clone()
}

fn film_ticks(session: &Session) -> u64 {
    let s = session.lock();
    let Mode::Record(r) = &s.mode else { panic!("a recording") };
    r.film.as_ref().unwrap().lock().unwrap().ticks()
}

/// Play one game of `ticks` ticks into the recorder and end it, so its
/// film is cut. Returns the film tick of its first update.
fn record_game(rec: &Arc<Session>, options: &[u8], words: [u32; 2], ticks: u64) -> u64 {
    let mut g = machine(options, words);
    rec.game_activated(&g);
    let first = film_ticks(rec) + 1;
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(first, "zanzibar", film::blam::Mode::Custom);
    }
    for n in 1..=ticks {
        assert!(tick(rec, &mut g, n));
    }
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(first, "zanzibar", film::blam::Mode::Custom);
        r.game_started_ms = Some(1_795_000_000_000 + first);
        r.match_over = true;
    }
    rec.finish(first * 10, false).unwrap();
    first
}

/// The films cut so far, oldest first.
fn cuts(rec: &Session) -> Vec<PathBuf> {
    let s = rec.lock();
    let mut films = s.outcome.films.clone();
    films.sort();
    films
}

struct Viewer {
    dir: Temp,
    rec: Arc<Session>,
    film: PathBuf,
    options: Vec<u8>,
    queue: calls::Shared,
    returned: Arc<Mutex<Vec<String>>>,
    first: u64,
}

const LENGTH: u64 = 30;
const MENUS: u64 = 10;

/// A session that has recorded one game of [`LENGTH`] ticks after
/// [`MENUS`] of menus, and is back in its menus.
fn viewer() -> Viewer {
    let dir = Temp::new();
    let rec = recorder_of(&dir, "auto-20261125-100000-000.h2film", b"xbe");
    let queue = calls::shared();
    calls::lock(&queue).arm();
    rec.attach_calls(queue.clone());
    let returned = Arc::new(Mutex::new(Vec::new()));
    let seen = returned.clone();
    rec.on_return(move |id| seen.lock().unwrap().push(id.to_owned()));
    let menus = arena();
    for _ in 0..MENUS {
        rec.checkpoint(&menus);
    }
    let options = named(MODE_CAMPAIGN, 1, FIRST);
    let first = record_game(&rec, &options, [11, 12], LENGTH);
    let film = cuts(&rec).pop().expect("the game's film");
    Viewer { dir, rec, film, options, queue, returned, first }
}

impl Viewer {
    /// Watch the film: plan, arm, and let its game activate. Returns the
    /// game the film plays in.
    fn watch(&self) -> runtime::Guest {
        let planned = self.rec.plan_watch(&self.film, api::InitialView::default()).unwrap();
        self.rec.arm_watch(planned);
        let g = machine(&self.options, [999, 888]);
        self.rec.game_activated(&g);
        assert!(self.rec.watching());
        g
    }

    fn pending(&self) -> Vec<Call> {
        calls::lock(&self.queue).drain().into_iter().map(|(_, call)| call).collect()
    }
}

#[test]
fn a_session_records_a_game_watches_it_leaves_and_records_another_and_both_recordings_are_whole() {
    let v = viewer();
    let rec = &v.rec;
    let store = store(rec);
    assert_eq!(v.first, MENUS + 1);

    // The viewer's setting is not the film's, which is recorded with the
    // default.
    store.run_as(SETTING, Setting::Bool(false)).unwrap();
    let planned = rec.plan_watch(&v.film, api::InitialView::default()).unwrap();
    assert_eq!(planned.length_ticks(), MENUS + LENGTH);
    assert_eq!(
        planned.steps(),
        level_boot::game_steps(&level_boot::Request::recorded(&v.options).unwrap()).unwrap()
    );
    rec.arm_watch(planned);
    assert_eq!(store.get(SETTING), Some(Setting::Bool(true)), "the film's setting");
    assert!(!rec.watching(), "the film starts with its game");

    // Until its game activates the session records its menus.
    let menus = arena();
    for _ in 0..3 {
        rec.checkpoint(&menus);
    }
    let recorded = film_ticks(rec);
    assert_eq!(recorded, MENUS + LENGTH + 3);

    let mut g = machine(&v.options, [999, 888]);
    rec.game_activated(&g);
    assert!(rec.watching());
    {
        let s = rec.lock();
        assert!(matches!(s.mode, Mode::Replay(..)) && s.play.is_some());
        assert!(!s.holds_in_game());
    }
    let mut ticks = 0;
    while tick(rec, &mut g, 0xee) {
        ticks += 1;
        assert!(ticks <= LENGTH, "the film ends with its last tick");
    }
    {
        let s = rec.lock();
        assert_eq!((ticks, s.failure.clone()), (LENGTH, None), "digests matched");
        assert!(s.complete && s.holds_in_game(), "the last tick is held");
        assert_eq!(s.tick, MENUS + LENGTH);
    }

    // Leave: the main menu is queued, and the game is the film's until it loads.
    rec.end_film().unwrap();
    assert!(rec.watching() && !rec.lock().holds_in_game());
    assert_eq!(v.pending(), vec![Call::QueueGame { steps: crate::game_menu::steps() }]);
    rec.game_activated(&game(&block(MODE_MAINMENU, 0), [0, 0]));
    assert!(!rec.watching());
    assert_eq!(*v.returned.lock().unwrap(), vec![rec.film_id_of(&v.film)]);
    assert_eq!(store.get(SETTING), Some(Setting::Bool(false)), "the viewer's setting is back");
    assert_eq!(film_ticks(rec), recorded, "a film being watched is not recorded");

    // The next game records as if the film had not been watched.
    for _ in 0..5 {
        rec.checkpoint(&menus);
    }
    let second = named(MODE_MULTIPLAYER, 2, SECOND);
    let first = record_game(rec, &second, [21, 22], 20);
    assert_eq!(first, recorded + 5 + 1);
    rec.finish(900, true).unwrap();

    let films = cuts(rec);
    assert_eq!(films.len(), 2);
    let starts = |path: &Path| {
        let mut reader = film::file::FilmReader::open(std::fs::File::open(film::file::beside(path)).unwrap()).unwrap();
        let blocks = reader.game_starts.clone();
        blocks.iter().map(|b| reader.game_start_at(b).unwrap()).collect::<Vec<_>>()
    };
    let length = |path: &Path| FilmActions::open(&film::file::beside(path)).unwrap().length_ticks();
    let (a, b) = (starts(&films[0]), starts(&films[1]));
    assert_eq!((a.len(), a[0].game.first_update, a[0].options.clone()), (1, 11, v.options.clone()));
    assert_eq!(length(&films[0]), MENUS + LENGTH);
    // The second film holds the first game and then its own: the ticks of
    // the watched film are in neither.
    assert_eq!(b.len(), 2);
    assert_eq!((b[0].game.first_update, b[1].game.first_update), (11, first));
    assert_eq!((b[1].options.clone(), b[1].random), (second, [21, 22]));
    assert_eq!(length(&films[1]), recorded + 5 + 20);
}

impl Session {
    /// The id a film's header names.
    fn film_id_of(&self, path: &Path) -> String {
        Reader::open(path).unwrap().header.meta.id
    }
}

#[test]
fn a_start_route_seek_queues_the_options_again_and_reaches_its_tick() {
    let v = viewer();
    let rec = &v.rec;
    let mut g = v.watch();
    for n in 0..25 {
        assert!(tick(rec, &mut g, n));
    }
    assert_eq!(rec.lock().tick, MENUS + 25);

    // Behind the tick shown: the film starts over.
    let target = v.first + 4;
    rec.transport(api::SEEK, json!({ "tick": target })).unwrap();
    {
        let s = rec.lock();
        assert_eq!((s.tick, s.restart, s.seeking), (0, None, Some(target)));
        assert!(s.play.as_ref().unwrap().awaiting());
        assert_eq!(s.seek.as_ref().unwrap().source, api::SeekSource::Boot);
    }
    let steps = level_boot::game_steps(&level_boot::Request::recorded(&v.options).unwrap()).unwrap();
    assert_eq!(v.pending(), vec![Call::QueueGame { steps }]);

    // The game that runs until the options load is not the film's.
    assert!(tick(rec, &mut g, 0));
    assert_eq!(rec.lock().tick, 0);
    assert_eq!(rec.lock().failure, None);

    // The queued game activates and the film plays from its first update.
    let mut again = machine(&v.options, [5, 6]);
    rec.game_activated(&again);
    assert!(!rec.lock().play.as_ref().unwrap().awaiting());
    // Bounded, so a seek that never arrives fails instead of hanging.
    for _ in 0..=(target - v.first + 1) {
        if rec.lock().tick >= target {
            break;
        }
        assert!(tick(rec, &mut again, 0xee));
    }
    let s = rec.lock();
    assert_eq!((s.tick, s.seeking, s.failure.clone()), (target, None, None));
    // `watching()` takes the session's lock, which `s` holds.
    assert!(s.watch.is_some());
}

#[test]
fn a_cancelled_start_route_seek_goes_back_to_the_tick_it_left_and_holds_there() {
    let v = viewer();
    let rec = &v.rec;
    let mut g = v.watch();
    for n in 0..25 {
        assert!(tick(rec, &mut g, n));
    }
    let from = MENUS + 25;
    rec.transport(api::SEEK, json!({ "tick": v.first + 4 })).unwrap();
    assert_eq!(v.pending().len(), 1);

    // Cancelled before it arrives: the game is queued again, for the tick
    // the seek left, and the film holds there.
    rec.transport(api::SEEK, json!({ "cancel": true })).unwrap();
    {
        let s = rec.lock();
        assert_eq!((s.seeking, s.restart, s.paused), (Some(from), None, true));
        assert_eq!(s.seek.as_ref().unwrap().target, from);
        assert!(s.play.as_ref().unwrap().awaiting());
    }
    assert_eq!(v.pending().len(), 1, "the game is queued again");
    let mut again = machine(&v.options, [5, 6]);
    rec.game_activated(&again);
    for _ in 0..=(from - v.first + 1) {
        if rec.lock().tick >= from {
            break;
        }
        assert!(tick(rec, &mut again, 0xee));
    }
    let s = rec.lock();
    assert_eq!((s.tick, s.seeking, s.failure.clone()), (from, None, None));
    assert!(s.holding());
}

#[test]
fn a_seek_ahead_runs_forward_from_the_tick_shown() {
    let v = viewer();
    let rec = &v.rec;
    let mut g = v.watch();
    for n in 0..5 {
        assert!(tick(rec, &mut g, n));
    }
    let target = v.first + 20;
    rec.transport(api::SEEK, json!({ "tick": target })).unwrap();
    {
        let s = rec.lock();
        assert_eq!((s.restart, s.seeking), (None, Some(target)));
        assert_eq!(s.seek.as_ref().unwrap().source, api::SeekSource::Forward);
        assert_eq!(s.demand().run_to, Some(target));
    }
    assert!(v.pending().is_empty(), "no game is queued");
    for _ in 0..=target {
        if rec.lock().tick >= target {
            break;
        }
        assert!(tick(rec, &mut g, 0xee));
    }
    let s = rec.lock();
    assert_eq!((s.tick, s.seeking, s.failure.clone()), (target, None, None));
    assert_eq!(s.demand().run_to, None);
}

#[test]
fn the_start_route_can_be_asked_for_and_a_seek_beyond_the_film_is_refused() {
    let v = viewer();
    let rec = &v.rec;
    let mut g = v.watch();
    assert!(tick(rec, &mut g, 0));
    let late = v.first + 10;
    rec.transport(api::SEEK, json!({ "tick": late, "source": "boot" })).unwrap();
    assert!(rec.lock().play.as_ref().unwrap().awaiting());
    assert_eq!(v.pending().len(), 1);
    assert!(rec.transport(api::SEEK, json!({ "tick": MENUS + LENGTH + 1 })).is_err());
}

#[test]
fn the_viewers_settings_come_back_when_the_film_never_starts_or_the_session_ends() {
    let v = viewer();
    let rec = &v.rec;
    let store = store(rec);
    store.run_as(SETTING, Setting::Bool(false)).unwrap();

    // Disarmed before its game.
    rec.arm_watch(rec.plan_watch(&v.film, api::InitialView::default()).unwrap());
    assert_eq!(store.get(SETTING), Some(Setting::Bool(true)));
    rec.disarm_watch();
    assert_eq!(store.get(SETTING), Some(Setting::Bool(false)));

    // Another game starts first: the film is not watched and the session
    // goes on recording.
    rec.arm_watch(rec.plan_watch(&v.film, api::InitialView::default()).unwrap());
    let other = machine(&named(MODE_MULTIPLAYER, 3, SECOND), [1, 2]);
    rec.game_activated(&other);
    assert!(!rec.watching());
    assert_eq!(store.get(SETTING), Some(Setting::Bool(false)));
    assert!(matches!(rec.lock().mode, Mode::Record(_)));

    // The session ends while the film plays.
    let mut other = v.watch();
    assert_eq!(store.get(SETTING), Some(Setting::Bool(true)));
    assert!(tick(rec, &mut other, 0));
    // One film at a time: another is refused until this one is left.
    let refused = rec.plan_watch(&v.film, api::InitialView::default()).err().unwrap();
    assert!(format!("{refused:#}").contains("already being watched"), "{refused:#}");
    rec.finish(900, true).unwrap();
    assert!(!rec.watching());
    assert_eq!(store.get(SETTING), Some(Setting::Bool(false)));
}

#[test]
fn a_film_that_this_session_cannot_play_is_refused_before_anything_is_queued() {
    let v = viewer();
    let rec = &v.rec;
    // Not a film of this session's XBE.
    let other = Temp::new();
    let foreign = recorder_of(&other, "auto-20261125-100001-000.h2film", b"another xbe");
    let menus = arena();
    for _ in 0..3 {
        foreign.checkpoint(&menus);
    }
    record_game(&foreign, &v.options, [1, 2], 5);
    let film = cuts(&foreign).pop().unwrap();
    let why = rec.plan_watch(&film, api::InitialView::default()).err().unwrap();
    assert!(format!("{why:#}").contains("different XBE"), "{why:#}");
    // No film file, so no game start.
    let why = rec
        .plan_watch(&v.dir.0.join("nothing.h2film"), api::InitialView::default())
        .err()
        .unwrap();
    assert_eq!(why.to_string(), NO_GAME_START);
    // A session that watches nothing has nothing to leave.
    assert!(!rec.watching() && rec.can_watch());
    assert!(rec.end_film().is_err());
    assert!(v.pending().is_empty());
}

#[test]
fn watch_and_end_are_methods_of_a_game_that_records_and_end_needs_a_film() {
    struct Empty;
    impl agent::Reader for Empty {
        fn status(&mut self) -> agent::GameStatus {
            Default::default()
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }
    let v = viewer();
    let a = agent::Agent::new();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let client = a.connect(tx);
    super::super::library_methods(
        &a,
        v.dir.0.clone(),
        v.dir.0.join("synthetic.xbe"),
        v.dir.0.clone(),
        v.dir.0.clone(),
        Installation::new(b"xbe", vec![], v.dir.0.clone()),
        Some(v.rec.clone()),
    );
    let call = |method: &str, params: Value| {
        a.submit(
            client,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        );
        a.on_tick(&mut Empty);
        serde_json::from_str::<Value>(&rx.try_recv().unwrap()).unwrap()
    };
    let answer = call(api::END, json!({}));
    assert_eq!(answer["error"]["code"], json!(agent_proto::code::UNAVAILABLE), "{answer}");
    assert!(answer["error"]["message"].as_str().unwrap().contains("no film"));
    // Transport requests with no film are the player process's, and there is none.
    let state = call(api::STATE, json!({}));
    assert_eq!(state["result"]["active"], json!(false), "{state}");

    let watching = v.watch();
    drop(watching);
    let ended = call(api::END, json!({}));
    assert_eq!(ended["result"]["leaving"], json!(true), "{ended}");
    let state = call(api::STATE, json!({}));
    assert_eq!(state["result"]["id"], json!(v.rec.film_id_of(&v.film)), "{state}");
}

#[test]
fn a_film_that_diverges_in_the_running_game_ends_like_end_film_and_the_game_goes_on() {
    let v = viewer();
    let rec = &v.rec;
    let store = store(rec);
    store.run_as(SETTING, Setting::Bool(false)).unwrap();
    let planned = rec.plan_watch(&v.film, api::InitialView::default()).unwrap();
    rec.arm_watch(planned);
    // The game's arena at its first update is not the one the film started
    // from.
    let mut g = machine(&v.options, [999, 888]);
    rec.game_activated(&g);
    assert!(rec.watching());
    g.set_u32(STATE, 0xbad0_bad0).unwrap();
    assert!(tick(rec, &mut g, 0));
    let why = rec.lock().failure.clone().expect("the start hash differs");
    assert!(why.contains("start hash"), "{why}");

    // The slice's end turns the failure into leaving the film: no error
    // for the guest, the main menu queued, the film's reason kept.
    rec.finish(1, false).expect("the guest goes on");
    assert_eq!(v.pending(), vec![Call::QueueGame { steps: crate::game_menu::steps() }]);
    {
        let s = rec.lock();
        assert!(s.watch.is_some() && s.failure.is_none() && !s.holds_in_game());
    }
    assert_eq!(rec.status().watch_failure, Some(why));
    rec.finish(2, false).expect("and keeps going while the menu loads");
    assert_eq!(store.get(SETTING), Some(Setting::Bool(true)), "the film's settings stay until the menu");

    rec.game_activated(&game(&block(MODE_MAINMENU, 0), [0, 0]));
    assert!(!rec.watching());
    assert_eq!(*v.returned.lock().unwrap(), vec![rec.film_id_of(&v.film)]);
    assert_eq!(store.get(SETTING), Some(Setting::Bool(false)), "the viewer's setting is back");
    // Nothing of the film was recorded: the library has the one game.
    assert_eq!(cuts(rec).len(), 1);
    assert!(rec.status().watch_failure.is_some(), "kept for the page");
    // Nothing is left that would stop the session: the slice loop stops on a
    // completed or failed playing state, a pending restart or seek, and a
    // closed window, and the recorder that came back has none of them.
    {
        let s = rec.lock();
        assert!(matches!(s.mode, Mode::Record(_)) && s.watch.is_none() && s.pacing.is_none());
        assert!(!s.complete && s.failure.is_none() && s.restart.is_none() && s.seeking.is_none());
    }
    rec.finish(3, false).expect("the recorder goes on at the menu");
    let menus = arena();
    rec.checkpoint(&menus);
    assert!(!rec.lock().complete);
}
