use super::*;
use crate::coop_verdict::format_line;

fn plan(count: usize) -> Plan {
    Plan {
        count,
        tag: "Tester".into(),
        map: None,
    }
}

/// A merged log of `lines`, one a second, after the plan line.
fn log_of(plan: &Plan, lines: &[&str]) -> String {
    let mut s = "# host Tester\n".to_string();
    s.push_str(&format_line(0.0, "Tester", &plan.line()));
    s.push('\n');
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&format_line(i as f64 + 1.0, "Tester", l));
        s.push('\n');
    }
    s
}

/// A whole run of count 5 as the game and the harness say it (the lines'
/// formats are the code's: `clankers` and `bots::phantom`).
fn whole_run() -> Vec<&'static str> {
    vec![
        "agent: listening on /tmp/h2-bots-test-1.sock",
        "live: playing as Tester (0009000000000001) on server abc",
        "clankers: Rusty Lugnut 1 (phantom, rostered) on (Normal)",
        "clankers: Rusty Lugnut 2 (phantom, rostered) on (Normal)",
        "clankers: Rusty Lugnut 3 (phantom, rostered) on (Normal)",
        "clankers: Tin Can (phantom, rostered) on (Normal)",
        "clankers: Oil Slick (phantom, rostered) on (Normal)",
        "bots-test: status phase menu mode main_menu map mainmenu",
        "bots-drive: press down on screen none (to Xbox Live)",
        "bots-drive: press a on screen none (Xbox Live (signs in))",
        "ui: screen 0x1e (window 5, slot 4), mode 3",
        "ui: screen 0xba (window 5, slot 4), live menu row 0",
        "bots-drive: press down on screen 0xba row 0 (toward Custom Game (row 4, from 0))",
        "bots-drive: press a on screen 0xba row 4 (Custom Game)",
        "coop-live: lobby mode 2",
        "live: session 0123456789abcdef advertised for 127.0.0.1:1000",
        "clankers: asking Live for 5 bots for session s-1",
        "clankers: Live rostered bots for session s-1",
        "bots: phantom Rusty Lugnut 1 waits: not a lobby it can join (this console does not host it (session host peer 0, this console's peer 1))",
        "bots: phantom Rusty Lugnut 1 joins (peer 1, in the pregame lobby)",
        "bots: phantom Rusty Lugnut 2 joins (peer 2, in the pregame lobby)",
        "bots: phantom Rusty Lugnut 3 joins (peer 3, in the pregame lobby)",
        "bots: phantom Tin Can joins (peer 4, in the pregame lobby)",
        "bots: phantom Oil Slick joins (peer 5, in the pregame lobby)",
        "bots-test: session s-1: 4 filled (public 4, private 0), 0 joined",
        "bots-drive: press a on screen none (Start Game: 5 phantoms in the lobby)",
        "bots-test: status phase loading mode multiplayer map lockout",
        "bots-test: status phase gameplay mode multiplayer map lockout",
        "bots-test: session s-1: 6 filled (public 6, private 0), 0 joined",
        "bots-test: game g1 on lockout: 6 players, 5 bots (5 with an account)",
    ]
}

