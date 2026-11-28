use super::*;
use guest_layout::{testing::FakeGuest, view::fake};
use guest_layout::tags::{GroupTag, entry_layout, index_layout};

const REGISTRY: u32 = 0x1000;
const COUNTS: u32 = 0x2000;
const DEFINITION: u32 = 0x3000;
const SIM: u32 = 0x8000;
const WORLD: u32 = 0x9000;
const OBJECTS: u32 = 0x20000;
const BODY: u32 = 0x28000;
const TAGS: u32 = 0x29000;
const TAG_ENTRIES: u32 = 0x2a000;

fn fake_registry(m: &mut FakeGuest) {
    m.map(REGISTRY, 0x108)
        .map(COUNTS, 8)
        .map(DEFINITION, STORAGE_LEN)
        .map(points::SIM_OBJECT_ENTITY_VTABLE.base, VTABLE_LEN)
        .put_u32(points::SIM_REPLICATION_REGISTRY.base, REGISTRY)
        .put_u32(REGISTRY, SLOT)
        .put_u32(REGISTRY + 0x44, 0x0815)
        .put_u32(REGISTRY + 0x84, 26)
        .put_u32(REGISTRY + 0x88, 0x4711)
        .put_u32(COUNTS, SLOT)
        .put_u32(COUNTS + 4, 26);
}

fn enable_probe(m: &mut FakeGuest) {
    install_definition(m, REGISTRY, COUNTS, DEFINITION, &[(0, 0x7123_0010)]).unwrap();
    let count = m.u32(COUNTS).unwrap();
    m.put_u32(REGISTRY, count);
}

fn fake_world(m: &mut FakeGuest, world_mode: u32) {
    m.map(SIM, 0x40)
        .map(WORLD, DATABASE + RECORDS + RECORD_STRIDE * INSTANCE_COUNT)
        .put_u32(points::SIM_GLOBALS.base, SIM)
        .put_u32(SIM + 4, WORLD)
        .put_u32(SIM + 8, world_mode);
}

fn fake_object(m: &mut FakeGuest, entity: u32) -> u32 {
    let pool = fake::datums::<objects::Header>(m, OBJECTS, 4);
    let (handle, header) = fake::push(m, &pool);
    m.map(BODY, 0x12c)
        .put_u32(points::OBJECTS.base, pool.array.addr)
        .put_u8(header.addr + 3, CREATURE)
        .put_u32(header.addr + 8, BODY)
        .put_u32(BODY + BODY_ENTITY, entity);
    handle.raw()
}

fn fake_tag(m: &mut FakeGuest, index: u16, group: GroupTag) -> u32 {
    let local = 0x9000_0000 | u32::from(index);
    let entry = TAG_ENTRIES + u32::from(index) * entry_layout::STRIDE;
    m.map(TAGS, 0x20)
        .map(TAG_ENTRIES, 0x1000)
        .put_u32(points::TAG_INDEX.base, TAGS)
        .put_u32(points::TAG_ENTRIES.base, TAG_ENTRIES)
        .put_u32(TAGS + index_layout::TAG_COUNT, u32::from(index) + 1)
        .put_u32(entry + entry_layout::DATUM, local)
        .put_u32(entry + entry_layout::GROUP, group.0)
        .put_u32(entry + entry_layout::ADDRESS, 0xcad0);
    local
}

#[test]
fn probe_definition_uses_the_selected_slot() {
    let mut m = FakeGuest::new();
    fake_registry(&mut m);
    assert!(!registered(&m));
    m.put_u32(points::SIM_OBJECT_ENTITY_VTABLE.base + 0x60, 0x1234);
    enable_probe(&mut m);
    assert!(registered(&m));
    assert_eq!(m.u32(REGISTRY).unwrap(), 18);
    assert_eq!(m.u32(REGISTRY + 0x44).unwrap(), 0x0815);
    assert_eq!(m.u32(REGISTRY + 0x48).unwrap(), DEFINITION);
    assert_eq!(m.u32(REGISTRY + 0x4c).unwrap(), 0);
    assert_eq!(m.u32(REGISTRY + 0x84).unwrap(), 26);
    assert_eq!(m.u32(REGISTRY + 0x88).unwrap(), 0x4711);
    assert_eq!(m.u32(COUNTS + 4).unwrap(), 26);
    assert_eq!(m.u32(DEFINITION + VTABLE).unwrap(), 0x7123_0010);
    assert_eq!(m.u32(DEFINITION + VTABLE + 0x60).unwrap(), 0x1234);
}

#[test]
fn the_probe_keeps_an_already_occupied_slot() {
    let mut m = FakeGuest::new();
    fake_registry(&mut m);
    m.put_u32(REGISTRY + ENTITY_POINTER, 0x1234);
    enable_probe(&mut m);
    assert_eq!(m.u32(COUNTS).unwrap(), SLOT);
    assert_eq!(m.u32(REGISTRY + ENTITY_POINTER).unwrap(), 0x1234);
    assert!(!registered(&m));
}

