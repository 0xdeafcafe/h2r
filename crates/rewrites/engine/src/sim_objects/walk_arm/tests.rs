//! The walk arm on synthetic state: its table, the turns, the prediction
//! inside a trial and its comparison, and the walk's visit count.

use super::*;
use runtime::guest::Permissions;

/// The arm's state is process-wide: one test at a time touches it.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn the_table_holds_the_walks_bd090_bipeds_and_weapons_and_nothing_else() {
    assert_eq!(TABLE.bodies().len(), POINTS.len());
    for point in POINTS {
        assert!(TABLE.owns(point.base), "{}", point.name);
    }
    for other in [
        &points::UNIT_UPDATE,
        &points::VEHICLE_UPDATE,
        &points::OBJECT_DAMAGE_UPDATE,
        &points::OBJECT_FUNCTION_VALUE,
        &points::OBJECT_CHANGE_COLORS,
        &points::OBJECT_BOUNDS,
        &points::LIGHTS_UPDATE,
    ] {
        assert!(!TABLE.owns(other.base), "{} is the guest's", other.name);
    }
    // A site maps to its body's entry.
    let pre = TABLE.at(points::OBJECTS_PRE_PHYSICS.base).unwrap().0;
    let site = pre.sites[0];
    assert_eq!(entry_of(site), Some(points::OBJECTS_PRE_PHYSICS.base));
}

fn prove(entry: u32) {
    state().entries.entry(entry).or_default().matched = PROVEN_AFTER;
}

#[test]
fn a_proven_entry_answers_except_each_audit_slot_and_a_difference_ends_every_take_over() {
    let _l = lock();
    reset();
    let entry = points::BIPED_UPDATE.base;
    let other = points::WEAPON_UPDATE.base;
    state().entries.entry(entry).or_default().matched = PROVEN_AFTER - 1;
    assert_eq!(turn(entry), Some(Turn::Compare));
    assert_eq!(turn(0x1234), None);
    prove(entry);
    prove(other);
    // Slot 0 went above; slots 64 and 128 are audits.
    for slot in 1..=2 * AUDIT_EVERY {
        let want = if slot % AUDIT_EVERY == 0 { Turn::Compare } else { Turn::Answer };
        assert_eq!(turn(entry), Some(want), "slot {slot}");
    }
    // A site takes its entry's slots.
    let site = TABLE.at(entry).unwrap().0.sites[0];
    assert_eq!(turn(site), Some(Turn::Answer));
    new_run();
    assert_eq!(turn(entry), Some(Turn::Compare), "slot 0 audits again");
    assert_eq!(turn(entry), Some(Turn::Answer));
    assert_eq!(turn(other), Some(Turn::Compare));
    assert_eq!(turn(other), Some(Turn::Answer));
    // One difference at another entry: the guest answers everywhere.
    {
        let mut s = state();
        s.entries.entry(points::OBJECT_PRE_UPDATE.base).or_default().mismatched = 1;
        s.failed = true;
    }
    assert!(failed());
    assert!((0..AUDIT_EVERY).all(|_| turn(entry) == Some(Turn::Compare)));
    assert!(!admit(other));
    // Until the next map.
    reset();
    assert!(!failed());
    assert!(stats().is_empty());
    assert_eq!(turn(entry), Some(Turn::Compare), "the proof is the map's");
}

#[test]
fn admit_takes_the_slot_only_when_it_admits() {
    let _l = lock();
    reset();
    let entry = points::OBJECT_PRE_UPDATE.base;
    assert!(!admit(entry), "unproven");
    assert_eq!(state().slots.get(&entry), None);
    prove(entry);
    // Slot 0 is an audit: refused, so the runtime's entry takes it.
    assert!(!admit(entry));
    assert_eq!(turn(entry), Some(Turn::Compare));
    assert!(admit(entry));
    assert_eq!(state().slots[&entry], 2);
    assert!(!admit(0x1234));
    reset();
}

const POOL: u32 = 0x0050_0000;
const HEADERS: u32 = 0x0050_1000;
const BITMAP: u32 = 0x0050_0100;
const BODY: u32 = 0x0051_0000;
const STACK: u32 = 0x0057_0800;
const CALLER: u32 = 0x0001_2345;

fn guest() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x004e_0000, 0x0009_0000, Permissions::RW).unwrap();
    g.set_u32(points::OBJECTS.base, POOL).unwrap();
    g.set_u32(POOL + 0x20, 64).unwrap();
    g.set_u32(POOL + 0x24, 12).unwrap();
    g.set_u32(POOL + 0x44, HEADERS).unwrap();
    g.set_u32(POOL + 0x48, BITMAP).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g
}

/// A live object in `slot`, with no links.
fn add(g: &mut Guest, slot: u32, kind: ObjectType, flags: u8) {
    let header = HEADERS + 12 * slot;
    let body = BODY + 0x400 * slot;
    g.set_u16(header, slot as u16 + 1).unwrap();
    g.set_u8(header + 2, flags).unwrap();
    g.set_u8(header + 3, kind as u8).unwrap();
    g.set_u32(header + 8, body).unwrap();
    for link in [0x0c, 0x10, 0x14] {
        g.set_u32(body + link, u32::MAX).unwrap();
    }
    let word = BITMAP + 4 * (slot / 32);
    let bits = g.u32(word).unwrap();
    g.set_u32(word, bits | (1 << (slot % 32))).unwrap();
    let high = g.u32(POOL + 0x38).unwrap();
    g.set_u32(POOL + 0x38, high.max(slot + 1)).unwrap();
}

