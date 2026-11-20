use super::*;
use std::collections::{BTreeMap, HashMap};

/// A sparse byte map standing in for guest memory.
#[derive(Default)]
struct Fake(BTreeMap<u32, u8>);
impl Gm for Fake {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.0.get(&(addr + i as u32)) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k| m.get(k).cloned()
}

#[test]
fn the_probe_is_off_unless_asked_for() {
    for v in [
        None,
        Some(""),
        Some("0"),
        Some("off"),
        Some("FALSE"),
        Some(" no "),
    ] {
        let e = match v {
            Some(v) => env(&[
                ("H2_COOP_PROBE", v),
                ("H2_COOP_PROBE_MAP", "03a_oldmombasa"),
            ]),
            None => env(&[("H2_COOP_PROBE_MAP", "03a_oldmombasa")]),
        };
        assert_eq!(parse(e), Ok(None), "{v:?}");
    }
    assert!(
        parse(env(&[("H2_COOP_PROBE", "maybe")]))
            .unwrap_err()
            .contains("H2_COOP_PROBE")
    );
}

#[test]
fn map_and_difficulty_have_defaults_and_are_checked() {
    assert_eq!(
        parse(env(&[("H2_COOP_PROBE", "1")])),
        Ok(Some(Config {
            map: DEFAULT_MAP.into(),
            difficulty: Difficulty::Normal
        }))
    );
    assert_eq!(
        parse(env(&[
            ("H2_COOP_PROBE", "on"),
            ("H2_COOP_PROBE_MAP", " 03a_oldmombasa.map "),
            ("H2_COOP_PROBE_DIFFICULTY", "Legendary"),
        ])),
        Ok(Some(Config {
            map: "03a_oldmombasa".into(),
            difficulty: Difficulty::Legendary
        }))
    );
    assert_eq!(
        parse(env(&[
            ("H2_COOP_PROBE", "yes"),
            ("H2_COOP_PROBE_MAP", ""),
            ("H2_COOP_PROBE_DIFFICULTY", "")
        ])),
        Ok(Some(Config {
            map: DEFAULT_MAP.into(),
            difficulty: Difficulty::Normal
        }))
    );
    let e = parse(env(&[
        ("H2_COOP_PROBE", "1"),
        ("H2_COOP_PROBE_DIFFICULTY", "mythic"),
    ]))
    .unwrap_err();
    assert!(e.contains("H2_COOP_PROBE_DIFFICULTY"), "{e}");
}

#[test]
fn only_a_campaign_level_with_ids_is_wanted() {
    let req = level_boot::Request {
        name: "03a_oldmombasa".into(),
        scenario: "scenarios\\solo\\03a_oldmombasa\\03a_oldmombasa".into(),
        campaign: 1,
        map: 5,
        difficulty: Difficulty::Heroic,
        kind: level_boot::Kind::Campaign,
    };
    assert_eq!(
        Want::from_request(&req),
        Ok(Want {
            name: "03a_oldmombasa".into(),
            campaign: 1,
            map: 5,
            difficulty: 2
        })
    );
    let mp = level_boot::Request {
        kind: level_boot::Kind::Multiplayer,
        ..req.clone()
    };
    assert!(
        Want::from_request(&mp)
            .unwrap_err()
            .contains("not a campaign level")
    );
    let unnamed = level_boot::Request {
        map: u32::MAX,
        ..req
    };
    assert!(Want::from_request(&unnamed).is_err());
}

const S: u32 = 0x0100_0000;

/// A host lobby with `peers` machines and `players` players, not campaign.
fn lobby(peers: u32, players: u32) -> Session {
    Session {
        addr: S,
        state: 4,
        peers,
        players,
        local_peer: 0,
        local_peer_params: 0,
        host_peer: 0,
        class: 1,
        privacy: 0,
        max_peers: 16,
        max_players: 16,
        campaign: u32::MAX,
        map: 80,
        protocol: 2,
        difficulty: 0xffff,
    }
}

fn campaign(s: Session) -> Session {
    Session {
        campaign: 1,
        map: 5,
        difficulty: 1,
        protocol: 1,
        ..s
    }
}