fn without(lines: &[&'static str], drop: impl Fn(&str) -> bool) -> Vec<&'static str> {
    lines.iter().copied().filter(|l| !drop(l)).collect()
}

fn run(plan: &Plan, lines: &[&str]) -> Verdict {
    verdict(&log_of(plan, lines)).expect("a plan line")
}

#[test]
fn the_plan_line_round_trips() {
    let p = plan(5);
    assert_eq!(p.phantoms(), 5);
    assert_eq!(
        p.line(),
        "bots-test: plan: count 5 (5 phantoms), gamertag Tester, map any"
    );
    assert_eq!(Plan::read(&[p.line().as_str()]), Some(p));
    let p = Plan {
        count: 2,
        tag: "Tester".into(),
        map: Some("lockout".into()),
    };
    assert_eq!(p.phantoms(), 2);
    assert_eq!(
        p.line(),
        "bots-test: plan: count 2 (2 phantoms), gamertag Tester, map lockout"
    );
    assert_eq!(Plan::read(&[p.line().as_str()]), Some(p));
    assert_eq!(Plan::read(&["bots: phantom Tin Can joins (peer 1)"]), None);
    assert!(verdict("# host Tester\n").is_none());
}

#[test]
fn the_filter_keeps_the_bots_lines() {
    for keep in [
        "bots: phantom Tin Can joins (peer 1, in the pregame lobby)",
        "clankers: asking Live for 5 bots for session s",
        "live: playing as Tester",
        "live: session 01 advertised for 1.2.3.4:5",
        "coop-live: lobby mode 2",
        "ui: screen 0xba (window 5, slot 4), live menu row 0",
        "thread 'main' panicked at x",
        "app: guest thread exited",
    ] {
        assert!(wanted(keep), "{keep}");
    }
    for drop in ["present 100: last 100 at 30.0 fps", "d3d: texture 0x1"] {
        assert!(!wanted(drop), "{drop}");
    }
}

#[test]
fn a_whole_run_passes() {
    let v = run(&plan(5), &whole_run());
    assert!(v.passed(), "{}", v.render());
    assert_eq!(v.checks.len(), 9);
    assert!(v.first_failure().is_none());
    let r = v.render();
    assert!(r.ends_with("verdict: PASS\n"), "{r}");
    assert!(
        r.contains("5 joined in the pregame lobby: Oil Slick, Rusty Lugnut 1, Rusty Lugnut 2, \
                    Rusty Lugnut 3, Tin Can"),
        "{r}"
    );
    assert!(r.contains("6 filled, 0 joined (needs 6)"), "{r}");
    assert!(r.contains("on lockout"), "{r}");
}

#[test]
fn menus_that_never_reached_the_lobby_show_the_last_presses() {
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .take_while(|l| !l.starts_with("coop-live: lobby mode"))
        .collect();
    let mut lines = lines;
    lines.push("bots-drive: stuck: Custom Game never opened a lobby");
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 4, "{}", v.render());
    assert!(
        c.lines
            .iter()
            .any(|l| l.contains("press a on screen 0xba row 4"))
    );
    assert!(c.lines.iter().any(|l| l.contains("ui: screen 0xba")));
    assert!(v.trouble.iter().any(|l| l.contains("stuck")));
}

#[test]
fn no_roster_says_whether_it_was_asked() {
    let mut lines = without(&whole_run(), |l| l.starts_with("clankers: Live rostered"));
    let at = lines
        .iter()
        .position(|l| l.starts_with("clankers: asking Live"))
        .unwrap()
        + 1;
    lines.insert(
        at,
        "clankers: no roster from Live (HTTP 500): host-made bots",
    );
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 5);
    assert!(c.detail.starts_with("asked, but"), "{}", c.detail);
    assert!(c.lines.iter().any(|l| l.contains("no roster from Live")));
    let lines = without(&whole_run(), |l| l.starts_with("clankers: asking Live"));
    let v = run(&plan(5), &lines);
    assert!(v.checks[4].detail.starts_with("never asked"));
}

#[test]
fn the_wrong_map_fails_with_the_map_played() {
    let p = Plan {
        map: Some("midship".into()),
        ..plan(5)
    };
    let v = run(&p, &whole_run());
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 6);
    assert!(
        c.detail.contains("played lockout, not midship"),
        "{}",
        c.detail
    );
    let p = Plan {
        map: Some("lockout".into()),
        ..plan(5)
    };
    assert!(run(&p, &whole_run()).passed());
}

#[test]
fn phantoms_that_wait_or_fail_are_named() {
    let lines = without(&whole_run(), |l| l.contains("Oil Slick joins"));
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 7);
    assert!(
        c.detail.starts_with("4 of 5 joined the pregame lobby (`"),
        "{}",
        c.detail
    );
    assert!(c.lines.iter().any(|l| l.contains("Rusty Lugnut 1 waits")));
    // Joined, but only once the game ran: the lobby join did not happen.
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| match l.strip_suffix(", in the pregame lobby)") {
            Some(_) if l.contains("Oil Slick") => "bots: phantom Oil Slick joins (peer 5)",
            _ => l,
        })
        .collect();
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 7);
    assert!(
        c.detail
            .contains("4 of 5 joined the pregame lobby, the rest only once the game ran"),
        "{}",
        c.detail
    );
    let mut lines = whole_run();
    lines.push("clankers: Oil Slick (phantom, rostered): the phantoms are full");
    let v = run(&plan(5), &lines);
    assert_eq!(v.first_failure().unwrap().0, 7);
    assert_eq!(v.checks[6].detail, "a phantom failed");
}

#[test]
fn a_small_count_needs_its_own_phantoms() {
    let lines = [
        "live: playing as Tester (0009000000000001) on server abc",
        "clankers: Tin Can (phantom, rostered) on (Normal)",
        "clankers: Oil Slick (phantom, rostered) on (Normal)",
    ];
    let v = run(&plan(3), &lines);
    assert!(!v.passed(), "{}", v.render());
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 2, "{}", v.render());
    assert!(
        c.detail.contains("only 2 of 3"),
        "the run seated two of the three planned phantoms: {}",
        c.detail
    );
}

