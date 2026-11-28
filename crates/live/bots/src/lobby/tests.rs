use super::*;
use guest_layout::testing::FakeGuest;

const S: u32 = 0x0070_0000;
const T: u32 = 0x0072_0000;

/// A host's Live custom game lobby: networking on, a pregame session in
/// state 5, this console the leader, `peers` peers with these map
/// statuses, no countdown.
fn lobby(maps: &[u32]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.put_u8(LOBBY_ON, 1)
        .put_u8(NETWORKING, 1)
        .put_u32(SESSION_TYPE, 2)
        .put_u32(MAP_FAILED_HERE, 0)
        .put_u32(SESSION, S)
        .map(S, 0x7500)
        .put_u32(S + STATE, 5)
        .put_u32(S + PEER, 0)
        .put_u32(S + LEADER_PEER, 0)
        .put_u32(S + PEER_COUNT, maps.len() as u32);
    for (i, &m) in (0u32..).zip(maps) {
        g.put_u32(S + PEERS + i * PEER_SIZE + PEER_MAP, m);
    }
    g
}

#[test]
fn a_hosts_lobby_with_the_map_loaded_is_ready() {
    let g = lobby(&[4]);
    let f = facts(&g).unwrap();
    assert!(f.leader && f.countdown.is_none());
    assert_eq!(f.maps, vec![4]);
    assert_eq!(expected(&f), READY);
    assert_eq!(blocked_by(READY, &f), "nothing (ready)");
}

#[test]
fn the_leader_check_compares_the_two_peers_not_any_player() {
    // 0x592f0: session +0x72d8 against +0x50. No slot, profile or Live
    // flag is read.
    let mut g = lobby(&[4, 4]);
    g.put_u32(S + PEER, 1);
    let f = facts(&g).unwrap();
    assert!(!f.leader);
    assert_eq!(expected(&f), 8, "Waiting for the leader to start the game");
    g.put_u32(S + LEADER_PEER, 1);
    assert_eq!(expected(&facts(&g).unwrap()), READY);
}

#[test]
fn the_map_check_follows_each_peers_status() {
    assert_eq!(map_status(&[]), 4);
    assert_eq!(map_status(&[4, 4]), 4);
    assert_eq!(map_status(&[4, 3]), 3);
    assert_eq!(map_status(&[3, 2, 4]), 2);
    assert_eq!(map_status(&[2, 1, 4]), 1, "a failure wins at once");
    // Someone loading holds a follower at 4; the leader's tail makes it 9
    // anyway (0x19a4e5), as the game does.
    let mut g = lobby(&[4, 2]);
    assert_eq!(expected(&facts(&g).unwrap()), READY);
    g.put_u32(S + PEER, 1);
    assert_eq!(expected(&facts(&g).unwrap()), 4);
    let mut g = lobby(&[4, 1]);
    assert_eq!(expected(&facts(&g).unwrap()), 5, "a peer failed");
    g.put_u32(MAP_FAILED_HERE, 1);
    assert_eq!(expected(&facts(&g).unwrap()), 6, "this console failed");
}

#[test]
fn no_session_or_a_session_out_of_the_lobby_is_state_0_and_a_wrong_type_1() {
    let mut g = lobby(&[4]);
    g.put_u32(S + STATE, 2);
    assert_eq!(expected(&facts(&g).unwrap()), 0);
    g.put_u32(S + STATE, 5).put_u32(SESSION, 0);
    let f = facts(&g).unwrap();
    assert_eq!(f.state, None);
    assert_eq!(expected(&f), 0);
    assert!(blocked_by(0, &f).starts_with("no squad session"));
    let mut g = lobby(&[4]);
    g.put_u8(NETWORKING, 0);
    assert_eq!(expected(&facts(&g).unwrap()), 0);
    let mut g = lobby(&[4]);
    g.put_u32(SESSION_TYPE, 5);
    let f = facts(&g).unwrap();
    assert_eq!(expected(&f), 1);
    assert_eq!(
        blocked_by(1, &f),
        "the session is not a pregame lobby (type 5)"
    );
}

#[test]
fn a_live_or_system_link_custom_game_lobby_is_a_pregame_lobby() {
    // Session types 1 and 2 (0x19a279 makes both 3, what 0x250b1e tests).
    for ty in [1, 2] {
        let mut g = lobby(&[4]);
        g.put_u32(SESSION_TYPE, ty);
        assert!(in_pregame(&g), "type {ty}");
    }
    // Every lobby state the start checks take (0x58d70: 3 to 8).
    for state in 3..=8 {
        let mut g = lobby(&[4]);
        g.put_u32(S + STATE, state);
        assert!(in_pregame(&g), "state {state}");
    }
}