#[test]
fn the_peer_check_is_lifted_only_for_a_two_machine_campaign() {
    assert_eq!(
        peer_bypass(&campaign(lobby(2, 2)), MAX_PLAYERS),
        Bypass::Fake {
            real: 2,
            players: None
        }
    );
    assert_eq!(
        peer_bypass(&campaign(lobby(2, 1)), MAX_PLAYERS),
        Bypass::Fake {
            real: 2,
            players: None
        }
    );
    assert!(
        matches!(peer_bypass(&lobby(2, 2), MAX_PLAYERS), Bypass::Retail(_)),
        "multiplayer stays retail"
    );
    assert!(
        matches!(
            peer_bypass(&campaign(lobby(1, 1)), MAX_PLAYERS),
            Bypass::Retail(_)
        ),
        "one machine"
    );
    assert!(matches!(
        peer_bypass(&campaign(lobby(3, 3)), MAX_PLAYERS),
        Bypass::Refuse(_)
    ));
    assert!(
        matches!(
            peer_bypass(&campaign(lobby(2, 3)), MAX_PLAYERS),
            Bypass::Refuse(_)
        ),
        "player cap"
    );
    let gone = Session {
        state: 2,
        ..campaign(lobby(2, 2))
    };
    assert!(matches!(peer_bypass(&gone, MAX_PLAYERS), Bypass::Retail(_)));
    let ended = Session {
        state: 9,
        ..campaign(lobby(2, 2))
    };
    assert!(matches!(
        peer_bypass(&ended, MAX_PLAYERS),
        Bypass::Retail(_)
    ));
}

fn join(players: u32, difficulty: u16) -> JoinInputs {
    JoinInputs {
        ready: true,
        state: 4,
        privacy: 0,
        players,
        max_players: 16,
        life_cycle: 0,
        difficulty,
    }
}

#[test]
fn joinable_drops_only_the_campaign_clause_and_caps_at_two() {
    // Multiplayer: as retail either way.
    assert!(joinable(&join(1, 0xffff), None));
    assert!(joinable(&join(1, 0xffff), Some(MAX_PLAYERS)));
    // Campaign: retail says no; the probe says yes while one player is in.
    assert!(!joinable(&join(1, 1), None));
    assert!(joinable(&join(1, 1), Some(MAX_PLAYERS)));
    assert!(
        !joinable(&join(2, 1), Some(MAX_PLAYERS)),
        "two players: full"
    );
    // Every other retail clause still holds.
    for j in [
        JoinInputs {
            ready: false,
            ..join(1, 1)
        },
        JoinInputs {
            state: 2,
            ..join(1, 1)
        },
        JoinInputs {
            state: 9,
            ..join(1, 1)
        },
        JoinInputs {
            privacy: 1,
            ..join(1, 1)
        },
        JoinInputs {
            max_players: 1,
            ..join(1, 1)
        },
        JoinInputs {
            life_cycle: 6,
            ..join(1, 1)
        },
        JoinInputs {
            life_cycle: 9,
            ..join(1, 1)
        },
    ] {
        assert!(!joinable(&j, Some(MAX_PLAYERS)), "{j:?}");
        assert!(
            !joinable(
                &JoinInputs {
                    difficulty: 0xffff,
                    ..j
                },
                None
            ),
            "{j:?}"
        );
    }
    assert!(joinable(
        &JoinInputs {
            life_cycle: 5,
            ..join(1, 1)
        },
        Some(MAX_PLAYERS)
    ));
}

fn want() -> Want {
    Want {
        name: "01a_tutorial".into(),
        campaign: 1,
        map: 5,
        difficulty: 1,
    }
}

