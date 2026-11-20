use super::*;
use crate::synth::{self, Pool};
use guest_layout::{Datum, ObjectType};

fn flags(p: &Pool, d: Datum) -> u8 {
    p.header(d.index()).flags(&p.ram).unwrap()
}

/// A frame in the pool's spare memory, above the tag data.
const S: Stack = Stack { esp: 0x0055_ff00, ebx: 0xb0b0_b0b0, esi: 0x5151_5151, edi: 0xd1d1_d1d1 };

fn stack(p: &Pool, addr: u32) -> u32 {
    Off::<u32>::new(0).read(&p.ram, addr).unwrap()
}

fn body(d: Datum) -> u32 {
    synth::BODIES + u32::from(d.index()) * synth::BODY_SIZE
}

#[test]
fn wake_sets_awake_up_the_whole_parent_chain() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Vehicle, header::ACTIVE);
    // An awake ancestor does not stop the walk: its parent is woken too.
    let b = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE | header::AWAKE);
    let c = p.add(3, 0x8003, ObjectType::Weapon, header::ACTIVE);
    p.attach(c, b);
    p.attach(b, a);
    let out = wake(&mut p.ram, c.raw(), S).unwrap();
    for d in [a, b, c] {
        assert_ne!(flags(&p, d) & header::AWAKE, 0, "{d:?}");
    }
    assert_eq!(
        out,
        Exit { eax: NONE, ecx: synth::DATA, edx: 3 * 3, flags: Flags::NoneCompared }
    );
}

#[test]
fn wake_leaves_an_awake_object_and_none_alone() {
    let mut p = Pool::new();
    let a = p.add(4, 0x8004, ObjectType::Scenery, header::ACTIVE);
    let b = p.add(5, 0x8005, ObjectType::Scenery, header::ACTIVE | header::AWAKE);
    p.attach(b, a);
    let out = wake(&mut p.ram, b.raw(), S).unwrap();
    assert_eq!(flags(&p, a) & header::AWAKE, 0, "the parent of an awake object stays asleep");
    assert_eq!(out, Exit { eax: b.raw(), ecx: synth::DATA, edx: 15, flags: Flags::AwakeTested });
}

#[test]
fn root_follows_parents_and_keeps_edx_for_none() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Vehicle, header::ACTIVE);
    let b = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE);
    let c = p.add(3, 0x8003, ObjectType::Weapon, header::ACTIVE);
    p.attach(c, b);
    p.attach(b, a);
    let out = root(&p.ram, c.raw(), 7).unwrap();
    assert_eq!(out, Exit { eax: a.raw(), ecx: NONE, edx: synth::DATA, flags: Flags::NoneCompared });
    assert_eq!(root(&p.ram, a.raw(), 7).unwrap().eax, a.raw());
    assert_eq!(root(&p.ram, NONE, 7).unwrap(), Exit { eax: NONE, ecx: NONE, edx: 7, flags: Flags::NoneCompared });
}

#[test]
fn wake_children_wakes_bipeds_vehicles_and_creatures_depth_first() {
    let mut p = Pool::new();
    let root_ = p.add(1, 0x8001, ObjectType::Scenery, header::ACTIVE);
    let biped = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE);
    let weapon = p.add(3, 0x8003, ObjectType::Weapon, header::ACTIVE);
    let creature = p.add(4, 0x8004, ObjectType::Creature, header::ACTIVE);
    let vehicle = p.add(5, 0x8005, ObjectType::Vehicle, header::ACTIVE);
    let scenery = p.add(6, 0x8006, ObjectType::Scenery, header::ACTIVE);
    p.attach(biped, root_);
    p.attach(weapon, root_);
    p.attach(scenery, root_);
    p.attach(creature, biped);
    p.attach(vehicle, weapon);
    let out = wake_children(&mut p.ram, root_.raw(), S).unwrap();
    assert_eq!(flags(&p, scenery) & header::AWAKE, 0, "scenery with nothing under it stays asleep");
    // The woken types, and (through `wake`'s parent walk) their ancestors.
    for d in [biped, creature, vehicle, weapon, root_] {
        assert_ne!(flags(&p, d) & header::AWAKE, 0, "{d:?}");
    }
    // The root frame returns with its last child's body and the edx of
    // that child's frame: the scenery's type mask.
    assert_eq!(out, Exit { eax: body(scenery), ecx: NONE, edx: 1 << 6, flags: Flags::NoneCompared });
}

