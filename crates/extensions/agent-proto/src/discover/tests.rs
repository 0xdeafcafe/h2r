use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use super::*;
use crate::Response;
use serde_json::{Value, json};

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("h2d-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_private_dir(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A listener that answers every request with `session.status` for `map`
/// (or, with `mute`, accepts and never answers).
fn fake_game(path: &Path, map: Option<&str>, mute: bool) -> UnixListener {
    let l = UnixListener::bind(path).unwrap();
    let l2 = l.try_clone().unwrap();
    let status = json!({
        "at": {"ticks": 5, "map_ticks": 5, "present": 1, "map": map, "phase": "gameplay",
               "paused": false, "boundary": "tick"},
        "protocol": 1, "controller": false, "claimed": false,
    });
    std::thread::spawn(move || {
        for s in l2.incoming().flatten() {
            let status = status.clone();
            std::thread::spawn(move || {
                let mut out = s.try_clone().unwrap();
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    if mute {
                        continue;
                    }
                    let id = serde_json::from_str::<Value>(&line).unwrap()["id"].clone();
                    let r = serde_json::to_string(&Response::ok(id, status.clone())).unwrap();
                    let _ = writeln!(out, "{r}");
                }
            });
        }
    });
    l
}

#[test]
fn a_games_default_socket_is_named_for_its_process() {
    let d = Path::new("/c/h2recomp/agents");
    assert_eq!(socket_in(d, 4242), d.join("4242.sock"));
    assert_eq!(pid_of(&socket_in(d, 4242)), Some(4242));
    assert_eq!(pid_of(Path::new("/c/agent.sock")), None);
    assert_eq!(pid_of(Path::new("/c/editor-1-0.sock")), None);
    assert!(dir().ends_with("h2recomp/agents"));
    assert_eq!(process_socket(), socket_in(&dir(), std::process::id()));
}

#[test]
fn off_values_switch_the_default_socket_off() {
    for v in ["off", "OFF", "0", " none ", "false", "no"] {
        assert!(is_off(v), "{v}");
    }
    for v in ["", "/tmp/a.sock", "tcp://0.0.0.0:4720", "on", "1"] {
        assert!(!is_off(v), "{v}");
    }
}

#[test]
fn the_directory_is_private_to_the_user() {
    let t = Temp::new("mode");
    let d = t.0.join("a/b");
    create_private_dir(&d).unwrap();
    let mode = std::fs::metadata(&d).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn a_scan_finds_live_games_with_their_maps() {
    let t = Temp::new("live");
    let _a = fake_game(&socket_in(&t.0, 11), Some("ascension"), false);
    let _b = fake_game(&socket_in(&t.0, 12), None, false);
    // A file that is not a socket, and a socket not named for a pid.
    std::fs::write(t.0.join("notes.sock"), "x").unwrap();
    let _c = fake_game(&t.0.join("custom.sock"), Some("zanzibar"), false);
    let found = scan(&t.0, Duration::from_secs(5));
    let seen: Vec<(Option<u32>, Option<&str>)> = found.iter().map(|f| (f.pid, f.map())).collect();
    assert_eq!(
        seen,
        [(Some(11), Some("ascension")), (Some(12), None), (None, Some("zanzibar"))]
    );
    assert!(found.iter().all(|f| f.status.is_some() && f.note.is_none()));
    assert!(t.0.join("notes.sock").exists(), "not a socket: left alone");
}

#[test]
fn a_socket_nothing_answers_on_is_removed() {
    let t = Temp::new("stale");
    let gone = socket_in(&t.0, 99);
    // The game died: its listener is closed but the file stays.
    drop(UnixListener::bind(&gone).unwrap());
    assert!(gone.exists());
    let _live = fake_game(&socket_in(&t.0, 100), Some("example"), false);
    let found = scan(&t.0, Duration::from_secs(5));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, Some(100));
    assert!(!gone.exists(), "the leftover is cleaned up");
    assert!(socket_in(&t.0, 100).exists());
}

#[test]
fn a_listener_that_does_not_answer_is_kept_and_noted() {
    let t = Temp::new("mute");
    let p = socket_in(&t.0, 7);
    let _l = fake_game(&p, None, true);
    let found = scan(&t.0, Duration::from_millis(150));
    assert_eq!(found.len(), 1);
    assert!(found[0].status.is_none());
    assert!(found[0].note.is_some());
    assert!(p.exists());
}

#[test]
fn a_missing_directory_has_no_games() {
    assert!(scan(Path::new("/nonexistent/h2/agents"), PROBE_TIMEOUT).is_empty());
}

#[test]
fn the_open_map_and_the_games_map_match_by_name() {
    use MapMatch::*;
    assert_eq!(MapMatch::of(Some("ascension"), Some("ascension")), Same);
    assert_eq!(MapMatch::of(Some("Ascension"), Some("ascension")), Same);
    assert_eq!(MapMatch::of(Some("maps/ascension.map"), Some("ascension")), Same);
    assert_eq!(MapMatch::of(Some("ascension"), Some("zanzibar")), Different);
    assert_eq!(MapMatch::of(None, Some("ascension")), Unknown);
    assert_eq!(MapMatch::of(Some("ascension"), None), Unknown);
    assert_eq!(MapMatch::of(Some(""), Some("")), Unknown);
}