#[test]
fn the_host_sets_the_lobby_once_a_second_machine_is_in() {
    assert!(
        matches!(host_setup(&lobby(1, 1), &want(), MAX_PLAYERS), Setup::Wait(w) if w.contains("second machine"))
    );
    assert_eq!(host_setup(&lobby(2, 2), &want(), MAX_PLAYERS), Setup::Due);
    assert_eq!(
        host_setup(&campaign(lobby(2, 2)), &want(), MAX_PLAYERS),
        Setup::Done
    );
    assert_eq!(
        host_setup(&campaign(lobby(1, 1)), &want(), MAX_PLAYERS),
        Setup::Done,
        "done needs no guest"
    );
    let hard = Session {
        difficulty: 3,
        ..campaign(lobby(2, 2))
    };
    assert_eq!(
        host_setup(&hard, &want(), MAX_PLAYERS),
        Setup::Due,
        "another difficulty"
    );
    let guest = Session {
        local_peer_params: 1,
        local_peer: 1,
        ..lobby(2, 2)
    };
    assert!(
        matches!(host_setup(&guest, &want(), MAX_PLAYERS), Setup::Wait(w) if w.contains("host"))
    );
    let other_class = Session {
        class: 2,
        ..lobby(2, 2)
    };
    assert!(matches!(
        host_setup(&other_class, &want(), MAX_PLAYERS),
        Setup::Wait(_)
    ));
    assert!(matches!(
        host_setup(&lobby(3, 3), &want(), MAX_PLAYERS),
        Setup::Wait(_)
    ));
    assert!(matches!(
        host_setup(
            &Session {
                state: 1,
                ..lobby(2, 2)
            },
            &want(),
            MAX_PLAYERS
        ),
        Setup::Wait(_)
    ));
}

#[test]
fn the_setup_calls_are_the_menus_two() {
    let [map, difficulty] = setup_calls(&Want {
        difficulty: 3,
        ..want()
    });
    assert_eq!(map.point.base, 0x0019_a6f2);
    assert_eq!((map.esi, map.args, map.eax), (Some(5), [Some(1)], None));
    assert_eq!(difficulty.point.base, 0x0019_a76d);
    assert_eq!(
        (difficulty.eax, difficulty.esi, difficulty.args),
        (Some(3), None, [None])
    );
}

#[test]
fn the_peer_limit_goes_back_to_two_for_a_campaign() {
    // What 0x6e720 leaves a campaign: peer limit 1, player limit 2.
    let one = Session {
        max_peers: 1,
        max_players: 2,
        ..campaign(lobby(2, 2))
    };
    assert_eq!(
        limits_fix(&one, MAX_PLAYERS),
        LimitsFix {
            peers: Some(2),
            players: None
        },
        "two players: only the peer limit, as before"
    );
    assert_eq!(
        limits_fix(
            &Session {
                max_peers: 2,
                ..one
            },
            MAX_PLAYERS
        ),
        LimitsFix::default()
    );
    assert_eq!(
        limits_fix(
            &Session {
                max_peers: 1,
                ..lobby(2, 2)
            },
            MAX_PLAYERS
        ),
        LimitsFix::default(),
        "multiplayer"
    );
    assert_eq!(
        limits_fix(&Session { state: 0, ..one }, MAX_PLAYERS),
        LimitsFix::default()
    );
}

/// The playerless sim lobby (docs/runtime/sim.md, "More than four"): a
/// System Link custom game the host leads with no player of its own, one
/// peer per phantom. Neither the start's peer-count window nor the game
/// limits' campaign fix may touch it, however the co-op arm is set: they
/// would cap a 16-peer custom game at co-op's two.
#[test]
fn co_op_session_hooks_leave_a_playerless_multiplayer_lobby_alone() {
    for (peers, players) in [(5, 4), (16, 15), (2, 1)] {
        let s = Session {
            state: 5,
            max_players: players,
            ..lobby(peers, players)
        };
        for limit in [MAX_PLAYERS, 4, CEILING] {
            assert!(
                matches!(peer_bypass(&s, limit), Bypass::Retail(_)),
                "{peers} peers, limit {limit}"
            );
            assert_eq!(limits_fix(&s, limit), LimitsFix::default());
            // Lowered limits (what 0x6e720 leaves a campaign) stay as retail set them too.
            let low = Session {
                max_peers: 1,
                max_players: 1,
                ..s
            };
            assert_eq!(limits_fix(&low, limit), LimitsFix::default());
        }
    }
}

