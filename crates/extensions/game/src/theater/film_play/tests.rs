use super::*;
use crate::level_boot::{MODE_CAMPAIGN, MODE_MULTIPLAYER};
use crate::theater::actions::{apply, apply_body, at_update, capture_body};
use crate::theater::game_start::first_update;
use crate::theater::game_start::tests::{Temp, block, game, RANDOM};
use crate::theater::tests::{arena, host_facts, options};
use crate::theater::{Mode, Presentation, Replay, VerificationMode};
use agent_proto::theater::{self as api, VerificationReport, Verify};
use serde_json::json;
use film::blam::SeatInput;
use film::columns::{BODY_BYTES, SEATS};
use film::file::{FilmWriter, Seed, TICK_DIGEST};
use runtime::guest::Permissions;
use std::{fs, io::Cursor};

/// Where the synthetic engine builds its update.
const UPDATE: u32 = 0xc2_0000;
/// The synthetic machine's whole state, in the engine's arena.
const STATE: u32 = 0x80_0000;
const ACTION: usize = 0x5c;
/// Where the update's body begins, past the action block.
const BODY: u32 = UPDATE + 0x5cc;

fn action(tick: u64) -> [u8; ACTION] {
    let mut a = [0x11; ACTION];
    a[0] = tick as u8;
    a
}

/// A guest in a game with the engine's arena and an update to build into.
fn machine(options: &[u8], words: [u32; 2]) -> runtime::Guest {
    let mut g = game(options, words);
    g.map_zeroed(UPDATE, 0x5000, Permissions::RW).unwrap();
    g
}

/// What the synthetic game does with an update: its state is a function of
/// every update it has taken, so a changed action changes every digest after.
fn step(g: &mut runtime::Guest) {
    let mask = g.u32(UPDATE + 8).unwrap();
    let first = g.u8(UPDATE + 12).unwrap();
    let state = g.u32(STATE).unwrap();
    let body = g.u32(BODY).unwrap();
    g.set_u32(
        STATE,
        state
            .wrapping_mul(31)
            .wrapping_add(u32::from(first))
            .wrapping_add(mask * 7)
            .wrapping_add(body.wrapping_mul(13))
            .wrapping_add(1),
    )
    .unwrap();
}

/// One application of the synthetic game's update, as a recording of it
/// writes the film tick: the engine builds the actions and a body that
/// holds the film tick's number, the game takes it, and the film keeps the
/// actions, the body and the digest if the game's tick ends here.
fn applied(w: &mut FilmWriter<Cursor<Vec<u8>>>, g: &mut runtime::Guest, digest: bool) {
    let a = action(w.ticks() + 1);
    apply(&mut *g, UPDATE, &[SeatInput { seat: 0, action: Some(a), pad: None }]).unwrap();
    g.set_u32(BODY, w.ticks() as u32 + 1).unwrap();
    step(g);
    let mut seats = [None; SEATS];
    seats[0] = Some(a);
    if digest {
        let hash = crate::sim_glue::film_gameplay_hash(g).expect("a digest in a game");
        w.push_tick(TICK_DIGEST, hash, &seats).unwrap();
    } else {
        w.push_tick(0, 0, &seats).unwrap();
    }
    w.tail(&capture_body(&*g, UPDATE).unwrap()).unwrap();
}

/// The film file of a session of `menus` ticks and then these games, each
/// after five more ticks of menus: ticks of play for each game, as a
/// recording of the synthetic game writes them. `twice` is the game tick
/// of the last game that applies its update twice, which is two film ticks.
fn film_of(
    header: &film::Header,
    menus: u64,
    games: &[(Vec<u8>, [u32; 2], u64)],
    twice: Option<u64>,
) -> Vec<u8> {
    let seed = Seed { header: header.clone(), descriptor: Default::default() };
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed, 30).unwrap();
    let none = [None; SEATS];
    for _ in 0..menus {
        w.push_tick(0, 0, &none).unwrap();
    }
    for (n, (options, words, ticks)) in games.iter().enumerate() {
        if n > 0 {
            for _ in 0..5 {
                w.push_tick(0, 0, &none).unwrap();
            }
        }
        let mut g = machine(options, *words);
        let start = first_update(&g, options.clone(), header, None, w.ticks() + 1).unwrap();
        w.game_start(&start).unwrap();
        for game_tick in 1..=*ticks {
            if twice == Some(game_tick) && n + 1 == games.len() {
                applied(&mut w, &mut g, false);
            }
            applied(&mut w, &mut g, true);
        }
    }
    w.finish().unwrap().into_inner()
}

