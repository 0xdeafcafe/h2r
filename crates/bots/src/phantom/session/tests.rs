use super::*;
use engine::datum::layout as dl;
use engine::handle::DatumHandle;
use engine::testing::FakeGuest;
use input::host::HostPadState;

const S: u32 = 0x0070_0000;
const SIM: u32 = 0x0071_0000;
const GAME: u32 = 0x0072_0000;
const TIME: u32 = 0x0072_1000;
const PLAYERS_HEADER: u32 = 0x0073_0000;
const PLAYERS_DATA: u32 = 0x0073_1000;
const OBJECTS_HEADER: u32 = 0x0074_0000;
const OBJECTS_DATA: u32 = 0x0074_1000;
const BODY: u32 = 0x0075_0000;

const HOST_MAC: [u8; 6] = [0x00, 0x50, 0xf2, 1, 2, 3];
const MAC_A: [u8; 6] = [0xc1, 0xa4, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0xc1, 0xa4, 0, 0, 0, 2];

/// A hosted multiplayer game: the session with the host's peer and player,
/// the simulation as distributed server, the game at tick 100, and the
/// host's player (index 0) with a configuration and a unit.
fn hosted() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(points::NETWORK_READY.base & !0xfff, 0x1000)
        .put_u8(points::NETWORK_READY.base, 1);
    g.map(points::SQUAD_SESSION.base & !0xfff, 0x1000)
        .put_u32(points::SQUAD_SESSION.base, S);
    g.map(S, 0x8000)
        .put_u32(S + STATE, STATE_LIVE)
        .put_u32(S + CLASS, 4)
        .put_u32(S + MAX_PEERS, 16)
        .put_u32(S + MAX_PLAYERS, 16)
        .put_u32(S + PEER_COUNT, 1)
        .put_u32(S + PLAYER_MASK, 1)
        .put_u32(S + PLAYER_COUNT, 1)
        .put_u32(S + HOST_PEER, 0);
    g.put(S + PEERS + PEER_MAC, &HOST_MAC);
    g.put_u32(S + PEERS + PEER_SLOTS, 0);
    for u in 1..4 {
        g.put_u32(S + PEERS + PEER_SLOTS + 4 * u, u32::MAX);
    }
    g.map(SIM_GLOBALS & !0xfff, 0x1000)
        .put_u32(SIM_GLOBALS, SIM)
        .map(SIM, 0x1000)
        .put_u32(SIM + SIM_MODE, WORLD_DIST_SERVER);
    for i in 0..16 {
        g.put_u32(SIM + SIM_SLOTS + i * SLOT_SIZE, u32::MAX);
    }
    g.map(points::GAME_GLOBALS.base & !0xfff, 0x1000)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .map(GAME, 0x400)
        .put_u32(GAME + guest::OPTIONS, guest::MODE_MULTIPLAYER);
    g.map(points::GAME_TIME.base & !0xfff, 0x1000)
        .put_u32(points::GAME_TIME.base, TIME)
        .map(TIME, 0x10)
        .put_u32(TIME + guest::GAME_TIME_TICK, 100);
    // Players: the host at 0; room for phantoms.
    g.map(points::PLAYERS.base & !0xfff, 0x1000)
        .put_u32(points::PLAYERS.base, PLAYERS_HEADER)
        .map(PLAYERS_HEADER, 0x48)
        .put_u32(PLAYERS_HEADER + dl::ELEMENT_SIZE, 0x21c)
        .put_u32(PLAYERS_HEADER + dl::HIGH_WATER, 1)
        .put_u32(PLAYERS_HEADER + dl::DATA, PLAYERS_DATA)
        .map(PLAYERS_DATA, 0x21c * 16);
    g.put_u16(PLAYERS_DATA, 0xe000)
        .put_u16(PLAYERS_DATA + 0x28, 0);
    g.put(PLAYERS_DATA + guest::PLAYER_MACHINE, &HOST_MAC);
    for i in 0..CONFIG_SIZE as u32 {
        g.put_u8(PLAYERS_DATA + PLAYER_CONFIG + i, 0x40 + i as u8);
    }
    g.map(points::OBJECTS.base & !0xfff, 0x1000)
        .put_u32(points::OBJECTS.base, OBJECTS_HEADER)
        .map(OBJECTS_HEADER, 0x48)
        .put_u32(OBJECTS_HEADER + dl::ELEMENT_SIZE, 0xc)
        .put_u32(OBJECTS_HEADER + dl::HIGH_WATER, 16)
        .put_u32(OBJECTS_HEADER + dl::DATA, OBJECTS_DATA)
        .map(OBJECTS_DATA, 0xc * 16)
        .map(BODY, 0x400 * 16);
    g
}

/// What the game does from the session a frame later: a player datum at
/// `index` for machine `mac`, with a unit facing `aim`, and its simulation
/// slot.
fn game_makes(g: &mut FakeGuest, index: u32, mac: [u8; 6], aim: [f32; 3]) {
    let p = PLAYERS_DATA + index * 0x21c;
    let hw = g.u32(PLAYERS_HEADER + dl::HIGH_WATER).unwrap();
    g.put_u32(PLAYERS_HEADER + dl::HIGH_WATER, hw.max(index + 1));
    let unit = DatumHandle::new(0xb000 + index as u16, index);
    g.put_u16(p, 0xe000 + index as u16)
        .put_u16(p + 0x28, 0xffff)
        .put_u32(p + 0x2c, unit.raw());
    g.put(p + guest::PLAYER_MACHINE, &mac);
    let h = OBJECTS_DATA + index * 0xc;
    let body = BODY + index * 0x400;
    g.put_u16(h, unit.salt()).put_u32(h + 8, body);
    for k in 0..3 {
        g.put_f32(body + guest::UNIT_AIM + 4 * k, aim[k as usize]);
    }
    g.put_u8(body + UNIT_WEAPON, 1);
    let slot = SIM + SIM_SLOTS + index * SLOT_SIZE;
    g.put_u32(slot, index).put(slot + SLOT_MAC, &mac);
    g.put_u32(slot + SLOT_STAMP, u32::MAX);
}

fn counters(g: &FakeGuest) -> (u32, u32) {
    (g.u32(S + UPDATE).unwrap(), g.u32(S + CHANGES).unwrap())
}

#[test]
fn only_the_host_of_a_running_multiplayer_game_takes_phantoms() {
    let g = hosted();
    assert_eq!(host_session(&g), Ok(S));
    assert_eq!(live_session(&g), Some(S));

    let mut menus = hosted();
    menus.put_u32(points::GAME_TIME.base, 0);
    assert_eq!(host_session(&menus), Err(NotNow::Wait), "no game");
    assert_eq!(live_session(&menus), Some(S), "the lobby is still live");

    let mut campaign = hosted();
    campaign.put_u32(GAME + guest::OPTIONS, 1);
    assert_eq!(host_session(&campaign), Err(NotNow::Wait));

    let mut client = hosted();
    client.put_u32(SIM + SIM_MODE, 5);
    assert!(
        matches!(host_session(&client), Err(NotNow::NotHost(w)) if w.contains("distributed server (world mode 5, distributed client;")),
        "{:?}",
        host_session(&client)
    );
    // The run 20261119-014620: a lone Live host whose world read 4, the
    // distributed server, taken for a client. It is the host.
    let mut host = hosted();
    host.put_u32(SIM + SIM_MODE, 4);
    assert_eq!(host_session(&host), Ok(S));

    // States 6 to 8 tear the simulation down: never a running game.
    let mut leaving = hosted();
    leaving.put_u32(S + STATE, 6);
    assert!(
        matches!(host_session(&leaving), Err(NotNow::NotHost(w)) if w.contains("not live (state 6")),
        "{:?}",
        host_session(&leaving)
    );
    assert_eq!(live_session(&leaving), None);

    let mut local = hosted();
    local.put_u8(points::NETWORK_READY.base, 0);
    assert!(
        matches!(host_session(&local), Err(NotNow::NotHost(w)) if w.starts_with("no network session")),
        "{:?}",
        host_session(&local)
    );

    // The h2 bots test run: a Split Screen game, simulation mode 1.
    let mut split = hosted();
    split.put_u32(SIM + SIM_MODE, MODE_LOCAL);
    assert!(
        matches!(host_session(&split), Err(NotNow::NotHost(w)) if w.starts_with("a local game (simulation mode 1, Split Screen)")),
        "{:?}",
        host_session(&split)
    );

    let mut class = hosted();
    class.put_u32(S + CLASS, 3);
    assert_eq!(
        host_session(&class),
        Err(NotNow::NotHost("the session's class is 3, not 4".into()))
    );
}

