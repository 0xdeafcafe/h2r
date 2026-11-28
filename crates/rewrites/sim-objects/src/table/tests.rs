use super::*;
use crate::synth::{self, Pool as Synth};
use crate::view::{ObjRef, Objects};
use guest_layout::object::header;
use guest_layout::{Datum, FlatRam, Object, ObjectType};

fn rec(flags: u8, body_addr: u32) -> ObjectRecord {
    ObjectRecord {
        flags,
        kind: ObjectType::Scenery as u8,
        cluster: 7,
        body_size: 4,
        body_addr,
        body: vec![1, 2, 3, 4],
    }
}

fn small() -> ObjectTable {
    ObjectTable::new(8, OBJECT_NAME)
}

/// `datum_new`'s order: the first free slot from the scan start, the salt
/// from `next_salt` (`"ob" | 0x8000`), and a delete lowers the scan start
/// and walks the high water down past free slots.
#[test]
fn slots_and_salts_follow_datum_new() {
    let mut t = small();
    let salt = OBJECT_NAME | 0x8000;
    let a = t.alloc(rec(0, 0x100)).unwrap();
    let b = t.alloc(rec(0, 0x200)).unwrap();
    let c = t.alloc(rec(0, 0x300)).unwrap();
    assert_eq!([a.raw(), b.raw(), c.raw()], [
        u32::from(salt) << 16,
        u32::from(salt + 1) << 16 | 1,
        u32::from(salt + 2) << 16 | 2,
    ]);
    assert_eq!((t.high_water(), t.len(), t.free_scan_start()), (3, 3, 3));
    t.delete(a).unwrap();
    assert_eq!(t.free_scan_start(), 0);
    let d = t.alloc(rec(0, 0x400)).unwrap();
    assert_eq!((d.index(), d.salt()), (0, salt + 3));
    // The top slots go: the high water follows the highest live one.
    t.delete(c).unwrap();
    t.delete(b).unwrap();
    assert_eq!(t.high_water(), 1);
    assert_eq!(t.delete(c), Err(StaleHandle(c)));
}

#[test]
fn a_full_table_refuses_and_a_stale_handle_reads_nothing() {
    let mut t = ObjectTable::new(2, OBJECT_NAME);
    let a = t.alloc(rec(0, 0x100)).unwrap();
    t.alloc(rec(0, 0x200)).unwrap();
    assert!(t.alloc(rec(0, 0x300)).is_none());
    t.delete(a).unwrap();
    assert!(t.get(a).is_none());
    assert!(!t.set_flags(a, header::ACTIVE));
}

#[test]
fn the_section_round_trips_with_its_free_slots_in_place() {
    let mut t = small();
    let hs: Vec<_> = (0..4).map(|i| t.alloc(rec(header::ACTIVE, 0x100 * (i + 1))).unwrap()).collect();
    t.delete(hs[1]).unwrap();
    assert!(t.set_flags(hs[2], header::ACTIVE | header::AWAKE));
    let bytes = t.encode();
    let back = small().decode(ObjectTable::VERSION, &bytes).unwrap();
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.section_hash(), t.section_hash());
    // The next handle is the one the original would hand out.
    let (mut a, mut b) = (t, back);
    assert_eq!(a.alloc(rec(0, 0x900)), b.alloc(rec(0, 0x900)));
    assert_eq!(small().decode(9, &bytes).err(), Some(RestoreError::SectionVersion { kind: KIND, version: 9 }));
    assert!(ObjectTable::new(16, OBJECT_NAME).decode(ObjectTable::VERSION, &bytes).is_err());
}

#[test]
fn a_change_changes_the_hash() {
    let mut t = small();
    let h = t.alloc(rec(0, 0x100)).unwrap();
    let before = t.section_hash();
    t.get_mut(h).unwrap().body[0] = 9;
    assert_ne!(t.section_hash(), before);
}

/// A synthetic guest pool with allocation state the importer needs.
fn guest() -> Synth {
    let mut p = Synth::new();
    p.add(0, 0xef62, ObjectType::Biped, header::ACTIVE | header::AWAKE);
    p.add(1, 0xef63, ObjectType::Scenery, header::ACTIVE);
    p.add(3, 0xef65, ObjectType::Weapon, header::ACTIVE | header::POST);
    p.add(4, 0xef66, ObjectType::Scenery, header::ACTIVE);
    p.attach(Datum(0xef66_0004), Datum(0xef62_0000));
    for slot in [0, 1, 3, 4] {
        let h = p.header(slot);
        h.set(&mut p.ram, ObjectHeader::BODY_SIZE, 0x130).unwrap();
        let b = p.body(slot);
        b.set(&mut p.ram, Object::POSITION, [slot as f32, 2.0, 3.0]).unwrap();
        b.set(&mut p.ram, Object::HEALTH, 0.5).unwrap();
    }
    p.free(2);
    let w = |ram: &mut FlatRam, at: u32, v: &[u8]| ram.write_bytes(synth::POOL + at, v).unwrap();
    w(&mut p.ram, 0x34, &2i32.to_le_bytes());
    w(&mut p.ram, 0x40, &0xef67u16.to_le_bytes());
    p
}

