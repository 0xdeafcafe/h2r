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
