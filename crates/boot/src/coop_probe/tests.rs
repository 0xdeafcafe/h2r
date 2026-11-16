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
        peer_bypass(&campaign(lobby(2, 2))),
        Bypass::Fake { real: 2 }
    );
    assert_eq!(
        peer_bypass(&campaign(lobby(2, 1))),
        Bypass::Fake { real: 2 }
    );
    assert!(
        matches!(peer_bypass(&lobby(2, 2)), Bypass::Retail(_)),
        "multiplayer stays retail"
    );
    assert!(
        matches!(peer_bypass(&campaign(lobby(1, 1))), Bypass::Retail(_)),
        "one machine"
    );
    assert!(matches!(
        peer_bypass(&campaign(lobby(3, 3))),
        Bypass::Refuse(_)
    ));
    assert!(
        matches!(peer_bypass(&campaign(lobby(2, 3))), Bypass::Refuse(_)),
        "player cap"
    );
    let gone = Session {
        state: 2,
        ..campaign(lobby(2, 2))
    };
    assert!(matches!(peer_bypass(&gone), Bypass::Retail(_)));
    let ended = Session {
        state: 9,
        ..campaign(lobby(2, 2))
    };
    assert!(matches!(peer_bypass(&ended), Bypass::Retail(_)));
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
    assert!(joinable(&join(1, 0xffff), false));
    assert!(joinable(&join(1, 0xffff), true));
    // Campaign: retail says no; the probe says yes while one player is in.
    assert!(!joinable(&join(1, 1), false));
    assert!(joinable(&join(1, 1), true));
    assert!(!joinable(&join(2, 1), true), "two players: full");
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
        assert!(!joinable(&j, true), "{j:?}");
        assert!(
            !joinable(
                &JoinInputs {
                    difficulty: 0xffff,
                    ..j
                },
                false
            ),
            "{j:?}"
        );
    }
    assert!(joinable(
        &JoinInputs {
            life_cycle: 5,
            ..join(1, 1)
        },
        true
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
        matches!(host_setup(&lobby(1, 1), &want()), Setup::Wait(w) if w.contains("second machine"))
    );
    assert_eq!(host_setup(&lobby(2, 2), &want()), Setup::Due);
    assert_eq!(host_setup(&campaign(lobby(2, 2)), &want()), Setup::Done);
    assert_eq!(
        host_setup(&campaign(lobby(1, 1)), &want()),
        Setup::Done,
        "done needs no guest"
    );
    let hard = Session {
        difficulty: 3,
        ..campaign(lobby(2, 2))
    };
    assert_eq!(host_setup(&hard, &want()), Setup::Due, "another difficulty");
    let guest = Session {
        local_peer_params: 1,
        local_peer: 1,
        ..lobby(2, 2)
    };
    assert!(matches!(host_setup(&guest, &want()), Setup::Wait(w) if w.contains("host")));
    let other_class = Session {
        class: 2,
        ..lobby(2, 2)
    };
    assert!(matches!(host_setup(&other_class, &want()), Setup::Wait(_)));
    assert!(matches!(host_setup(&lobby(3, 3), &want()), Setup::Wait(_)));
    assert!(matches!(
        host_setup(
            &Session {
                state: 1,
                ..lobby(2, 2)
            },
            &want()
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
    let one = Session {
        max_peers: 1,
        ..campaign(lobby(2, 2))
    };
    assert_eq!(max_peers_fix(&one), Some(2));
    assert_eq!(
        max_peers_fix(&Session {
            max_peers: 2,
            ..one
        }),
        None
    );
    assert_eq!(
        max_peers_fix(&Session {
            max_peers: 1,
            ..lobby(2, 2)
        }),
        None,
        "multiplayer"
    );
    assert_eq!(max_peers_fix(&Session { state: 0, ..one }), None);
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
    m.set_u32(g + 8, 3);
    m.set_u32(g + 0x28, 77);
    m.wr(g + 0x2c, &[0]);
    let s = read_sim(&m).unwrap();
    assert_eq!(
        s,
        Sim {
            mode: 3,
            update: 77,
            desync: 0,
            ended: 0
        }
    );
    assert_eq!(
        sim_changes(None, s, Some(10)),
        ["simulation mode sync-server (3) at tick 10"]
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
        ("sync-client", "guest", "host", "local")
    );
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