#[test]
fn creature_scheduling_preserves_stock_tuning() {
    let mut m = FakeGuest::new();
    const RECORD: u32 = 0x1800;
    const CONTEXT: u32 = 0x2000;
    const TIMEOUT: u32 = 0x004c_f474;
    m.map(RECORD, RECORD_STRIDE)
        .map(CONTEXT, 12)
        .put_u32(TIMEOUT, 0x1234_5678)
        .put_u32(TIMEOUT - 4, 0x8765_4321)
        .put_u16(RECORD + RECORD_TYPE, 9);
    assert_eq!(
        creature_update_priority(&m, RECORD, 0xdead_0000).unwrap(),
        None
    );
    assert_eq!(creature_record(&m, RECORD - RECORDS, 0), None);

    m.put_u16(RECORD + RECORD_TYPE, SLOT as u16)
        .put_u32(CONTEXT, POSITION)
        .put_u32(CONTEXT + 4, 1000)
        .put_u8(points::SIM_NETWORK_TIME_OVERRIDE.base, 1)
        .put_u32(points::SIM_NETWORK_TIME_OVERRIDE.base + 4, 1199);
    assert_eq!(
        creature_update_priority(&m, RECORD, CONTEXT).unwrap(),
        Some(0.0)
    );
    m.put_u32(points::SIM_NETWORK_TIME_OVERRIDE.base + 4, 1200);
    assert_eq!(
        creature_update_priority(&m, RECORD, CONTEXT).unwrap(),
        Some(MOVEMENT_PRIORITY)
    );
    assert_eq!(movement_relevance(0, 0, 1200), 0.0);
    assert_eq!(
        movement_relevance(POSITION, u32::MAX - 100, 100),
        MOVEMENT_PRIORITY
    );
    assert_eq!(creation_relevance(0, false), (CREATION_PRIORITY, 310));
    assert_eq!(
        creation_relevance(POSITION, false),
        (CREATION_PRIORITY, 406)
    );
    assert_eq!(creation_relevance(0, true), (0.0, 0));
    assert_ne!(CREATION_PRIORITY, MOVEMENT_PRIORITY);
    assert_eq!(m.u32(TIMEOUT).unwrap(), 0x1234_5678);
    assert_eq!(m.u32(TIMEOUT - 4).unwrap(), 0x8765_4321);
}

#[test]
fn a_spawn_can_enroll_once_on_the_distributed_host() {
    let mut m = FakeGuest::new();
    fake_registry(&mut m);
    fake_world(&mut m, HOST);
    let object = fake_object(&mut m, NO_ENTITY);
    let tag = fake_tag(&mut m, 3, GroupTag::from_chars(*b"crea"));
    m.put_u32(BODY, tag)
        .put_u32(BODY + 0x70, 1.0f32.to_bits())
        .put_u32(BODY + 0x84, 1.0f32.to_bits());
    assert!(spawn_enrollment(&m, object).is_none());
    enable_probe(&mut m);
    assert_eq!(
        spawn_enrollment(&m, object),
        Some(GuestCall::new(points::SIM_ENROLL_OBJECT.base).args([object]))
    );
    m.put_u32(SIM + 8, CLIENT);
    assert!(spawn_enrollment(&m, object).is_none());
    m.put_u32(SIM + 8, HOST).put_u32(BODY + BODY_ENTITY, 3);
    assert!(spawn_enrollment(&m, object).is_none());
    m.put_u32(BODY + BODY_ENTITY, NO_ENTITY);
    fake_tag(&mut m, 3, GroupTag::from_chars(*b"bipd"));
    assert!(spawn_enrollment(&m, object).is_none());
}

#[test]
fn snapshot_uses_the_current_identity_object_and_lifetime() {
    let mut m = FakeGuest::new();
    fake_registry(&mut m);
    enable_probe(&mut m);
    fake_world(&mut m, HOST);
    let entity = 0x4000_0003;
    let object = fake_object(&mut m, entity);
    let record = record_at(WORLD + DATABASE, instance_slot(entity));
    m.put_u32(record, entity)
        .put_u16(record + RECORD_TYPE, SLOT as u16)
        .put_u8(record + RECORD_LIVE, 1)
        .put_u32(record + RECORD_OBJECT, object);
    init_counter(&mut m, DEFINITION, entity, object).unwrap();
    count_update(&mut m, DEFINITION, entity, object).unwrap();
    count_update(&mut m, DEFINITION, entity, object).unwrap();
    assert_eq!(
        snapshot(&m),
        [CreatureRow {
            entity,
            object,
            updates: 2
        }]
    );

    m.put_u32(SIM + 8, CLIENT);
    assert_eq!(is_replica(&m, object), Some(()));
    assert_eq!(snapshot(&m)[0].updates, 2);
    m.put_u8(record + RECORD_LIVE, 0);
    assert!(snapshot(&m).is_empty());
    m.put_u8(record + RECORD_LIVE, 1);
    let reused = 0x5000_0003;
    m.put_u32(record, reused);
    assert_eq!(is_replica(&m, object), None);
    assert!(snapshot(&m).is_empty());
    m.put_u32(BODY + BODY_ENTITY, reused);
    assert_eq!(
        snapshot(&m),
        [CreatureRow {
            entity: reused,
            object,
            updates: 0
        }]
    );
    m.put_u32(SIM + 8, 1);
    assert!(snapshot(&m).is_empty());
}
