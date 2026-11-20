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
    assert!(!o.no_drive);
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

#[test]
fn wait_clang_runs_clangs_build_and_settings_come_from_the_play_data() {
    let o = opts(&["test"]);
    assert!(!o.wait_clang);
    assert_eq!(o.settings_from, content::store::default_data_root());
    let g = plan(&o, "http://s", Path::new("/r"), Path::new("/a.sock"));
    assert!(!g.args.iter().any(|a| a == "--tier"));

    let o = opts(&[
        "test",
        "--wait-clang",
        "--settings-from",
        "/d",
        "--",
        "--fps",
        "60",
    ]);
    assert!(o.wait_clang);
    assert_eq!(o.settings_from, PathBuf::from("/d"));
    let g = plan(&o, "http://s", Path::new("/r"), Path::new("/a.sock"));
    let a = g.args.join(" ");
    assert!(a.ends_with("--tier clang --fps 60"), "{a}");
    assert!(err(&["test", "--settings-from"]).contains("--settings-from needs a value"));
}

#[test]
fn only_the_graphics_section_is_copied() {
    let text = "[graphics]\nfps = 120\nrender_scale = 2\n\n[live]\nvisibility = \"friends\"\n\n[clankers]\nenabled = true\n";
    let (out, n) = graphics_only(text).unwrap().unwrap();
    assert_eq!(n, 2);
    let back: toml::Table = out.parse().unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back["graphics"]["fps"].as_integer(), Some(120));
    assert_eq!(back["graphics"]["render_scale"].as_integer(), Some(2));
    assert_eq!(graphics_only("[live]\nvisibility = 1\n").unwrap(), None);
    assert_eq!(graphics_only("").unwrap(), None);
    assert!(graphics_only("[graphics\n").is_err());
}

#[test]
fn a_background_build_is_running_while_its_log_changes() {
    use std::time::{Duration, SystemTime};
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
    assert!(still_writing(now - Duration::from_secs(5), now));
    assert!(still_writing(now - Duration::from_secs(119), now));
    assert!(!still_writing(now - Duration::from_secs(121), now));
    assert!(
        still_writing(now + Duration::from_secs(1), now),
        "a clock skew"
    );
}

#[test]
fn no_drive_defaults_to_a_long_timeout() {
    let o = opts(&["test", "--no-drive"]);
    assert!(o.no_drive);
    assert_eq!(o.timeout, Duration::from_secs(900));
    // Either order, an explicit --timeout wins.
    let o = opts(&["test", "--timeout", "120", "--no-drive"]);
    assert_eq!(o.timeout, Duration::from_secs(120));
    let o = opts(&["test", "--no-drive", "--timeout", "1200"]);
    assert_eq!(o.timeout, Duration::from_secs(1200));
    // A driven run keeps its default.
    assert_eq!(
        opts(&["test", "--timeout", "120"]).timeout,
        Duration::from_secs(120)
    );
}

#[test]
fn usage_names_no_drive_and_progress() {
    let u = err(&["test", "--help"]);
    assert!(u.contains("--no-drive") && u.contains("progress.json") && u.contains("900"));
}

#[test]
fn clankermatch_is_asked_for_by_flag_or_playlist() {
    let o = opts(&["test"]);
    assert_eq!(o.clankermatch, None);
    let o = opts(&["test", "--clankermatch"]);
    assert_eq!(o.clankermatch, Some(CmPlaylist::Slayer));
    assert_eq!(o.timeout, Duration::from_secs(900));
    let o = opts(&["test", "--playlist", "teams", "--keep"]);
    assert_eq!(o.clankermatch, Some(CmPlaylist::Teams));
    assert!(o.keep);
    let o = opts(&["test", "--playlist", "Doubles", "--clankermatch"]);
    assert_eq!(
        o.clankermatch,
        Some(CmPlaylist::Doubles),
        "the flag keeps it"
    );
    let o = opts(&["test", "--clankermatch", "--no-drive"]);
    assert_eq!(o.timeout, Duration::from_secs(1200));
    let o = opts(&["test", "--clankermatch", "--timeout", "600"]);
    assert_eq!(o.timeout, Duration::from_secs(600));
    assert!(err(&["test", "--playlist", "swat"]).contains("slayer, teams or doubles"));
    assert!(err(&["test", "--clankermatch", "--count", "3"]).contains("--count and --map"));
    assert!(
        err(&["test", "--playlist", "slayer", "--map", "lockout"]).contains("--count and --map")
    );
    assert!(err(&["test", "--help"]).contains("--clankermatch"));
}