struct Fixture {
    dir: Temp,
    file: std::path::PathBuf,
    second: Vec<u8>,
    first_update: u64,
    /// The film ticks of the second game: its game ticks, and one more when
    /// one of them applied its update twice.
    second_ticks: u64,
}

const MENUS: u64 = 10;
const FIRST_GAME: u64 = 30;
const SECOND_GAME: u64 = 40;

impl Fixture {
    /// A container recorded by a session, then the film file of a session:
    /// menus, a game, menus, a second game.
    fn new() -> Self {
        Self::with(None)
    }

    /// [`Fixture::new`], with the second game applying its update twice on
    /// game tick `twice`.
    fn with(twice: Option<u64>) -> Self {
        let dir = Temp::new();
        let file = dir.0.join("fixture.h2film");
        let rec = Session::open(Some(file.clone()), None, host_facts(), b"xbe", &options(&dir.0))
            .unwrap()
            .unwrap();
        let menus = arena();
        for _ in 0..30 {
            rec.checkpoint(&menus);
        }
        rec.finish(90, true).unwrap();
        let header = film::Reader::open(&file).unwrap().header;
        let second = block(MODE_CAMPAIGN, 2);
        let bytes = film_of(
            &header,
            MENUS,
            &[
                (block(MODE_MULTIPLAYER, 1), [11, 12], FIRST_GAME),
                (second.clone(), [21, 22], SECOND_GAME),
            ],
            twice,
        );
        fs::write(film::file::beside(&file), bytes).unwrap();
        Self {
            dir,
            file,
            second,
            first_update: MENUS + FIRST_GAME + 5 + 1,
            second_ticks: SECOND_GAME + u64::from(twice.is_some()),
        }
    }

    /// The player on an empty data directory.
    fn open(&self) -> Arc<Session> {
        self.try_open().unwrap().unwrap()
    }

    fn try_open(&self) -> Result<Option<Arc<Session>>> {
        self.open_with(|_| ())
    }

    /// The player as a verification worker for `request`, writing its
    /// report to `report`.
    fn verifier(&self, request: Verify, report: &std::path::Path) -> Arc<Session> {
        let mode = VerificationMode::Worker { request, report: report.to_owned() };
        self.open_with(|replay| replay.verification = Some(mode)).unwrap().unwrap()
    }

    /// The player, with `edit` made to how it is opened.
    fn open_with(&self, edit: impl FnOnce(&mut Replay)) -> Result<Option<Arc<Session>>> {
        let data = self.dir.0.join("empty-data");
        fs::create_dir_all(&data).unwrap();
        let mut replay = Replay {
            verification: None,
            path: self.file.clone(),
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Presentation::default(),
            camera: Default::default(),
            window_closed: None,
        };
        edit(&mut replay);
        Session::open(
            None,
            Some(replay),
            host_facts(),
            b"xbe",
            &options(&data),
        )
    }
}

/// One tick of the synthetic game as the engine runs it: the update is
/// built, the hook sees it, the game takes it, the tick ends. `tamper` can
/// change the update after the hook.
fn tick(
    session: &Arc<Session>,
    g: &mut runtime::Guest,
    tamper: impl FnOnce(&mut runtime::Guest),
) -> bool {
    {
        let state = session.lock();
        if state.complete || state.failure.is_some() {
            return false;
        }
        // The engine builds each update afresh: its body is empty.
        apply_body(&mut *g, UPDATE, &[0; BODY_BYTES]).unwrap();
        at_update(session, state, g, Ok(UPDATE));
    }
    tamper(g);
    step(g);
    session.checkpoint(g);
    true
}

#[test]
fn a_film_recorded_after_menus_and_a_first_game_plays_its_second_game_to_matching_digests() {
    let fixture = Fixture::new();
    let session = fixture.open();
    assert!(session.plays_from_start() && session.plays_back());
    assert!(session.seal_on_drop().is_none(), "a film player seals nothing");
    // The player answers the guest's tick count; the call taps are the host's.
    assert!(session.lock().pacing.is_some());
    assert!(!film::tape::Tap::replaying(&*session));
    let start = session.game_start().unwrap();
    assert_eq!(start.options, fixture.second);
    assert_eq!(start.game.first_update, fixture.first_update);
    assert_eq!(session.lock().length(), MENUS + FIRST_GAME + 5 + SECOND_GAME);

    // The game's own ticks before its first update are not the film's.
    let menus = arena();
    for _ in 0..7 {
        session.checkpoint(&menus);
    }
    assert_eq!(session.lock().tick, 0);

    // The player's shared random words are not the film's until the update.
    let mut g = machine(&fixture.second, [999, 888]);
    let mut ticks = 0;
    while tick(&session, &mut g, |_| ()) {
        ticks += 1;
        if ticks == 1 {
            assert_eq!(session.lock().tick, fixture.first_update);
            assert_eq!([g.u32(RANDOM).unwrap(), g.u32(RANDOM + 4).unwrap()], [21, 22]);
        }
        assert!(ticks <= SECOND_GAME, "the film ends with its last tick");
    }
    let s = session.lock();
    assert_eq!(s.failure, None);
    assert!(s.complete);
    assert_eq!((ticks, s.tick), (SECOND_GAME, MENUS + FIRST_GAME + 5 + SECOND_GAME));
}