#[test]
fn a_peer_is_written_as_the_join_path_writes_it() {
    let mut g = hosted();
    g.put(S + PEERS + PEER_SIZE, &[0xaa; PEER_SIZE as usize]); // stale bytes
    assert_eq!(add_peer(&mut g, S, MAC_A, "Rusty Lugnut").unwrap(), Some(1));
    assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
    let p = S + PEERS + PEER_SIZE;
    assert_eq!(mac_at(&g, p + PEER_MAC).unwrap(), MAC_A);
    assert_eq!(g.u16(p + PEER_NAME).unwrap(), u16::from(b'R'));
    assert_eq!(g.u16(p + PEER_NAME + 2 * 11).unwrap(), u16::from(b't'));
    assert_eq!(g.u16(p + PEER_NAME + 2 * 12).unwrap(), 0);
    assert_eq!(g.u32(p + PEER_PLAYERS).unwrap(), 0);
    assert_eq!(g.u8(p + 0x24).unwrap(), 0, "the stale bytes are cleared");
    for u in 0..4 {
        assert_eq!(g.u32(p + PEER_SLOTS + 4 * u).unwrap(), u32::MAX);
    }
    let c = S + CHANNELS + CHANNEL_SIZE;
    assert_eq!(g.u8(c).unwrap(), 1, "valid");
    assert_eq!(
        g.u8(c + 1).unwrap(),
        0,
        "never connected: no sends, no drops"
    );
    assert_eq!(g.u32(c + 4).unwrap(), u32::MAX, "no channel: no view");
    assert_eq!(counters(&g), (1, 1));
}

#[test]
fn no_peer_past_the_sessions_limit() {
    let mut g = hosted();
    g.put_u32(S + MAX_PEERS, 2);
    assert_eq!(add_peer(&mut g, S, MAC_A, "A").unwrap(), Some(1));
    assert_eq!(add_peer(&mut g, S, MAC_B, "B").unwrap(), None);
    assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
}

#[test]
fn a_player_is_written_as_the_add_player_path_writes_it() {
    let mut g = hosted();
    add_peer(&mut g, S, MAC_A, "A").unwrap();
    let cfg = [7u8; CONFIG_SIZE];
    let id = player_id(0x000e_0000_0000_0001, MAC_A);
    assert_eq!(
        add_player(&mut g, S, 1, id, &cfg).unwrap(),
        Some(1),
        "lowest free slot"
    );
    let p = S + PLAYERS + PLAYER_SIZE;
    let mut got = [0; 12];
    g.read(p, &mut got).unwrap();
    assert_eq!(got, id);
    assert_eq!(g.u32(p + P_PEER).unwrap(), 1);
    assert_eq!(g.u32(p + P_USER).unwrap(), 0);
    assert_eq!(
        g.u32(p + P_CONTROLLER).unwrap(),
        0,
        "never -1: the diff skips -1"
    );
    for o in [P_REQUESTED, P_EFFECTIVE] {
        let mut c = [0; CONFIG_SIZE];
        g.read(p + o, &mut c).unwrap();
        assert_eq!(c, cfg);
    }
    assert_eq!(g.u32(S + PLAYER_MASK).unwrap(), 0b11);
    assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
    let peer = S + PEERS + PEER_SIZE;
    assert_eq!(g.u32(peer + PEER_SLOTS).unwrap(), 1);
    assert_eq!(g.u32(peer + PEER_PLAYERS).unwrap(), 1);
    assert_eq!(counters(&g), (2, 2));
    // The same id twice is refused, as 0x5f890 refuses it.
    add_peer(&mut g, S, MAC_B, "B").unwrap();
    assert_eq!(add_player(&mut g, S, 2, id, &cfg).unwrap(), None);
}

#[test]
fn no_player_past_the_sessions_limit() {
    let mut g = hosted();
    g.put_u32(S + MAX_PLAYERS, 1);
    add_peer(&mut g, S, MAC_A, "A").unwrap();
    let id = player_id(1, MAC_A);
    assert_eq!(
        add_player(&mut g, S, 1, id, &[0; CONFIG_SIZE]).unwrap(),
        None
    );
    assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 1);
}

#[test]
fn removing_a_peer_moves_the_later_ones_down() {
    let mut g = hosted();
    for (mac, xuid) in [(MAC_A, 1), (MAC_B, 2)] {
        let p = add_peer(&mut g, S, mac, "X").unwrap().unwrap();
        add_player(&mut g, S, p, player_id(xuid, mac), &[0; CONFIG_SIZE]).unwrap();
    }
    g.put_u32(S + HOST_PEER, 2); // a session index above the one removed
    remove_peer(&mut g, S, 1).unwrap();
    assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
    assert_eq!(find_peer(&g, S, MAC_A).unwrap(), None);
    assert_eq!(find_peer(&g, S, MAC_B).unwrap(), Some(1));
    assert_eq!(find_peer(&g, S, HOST_MAC).unwrap(), Some(0));
    assert_eq!(g.u32(S + PLAYER_MASK).unwrap(), 0b101, "A's player out");
    assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
    assert_eq!(
        g.u32(S + PLAYERS + 2 * PLAYER_SIZE + P_PEER).unwrap(),
        1,
        "B's player follows its peer down"
    );
    assert_eq!(g.u32(S + PLAYERS + P_PEER).unwrap(), 0, "the host's stays");
    assert_eq!(g.u32(S + HOST_PEER).unwrap(), 1);
    assert_eq!(
        g.u32(S + PEERS + PEER_SIZE + PEER_SLOTS).unwrap(),
        2,
        "B's peer keeps its player's slot"
    );
    assert_eq!(
        mac_at(&g, S + PEERS + 2 * PEER_SIZE + PEER_MAC).unwrap(),
        [0; 6],
        "the last cleared"
    );
    assert_eq!(g.u8(S + CHANNELS + 2 * CHANNEL_SIZE).unwrap(), 0);
}

#[test]
fn the_configuration_is_the_hosts_with_a_name_and_team() {
    let g = hosted();
    let c = config(&g, "Bolt", Some(2)).unwrap().unwrap();
    // The bot mark (U+2699), a space, the name.
    assert_eq!(
        &c[..14],
        &[
            0x99, 0x26, b' ', 0, b'B', 0, b'o', 0, b'l', 0, b't', 0, 0, 0
        ]
    );
    assert!(
        c[14..32].iter().all(|&b| b == 0),
        "the rest of the name cleared"
    );
    assert_eq!(c[CONFIG_TEAM], 2);
    assert_eq!(c[0x44], 0x40 + 0x44, "the rest is the host's");
    assert_eq!(
        config(&g, "Bolt", None).unwrap().unwrap()[CONFIG_TEAM],
        0x40 + CONFIG_TEAM as u8,
        "no team asked: the host's"
    );
}

