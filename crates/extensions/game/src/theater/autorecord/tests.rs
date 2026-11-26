use super::*;
use crate::level_boot::{MODE_CAMPAIGN, MODE_MULTIPLAYER};
use crate::theater::Outcome;
use crate::theater::game_start::tests::{block, game, play};
use crate::theater::tests::{arena, host_facts, options};
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

/// A player session's default recording into the library `dir`.
fn library(dir: &Temp) -> Arc<Session> {
    let facts = Facts { light: false, player_features: true, ..host_facts() };
    Session::open(Some(dir.0.clone()), None, facts, b"xbe", &options(&dir.data()))
        .unwrap()
        .unwrap()
}

/// A `--film` recording to `path`: one film, opened at boot.
fn named(dir: &Temp, path: &Path) -> Arc<Session> {
    let facts = Facts { light: false, player_features: false, ..host_facts() };
    Session::open(Some(path.to_owned()), None, facts, b"xbe", &options(&dir.data()))
        .unwrap()
        .unwrap()
}

const LAUNCH_SKIP: &str = "a lab job records a film only with H2_FILM_OUT";

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
    assert_eq!(decide(&Inputs { feature_on: false, ..inputs(Request::Skip(LAUNCH_SKIP)) }), Decision::Quiet);
}

#[test]
fn a_launch_join_and_lockstep_skip_recording_and_say_why() {
    assert_eq!(decide(&inputs(Request::Skip(LAUNCH_SKIP))), Decision::Skip(LAUNCH_SKIP));
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
    // A stats container left alone takes its film's name too.
    fs::write(film::file::stats_of(&second), b"taken").unwrap();
    let third = destination(&dir.0, at);
    assert!(third != first && third != second, "{}", third.display());
}

/// A sealed film at `path`, film file and stats container, as any
/// recording seals.
fn sealed_film(dir: &Temp, path: &Path) {
    named(dir, path).finish(true).unwrap();
    assert!(path.is_file() && film::file::stats_of(path).is_file());
}

#[test]
fn pruning_keeps_the_newest_default_films_and_every_saved_or_explicit_one() {
    let dir = Temp::new("prune");
    let start = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
    let mut sealed = Vec::new();
    for n in 0..5 {
        let path = dir.0.join(file_name(start + Duration::from_secs(n)));
        sealed_film(&dir, &path);
        sealed.push(path);
    }
    // The oldest is saved: never pruned.
    fs::write(sealed[0].with_extension("h2film.saved"), b"").unwrap();
    // A film the player named, and a stale default film a crash left as
    // bytes that do not read.
    let named = dir.0.join("match.h2film");
    fs::write(&named, b"not sealed").unwrap();
    let stale = dir.0.join(file_name(start - Duration::from_secs(60)));
    fs::write(&stale, b"not sealed").unwrap();
    fs::write(film::file::stats_of(&stale), b"not sealed").unwrap();
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
    for gone in &removed {
        assert!(!gone.exists() && !film::file::stats_of(gone).exists(), "{} stays", gone.display());
    }
    for kept in [&sealed[0], &sealed[3], &sealed[4]] {
        assert!(kept.exists() && film::file::stats_of(kept).exists(), "{} was pruned", kept.display());
    }
    assert!(named.exists() && running.exists());
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
    let auto = library(&dir);
    assert!(auto.player_features() && !auto.plays_back());
    let named = named(&dir, &dir.0.join("named.h2film"));
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

/// The default films in `dir`: film files with their stats containers.
fn films(dir: &Path) -> Vec<PathBuf> {
    let mut films: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_auto(p) && film::file::stats_of(p).is_file())
        .collect();
    films.sort();
    films
}

/// End the game in play as the observer would, named `level` in `mode`:
/// the next slice's check seals its film, as boot's slice loop does.
fn end_game(rec: &Session, level: &str, mode: film::blam::Mode) {
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(1, level, mode);
        r.match_over = true;
    }
    rec.finish(false).unwrap();
}

fn menus(rec: &Session, guest: &runtime::Guest, n: u64) {
    for _ in 0..n {
        rec.checkpoint(guest);
    }
}