#[test]
fn a_changed_start_hash_stops_before_the_first_update() {
    let fixture = Fixture::new();
    let session = fixture.open();
    // The player's arena is not the one the film started from.
    let mut g = machine(&fixture.second, [21, 22]);
    g.set_u32(STATE + 64, 0xfeed).unwrap();
    assert!(tick(&session, &mut g, |_| ()));
    let s = session.lock();
    let why = s.failure.as_deref().expect("the film stopped");
    assert!(
        why.contains(&format!("first divergence at tick {}", fixture.first_update))
            && why.contains("start hash"),
        "{why}"
    );
    // The report names what differs: one page of the arena.
    assert!(why.contains("1 arena page(s) differ"), "{why}");
    assert!(!s.play.as_ref().unwrap().begun());
    // Nothing was written into the update.
    drop(s);
    assert_eq!(g.u32(UPDATE + 8).unwrap(), 0);
}

#[test]
fn a_changed_action_stops_at_its_tick() {
    let fixture = Fixture::new();
    let session = fixture.open();
    let mut g = machine(&fixture.second, [21, 22]);
    let changed = 9;
    let mut n = 0;
    while tick(&session, &mut g, |g| {
        n += 1;
        if n == changed {
            g.set_u8(UPDATE + 12, 0xee).unwrap();
        }
    }) {}
    let s = session.lock();
    let why = s.failure.as_deref().expect("the film stopped");
    let at = fixture.first_update + changed - 1;
    assert!(
        why.contains(&format!("first divergence at tick {at}")) && why.contains("gameplay"),
        "{why}"
    );
    assert_eq!(s.tick, at);
    assert!(!s.complete);
}

#[test]
fn a_game_that_skips_the_recorded_update_or_takes_one_the_film_does_not_hold_is_stopped() {
    let fixture = Fixture::new();
    let session = fixture.open();
    let mut g = machine(&fixture.second, [21, 22]);
    assert!(tick(&session, &mut g, |_| ()));
    // A tick that ends with no update taken has missed the film's actions.
    session.checkpoint(&g);
    let why = session.lock().failure.clone().expect("the film stopped");
    assert!(why.contains("skipped the recorded player update"), "{why}");

    // A second update in a tick takes the film's next tick, which the game
    // has not reached: its digest is not the game's.
    let session = fixture.open();
    let mut g = machine(&fixture.second, [21, 22]);
    for _ in 0..2 {
        let state = session.lock();
        at_update(&session, state, &mut g, Ok(UPDATE));
    }
    assert_eq!(session.lock().failure, None);
    step(&mut g);
    session.checkpoint(&g);
    let why = session.lock().failure.clone().expect("the film stopped");
    assert!(why.contains("gameplay"), "{why}");
}

#[test]
fn a_tick_that_applies_its_update_twice_replays_as_two_ticks() {
    let twice = 7;
    let fixture = Fixture::with(Some(twice));
    let session = fixture.open();
    assert_eq!(
        session.lock().length(),
        MENUS + FIRST_GAME + 5 + SECOND_GAME + 1,
        "the film holds a record for each application"
    );
    let mut g = machine(&fixture.second, [999, 888]);
    let mut ticks = 0;
    while tick(&session, &mut g, |_| ()) {
        ticks += 1;
        assert!(ticks <= fixture.second_ticks, "the film ends with its last tick");
    }
    let s = session.lock();
    assert_eq!(s.failure, None);
    assert!(s.complete);
    assert_eq!((ticks, s.tick), (SECOND_GAME + 1, MENUS + FIRST_GAME + 5 + SECOND_GAME + 1));
}