#[test]
fn import_reads_the_arena_and_scatter_writes_it_back_bit_for_bit() {
    let p = guest();
    let t = ObjectTable::import(&p.ram).unwrap().unwrap();
    assert_eq!((t.len(), t.high_water(), t.free_scan_start(), t.next_salt()), (4, 5, 2, 0xef67));
    assert_eq!(t.capacity(), u32::from(synth::CAPACITY));
    // The hole at slot 2 is the next allocation, as `datum_new` would.
    let mut u = ObjectTable::import(&p.ram).unwrap().unwrap();
    assert_eq!(u.alloc(rec(0, 0x100)).unwrap().raw(), 0xef67_0002);

    // Scatter over a zeroed copy of the same arena reproduces every byte
    // the table owns: headers, bitmap, allocation state and bodies.
    let mut q = Synth::new();
    t.scatter(&mut q.ram).unwrap();
    for slot in [0u16, 1, 3, 4] {
        let (mut a, mut b) = ([0u8; 0x130], [0u8; 0x130]);
        Mem::read_bytes(&p.ram, synth::BODIES + u32::from(slot) * synth::BODY_SIZE, &mut a).unwrap();
        Mem::read_bytes(&q.ram, synth::BODIES + u32::from(slot) * synth::BODY_SIZE, &mut b).unwrap();
        assert_eq!(a, b, "body {slot}");
    }
    let (mut a, mut b) = (vec![0u8; 5 * 12], vec![0u8; 5 * 12]);
    Mem::read_bytes(&p.ram, synth::DATA, &mut a).unwrap();
    Mem::read_bytes(&q.ram, synth::DATA, &mut b).unwrap();
    assert_eq!(a, b);
    let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
    Mem::read_bytes(&p.ram, synth::BITMAP, &mut a).unwrap();
    Mem::read_bytes(&q.ram, synth::BITMAP, &mut b).unwrap();
    assert_eq!(a, b);
    let re = ObjectTable::import(&q.ram).unwrap().unwrap();
    assert_eq!(re.encode(), t.encode());
}

#[test]
fn scatter_clears_what_a_delete_freed() {
    let p = guest();
    let mut t = ObjectTable::import(&p.ram).unwrap().unwrap();
    let mut ram = p.ram.clone();
    let h = t.handle_at(4).unwrap();
    t.delete(h).unwrap();
    t.scatter(&mut ram).unwrap();
    let o = Objects::read(&ram).unwrap().unwrap();
    assert_eq!(o.high_water(&ram).unwrap(), 4);
    assert!(!o.is_live(&ram, 4).unwrap());
    assert_eq!(o.header(4).salt(&ram).unwrap(), 0);
}

#[test]
fn the_views_read_the_pool_as_they_read_the_arena() {
    let mut p = guest();
    p.attach(Datum(0xef63_0001), Datum(0xef62_0000));
    let t = ObjectTable::import(&p.ram).unwrap().unwrap();
    let mem = PoolMem::new(&t);
    let (g, r) = (Objects::read(&p.ram).unwrap().unwrap(), Objects::read(&mem).unwrap().unwrap());
    let key = |o: &ObjRef, m: &dyn Fn(&ObjRef) -> (u8, f32, [f32; 3])| (o.handle, m(o));
    let guest_side: Vec<_> = g
        .iter(&p.ram)
        .map(|o| {
            let o = o.unwrap();
            key(&o, &|o| (o.flags(&p.ram).unwrap(), o.body.health(&p.ram).unwrap(), o.body.position(&p.ram).unwrap()))
        })
        .collect();
    let pool_side: Vec<_> = r
        .iter(&mem)
        .map(|o| {
            let o = o.unwrap();
            key(&o, &|o| (o.flags(&mem).unwrap(), o.body.health(&mem).unwrap(), o.body.position(&mem).unwrap()))
        })
        .collect();
    assert_eq!(guest_side.len(), 4);
    assert_eq!(guest_side, pool_side);
    // Handles resolve, stale ones do not, and the child links walk.
    let parent = r.resolve(&mem, Datum(0xef62_0000)).unwrap().unwrap();
    let kids: Vec<_> = r.children(&mem, parent).map(|c| c.unwrap().handle.raw()).collect();
    let want: Vec<_> = g
        .children(&p.ram, g.resolve(&p.ram, Datum(0xef62_0000)).unwrap().unwrap())
        .map(|c| c.unwrap().handle.raw())
        .collect();
    assert_eq!(kids, want);
    assert!(r.resolve(&mem, Datum(0xdead_0000)).unwrap().is_none());
    // An address the pool does not hold is a fault, not zeros.
    assert!(mem.read_bytes(0x1234, &mut [0u8; 4]).is_err());
}

#[test]
fn no_pool_while_the_global_is_null() {
    let ram = FlatRam::new(synth::BASE, 0x10_0000);
    assert!(ObjectTable::import(&ram).unwrap().is_none());
}

#[test]
fn an_array_whose_allocation_state_disagrees_is_refused_by_name() {
    let mut p = guest();
    p.ram.write_bytes(synth::POOL + 0x40, &0x1234u16.to_le_bytes()).unwrap();
    assert_eq!(ObjectTable::import(&p.ram).err(), Some(ImportError::Corrupt("next salt out of range")));
}
