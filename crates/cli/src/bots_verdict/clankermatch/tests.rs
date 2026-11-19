use super::*;
use crate::bots_verdict::verdict as any_verdict;
use crate::coop_verdict::format_line;

fn slayer() -> CmPlan {
    CmPlan {
        playlist: "slayer".into(),
        title: "Clanker Slayer".into(),
        hopper: 8,
        row: 0,
        min: 2,
        max: 8,
        teams: 0,
        bots: 7,
        local: 3,
        tag: "Tester".into(),
    }
}

/// A merged log of `lines`, one a second, after the plan line.
fn log_of(plan: &CmPlan, lines: &[&str]) -> String {
    let mut s = "# host Tester\n".to_string();
    s.push_str(&format_line(0.0, "Tester", &plan.line()));
    s.push('\n');
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&format_line(i as f64 + 1.0, "Tester", l));
        s.push('\n');
    }
    s
}

fn run(plan: &CmPlan, lines: &[&str]) -> Verdict {
    any_verdict(&log_of(plan, lines)).expect("a plan line")
}

/// A whole Clanker Slayer run as the game, the drive and the harness say
/// it (the formats are the code's: `live_hle::state` and `sessions`,
/// `boot::clankermatch`, `boot::clankers`, `boot::bot_glue`,
/// `bots::phantom`).
fn whole_run() -> Vec<&'static str> {
    vec![
        "bots-test: server clanker hoppers [8, 9, 10]",
        "agent: listening on /tmp/h2-bots-test-1.sock",
        "bots-test: status phase menu mode - map mainmenu",
        "live: playing as Tester (0009000000000001) on server abc",
        "live: clankermatch: clanker hoppers [8, 9, 10]",
        "ui: screen 0xba (window 5, slot 4), live menu row 0",
        "bots-drive: press down on screen 0xba row 0 (toward Clankermatch (row 2, from 0))",
        "bots-drive: press down on screen 0xba row 1 (toward Clankermatch (row 2, from 1))",
        "bots-drive: press a on screen 0xba row 2 (Clankermatch)",
        "clankermatch: the playlists are the clanker hoppers",
        "ui: screen none (slot 4: window 5 active 0x82540000 pending 0x0 list [0x1/0xe3000000]; slot 0: no window)",
        "bots-drive: press a on screen none (the clanker playlist on row 0: search)",
        "live: clankermatch: searching clanker playlist 8: no one to find; the console hosts in 7.3 s",
        "coop-live: lobby mode 6",
        "live: session 0123456789abcdef advertised for 127.0.0.1:1000",
        "clankers: clankermatch: hosting clanker playlist 8 (session 0123456789abcdef): 7 bots join over 12.0 s",
        "clankers: clankermatch: 3 local bots join the lobby on free controller ports; 4 phantoms join the lobby as peers (\"bots: phantom ... joins\")",
        "clankers: asking Live for 7 bots for session 0123456789abcdef",
        "clankers: Live rostered bots for session 0123456789abcdef",
        "clankers: Clanker 1 on (Normal)",
        "clankers: Clanker 1 is Rusty Lugnut on Xbox Live (bot account 000b000000000001)",
        "bots: join gate port 1: screen lobby (top none), dialog none, signed_in false (slot 0x1): signs in",
        "bots: signing Clanker 1 in on port 1 in the pregame lobby",
        "bots: Rusty Lugnut on port 1 is on Xbox Live as its own bot account, beside port 0's",
        "clankers: Clanker 2 on (Normal)",
        "clankers: Clanker 2 is Bent Sprocket on Xbox Live (bot account 000b000000000002)",
        "bots: join gate port 2: screen lobby (top none), dialog none, signed_in false (slot 0x1): signs in",
        "bots: signing Clanker 2 in on port 2 in the pregame lobby",
        "bots: Bent Sprocket on port 2 is on Xbox Live as its own bot account, beside port 0's",
        "clankers: Clanker 3 on (Normal)",
        "clankers: Clanker 3 is Loose Bolt on Xbox Live (bot account 000b000000000003)",
        "bots: join gate port 3: screen lobby (top none), dialog none, signed_in false (slot 0x1): signs in",
        "bots: signing Clanker 3 in on port 3 in the pregame lobby",
        "bots: Loose Bolt on port 3 is on Xbox Live as its own bot account, beside port 0's",
        "bots-test: session 0123456789abcdef: 4 filled (public 4, private 0), 0 joined",
        "clankers: Tin Can (phantom, rostered) on (Normal)",
        "bots: phantom Tin Can joins (peer 1, in the pregame lobby)",
        "bots: phantom Oil Slick joins (peer 2, in the pregame lobby)",
        "bots: phantom Spare Gear joins (peer 3, in the pregame lobby)",
        "bots: phantom Hex Nut joins (peer 4, in the pregame lobby)",
        "bots-test: status phase loading mode multiplayer map lockout",
        "bots-test: status phase gameplay mode multiplayer map lockout",
        "bots-test: session 0123456789abcdef: 8 filled (public 8, private 0), 0 joined",
        "bots-test: game g1 on lockout: 8 players, 7 bots (7 with an account), kind clankermatch, playlist Clanker Slayer",
        "bots-test: status phase menu mode - map mainmenu",
        "bots-drive: press b on screen none (back out of the Clankermatch lobby)",
        "live: session 0123456789abcdef deleted",
        "clankers: clankermatch: the game is over: the bots go",
        "clankers: Hex Nut off",
        "clankers: Clanker 3 off",
        "clankers: Clanker 2 off",
        "clankers: Clanker 1 off",
    ]
}