#[test]
fn a_bots_name_is_marked_within_fifteen_units() {
    let s = |units: Vec<u16>| String::from_utf16(&units).unwrap();
    assert_eq!(s(marked_name("Clanker 12")), "\u{2699} Clanker 12");
    // The longest that keeps the space: 13 characters.
    assert_eq!(s(marked_name("Thirteen char")), "\u{2699} Thirteen char");
    // 14: no space; 15 (the most a gamertag has): no space, its last
    // character gone. Never more than the 15 units the name holds.
    assert_eq!(s(marked_name("Fourteen chars")), "\u{2699}Fourteen chars");
    assert_eq!(s(marked_name("Fifteen chars!!")), "\u{2699}Fifteen chars!");
    for n in 1..=20 {
        let m = marked_name(&"x".repeat(n));
        assert!(m.len() <= CONFIG_NAME_UNITS - 1, "{n}: {}", m.len());
        assert_eq!(m[0], MARK as u16);
    }
}

#[test]
fn the_host_team_is_the_first_local_players() {
    let g = hosted();
    assert_eq!(host_team(&g).unwrap(), Some(0x40 + CONFIG_TEAM as u8));
    // No players array: none.
    let mut none = FakeGuest::new();
    none.map(points::PLAYERS.base & !0xfff, 0x1000);
    assert_eq!(host_team(&none).unwrap(), None);
}

#[test]
fn an_action_passes_the_validators_ranges() {
    let a = Action {
        flags: 0,
        yaw: -0.5,
        pitch: 3.0,
        forward: 2.0,
        strafe: -2.0,
        trigger: 1.5,
        trigger2: -1.0,
        weapon: 1,
    };
    let b = a.bytes();
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    assert!(
        (0.0..std::f32::consts::TAU).contains(&f(4)),
        "yaw wrapped into [0, 2pi)"
    );
    assert!(f(8).abs() <= 1.5);
    assert_eq!((f(0xc), f(0x10), f(0x14), f(0x18)), (1.0, -1.0, 1.0, 0.0));
    assert_eq!((b[0x20], b[0x21]), (1, 0xff), "the weapon, no second");
    assert_eq!(&b[0x22..0x26], &[0xff; 4], "grenade and +0x24 none");
    assert!(b[0x26..].iter().all(|&x| x == 0), "no target, no aim block");
    assert_eq!(Action { weapon: 9, ..a }.bytes()[0x20], 0xff);
}

