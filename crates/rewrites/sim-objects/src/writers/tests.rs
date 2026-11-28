use super::*;
use crate::synth::{self, Pool as Synth};
use guest_layout::object::header;
use guest_layout::{Datum, FlatRam, ObjectHeader, ObjectType};

/// The heap header, and the pool blocks around the bodies.
const HEAP_AT: u32 = 0x004d_f000;
const BASE: u32 = 0x004d_0000;

fn word(ram: &FlatRam, at: u32) -> u32 {
    let mut b = [0u8; 4];
    Mem::read_bytes(ram, at, &mut b).unwrap();
    u32::from_le_bytes(b)
}

fn put(ram: &mut FlatRam, at: u32, v: u32) {
    ram.write_bytes(at, &v.to_le_bytes()).unwrap();
}

/// The synthetic arena inside a larger RAM that holds the heap global, with
/// bodies 0, 1 and 3 as one pool block list (head 0, 1, 3 tail): each
/// block's header sits `0x10` before its body.
fn guest() -> FlatRam {
    let mut p = Synth::new();
    p.add(0, 0xef62, ObjectType::Biped, header::ACTIVE | header::CHILD);
    p.add(1, 0xef63, ObjectType::Scenery, header::ACTIVE | header::CHILD);
    p.add(2, 0xef64, ObjectType::Scenery, header::ACTIVE);
    p.add(3, 0xef65, ObjectType::Weapon, header::ACTIVE);
    for slot in 0..4 {
        p.header(slot).set(&mut p.ram, ObjectHeader::BODY_SIZE, 0x130).unwrap();
    }
    // 0 is the parent of 1 and 3 (in that order); 2 stands alone.
    p.attach(Datum(0xef63_0001), Datum(0xef62_0000));
    p.attach(Datum(0xef65_0003), Datum(0xef62_0000));
    let mut bytes = vec![0u8; (synth::BASE - BASE) as usize];
    bytes.extend_from_slice(p.ram.bytes());
    let mut ram = FlatRam::from_bytes(BASE, bytes);
    put(&mut ram, HEAP, HEAP_AT);
    put(&mut ram, HEAP_AT + 0x30, 1000);
    put(&mut ram, HEAP_AT + 0x34, block(3));
    put(&mut ram, HEAP_AT + 0x38, block(0));
    for (slot, size) in [(0u32, 0x140u32), (1, 0x150), (2, 0x160), (3, 0x170)] {
        let b = block(slot);
        put(&mut ram, b, size);
        put(&mut ram, b + 8, if slot == 0 { 0 } else { block(slot - 1) });
        put(&mut ram, b + 0x0c, if slot == 3 { 0 } else { block(slot + 1) });
    }
    ram
}

fn block(slot: u32) -> u32 {
    synth::BODIES + slot * synth::BODY_SIZE - BLOCK_HEADER
}

fn handle(slot: u16, salt: u16) -> Handle {
    Handle::new(salt, slot)
}

fn table(ram: &FlatRam) -> ObjectTable {
    ObjectTable::import(ram).unwrap().unwrap()
}

#[test]
fn detach_cuts_the_first_child_out_of_its_parents_list() {
    let ram = guest();
    let mut t = table(&ram);
    detach_from_parent(&mut t, handle(1, 0xef63)).unwrap();
    let body = |slot| t.get(t.handle_at(slot).unwrap()).unwrap();
    // The parent's head is now 3; 1 has no sibling, no parent, no flag.
    assert_eq!(u32_at(&body(0).body, 0x10), 0xef65_0003);
    assert_eq!(u32_at(&body(1).body, 0x0c), NONE);
    assert_eq!(u32_at(&body(1).body, 0x14), NONE);
    assert_eq!(body(1).body[0x18], 0xff);
    assert_eq!(body(1).flags, header::ACTIVE);
    assert_eq!(body(3).flags, header::ACTIVE | header::CHILD);
}

#[test]
fn detach_cuts_a_later_child_and_closes_the_gap() {
    let ram = guest();
    let mut t = table(&ram);
    detach_from_parent(&mut t, handle(3, 0xef65)).unwrap();
    let body = |slot| t.get(t.handle_at(slot).unwrap()).unwrap();
    assert_eq!(u32_at(&body(0).body, 0x10), 0xef63_0001);
    assert_eq!(u32_at(&body(1).body, 0x0c), NONE);
    assert_eq!(u32_at(&body(3).body, 0x14), NONE);
}

#[test]
fn detach_through_the_arena_scatters() {
    let mut ram = guest();
    detach_in_guest(&mut ram, handle(1, 0xef63)).unwrap();
    let t = table(&ram);
    assert_eq!(u32_at(&t.get(t.handle_at(0).unwrap()).unwrap().body, 0x10), 0xef65_0003);
    assert_eq!(t.get(t.handle_at(1).unwrap()).unwrap().flags, header::ACTIVE);
}

#[test]
fn free_a_middle_block_relinks_its_neighbours_and_returns_its_size() {
    let mut ram = guest();
    free_in_guest(&mut ram, handle(1, 0xef63)).unwrap();
    assert_eq!(word(&ram, HEAP_AT + 0x30), 1000 + 0x150);
    assert_eq!(word(&ram, block(0) + 0x0c), block(2));
    assert_eq!(word(&ram, block(2) + 8), block(0));
    assert_eq!((word(&ram, HEAP_AT + 0x34), word(&ram, HEAP_AT + 0x38)), (block(3), block(0)));
    // The datum is gone: slot 1 free, three live.
    let t = table(&ram);
    assert_eq!(t.len(), 3);
    assert!(t.handle_at(1).is_none());
}

#[test]
fn free_the_head_and_the_tail_move_the_pool_ends() {
    let mut ram = guest();
    free_in_guest(&mut ram, handle(0, 0xef62)).unwrap();
    assert_eq!(word(&ram, HEAP_AT + 0x38), block(1));
    assert_eq!(word(&ram, block(1) + 8), 0);
    free_in_guest(&mut ram, handle(3, 0xef65)).unwrap();
    assert_eq!(word(&ram, HEAP_AT + 0x34), block(2));
    assert_eq!(word(&ram, block(2) + 0x0c), 0);
    assert_eq!(word(&ram, HEAP_AT + 0x30), 1000 + 0x140 + 0x170);
}

#[test]
fn free_a_bodiless_object_only_deletes_the_datum() {
    let mut ram = guest();
    let mut t = table(&ram);
    t.get_mut(handle(2, 0xef64)).unwrap().body_addr = 0;
    t.get_mut(handle(2, 0xef64)).unwrap().body.clear();
    t.scatter(&mut ram).unwrap();
    free_in_guest(&mut ram, handle(2, 0xef64)).unwrap();
    assert_eq!(word(&ram, HEAP_AT + 0x30), 1000);
    assert_eq!(table(&ram).len(), 3);
}

#[test]
fn an_unlive_slot_is_a_named_error() {
    let ram = guest();
    let mut t = table(&ram);
    let e = free_object(&mut ram.clone(), &mut t, handle(9, 0xef70)).unwrap_err();
    assert_eq!(e, WriterError::Stale(handle(9, 0xef70)));
}