#[test]
fn a_recorded_body_is_written_over_the_engines_and_the_head_and_stamp_are_not() {
    let fixture = Fixture::new();
    let session = fixture.open();
    let mut g = machine(&fixture.second, [21, 22]);
    g.set_u32(UPDATE, 0xaaaa_0001).unwrap();
    g.set_u32(UPDATE + 4, 0xaaaa_0002).unwrap();
    g.set_u32(UPDATE + 0x4040, 0xbbbb_0001).unwrap();
    g.set_u32(UPDATE + 0x4044, 0xbbbb_0002).unwrap();
    // What the engine built: a body the film does not hold.
    apply_body(&mut g, UPDATE, &[0xee; BODY_BYTES]).unwrap();
    let state = session.lock();
    at_update(&session, state, &mut g, Ok(UPDATE));
    assert_eq!(session.lock().failure, None);
    // The film's body for its first tick: the film tick's number, then zeros.
    assert_eq!(g.u32(BODY).unwrap(), fixture.first_update as u32);
    assert_eq!(capture_body(&g, UPDATE).unwrap()[4..], [0u8; BODY_BYTES - 4]);
    assert_eq!(
        [
            g.u32(UPDATE).unwrap(),
            g.u32(UPDATE + 4).unwrap(),
            g.u32(UPDATE + 0x4040).unwrap(),
            g.u32(UPDATE + 0x4044).unwrap(),
        ],
        [0xaaaa_0001, 0xaaaa_0002, 0xbbbb_0001, 0xbbbb_0002]
    );
}

/// A recording on the synthetic engine's update: `applications` of the
/// update for each of its ticks, each stamped as given.
fn recorded(stamps: &[&[u32]]) -> (Temp, std::path::PathBuf, Arc<Session>) {
    let dir = Temp::new();
    let file = dir.0.join("recorded.h2film");
    let rec = Session::open(Some(file.clone()), None, host_facts(), b"xbe", &options(&dir.0))
        .unwrap()
        .unwrap();
    let g = arena();
    for tick in stamps {
        for &stamp in *tick {
            rec.record_update(vec![seat_input(stamp)], Some(body_of(stamp)), Some(stamp));
        }
        rec.checkpoint(&g);
    }
    (dir, file, rec)
}

fn seat_input(n: u32) -> SeatInput {
    SeatInput { seat: 0, action: Some([n as u8; 92]), pad: None }
}

fn body_of(n: u32) -> Vec<u8> {
    let mut body = vec![0u8; BODY_BYTES];
    body[..4].copy_from_slice(&n.to_le_bytes());
    body[BODY_BYTES - 1] = n as u8;
    body
}

#[test]
fn two_applications_in_one_tick_give_two_records() {
    // Three game ticks; the second applies its update twice.
    let (_dir, file, rec) = recorded(&[&[10], &[11, 12], &[13]]);
    rec.finish(5, true).unwrap();
    let mut actions = film::file::FilmActions::open(&film::file::beside(&file)).unwrap();
    assert_eq!(actions.length_ticks(), 4);
    for (tick, n) in [(1, 10), (2, 11), (3, 12), (4, 13)] {
        let row = actions.row(tick).unwrap();
        assert_eq!(row.inputs, vec![seat_input(n)], "tick {tick}");
        assert_eq!(row.body, body_of(n), "tick {tick}");
    }
    // The digest is the game tick's, on its last record.
    assert_eq!(actions.row(2).unwrap().digest, None);
}

#[test]
fn a_stamp_that_skips_cuts_the_film_at_the_tick_before() {
    let (_dir, file, rec) = recorded(&[&[10], &[11], &[13], &[14]]);
    {
        let state = rec.lock();
        let Mode::Record(r) = &state.mode else { panic!("a recording") };
        assert!(r.frame.stamp_skipped && r.match_over, "the film is to be cut");
    }
    rec.finish(5, true).unwrap();
    let mut actions = film::file::FilmActions::open(&film::file::beside(&file)).unwrap();
    assert_eq!(actions.length_ticks(), 2);
    assert_eq!(actions.row(2).unwrap().inputs, vec![seat_input(11)]);
}

#[test]
fn a_stamp_follows_when_it_is_the_same_tick_the_next_or_an_earlier_one() {
    use crate::theater::actions::stamp_follows;
    assert!(stamp_follows(None, 900));
    assert!(stamp_follows(Some(5), 5));
    assert!(stamp_follows(Some(5), 6));
    assert!(!stamp_follows(Some(5), 7));
    // A campaign revert puts the clock back.
    assert!(stamp_follows(Some(500), 120));
    assert!(!stamp_follows(Some(u32::MAX - 2), u32::MAX));
}

