use super::*;
use crate::theater::tests::{host_facts, options};
use crate::theater::{Facts, Mode, Session};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("film-auto-{tag}-{}", live_proto::sign::random_id()));
        fs::create_dir_all(dir.join("data")).unwrap();
        Self(dir)
    }
    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A player session's recording: by default (`auto`) or with `--film`.
fn local(dir: &Temp, path: &Path, auto: bool) -> Arc<Session> {
    let facts = Facts { light: false, player_features: auto, ..host_facts() };
    Session::open(Some(path.to_owned()), None, facts, b"xbe", &options(&dir.data()))
        .unwrap()
        .unwrap()
}

fn inputs(request: Request) -> Inputs {
    Inputs {
        request,
        feature_on: true,
        explicit: false,
        replaying: false,
        host_light: false,
        sim: false,
        lockstep: false,
    }
}

#[test]
fn a_player_session_with_no_destination_records_and_every_other_run_stays_quiet() {
    assert_eq!(decide(&inputs(Request::On)), Decision::Record);
    // Not a player session: `h2 run`, `h2 serve`, hosts and Theater children.
    assert_eq!(decide(&inputs(Request::Off)), Decision::Quiet);
    // `--film` or H2_FILM_OUT already chose a destination.
    assert_eq!(decide(&Inputs { explicit: true, ..inputs(Request::On) }), Decision::Quiet);
    assert_eq!(decide(&Inputs { replaying: true, ..inputs(Request::On) }), Decision::Quiet);
    assert_eq!(decide(&Inputs { host_light: true, ..inputs(Request::On) }), Decision::Quiet);
    assert_eq!(decide(&Inputs { sim: true, ..inputs(Request::On) }), Decision::Quiet);
}

#[test]
fn the_switch_turns_default_recording_off_without_a_line() {
    let off = Inputs { feature_on: false, ..inputs(Request::On) };
    assert_eq!(decide(&off), Decision::Quiet);
    // Even a session that would have said why stays quiet once it is off.
    assert_eq!(decide(&Inputs { feature_on: false, ..inputs(Request::Skip(JOIN_AT_LAUNCH)) }), Decision::Quiet);
}

#[test]
fn a_launch_join_and_lockstep_skip_recording_and_say_why() {
    assert_eq!(decide(&inputs(Request::Skip(JOIN_AT_LAUNCH))), Decision::Skip(JOIN_AT_LAUNCH));
    let lockstep = decide(&Inputs { lockstep: true, ..inputs(Request::On) });
    assert!(matches!(lockstep, Decision::Skip(why) if why.contains("H2_LOCKSTEP")), "{lockstep:?}");
}

#[test]
fn default_films_are_named_by_their_utc_start_and_never_overwrite() {
    let at = UNIX_EPOCH + Duration::from_millis(1_795_000_000_123);
    let name = file_name(at);
    assert_eq!(name, "auto-20261118-110640-123.h2film");
    assert!(is_auto(Path::new(&name)));
    assert!(!is_auto(Path::new("match.h2film")));
    assert!(!is_auto(Path::new("auto-x.h2stats")));
    // Later starts sort after earlier ones, so the newest name is last.
    let later = file_name(at + Duration::from_secs(1));
    assert!(name < later);

    let dir = Temp::new("name");
    let first = destination(&dir.0, at);
    assert_eq!(first, dir.0.join(&name));
    fs::write(&first, b"taken").unwrap();
    let second = destination(&dir.0, at);
    assert_ne!(second, first);
    assert!(is_auto(&second) && !second.exists());
}

