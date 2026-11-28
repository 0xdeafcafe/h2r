use super::*;
use guest_layout::entry::{self, EntryFuture, Regs, Ret};
use guest_layout::testing::FakeGuest;

const STACK: u32 = 0x0030_0000;
const HEADER: u32 = 0x1000;
const DATA: u32 = 0x2000;
const NONE: u32 = u32::MAX;

fn handle(salt: u16, index: u32) -> u32 {
    u32::from(salt) << 16 | index
}

fn body_at(index: u32) -> u32 {
    0x4000 + index * 0x100
}

/// A pool of `(salt, flags, parent)` slots, salt 0 for a dead slot.
fn pool(slots: &[(u16, u32, u32)]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000);
    g.put_u32(points::OBJECTS.base, HEADER);
    g.put_u32(HEADER + layout::HIGH_WATER, slots.len() as u32);
    g.put_u32(HEADER + layout::DATA, DATA);
    for (i, &(salt, flags, parent)) in slots.iter().enumerate() {
        let i = i as u32;
        let slot = DATA + i * SLOT;
        g.put_u16(slot, salt).put_u8(slot + 3, 2).put_u32(slot + 8, body_at(i));
        g.put_u32(body_at(i) + BODY_FLAGS, flags).put_u32(body_at(i) + PARENT, parent);
    }
    g
}

/// `0x000bd090` as the tests see it: writes a matrix word that depends
/// only on the object, and clears bit 30.
fn build(c: &entry::GuestCall, m: &mut dyn GuestMemory) -> Regs {
    assert_eq!(c.target, points::OBJECT_NODE_MATRICES.base);
    let index = c.args[0] & 0xffff;
    let at = body_at(index);
    let flags = m.u32(at + BODY_FLAGS).unwrap();
    m.set_u32(at + BODY_FLAGS, flags & !DIRTY).unwrap();
    m.set_u32(at + 0x200, 0x3f80_0000 | index).unwrap();
    Regs::default()
}

fn phase(g: FakeGuest, callee: impl FnMut(&entry::GuestCall, &mut dyn GuestMemory) -> Regs) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            run(&cx, 0x4048).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let r = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, callee);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    r
}

#[test]
fn only_bit_30_without_bit_29_is_pending() {
    assert!(pending(DIRTY));
    assert!(pending(DIRTY | 1 << 8));
    assert!(!pending(DIRTY | HOLD));
    assert!(!pending(HOLD));
    assert!(!pending(0));
}

#[test]
fn the_plan_is_index_order_skipping_dead_held_and_clean_objects() {
    let g = pool(&[(1, DIRTY, NONE), (2, DIRTY | HOLD, NONE), (0, DIRTY, NONE), (4, 0, NONE), (5, DIRTY, NONE)]);
    assert_eq!(plan(&g), Some(vec![handle(1, 0), handle(5, 4)]));
}

#[test]
fn a_pending_parent_is_built_before_its_child_and_once() {
    // Slot 0 is attached to slot 2, slot 1 to slot 0: build 2, 0, 1.
    let g = pool(&[(1, DIRTY, handle(3, 2)), (2, DIRTY, handle(1, 0)), (3, DIRTY, NONE)]);
    assert_eq!(plan(&g), Some(vec![handle(3, 2), handle(1, 0), handle(2, 1)]));
}

#[test]
fn a_clean_or_stale_parent_does_not_join_the_plan() {
    // Slot 1's parent is clean; slot 2's parent handle has a stale salt.
    let g = pool(&[(1, 0, NONE), (2, DIRTY, handle(1, 0)), (3, DIRTY, handle(9, 1))]);
    assert_eq!(plan(&g), Some(vec![handle(2, 1), handle(3, 2)]));
}

#[test]
fn a_parent_loop_ends() {
    let g = pool(&[(1, DIRTY, handle(2, 1)), (2, DIRTY, handle(1, 0))]);
    assert_eq!(plan(&g), Some(vec![handle(2, 1), handle(1, 0)]));
}

#[test]
fn no_pool_is_no_plan_and_no_call() {
    let mut g = FakeGuest::new();
    g.map(STACK - 0x8000, 0x10000);
    assert_eq!(plan(&g), None);
    assert!(phase(g, build).calls.is_empty());
}

/// The coupling's proof in the tick's terms: which objects a view listed
/// is not an input. Every pending object is built at tick end with the
/// object as `0x000bd090`'s one stack argument, below the driver's
/// reserve, and leaves the phase clean, so the frame step finds no dirty
/// pose to evaluate.
#[test]
fn every_pending_object_is_built_at_tick_end_whatever_the_view_listed() {
    let g = pool(&[(1, DIRTY, NONE), (2, 0, NONE), (3, DIRTY | 1 << 8, NONE)]);
    let before = stats();
    let r = phase(g, build);
    let got: Vec<_> = r.calls.iter().map(|c| (c.target, c.args.clone(), c.reserve)).collect();
    let bd090 = points::OBJECT_NODE_MATRICES.base;
    assert_eq!(got, [(bd090, vec![handle(1, 0)], 0x4048), (bd090, vec![handle(3, 2)], 0x4048)]);
    for i in [0, 2] {
        assert_eq!(r.guest.u32(body_at(i) + BODY_FLAGS).unwrap() & DIRTY, 0);
        assert_eq!(r.guest.u32(body_at(i) + 0x200).unwrap(), 0x3f80_0000 | i);
    }
    assert_eq!(r.guest.u32(body_at(2) + BODY_FLAGS).unwrap(), 1 << 8, "only bit 30 changes");
    assert_eq!(plan(&r.guest), Some(vec![]), "a second phase builds nothing");
    let after = stats();
    assert!(after.built >= before.built + 2 && after.phases > before.phases);
}

#[test]
fn an_object_a_build_already_cleaned_is_not_built_again() {
    // The parent's build also cleans the child (as a callee could).
    let g = pool(&[(1, DIRTY, NONE), (2, DIRTY, NONE)]);
    let r = phase(g, |c, m| {
        let regs = build(c, m);
        let child = body_at(1) + BODY_FLAGS;
        let flags = m.u32(child).unwrap();
        m.set_u32(child, flags & !DIRTY).unwrap();
        regs
    });
    assert_eq!(r.calls.len(), 1);
}

#[test]
fn a_build_that_leaves_bit_30_is_counted() {
    let g = pool(&[(1, DIRTY, NONE)]);
    let before = stats().left_dirty;
    let r = phase(g, |_, _| Regs::default());
    assert_eq!(r.calls.len(), 1);
    assert!(stats().left_dirty > before);
}