#[test]
fn a_larger_co_op_lifts_both_limits_to_it() {
    let one = Session {
        max_peers: 1,
        max_players: 2,
        ..campaign(lobby(4, 4))
    };
    assert_eq!(
        limits_fix(&one, 4),
        LimitsFix {
            peers: Some(4),
            players: Some(4)
        }
    );
    assert_eq!(
        limits_fix(&one, CEILING),
        LimitsFix {
            peers: Some(16),
            players: Some(16)
        }
    );
    assert_eq!(
        limits_fix(
            &Session {
                max_peers: 16,
                max_players: 16,
                ..one
            },
            4
        ),
        LimitsFix::default(),
        "never lowered"
    );
}

#[test]
fn a_larger_co_op_shows_the_start_one_peer_and_two_players() {
    // Up to the limit: one peer, and two players when there are more.
    assert_eq!(
        peer_bypass(&campaign(lobby(3, 3)), 4),
        Bypass::Fake {
            real: 3,
            players: Some(3)
        }
    );
    assert_eq!(
        peer_bypass(&campaign(lobby(16, 16)), CEILING),
        Bypass::Fake {
            real: 16,
            players: Some(16)
        }
    );
    assert_eq!(
        peer_bypass(&campaign(lobby(2, 2)), 4),
        Bypass::Fake {
            real: 2,
            players: None
        },
        "two players pass 0x6ee9b as they are"
    );
    // Past the limit: retail refuses.
    assert!(
        matches!(peer_bypass(&campaign(lobby(5, 5)), 4), Bypass::Refuse(w) if w.contains("5 peers"))
    );
    assert!(
        matches!(peer_bypass(&campaign(lobby(4, 5)), 4), Bypass::Refuse(w) if w.contains("5 players"))
    );
    assert!(
        matches!(peer_bypass(&lobby(4, 4), 4), Bypass::Retail(_)),
        "multiplayer"
    );
}

#[test]
fn the_window_puts_both_counts_back() {
    let mut m = Fake::default();
    let s = campaign(lobby(4, 4));
    put_session(&mut m, &s);
    m.set_u32(s.addr + layout::PEERS, 1);
    m.set_u32(s.addr + layout::PLAYERS, START_PLAYER_CHECK);
    let lines = restore_counts(
        &mut m,
        Pending {
            addr: s.addr,
            peers: 4,
            players: Some(4),
        },
        "before OPTIONS_NEW",
    );
    assert_eq!(m.u32(s.addr + layout::PEERS), Some(4));
    assert_eq!(m.u32(s.addr + layout::PLAYERS), Some(4));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].contains("player count back to 4"), "{lines:?}");
    // Two players: only the peers were changed.
    m.set_u32(s.addr + layout::PEERS, 1);
    let lines = restore_counts(
        &mut m,
        Pending {
            addr: s.addr,
            peers: 2,
            players: None,
        },
        "after a refused start",
    );
    assert_eq!(m.u32(s.addr + layout::PEERS), Some(2));
    assert_eq!(m.u32(s.addr + layout::PLAYERS), Some(4), "untouched");
    assert_eq!(lines.len(), 1);
}

#[test]
fn a_larger_co_op_stays_joinable_and_due_up_to_its_limit() {
    let mut j = join(3, 1);
    assert!(joinable(&j, Some(4)));
    assert!(!joinable(&j, Some(MAX_PLAYERS)), "two: full at two");
    j.players = 4;
    assert!(!joinable(&j, Some(4)), "full at the limit");
    assert!(!joinable(&j, None), "retail: never for a campaign");
    assert_eq!(host_setup(&lobby(3, 3), &want(), 4), Setup::Due);
    assert_eq!(host_setup(&lobby(4, 4), &want(), 4), Setup::Due);
    assert!(
        matches!(host_setup(&lobby(5, 5), &want(), 4), Setup::Wait(w) if w.contains("stops at 4"))
    );
}

