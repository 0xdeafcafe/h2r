use super::*;
use guest_layout::object::header;
use guest_layout::{FlatRam, Object, ObjectHeader};
use sim_objects::synth::{self, Pool};

#[test]
fn no_map_no_objects() {
    let ram = FlatRam::new(synth::BASE, 0x10_0000);
    assert_eq!(objects(&ram).unwrap().count(), 0);
    assert!(object(&ram, ObjectId(0x8000_0001)).unwrap().is_none());
}

#[test]
fn objects_list_in_slot_order_with_kind_and_flags() {
    let mut p = Pool::new();
    let a = p.add(5, 0x8005, ObjectKind::Weapon, header::ACTIVE | header::DELETE);
    let b = p.add(1, 0x8001, ObjectKind::Biped, header::ACTIVE | header::AWAKE | header::POST);
    let c = p.add(40, 0x8028, ObjectKind::Crate, header::ACTIVE | header::NEW);
    let listed: Vec<(ObjectId, Option<ObjectKind>, ObjectFlags)> = objects(&p.ram)
        .unwrap()
        .map(|o| {
            let o = o.unwrap();
            (o.id(), o.kind().unwrap(), o.flags().unwrap())
        })
        .collect();
    assert_eq!(listed.iter().map(|l| l.0).collect::<Vec<_>>(), [b, a, c].map(ObjectId::from));
    assert_eq!(listed[0].1, Some(ObjectKind::Biped));
    assert_eq!(
        listed[0].2,
        ObjectFlags { active: true, awake: true, post: true, ..Default::default() }
    );
    assert!(listed[1].2.deleting && !listed[1].2.awake);
    assert!(listed[2].2.new);
    assert_eq!(listed[2].1, Some(ObjectKind::Crate));
}

#[test]
fn a_type_outside_the_table_has_no_kind() {
    let mut p = Pool::new();
    let a = p.add(3, 0x8003, ObjectKind::Scenery, header::ACTIVE);
    p.header(3).set(&mut p.ram, ObjectHeader::KIND, 13).unwrap();
    let o = object(&p.ram, a.into()).unwrap().unwrap();
    assert_eq!(o.kind().unwrap(), None);
    assert!(o.as_unit().unwrap().is_none());
}

#[test]
fn transform_sphere_vitality_and_definition_read_the_body() {
    let mut p = Pool::new();
    let a = p.add(2, 0x8002, ObjectKind::Vehicle, header::ACTIVE);
    let b = p.body(2);
    let r = &mut p.ram;
    b.set(r, Object::POSITION, [1.0, 2.0, 3.0]).unwrap();
    b.set(r, Object::FORWARD, [0.0, 1.0, 0.0]).unwrap();
    b.set(r, Object::UP, [0.0, 0.0, 1.0]).unwrap();
    b.set(r, Object::SCALE, 1.5).unwrap();
    b.set(r, Object::VELOCITY, [4.0, 0.0, -1.0]).unwrap();
    b.set(r, Object::BOUND_CENTER, [1.0, 2.0, 3.5]).unwrap();
    b.set(r, Object::BOUND_RADIUS, 2.25).unwrap();
    b.set(r, Object::HEALTH, 0.0).unwrap();
    b.set(r, Object::SHIELD, 0.5).unwrap();
    b.set(r, Object::DAMAGE_FLAGS, guest_layout::object::DEPLETED).unwrap();
    b.set(r, Object::DEFINITION, Datum(0xe123_0007)).unwrap();
    let o = object(&p.ram, a.into()).unwrap().unwrap();
    assert_eq!(
        o.transform().unwrap(),
        Transform { position: [1.0, 2.0, 3.0], forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], scale: 1.5 }
    );
    assert_eq!(o.velocity().unwrap(), [4.0, 0.0, -1.0]);
    assert_eq!(o.bounding_sphere().unwrap(), Sphere { center: [1.0, 2.0, 3.5], radius: 2.25 });
    let v = o.vitality().unwrap();
    assert_eq!((v.body, v.shield), (0.0, 0.5));
    assert!(v.depleted());
    assert_eq!(o.definition().unwrap(), Datum(0xe123_0007));
}

#[test]
fn parents_and_children() {
    let mut p = Pool::new();
    let parent = p.add(1, 0x8001, ObjectKind::Vehicle, header::ACTIVE);
    let gunner = p.add(2, 0x8002, ObjectKind::Biped, header::ACTIVE);
    let gun = p.add(3, 0x8003, ObjectKind::Weapon, header::ACTIVE);
    p.attach(gunner, parent);
    p.attach(gun, parent);
    let o = object(&p.ram, parent.into()).unwrap().unwrap();
    assert_eq!(o.parent().unwrap(), None);
    let kids: Vec<ObjectId> = o.children().map(|c| c.unwrap().id()).collect();
    assert_eq!(kids, [gunner, gun].map(ObjectId::from));
    let child = object(&p.ram, gun.into()).unwrap().unwrap();
    assert_eq!(child.parent().unwrap(), Some(parent.into()));
    assert!(child.flags().unwrap().child);
}

