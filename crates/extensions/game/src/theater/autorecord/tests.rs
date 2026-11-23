use super::*;
use crate::theater::tests::{host_facts, options};
use crate::theater::{Facts, Session};

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

#[test]
fn a_sealed_default_film_survives_a_later_join() {
    let dir = Temp::new("sealed");
    let path = dir.0.join("auto.h2film");
    let auto = local(&dir, &path, true);
    // The first game's end sealed it; a join afterwards leaves it alone.
    auto.lock().complete = true;
    auto.stop_auto("a Live session link was joined");
    assert!(path.exists());
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