#[test]
fn players_the_host_reported_joining_count_for_the_session() {
    // Retail Halo 2 fills no slots (20261119-005001: 0 filled): the
    // players the host reported joining count instead.
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| match l {
            "bots-test: session s-1: 4 filled (public 4, private 0), 0 joined" => {
                "bots-test: session s-1: 0 filled (public 0, private 0), 4 joined"
            }
            "bots-test: session s-1: 6 filled (public 6, private 0), 0 joined" => {
                "bots-test: session s-1: 0 filled (public 0, private 0), 6 joined"
            }
            l => l,
        })
        .collect();
    let v = run(&plan(5), &lines);
    assert!(v.passed(), "{}", v.render());
    assert_eq!(v.checks[7].detail, "0 filled, 6 joined (needs 6)");
    let lines: Vec<&str> = lines
        .into_iter()
        .filter(|l| !l.contains("6 joined"))
        .collect();
    let v = run(&plan(5), &lines);
    assert_eq!(v.first_failure().unwrap().0, 8);
    assert!(
        v.checks[7]
            .detail
            .starts_with("at most 0 filled and 4 joined of the 6"),
        "{}",
        v.checks[7].detail
    );
}

#[test]
fn the_session_count_and_the_record_must_reach_the_count() {
    let lines = without(&whole_run(), |l| {
        l.contains("6 filled") || l.starts_with("bots-test: game")
    });
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 8);
    assert!(
        c.detail
            .starts_with("at most 4 filled and 0 joined of the 6"),
        "{}",
        c.detail
    );
    assert!(v.checks[8].detail.contains("no game record"));
    let mut lines = without(&whole_run(), |l| l.starts_with("bots-test: game"));
    lines.push("bots-test: game g1 on lockout: 4 players, 3 bots (3 with an account)");
    let v = run(&plan(5), &lines);
    assert_eq!(v.first_failure().unwrap().0, 9);
    assert!(
        v.checks[8]
            .detail
            .starts_with("3 bots on the record of the 5")
    );
}

#[test]
fn a_game_that_never_signed_in_fails_first() {
    let v = run(
        &plan(5),
        &[
            "agent: listening on /tmp/x.sock",
            "bots-test: timed out after 360 s",
        ],
    );
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 1);
    assert!(c.detail.contains("live: playing as Tester"));
    assert!(v.trouble.iter().any(|l| l.contains("timed out")));
    assert!(v.checks.iter().all(|c| !c.pass));
}

#[test]
fn the_line_readers() {
    assert_eq!(
        signing_port("bots: signing Rusty Lugnut in on port 3 in the pregame lobby"),
        Some(3)
    );
    assert_eq!(
        phantom_joined("bots: phantom Clanker 4 joins (peer 1)"),
        Some("Clanker 4")
    );
    assert_eq!(
        phantom_joined("bots: phantom Clanker 4 joins (peer 1, in the pregame lobby)"),
        Some("Clanker 4")
    );
    assert_eq!(
        phantom_joined_lobby("bots: phantom Clanker 4 joins (peer 1, in the pregame lobby)"),
        Some("Clanker 4")
    );
    assert_eq!(
        phantom_joined_lobby("bots: phantom Clanker 4 joins (peer 1)"),
        None
    );
    assert!(!phantom_failed(
        "clankers: Tin Can (phantom, rostered) on (Normal)"
    ));
    assert!(phantom_failed("clankers: Tin Can (phantom): no room"));
    assert_eq!(
        read_status(&status_line("gameplay", "multiplayer", "lockout")),
        Some(("gameplay", "multiplayer", "lockout"))
    );
    assert_eq!(read_session(&session_line("s-1", 3, 1, 0)), Some((4, 0)));
    assert_eq!(read_session(&session_line("s-1", 0, 0, 6)), Some((0, 6)));
    assert_eq!(read_game(&game_line("g", "lockout", 6, 5, 4)), Some((5, 4)));
}

#[test]
fn a_run_that_ended_in_split_screen_is_not_on_xbox_live() {
    // The first real run (20261118-232735): the players' screen in Split
    // Screen, a custom game lobby there, no Live session.
    let lines = [
        "live: playing as Tester (000994cdf4a025e1) on http://127.0.0.1:8077",
        "clankers: Clanker 1 (phantom) on (Normal)",
        "clankers: Clanker 2 (phantom) on (Normal)",
        "clankers: Clanker 3 (phantom) on (Normal)",
        "bots-drive: stuck: no Xbox Live menu after 4 tries (on none)",
        "ui: screen 0x1e (window 5, slot 4), mode 1",
        "coop-live: lobby mode 2",
        "bots-test: status phase gameplay mode multiplayer map cyclotron",
    ];
    let v = run(&plan(3), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 3, "{}", v.render());
    assert_eq!(c.detail, "not on Xbox Live: ended in Split Screen");
    assert!(c.lines[0].contains("mode 1"));
    assert!(!v.checks[3].pass, "lobby mode 2 is not a Live lobby");
    assert!(
        v.checks[3]
            .detail
            .starts_with("a custom game lobby opened in Split Screen"),
        "{}",
        v.checks[3].detail
    );
}