#[test]
fn the_pad_turns_from_the_current_facing() {
    let still = action_from_pad(&HostPadState::default(), [0.0, 1.0, 0.0], 0, 1.0 / 30.0);
    assert!((still.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(still.pitch, 0.0);
    let pad = HostPadState {
        right_stick: (1.0, 1.0),
        left_stick: (1.0, 0.5),
        right_trigger: 1.0,
        left_trigger: 0.25,
        ..Default::default()
    };
    let a = action_from_pad(&pad, [1.0, 0.0, 0.0], 2, 0.5);
    assert!(
        (a.yaw + TURN_RATE * 0.5).abs() < 1e-6,
        "+x turns right (yaw down), as the brain steers"
    );
    assert!((a.pitch - TURN_RATE * 0.5).abs() < 1e-6, "+y looks up");
    assert_eq!((a.forward, a.strafe), (0.5, -1.0));
    assert_eq!((a.trigger, a.trigger2, a.weapon), (1.0, 0.25, 2));
}

#[test]
fn the_action_goes_in_the_slot_stamped_with_the_tick() {
    let mut g = hosted();
    game_makes(&mut g, 1, MAC_A, [1.0, 0.0, 0.0]);
    let slot = slot_of(&g, MAC_A).unwrap().unwrap();
    assert_eq!(slot, SIM + SIM_SLOTS + SLOT_SIZE);
    assert_eq!(slot_of(&g, MAC_B).unwrap(), None);
    let a = Action {
        trigger: 1.0,
        weapon: -1,
        ..Default::default()
    };
    write_action(&mut g, slot, &a).unwrap();
    assert_eq!(g.u32(slot + SLOT_STAMP).unwrap(), 100);
    let mut got = [0; ACTION_SIZE];
    g.read(slot + SLOT_ACTION, &mut got).unwrap();
    assert_eq!(got, a.bytes());
    assert_eq!(
        facing(&g, PLAYERS_DATA + 0x21c).unwrap(),
        Some(([1.0, 0.0, 0.0], 1))
    );
}

/// The run 20261119-005001's game start, as the bots' hook reads it: a
/// Live custom game with this console alone in the session, distributed,
/// in a host state, whose host peer is not this console's own.
fn one_console_start(host: u32) -> FakeGuest {
    let mut g = hosted();
    g.put_u32(S + PROTOCOL, PROTOCOL_DISTRIBUTED)
        .put_u32(S + LOCAL_PEER, 0)
        .put_u32(S + HOST_PEER, host);
    g
}

#[test]
fn the_world_numbers_its_modes_otherwise_than_the_game() {
    // 0x68c00's jump table at 0x68da8.
    let worlds: Vec<_> = (0..=6).map(world_mode).collect();
    assert_eq!(
        worlds,
        [None, Some(1), Some(3), Some(2), Some(5), Some(4), None]
    );
    assert_eq!(world_mode(GAME_DIST_SERVER), Some(WORLD_DIST_SERVER));
    assert_eq!(world_mode(MODE_LOCAL), Some(MODE_LOCAL));
    assert_eq!(world_mode_name(WORLD_DIST_SERVER), "distributed server");
    assert_eq!(world_mode_name(5), "distributed client");
    assert_eq!(world_mode_name(2), "synchronous server");
}

#[test]
fn the_start_facts_give_the_mode_the_session_start_chooses() {
    let g = one_console_start(1);
    let f = StartFacts::read(&g, S).unwrap();
    assert_eq!(
        f,
        StartFacts {
            state: STATE_LIVE,
            protocol: 2,
            peers: 1,
            host: 1,
            local: 0,
            consoles: 0,
        }
    );
    assert_eq!(f.mode(), Some(4), "a distributed client: the run's mode 4");
    assert_eq!(
        f.describe(),
        "session state 5, protocol 2, peer count 1 (0 other consoles), host peer 1, this console's peer 0"
    );
    let host = StartFacts {
        host: 0,
        ..f.clone()
    };
    assert_eq!(host.mode(), Some(GAME_DIST_SERVER));
    assert_eq!(host.mode().and_then(world_mode), Some(WORLD_DIST_SERVER));
    assert_eq!(
        StartFacts {
            protocol: 0,
            ..f.clone()
        }
        .mode(),
        Some(MODE_LOCAL)
    );
    assert_eq!(
        StartFacts {
            protocol: 1,
            ..host
        }
        .mode(),
        Some(3)
    );
    assert_eq!(StartFacts { protocol: 7, ..f }.mode(), None);
}

#[test]
fn a_phantom_peer_is_not_another_console() {
    let mut g = one_console_start(1);
    add_peer(&mut g, S, MAC_A, "A").unwrap().unwrap();
    let f = StartFacts::read(&g, S).unwrap();
    assert_eq!((f.peers, f.consoles), (2, 0));
    // A real console's peer counts.
    add_peer(&mut g, S, [0x00, 0x50, 0xf2, 9, 9, 9], "Guest")
        .unwrap()
        .unwrap();
    assert_eq!(StartFacts::read(&g, S).unwrap().consoles, 1);
}

#[test]
fn a_lone_hosts_session_is_made_its_own_host_when_phantoms_are_planned() {
    let mut g = one_console_start(1);
    let f = StartFacts::read(&g, S).unwrap();
    assert_eq!(host_fix(&f, true), HostFix::Host { host: 1, local: 0 });
    make_host(&mut g, S, 0).unwrap();
    let after = StartFacts::read(&g, S).unwrap();
    assert_eq!(after.mode(), Some(GAME_DIST_SERVER));
    assert!(
        matches!(host_fix(&after, true), HostFix::Keep(w) if w.contains("is the session's host"))
    );
    // A host peer of none (-1) is fixed the same way.
    let g = one_console_start(u32::MAX);
    let f = StartFacts::read(&g, S).unwrap();
    assert_eq!(
        host_fix(&f, true),
        HostFix::Host {
            host: u32::MAX,
            local: 0
        }
    );
}

#[test]
fn the_session_is_left_alone_unless_it_can_only_be_this_consoles() {
    let base = StartFacts {
        state: STATE_LIVE,
        protocol: 2,
        peers: 1,
        host: 1,
        local: 0,
        consoles: 0,
    };
    let keep = |f: StartFacts, planned: bool, why: &str| match host_fix(&f, planned) {
        HostFix::Keep(w) => assert!(w.contains(why), "{w:?} lacks {why:?}"),
        other => panic!("{other:?} for {f:?}"),
    };
    keep(base.clone(), false, "no phantoms planned");
    keep(
        StartFacts {
            protocol: 0,
            ..base.clone()
        },
        true,
        "not distributed",
    );
    keep(
        StartFacts {
            protocol: 1,
            ..base.clone()
        },
        true,
        "not distributed",
    );
    keep(
        StartFacts {
            host: 0,
            ..base.clone()
        },
        true,
        "is the session's host",
    );
    // A client's states: this console joined another's game.
    keep(
        StartFacts {
            state: 3,
            ..base.clone()
        },
        true,
        "not a host's",
    );
    keep(
        StartFacts {
            state: 4,
            ..base.clone()
        },
        true,
        "not a host's",
    );
    keep(
        StartFacts {
            // Host 0, so the peers differ and the range is what refuses.
            host: 0,
            local: 1,
            ..base.clone()
        },
        true,
        "not one of the 1 peers",
    );
    keep(
        StartFacts {
            local: u32::MAX,
            ..base.clone()
        },
        true,
        "not one of",
    );
    keep(
        StartFacts {
            peers: 2,
            consoles: 1,
            ..base.clone()
        },
        true,
        "1 other console in the session",
    );
    // Phantom peers are no other console: still fixed.
    assert_eq!(
        host_fix(
            &StartFacts {
                peers: 3,
                ..base.clone()
            },
            true
        ),
        HostFix::Host { host: 1, local: 0 }
    );
    for state in HOST_STATES {
        assert!(matches!(
            host_fix(
                &StartFacts {
                    state,
                    ..base.clone()
                },
                true
            ),
            HostFix::Host { .. }
        ));
    }
}

#[test]
fn the_game_start_hook_says_what_it_read_and_makes_the_host() {
    let mut g = one_console_start(1);
    let log = game_start(&mut g, S, true);
    assert_eq!(
        log,
        vec![
            "bots: game start: session state 5, protocol 2, peer count 1 (0 other consoles), \
             host peer 1, this console's peer 0: game mode 4 (simulation world mode 5, \
             distributed client)"
                .to_owned(),
            "bots: game start: phantoms planned and no other console in the session: this \
             console hosts it (host peer 1 -> 0), so the game starts as the distributed server \
             (game mode 5, world mode 4)"
                .to_owned(),
        ]
    );
    assert_eq!(g.u32(S + HOST_PEER).unwrap(), 0);
    // Again: nothing more to do.
    let log = game_start(&mut g, S, true);
    assert!(
        log[0].ends_with("game mode 5 (simulation world mode 4, distributed server)"),
        "{log:?}"
    );
    assert!(
        log[1].contains("session as it is (this console is the session's host"),
        "{log:?}"
    );

    // Without phantoms nothing is written.
    let mut g = one_console_start(1);
    let log = game_start(&mut g, S, false);
    assert!(log[1].contains("no phantoms planned"), "{log:?}");
    assert_eq!(g.u32(S + HOST_PEER).unwrap(), 1);
}

mod books {
    use super::*;
    use crate::phantom::{Identity, Phantoms, State, machine_id};

    fn firing() -> Box<dyn crate::local::PadSource> {
        Box::new(|f: &crate::local::Frame<'_>| {
            assert_eq!(f.port, 4);
            assert!(f.machine.is_some());
            HostPadState {
                right_trigger: 1.0,
                ..Default::default()
            }
        })
    }

    fn who(xuid: u64, tag: &str) -> Identity {
        Identity {
            xuid,
            gamertag: tag.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_phantom_joins_plays_and_leaves_with_the_game() {
        let ph = Phantoms::default();
        let h = ph
            .add(
                who(0x000e_0000_0000_0001, "Rusty Lugnut"),
                Some(1),
                firing(),
            )
            .unwrap();
        let mac = machine_id(h);

        // In the menus: it waits.
        let mut g = hosted();
        g.put_u32(points::GAME_TIME.base, 0);
        ph.tick(&mut g, 0).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 1);
        assert_eq!(ph.list()[0].state, State::Pending);

        // The game runs: a peer and a player in the session.
        g.put_u32(points::GAME_TIME.base, TIME);
        let log = ph.tick(&mut g, 1).unwrap();
        assert!(log[0].contains("Rusty Lugnut joins"), "{log:?}");
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
        let cfg = S + PLAYERS + PLAYER_SIZE + P_EFFECTIVE;
        // The bot mark, a space, then the gamertag.
        assert_eq!(g.u16(cfg).unwrap(), 0x2699);
        assert_eq!(g.u16(cfg + 2).unwrap(), u16::from(b' '));
        assert_eq!(g.u16(cfg + 4).unwrap(), u16::from(b'R'));
        assert_eq!(g.u8(cfg + CONFIG_TEAM as u32).unwrap(), 1);
        assert_eq!(
            ph.list()[0].state,
            State::Pending,
            "until the game makes the player"
        );

        // The game makes it: joined, and its source fires.
        game_makes(&mut g, 1, mac, [1.0, 0.0, 0.0]);
        ph.tick(&mut g, 2).unwrap();
        assert_eq!(ph.player(h), Some(1));
        assert_eq!(ph.list()[0].state, State::Joined);
        let slot = slot_of(&g, mac).unwrap().unwrap();
        assert_eq!(g.u32(slot + SLOT_STAMP).unwrap(), 100);
        assert_eq!(g.u32(slot + SLOT_ACTION + 0x14).unwrap(), 1.0f32.to_bits());
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2, "made once");

        // The game ends: out of the session, waiting for the next.
        g.put_u32(points::GAME_TIME.base, 0);
        ph.tick(&mut g, 3).unwrap();
        assert_eq!(find_peer(&g, S, mac).unwrap(), None);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 1);
        let l = ph.list();
        assert_eq!((l[0].state.clone(), l[0].player), (State::Pending, None));
    }

    #[test]
    fn a_removed_phantom_leaves_the_session() {
        let ph = Phantoms::default();
        let a = ph.add(who(1, "A"), None, firing()).unwrap();
        let b = ph.add(who(2, "B"), None, firing()).unwrap();
        let mut g = hosted();
        ph.tick(&mut g, 0).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 3);
        ph.remove(a).unwrap();
        ph.tick(&mut g, 1).unwrap();
        assert_eq!(find_peer(&g, S, machine_id(a)).unwrap(), None);
        assert_eq!(find_peer(&g, S, machine_id(b)).unwrap(), Some(1));
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
    }

    #[test]
    fn a_phantom_waits_while_this_console_is_not_the_host_then_joins() {
        // The user's run: "phantom Clanker 4 failed: not the host" for
        // good, from a check made before this console hosted the game.
        let ph = Phantoms::default();
        ph.add(who(1, "Clanker 4"), None, firing()).unwrap();
        let mut g = hosted();
        g.put_u32(SIM + SIM_MODE, 5);
        let log = ph.tick(&mut g, 0).unwrap();
        assert_eq!(
            log,
            vec![
                "bots: phantom Clanker 4 waits: this console is not the game's host yet \
                 (the simulation is not the distributed server (world mode 5, distributed \
                 client; the server's is 4; session host peer 0, this console's peer 0, peer count 1))"
                    .to_owned()
            ]
        );
        assert_eq!(ph.list()[0].state, State::Pending, "not failed");
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 1, "nothing written");

        // Said once: the same reason the next frames says nothing.
        for f in 1..5 {
            assert!(ph.tick(&mut g, f).unwrap().is_empty());
        }
        // Another reason is said.
        g.put_u32(SIM + SIM_MODE, WORLD_DIST_SERVER)
            .put_u32(S + STATE, 6);
        let log = ph.tick(&mut g, 5).unwrap();
        assert!(
            log[0].contains("the session is not live (state 6"),
            "{log:?}"
        );

        // Hosting is established: it joins.
        g.put_u32(S + STATE, STATE_LIVE);
        let log = ph.tick(&mut g, 6).unwrap();
        assert!(log[0].contains("Clanker 4 joins (peer 1)"), "{log:?}");
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
    }

    #[test]
    fn a_full_session_fails_the_extra() {
        let ph = Phantoms::default();
        ph.add(who(1, "A"), None, firing()).unwrap();
        ph.add(who(2, "B"), None, firing()).unwrap();
        let mut g = hosted();
        g.put_u32(S + MAX_PLAYERS, 2);
        ph.tick(&mut g, 0).unwrap();
        let l = ph.list();
        assert_eq!(l[0].state, State::Pending);
        assert!(matches!(&l[1].state, State::Failed(w) if w.contains("player")));
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2, "B's peer taken back out");
    }
}