#[test]
fn a_leaf_without_children_returns_its_type_mask_or_the_wake() {
    let mut p = Pool::new();
    let s = p.add(6, 0x8006, ObjectType::Scenery, header::ACTIVE);
    let out = wake_children(&mut p.ram, s.raw(), S).unwrap();
    assert_eq!(out, Exit { eax: s.raw(), ecx: NONE, edx: 1 << 6, flags: Flags::NoneCompared });
    let b = p.add(7, 0x8007, ObjectType::Biped, header::ACTIVE);
    let out = wake_children(&mut p.ram, b.raw(), S).unwrap();
    assert_eq!(out, Exit { eax: NONE, ecx: NONE, edx: 7 * 3, flags: Flags::NoneCompared });
}

#[test]
fn wake_for_post_goes_to_the_root_and_sets_post_when_active() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Vehicle, header::ACTIVE);
    let b = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE);
    p.attach(b, a);
    let out = wake_for_post(&mut p.ram, b.raw(), S).unwrap();
    assert_eq!(flags(&p, a) & (header::AWAKE | header::POST), header::AWAKE | header::POST);
    assert_eq!(flags(&p, b) & header::POST, 0, "POST goes on the root only");
    assert_ne!(flags(&p, b) & header::AWAKE, 0, "the biped child is woken");
    assert_eq!(out.eax, body(b));
    // Not ACTIVE: no POST, but the children pass still runs (and wakes
    // the parent through the biped's parent walk).
    let c = p.add(3, 0x8003, ObjectType::Scenery, 0);
    let d = p.add(4, 0x8004, ObjectType::Biped, header::ACTIVE);
    p.attach(d, c);
    wake_for_post(&mut p.ram, c.raw(), S).unwrap();
    assert_eq!(flags(&p, c) & header::POST, 0);
    assert_ne!(flags(&p, d) & header::AWAKE, 0);
}

#[test]
fn a_child_cycle_is_a_fault_not_a_hang() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Scenery, header::ACTIVE);
    let b = p.add(2, 0x8002, ObjectType::Scenery, header::ACTIVE);
    p.attach(b, a);
    p.body(b.index()).set(&mut p.ram, Object::FIRST_CHILD, a).unwrap();
    // Low in the pool's memory, so the recursion runs off its bottom
    // before it reaches the pool, as retail runs out of stack.
    let s = Stack { esp: synth::BASE + 0x100, ..S };
    assert!(wake_children(&mut p.ram, a.raw(), s).is_err());
}

#[test]
fn wake_children_leaves_the_stack_retail_leaves() {
    let mut p = Pool::new();
    let root_ = p.add(1, 0x8001, ObjectType::Scenery, header::ACTIVE);
    let biped = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE);
    p.attach(biped, root_);
    wake_children(&mut p.ram, root_.raw(), S).unwrap();
    let e = S.esp;
    let expect = [
        // The root frame: `push esi`, then `push ecx; call bc7b0`.
        (e - 4, S.esi),
        (e - 8, biped.raw()),
        (e - 12, 0x000b_c80c),
        // The biped's frame: `push esi` (its header), `call b7360`.
        (e - 16, synth::DATA + 2 * 12),
        (e - 20, 0x000b_c7e6),
        // `b7360`: `push ebx; push esi` (the biped's body).
        (e - 24, S.ebx),
        (e - 28, body(biped)),
    ];
    for (addr, v) in expect {
        assert_eq!(stack(&p, addr), v, "{addr:#x}");
    }
}

#[test]
fn wake_for_post_leaves_the_stack_retail_leaves() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Scenery, header::ACTIVE);
    let b = p.add(2, 0x8002, ObjectType::Scenery, header::ACTIVE);
    p.attach(b, a);
    wake_for_post(&mut p.ram, b.raw(), S).unwrap();
    let e = S.esp;
    let expect = [
        (e - 4, S.ebx),
        (e - 8, S.esi),
        (e - 12, S.edi),
        // `push esi` (the root) over the return address of `call b7360`,
        // whose pushes are below it.
        (e - 16, a.raw()),
        (e - 20, 0x000b_ba6d),
        // `bc7b0` on the root: `push esi; push ecx; call bc7b0` over
        // `b7360`'s `push ebx` (the data) and `push esi` (the root).
        (e - 24, a.raw()),
        (e - 28, b.raw()),
        (e - 32, 0x000b_c80c),
        // The child's frame: `push esi`, its header.
        (e - 36, synth::DATA + 2 * 12),
    ];
    for (addr, v) in expect {
        assert_eq!(stack(&p, addr), v, "{addr:#x}");
    }
}