#[test]
fn clankermatch_leaves_clankers_off() {
    let o = opts(&["test", "--clankermatch", "--wait-clang"]);
    let g = plan(&o, "http://s", Path::new("/r"), Path::new("/a.sock"));
    let a = g.args.join(" ");
    assert!(a.contains("--set clankers.enabled=false"), "{a}");
    assert!(!a.contains("clankers.count"), "{a}");
    assert!(a.contains("Halo 2 — clankermatch test"), "{a}");
    assert!(a.ends_with("--tier clang"), "{a}");
    for w in g.args.windows(2).filter(|w| w[0] == "--set") {
        crate::settings_cmd::set_flag(&w[1]).unwrap();
    }
}

#[test]
fn the_clankermatch_plans_come_from_the_built_in_lineup() {
    let s = cm_plan(CmPlaylist::Slayer, "Tester").unwrap();
    assert_eq!(
        (s.title.as_str(), s.hopper, s.row, s.min, s.max, s.teams),
        ("Clanker Slayer", 8, 0, 2, 8, 0)
    );
    assert_eq!((s.bots, s.phantoms()), (7, 7), "phantoms only");
    let t = cm_plan(CmPlaylist::Teams, "Tester").unwrap();
    assert_eq!(
        (t.title.as_str(), t.hopper, t.row, t.max, t.teams),
        ("Clanker Teams", 9, 1, 8, 2)
    );
    assert_eq!((t.bots, t.phantoms()), (7, 7));
    let d = cm_plan(CmPlaylist::Doubles, "Tester").unwrap();
    assert_eq!(
        (d.title.as_str(), d.hopper, d.row, d.max, d.teams),
        ("Clanker Doubles", 10, 2, 4, 2)
    );
    assert_eq!((d.bots, d.phantoms()), (3, 3));
    for p in [&s, &t, &d] {
        assert!((2..=p.max).contains(&p.min), "{p:?}");
        // The plan line reads back.
        assert_eq!(CmPlan::read(&[p.line().as_str()]).as_ref(), Some(p));
    }

    // A fresh server's marks (the built-in lineup's) agree; a server
    // without the hopper, or with another lineup, stops the run.
    use live_proto::hopper::{HOPPERS, HopperFile, clankers::ClankerHoppers};
    let file = HopperFile::default_set();
    let served: Vec<_> = (0..HOPPERS)
        .filter_map(|i| file.descriptor(i).ok())
        .collect();
    let marks = ClankerHoppers::of_defaults(&served);
    for p in [&s, &t, &d] {
        check_marks(p, &marks).unwrap();
    }
    let e = check_marks(&s, &ClankerHoppers::default()).unwrap_err();
    assert!(format!("{e:#}").contains("not 8 (Clanker Slayer)"), "{e:#}");
    let only_teams = ClankerHoppers {
        hoppers: marks
            .hoppers
            .iter()
            .filter(|h| h.hopper_id == 9)
            .copied()
            .collect(),
    };
    let e = check_marks(&t, &only_teams).unwrap_err();
    assert!(format!("{e:#}").contains("row 0"), "{e:#}");
}

#[test]
fn a_clankermatch_game_line_has_its_kind_and_playlist() {
    let page = serde_json::json!({"items": [{
        "game_id": "g1",
        "map": "lockout",
        "kind": "clankermatch",
        "playlist": "Clanker Slayer",
        "players": [
            {"index": 0, "name": "Tester", "xuid": "0009000000000001", "team": 0},
            {"index": 1, "name": "Rusty Lugnut", "xuid": "000B000000000001", "team": 1, "bot": true}
        ]
    }]});
    assert_eq!(
        cm_game_from(&page).unwrap(),
        "bots-test: game g1 on lockout: 2 players, 1 bots (1 with an account), kind \
         clankermatch, playlist Clanker Slayer"
    );
}