/// Phantoms in the pregame lobby (docs/game/bots.md §5.1, "Phantoms in
/// the pregame lobby").
mod lobby {
    use super::*;
    use crate::lobby as l;
    use crate::phantom::{Identity, Phantoms, State, lobby_wanted, machine_id};

    /// The hopper the lobby's peers report on.
    const HOPPER: u32 = 8;

    /// A Live custom game lobby this console hosts: [`hosted`]'s session
    /// with no game running, a pregame lobby (type 2, state 5),
    /// distributed, this console's peer 0 the host, its session player in
    /// slot 0 (requested configuration `0x10 + i`, team 0), and its peer's
    /// properties: name "Tester", map loaded (4), hopper 8 ready (1).
    fn lobby() -> FakeGuest {
        let mut g = hosted();
        g.put_u32(points::GAME_TIME.base, 0);
        g.put_u8(l::LOBBY_ON, 1)
            .put_u32(l::SESSION_TYPE, 2)
            .put_u32(l::MAP_FAILED_HERE, 0)
            .put_u32(S + PROTOCOL, PROTOCOL_DISTRIBUTED)
            .put_u32(S + LOCAL_PEER, 0)
            .put_u32(S + HOST_PEER, 0)
            .put_u32(S + PEERS + PEER_PLAYERS, 1);
        let host = S + PLAYERS;
        for i in 0..CONFIG_SIZE as u32 {
            g.put_u8(host + P_REQUESTED + i, 0x10 + i as u8);
            g.put_u8(host + P_EFFECTIVE + i, 0x10 + i as u8);
        }
        g.put_u8(host + P_REQUESTED + CONFIG_TEAM as u32, 0)
            .put_u8(host + P_EFFECTIVE + CONFIG_TEAM as u32, 0)
            .put_u32(host + P_PEER, 0);
        let p = S + PEERS;
        for (i, u) in "Tester".encode_utf16().enumerate() {
            g.put_u16(p + PEER_NAME + 2 * i as u32, u);
        }
        g.put_u8(p + PEER_PROPS_SET, 1)
            .put_u32(p + PEER_MAP_STATUS, 4)
            .put_u32(p + PEER_MAP_STATUS + 4, 100)
            .put_u32(p + PEER_HOPPERS + 4 * HOPPER, 1);
        g
    }

    fn make_team_game(g: &mut FakeGuest) {
        g.put_u32(S + VARIANT + VARIANT_FLAGS, 1);
    }