#[test]
fn no_session_another_type_or_a_session_out_of_the_lobby_is_no_pregame_lobby() {
    let mut g = lobby(&[4]);
    g.put_u32(SESSION, 0);
    assert!(!in_pregame(&g), "no session");
    let mut g = lobby(&[4]);
    g.put_u32(S + STATE, 2);
    assert!(!in_pregame(&g), "a session still being made");
    let mut g = lobby(&[4]);
    g.put_u32(S + STATE, 9);
    assert!(!in_pregame(&g), "a session past the lobby states");
    for ty in [0, 3, 4, 5, 6] {
        let mut g = lobby(&[4]);
        g.put_u32(SESSION_TYPE, ty);
        assert!(!in_pregame(&g), "type {ty}");
    }
    let mut g = lobby(&[4]);
    g.put_u8(NETWORKING, 0);
    assert!(!in_pregame(&g), "networking off");
    let mut g = lobby(&[4]);
    g.put_u8(LOBBY_ON, 0);
    assert!(!in_pregame(&g), "no lobby");
    assert!(
        !in_pregame(&FakeGuest::new()),
        "nothing mapped: a fault is no"
    );
}

#[test]
fn the_lobby_facts_line_says_each_fact() {
    let f = facts(&lobby(&[4])).unwrap();
    assert_eq!(
        pregame_line(&f),
        "bots: lobby facts: lobby true, networking true, session type 2, session state 5: a pregame lobby"
    );
    let mut g = lobby(&[4]);
    g.put_u32(SESSION, 0);
    assert_eq!(
        pregame_line(&facts(&g).unwrap()),
        "bots: lobby facts: lobby true, networking true, session type 2, session state none: no pregame lobby"
    );
}

#[test]
fn a_countdown_is_state_10() {
    let mut g = lobby(&[4]);
    g.put_u8(S + COUNTDOWN, 1).put_u32(S + COUNTDOWN + 4, 5);
    let f = facts(&g).unwrap();
    assert_eq!(f.countdown, Some(5));
    assert_eq!(expected(&f), COUNTING_DOWN);
    assert_eq!(
        blocked_by(COUNTING_DOWN, &f),
        "nothing (the countdown runs: 5)"
    );
}

fn matchmaking_host_lobby() -> FakeGuest {
    let mut g = lobby(&[4]);
    g.put_u32(SESSION_TYPE, 6)
        .put_u32(TARGET_SESSION, T)
        .map(T, 0x7500)
        .put_u32(T + STATE, 5)
        .put_u32(T + HOST_PEER, 0)
        .put_u32(T + PEER, 0);
    g
}

#[test]
fn the_game_session_is_the_target_session_in_matchmaking() {
    // 0x65340's table (0x65520): types 1 to 4 the squad session, 6 to 9
    // the target session.
    for ty in [0, 1, 2, 3, 4, 5] {
        assert_eq!(game_session_at(ty), SESSION, "type {ty}");
    }
    for ty in 6..=9 {
        assert_eq!(game_session_at(ty), TARGET_SESSION, "type {ty}");
    }
    let g = matchmaking_host_lobby();
    assert_eq!(game_session(&g), Ok(T));
    assert_eq!(game_session(&lobby(&[4])), Ok(S));
    // Networking off: the life cycle reads as none, the squad session.
    let mut g = matchmaking_host_lobby();
    g.put_u8(NETWORKING, 0);
    assert_eq!(game_session(&g), Ok(S));
}

#[test]
fn a_matchmaking_lobby_this_console_hosts_takes_bots() {
    let g = matchmaking_host_lobby();
    let f = facts(&g).unwrap();
    assert!(!pregame(&f), "not a pregame lobby: type 6");
    assert!(matchmaking_host(&f));
    assert!(joinable(&f));
    assert!(in_joinable(&g));
    assert!(!in_pregame(&g));
    assert_eq!(
        pregame_line(&f),
        "bots: lobby facts: lobby true, networking true, session type 6, session state 5, \
         match session state 5 (this console hosts it): a matchmaking lobby this console hosts"
    );
    // Type 7 too (0x19a279 gives 6 and 7 both 2), and every lobby state.
    let mut g = matchmaking_host_lobby();
    g.put_u32(SESSION_TYPE, 7);
    assert!(in_joinable(&g));
    for state in 3..=8 {
        let mut g = matchmaking_host_lobby();
        g.put_u32(T + STATE, state);
        assert!(in_joinable(&g), "state {state}");
    }
    // A custom lobby is still one.
    assert!(in_joinable(&lobby(&[4])));
}