#[test]
fn the_limit_is_the_hosts_setting_and_a_guest_follows_the_host() {
    let arm = Arm::session();
    assert_eq!(arm.max_players(), MAX_PLAYERS, "retail by default");
    assert_eq!(arm.set_max_players(4), 4);
    assert_eq!(arm.set_max_players(1), MAX_PLAYERS, "held to 2");
    assert_eq!(arm.set_max_players(99), CEILING, "held to 16");
    arm.set_max_players(4);
    let host = campaign(lobby(3, 3));
    assert!(host.is_host());
    assert_eq!(arm.limit_for(&host), 4);
    let guest = Session {
        local_peer: 1,
        ..host
    };
    assert_eq!(arm.limit_for(&guest), CEILING);
    assert_eq!(
        Arm::forced(None).max_players(),
        MAX_PLAYERS,
        "the probe stays at two"
    );
}

fn put_session(m: &mut Fake, s: &Session) {
    use layout::*;
    for (o, v) in [
        (STATE, s.state),
        (PEERS, s.peers),
        (PLAYERS, s.players),
        (LOCAL_PEER, s.local_peer),
        (LOCAL_PEER_PARAMS, s.local_peer_params),
        (HOST_PEER, s.host_peer),
        (CLASS, s.class),
        (PRIVACY, s.privacy),
        (MAX_PEERS, s.max_peers),
        (MAX_PLAYERS, s.max_players),
        (CAMPAIGN, s.campaign),
        (MAP, s.map),
        (PROTOCOL, s.protocol),
    ] {
        m.set_u32(s.addr + o, v);
    }
    m.set_u16(s.addr + DIFFICULTY, s.difficulty);
}

#[test]
fn the_session_is_read_through_the_life_cycle() {
    let mut m = Fake::default();
    assert_eq!(read_session(&m), None, "nothing mapped");
    m.wr(points::NETWORK_READY.base, &[0]);
    m.set_u32(points::SQUAD_SESSION.base, S);
    assert_eq!(read_session(&m), None, "not ready");
    m.wr(points::NETWORK_READY.base, &[1]);
    assert_eq!(read_session(&m), None, "session not mapped");
    let s = campaign(lobby(2, 2));
    put_session(&mut m, &s);
    assert_eq!(read_session(&m), Some(s));
    assert!(
        s.describe().contains("peers 2 (max 16), players 2"),
        "{}",
        s.describe()
    );
    assert!(s.describe().contains("protocol 1 (synchronous)"));
    m.set_u32(S + layout::STATE, 0);
    assert_eq!(read_session(&m), None, "state 0: no session");
    m.set_u32(points::SQUAD_SESSION.base, 0);
    assert_eq!(read_session(&m), None);
    // The joinable inputs follow the session.
    m.set_u32(points::SQUAD_SESSION.base, S);
    m.set_u32(S + layout::STATE, 4);
    m.set_u32(LIFE_CYCLE, 7);
    let j = read_join(&m);
    assert_eq!(
        (j.ready, j.players, j.difficulty, j.life_cycle),
        (true, 2, 1, 7)
    );
}

#[test]
fn the_simulation_mode_and_latches_are_reported_on_change() {
    let mut m = Fake::default();
    assert_eq!(read_sim(&m), None);
    let g = 0x0200_0000;
    m.set_u32(points::SIM_GLOBALS.base, g);
    m.set_u32(g + 8, 2);
    m.set_u32(g + 0x28, 77);
    m.wr(g + 0x2c, &[0]);
    let s = read_sim(&m).unwrap();
    assert_eq!(
        s,
        Sim {
            mode: 2,
            update: 77,
            desync: 0,
            ended: 0
        }
    );
    assert_eq!(
        sim_changes(None, s, Some(10)),
        ["simulation mode sync-server (2) at tick 10"]
    );
    assert!(sim_changes(Some(s), s, Some(11)).is_empty());
    let bad = Sim { desync: 1, ..s };
    let l = sim_changes(Some(s), bad, Some(12));
    assert_eq!(l.len(), 1);
    assert!(
        l[0].starts_with("OUT OF SYNC") && l[0].contains("tick 12, update 77"),
        "{l:?}"
    );
    assert!(sim_changes(Some(bad), bad, Some(13)).is_empty(), "once");
    assert_eq!(
        sim_changes(Some(bad), s, None),
        ["the out-of-sync latch cleared"]
    );
    let ended = Sim { ended: 1, ..s };
    assert!(sim_changes(Some(s), ended, Some(14))[0].contains("ended latch"));
    assert_eq!(
        (mode_name(2), role(2), role(3), role(1)),
        ("sync-server", "host", "guest", "local")
    );
    // The world's numbering, from the game's through 0x68da8.
    let names: Vec<_> = (1..=5)
        .map(|game| {
            let world = bots::phantom::session::world_mode(game).unwrap();
            (mode_name(world), role(world))
        })
        .collect();
    assert_eq!(
        names,
        [
            ("local", "local"),
            ("sync-client", "guest"),
            ("sync-server", "host"),
            ("dist-client", "guest"),
            ("dist-server", "host"),
        ]
    );
    assert_eq!((mode_name(0), mode_name(6)), ("none", "?"));
}

