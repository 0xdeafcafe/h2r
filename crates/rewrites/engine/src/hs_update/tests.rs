use super::*;
use crate::entry::{self, EntryFuture, Regs};
use crate::testing::FakeGuest;

const STACK: u32 = 0x0030_0000;
const TIME: u32 = 0x0052_0000;
const POOL: u32 = 0x0053_0000;
const DATA: u32 = 0x0053_1000;
const BITMAP: u32 = 0x0053_0800;
const TICK: i32 = 100;
/// The object lists pool the tail call sweeps: one unreferenced list.
const LISTS: u32 = 0x0054_0000;
const LIST_BITS: u32 = 0x0054_0800;
const LIST_DATA: u32 = 0x0054_1000;

/// A pool of `threads`: (kind byte, wake), slot by slot; a `None` slot is
/// free. Slot `i` has salt `0x8000 + i`.
fn guest(threads: &[Option<(u8, i32)>]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000).map(0x004f_9000, 0x1000).map(TIME, 0x1000);
    g.map(POOL, 0x1000).map(DATA, THREAD * threads.len() as u32 + 0x10);
    g.put_u32(crate::points::GAME_TIME.base, TIME).put_u32(TIME + 8, TICK as u32);
    g.put_u8(ENABLED, 1).put_u32(THREADS, POOL);
    g.put_u32(POOL + layout::ELEMENT_SIZE, THREAD)
        .put_u32(POOL + layout::HIGH_WATER, threads.len() as u32)
        .put_u32(POOL + layout::DATA, DATA)
        .put_u32(POOL + layout::BITMAP, BITMAP);
    let mut bits = 0u32;
    for (i, t) in threads.iter().enumerate() {
        let at = DATA + THREAD * i as u32;
        if let Some((kind, wake)) = *t {
            bits |= 1 << i;
            g.put_u16(at, 0x8000 + i as u16).put_u8(at + KIND, kind).put_u32(at + WAKE, wake as u32);
        }
    }
    g.put_u32(BITMAP, bits);
    g.map(0x004f_5000, 0x1000).map(LISTS, 0x2000).put_u32(crate::hs_object_lists::LISTS, LISTS);
    g.put_u32(LISTS + layout::ELEMENT_SIZE, 12)
        .put_u32(LISTS + layout::HIGH_WATER, 1)
        .put_u32(LISTS + layout::ACTIVE_COUNT, 1)
        .put_u32(LISTS + layout::DATA, LIST_DATA)
        .put_u32(LISTS + layout::BITMAP, LIST_BITS)
        .put_u32(LIST_BITS, 1)
        .put_u16(LIST_DATA, 0x9000)
        .put_u32(LIST_DATA + 8, u32::MAX);
    g
}

/// The sweep freed the unreferenced list.
fn swept(r: &entry::testing::Run) -> bool {
    r.guest.u16(LIST_DATA).unwrap() == 0 && r.guest.u32(LISTS + layout::ACTIVE_COUNT).unwrap() == 0
}

fn handle(slot: u32) -> u32 {
    (0x8000 + slot) << 16 | slot
}

fn run_on(g: FakeGuest, callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            run(&cx, 0x40).await?;
            Ok(crate::entry::Ret::new(0, 0))
        })
    };
    entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, callee)
}

fn calls(r: &entry::testing::Run) -> Vec<(u32, Option<u32>)> {
    r.calls.iter().map(|c| (c.target, c.edi)).collect()
}

#[test]
fn runnable_threads_run_in_slot_order_then_the_sweep() {
    let threads = [
        Some((0, TICK)),     // due now
        Some((EXTERNAL, 0)), // external: never run here
        None,                // free
        Some((0, TICK + 1)), // asleep
        Some((0, -1)),       // never
        Some((1, 0)),        // due
    ];
    let r = run_on(guest(&threads), |_, _| Regs::default());
    assert!(r.result.is_ok());
    assert_eq!(calls(&r), [(EXECUTE, Some(handle(0))), (EXECUTE, Some(handle(5)))]);
    assert!(swept(&r), "the tail call's sweep is Rust's, no guest call");
}

#[test]
fn scripts_off_make_no_call() {
    let mut g = guest(&[Some((0, 0))]);
    g.put_u8(ENABLED, 0);
    let r = run_on(g, |_, _| Regs::default());
    assert!(r.result.is_ok());
    assert!(r.calls.is_empty(), "no executor");
    assert!(!swept(&r), "and no tail call");
}

#[test]
fn a_thread_that_turns_scripts_off_ends_the_pass() {
    let r = run_on(guest(&[Some((0, 0)), Some((0, 0))]), |c, m| {
        if c.target == EXECUTE {
            m.set_u8(ENABLED, 0).unwrap();
        }
        Regs::default()
    });
    assert_eq!(calls(&r), [(EXECUTE, Some(handle(0)))]);
    assert!(swept(&r));
}

#[test]
fn the_pool_is_read_again_after_each_thread() {
    // The first thread wakes the second: it runs in the same pass.
    let r = run_on(guest(&[Some((0, 0)), Some((0, TICK + 50))]), |c, m| {
        if c.target == EXECUTE && c.edi == Some(handle(0)) {
            m.set_u32(DATA + THREAD + WAKE, 0).unwrap();
        }
        Regs::default()
    });
    assert_eq!(calls(&r), [(EXECUTE, Some(handle(0))), (EXECUTE, Some(handle(1)))]);
}
