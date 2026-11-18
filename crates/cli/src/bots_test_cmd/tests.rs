use super::*;

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|a| a.to_string()).collect()
}

fn env(k: &str) -> Option<String> {
    match k {
        "TMPDIR" => Some("/tmp/t".into()),
        "H2_DISC" => Some("/discs/halo2.iso".into()),
        _ => None,
    }
}

fn opts(s: &[&str]) -> Opts {
    match parse(args(s), env).unwrap() {
        Cmd::Test(o) => o,
        c => panic!("{c:?}"),
    }
}

fn err(s: &[&str]) -> String {
    format!("{:#}", parse(args(s), env).unwrap_err())
}

#[test]
fn defaults() {
    let o = opts(&["test"]);
    assert_eq!(o.disc, PathBuf::from("/discs/halo2.iso"));
    assert_eq!(o.count, 5);
    assert_eq!(o.map, None);
    assert!(!o.keep && !o.dry_run);
    assert_eq!(o.timeout, Duration::from_secs(360));
    assert_eq!(o.root, PathBuf::from("/tmp/t/h2-bots-test"));
    assert_eq!(o.out, None);
    assert_eq!(o.server_port, 8077);
    assert!(o.play_args.is_empty());
}

#[test]
fn flags() {
    let o = opts(&[
        "test",
        "--disc",
        "/d.iso",
        "--count",
        "2",
        "--map",
        "lockout",
        "--keep",
        "--timeout",
        "600",
        "--root",
        "/r",
        "--out",
        "/o",
        "--server-port",
        "9000",
        "--dry-run",
        "--",
        "--fps",
        "60",
    ]);
    assert_eq!(o.disc, PathBuf::from("/d.iso"));
    assert_eq!(o.count, 2);
    assert_eq!(o.map.as_deref(), Some("lockout"));
    assert!(o.keep && o.dry_run);
    assert_eq!(o.timeout, Duration::from_secs(600));
    assert_eq!(o.root, PathBuf::from("/r"));
    assert_eq!(o.out, Some(PathBuf::from("/o")));
    assert_eq!(o.server_port, 9000);
    assert_eq!(o.play_args, ["--fps", "60"]);
}

#[test]
fn bad_arguments_say_why() {
    assert!(err(&["test", "--count", "0"]).contains("--count 0: 1 to 15"));
    assert!(err(&["test", "--count", "16"]).contains("1 to 15"));
    assert!(err(&["test", "--count"]).contains("--count needs a value"));
    assert!(err(&["test", "--timeout", "0"]).contains("--timeout 0"));
    assert!(err(&["test", "--server-port", "x"]).contains("--server-port x"));
    assert!(err(&["test", "--map", "two words"]).contains("--map"));
    assert!(err(&["test", "--bogus"]).contains("unexpected \"--bogus\""));
    assert!(err(&["nav"]).contains("unknown `h2 bots nav`"));
    assert!(err(&["test", "--help"]).contains("usage:"));
    assert!(err(&["verdict"]).contains("verdict takes a bots.log"));
    assert!(err(&["verdict", "a.log", "b.log"]).contains("unexpected \"b.log\""));
    let no_disc = parse(args(&["test"]), |_| None).unwrap_err();
    assert!(format!("{no_disc:#}").contains("no disc"));
}

#[test]
fn verdict_takes_a_log() {
    assert_eq!(
        parse(args(&["verdict", "/l/bots.log"]), env).unwrap(),
        Cmd::Verdict(PathBuf::from("/l/bots.log"))
    );
}

#[test]
fn the_game_turns_clankers_on_for_the_run() {
    let o = opts(&["test", "--count", "4", "--out", "/o", "--", "--fps", "60"]);
    let g = plan(
        &o,
        "http://127.0.0.1:8077",
        Path::new("/r/run-1"),
        Path::new("/s/a.sock"),
    );
    assert_eq!(g.tag, "Tester");
    assert!(g.host);
    assert_eq!(g.data, PathBuf::from("/r/run-1/data"));
    assert_eq!(g.live, PathBuf::from("/r/run-1/live"));
    let a = g.args.join(" ");
    assert!(
        a.starts_with("play /discs/halo2.iso --data /r/run-1/data"),
        "{a}"
    );
    assert!(
        a.contains("--live-server http://127.0.0.1:8077 --gamertag Tester"),
        "{a}"
    );
    assert!(
        a.contains("--set clankers.enabled=true --set clankers.count=4"),
        "{a}"
    );
    assert!(a.contains("--no-mappack --no-halo1"), "{a}");
    assert!(a.ends_with("--out /o --fps 60"), "{a}");
    let env: Vec<(&str, &str)> = g
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert!(env.contains(&("H2_LIVE_DIR", "/r/run-1/live")));
    assert!(env.contains(&("H2_AGENT", "/s/a.sock")));
    assert!(env.contains(&("H2_UI_WATCH", "1")));
    // Each --set is one the settings accept.
    for w in g.args.windows(2).filter(|w| w[0] == "--set") {
        crate::settings_cmd::set_flag(&w[1]).unwrap();
    }
}

#[test]
fn status_session_and_game_lines_from_their_answers() {
    let v = serde_json::json!({"at": {"phase": "gameplay", "mode": "multiplayer", "map": "lockout", "ticks": 3}});
    assert_eq!(
        read_status(&v),
        ("gameplay".into(), "multiplayer".into(), "lockout".into())
    );
    let v = serde_json::json!({"at": {"phase": "menu"}});
    assert_eq!(read_status(&v), ("menu".into(), "-".into(), "-".into()));

    let page = serde_json::json!({"items": [{
        "session_id": "s-1",
        "advertisement": {"slots": {"public_filled": 4, "public_open": 4, "private_filled": 1, "private_open": 0}},
        "players": {"0009000000000002": "1"}
    }], "next": null, "position": 3});
    assert_eq!(
        session_from(&page).unwrap(),
        "bots-test: session s-1: 5 filled (public 4, private 1), 1 joined"
    );
    assert_eq!(session_from(&serde_json::json!({"items": []})), None);

    let page = serde_json::json!({"items": [{
        "game_id": "g1",
        "map": "lockout",
        "players": [
            {"index": 0, "name": "Tester", "xuid": "0009000000000001", "team": 0},
            {"index": 1, "name": "Rusty Lugnut", "xuid": "000B000000000001", "team": 1, "bot": true},
            {"index": 2, "name": "Clanker 4", "team": 1, "bot": true}
        ]
    }]});
    assert_eq!(
        game_from(&page).unwrap(),
        "bots-test: game g1 on lockout: 3 players, 2 bots (1 with an account)"
    );
}
