use super::*;
use guest_layout::entry::{self, EntryFuture, Regs, Ret};
use guest_layout::testing::FakeGuest;

const STACK: u32 = 0x0030_0000;
const G: u32 = 0x0060_0000;
const E: u32 = 0x0061_0000;
/// The game engine object, its vtable and its state virtual (`+0x90`).
const OBJ: u32 = 0x0062_0000;
const VT: u32 = 0x0062_0100;
const VCALL: u32 = 0x0015_8000;

struct Case {
    engine: u32,
    mode: u8,
    over: u8,
    state: i16,
    round: i16,
    countdown: u32,
    ending: u32,
    ended: u8,
    show: u8,
    held: u8,
}

const BASE: Case =
    Case { engine: 1, mode: 1, over: 0, state: 3, round: 0, countdown: 1, ending: 1, ended: 0, show: 1, held: 0 };

fn guest(c: &Case) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000).map(G, 0x1000).map(E, 0x2000).map(0x004e_6000, 0x4000).map(0x0051_0000, 0x1000);
    g.map(ENGINES, 0x40);
    g.map(OBJ, 0x200).put_u32(OBJ, VT).put_u32(VT + 0x90, VCALL);
    let engine = if c.engine != 0 { OBJ } else { 0 };
    g.put_u32(GAME, G).put_u32(ENGINE, E).put_u32(G + 0xc14, 2).put_u32(ENGINES + 8, engine);
    // No game object: the round step's last call does not run.
    g.put_u32(G + 0x24, u32::MAX);
    // The game time the round step reads.
    g.put_u32(crate::round_state::TIME, E + 0x1800);
    g.put_u8(E + 0xc, c.mode).put_u8(E + OVER, c.over);
    g.put_u16(G + STATE, c.state as u16).put_u16(G + ROUND, c.round as u16);
    g.put_u32(G + COUNTDOWN, c.countdown).put_u32(G + ENDING, c.ending).put_u8(G + ENDED, c.ended);
    g.put_u8(SHOW, c.show).put_u8(HELD, c.held).put_u8(QUIET, 7);
    g
}

fn tick(g: FakeGuest) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            run(&cx, 0x40).await?;
            Ok(Ret::new(0, 0))
        })
    };
    entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default())
}

fn targets(c: &Case) -> (Vec<u32>, FakeGuest) {
    let r = tick(guest(c));
    assert!(r.result.is_ok());
    (r.calls.iter().map(|c| c.target).collect(), r.guest)
}

#[test]
fn no_engine_or_a_client_does_nothing() {
    for c in [Case { engine: 0, ..BASE }, Case { mode: CLIENT, ..BASE }] {
        let (t, g) = targets(&c);
        assert!(t.is_empty());
        assert_eq!(g.u32(G + COUNTDOWN).unwrap(), 1);
    }
}

#[test]
fn state_three_at_zero_changes_state_and_marks_the_game() {
    // The next state is the state: the round step returns at once.
    let (t, g) = targets(&Case { ending: 3, ..BASE });
    assert_eq!(t, [STATE_CHANGE]);
    assert_eq!(g.u32(G + COUNTDOWN).unwrap(), 0);
    assert_eq!(g.u32(G).unwrap() & 0x20, 0x20);
}

#[test]
fn state_two_picks_by_round() {
    assert_eq!(targets(&Case { state: 2, ending: 2, ..BASE }).0, [STATE_CHANGE]);
    assert_eq!(targets(&Case { state: 2, ending: 2, round: LAST_ROUND, ..BASE }).0, [LAST]);
}

#[test]
fn a_running_countdown_or_other_state_only_closes() {
    let (t, g) = targets(&Case { countdown: 5, ending: 3, ..BASE });
    assert!(t.is_empty());
    assert_eq!(g.u32(G + COUNTDOWN).unwrap(), 4);
    let (t, g) = targets(&Case { state: 1, ..BASE });
    assert!(t.is_empty());
    assert_eq!(g.u32(G + COUNTDOWN).unwrap(), 1, "only states 2 and 3 count down");
}

#[test]
fn the_end_runs_once() {
    let over = Case { over: 1, state: 1, ..BASE };
    let (t, g) = targets(&over);
    assert_eq!(t, [END, SCORE, SCORED, FINISH, REQUEST, POINTER, QUIET_A, QUIET_B, LOUD]);
    assert_eq!((g.u8(G + ENDED).unwrap(), g.u8(QUIET).unwrap()), (1, 1));
    assert!(targets(&Case { ended: 1, ..over }).0.is_empty());
}

#[test]
fn the_end_closes_when_not_ending_and_skips_the_show_when_held() {
    let (t, _) = targets(&Case { over: 1, state: 1, ending: 0, held: 1, ..BASE });
    // The round step moves state 1 to 0: the engine's state virtual.
    assert_eq!(t, [END, VCALL, FINISH, REQUEST]);
    let (t, _) = targets(&Case { over: 1, state: 0, show: 0, ..BASE });
    assert_eq!(t, [END, FINISH, REQUEST]);
}

#[test]
fn a_state_change_runs_the_round_step() {
    // State 3, next 1: the next round begins.
    let (t, g) = targets(&Case { countdown: 5, ..BASE });
    use crate::round_state::{NEXT_PLAYER, POINTER as MARK, ROUND_RESET};
    assert_eq!(t, [ROUND_RESET, MARK, NEXT_PLAYER, VCALL]);
    assert_eq!(g.u16(G + STATE).unwrap(), 1);
}

#[test]
fn the_request_is_built_on_the_frame() {
    let r = tick(guest(&Case { over: 1, state: 0, show: 0, ..BASE }));
    let req = r.calls.iter().find(|c| c.target == REQUEST).unwrap();
    let at = req.eax.unwrap();
    assert_eq!(at, STACK - 0x40 - 0x28);
    for (o, v) in REQUEST_WORDS {
        assert_eq!(r.guest.u32(at + o).unwrap(), v, "{o:#x}");
    }
    assert_eq!(r.guest.u16(at + 0x20).unwrap(), 0xffff);
    assert_eq!(req.ecx, Some(0));
}