    fn idle() -> Box<dyn crate::local::PadSource> {
        Box::new(|_: &crate::local::Frame<'_>| HostPadState {
            right_trigger: 1.0,
            ..Default::default()
        })
    }

    fn who(xuid: u64, tag: &str) -> Identity {
        Identity {
            xuid,
            gamertag: tag.into(),
            ..Default::default()
        }
    }

    fn team_of(g: &FakeGuest, slot: u32) -> (u8, u8) {
        let a = S + PLAYERS + slot * PLAYER_SIZE + CONFIG_TEAM as u32;
        (
            g.u8(a + P_REQUESTED).unwrap(),
            g.u8(a + P_EFFECTIVE).unwrap(),
        )
    }

    #[test]
    fn the_lobby_this_console_hosts_takes_phantoms() {
        let g = lobby();
        assert_eq!(host_lobby(&g), Ok(S));
        assert_eq!(owned_session(&g), Some(S));

        // Not a pregame lobby: no lobby facts, another session type, a
        // running game (the game's own check decides then).
        let mut menus = lobby();
        menus.put_u8(l::LOBBY_ON, 0);
        assert_eq!(host_lobby(&menus), Err(NotNow::Wait));
        let mut other = lobby();
        other.put_u32(l::SESSION_TYPE, 3);
        assert_eq!(host_lobby(&other), Err(NotNow::Wait));
        let mut game = lobby();
        game.put_u32(points::GAME_TIME.base, TIME);
        assert_eq!(host_lobby(&game), Err(NotNow::Wait));

        // A lobby, but not one a phantom can join: say why.
        let mut leaving = lobby();
        leaving.put_u32(S + STATE, 6);
        assert!(
            matches!(host_lobby(&leaving), Err(NotNow::NotHost(w)) if w.contains("not live (state 6")),
        );
        let mut split = lobby();
        split.put_u32(S + PROTOCOL, 0);
        assert!(matches!(host_lobby(&split), Err(NotNow::NotHost(w)) if w.contains("protocol 0")),);
        assert_eq!(owned_session(&split), None);
        let mut client = lobby();
        client.put_u32(S + LOCAL_PEER, 1).put_u32(S + PEER_COUNT, 2);
        assert_eq!(
            host_lobby(&client),
            Err(NotNow::NotHost(
                "this console does not host it (session host peer 0, this console's peer 1)".into()
            ))
        );
        assert_eq!(owned_session(&client), None);
    }

    #[test]
    fn reservations_keep_player_slots_free() {
        let mut g = lobby();
        g.put_u32(S + MAX_PLAYERS, 4);
        assert_eq!(free_player_slots(&g, S).unwrap(), 3);
        // One held, not taken; one taken (the player is in): only the
        // first counts (0x62e30).
        g.put_u8(S + RESERVATIONS, 1)
            .put_u8(S + RESERVATIONS + RESERVATION_SIZE, 1)
            .put_u8(S + RESERVATIONS + RESERVATION_SIZE + 1, 1);
        assert_eq!(free_player_slots(&g, S).unwrap(), 2);
        g.put_u8(S + RESERVATIONS + 2 * RESERVATION_SIZE, 1)
            .put_u8(S + RESERVATIONS + 3 * RESERVATION_SIZE, 1);
        assert_eq!(free_player_slots(&g, S).unwrap(), 0);
        add_peer(&mut g, S, MAC_A, "A").unwrap();
        assert_eq!(
            add_player(&mut g, S, 1, player_id(1, MAC_A), &[0; CONFIG_SIZE]).unwrap(),
            None,
            "the reserved slots are not a phantom's"
        );
    }

    #[test]
    fn a_phantoms_properties_are_this_consoles_with_its_name() {
        let mut g = lobby();
        add_peer(&mut g, S, MAC_A, "Rusty Lugnut").unwrap();
        let before = counters(&g);
        assert!(sync_props(&mut g, S, 1, "Rusty Lugnut").unwrap());
        let p = S + PEERS + PEER_SIZE;
        assert_eq!(g.u8(p + PEER_PROPS_SET).unwrap(), 1);
        assert_eq!(g.u32(p + PEER_MAP_STATUS).unwrap(), 4, "the map loaded");
        assert_eq!(g.u32(p + PEER_MAP_STATUS + 4).unwrap(), 100);
        assert_eq!(
            g.u32(p + PEER_HOPPERS + 4 * HOPPER).unwrap(),
            1,
            "the hopper ready, as 0x66050 wants"
        );
        for at in [PEER_NAME, PEER_NAME2] {
            assert_eq!(g.u16(at + p).unwrap(), u16::from(b'R'));
            assert_eq!(g.u16(at + p + 2 * 11).unwrap(), u16::from(b't'));
            assert_eq!(g.u16(at + p + 2 * 12).unwrap(), 0, "not the host's name");
        }
        assert_eq!(counters(&g), (before.0 + 1, before.1 + 1));
        // The same again: nothing written.
        assert!(!sync_props(&mut g, S, 1, "Rusty Lugnut").unwrap());
        assert_eq!(counters(&g), (before.0 + 1, before.1 + 1));
        // This console loads another map: the phantom follows.
        g.put_u32(S + PEERS + PEER_MAP_STATUS, 2);
        assert!(sync_props(&mut g, S, 1, "Rusty Lugnut").unwrap());
        assert_eq!(g.u32(p + PEER_MAP_STATUS).unwrap(), 2);
        let f = l::facts(&g).unwrap();
        assert_eq!(l::map_status(&f.maps), 2);
    }

    #[test]
    fn the_configuration_is_the_hosts_session_players() {
        let g = lobby();
        let c = config_in(&g, S, "Rusty Lugnut", None).unwrap().unwrap();
        assert_eq!(c[0x40], 0x50, "the host's requested configuration");
        assert_eq!(u16::from_le_bytes([c[0], c[1]]), MARK as u16);
        assert_eq!(c[CONFIG_TEAM], 0);
        let c = config_in(&g, S, "Rusty Lugnut", Some(1)).unwrap().unwrap();
        assert_eq!(c[CONFIG_TEAM], 1);
        // No session player of its own: the player datum's (the game's).
        let mut g = hosted();
        g.put_u32(S + PEERS + PEER_SLOTS, u32::MAX);
        let c = config_in(&g, S, "X", None).unwrap().unwrap();
        assert_eq!(c[0x40], 0x80);
    }

    #[test]
    fn a_new_phantom_goes_to_the_smaller_team() {
        let mut g = lobby();
        make_team_game(&mut g);
        assert!(team_game(&g, S).unwrap());
        assert_eq!(lobby_team(&g, S).unwrap(), 1, "the host is red");
        g.put_u8(S + PLAYERS + P_EFFECTIVE + CONFIG_TEAM as u32, 1);
        assert_eq!(lobby_team(&g, S).unwrap(), 0);
        // A human on green (team 3): green counts as a team too.
        g.put_u32(S + PLAYER_MASK, 0b11).put_u8(
            S + PLAYERS + PLAYER_SIZE + P_EFFECTIVE + CONFIG_TEAM as u32,
            3,
        );
        g.put_u8(S + PLAYERS + P_EFFECTIVE + CONFIG_TEAM as u32, 0);
        assert_eq!(lobby_team(&g, S).unwrap(), 1, "red 1, blue 0, green 1");
        assert_eq!(lobby_host_team(&g).unwrap(), Some(0));
    }

    #[test]
    fn rebalance_moves_a_phantom_only_for_two_or_more() {
        let mut g = lobby();
        make_team_game(&mut g);
        // Host red, phantoms on peers 1 to 3 all red: 4 to 0.
        for (i, mac) in [MAC_A, MAC_B, [0xc1, 0xa4, 0, 0, 0, 3]]
            .into_iter()
            .enumerate()
        {
            let p = add_peer(&mut g, S, mac, "B").unwrap().unwrap();
            let mut c = [0; CONFIG_SIZE];
            c[CONFIG_TEAM] = 0;
            add_player(&mut g, S, p, player_id(i as u64 + 1, mac), &c).unwrap();
        }
        let moved = rebalance(&mut g, S, &[1, 2, 3]).unwrap();
        assert_eq!(moved, vec![(1, 0, 1), (2, 0, 1)], "2 against 2");
        assert_eq!(team_of(&g, 1), (1, 1), "requested and effective");
        assert_eq!(team_of(&g, 3), (0, 0));
        assert!(rebalance(&mut g, S, &[1, 2, 3]).unwrap().is_empty(), "even");
        // The host goes blue: 1 red against 3 blue, so one comes back.
        g.put_u8(S + PLAYERS + P_EFFECTIVE + CONFIG_TEAM as u32, 1);
        assert_eq!(rebalance(&mut g, S, &[1, 2, 3]).unwrap(), vec![(1, 1, 0)]);
        // A phantom with its own team (not movable) stays put.
        assert!(rebalance(&mut g, S, &[]).unwrap().is_empty());
        // Not a team game: nothing moves.
        g.put_u32(S + VARIANT + VARIANT_FLAGS, 0);
        g.put_u8(
            S + PLAYERS + 2 * PLAYER_SIZE + P_EFFECTIVE + CONFIG_TEAM as u32,
            5,
        );
        assert!(rebalance(&mut g, S, &[1, 2, 3]).unwrap().is_empty());
    }

    #[test]
    fn the_lobby_switch_reads_off_values() {
        assert!(lobby_wanted(None));
        assert!(lobby_wanted(Some("1")));
        assert!(lobby_wanted(Some("on")));
        for off in ["0", "off", "OFF", " false ", "no"] {
            assert!(!lobby_wanted(Some(off)), "{off}");
        }
    }

    #[test]
    fn a_phantom_joins_the_lobby_stays_for_the_game_and_comes_back() {
        let ph = Phantoms::default();
        let h = ph
            .add(who(0x000e_0000_0000_0001, "Rusty Lugnut"), None, idle())
            .unwrap();
        let mac = machine_id(h);
        let mut g = lobby();

        // The lobby: a peer with the host's properties and a player.
        let log = ph.tick(&mut g, 0).unwrap();
        assert_eq!(
            log,
            vec!["bots: phantom Rusty Lugnut joins (peer 1, in the pregame lobby)".to_owned()]
        );
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));
        let p = S + PEERS + PEER_SIZE;
        assert_eq!(g.u32(p + PEER_HOPPERS + 4 * HOPPER).unwrap(), 1);
        assert_eq!(g.u32(p + PEER_MAP_STATUS).unwrap(), 4);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
        let cfg = S + PLAYERS + PLAYER_SIZE + P_EFFECTIVE;
        assert_eq!(
            g.u16(cfg).unwrap(),
            MARK as u16,
            "listed by its marked name"
        );
        assert_eq!(g.u8(cfg + 0x40).unwrap(), 0x50, "the host's look");
        assert_eq!(
            ph.list()[0].state,
            State::Pending,
            "no player datum before the game"
        );
        // The start checks see every peer with the map.
        assert_eq!(l::expected(&l::facts(&g).unwrap()), l::READY);
        // Nothing more the next frames.
        assert!(ph.tick(&mut g, 1).unwrap().is_empty());
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);

        // The game loads: its first frames are not hosted yet. The
        // phantom stays in the session and says nothing.
        g.put_u32(points::GAME_TIME.base, TIME)
            .put_u32(SIM + SIM_MODE, 5);
        assert!(ph.tick(&mut g, 2).unwrap().is_empty());
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));

        // Hosted: no second join; the game makes its player; it plays.
        g.put_u32(SIM + SIM_MODE, WORLD_DIST_SERVER);
        assert!(ph.tick(&mut g, 3).unwrap().is_empty());
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2, "made once");
        game_makes(&mut g, 1, mac, [1.0, 0.0, 0.0]);
        ph.tick(&mut g, 4).unwrap();
        assert_eq!(ph.player(h), Some(1));
        let slot = slot_of(&g, mac).unwrap().unwrap();
        assert_eq!(g.u32(slot + SLOT_STAMP).unwrap(), 100);

        // The game ends, back in the lobby: still in the session, no
        // action written there.
        g.put_u32(points::GAME_TIME.base, 0);
        g.put_u32(slot + SLOT_STAMP, 7);
        assert!(ph.tick(&mut g, 5).unwrap().is_empty());
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));
        assert_eq!(g.u32(slot + SLOT_STAMP).unwrap(), 7);

        // No lobby screen (the carnage report, say), but the session is
        // still this console's: kept.
        g.put_u8(l::LOBBY_ON, 0);
        assert!(ph.tick(&mut g, 6).unwrap().is_empty());
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));
        g.put_u8(l::LOBBY_ON, 1);

        // The host leaves the lobby (the session no longer live): off the
        // session's books; a dead session is left alone.
        g.put_u32(S + STATE, 7);
        ph.tick(&mut g, 7).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
        // Live again with the old peer still there: it is made anew, never
        // twice.
        g.put_u32(S + STATE, STATE_LIVE);
        let log = ph.tick(&mut g, 8).unwrap();
        assert!(
            log[0].contains("Rusty Lugnut joins (peer 1, in the pregame lobby)"),
            "{log:?}"
        );
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
        assert_eq!(find_peer(&g, S, mac).unwrap(), Some(1));
    }

    #[test]
    fn a_session_this_console_no_longer_hosts_loses_its_phantoms() {
        let ph = Phantoms::default();
        let h = ph.add(who(1, "A"), None, idle()).unwrap();
        let mut g = lobby();
        ph.tick(&mut g, 0).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2);
        // Another console became the host (host migration).
        g.put_u32(S + HOST_PEER, 1);
        let log = ph.tick(&mut g, 1).unwrap();
        assert_eq!(find_peer(&g, S, machine_id(h)).unwrap(), None);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 1);
        assert!(log.iter().all(|l| !l.contains("joins")), "{log:?}");
    }

    #[test]
    fn a_lower_count_takes_a_phantom_out_of_the_lobby() {
        let ph = Phantoms::default();
        let a = ph.add(who(1, "A"), None, idle()).unwrap();
        let b = ph.add(who(2, "B"), None, idle()).unwrap();
        let mut g = lobby();
        ph.tick(&mut g, 0).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 3);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 3);
        ph.remove(a).unwrap();
        ph.tick(&mut g, 1).unwrap();
        assert_eq!(find_peer(&g, S, machine_id(a)).unwrap(), None);
        assert_eq!(find_peer(&g, S, machine_id(b)).unwrap(), Some(1));
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 2);
        // Off: the last one goes too.
        ph.remove(b).unwrap();
        ph.tick(&mut g, 2).unwrap();
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 1);
        assert_eq!(g.u32(S + PLAYER_COUNT).unwrap(), 1);
        assert!(ph.is_empty());
    }

    #[test]
    fn phantoms_balance_the_teams_as_they_join_and_follow_the_humans() {
        let ph = Phantoms::default();
        for x in 1..=3 {
            ph.add(who(x, &format!("Bot {x}")), None, idle()).unwrap();
        }
        // One with its own team stays on it.
        ph.add(who(9, "Red Bot"), Some(0), idle()).unwrap();
        let mut g = lobby();
        make_team_game(&mut g);
        ph.tick(&mut g, 0).unwrap();
        // Host red; Bot 1 blue, Bot 2 red, Bot 3 blue, Red Bot red.
        assert_eq!(team_of(&g, 1), (1, 1));
        assert_eq!(team_of(&g, 2), (0, 0));
        assert_eq!(team_of(&g, 3), (1, 1));
        assert_eq!(team_of(&g, 4), (0, 0));
        // The host goes blue: 2 red against 3 blue is fine.
        g.put_u8(S + PLAYERS + P_EFFECTIVE + CONFIG_TEAM as u32, 1);
        assert!(ph.tick(&mut g, 1).unwrap().is_empty());
        // A human joins blue (a real peer): 2 against 4, one bot crosses.
        g.put_u32(S + PLAYER_MASK, 0b10_0001_1111).put_u8(
            S + PLAYERS + 9 * PLAYER_SIZE + P_EFFECTIVE + CONFIG_TEAM as u32,
            1,
        );
        let log = ph.tick(&mut g, 2).unwrap();
        assert_eq!(
            log,
            vec!["bots: phantom Bot 1 changes team 1 -> 0 to keep the teams even".to_owned()]
        );
        assert_eq!(team_of(&g, 1), (0, 0));
    }

    #[test]
    fn a_phantom_the_game_took_out_joins_again() {
        let ph = Phantoms::default();
        let h = ph.add(who(1, "A"), None, idle()).unwrap();
        let mut g = lobby();
        ph.tick(&mut g, 0).unwrap();
        remove_peer(&mut g, S, 1).unwrap();
        let log = ph.tick(&mut g, 1).unwrap();
        assert_eq!(
            log,
            vec![
                "bots: phantom A is no longer in the session: it joins again".to_owned(),
                "bots: phantom A joins (peer 1, in the pregame lobby)".to_owned(),
            ]
        );
        assert_eq!(find_peer(&g, S, machine_id(h)).unwrap(), Some(1));
    }

    #[test]
    fn a_lobby_this_console_joined_makes_them_wait() {
        let ph = Phantoms::default();
        ph.add(who(1, "A"), None, idle()).unwrap();
        let mut g = lobby();
        g.put_u32(S + LOCAL_PEER, 1).put_u32(S + PEER_COUNT, 2);
        let log = ph.tick(&mut g, 0).unwrap();
        assert_eq!(
            log,
            vec![
                "bots: phantom A waits: not a lobby it can join (this console does not host it \
                 (session host peer 0, this console's peer 1))"
                    .to_owned()
            ]
        );
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 2, "nothing written");
        assert!(ph.tick(&mut g, 1).unwrap().is_empty(), "said once");
    }

    #[test]
    fn with_the_lobby_join_off_they_wait_for_the_game_and_leave_with_it() {
        let ph = Phantoms::default();
        ph.set_lobby(false);
        let h = ph.add(who(1, "A"), None, idle()).unwrap();
        let mut g = lobby();
        assert!(ph.tick(&mut g, 0).unwrap().is_empty());
        assert_eq!(g.u32(S + PEER_COUNT).unwrap(), 1, "not in the lobby");
        g.put_u32(points::GAME_TIME.base, TIME);
        let log = ph.tick(&mut g, 1).unwrap();
        assert_eq!(log, vec!["bots: phantom A joins (peer 1)".to_owned()]);
        g.put_u32(points::GAME_TIME.base, 0);
        ph.tick(&mut g, 2).unwrap();
        assert_eq!(
            find_peer(&g, S, machine_id(h)).unwrap(),
            None,
            "out with the game"
        );
    }
}