#[test]
fn a_prediction_puts_every_byte_back_and_is_compared_with_the_guests_outcome() {
    let _l = lock();
    reset();
    let mut g = guest();
    add(&mut g, 0, ObjectType::Biped, 3);
    // The carrier `baf40` of slot 0: it pushes and pops `esi` and returns
    // the object, whose body `+4` bit 26 is clear.
    let carrier = points::OBJECT_CARRIER.base;
    let cpu = CpuAbi { esp: STACK, eax: 1 << 16, esi: 0x5151_5151, ..Default::default() };
    let p = predict(&cpu, &mut g, carrier).unwrap();
    assert_eq!(g.u32(STACK - 4).unwrap(), 0, "the push went back");
    assert_eq!(p.transfer, TransferAbi::ret(CALLER));
    assert_eq!((p.cpu.eax, p.cpu.esp, p.cpu.esi, p.cpu.eip), (1 << 16, STACK + 4, 0x5151_5151, CALLER));
    assert_eq!(p.stores, (0..4).map(|i| (STACK - 4 + i, 0x51)).collect::<Vec<_>>());

    // The guest's outcome as retail leaves it matches.
    let after = p.cpu;
    g.set_u32(STACK - 4, 0x5151_5151).unwrap();
    assert_eq!(p.difference(&after, &g, TransferAbi::ret(CALLER)), None);
    // A register, a store and the transfer each differ.
    let edx = CpuAbi { edx: after.edx ^ 1, ..after };
    assert_eq!(p.difference(&edx, &g, p.transfer).unwrap().what, "edx");
    assert_eq!(p.difference(&after, &g, TransferAbi::ret(CALLER + 1)).unwrap().what, "transfer");
    g.set_u8(STACK - 2, 0).unwrap();
    let m = p.difference(&after, &g, p.transfer).unwrap();
    assert_eq!((m.pc, m.what.as_str(), m.rust.as_str(), m.guest.as_str()), (carrier, "memory 0x005707fe", "0x51", "0x00"));
    // An undefined flag on either side is not compared.
    let flags = CpuAbi { eflags: after.eflags ^ 1, eflags_defined: after.eflags_defined & !1, ..after };
    g.set_u32(STACK - 4, 0x5151_5151).unwrap();
    assert_eq!(p.difference(&flags, &g, p.transfer), None);

    // Counted: a match, then a mismatch that fails the arm.
    p.compare(&after, &g, TransferAbi::ret(CALLER));
    let p = predict(&cpu, &mut g, carrier).unwrap();
    p.compare(&edx, &g, TransferAbi::ret(CALLER));
    let s = &stats()[0].1;
    assert_eq!((s.matched, s.mismatched), (1, 1));
    assert_eq!(s.first.as_ref().unwrap().what, "edx");
    assert!(failed());
    // A guest that yields is not parity.
    reset();
    predict(&cpu, &mut g, carrier).unwrap().compare(&after, &g, TransferAbi::resume(carrier + 3));
    assert_eq!((stats()[0].1.interrupted, failed()), (1, false));
    reset();
}

#[test]
fn a_faulting_segment_is_declined_and_leaves_nothing() {
    let _l = lock();
    reset();
    let mut g = guest();
    // Slot 0's header points at an unmapped body.
    add(&mut g, 0, ObjectType::Biped, 3);
    g.set_u32(HEADERS + 8, 0x0100_0000).unwrap();
    let cpu = CpuAbi { esp: STACK, eax: 1 << 16, esi: 7, ..Default::default() };
    assert!(predict(&cpu, &mut g, points::OBJECT_CARRIER.base).is_none());
    assert_eq!(g.u32(STACK - 4).unwrap(), 0);
    assert_eq!(stats()[0].1.declined, 1);
    reset();
}

#[test]
fn a_walk_entry_counts_its_slot_pass_by_ported_type() {
    let _l = lock();
    reset();
    let mut g = guest();
    let awake = 3;
    add(&mut g, 0, ObjectType::Biped, awake);
    add(&mut g, 1, ObjectType::Weapon, awake | 4);
    add(&mut g, 2, ObjectType::Vehicle, awake);
    add(&mut g, 5, ObjectType::Scenery, 1 | 4);
    add(&mut g, 6, ObjectType::Projectile, awake | 8);
    assert_eq!(walk_at(points::OBJECTS_PRE_PHYSICS.base), Some(Walk::Pre));
    assert_eq!(walk_at(points::OBJECTS_POST_PHYSICS.base), Some(Walk::Post));
    assert_eq!(walk_at(points::OBJECTS_FINALIZE.base), None);
    walk_entered(&g, Walk::Pre);
    assert_eq!(visits(), Visits { ported: 2, guest: 1 });
    walk_entered(&g, Walk::Post);
    assert_eq!(visits(), Visits { ported: 3, guest: 2 });
    // No pool: nothing.
    g.set_u32(points::OBJECTS.base, 0).unwrap();
    walk_entered(&g, Walk::Pre);
    assert_eq!(visits(), Visits { ported: 3, guest: 2 });
    reset();
}