#[test]
fn pruning_keeps_the_newest_default_films_and_every_saved_or_explicit_one() {
    let dir = Temp::new("prune");
    let start = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
    let mut sealed = Vec::new();
    for n in 0..5 {
        let path = dir.0.join(file_name(start + Duration::from_secs(n)));
        // A sealed film, written the way any recording seals.
        local(&dir, &path, false).finish(1, true).unwrap();
        assert!(film::LibraryEntry::read(&path).is_ok());
        sealed.push(path);
    }
    // The oldest is saved: never pruned.
    fs::write(sealed[0].with_extension("h2film.saved"), b"").unwrap();
    // A film the player named, and a stale default film a crash left unsealed.
    let named = dir.0.join("match.h2film");
    fs::write(&named, b"not sealed").unwrap();
    let stale = dir.0.join(file_name(start - Duration::from_secs(60)));
    fs::write(&stale, b"not sealed").unwrap();
    let hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    fs::File::options().write(true).open(&stale).unwrap().set_modified(hours_ago).unwrap();
    fs::File::options().write(true).open(&named).unwrap().set_modified(hours_ago).unwrap();
    // A default film another running session is still writing.
    let running = dir.0.join(file_name(start + Duration::from_secs(10)));
    fs::write(&running, b"being written").unwrap();

    // Room for one more: keep 3 means the two newest unsaved stay.
    let removed = prune(&dir.0, 3, SystemTime::now()).unwrap();
    assert!(removed.contains(&stale));
    assert!(removed.contains(&sealed[1]) && removed.contains(&sealed[2]));
    assert_eq!(removed.len(), 3, "{removed:?}");
    for kept in [&sealed[0], &sealed[3], &sealed[4], &named, &running] {
        assert!(kept.exists(), "{} was pruned", kept.display());
    }
    assert!(sealed[0].with_extension("h2film.saved").exists());
    // A second pass has nothing more to do.
    assert!(prune(&dir.0, 3, SystemTime::now()).unwrap().is_empty());
}

#[test]
fn a_missing_library_prunes_nothing() {
    let dir = Temp::new("missing");
    assert!(prune(&dir.0.join("films"), KEEP, SystemTime::now()).unwrap().is_empty());
}

#[test]
fn only_a_player_sessions_default_recording_keeps_its_player_features() {
    let dir = Temp::new("facts");
    let auto = local(&dir, &dir.0.join("auto.h2film"), true);
    assert!(auto.player_features() && !auto.plays_back());
    let named = local(&dir, &dir.0.join("named.h2film"), false);
    assert!(!named.player_features());
    // A dedicated host's recording never does, whatever it is told.
    let host = Session::open(
        Some(dir.0.join("host.h2film")),
        None,
        Facts { player_features: true, ..host_facts() },
        b"xbe",
        &options(&dir.data()),
    )
    .unwrap()
    .unwrap();
    assert!(!host.player_features());
}

#[test]
fn a_join_or_an_editor_write_discards_only_a_default_recording() {
    let dir = Temp::new("stop");
    let auto_path = dir.0.join("auto.h2film");
    let auto = local(&dir, &auto_path, true);
    assert!(auto_path.exists());
    let stop = stopper(Some(&auto), "a Live session link was joined").expect("a default recording");
    stop();
    assert!(!auto_path.exists(), "the default film is discarded");
    // The session goes on: later taps and the teardown do nothing.
    stop();
    auto.finish(10, true).unwrap();
    assert!(!auto_path.exists());

    let named_path = dir.0.join("named.h2film");
    let named = local(&dir, &named_path, false);
    assert!(stopper(Some(&named), "a Live session link was joined").is_none());
    named.stop_auto("an editor changed the game");
    assert!(named_path.exists(), "an explicit film is never discarded");
    assert!(stopper(None, "no film").is_none());
}

fn ticks(film: &Session, guest: &runtime::Guest, n: u64) {
    for _ in 0..n {
        film.checkpoint(guest);
    }
}

/// A game on the tape from tick `start` to now, ended: the next slice's
/// check cuts it, as boot's slice loop does.
fn end_game(film: &Session, start: u64, slices: u64) {
    end_game_as(film, start, slices, film::blam::Mode::Custom, "zanzibar")
}

/// [`end_game`] with the game's mode and level, as the descriptor read them.
fn end_game_as(film: &Session, start: u64, slices: u64, mode: film::blam::Mode, level: &str) {
    {
        let mut s = film.lock();
        let Mode::Record(rec) = &mut s.mode else { panic!("a recording") };
        rec.meta.start_for_test(start, level, mode);
        rec.game_started_ms = Some(1_795_000_000_000 + start);
        rec.match_over = true;
    }
    film.finish(slices, false).unwrap();
}

