use super::*;
use crate::synth::{self, Pool};
use guest_layout::{Datum, ObjectType};

fn flags(p: &Pool, d: Datum) -> u8 {
    p.header(d.index()).flags(&p.ram).unwrap()
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
    let out = wake(&mut p.ram, c.raw()).unwrap();
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
    let out = wake(&mut p.ram, b.raw()).unwrap();
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
    let out = wake_children(&mut p.ram, root_.raw()).unwrap();
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
    let out = wake_children(&mut p.ram, s.raw()).unwrap();
    assert_eq!(out, Exit { eax: s.raw(), ecx: NONE, edx: 1 << 6, flags: Flags::NoneCompared });
    let b = p.add(7, 0x8007, ObjectType::Biped, header::ACTIVE);
    let out = wake_children(&mut p.ram, b.raw()).unwrap();
    assert_eq!(out, Exit { eax: NONE, ecx: NONE, edx: 7 * 3, flags: Flags::NoneCompared });
}

#[test]
fn wake_for_post_goes_to_the_root_and_sets_post_when_active() {
    let mut p = Pool::new();
    let a = p.add(1, 0x8001, ObjectType::Vehicle, header::ACTIVE);
    let b = p.add(2, 0x8002, ObjectType::Biped, header::ACTIVE);
    p.attach(b, a);
    let out = wake_for_post(&mut p.ram, b.raw()).unwrap();
    assert_eq!(flags(&p, a) & (header::AWAKE | header::POST), header::AWAKE | header::POST);
    assert_eq!(flags(&p, b) & header::POST, 0, "POST goes on the root only");
    assert_ne!(flags(&p, b) & header::AWAKE, 0, "the biped child is woken");
    assert_eq!(out.eax, body(b));
    // Not ACTIVE: no POST, but the children pass still runs (and wakes
    // the parent through the biped's parent walk).
    let c = p.add(3, 0x8003, ObjectType::Scenery, 0);
    let d = p.add(4, 0x8004, ObjectType::Biped, header::ACTIVE);
    p.attach(d, c);
    wake_for_post(&mut p.ram, c.raw()).unwrap();
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
    assert!(wake_children(&mut p.ram, a.raw()).is_err());
}
