use super::*;
use crate::synth::{self, Pool};
use guest_layout::{FlatRam, MemMut};

fn pool_view(p: &Pool) -> Objects {
    Objects::read(&p.ram).unwrap().unwrap()
}

#[test]
fn no_pool_while_the_global_is_null() {
    let ram = FlatRam::new(synth::BASE, 0x10_0000);
    assert_eq!(Objects::read(&ram).unwrap(), None);
}

#[test]
fn the_live_scan_crosses_word_boundaries_and_stops_at_the_high_water_mark() {
    let mut p = Pool::new();
    for slot in [0, 31, 32, 33, 63, 64, 95] {
        p.add(slot, 0x8000 + slot, ObjectType::Scenery, header::ACTIVE);
    }
    let o = pool_view(&p);
    let slots = |p: &Pool| -> Vec<u16> {
        o.iter(&p.ram).map(|r| r.unwrap().handle.index()).collect()
    };
    assert_eq!(slots(&p), [0, 31, 32, 33, 63, 64, 95]);
    assert_eq!(o.next_live(&p.ram, 1).unwrap(), Some(31));
    assert_eq!(o.next_live(&p.ram, 34).unwrap(), Some(63));
    assert_eq!(o.next_live(&p.ram, 96).unwrap(), None);
    // A lower high-water mark hides the slots at and past it, even in a
    // word that has them.
    p.set_high_water(64);
    assert_eq!(slots(&p), [0, 31, 32, 33, 63]);
    p.set_high_water(33);
    assert_eq!(o.next_live(&p.ram, 33).unwrap(), None);
    p.set_high_water(0);
    assert_eq!(slots(&p), Vec::<u16>::new());
    p.set_high_water(-1);
    assert_eq!(slots(&p), Vec::<u16>::new());
}

#[test]
fn the_scan_never_reads_a_word_past_the_one_holding_the_next_slot() {
    let mut p = Pool::new();
    p.add(3, 0x8003, ObjectType::Scenery, header::ACTIVE);
    p.add(40, 0x8028, ObjectType::Scenery, header::ACTIVE);
    // Memory that ends after the first bitmap word: finding slot 3 must
    // not touch the second.
    let mut short = FlatRam::new(synth::BASE, (synth::BITMAP + 4 - synth::BASE) as usize);
    for a in (synth::BASE..synth::BITMAP + 4).step_by(4) {
        let mut b = [0; 4];
        guest_layout::Mem::read_bytes(&p.ram, a, &mut b).unwrap();
        short.write_bytes(a, &b).unwrap();
    }
    let o = pool_view(&p);
    assert_eq!(o.next_live(&short, 0).unwrap(), Some(3));
    assert!(o.next_live(&short, 4).is_err());
}

#[test]
fn handles_resolve_only_when_live_in_range_and_salted() {
    let mut p = Pool::new();
    let a = p.add(7, 0x9007, ObjectType::Biped, header::ACTIVE | header::AWAKE);
    let o = pool_view(&p);
    let r = o.resolve(&p.ram, a).unwrap().unwrap();
    assert_eq!(r.handle, a);
    assert_eq!(r.body, p.body(7));
    assert_eq!(r.kind(&p.ram).unwrap(), Some(ObjectType::Biped));
    assert!(r.is_awake(&p.ram).unwrap());
    assert_eq!(o.resolve(&p.ram, Datum::NONE).unwrap(), None);
    assert_eq!(o.resolve(&p.ram, Datum(a.raw() ^ 0x0001_0000)).unwrap(), None, "stale salt");
    assert_eq!(o.resolve(&p.ram, Datum(0x9007_0050)).unwrap(), None, "past the high-water mark");
    p.free(7);
    assert_eq!(o.resolve(&p.ram, a).unwrap(), None, "freed");
}

#[test]
fn a_moved_body_is_seen_at_its_new_address() {
    let mut p = Pool::new();
    let a = p.add(2, 0x9002, ObjectType::Weapon, header::ACTIVE);
    let o = pool_view(&p);
    let before = o.resolve(&p.ram, a).unwrap().unwrap();
    // Pool compaction rewrites the header's body pointer.
    let moved = synth::BODIES + 100 * synth::BODY_SIZE;
    p.header(2).set(&mut p.ram, ObjectHeader::BODY, moved).unwrap();
    let after = o.resolve(&p.ram, a).unwrap().unwrap();
    assert_ne!(before.body, after.body);
    assert_eq!(after.body, Object::at(moved));
}

#[test]
fn children_follow_first_child_then_siblings() {
    let mut p = Pool::new();
    let parent = p.add(1, 0x9001, ObjectType::Vehicle, header::ACTIVE);
    let kids: Vec<Datum> = [4, 2, 9]
        .into_iter()
        .map(|s| p.add(s, 0x9000 + s, ObjectType::Weapon, header::ACTIVE))
        .collect();
    for &k in &kids {
        p.attach(k, parent);
    }
    let o = pool_view(&p);
    let r = o.resolve(&p.ram, parent).unwrap().unwrap();
    let seen: Vec<Datum> = o.children(&p.ram, r).map(|c| c.unwrap().handle).collect();
    assert_eq!(seen, kids);
    for &k in &kids {
        let f = o.resolve(&p.ram, k).unwrap().unwrap().flags(&p.ram).unwrap();
        assert_ne!(f & header::CHILD, 0);
    }
    // A child deleted mid-list ends the walk there, as a stale link does.
    p.free(2);
    let seen: Vec<Datum> = o.children(&p.ram, r).map(|c| c.unwrap().handle).collect();
    assert_eq!(seen, kids[..1]);
}

#[test]
fn a_sibling_cycle_ends() {
    let mut p = Pool::new();
    let parent = p.add(1, 0x9001, ObjectType::Vehicle, header::ACTIVE);
    let a = p.add(2, 0x9002, ObjectType::Weapon, header::ACTIVE);
    p.attach(a, parent);
    // a's next sibling is a.
    p.body(2).set(&mut p.ram, Object::NEXT_SIBLING, a).unwrap();
    let o = pool_view(&p);
    let r = o.resolve(&p.ram, parent).unwrap().unwrap();
    assert_eq!(o.children(&p.ram, r).count(), 1 << 16);
}