/// The sealed default films in `dir`, by their game's start tick.
fn cut_films(dir: &Path) -> Vec<(PathBuf, film::Reader<fs::File>)> {
    let mut films: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_auto(p))
        .filter_map(|p| film::Reader::open(&p).ok().map(|r| (p, r)))
        .collect();
    films.sort_by_key(|(_, r)| r.front.as_ref().and_then(|f| f.match_start_tick));
    films
}

#[test]
fn each_game_is_cut_into_a_film_of_its_own_while_the_tape_records_on() {
    let dir = Temp::new("cuts");
    let tape = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_000)));
    let auto = local(&dir, &tape, true);
    let tape_id = auto.lock().header.meta.id.clone();
    let guest = crate::theater::tests::arena();
    ticks(&auto, &guest, 40);
    end_game(&auto, 10, 80);
    {
        let s = auto.lock();
        let Mode::Record(rec) = &s.mode else { panic!("a recording") };
        assert!(!s.complete && !rec.match_over, "the tape records on");
        assert!(rec.meta.descriptor().match_start_tick.is_none() && rec.meta.between_games());
    }
    ticks(&auto, &guest, 30);
    end_game(&auto, 50, 140);
    // Menus after the last game.
    ticks(&auto, &guest, 5);
    assert!(film::LibraryEntry::read(&tape).is_err(), "the running tape is never listed");
    auto.finish(150, true).unwrap();
    assert!(!tape.exists(), "the tape goes at the session's end");

    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 2, "one film per game");
    let mut ids = Vec::new();
    for ((path, reader), (start, length)) in films.iter().zip([(10, 40), (50, 70)]) {
        let header = &reader.header;
        assert_eq!(header.tape_id.as_deref(), Some(tape_id.as_str()));
        assert!(header.player_features);
        assert_eq!(header.meta.length_ticks, length);
        assert_eq!(header.meta.started_ms, 1_795_000_000_000 + start);
        assert_eq!(reader.front.as_ref().unwrap().match_start_tick, Some(start));
        assert_eq!(opening_tick(header, reader.front.as_ref()), start);
        let entry = film::LibraryEntry::read(path).unwrap();
        assert_eq!(entry.meta.map, "zanzibar");
        ids.push(header.meta.id.clone());
    }
    assert!(ids[0] != ids[1] && !ids.contains(&tape_id), "each film has its own ID");
}

#[test]
fn a_cut_plays_back_with_the_recordings_player_features() {
    let dir = Temp::new("replay");
    let auto = local(&dir, &dir.0.join("auto-tape.h2film"), true);
    ticks(&auto, &crate::theater::tests::arena(), 40);
    end_game(&auto, 10, 80);
    let (cut, _) = cut_films(&dir.0).pop().expect("the game's film");
    let replay = Session::open(
        None,
        Some(crate::theater::Replay {
            verification: None,
            path: cut,
            cache: None,
            unpaced: false,
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            from_tick: None,
            seek_source: None,
            paused_at_target: false,
            presentation: crate::theater::Presentation::default(),
            restart: dir.0.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }),
        // The viewer's own facts say nothing of the player features.
        Facts { light: false, ..host_facts() },
        b"xbe",
        &options(&dir.data()),
    )
    .unwrap()
    .unwrap();
    assert!(replay.plays_back());
    assert!(replay.player_features(), "the replay plans the recording's player features");
}

#[test]
fn a_join_after_a_game_discards_the_tape_and_keeps_the_games_film() {
    let dir = Temp::new("join");
    let tape = dir.0.join("auto-tape.h2film");
    let auto = local(&dir, &tape, true);
    ticks(&auto, &crate::theater::tests::arena(), 40);
    end_game(&auto, 10, 80);
    auto.stop_auto("a Live session link was joined");
    assert!(!tape.exists());
    auto.finish(90, true).unwrap();
    assert_eq!(cut_films(&dir.0).len(), 1, "the finished game's film stays");
}