#[test]
fn a_lobby_with_no_advertisement_is_not_hosting() {
    let lines = without(&whole_run(), |l| l.starts_with("live: session "));
    let v = run(&plan(5), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 4, "{}", v.render());
    assert!(
        c.detail.contains("advertised no Live session"),
        "{}",
        c.detail
    );
    assert!(c.lines[0].contains("lobby mode 2"));
}

#[test]
fn split_screen_then_xbox_live_is_on_xbox_live() {
    // Backed out of Split Screen and on to the Live menu.
    let mut lines = whole_run();
    lines.insert(10, "ui: screen 0x1e (window 5, slot 4), mode 1");
    assert!(run(&plan(5), &lines).passed());
}

#[test]
fn an_account_picker_that_never_signed_in_is_named() {
    // Run 2 (20261119-001305): the account picker opened on each Xbox Live
    // press, took A on its no-account row and closed, signed out.
    let lines = [
        "live: playing as Tester (00097660034d6ee9) on http://127.0.0.1:8077",
        "clankers: Clanker 1 (phantom) on (Normal)",
        "clankers: Clanker 2 (phantom) on (Normal)",
        "clankers: Clanker 3 (phantom) on (Normal)",
        "bots-drive: press a on screen none main menu row 1 (Xbox Live (signs in))",
        "ui: screen none (slot 4: window 3, slot 4), dialog 0x14 dialog_live_signin (window 3, slot 4)",
        "bots-drive: press a on screen none dialog 0x14 (an account row)",
        "ui: screen none (slot 4: window 3, slot 4), dialog none",
        "bots-drive: press a on screen none main menu row 1 (Xbox Live (signs in))",
        "ui: screen none (slot 4: window 3, slot 4), dialog 0x14 dialog_live_signin (window 3, slot 4)",
        "bots-drive: stuck: no Xbox Live menu after 4 tries; back on the main menu",
    ];
    let v = run(&plan(3), &lines);
    let (n, c) = v.first_failure().unwrap();
    assert_eq!(n, 3, "{}", v.render());
    assert!(
        c.detail.contains("account picker (dialog 0x14") && c.detail.contains("opened 2 time(s)"),
        "{}",
        c.detail
    );
}

#[test]
fn an_advertisement_from_another_lobby_is_not_hosting_the_custom_game() {
    // Run 3 (20261119-003044): OptiMatch's lobby (mode 3) advertised a
    // session, deleted it, and advertised another, all off Custom Game.
    let lines = [
        "live: playing as Tester (00097660034d6ee9) on http://127.0.0.1:8077",
        "clankers: Clanker 1 (phantom) on (Normal)",
        "clankers: Clanker 2 (phantom) on (Normal)",
        "clankers: Clanker 3 (phantom) on (Normal)",
        "ui: screen 0x1e (window 5, slot 4), mode 3",
        "coop-live: lobby mode 3",
        "live: session 84098833205b4d5e advertised for 10.0.3.7:3074",
        "live: session 84098833205b4d5e deleted",
        "live: session 801787df07153bc8 advertised for 10.0.3.7:3074",
    ];
    let v = run(&plan(5), &lines);
    let c = &v.checks[3];
    assert!(!c.pass, "{}", v.render());
    assert!(c.detail.contains("lobby mode 3"), "{}", c.detail);
    assert!(c.lines[0].contains("lobby mode 3") && c.lines[1].contains("84098833205b4d5e"));
}

#[test]
fn the_custom_game_lobby_hosts_in_either_order_but_not_after_a_delete() {
    let base = [
        "live: playing as Tester (00097660034d6ee9) on http://127.0.0.1:8077",
        "clankers: Clanker 1 (phantom) on (Normal)",
        "clankers: Clanker 2 (phantom) on (Normal)",
        "clankers: Clanker 3 (phantom) on (Normal)",
    ];
    let with = |more: &[&'static str]| {
        let mut v: Vec<&str> = base.to_vec();
        v.extend_from_slice(more);
        run(&plan(5), &v)
    };
    let v = with(&[
        "live: session 0123456789abcdef advertised for 127.0.0.1:1000",
        "coop-live: lobby mode 2",
    ]);
    assert!(v.checks[3].pass, "{}", v.render());
    let v = with(&[
        "live: session 0123456789abcdef advertised for 127.0.0.1:1000",
        "live: session 0123456789abcdef deleted",
        "coop-live: lobby mode 2",
    ]);
    assert!(!v.checks[3].pass, "{}", v.render());
}