#[test]
fn tick_lines_parse_back_as_a_hash_log() {
    let pools = vec![
        lockstep::pools::Pool {
            name: "(head)".into(),
            offset: 0,
            len: 0x10,
        },
        lockstep::pools::Pool {
            name: "cached object render states".into(),
            offset: 0x10,
            len: 0x10,
        },
    ];
    let mut text = format!("{}\n# role host\n", lockstep::pools::LOG_HEADER);
    text += &tick_lines(7, 0xabc, &pools, &[1, 2]);
    text += "# tick 8 checkpoint save\n";
    text += &tick_lines(8, 0xdef, &pools, &[1, 3]);
    let log = lockstep::pools::Log::parse(&text).unwrap();
    assert_eq!(log.ticks.len(), 2);
    assert_eq!(log.ticks[&7]["*"], 0xabc);
    assert_eq!(log.ticks[&8]["cached_object_render_states"], 3);
    assert_eq!(log.notes_at(8), ["checkpoint save"]);
    assert_eq!(tick_lines(1, 2, &[], &[]), "1 * 0000000000000002\n");
}

fn runtime() -> std::sync::Arc<runtime::Runtime> {
    runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    )
}

#[test]
fn off_adds_nothing() {
    let rt = runtime();
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let got = install(
        &rt,
        &mut r,
        None,
        Path::new("/nonexistent"),
        Path::new("/nonexistent"),
    )
    .unwrap();
    assert_eq!(got, Installed::Off);
    let plan = r.build(&feature::Base).unwrap();
    assert!(!plan.text().contains(ID), "{}", plan.text());
}

#[test]
fn on_it_places_every_hook_in_the_plan() {
    let rt = runtime();
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let cfg = Config {
        map: "01a_tutorial".into(),
        difficulty: Difficulty::Normal,
    };
    // No disc here: the level does not resolve, so no host setup, but the
    // rest is placed.
    let got = install(
        &rt,
        &mut r,
        Some(cfg),
        Path::new("/nonexistent/disc.iso"),
        Path::new("/nonexistent"),
    )
    .unwrap();
    assert_eq!(got, Installed::Added { level: None });
    let plan = r.build(&feature::Base).unwrap();
    let text = plan.text();
    for p in HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    assert!(!text.contains("host lobby setup"), "{text}");
}

#[test]
fn with_a_level_the_host_setup_is_on_the_tick() {
    let mut r = feature::Registry::new(feature::Switches::defaults());
    r.add(&Probe::new(Some(want()), PathBuf::from("/nonexistent")))
        .unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    assert!(text.contains("host lobby setup"), "{text}");
    // It shares the tick with another feature's tick listener.
    let mut r = feature::Registry::new(feature::Switches::defaults());
    r.add(&Probe::new(Some(want()), PathBuf::from("/nonexistent")))
        .unwrap();
    r.add(&lockstep::TickObserver(lockstep::Recorder::new(None)))
        .unwrap();
    assert!(r.build(&feature::Base).is_ok());
    // H2_FEATURES=-coop-probe still turns it off.
    let mut r = feature::Registry::new(feature::Switches::parse("-coop-probe").unwrap());
    assert_eq!(
        r.add(&Probe::new(None, PathBuf::from("/nonexistent"))),
        Ok(false)
    );
}