#[test]
fn a_game_left_in_play_at_exit_is_cut_at_exit() {
    let dir = Temp::new("exit");
    let tape = dir.0.join("auto-tape.h2film");
    let auto = local(&dir, &tape, true);
    ticks(&auto, &crate::theater::tests::arena(), 40);
    {
        let mut s = auto.lock();
        let Mode::Record(rec) = &mut s.mode else { panic!("a recording") };
        rec.meta.start_for_test(12, "zanzibar", film::blam::Mode::Campaign);
    }
    auto.finish(80, true).unwrap();
    assert!(!tape.exists());
    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 1);
    assert_eq!(films[0].1.header.meta.length_ticks, 40);
}

#[test]
fn each_cut_keeps_the_library_at_its_cap() {
    let dir = Temp::new("cap");
    let start = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
    let mut old = Vec::new();
    for n in 0..KEEP as u64 {
        let path = dir.0.join(file_name(start + Duration::from_secs(n)));
        local(&dir, &path, false).finish(1, true).unwrap();
        old.push(path);
    }
    let auto = local(&dir, &dir.0.join(file_name(start + Duration::from_secs(1_000))), true);
    ticks(&auto, &crate::theater::tests::arena(), 40);
    end_game(&auto, 10, 80);
    let sealed = fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_auto(p) && film::LibraryEntry::read(p).is_ok())
        .count();
    assert_eq!(sealed, KEEP);
    assert!(!old[0].exists(), "the oldest default film made room");
    assert!(old[1].exists());
}

#[test]
fn only_a_cut_opens_at_its_games_start() {
    let dir = Temp::new("open");
    let path = dir.0.join("named.h2film");
    local(&dir, &path, false).finish(1, true).unwrap();
    let reader = film::Reader::open(&path).unwrap();
    assert_eq!(opening_tick(&reader.header, reader.front.as_ref()), 0);
    let mut cut = reader.header.clone();
    cut.tape_id = Some("tape".into());
    cut.meta.length_ticks = 10;
    let mut front = reader.front.clone().unwrap();
    front.match_start_tick = Some(4);
    assert_eq!(opening_tick(&cut, Some(&front)), 4);
    // A start past the film's end opens at boot.
    front.match_start_tick = Some(11);
    assert_eq!(opening_tick(&cut, Some(&front)), 0);
}

#[test]
fn a_session_that_played_no_game_leaves_no_default_film() {
    let dir = Temp::new("menus");
    let auto_path = dir.0.join("auto.h2film");
    local(&dir, &auto_path, true).finish(5, true).unwrap();
    assert!(!auto_path.exists(), "menus alone are not listed in Recent");

    let named_path = dir.0.join("named.h2film");
    local(&dir, &named_path, false).finish(5, true).unwrap();
    assert!(film::LibraryEntry::read(&named_path).is_ok(), "an explicit film still seals");
}

#[test]
fn a_campaign_mission_ends_into_a_film_like_a_multiplayer_game() {
    let dir = Temp::new("campaign");
    let tape = dir.0.join("auto-tape.h2film");
    let auto = local(&dir, &tape, true);
    ticks(&auto, &crate::theater::tests::arena(), 40);
    end_game_as(&auto, 10, 80, film::blam::Mode::Campaign, "deltahalo");
    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 1, "the mission's film is cut at its end");
    let entry = film::LibraryEntry::read(&films[0].0).unwrap();
    assert_eq!(entry.meta.map, "deltahalo");
    assert_eq!(films[0].1.front.as_ref().unwrap().match_start_tick, Some(10));
    // Firefight is a variant of a game, not a mode of its own: the same cut.
    ticks(&auto, &crate::theater::tests::arena(), 10);
    end_game_as(&auto, 50, 100, film::blam::Mode::Custom, "guardhouse");
    assert_eq!(cut_films(&dir.0).len(), 2);
    auto.finish(110, true).unwrap();
    assert!(!tape.exists());
}