fn without(lines: &[&'static str], drop: impl Fn(&str) -> bool) -> Vec<&'static str> {
    lines.iter().copied().filter(|l| !drop(l)).collect()
}

/// The first failure's step and the rendered verdict.
fn first(v: &Verdict) -> (usize, String) {
    let r = v.render();
    (v.first_failure().map_or(0, |(n, _)| n), r)
}

#[test]
fn the_plan_line_round_trips() {
    let p = slayer();
    assert_eq!(p.phantoms(), 4);
    assert_eq!(
        p.line(),
        "bots-test: clankermatch plan: playlist slayer (Clanker Slayer), hopper 8, row 0, \
         players 2 to 8, teams 0, bots 7 (3 local, 4 phantoms), gamertag Tester"
    );
    assert_eq!(CmPlan::read(&[p.line().as_str()]), Some(p));
    // The custom game's plan line is not this one, and back.
    let custom = Plan {
        count: 5,
        tag: "Tester".into(),
        map: None,
    };
    assert_eq!(CmPlan::read(&[custom.line().as_str()]), None);
    assert_eq!(Plan::read(&[slayer().line().as_str()]), None);
}

#[test]
fn the_lines_are_read() {
    assert_eq!(read_marks("[8, 9, 10]"), Some(vec![8, 9, 10]));
    assert_eq!(read_marks("[]"), Some(vec![]));
    assert_eq!(read_marks("8, 9"), None);
    assert_eq!(
        hosting(
            "clankers: clankermatch: hosting clanker playlist 9 (session ab12): 7 bots join over 9.0 s, 2 teams, the host on team 0"
        ),
        Some((9, "ab12"))
    );
    assert_eq!(
        local_planned(
            "clankers: clankermatch: 1 local bots join the lobby on free controller ports; 2 phantoms join the lobby as peers"
        ),
        Some(1)
    );
    assert_eq!(
        local_planned("clankers: clankermatch: hosting clanker playlist 8 (session a): 7"),
        None
    );
    assert_eq!(seated_port("clankers: Clanker 2 on (Hard)"), Some(2));
    assert_eq!(seated_port("clankers: Clanker 2 off"), None);
    assert_eq!(off_port("clankers: Clanker 2 off"), Some(2));
    assert_eq!(off_port("clankers: Hex Nut off"), None);
    assert_eq!(
        advertised_id("live: session 0123 advertised for 127.0.0.1:1000"),
        Some("0123")
    );
    let g = game_line("g1", "lockout", (8, 7, 7), "clankermatch", "Clanker Slayer");
    assert_eq!(
        g,
        "bots-test: game g1 on lockout: 8 players, 7 bots (7 with an account), kind \
         clankermatch, playlist Clanker Slayer"
    );
    assert_eq!(read_game(&g), Some((7, "clankermatch", "Clanker Slayer")));
}

#[test]
fn the_filter_keeps_the_clankermatch_lines() {
    for keep in [
        "clankermatch: the playlists are the clanker hoppers",
        "live: clankermatch: searching clanker playlist 8: no one to find; the console hosts in 4.0 s",
        "live: optimatch: searching playlist 2 for a party of 1, mean level 1, wide",
        "clankers: clankermatch: the game is over: the bots go",
    ] {
        assert!(crate::bots_verdict::wanted(keep), "{keep}");
    }
}

#[test]
fn a_whole_run_passes() {
    let v = run(&slayer(), &whole_run());
    assert!(v.passed(), "{}", v.render());
    assert_eq!(v.checks.len(), 14);
    let r = v.render();
    assert!(
        r.starts_with("bots test: Clankermatch, Clanker Slayer (hopper 8): 7 bots (3 local, 4 phantoms) with Tester\n"),
        "{r}"
    );
    assert!(r.ends_with("verdict: PASS\n"), "{r}");
    assert!(
        r.contains("port 1 in the lobby, port 2 in the lobby, port 3 in the lobby"),
        "{r}"
    );
    assert!(
        r.contains("4 players in the lobby (the playlist starts at 2)"),
        "{r}"
    );
    assert!(
        r.contains("4 joined in the lobby: Hex Nut, Oil Slick, Spare Gear, Tin Can"),
        "{r}"
    );
    assert!(r.contains("Clanker Slayer: 7 bots of the 7 planned"), "{r}");
    assert!(r.contains("the fill ended; 3 local bots off"), "{r}");
}

#[test]
fn no_clanker_hoppers_from_the_server_fails_step_2_saying_why() {
    let mut lines = without(&whole_run(), |l| {
        l.starts_with("live: clankermatch: clanker hoppers")
    });
    lines.insert(
        4,
        "live: clankermatch: no clanker hoppers from the server (0x80150001)",
    );
    let v = run(&slayer(), &lines);
    let (n, r) = first(&v);
    assert_eq!(n, 2, "{r}");
    assert!(
        r.contains("no clanker hoppers from the server (0x80150001)"),
        "{r}"
    );
    // The server's own marks without the hopper: its file is not the
    // built-in lineup.
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| {
            if l.starts_with(SERVER_MARKS) {
                "bots-test: server clanker hoppers [9, 10]"
            } else {
                l
            }
        })
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 2, "{r}");
    assert!(r.contains("lack hopper 8"), "{r}");
}

