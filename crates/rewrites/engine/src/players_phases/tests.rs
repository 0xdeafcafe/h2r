use super::*;
use guest_layout::entry::{self, EntryFuture, Regs, Ret};
use guest_layout::testing::FakeGuest;

const STACK: u32 = 0x0030_0000;
const GLOB: u32 = 0x0060_0000;
const PLAYER_ARRAY: u32 = 0x0061_0000;
const PLAYER_DATA: u32 = 0x0062_0000;
const FP: u32 = 0x0064_0000;
const OBJ_ARRAY: u32 = 0x0068_0000;
const OBJ_DATA: u32 = 0x0069_0000;
const OBJ: u32 = 0x006a_0000;
const UNIT_HANDLE: u32 = 0x1234_0002;

/// Users 0 and 2 mapped to players 1 and 0; the others `-1`.
fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000).map(0x004e_0000, 0x10000).map(GLOB, 0x1000).map(PLAYER_ARRAY, 0x100);
    g.map(PLAYER_DATA, 0x1000).map(FP, 4 * USER_SIZE).map(OBJ_ARRAY, 0x100).map(OBJ_DATA, 0x100).map(OBJ, 0x400);
    g.put_u32(GLOBALS, GLOB).put_u32(PLAYERS, PLAYER_ARRAY).put_u32(BLOCK, FP).put_u32(OBJECTS, OBJ_ARRAY);
    g.put_u32(PLAYER_ARRAY + 0x44, PLAYER_DATA).put_u32(OBJ_ARRAY + 0x44, OBJ_DATA);
    for u in 0..4 {
        g.put_u32(GLOB + 0xc + 4 * u, u32::MAX);
    }
    g.put_u32(GLOB + 0xc, 1).put_u32(GLOB + 0x14, 0);
    // Unit handle slot 2 -> OBJ.
    g.put_u32(OBJ_DATA + 2 * 12 + 8, OBJ);
    for p in [0, 1] {
        let at = PLAYER_DATA + p * PLAYER_SIZE;
        g.put_u8(at + SELECTED, 0xfe).put_u32(at + UNIT, UNIT_HANDLE);
    }
    g
}

/// The callee answers: selection 7, slot ids `0x40 + slot`.
fn answer(c: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
    let eax = match c.target {
        SELECTION => 7,
        SLOT_ID => 0x40 + (c.ecx.unwrap() & 1),
        REFRESH => {
            // The refresh sets the block's unit and selection.
            let user = c.args[0];
            m.set_u32(FP + user * USER_SIZE + 4, c.eax.unwrap()).unwrap();
            m.set_u32(FP + user * USER_SIZE + 8, c.args[1]).unwrap();
            0
        }
        _ => 0,
    };
    Regs { eax, ..Default::default() }
}

fn tick(g: FakeGuest) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            first_person(&cx, 0x40).await?;
            Ok(Ret::new(0, 0))
        })
    };
    entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, answer)
}

#[test]
fn unmapped_users_are_skipped() {
    let mut g = guest();
    g.put_u32(GLOB + 0xc, u32::MAX).put_u32(GLOB + 0x14, u32::MAX);
    let r = tick(g);
    assert!(r.result.is_ok());
    assert!(r.calls.is_empty());
}

#[test]
fn a_changed_block_is_refreshed_and_its_slots_checked() {
    let r = tick(guest());
    assert!(r.result.is_ok());
    let t: Vec<u32> = r.calls.iter().map(|c| c.target).collect();
    // User 0 (player 1): selection, refresh (the block is empty), then both
    // slots (ids 0x40, 0x41 against zero state) and the finish; user 2 the same.
    let one = [SELECTION, REFRESH, SLOT_ID, SLOT_REFRESH, SLOT_ID, SLOT_REFRESH, FINISH];
    assert_eq!(t, [one, one].concat());
    let refresh = &r.calls[1];
    assert_eq!((refresh.eax, refresh.args.clone()), (Some(UNIT_HANDLE), vec![0, 7]));
    let sel = &r.calls[0];
    assert_eq!(sel.eax, Some(0xffff_fffe), "the selection byte sign-extends");
    assert_eq!(r.calls[7 + 1].args, vec![2, 7], "user 2's refresh");
}

#[test]
fn a_current_block_is_not_refreshed() {
    let mut g = guest();
    g.put_u32(FP + 4, UNIT_HANDLE).put_u32(FP + 8, 7);
    g.put_u32(FP + SLOT_STATE, 0x40).put_u32(FP + SLOT_STATE + SLOT_STRIDE, 0x41);
    g.put_u32(GLOB + 0x14, u32::MAX);
    let r = tick(g);
    let t: Vec<u32> = r.calls.iter().map(|c| c.target).collect();
    assert_eq!(t, [SELECTION, SLOT_ID, SLOT_ID, FINISH]);
}

#[test]
fn a_block_left_with_no_unit_stops_before_the_slots() {
    let mut g = guest();
    g.put_u32(GLOB + 0x14, u32::MAX);
    // The player has no unit and the block has none: nothing to refresh
    // (unit -1 == -1) except the selection mismatch.
    g.put_u32(PLAYER_DATA + PLAYER_SIZE + UNIT, u32::MAX).put_u32(FP + 4, u32::MAX).put_u32(FP + 8, 0);
    let r = tick(g);
    let t: Vec<u32> = r.calls.iter().map(|c| c.target).collect();
    assert_eq!(t, [SELECTION, REFRESH]);
}

#[test]
fn the_slot_register_is_the_pointer_with_a_signed_low_word() {
    assert_eq!(slot_ecx(0x006a_0000, 0x01), 0x006a_0001);
    assert_eq!(slot_ecx(0x006a_1234, 0xff), 0x006a_ffff);
}