#[test]
fn a_session_that_goes_away_without_a_teardown_cuts_the_game_in_play() {
    let dir = Temp::new("seal");
    let tape = dir.0.join("auto-tape.h2film");
    let auto = local(&dir, &tape, true);
    // The boot loop holds one across its slice loop, so an error exit or a
    // panic that skips the final finish still seals the session.
    let seal = auto.seal_on_drop().expect("a default recording seals on drop");
    ticks(&auto, &crate::theater::tests::arena(), 40);
    {
        let mut s = auto.lock();
        let Mode::Record(rec) = &mut s.mode else { panic!("a recording") };
        rec.meta.start_for_test(12, "zanzibar", film::blam::Mode::Campaign);
    }
    seal.advance(79);
    drop(seal);
    assert!(!tape.exists(), "the tape goes as a teardown would");
    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 1);
    assert_eq!(films[0].1.header.meta.length_ticks, 40);

    // Only a default recording seals this way: an explicit film and a
    // replay manage their own sealing.
    assert!(
        local(&dir, &dir.0.join("named.h2film"), false)
            .seal_on_drop()
            .is_none()
    );
    let replay = Session::open(
        None,
        Some(crate::theater::Replay {
            verification: None,
            path: films[0].0.clone(),
            cache: None,
            unpaced: false,
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            from_tick: None,
            seek_source: None,
            paused_at_target: false,
            presentation: crate::theater::Presentation::default(),
            restart: dir.0.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }),
        Facts {
            light: false,
            ..host_facts()
        },
        b"xbe",
        &options(&dir.data()),
    )
    .unwrap()
    .unwrap();
    assert!(replay.seal_on_drop().is_none());
}

#[test]
fn a_campaign_mission_left_mid_session_is_cut_and_the_next_describes_itself() {
    let dir = Temp::new("left-campaign");
    let tape = dir.0.join("auto-tape.h2film");
    let auto = local(&dir, &tape, true);
    let guest = crate::theater::tests::arena();
    ticks(&auto, &guest, 40);
    // A mission starts at tick 10 and is left (Save & Exit): the guest
    // leaves its map without the retail game-over latch.
    {
        let mut s = auto.lock();
        let Mode::Record(rec) = &mut s.mode else { panic!("a recording") };
        rec.meta.start_for_test(10, "deltahalo", film::blam::Mode::Campaign);
        rec.meta.on_map_for_test();
    }
    ticks(&auto, &guest, 30);
    auto.finish(140, false).unwrap();
    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 1, "the left mission keeps its film");
    assert_eq!(films[0].1.front.as_ref().unwrap().match_start_tick, Some(10));
    {
        let s = auto.lock();
        let Mode::Record(rec) = &s.mode else { panic!("a recording") };
        assert!(
            rec.meta.descriptor().match_start_tick.is_none(),
            "the next mission describes itself"
        );
    }
    // The next mission is a film of its own, not this one's sequel.
    {
        let mut s = auto.lock();
        let Mode::Record(rec) = &mut s.mode else { panic!("a recording") };
        rec.meta.start_for_test(80, "scarab", film::blam::Mode::Campaign);
        rec.meta.on_map_for_test();
    }
    ticks(&auto, &guest, 10);
    auto.finish(160, false).unwrap();
    let films = cut_films(&dir.0);
    assert_eq!(films.len(), 2);
    let entry = film::LibraryEntry::read(&films[1].0).unwrap();
    assert_eq!(entry.meta.map, "scarab");
    auto.finish(170, true).unwrap();
    assert!(!tape.exists());
}