#[test]
fn a_matchmaking_lobby_another_console_hosts_or_its_game_takes_none() {
    // Joined someone's match: their lobby, not ours to fill.
    let mut g = matchmaking_host_lobby();
    g.put_u32(T + HOST_PEER, 1);
    let f = facts(&g).unwrap();
    assert!(!joinable(&f));
    assert_eq!(
        pregame_line(&f),
        "bots: lobby facts: lobby true, networking true, session type 6, session state 5, \
         match session state 5 (another console hosts it): no pregame lobby"
    );
    // No target session yet (the search runs).
    let mut g = matchmaking_host_lobby();
    g.put_u32(TARGET_SESSION, 0);
    let f = facts(&g).unwrap();
    assert!(!joinable(&f));
    assert!(pregame_line(&f).contains("match session state none"));
    // The target session out of the lobby states.
    for state in [0, 2, 9] {
        let mut g = matchmaking_host_lobby();
        g.put_u32(T + STATE, state);
        assert!(!in_joinable(&g), "state {state}");
    }
    // The matchmade game (8) and after it (9): not a lobby.
    for ty in [8, 9] {
        let mut g = matchmaking_host_lobby();
        g.put_u32(SESSION_TYPE, ty);
        assert!(!in_joinable(&g), "type {ty}");
    }
    let mut g = matchmaking_host_lobby();
    g.put_u8(LOBBY_ON, 0);
    assert!(!in_joinable(&g), "no lobby");
}

#[test]
fn a_matchmaking_lobby_keeps_the_start_states_reading_of_the_squad() {
    // 0x19a2ce reads the squad session (0x59670) and gives 1 for any type
    // but 1 and 2: the matchmaking lobby has no Start.
    let f = facts(&matchmaking_host_lobby()).unwrap();
    assert_eq!(f.state, Some(5));
    assert_eq!(expected(&f), 1);
}

#[test]
fn the_lobbys_players_are_the_game_sessions() {
    let mut g = matchmaking_host_lobby();
    g.put_u32(T + PLAYER_COUNT, 4).put_u32(S + PLAYER_COUNT, 1);
    let f = facts(&g).unwrap();
    assert_eq!(
        lobby_players(&g, &f),
        Ok(Some(4)),
        "the target's, not the party's"
    );
    assert_eq!(
        players_line(4),
        "bots: lobby players: 4 in the game's session"
    );
    let mut g = lobby(&[4]);
    g.put_u32(S + PLAYER_COUNT, 2);
    let f = facts(&g).unwrap();
    assert_eq!(lobby_players(&g, &f), Ok(Some(2)));
    // Out of a joinable lobby: none.
    let mut g = matchmaking_host_lobby();
    g.put_u32(T + HOST_PEER, 1);
    let f = facts(&g).unwrap();
    assert_eq!(lobby_players(&g, &f), Ok(None));
}

#[test]
fn the_party_is_the_squad_sessions_players_in_a_match_this_console_hosts() {
    // A party of two (two consoles, or a console and its guest) and two
    // phantoms in the target: the party's count is the squad's.
    let mut g = matchmaking_host_lobby();
    g.put_u32(S + PLAYER_COUNT, 2).put_u32(T + PLAYER_COUNT, 4);
    let f = membership(&g).unwrap();
    assert_eq!(party_players(&g, &f), Ok(Some(2)));
    // Alone.
    g.put_u32(S + PLAYER_COUNT, 1);
    assert_eq!(party_players(&g, &f), Ok(Some(1)));
    // An unread or nonsense count is none, so the caller keeps one human.
    g.put_u32(S + PLAYER_COUNT, 0);
    assert_eq!(party_players(&g, &f), Ok(None));
    g.put_u32(S + PLAYER_COUNT, 40);
    assert_eq!(party_players(&g, &f), Ok(None));
    // A match another console hosts, and a custom game's lobby: none.
    let mut g = matchmaking_host_lobby();
    g.put_u32(S + PLAYER_COUNT, 2).put_u32(T + HOST_PEER, 1);
    let f = membership(&g).unwrap();
    assert_eq!(party_players(&g, &f), Ok(None));
    let mut g = lobby(&[4]);
    g.put_u32(S + PLAYER_COUNT, 2);
    let f = membership(&g).unwrap();
    assert_eq!(party_players(&g, &f), Ok(None));
}
