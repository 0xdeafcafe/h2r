//! `next_round` on the host (docs/game/rules-engine.md, "The engine API"):
//! the call at the boundary goes to the engine's `ROUND_END` while a round
//! is in play, is refused by name otherwise, and the round reset that
//! follows clears the VM's round state before `on round_start`.

use super::*;

/// The engine globals' round state in force and asked for.
const ROUND_STATE: u32 = 0x6c;
const ROUND_ASKED: u32 = 0xc04;

const ROUNDS: &str = "rules \"Rounds\" 1\ntype slayer\nrequires game.rounds\n\
                      global var starts: int = 0\n\
                      on round_start { starts = starts + 1 }\n\
                      on kill(victim, killer) { next_round(none) }\n";

/// A runtime running [`ROUNDS`] on a Slayer record whose round is in
/// state `state`, asked for `asked`.
fn rounds(store: &Local, state: u16, asked: u32) -> (Arc<Shared>, FakeGuest) {
    let ir = ::rules::compile(ROUNDS).unwrap();
    let blob = ir.encode();
    let id = ir.id();
    let s = shared(store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (mut m, _) = guest(block(prefix(&id), 0), SLAYER);
    m.put_u16(ENGINE + ROUND_STATE, state);
    m.put_u32(ENGINE + ROUND_ASKED, asked);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    (s, m)
}

#[test]
fn next_round_is_hosted_and_ends_the_round_through_the_engine() {
    assert!(ACTIONS.contains(&id::NEXT_ROUND));
    let ir = ::rules::compile(ROUNDS).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let store = Local::default();
    let (s, m) = rounds(&store, 1, 1);
    s.lock().kill(&m, 1, Some(0));
    assert_eq!(s.lock().running().unwrap().pending(), [Call::NextRound]);
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
            GuestCall::new(points::ROUND_END.base).args([0]),
            GuestCall::new(points::GAME_TICK.base),
        ]
    );
    assert!(s.lock().state().errors.is_empty(), "{:?}", s.lock().state().errors);
    assert!(s.lock().state().running, "the game goes on");
}

#[test]
fn next_round_is_refused_by_name_while_the_round_is_already_ending() {
    let store = Local::default();
    let (s, m) = rounds(&store, 1, 2);
    s.lock().kill(&m, 1, Some(0));
    let work = s.lock().pre_tick(&m).unwrap();
    let calls = drive(&s, m, work, true);
    assert!(calls.iter().all(|c| c.target != points::ROUND_END.base), "{calls:?}");
    let errors = s.lock().state().errors;
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].call, "next_round");
    assert!(errors[0].error.contains("the round is already ending"), "{errors:?}");
}

#[test]
fn the_round_reset_ends_the_vms_round_then_starts_the_next() {
    // The first round sets team 0's objective line; the next sets none.
    let src = "rules \"Lines\" 1\ntype slayer\nrequires hud.objective\n\
               global var starts: int = 0\n\
               on round_start {\n if starts == 0 { objective(team 0, \"First\") }\n starts = starts + 1\n}\n";
    let ir = ::rules::compile(src).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let blob = ir.encode();
    let id = ir.id();
    let store = Local::default();
    let s = shared(&store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (m, _) = guest(block(prefix(&id), 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let line_of = |s: &Arc<Shared>| s.lock().running().unwrap().vm.ext().objectives[0].text;
    assert!(line_of(&s).is_some());
    s.lock().round_start(&m);
    assert_eq!(s.lock().running().unwrap().vm.global("starts"), Some(2));
    assert_eq!(line_of(&s), None, "the round's end took the first round's line");
}

#[test]
fn each_rounds_start_logs_the_scores_the_game_holds_for_the_sim_report() {
    let store = Local::default();
    let (s, mut m) = rounds(&store, 1, 1);
    // Player 1's statborg score, as the game reads it for `player_score`.
    m.put_u16(ENGINE + 0x308 + 0x1c, (-4i16) as u16);
    s.lock().round_start(&m);
    m.put_u16(ENGINE + 0x308 + 0x1c, 7);
    s.lock().round_start(&m);
    let log = crate::sim_rounds::take();
    // `start` took the first round's (all zero, before the edit), then the
    // two later rounds' as the game held them.
    assert_eq!(log.len(), 3, "{log:?}");
    let score_of = |i: usize| log[i].players.iter().find(|p| p.0 == 1).map(|p| p.2);
    assert_eq!([score_of(0), score_of(1), score_of(2)], [Some(0), Some(-4), Some(7)]);
    assert_eq!(log[0].teams.iter().map(|t| t.0).collect::<Vec<_>>(), [0, 1]);
    assert!(log.iter().all(|s| s.players.len() == 2), "{log:?}");
}

/// The engine's round reset zeroes each statborg row's round score and adds
/// it into the row's earlier rounds' score (`0x1e97b0`), and the game's end
/// adds it in and keeps it. `player_score` and team score are the game's:
/// a two-round Siege whose defenders hold both reads 1 each, through the
/// reset and after the end, not the round's 0.
#[test]
fn player_and_team_scores_are_the_games_across_the_round_reset() {
    const ROW: u32 = ENGINE + 0x308;
    const TEAM: u32 = ENGINE + 0x4c8;
    let (mut m, _) = guest([0; LEN], SLAYER);
    let read = |m: &FakeGuest| {
        (
            guest::player_score(m, 0).unwrap(),
            guest::player_score(m, 1).unwrap(),
            guest::team_score(m, 0).unwrap(),
            guest::team_score(m, 1).unwrap(),
        )
    };
    // Round 1: team 1 (player 1) held; `SCORE_ADD` wrote the round's rows.
    m.put_u16(ROW + 0x1c, 1).put_u16(TEAM + 0x12, 1);
    assert_eq!(read(&m), (0, 1, 0, 1));
    // The round reset: the round's score moved into the earlier rounds'.
    m.put_u16(ROW + 0x1c, 0).put_u16(ROW + 0x1c + 2, 1);
    m.put_u16(TEAM + 0x12, 0).put_u16(TEAM + 0x12 + 2, 1);
    assert_eq!(read(&m), (0, 1, 0, 1), "the reset keeps the game's score");
    // Round 2: team 0 (player 0) held.
    m.put_u16(ROW, 1).put_u16(TEAM, 1);
    assert_eq!(read(&m), (1, 1, 1, 1), "one round each: a draw");
    // The game's end: the round's score added in and kept, counted once.
    m.put_u16(ROW + 2, 1).put_u16(TEAM + 2, 1);
    m.put_u8(ENGINE + 0xc08, 1);
    assert_eq!(read(&m), (1, 1, 1, 1));
}

/// The same team winning both rounds leads by two.
#[test]
fn a_team_that_wins_both_rounds_leads_the_game() {
    const TEAM: u32 = ENGINE + 0x4c8;
    let (mut m, _) = guest([0; LEN], SLAYER);
    m.put_u16(TEAM, 1);
    // The reset, then the second round's point.
    m.put_u16(TEAM, 0).put_u16(TEAM + 2, 1);
    m.put_u16(TEAM, 1);
    assert_eq!(guest::team_score(&m, 0).unwrap(), 2);
    assert_eq!(guest::team_score(&m, 1).unwrap(), 0);
}
