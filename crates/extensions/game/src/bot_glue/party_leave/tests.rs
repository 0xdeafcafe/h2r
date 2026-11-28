use super::*;
use guest_layout::testing::FakeGuest;
use session::{add_peer, add_player, at::*};

const SQUAD: u32 = 0x0070_0000;
const TARGET: u32 = 0x0071_0000;
const HOST: [u8; 6] = [2, 0, 0, 0, 0, 1];
const HUMAN: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn bot(number: u32) -> [u8; 6] {
    use bots::phantom::{PhantomHandle, machine_id};
    machine_id(PhantomHandle(number))
}

fn session_with(m: &mut FakeGuest, s: u32, peers: &[[u8; 6]]) {
    m.map(s, 0x8000)
        .put_u32(s + STATE, session::STATE_LIVE)
        .put_u32(s + MAX_PEERS, session::MAX)
        .put_u32(s + MAX_PLAYERS, session::MAX);
    for (i, &machine) in peers.iter().enumerate() {
        let peer = add_peer(m, s, machine, "Player").unwrap().unwrap();
        let id = session::player_id((i + 1) as u64, machine);
        add_player(m, s, peer, id, &[0; CONFIG_SIZE])
            .unwrap()
            .unwrap();
        if machine == HOST {
            m.put_u32(s + HOST_PEER, peer)
                .put_u32(s + LOCAL_PEER, peer)
                .put_u32(s + PEER_50, peer);
        }
    }
}

fn party(peers: &[[u8; 6]]) -> FakeGuest {
    let mut m = FakeGuest::new();
    m.put_u8(lobby::NETWORKING, 1)
        .put_u32(lobby::SESSION_TYPE, 1)
        .put_u32(lobby::SESSION, SQUAD);
    session_with(&mut m, SQUAD, peers);
    m
}

#[test]
fn clanker_only_party_uses_solo_leave_and_retires_the_peers_on_exit() {
    let mut m = party(&[bot(1), HOST, bot(2)]);
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), Some(1));
    assert_eq!(leave_peer_count(&m, at::PAUSE_LEAVE_COUNT), Some(1));
    assert_eq!(leave_peer_count(&m, 0), None);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 3);

    retire(&mut m, &Phantoms::default(), false);

    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 1);
    assert_eq!(m.u32(SQUAD + PLAYER_COUNT).unwrap(), 1);
    assert_eq!(m.u32(SQUAD + PLAYER_MASK).unwrap(), 1 << 1);
    assert_eq!(m.u32(SQUAD + LOCAL_PEER).unwrap(), 0);
    assert_eq!(m.u32(SQUAD + PEER_50).unwrap(), 0);
    assert_eq!(session::find_peer(&m, SQUAD, HOST).unwrap(), Some(0));
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), None);
}

#[test]
fn mixed_party_keeps_retail_leave_and_only_humans_are_leader_candidates() {
    let mut m = party(&[HOST, bot(1), HUMAN]);
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), None);
    assert_eq!(leave_peer_count(&m, at::PAUSE_LEAVE_COUNT), None);
    for caller in [
        at::LEADER_ROW_BUILD,
        at::LEADER_ROW_LABEL,
        at::LEADER_ROW_PRESS,
        at::LEADER_AUTO_SELECT,
        at::LEADER_APPOINT,
    ] {
        assert!(reject_candidate(&m, caller, 0xe000_0001));
        assert!(!reject_candidate(&m, caller, 2));
        assert!(!reject_candidate(&m, caller, u32::MAX));
    }
    assert!(!reject_candidate(&m, 0, 1));

    retire(&mut m, &Phantoms::default(), false);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 3);
    assert_eq!(m.u32(SQUAD + PLAYER_MASK).unwrap(), 0b111);
}

#[test]
fn another_local_player_or_a_joining_human_peer_preserves_retail_leave() {
    let mut m = party(&[HOST, bot(1)]);
    m.put_u32(SQUAD + PLAYER_MASK, 0b111)
        .put_u32(SQUAD + PLAYER_COUNT, 3)
        .put_u32(SQUAD + PLAYERS + 2 * PLAYER_SIZE + P_PEER, 0);
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), None);

    let mut m = party(&[HOST, bot(1)]);
    add_peer(&mut m, SQUAD, HUMAN, "Joining").unwrap().unwrap();
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), None);
}

#[test]
fn leaving_requires_the_local_party_leader() {
    let mut m = party(&[HOST, bot(1)]);
    m.put_u32(SQUAD + PEER_50, 1);
    assert_eq!(leave_peer_count(&m, at::LOBBY_BACK_COUNT), None);
    retire(&mut m, &Phantoms::default(), false);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 2);
}

#[test]
fn confirming_matchmaking_back_retires_the_target_and_keeps_the_party() {
    let mut m = party(&[HOST, bot(1)]);
    m.put_u32(lobby::SESSION_TYPE, 6)
        .put_u32(lobby::TARGET_SESSION, TARGET);
    session_with(&mut m, TARGET, &[HOST, bot(2), bot(3)]);
    assert_eq!(leave_peer_count(&m, at::PAUSE_LEAVE_COUNT), Some(1));
    assert_eq!(m.u32(TARGET + PEER_COUNT).unwrap(), 3);

    let phantoms = Phantoms::default();
    retire(&mut m, &phantoms, true);
    assert_eq!(m.u32(TARGET + PEER_COUNT).unwrap(), 1);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 2);

    retire(&mut m, &phantoms, false);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 1);
}

#[test]
fn a_human_in_the_matchmaking_target_keeps_retail_leave() {
    let mut m = party(&[HOST, bot(1)]);
    m.put_u32(lobby::SESSION_TYPE, 7)
        .put_u32(lobby::TARGET_SESSION, TARGET);
    session_with(&mut m, TARGET, &[HOST, bot(2), HUMAN]);

    assert_eq!(leave_peer_count(&m, at::PAUSE_LEAVE_COUNT), None);
    let phantoms = Phantoms::default();
    retire(&mut m, &phantoms, true);
    retire(&mut m, &phantoms, false);
    assert_eq!(m.u32(SQUAD + PEER_COUNT).unwrap(), 2);
    assert_eq!(m.u32(TARGET + PEER_COUNT).unwrap(), 3);
}
