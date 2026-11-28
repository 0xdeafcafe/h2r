use super::*;
use crate::table::{ObjectRecord, ObjectTable};
use guest_layout::FlatRam;

const BASE: u32 = 0x1000_0000;
const ARRAY: u32 = BASE + 0x1000;
const DATA: u32 = BASE + 0x2000;
const BITMAP: u32 = BASE + 0x1800;

fn light(flags: u8, owner: u32, parent: u32, created: u32, list: u32) -> LightRecord {
    let mut r = LightRecord::default();
    r.set_flags(flags);
    r.set_u32(field::OWNER, owner);
    r.set_u32(field::PARENT, parent);
    r.set_u32(field::CREATED, created);
    r.set_u32(field::LIST, list);
    r.set_u32(field::DEFINITION, 0xd0d0);
    r
}

fn object(flags: u8, body: Vec<u8>) -> ObjectRecord {
    ObjectRecord { flags, body_size: body.len() as u16, body_addr: 0x100, body, ..ObjectRecord::default() }
}

#[derive(Default)]
struct Host {
    calls: Vec<String>,
    lifetime: f32,
}

impl LightHost for Host {
    fn record_update(&mut self, _: &mut LightTable, l: Handle) {
        self.calls.push(format!("update {}", l.index()));
    }
    fn link(&mut self, lights: &mut LightTable, l: Handle) {
        self.calls.push(format!("link {}", l.index()));
        let r = lights.get_mut(l).unwrap();
        r.set_flags(r.flags() | flag::LINKED);
    }
    fn list_remove(&mut self, l: Handle, record: u32) {
        self.calls.push(format!("remove {} {record:#x}", l.index()));
    }
    fn lifetime(&self, definition: u32) -> u32 {
        assert_eq!(definition, 0xd0d0);
        self.lifetime.to_bits()
    }
}

fn clock(tick: i32) -> Clock {
    Clock { tick, ticks_per_second: 30 }
}

#[test]
fn walk_updates_every_light_in_slot_order() {
    let mut t = LightTable::new(8, 0x696c);
    for _ in 0..3 {
        t.alloc(light(flag::ACTIVE, NONE, NONE, 0, 0)).unwrap();
    }
    let mut host = Host::default();
    walk(&mut t, &mut ObjectTable::default(), &mut host, clock(1));
    assert_eq!(host.calls, ["update 0", "update 1", "update 2"]);
}

#[test]
fn a_linked_light_on_a_post_owner_is_relinked() {
    let mut objects = ObjectTable::new(4, 0x626f);
    let post = objects.alloc(object(4, vec![0; 8])).unwrap();
    let plain = objects.alloc(object(0, vec![0; 8])).unwrap();
    let mut t = LightTable::new(8, 0x696c);
    // Active and linked, owner POST: remove, clear, link.
    t.alloc(light(flag::ACTIVE | flag::LINKED, post.raw(), NONE, 0, 0x77)).unwrap();
    // Linked but inactive, marker parent POST: link only.
    t.alloc(light(flag::LINKED, NONE, post.raw(), 0, 0)).unwrap();
    // Owner not POST: untouched.
    t.alloc(light(flag::ACTIVE | flag::LINKED, plain.raw(), NONE, 0, 0)).unwrap();
    // Not linked: untouched.
    t.alloc(light(flag::ACTIVE, post.raw(), NONE, 0, 0)).unwrap();
    let mut host = Host::default();
    walk(&mut t, &mut objects, &mut host, clock(1));
    assert_eq!(
        host.calls,
        ["update 0", "remove 0 0x77", "link 0", "update 1", "link 1", "update 2", "update 3"]
    );
    let first = t.get(t.handle_at(0).unwrap()).unwrap();
    assert_eq!(first.flags(), flag::ACTIVE | flag::LINKED);
}

#[test]
fn a_timed_light_expires_only_past_its_lifetime() {
    let mut objects = ObjectTable::default();
    let mut t = LightTable::new(8, 0x696c);
    let h = t.alloc(light(flag::TIMED, NONE, NONE, 10, 0)).unwrap();
    let mut host = Host { lifetime: 2.0, ..Host::default() };
    // 30 ticks per second times 2.0 is 60: age 60 is not past it.
    walk(&mut t, &mut objects, &mut host, clock(70));
    assert!(t.get(h).is_some());
    walk(&mut t, &mut objects, &mut host, clock(71));
    assert!(t.get(h).is_none());
    assert!(t.is_empty());
}

#[test]
fn a_nan_lifetime_never_expires() {
    let mut t = LightTable::new(8, 0x696c);
    let h = t.alloc(light(flag::TIMED, NONE, NONE, 0, 0)).unwrap();
    let mut host = Host { lifetime: f32::NAN, ..Host::default() };
    walk(&mut t, &mut ObjectTable::default(), &mut host, clock(1_000_000));
    assert!(t.get(h).is_some());
}

#[test]
fn expire_sets_the_parent_bit_while_another_light_shares_it() {
    let mut objects = ObjectTable::new(4, 0x626f);
    let parent = objects.alloc(object(0, vec![0; 8])).unwrap();
    let mut t = LightTable::new(8, 0x696c);
    let a = t.alloc(light(flag::ACTIVE | flag::LINKED, NONE, parent.raw(), 0, 0x5)).unwrap();
    let b = t.alloc(light(0, NONE, parent.raw(), 0, 0)).unwrap();
    let mut host = Host::default();
    expire(&mut t, &mut objects, &mut host, a, true);
    assert_eq!(host.calls, ["remove 0 0x5"]);
    assert!(t.get(a).is_none());
    let flags = |o: &ObjectTable| u32::from_le_bytes(o.get(parent).unwrap().body[4..8].try_into().unwrap());
    assert_eq!(flags(&objects), 0x40);
    expire(&mut t, &mut objects, &mut host, b, true);
    assert_eq!(flags(&objects), 0);
}