#[test]
fn three_games_leave_three_films_and_the_menus_between_them_none() {
    let dir = Temp::new("three");
    let rec = library(&dir);
    let guest = arena();
    menus(&rec, &guest, 10);
    assert!(fs::read_dir(&dir.0).unwrap().all(|e| e.unwrap().path() == dir.data()));
    let games = [("zanzibar", MODE_MULTIPLAYER, 30), ("deltahalo", MODE_CAMPAIGN, 20), ("lockout", MODE_MULTIPLAYER, 40)];
    for (n, (level, mode, ticks)) in games.into_iter().enumerate() {
        assert_eq!(play(&rec, &guest, &block(mode, n as u8 + 1), [n as u32, 7], ticks), 1);
        let status = rec.status();
        assert!(status.recording.is_some_and(|p| is_auto(Path::new(&p))));
        let film_mode = if mode == MODE_CAMPAIGN { film::blam::Mode::Campaign } else { film::blam::Mode::Custom };
        end_game(&rec, level, film_mode);
        assert!(rec.status().recording.is_none(), "the film is sealed at the game's end");
        menus(&rec, &guest, 5);
    }
    rec.finish(true).unwrap();
    assert!(rec.lock().outcome.reported);

    let films = films(&dir.0);
    assert_eq!(films.len(), 3, "one film for each game");
    let status = rec.status();
    assert_eq!(status.films.len(), 3);
    let mut ids = Vec::new();
    let mut rows: Vec<_> = films.iter().map(|p| film::LibraryEntry::read(p).unwrap()).collect();
    rows.sort_by_key(|row| row.meta.length_ticks);
    for (row, (level, length)) in rows.iter().zip([("deltahalo", 20), ("zanzibar", 30), ("lockout", 40)]) {
        assert_eq!((row.meta.map.as_str(), row.meta.length_ticks), (level, length));
        assert!(row.summary.is_some(), "a sealed stats container adds its scoreboard");
        let reader = film::file::FilmReader::open(fs::File::open(&row.path).unwrap()).unwrap();
        assert!(reader.sealed);
        assert_eq!(reader.game_starts.len(), 1, "a film holds its own game's start");
        let header = film::file::header_of(&row.path).unwrap();
        assert!(header.player_features);
        assert_eq!(header.meta.id, row.meta.id);
        ids.push(row.meta.id.clone());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "each film has its own ID");
}

#[test]
fn the_library_lists_a_film_from_its_header_alone() {
    let dir = Temp::new("header");
    let rec = library(&dir);
    let guest = arena();
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 1), [1, 2], 25);
    end_game(&rec, "zanzibar", film::blam::Mode::Custom);
    rec.finish(true).unwrap();
    let [path] = &films(&dir.0)[..] else { panic!("one film") };
    let with_stats = film::LibraryEntry::read(path).unwrap();
    fs::remove_file(film::file::stats_of(path)).unwrap();
    // The film file's header and game start name the row; no tick is read.
    let alone = film::LibraryEntry::read(path).unwrap();
    assert_eq!(alone.meta.id, with_stats.meta.id);
    assert_eq!(alone.meta.map, "zanzibar");
    assert_eq!(alone.meta.length_ticks, 25);
    assert!(alone.summary.is_none());
    let rows = film::Library::open(&dir.0).unwrap().list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].meta.id, alone.meta.id);
}

#[test]
fn a_session_of_menus_leaves_no_film() {
    let dir = Temp::new("menus");
    let rec = library(&dir);
    let guest = arena();
    menus(&rec, &guest, 20);
    // A game that activates and is gone before its first update has no
    // film either.
    rec.game_activated(&game(&block(MODE_MULTIPLAYER, 1), [1, 2]));
    assert!(rec.status().recording.is_some());
    menus(&rec, &guest, 5);
    rec.finish(true).unwrap();
    assert!(films(&dir.0).is_empty(), "menus alone are not listed in Recent");
    assert!(fs::read_dir(&dir.0).unwrap().all(|e| e.unwrap().path() == dir.data()));
    let status = rec.status();
    assert!(status.films.is_empty() && status.recording.is_none());
    assert!(rec.lock().outcome.reported);

    // An explicit film still seals, games or not.
    let named_path = dir.0.join("named.h2film");
    sealed_film(&dir, &named_path);
    assert!(film::LibraryEntry::read(&named_path).is_ok());
}