#[test]
fn an_address_overridden_outside_the_plan_refuses_the_probe_cleanly() {
    let rt = runtime();
    rt.edit_dispatch(|d| {
        d.register_override(
            points::SESSION_GAME_START.base,
            "someone-else",
            |_, _, _| unreachable!(),
        );
    });
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let cfg = Config {
        map: DEFAULT_MAP.into(),
        difficulty: Difficulty::Normal,
    };
    let got = install(
        &rt,
        &mut r,
        Some(cfg),
        Path::new("/nonexistent"),
        Path::new("/nonexistent"),
    )
    .unwrap();
    let Installed::Refused(why) = got else {
        panic!("{got:?}")
    };
    assert_eq!(why.len(), 1);
    assert!(
        why[0].contains("SESSION_GAME_START") && why[0].contains("someone-else"),
        "{why:?}"
    );
    // Nothing of the probe is in the plan, so the plan installs.
    let plan = r.build(&feature::Base).unwrap();
    assert!(!plan.text().contains(ID));
    assert_eq!(
        occupied(|a| (a == points::GAME_TICK.base).then(|| "x".to_owned())),
        ["GAME_TICK (0x00137fe0) has x"]
    );
    assert!(occupied(|_| None).is_empty());
}

/// The game-end path can return every tick: the probe said "game end at
/// tick N" on each, thousands of lines. Once a game
/// now, again after the next start.
#[test]
fn game_end_is_said_once_a_game() {
    let mut s = State::default();
    assert!(s.end_first(), "the first return of the end path");
    for _ in 0..1000 {
        assert!(!s.end_first(), "the frames after it");
    }
    s.started();
    assert!(s.end_first(), "the next game's end");
    assert!(!s.end_first());
}

/// docs/runtime/snapshots.md: the hooks block a snapshot while armed, while
/// a session start's counts are shown changed, and while a hash log is
/// open; disarmed and idle they don't.
#[test]
fn the_probe_blocks_snapshots_while_armed_mid_start_or_logging() {
    let arm = Arm::session();
    let st = Mutex::new(State::default());
    assert_eq!(probe_live(&arm, &st), None, "disarmed and idle");
    arm.arm(None);
    assert_eq!(
        probe_live(&arm, &st).as_deref(),
        Some("a co-op session has armed the co-op hooks")
    );
    arm.disarm();
    assert_eq!(probe_live(&arm, &st), None);

    lock(&st).pending = Some(Pending {
        addr: 0x1000,
        peers: 2,
        players: None,
    });
    assert_eq!(
        probe_live(&arm, &st).as_deref(),
        Some("a session start's peer count is shown changed")
    );
    lock(&st).pending = None;

    let name = format!("coop-probe-snap-{}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    lock(&st).log = Some(HashLog::open(&dir, "host", &[]).unwrap());
    let why = probe_live(&arm, &st).unwrap();
    assert!(why.starts_with("the co-op hash log "), "{why}");
    lock(&st).log = None;
    assert_eq!(probe_live(&arm, &st), None);
    let _ = std::fs::remove_dir_all(&dir);

    let forced = Arm::forced(None);
    assert_eq!(
        probe_live(&forced, &st).as_deref(),
        Some("the co-op probe is on")
    );
}

/// The co-op over Live features on one plan: none blocks a snapshot until
/// a session arms them; armed, the probe, players and join in progress do.
#[test]
fn the_co_op_features_block_snapshots_only_while_armed() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let arm = install_live(&rt, &mut r, Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    let plan = r.build(&feature::Base).unwrap();
    assert_eq!(plan.snapshot_blockers(), Vec::new());
    arm.arm(None);
    let blockers = plan.snapshot_blockers();
    let mut ids: Vec<&str> = blockers.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            crate::coop_jip::FEATURE,
            LIVE_ID,
            crate::coop_players::FEATURE,
        ]
    );
    arm.disarm();
    assert_eq!(plan.snapshot_blockers(), Vec::new());
}