/// Phantoms in the game: spawning, the start's wait, the bot mark on
/// local bots (docs/game/bots.md §5.1, "Phantoms in the game").
mod in_game {
    use super::*;
    use crate::phantom::{Identity, Phantoms, machine_id};

    const SET: u32 = 0x0076_0000;
    const REAL: [u8; 6] = [0x00, 0x50, 0xf2, 9, 9, 9];

    /// [`hosted`]'s game starting: the world in state 3 waiting for
    /// machines 0 to 2 (the host, `ids[1]`, `ids[2]`).
    fn starting(ids: [[u8; 6]; 2]) -> FakeGuest {
        let mut g = hosted();
        g.put_u32(SIM + SIM_STATE, SIM_STARTING)
            .put_u32(SIM + SIM_WAIT, 0b111)
            .put_u32(MACHINES, SET)
            .map(SET, 0x100)
            .put_u32(SET + MACHINES_MASK, 0b111);
        g.put(SET + MACHINES_IDS, &HOST_MAC);
        g.put(SET + MACHINES_IDS + 6, &ids[0]);
        g.put(SET + MACHINES_IDS + 12, &ids[1]);
        g
    }

    #[test]
    fn the_start_stops_waiting_for_phantom_machines() {
        let mut g = starting([MAC_A, REAL]);
        assert_eq!(unwait_phantoms(&mut g).unwrap(), Some((0b111, 0b101)));
        assert_eq!(
            g.u32(SIM + SIM_WAIT).unwrap(),
            0b101,
            "the real console still waited for"
        );
        assert_eq!(unwait_phantoms(&mut g).unwrap(), None, "once");

        // Real consoles only: nothing changes.
        let mut g = starting([REAL, [0x00, 0x50, 0xf2, 8, 8, 8]]);
        assert_eq!(unwait_phantoms(&mut g).unwrap(), None);
        // Not starting (state 4, views joining), or a client's world.
        let mut g = starting([MAC_A, MAC_B]);
        g.put_u32(SIM + SIM_STATE, 4);
        assert_eq!(unwait_phantoms(&mut g).unwrap(), None);
        let mut g = starting([MAC_A, MAC_B]);
        g.put_u32(SIM + SIM_MODE, 5);
        assert_eq!(unwait_phantoms(&mut g).unwrap(), None);
        // A synchronous server waits the same way.
        let mut g = starting([MAC_A, MAC_B]);
        g.put_u32(SIM + SIM_MODE, 2);
        assert_eq!(unwait_phantoms(&mut g).unwrap(), Some((0b111, 0b001)));
    }