#[test]
fn a_hard_killed_game_opens_to_its_last_checkpoint_and_the_prune_seals_it() {
    let dir = Temp::new("killed");
    let rec = library(&dir);
    let guest = arena();
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 1), [1, 2], 200);
    // A hard kill: the files stay as the recorder last wrote them.
    let live = PathBuf::from(rec.status().recording.expect("a game in play"));
    let killed_dir = dir.0.join("killed");
    fs::create_dir(&killed_dir).unwrap();
    let killed = killed_dir.join(live.file_name().unwrap());
    fs::copy(&live, &killed).unwrap();
    fs::copy(film::file::stats_of(&live), film::file::stats_of(&killed)).unwrap();

    let reader = film::file::FilmReader::open(fs::File::open(&killed).unwrap()).unwrap();
    assert!(!reader.sealed);
    assert_eq!(reader.length_ticks, 150, "the ticks up to the last checkpoint");
    assert_eq!(reader.game_starts.len(), 1);
    let actions = film::file::FilmActions::open(&killed).unwrap();
    assert_eq!(actions.length_ticks(), 150);
    assert!(film::file::header_of(&killed).is_ok());
    let row = film::LibraryEntry::read(&killed).unwrap();
    assert_eq!(row.meta.length_ticks, 150);

    // A newer unsealed film is a recording in progress: left alone.
    assert!(prune(&killed_dir, KEEP, SystemTime::now()).unwrap().is_empty());
    assert!(!film::file::FilmReader::open(fs::File::open(&killed).unwrap()).unwrap().sealed);
    // Once idle, the prune seals it at what it holds.
    let later = SystemTime::now() + ABANDONED_AFTER;
    assert!(prune(&killed_dir, KEEP, later).unwrap().is_empty());
    let reader = film::file::FilmReader::open(fs::File::open(&killed).unwrap()).unwrap();
    assert!(reader.sealed);
    assert_eq!(reader.length_ticks, 150);
    assert_eq!(reader.game_starts.len(), 1);

    // The session that was not killed seals the whole game.
    rec.finish(true).unwrap();
    let [sealed] = &films(&dir.0)[..] else { panic!("one film") };
    assert_eq!(film::file::FilmActions::open(sealed).unwrap().length_ticks(), 200);
}

#[test]
fn a_game_in_progress_is_not_listed_and_the_prune_leaves_it_alone() {
    let dir = Temp::new("progress");
    let rec = library(&dir);
    play(&rec, &arena(), &block(MODE_MULTIPLAYER, 1), [1, 2], 200);
    let live = PathBuf::from(rec.status().recording.expect("a game in play"));
    assert!(film::file::being_written(&live));
    assert!(film::LibraryEntry::read(&live).is_err(), "the game in progress is not listed");
    assert!(film::Library::open(&dir.0).unwrap().list().unwrap().is_empty());
    // Even untouched for hours (a long pause), a held film is not sealed
    // under its recorder.
    let hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    fs::File::options().write(true).open(&live).unwrap().set_modified(hours_ago).unwrap();
    assert!(prune(&dir.0, KEEP, SystemTime::now()).unwrap().is_empty());
    assert!(!film::file::FilmReader::open(fs::File::open(&live).unwrap()).unwrap().sealed);
    // Sealed and closed, it lists.
    rec.finish(true).unwrap();
    assert!(!film::file::being_written(&live));
    assert_eq!(film::LibraryEntry::read(&live).unwrap().meta.length_ticks, 200);
    assert_eq!(film::Library::open(&dir.0).unwrap().list().unwrap().len(), 1);
}

#[test]
fn a_killed_film_with_no_game_start_is_removed_with_its_stats() {
    let dir = Temp::new("no-start");
    let path = dir.0.join(file_name(UNIX_EPOCH + Duration::from_secs(1_795_000_000)));
    // A film file with a header and nothing else, as a recording killed
    // between its game's activation and its first update leaves.
    let rec = library(&dir);
    rec.game_activated(&game(&block(MODE_MULTIPLAYER, 1), [1, 2]));
    let open = PathBuf::from(rec.status().recording.unwrap());
    fs::copy(&open, &path).unwrap();
    fs::copy(film::file::stats_of(&open), film::file::stats_of(&path)).unwrap();
    rec.finish(true).unwrap();
    assert!(!open.exists(), "a game with no first update has no film");
    let removed = prune(&dir.0, KEEP, SystemTime::now() + ABANDONED_AFTER).unwrap();
    assert_eq!(removed, [path.clone()]);
    assert!(!path.exists() && !film::file::stats_of(&path).exists());
}

