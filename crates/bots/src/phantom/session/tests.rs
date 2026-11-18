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
        .put_u32(SIM + SIM_MODE, MODE_DIST_SERVER);
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
    client.put_u32(SIM + SIM_MODE, 4);
    assert_eq!(host_session(&client), Err(NotNow::NotHost));

    // States 6 to 8 tear the simulation down: never a running game.
    let mut leaving = hosted();
    leaving.put_u32(S + STATE, 6);
    assert_eq!(host_session(&leaving), Err(NotNow::NotHost));
    assert_eq!(live_session(&leaving), None);

    let mut local = hosted();
    local.put_u8(points::NETWORK_READY.base, 0);
    assert_eq!(host_session(&local), Err(NotNow::NotHost));
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
    assert_eq!(&c[..10], &[b'B', 0, b'o', 0, b'l', 0, b't', 0, 0, 0]);
    assert!(
        c[10..32].iter().all(|&b| b == 0),
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
        assert_eq!(g.u16(cfg).unwrap(), u16::from(b'R'));
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
    fn a_client_fails_its_phantoms_and_a_full_session_fails_the_extra() {
        let ph = Phantoms::default();
        ph.add(who(1, "A"), None, firing()).unwrap();
        let mut client = hosted();
        client.put_u32(SIM + SIM_MODE, 4);
        ph.tick(&mut client, 0).unwrap();
        assert!(matches!(ph.list()[0].state, State::Failed(_)));
        assert_eq!(client.u32(S + PEER_COUNT).unwrap(), 1);

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
