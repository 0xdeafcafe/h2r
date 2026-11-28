use super::*;
use guest_layout::entry::{self, EntryFuture, Regs, Ret};
use guest_layout::testing::FakeGuest;

const STACK: u32 = 0x0030_0000;
const RESERVE: u32 = 0x40;
const G: u32 = 0x0060_0000;
const E: u32 = 0x0061_0000;
const T: u32 = 0x0061_1000;
const OBJ: u32 = 0x0062_0000;
const VT: u32 = 0x0062_0100;
const VCALL: u32 = 0x0015_8000;
const PLAYER: u32 = 0x0063_0000;
const MARKED: u32 = 0x0064_0000;

fn guest(state: i16, next: u32, mode: u8) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000).map(G, 0x1000).map(E, 0x2000).map(OBJ, 0x200);
    g.map(0x004e_6000, 0x4000).map(0x0051_0000, 0x1000).map(ENGINES, 0x40).map(PLAYER, 0x400).map(MARKED, 4);
    g.put_u32(GAME, G).put_u32(ENGINE, E).put_u32(TIME, T).put_u32(PLAYERS, 0x00aa_0000);
    g.put_u32(G + INDEX, 2).put_u32(ENGINES + 8, OBJ).put_u32(OBJ, VT).put_u32(VT + VIRTUAL, VCALL);
    g.put_u16(G + STATE, state as u16).put_u32(G + NEXT, next).put_u16(G + ROUND, 4);
    g.put_u32(G + OBJECT, 0x00bb_0001).put_u8(E + 0xc, mode).put_u32(E + 0x1b8, 0);
    g.put_u16(T + 2, 30).put_u32(T + 8, 777);
    g.put_u8(SHOW, 1);
    // Player 3 drives unit 0x00cc_0003.
    g.put_u32(G + UNITS + 4 * 3, 0x00cc_0003).put_u8(PLAYER + 2, 1);
    g
}

/// Runs the step; `162030` answers [`MARKED`] and the iterator yields
/// player 3 once.
fn step(g: FakeGuest, ebx: Option<u32>) -> entry::testing::Run {
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            run(&cx, RESERVE, G, ebx).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let mut yielded = false;
    entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, move |c, m| match c.target {
        POINTER => Regs { eax: MARKED, ..Default::default() },
        NEXT_PLAYER if !yielded => {
            yielded = true;
            let iter = c.esi.unwrap();
            m.set_u32(iter, PLAYER).unwrap();
            m.set_u32(iter + 8, 0x8003 << 16 | 3).unwrap();
            Regs { eax: 1, ..Default::default() }
        }
        _ => Regs::default(),
    })
}

fn targets(r: &entry::testing::Run) -> Vec<u32> {
    r.calls.iter().map(|c| c.target).collect()
}

#[test]
fn no_change_makes_no_call() {
    for (state, next) in [(1i16, 1u32), (-1, u32::MAX)] {
        let r = step(guest(state, next, 1), None);
        assert!(r.result.is_ok());
        assert!(r.calls.is_empty());
    }
}

#[test]
fn next_three_ends_the_round() {
    let r = step(guest(1, 3, 1), Some(1));
    assert_eq!(targets(&r), [END, VCALL, OBJECT_FLAGS]);
    assert_eq!(r.calls[0].args, [8]);
    assert_eq!(r.calls[0].ebx, Some(1));
    assert_eq!(r.guest.u16(G + ROUND).unwrap(), 5);
    assert_eq!(r.guest.u32(G + COUNTDOWN).unwrap(), 30);
    assert_eq!(r.guest.u16(G + STATE).unwrap(), 3);
    assert_eq!((r.calls[2].args.clone(), r.calls[2].edi), (vec![0x00bb_0001, 2], Some(G)));
}

#[test]
fn next_two_marks_and_counts_down_five_times() {
    let r = step(guest(3, 2, 1), None);
    assert_eq!(targets(&r), [POINTER, POINTER, VCALL, OBJECT_FLAGS]);
    assert_eq!(r.guest.u8(MARKED).unwrap(), 0);
    assert_eq!(r.guest.u8(QUIET).unwrap(), 1);
    assert_eq!(r.guest.u32(G + COUNTDOWN).unwrap(), 150);
    let r = step(guest(3, 2, CLIENT), None);
    assert_eq!(r.guest.u8(QUIET).unwrap(), 0, "a client leaves the screen loud");
}

#[test]
fn next_one_begins_a_round_and_walks_the_players() {
    let mut g = guest(3, 1, 1);
    g.put_u8(MARKED, 0);
    let r = step(g, None);
    assert_eq!(targets(&r), [ROUND_RESET, POINTER, POINTER, NEXT_PLAYER, OBJECT_FLAGS, NEXT_PLAYER, VCALL, OBJECT_FLAGS]);
    assert_eq!(r.guest.u8(MARKED).unwrap(), 1);
    assert_eq!(r.guest.u32(G + ROUND_PLAYED).unwrap(), 777);
    // The iterator is the frame's locals, as the guest builds it.
    let iter = STACK - RESERVE - ITERATOR;
    assert_eq!(r.calls[3].esi, Some(iter));
    assert_eq!(r.guest.u32(iter + 4).unwrap(), 0x00aa_0000);
    assert_eq!((r.calls[4].args.clone(), r.calls[4].esi), (vec![0x00cc_0003, 0x20], Some(PLAYER)));
    assert_eq!(r.guest.u16(PLAYER + 0x1ac).unwrap(), 0xffff, "no seat: -1");
    assert_eq!(r.guest.u8(PLAYER + 2).unwrap(), 9);
    assert_eq!(r.guest.u16(G + STATE).unwrap(), 1);
}

#[test]
fn a_client_walks_no_player_and_no_object_skips_the_last_call() {
    let mut g = guest(3, 1, CLIENT);
    g.put_u32(G + OBJECT, u32::MAX);
    let r = step(g, None);
    assert_eq!(targets(&r), [ROUND_RESET, POINTER, POINTER, VCALL]);
    assert_eq!(r.calls[3].ecx, Some(OBJ));
    assert_eq!(r.calls[3].args, [1]);
}