/// A guest stopped inside the game's first update (the spawn gather's stop
/// on cyclotron) leaves the film open with its base set and no film tick:
/// the session's end deletes it and reports nothing failed. Sealing it
/// asked the track for records before its first tick, which it refused as
/// "tagged records require film v3".
#[test]
fn a_game_stopped_inside_its_first_update_has_no_film_and_no_failure() {
    let dir = Temp::new("first-update");
    let rec = library(&dir);
    let guest = arena();
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 1), [1, 2], 0);
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        let game = r.game.as_ref().expect("the game's film is open");
        assert!(game.base.is_some(), "the first update entered");
        assert_eq!(game.track.ticks(), 0, "and never ended");
        // What the game's observers had queued for its film by then.
        r.unified.marker("cyclotron");
    }
    let open = PathBuf::from(rec.status().recording.unwrap());
    rec.finish(true).unwrap();
    assert!(!open.exists() && !film::file::stats_of(&open).exists());
    assert!(films(&dir.0).is_empty());
    assert_eq!(rec.lock().outcome.discard, None);
}

#[test]
fn a_film_recorded_after_the_hooks_are_installed_carries_their_digest() {
    let dir = Temp::new("digest");
    let rec = library(&dir);
    // Boot installs the hooks after the session opens.
    rec.set_simulation_hooks("digest-of-the-installed-hooks".into()).unwrap();
    play(&rec, &arena(), &block(MODE_MULTIPLAYER, 1), [1, 2], 40);
    end_game(&rec, "zanzibar", film::blam::Mode::Custom);
    let [path] = &films(&dir.0)[..] else { panic!("one film") };
    let entry = film::LibraryEntry::read(path).unwrap();
    assert_eq!(entry.simulation_hooks.as_deref(), Some("digest-of-the-installed-hooks"));
    let header = film::file::header_of(path).unwrap();
    assert_eq!(header.simulation_hooks.as_deref(), Some("digest-of-the-installed-hooks"));
}

#[test]
fn a_join_or_an_editor_write_never_stops_a_default_recording_and_never_touches_another() {
    let dir = Temp::new("stop");
    let auto = library(&dir);
    let stop = stopper(Some(&auto), "a Live session link was joined", false).expect("a default recording");
    stop();
    assert_eq!(auto.status().notes, ["a Live session link was joined"]);
    assert!(auto.status().last_discard.is_none());

    let named_path = dir.0.join("named.h2film");
    let named = named(&dir, &named_path);
    assert!(stopper(Some(&named), "a Live session link was joined", false).is_none());
    named.outside_change("an editor changed the game", true);
    assert!(named_path.exists(), "an explicit film is never discarded");
    assert!(named.status().notes.is_empty());
    assert!(stopper(None, "no film", false).is_none());
}

#[test]
fn a_join_keeps_the_game_in_play_and_an_editor_write_drops_only_that_game() {
    let dir = Temp::new("join");
    let rec = library(&dir);
    let guest = arena();
    // A link joined with a game in play: the game keeps its film.
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 1), [1, 2], 20);
    rec.outside_change("a Live session link was joined", false);
    menus(&rec, &guest, 10);
    end_game(&rec, "lockout", film::blam::Mode::Matchmaking);
    assert_eq!(films(&dir.0).len(), 1);

    // An editor write with a game in play: that game's film goes.
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 2), [3, 4], 20);
    let dropped = PathBuf::from(rec.status().recording.unwrap());
    rec.outside_change("an editor changed the game", true);
    let status = rec.status();
    assert!(!status.in_game && status.recording.is_none());
    assert!(status.last_discard.is_some_and(|why| why.contains("an editor changed the game")));
    assert!(!dropped.exists() && !film::file::stats_of(&dropped).exists());
    rec.finish(true).unwrap();
    assert_eq!(films(&dir.0).len(), 1, "the finished game's film stays");
    assert_eq!(rec.status().notes, ["a Live session link was joined"]);
}