#[test]
fn an_empty_list_or_no_row_fails_step_3() {
    let mut lines = whole_run();
    lines.insert(
        10,
        "clankermatch: the server named no clanker hoppers: the list is empty",
    );
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 3, "{r}");
    assert!(r.contains("the playlist list was empty"), "{r}");
    let lines = without(&whole_run(), |l| l.starts_with(ACTIVE));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 3, "{r}");
    assert!(r.contains("row 2 (Clankermatch) was never pressed"), "{r}");
}

#[test]
fn a_human_search_or_the_wrong_hopper_fails_step_4() {
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| {
            if l.starts_with(SEARCH) {
                "live: optimatch: searching playlist 2 for a party of 1, mean level 1, wide"
            } else {
                l
            }
        })
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 4, "{r}");
    assert!(r.contains("a human playlist was searched"), "{r}");
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| {
            if l.starts_with(SEARCH) {
                "live: clankermatch: searching clanker playlist 9: no one to find; the console hosts in 5.0 s"
            } else {
                l
            }
        })
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 4, "{r}");
    assert!(
        r.contains("searched clanker playlist Some(9), not 8"),
        "{r}"
    );
}

#[test]
fn a_search_that_never_hosts_fails_step_5_saying_what_came() {
    let lines = without(&whole_run(), |l| {
        l.starts_with(HOSTING) || l.starts_with("live: session")
    });
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 5, "{r}");
    assert!(r.contains("no session was advertised after it"), "{r}");
    // Advertised, but the fill never saw a clanker hopper's session.
    let lines = without(&whole_run(), |l| l.starts_with(HOSTING));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 5, "{r}");
    assert!(
        r.contains("did not read the session's hopper as a clanker one"),
        "{r}"
    );
}