    #[test]
    fn the_tick_says_the_start_no_longer_waits() {
        let ph = Phantoms::default();
        let h = ph
            .add(
                Identity {
                    xuid: 1,
                    gamertag: "A".into(),
                    ..Default::default()
                },
                None,
                Box::new(|_: &crate::local::Frame<'_>| HostPadState::default()),
            )
            .unwrap();
        let mut g = starting([machine_id(h), REAL]);
        let log = ph.tick(&mut g, 0).unwrap();
        assert_eq!(
            log[0],
            "bots: phantoms: the game's start waits for real consoles only (machines 0x0007 -> 0x0005)"
        );
        assert!(
            !ph.tick(&mut g, 1)
                .unwrap()
                .iter()
                .any(|l| l.contains("start waits"))
        );
    }

    #[test]
    fn a_phantom_in_the_session_stands_in_the_game() {
        let ph = Phantoms::default();
        let who = |x: u64| Identity {
            xuid: x,
            gamertag: format!("B{x}"),
            ..Default::default()
        };
        let idle = || -> Box<dyn crate::local::PadSource> {
            Box::new(|_: &crate::local::Frame<'_>| HostPadState::default())
        };
        let a = ph.add(who(1), None, idle()).unwrap();
        assert!(
            !ph.stands_for(machine_id(a)),
            "on the books, not in the session"
        );
        let mut g = hosted();
        ph.tick(&mut g, 0).unwrap();
        assert!(ph.stands_for(machine_id(a)));
        assert!(!ph.stands_for(HOST_MAC), "a console answers for itself");
        assert!(!ph.stands_for(machine_id(crate::phantom::PhantomHandle(99))));
        ph.remove(a).unwrap();
        assert!(!ph.stands_for(machine_id(a)));
    }

    /// [`hosted`]'s session with a local bot on controller 1 in slot 1
    /// (effective name "Clanker 1", requested "Profile") and the human on
    /// controller 0 in slot 0.
    fn with_local_bot() -> FakeGuest {
        let mut g = hosted();
        g.put_u32(S + PLAYER_MASK, 0b11)
            .put_u32(S + PLAYER_COUNT, 2)
            .put_u32(S + PEERS + PEER_SLOTS + 4, 1)
            .put_u32(S + PEERS + PEER_PLAYERS, 2);
        let name = |g: &mut FakeGuest, at: u32, s: &str| {
            for (i, u) in s.encode_utf16().enumerate() {
                g.put_u16(at + 2 * i as u32, u);
            }
        };
        let human = S + PLAYERS;
        name(&mut g, human + P_EFFECTIVE, "Tester");
        let bot = S + PLAYERS + PLAYER_SIZE;
        g.put_u32(bot + P_PEER, 0)
            .put_u32(bot + P_USER, 1)
            .put_u32(bot + P_CONTROLLER, 1);
        name(&mut g, bot + P_EFFECTIVE, "Clanker 1");
        name(&mut g, bot + P_REQUESTED, "Profile");
        g
    }

    fn name_at(g: &FakeGuest, at: u32) -> Vec<u16> {
        (0..CONFIG_NAME_UNITS as u32)
            .map(|i| g.u16(at + 2 * i).unwrap())
            .take_while(|&u| u != 0)
            .collect()
    }

    #[test]
    fn a_local_bot_gets_the_bot_mark_on_the_host() {
        let mut g = with_local_bot();
        let before = counters(&g);
        assert_eq!(mark_local_bots(&mut g, 0b10).unwrap(), vec![1]);
        let bot = S + PLAYERS + PLAYER_SIZE;
        assert_eq!(name_at(&g, bot + P_EFFECTIVE), marked_name("Clanker 1"));
        assert_eq!(
            name_at(&g, bot + P_REQUESTED),
            "Profile".encode_utf16().collect::<Vec<_>>(),
            "the requested name is the profile's"
        );
        assert_eq!(
            name_at(&g, S + PLAYERS + P_EFFECTIVE),
            "Tester".encode_utf16().collect::<Vec<_>>(),
            "the human is no bot"
        );
        assert_eq!(counters(&g), (before.0 + 1, before.1 + 1));
        // Marked once.
        assert!(mark_local_bots(&mut g, 0b10).unwrap().is_empty());
        assert_eq!(counters(&g), (before.0 + 1, before.1 + 1));
    }

    #[test]
    fn the_bot_mark_waits_for_the_host_and_a_bots_port() {
        // Not this console's session: its host's configurations win.
        let mut g = with_local_bot();
        g.put_u32(S + HOST_PEER, 1).put_u32(S + PEER_COUNT, 2);
        assert!(mark_local_bots(&mut g, 0b10).unwrap().is_empty());
        // Port 1 is no bot's.
        let mut g = with_local_bot();
        assert!(mark_local_bots(&mut g, 0b100).unwrap().is_empty());
        // No live session.
        let mut g = with_local_bot();
        g.put_u32(S + STATE, 6);
        assert!(mark_local_bots(&mut g, 0b10).unwrap().is_empty());
        // A long name keeps the mark within the 15 units.
        let mut g = with_local_bot();
        let bot = S + PLAYERS + PLAYER_SIZE + P_EFFECTIVE;
        for (i, u) in "Fifteen Chars!!".encode_utf16().enumerate() {
            g.put_u16(bot + 2 * i as u32, u);
        }
        mark_local_bots(&mut g, 0b10).unwrap();
        let n = name_at(&g, bot);
        assert_eq!(n.len(), 15);
        assert_eq!(n[0], MARK as u16);
    }
}