#[test]
fn a_game_left_before_its_end_is_a_film_of_what_was_played() {
    let dir = Temp::new("left");
    let rec = library(&dir);
    let guest = arena();
    // A matchmade game is left from the pause menu: the guest leaves its
    // map with no game-over.
    play(&rec, &guest, &block(MODE_MULTIPLAYER, 1), [1, 2], 10);
    {
        let mut s = rec.lock();
        let Mode::Record(r) = &mut s.mode else { panic!("a recording") };
        r.meta.start_for_test(1, "lockout", film::blam::Mode::Matchmaking);
        r.meta.on_map_for_test();
    }
    menus(&rec, &guest, 5);
    rec.finish(false).unwrap();
    let [path] = &films(&dir.0)[..] else { panic!("the left game keeps its film") };
    assert_eq!(film::LibraryEntry::read(path).unwrap().meta.map, "lockout");
    assert!(rec.status().recording.is_none());
    // The next game is a film of its own, not this one's sequel.
    play(&rec, &guest, &block(MODE_CAMPAIGN, 2), [3, 4], 10);
    end_game(&rec, "scarab", film::blam::Mode::Campaign);
    rec.finish(true).unwrap();
    assert_eq!(films(&dir.0).len(), 2);
    assert_eq!(rec.status().films.len(), 2);
}

#[test]
fn a_game_in_play_at_exit_is_sealed_at_exit() {
    let dir = Temp::new("exit");
    let rec = library(&dir);
    play(&rec, &arena(), &block(MODE_CAMPAIGN, 1), [1, 2], 40);
    rec.finish(true).unwrap();
    let [path] = &films(&dir.0)[..] else { panic!("one film") };
    assert_eq!(film::file::FilmActions::open(path).unwrap().length_ticks(), 40);
}

#[test]
fn a_session_that_goes_away_without_a_teardown_seals_the_game_in_play() {
    let dir = Temp::new("seal");
    let rec = library(&dir);
    // The boot loop holds one across its slice loop, so an error exit or a
    // panic that skips the final finish still seals the game in play.
    let seal = rec.seal_on_drop().expect("a default recording seals on drop");
    play(&rec, &arena(), &block(MODE_CAMPAIGN, 1), [1, 2], 40);
    drop(seal);
    let [path] = &films(&dir.0)[..] else { panic!("one film") };
    assert_eq!(film::file::FilmActions::open(path).unwrap().length_ticks(), 40);

    // Only a default recording seals this way: an explicit film manages
    // its own sealing, as a film player does (`film_play`).
    assert!(named(&dir, &dir.0.join("named.h2film")).seal_on_drop().is_none());
}

#[test]
fn a_failed_recording_keeps_the_game_in_play() {
    let dir = Temp::new("failed");
    let rec = library(&dir);
    play(&rec, &arena(), &block(MODE_MULTIPLAYER, 1), [1, 2], 40);
    rec.lock().fail("writing the stats stream: disk full".into());
    assert!(rec.status().failure.is_some());
    rec.finish(true).unwrap();
    let [path] = &films(&dir.0)[..] else { panic!("what was recorded is kept") };
    assert!(film::file::FilmReader::open(fs::File::open(path).unwrap()).unwrap().sealed);
}

#[test]
fn each_new_film_keeps_the_library_at_its_cap() {
    let dir = Temp::new("cap");
    let start = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
    let mut old = Vec::new();
    for n in 0..KEEP as u64 {
        let path = dir.0.join(file_name(start + Duration::from_secs(n)));
        sealed_film(&dir, &path);
        old.push(path);
    }
    let rec = library(&dir);
    play(&rec, &arena(), &block(MODE_MULTIPLAYER, 1), [1, 2], 40);
    end_game(&rec, "zanzibar", film::blam::Mode::Custom);
    let sealed = fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| is_auto(p))
        .filter(|p| film::file::FilmReader::open(fs::File::open(p).unwrap()).is_ok_and(|r| r.sealed))
        .count();
    assert_eq!(sealed, KEEP);
    assert!(!old[0].exists() && !film::file::stats_of(&old[0]).exists(), "the oldest default film made room");
    assert!(old[1].exists());
}

#[test]
fn the_end_of_the_session_says_what_was_saved_or_why_nothing_was() {
    let dir = Path::new("/films");
    let mut outcome = Outcome::default();
    assert_eq!(outcome.line(dir, None), "theater: no film: no game was played this session");
    outcome.discard = Some("could not keep this game's film: disk full".into());
    assert_eq!(
        outcome.line(dir, None),
        "theater: no film: could not keep this game's film: disk full"
    );
    outcome.discard = None;
    assert!(outcome.line(dir, Some("recording stopped at tick 9: boom")).ends_with("boom"));
    outcome.films.push("/films/a.h2film".into());
    assert_eq!(outcome.line(dir, None), "theater: 1 film saved to /films");
    outcome.films.push("/films/b.h2film".into());
    assert_eq!(outcome.line(dir, None), "theater: 2 films saved to /films");
}