#[test]
fn expire_without_notify_leaves_the_parent_alone() {
    let mut objects = ObjectTable::new(4, 0x626f);
    let parent = objects.alloc(object(0, vec![0, 0, 0, 0, 0x40, 0, 0, 0])).unwrap();
    let mut t = LightTable::new(8, 0x696c);
    let a = t.alloc(light(0, NONE, parent.raw(), 0, 0)).unwrap();
    expire(&mut t, &mut objects, &mut Host::default(), a, false);
    assert_eq!(objects.get(parent).unwrap().body[4], 0x40);
}

#[test]
fn unlink_removes_an_active_light_only() {
    let mut t = LightTable::new(8, 0x696c);
    let a = t.alloc(light(flag::ACTIVE | flag::LINKED, NONE, NONE, 0, 9)).unwrap();
    let b = t.alloc(light(flag::LINKED, NONE, NONE, 0, 9)).unwrap();
    let mut host = Host::default();
    unlink(&mut t, &mut host, a);
    unlink(&mut t, &mut host, b);
    assert_eq!(host.calls, ["remove 0 0x9"]);
    assert_eq!(t.get(a).unwrap().flags(), flag::ACTIVE);
    assert_eq!(t.get(b).unwrap().flags(), flag::LINKED);
}

/// A guest array of `high` slots: slot 0 live, slot 1 free, slot 2 live.
fn guest() -> FlatRam {
    let mut ram = FlatRam::new(BASE, 0x10_0000);
    let w = |ram: &mut FlatRam, at: u32, v: &[u8]| ram.write_bytes(at, v).unwrap();
    w(&mut ram, guest_layout::global::LIGHTS.base, &ARRAY.to_le_bytes());
    w(&mut ram, ARRAY, &0x696cu16.to_le_bytes());
    w(&mut ram, ARRAY + 0x20, &0x80i32.to_le_bytes());
    w(&mut ram, ARRAY + 0x24, &(STRIDE as i32).to_le_bytes());
    w(&mut ram, ARRAY + 0x34, &1i32.to_le_bytes());
    w(&mut ram, ARRAY + 0x38, &3i32.to_le_bytes());
    w(&mut ram, ARRAY + 0x3c, &2i32.to_le_bytes());
    w(&mut ram, ARRAY + 0x40, &0xe972u16.to_le_bytes());
    w(&mut ram, ARRAY + 0x44, &DATA.to_le_bytes());
    w(&mut ram, ARRAY + 0x48, &BITMAP.to_le_bytes());
    w(&mut ram, BITMAP, &0b101u32.to_le_bytes());
    for (slot, salt) in [(0u32, 0xe96cu16), (2, 0xe96e)] {
        let at = DATA + slot * STRIDE;
        w(&mut ram, at, &salt.to_le_bytes());
        w(&mut ram, at + 2, &[flag::ACTIVE]);
        w(&mut ram, at + 0x58, &NONE.to_le_bytes());
    }
    ram
}

#[test]
fn import_reads_the_array_and_scatter_writes_it_back_bit_for_bit() {
    let mut ram = guest();
    let t = LightTable::import(&ram).unwrap().unwrap();
    assert_eq!((t.len(), t.high_water(), t.free_scan_start(), t.next_salt()), (2, 3, 1, 0xe972));
    assert_eq!(t.handle_at(2).unwrap().raw(), 0xe96e_0002);
    let mut before = vec![0u8; 3 * STRIDE as usize];
    ram.read_bytes(DATA, &mut before).unwrap();
    t.scatter(&mut ram).unwrap();
    let mut after = vec![0u8; 3 * STRIDE as usize];
    ram.read_bytes(DATA, &mut after).unwrap();
    assert_eq!(before, after);
    let again = LightTable::import(&ram).unwrap().unwrap();
    assert_eq!(again.section_hash(), t.section_hash());
}

#[test]
fn scatter_clears_what_a_delete_freed() {
    let mut ram = guest();
    let mut t = LightTable::import(&ram).unwrap().unwrap();
    let h = t.handle_at(2).unwrap();
    t.delete(h).unwrap();
    t.scatter(&mut ram).unwrap();
    let re = LightTable::import(&ram).unwrap().unwrap();
    assert_eq!((re.len(), re.high_water()), (1, 1));
}

#[test]
fn the_section_round_trips_and_a_rewritten_light_changes_the_hash() {
    let ram = guest();
    let mut t = LightTable::import(&ram).unwrap().unwrap();
    let bytes = t.encode();
    let back = t.decode(LightTable::VERSION, &bytes).unwrap();
    assert_eq!(back.section_hash(), t.section_hash());
    let before = t.section_hash();
    let h = t.handle_at(0).unwrap();
    t.get_mut(h).unwrap().set_u32(field::CREATED, 5);
    assert_ne!(t.section_hash(), before);
}

#[test]
fn no_map_is_no_table() {
    let ram = FlatRam::new(BASE, 0x1000);
    assert!(LightTable::import(&ram).unwrap().is_none());
}