/// An unsealed tape as a hard kill leaves it: a boot header, the start
/// manifest, three time chunks' worth of ticks with the third still
/// pending, and a capture mark inside the second chunk.
fn dead_tape(path: &Path) {
    use film::blam::{Tick, TrackWriter};
    use film::tape::{Call, Check, Mark};
    let header = film::Header {
        version: 3,
        meta: film::FilmMeta {
            id: "dead".into(),
            map: String::new(),
            variant: String::new(),
            started_ms: 1,
            length_ticks: 0,
            players: vec![],
            source: film::Source::Local,
            image_stamp: "image".into(),
        },
        title_id: 2,
        xbe_sha256: "xbe".into(),
        runtime_stamp: String::new(),
        simulation_hooks: None,
        guest_image: None,
        epoch_100ns: 0,
        lease_id: None,
        game_id: None,
        xnet_addr: "127.0.0.1:3074".into(),
        transfers_per_slice: 5000,
        lobby: false,
        lobby_machine_start: false,
        host_bots: false,
        telemetry: false,
        telemetry_hooks: None,
        render_hooks: None,
        live_service: false,
        features: String::new(),
        player_features: true,
        tape_id: None,
    };
    let mut tape = film::Writer::new(fs::File::create(path).unwrap(), &header).unwrap();
    tape.chunk(film::DATA, &film::manifest::Manifest::default()).unwrap();
    let mut track = TrackWriter::default();
    for n in 1..=300u64 {
        track
            .push(
                &mut tape,
                Tick {
                    tick: n,
                    inputs: vec![],
                    events: vec![],
                    closing_events: vec![],
                    post_hash: None,
                    forge: vec![],
                },
            )
            .unwrap();
        if n.is_multiple_of(30) {
            tape.chunk(
                film::HASH,
                &Check {
                    tick: n,
                    sim: Some(n),
                    calls: 0,
                },
            )
            .unwrap();
        }
        if n == 200 {
            tape.chunk(
                film::NETR,
                &vec![Call {
                    index: 0,
                    op: "Recv".into(),
                    thread: 7,
                    args: vec![],
                    result: Ok(1),
                    outputs: vec![],
                }],
            )
            .unwrap();
            tape.chunk(
                film::MARK,
                &Mark::D3dBarrier {
                    slices: 400,
                    tick: 200,
                    saved: true,
                },
            )
            .unwrap();
        }
    }
}

#[test]
fn the_next_session_recovers_tapes_a_stopped_one_left() {
    let dir = Temp::new("recover");
    let dead = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_000)));
    dead_tape(&dead);
    // A tape a running session is still writing every few seconds: recent,
    // so never touched.
    let running = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_100)));
    dead_tape(&running);
    // A session that died before any game captured: nothing to keep.
    let menus = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_200)));
    fs::write(&menus, [0u8; 512]).unwrap();
    // A sealed film stays as it is.
    let sealed_path = dir.0.join("auto-sealed.h2film");
    local(&dir, &sealed_path, false).finish(1, true).unwrap();

    let hours_ago = SystemTime::now() - ABANDONED_AFTER - Duration::from_secs(60);
    for path in [&dead, &menus] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(hours_ago)
            .unwrap();
    }

    let recovered = recover(&dir.0, SystemTime::now());
    assert_eq!(recovered, vec![dead.clone()]);
    let entry = film::LibraryEntry::read(&dead).unwrap();
    assert_eq!(entry.meta.length_ticks, 200, "sealed to its last capture mark");
    assert_eq!(entry.meta.map, "", "no descriptor survives a dead session");
    assert!(!menus.exists(), "a tape with no game is deleted, not listed");
    assert!(
        running.exists() && film::LibraryEntry::read(&running).is_err(),
        "a live tape is untouched"
    );
    assert!(film::LibraryEntry::read(&sealed_path).is_ok());
}

#[test]
fn an_unsealed_tape_a_stopped_session_left_is_never_listed() {
    let dir = Temp::new("unsealed");
    let tape = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_000)));
    // A session a hard kill caught mid-game: the tape holds ticks but was
    // never sealed, cut or discarded.
    fs::write(&tape, [0u8; 4096]).unwrap();
    assert!(
        film::LibraryEntry::read(&tape).is_err(),
        "the unsealed tape is never listed"
    );
    let library = film::Library::open(&dir.0).unwrap();
    assert!(library.list().unwrap().is_empty(), "Recent shows only sealed films");
    // The prune is what removes it, after the abandoned interval.
    assert!(
        prune(&dir.0, KEEP, SystemTime::now() + ABANDONED_AFTER)
            .unwrap()
            .contains(&tape)
    );
}