#[test]
fn a_bot_not_seated_or_not_signed_in_fails_step_7() {
    let lines = without(&whole_run(), |l| l.starts_with("clankers: Clanker 3 on"));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 7, "{r}");
    assert!(
        r.contains("no `clankers: Clanker P on` for port(s) [3]"),
        "{r}"
    );
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| {
            if l.starts_with("bots: join gate port 2") {
                "bots: join gate port 2: screen none (top none), dialog none, signed_in false (slot 0x0): waits"
            } else {
                l
            }
        })
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 7, "{r}");
    assert!(r.contains("port(s) [2] seated but never signed in"), "{r}");
    assert!(r.contains("signed_in false (slot 0x0): waits"), "{r}");
}

#[test]
fn a_lobby_short_of_players_fails_step_9_and_10() {
    let lines: Vec<&str> = without(&whole_run(), |l| {
        l.starts_with("bots-test: status phase loading")
            || l.starts_with("bots-test: status phase gameplay")
    })
    .into_iter()
    .map(|l| {
        if l.starts_with("bots-test: session") {
            "bots-test: session 0123456789abcdef: 1 filled (public 1, private 0), 0 joined"
        } else {
            l
        }
    })
    .collect();
    let plan = CmPlan { min: 4, ..slayer() };
    let v = run(&plan, &lines);
    let (n, r) = first(&v);
    assert_eq!(n, 9, "{r}");
    assert!(r.contains("at most 1 players in the lobby of the 4"), "{r}");
    assert!(!v.checks[9].pass);
    assert!(
        v.checks[9].detail.contains("short of its fewest players"),
        "{r}"
    );
}

#[test]
fn a_full_lobby_that_never_starts_says_so() {
    let lines = without(&whole_run(), |l| {
        l.starts_with("bots-test: status phase loading")
            || l.starts_with("bots-test: status phase gameplay")
    });
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 10, "{r}");
    assert!(r.contains("the game's matchmaking start"), "{r}");
}

#[test]
fn a_phantom_only_in_the_game_fails_step_12() {
    // Tin Can joined once the game ran, not in the lobby.
    let mut lines = without(&whole_run(), |l| {
        l.starts_with("bots: phantom Tin Can joins")
    });
    let at = lines
        .iter()
        .position(|l| l.contains("phase gameplay"))
        .unwrap();
    lines.insert(at, "bots: phantom Tin Can joins (peer 4)");
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 12, "{r}");
    assert!(
        r.contains("3 of 4 joined the lobby; 1 joined only once the game ran"),
        "{r}"
    );
    // One missing altogether.
    let lines = without(&whole_run(), |l| l.starts_with("bots: phantom Hex Nut"));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 12, "{r}");
    assert!(r.contains("3 of 4 joined the lobby (`"), "{r}");
}

#[test]
fn a_record_of_another_kind_fails_step_13() {
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .map(|l| {
            if l.starts_with("bots-test: game ") {
                "bots-test: game g1 on lockout: 8 players, 7 bots (7 with an account), kind matchmaking, playlist Clanker Slayer"
            } else {
                l
            }
        })
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 13, "{r}");
    assert!(r.contains("recorded as \"matchmaking\""), "{r}");
}

#[test]
fn bots_that_stay_fail_step_14() {
    let lines = without(&whole_run(), |l| BOTS_GO.contains(&l));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 14, "{r}");
    assert!(r.contains("still hosts the clanker session"), "{r}");
    let lines = without(&whole_run(), |l| l == "clankers: Clanker 2 off");
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 14, "{r}");
    assert!(r.contains("only 2 of 3 local bots"), "{r}");
}

#[test]
fn progress_reads_a_clankermatch_log() {
    let text = log_of(&slayer(), &whole_run()[..12]);
    let json = crate::bots_progress::render(&text, false).expect("a plan line");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["checks"].as_array().unwrap().len(), 14);
    assert_eq!(v["checks"][0]["state"], "pass");
    assert_eq!(v["checks"][3]["state"], "pending");
}
