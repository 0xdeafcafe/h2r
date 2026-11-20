use super::*;
use crate::bots_verdict::verdict as any_verdict;
use crate::coop_verdict::format_line;

/// The fill's seven phantom names, in peer order.
const PHANTOMS: [&str; 7] = [
    "Rusty Lugnut",
    "Bent Sprocket",
    "Loose Bolt",
    "Tin Can",
    "Oil Slick",
    "Spare Gear",
    "Hex Nut",
];

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
/// `boot::clankermatch`, `boot::clankers`, `bots::phantom`). Every bot is
/// a phantom; none signs in on a controller port.
fn whole_run() -> Vec<&'static str> {
    let mut out = vec![
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
        "clankers: clankermatch: 7 phantoms join the lobby as peers (\"bots: phantom ... joins\"), in the match only",
        "clankers: asking Live for 7 bots for session 0123456789abcdef",
        "clankers: Live rostered bots for session 0123456789abcdef",
    ];
    for (i, name) in PHANTOMS.iter().enumerate() {
        out.push(Box::leak(
            format!("clankers: {name} (phantom, rostered) on (Normal)").into_boxed_str(),
        ));
        out.push(Box::leak(
            format!("bots: phantom {name} joins (peer {}, in the pregame lobby)", i + 1)
                .into_boxed_str(),
        ));
    }
    out.extend([
        "bots: lobby players: 8 in the game's session",
        "bots-test: status phase loading mode multiplayer map lockout",
        "bots-test: status phase gameplay mode multiplayer map lockout",
        "bots-test: session 0123456789abcdef: 8 filled (public 8, private 0), 0 joined",
        "bots-test: game g1 on lockout: 8 players, 7 bots (7 with an account), kind clankermatch, playlist Clanker Slayer",
        "bots-test: status phase menu mode - map mainmenu",
        "bots-drive: press b on screen none (back out of the Clankermatch lobby)",
        "live: session 0123456789abcdef deleted",
        "clankers: clankermatch: the game is over: the bots go",
    ]);
    for name in PHANTOMS.iter().rev() {
        out.push(Box::leak(format!("clankers: {name} off").into_boxed_str()));
    }
    out
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
    assert_eq!(p.phantoms(), 7);
    assert_eq!(
        p.line(),
        "bots-test: clankermatch plan: playlist slayer (Clanker Slayer), hopper 8, row 0, \
         players 2 to 8, teams 0, bots 7 (7 phantoms), gamertag Tester"
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
        phantoms_planned("clankers: clankermatch: 7 phantoms join the lobby as peers"),
        Some(7)
    );
    assert_eq!(
        phantoms_planned("clankers: clankermatch: hosting clanker playlist 8 (session a): 7"),
        None
    );
    assert_eq!(phantom_off("clankers: Hex Nut off"), Some("Hex Nut"));
    assert_eq!(phantom_off("clankers: Hex Nut (phantom) on (Normal)"), None);
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
        r.starts_with("bots test: Clankermatch, Clanker Slayer (hopper 8): 7 bots (7 phantoms) with Tester\n"),
        "{r}"
    );
    assert!(r.ends_with("verdict: PASS\n"), "{r}");
    assert!(r.contains("no bot in the party"), "{r}");
    assert!(r.contains("7 rostered phantoms"), "{r}");
    assert!(
        r.contains("8 players in the game's lobby (the playlist starts at 2)"),
        "{r}"
    );
    assert!(
        r.contains("7 joined in the lobby: Bent Sprocket, Hex Nut, Loose Bolt, Oil Slick, Rusty Lugnut, Spare Gear, Tin Can"),
        "{r}"
    );
    assert!(r.contains("Clanker Slayer: 7 bots of the 7 planned"), "{r}");
    assert!(r.contains("the fill ended; 7 phantoms off"), "{r}");
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
fn a_picker_answered_signed_out_fails_step_2_naming_it() {
    // Run 20261119-115144: the picker opened at boot and on each Xbox
    // Live press, and was closed on its no-account row every time. The
    // game's own line names it; a cursor move is the same opening.
    let picker = "ui: screen none (slot 4: window 3 active 0x8253b0b0 pending 0x0 list [0x1/0xe1670000]; slot 0: no window), dialog 0x14 dialog_live_signin (window 3, slot 4)";
    let picker_row_1 = "ui: screen none (slot 4: window 3 active 0x8253b0b0 pending 0x0 list [0x1/0xe1680001]; slot 0: no window), dialog 0x14 dialog_live_signin (window 3, slot 4)";
    let main = "ui: screen none (slot 4: window 5 active 0x82539070 pending 0x0 list [0x1/0xe16e0001]; slot 0: no window)";
    let lines = [
        "bots-test: server clanker hoppers [8, 9, 10]",
        "live: playing as Tester (0009f57cc2a65878) on http://127.0.0.1:8077",
        picker,
        picker_row_1,
        main,
        "bots-drive: press a on screen none main menu row 1 (Xbox Live (signs in))",
        picker,
        main,
        "bots-drive: stuck: no Xbox Live menu after 4 tries; back on the main menu",
    ];
    let v = run(&slayer(), &lines);
    let (n, r) = first(&v);
    assert_eq!(n, 2, "{r}");
    assert!(
        r.contains("account picker (dialog 0x14") && r.contains("opened 2 time(s)"),
        "{r}"
    );
    assert!(
        r.contains("bots-drive: stuck"),
        "the drive's last words: {r}"
    );
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
fn a_lobby_short_of_players_fails_step_9_and_10() {
    let lines: Vec<&str> = without(&whole_run(), |l| {
        l.starts_with("bots-test: status phase loading")
            || l.starts_with("bots-test: status phase gameplay")
    })
    .into_iter()
    .map(|l| {
        if l.starts_with("bots: lobby players") {
            "bots: lobby players: 1 in the game's session"
        } else {
            l
        }
    })
    .collect();
    let plan = CmPlan { min: 4, ..slayer() };
    let v = run(&plan, &lines);
    let (n, r) = first(&v);
    assert_eq!(n, 9, "{r}");
    assert!(
        r.contains("at most 1 players in the game's lobby of the 4"),
        "{r}"
    );
    assert!(!v.checks[9].pass);
    assert!(
        v.checks[9].detail.contains("short of its fewest players"),
        "{r}"
    );
}

/// cm-run2: the Live server listed 4 joined (the host's report), the
/// game's lobby never had a bot, and the "loading" that came was the main
/// menu's. Steps 9 and 10 passed on those; now neither does.
#[test]
fn the_servers_count_and_the_main_menus_loading_are_no_start() {
    let lines: Vec<&str> = without(&whole_run(), |l| {
        l.starts_with("bots: lobby players")
            || l.starts_with("bots-test: status phase loading")
            || l.starts_with("bots-test: status phase gameplay")
    })
    .into_iter()
    .flat_map(|l| {
        if l.starts_with("bots: phantom Hex Nut joins") {
            vec![
                l,
                "bots-test: session 0123456789abcdef: 1 filled (public 1, private 0), 4 joined",
                "bots-test: status phase loading mode main_menu map mainmenu",
                "bots-test: status phase menu mode main_menu map mainmenu",
            ]
        } else {
            vec![l]
        }
    })
    .collect();
    let v = run(&slayer(), &lines);
    assert!(!v.checks[8].pass, "{}", v.checks[8].detail);
    assert!(
        v.checks[8].detail.contains("no `bots: lobby players: N`"),
        "{}",
        v.checks[8].detail
    );
    assert!(!v.checks[9].pass, "{}", v.checks[9].detail);
    assert!(
        v.checks[9].detail.contains("no multiplayer map loaded"),
        "{}",
        v.checks[9].detail
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
        r.contains("6 of 7 joined the lobby; 1 joined only once the game ran"),
        "{r}"
    );
    // One missing altogether.
    let lines = without(&whole_run(), |l| l.starts_with("bots: phantom Hex Nut"));
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 12, "{r}");
    assert!(r.contains("6 of 7 joined the lobby (`"), "{r}");
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
    let lines = without(&whole_run(), |l| l == "clankers: Hex Nut off");
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 14, "{r}");
    assert!(r.contains("only 6 of 7 phantoms"), "{r}");
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

/// The user's run on f5d3010c: a bot on a controller port showed in the
/// party. A signing line fails step 7.
#[test]
fn a_bot_signed_in_on_a_port_fails_step_7() {
    let mut lines = whole_run();
    let at = lines
        .iter()
        .position(|l| l.starts_with("bots: phantom "))
        .unwrap();
    lines.insert(
        at,
        "bots: signing Clanker 1 in on port 1 in the pregame lobby",
    );
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 7, "{r}");
    assert!(r.contains("it shows in the party"), "{r}");
}

#[test]
fn host_made_phantoms_fail_step_8() {
    let lines: Vec<&str> = whole_run()
        .into_iter()
        .filter(|l| !l.contains("(phantom, rostered)"))
        .collect();
    let (n, r) = first(&run(&slayer(), &lines));
    assert_eq!(n, 8, "{r}");
    assert!(r.contains("0 of 7 phantoms rostered"), "{r}");
}
