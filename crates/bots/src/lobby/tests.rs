use super::*;
use crate::guest::{
    BUTTON_A, BUTTON_START, EVENT_BUTTON, EVENT_CONTROLLER, SLOT_PROFILE, SLOT_STRIDE, SLOTS, slot,
};
use engine::testing::FakeGuest;

const S: u32 = 0x0070_0000;
const EVENT: u32 = 0x0071_0000;

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

#[test]
fn the_start_line_names_the_check_and_the_bot_slots() {
    let f = facts(&lobby(&[4])).unwrap();
    assert_eq!(
        start_line(
            8,
            &f,
            &[
                "port 1 0x10 offline".into(),
                "port 2 0x30 Live guest 1".into()
            ]
        ),
        "bots: start state 8, blocked by the party leader check: this console does not lead \
         the party (session +0x72d8 is not +0x50); port 1 0x10 offline, port 2 0x30 Live guest 1"
    );
    assert!(
        start_line(7, &f, &[])
            .starts_with("bots: start state 7, blocked by the teams or co-op check")
    );
    assert_eq!(
        start_line(13, &f, &[]),
        "bots: start state 13, blocked by the matchmaking check (state 13)"
    );
}

#[test]
fn a_start_press_is_a_or_start_from_a_controller_with_a_profile() {
    let mut g = FakeGuest::new();
    g.map(SLOTS, 4 * SLOT_STRIDE).put_u32(slot(0), SLOT_PROFILE);
    g.put_u32(EVENT + EVENT_KIND, EVENT_BUTTON_PRESS)
        .put_u32(EVENT + EVENT_CONTROLLER, 0)
        .put_u32(EVENT + EVENT_BUTTON, BUTTON_START);
    assert_eq!(start_press(&g, EVENT).unwrap(), Some(0));
    g.put_u32(EVENT + EVENT_BUTTON, BUTTON_A);
    assert_eq!(start_press(&g, EVENT).unwrap(), Some(0));
    g.put_u32(EVENT + EVENT_BUTTON, 1);
    assert_eq!(start_press(&g, EVENT).unwrap(), None, "B");
    g.put_u32(EVENT + EVENT_BUTTON, BUTTON_A)
        .put_u32(EVENT + EVENT_CONTROLLER, 1);
    assert_eq!(
        start_press(&g, EVENT).unwrap(),
        None,
        "no profile on port 1"
    );
    g.put_u32(EVENT + EVENT_CONTROLLER, 0)
        .put_u32(EVENT + EVENT_KIND, 2);
    assert_eq!(start_press(&g, EVENT).unwrap(), None, "not a button");
}

#[test]
fn a_start_press_asks_the_game_its_state_then_runs_the_frame() {
    use engine::entry::{EntryFuture, Regs, testing};
    use std::sync::{Arc, Mutex};
    const THEN: u32 = 0x0012_5b40;
    let said = Arc::new(Mutex::new(String::new()));
    let out = Arc::clone(&said);
    let f = move |cx: Cx| -> EntryFuture {
        let out = Arc::clone(&out);
        Box::pin(async move {
            start_state_then(cx, vec![1, 2], THEN)
                .await
                .map(|(line, r)| {
                    *out.lock().unwrap() = line;
                    r
                })
        })
    };
    let mut g = lobby(&[4]);
    g.map(SLOTS, 4 * SLOT_STRIDE)
        .put_u32(slot(0), SLOT_PROFILE | crate::guest::SLOT_LIVE)
        .put_u32(slot(1), SLOT_PROFILE)
        .put_u32(slot(2), SLOT_PROFILE);
    g.map(0x0067_0700, 0x200);
    let regs = Regs {
        esp: 0x0067_0800,
        ..Regs::default()
    };
    let run = testing::run(&f, g, regs, |c, _| Regs {
        eax: match c.target {
            THEN => 0x77,
            _ => 8,
        },
        ..Regs::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0x77, 0)), "the frame's own return");
    let calls: Vec<(u32, Vec<u32>)> = run
        .calls
        .iter()
        .map(|c| (c.target, c.args.clone()))
        .collect();
    assert_eq!(
        calls,
        vec![(points::LOBBY_START_STATE.base, vec![0]), (THEN, vec![])],
        "the state with no progress out, then the frame"
    );
    assert_eq!(
        *said.lock().unwrap(),
        "bots: start state 8, blocked by the party leader check: this console does not lead \
         the party (session +0x72d8 is not +0x50); port 1 0x10 offline, port 2 0x10 offline"
    );
}

#[test]
fn a_on_the_start_game_row_from_a_controller_with_a_profile_is_a_start_press() {
    // The run 20261119-005001: the drive's A on Start Game reached the
    // list's row handler, never the screen's input handler.
    const ITEM: u32 = 0x0072_0000;
    const ROW: u32 = 0x0072_0100;
    let mut g = FakeGuest::new();
    g.map(SLOTS, 4 * SLOT_STRIDE).put_u32(slot(0), SLOT_PROFILE);
    g.put_u32(ITEM, EVENT)
        .put_u32(EVENT + EVENT_KIND, EVENT_BUTTON_PRESS)
        .put_u32(EVENT + EVENT_CONTROLLER, 0)
        .put_u32(EVENT + EVENT_BUTTON, BUTTON_A)
        .put_u16(ROW, ROW_START_GAME);
    assert_eq!(start_row_press(&g, ITEM, ROW).unwrap(), Some(0));
    g.put_u16(ROW, 1);
    assert_eq!(
        start_row_press(&g, ITEM, ROW).unwrap(),
        None,
        "the other row"
    );
    g.put_u16(ROW, ROW_START_GAME)
        .put_u32(EVENT + EVENT_CONTROLLER, 2);
    assert_eq!(
        start_row_press(&g, ITEM, ROW).unwrap(),
        None,
        "no profile on port 2"
    );
    g.put_u32(EVENT + EVENT_CONTROLLER, 7);
    assert_eq!(
        start_row_press(&g, ITEM, ROW).unwrap(),
        None,
        "no such port"
    );
}