#[test]
fn the_neutral_ports_follow_the_players_the_options_name() {
    let mut options = vec![0u8; 0x1118];
    assert_eq!(ports(&options), 1);
    for (count, ports_for) in [(1u32, 1), (2, 2), (4, 4), (9, 4)] {
        options[OPT_PLAYER_COUNT..OPT_PLAYER_COUNT + 4].copy_from_slice(&count.to_le_bytes());
        assert_eq!(ports(&options), ports_for);
    }
    assert_eq!(ports(&[]), 1);
}

#[test]
fn a_film_with_no_game_start_does_not_play_from_one() {
    let dir = Temp::new();
    let file = dir.0.join("old.h2film");
    assert!(Play::open(&file).unwrap().is_none(), "no film file");
    fs::write(film::file::beside(&file), b"not a film").unwrap();
    assert!(Play::open(&file).is_err(), "a film file that is not one");
}

#[test]
fn a_film_that_predates_game_starts_is_refused_by_name_and_not_played_another_way() {
    let fixture = Fixture::new();
    fs::remove_file(film::file::beside(&fixture.file)).unwrap();
    let Err(error) = fixture.try_open() else {
        panic!("a film with no game start does not open");
    };
    assert_eq!(error.to_string(), crate::theater::NO_GAME_START);
}

#[test]
fn verification_counts_only_the_requested_ticks_and_reports_the_first_divergence() {
    let fixture = Fixture::new();
    let report = fixture.dir.0.join("report.json");
    let result = || serde_json::from_slice::<VerificationReport>(&fs::read(&report).unwrap()).unwrap();
    let first = fixture.first_update;
    let request = Verify { from: Some(first + 10), to: Some(first + 19) };

    let session = fixture.verifier(request, &report);
    let mut g = machine(&fixture.second, [21, 22]);
    while tick(&session, &mut g, |_| ()) {}
    assert!(session.lock().complete);
    session.finish(90, true).unwrap();
    let clean = result();
    assert_eq!((clean.ticks, clean.digests_checked), (10, 10));
    assert_eq!(clean.first_divergence, None);

    // A changed action inside the range is the report's first divergence.
    let session = fixture.verifier(request, &report);
    let mut g = machine(&fixture.second, [21, 22]);
    let changed = 15;
    let mut n = 0;
    while tick(&session, &mut g, |g| {
        n += 1;
        if n == changed {
            g.set_u8(UPDATE + 12, 0xee).unwrap();
        }
    }) {}
    assert!(session.finish(90, false).is_err());
    let mismatch = result();
    let at = first + changed - 1;
    assert_eq!((mismatch.ticks, mismatch.digests_checked), (changed - 10, changed - 11));
    let divergence = mismatch.first_divergence.expect("a divergence");
    assert_eq!(divergence.tick, at);
    assert!(divergence.kind.contains("gameplay"), "{}", divergence.kind);
}

#[test]
fn a_step_advances_one_checked_tick_and_holds() {
    let fixture = Fixture::new();
    let session = fixture.open();
    let mut g = machine(&fixture.second, [21, 22]);
    assert!(tick(&session, &mut g, |_| ()));
    let at = session.lock().tick;
    assert!(session.transport(api::STEP, json!({"ticks":1})).is_err(), "a playing film does not step");
    session.transport(api::PAUSE, json!({})).unwrap();
    let left = session.lock().length() - at;
    for ticks in [0, left + 1, u64::MAX] {
        assert!(session.transport(api::STEP, json!({"ticks":ticks})).is_err(), "{ticks}");
    }
    session.transport(api::STEP, json!({"ticks":1})).unwrap();
    assert!(!session.lock().holding());
    assert!(tick(&session, &mut g, |_| ()));
    let s = session.lock();
    assert_eq!((s.tick, s.paused, s.failure.clone()), (at + 1, true, None));
    assert!(s.holding());
    assert!(s.restart.is_none());
}

#[test]
fn a_launch_seek_reports_its_remaining_ticks_and_holds_at_its_target() {
    let fixture = Fixture::new();
    let target = fixture.first_update + 4;
    let session = fixture
        .open_with(|replay| {
            replay.seek_tick = target;
            replay.paused_at_target = true;
        })
        .unwrap()
        .unwrap();
    let mut g = machine(&fixture.second, [21, 22]);
    let seek = |session: &Session| session.lock().seek_info().unwrap();
    assert_eq!((seek(&session).source, seek(&session).remaining_ticks), (api::SeekSource::Boot, target));
    for remaining in (0..=4).rev() {
        assert!(tick(&session, &mut g, |_| ()));
        assert_eq!(seek(&session).remaining_ticks, remaining);
        assert_eq!(session.lock().seeking.is_some(), remaining != 0);
    }
    let s = session.lock();
    assert_eq!((s.tick, s.failure.clone()), (target, None));
    assert!(s.holding());
}