#[test]
fn nodes_are_copied_from_the_body() {
    let mut p = Pool::new();
    let a = p.add(4, 0x8004, ObjectKind::Biped, header::ACTIVE);
    let o = object(&p.ram, a.into()).unwrap().unwrap();
    assert_eq!(o.node_count().unwrap(), 0);
    assert_eq!(o.nodes(&mut [NodeMatrix::default(); 2]).unwrap(), 0);

    let b = p.body(4);
    let offset = 0x200i16;
    b.set(&mut p.ram, Object::NODES_OFFSET, offset).unwrap();
    b.set(&mut p.ram, Object::NODES_SIZE, 3 * NODE_MATRIX_SIZE as u16).unwrap();
    let matrix = Off::<[f32; 13]>::new(0);
    for i in 0..3u32 {
        let at = b.addr + offset as u32 + i * NODE_MATRIX_SIZE;
        matrix.write(&mut p.ram, at, std::array::from_fn(|k| (10 * i + k as u32) as f32)).unwrap();
    }
    let o = object(&p.ram, a.into()).unwrap().unwrap();
    assert_eq!(o.node_count().unwrap(), 3);
    // Fewer slots than nodes: as many as fit, and the full count.
    let mut out = [NodeMatrix::default(); 2];
    assert_eq!(o.nodes(&mut out).unwrap(), 3);
    assert_eq!(out[1].0[0], 10.0);
    assert_eq!(out[1].0[12], 22.0);
    let mut out = [NodeMatrix::default(); 4];
    assert_eq!(o.nodes(&mut out).unwrap(), 3);
    assert_eq!(out[2].0[5], 25.0);
    assert_eq!(out[3], NodeMatrix::default());
}

#[test]
fn units_read_player_actor_team_seat_and_weapons() {
    let mut p = Pool::new();
    let a = p.add(6, 0x8006, ObjectKind::Biped, header::ACTIVE);
    let w = p.add(7, 0x8007, ObjectKind::Weapon, header::ACTIVE);
    let u = p.body(6).as_unit();
    let r = &mut p.ram;
    u.set(r, Unit::PLAYER, Datum(0xec70_0000)).unwrap();
    u.set(r, Unit::ACTOR, Datum::NONE).unwrap();
    u.set(r, Unit::TEAM, 3).unwrap();
    u.set(r, Unit::SEAT, -1).unwrap();
    u.set(r, Unit::WEAPONS, [Datum::NONE, w, Datum::NONE, Datum::NONE]).unwrap();
    u.set(r, Unit::WEAPON_INDEX, 1).unwrap();
    u.set(r, Unit::AIM, [0.0, 0.0, -1.0]).unwrap();
    let o = object(&p.ram, a.into()).unwrap().unwrap();
    let unit = o.as_unit().unwrap().unwrap();
    assert_eq!(unit.player().unwrap(), Some(Datum(0xec70_0000)));
    assert_eq!(unit.actor().unwrap(), None);
    assert_eq!(unit.team().unwrap(), 3);
    assert_eq!(unit.seat().unwrap(), None);
    assert_eq!(unit.weapons().unwrap(), [None, Some(w.into()), None, None]);
    assert_eq!(unit.held_weapon().unwrap(), Some(w.into()));
    assert_eq!(unit.aim().unwrap(), [0.0, 0.0, -1.0]);
    // A weapon is not a unit.
    assert!(object(&p.ram, w.into()).unwrap().unwrap().as_unit().unwrap().is_none());
}

#[test]
fn a_stale_id_does_not_resolve() {
    let mut p = Pool::new();
    let a = p.add(9, 0x8009, ObjectKind::Scenery, header::ACTIVE);
    p.free(9);
    assert!(object(&p.ram, a.into()).unwrap().is_none());
    p.add(9, 0x800a, ObjectKind::Scenery, header::ACTIVE);
    assert!(object(&p.ram, a.into()).unwrap().is_none());
}

/// The pool answers the readers as the arena does: the same ids, kinds,
/// flags, transforms and child links.
#[test]
fn the_object_table_reads_as_the_arena_does() {
    let mut p = Pool::new();
    let a = p.add(0, 0xef62, ObjectKind::Biped, header::ACTIVE | header::AWAKE);
    let b = p.add(2, 0xef64, ObjectKind::Weapon, header::ACTIVE | header::POST);
    for slot in [0, 2] {
        p.header(slot).set(&mut p.ram, ObjectHeader::BODY_SIZE, 0x130).unwrap();
        p.body(slot).set(&mut p.ram, Object::POSITION, [slot as f32, 1.0, 2.0]).unwrap();
    }
    p.attach(b, a);
    let set = |ram: &mut FlatRam, at: u32, v: &[u8]| guest_layout::MemMut::write_bytes(ram, synth::POOL + at, v).unwrap();
    set(&mut p.ram, 0x34, &1i32.to_le_bytes());
    set(&mut p.ram, 0x40, &0xef65u16.to_le_bytes());
    let table = ObjectTable::import(&p.ram).unwrap().unwrap();
    let mem = PoolMem::new(&table);
    let summary = |m: &dyn Fn() -> Vec<(ObjectId, Option<ObjectKind>, u8, [f32; 3], Vec<ObjectId>)>| m();
    let from_arena = summary(&|| {
        objects(&p.ram)
            .unwrap()
            .map(|o| {
                let o = o.unwrap();
                let kids = o.children().map(|c| c.unwrap().id()).collect();
                (o.id(), o.kind().unwrap(), o.flag_bits().unwrap(), o.transform().unwrap().position, kids)
            })
            .collect()
    });
    let from_pool = summary(&|| {
        objects(&mem)
            .unwrap()
            .map(|o| {
                let o = o.unwrap();
                let kids = o.children().map(|c| c.unwrap().id()).collect();
                (o.id(), o.kind().unwrap(), o.flag_bits().unwrap(), o.transform().unwrap().position, kids)
            })
            .collect()
    });
    assert_eq!(from_arena.len(), 2);
    assert_eq!(from_pool, from_arena);
    assert_eq!(from_pool[0].4, [ObjectId::from(b)]);
    assert!(object(&mem, ObjectId::from(a)).unwrap().is_some());
    assert!(object(&mem, ObjectId(0xef62_0009)).unwrap().is_none());
}
