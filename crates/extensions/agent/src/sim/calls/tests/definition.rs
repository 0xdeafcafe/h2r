//! `world.object.respawn {definition}` over the fake guest: the placements
//! naming a definition are remade linked, and what the engine path cannot
//! remake is listed and left alone.
use super::*;

const SECOND_BODY: u32 = 0x8030_0000;
const THIRD_BODY: u32 = 0x8031_0000;
const ENTRIES: u32 = 0x8105_0000;
const VEHICLES: u32 = 0x70;

/// `placed_vehicle`'s placement 0 and a second, both of palette entry 1, with
/// a live object on each, which it returns.
fn two_vehicles() -> (World, ObjectId, ObjectId) {
    let (mut w, first) = placed_vehicle();
    let root = 0x8100_0000;
    let stride = u32::from(tags::object::OBJECT_TYPES[guest_layout::ObjectType::Vehicle as usize]
        .placement
        .unwrap()
        .2);
    let entry = w.g.bytes(ENTRY, 0x54).unwrap();
    w.g.map(ENTRIES, stride * 2 + 0x10);
    w.g.write(ENTRIES, &entry).unwrap();
    w.g.write(ENTRIES + stride, &entry).unwrap();
    put_vec(&mut w.g, ENTRIES + stride + 8, [9.0, 1.0, 2.0]).unwrap();
    w.g.put_u32(root + VEHICLES, 2).put_u32(root + VEHICLES + 4, ENTRIES);
    let second = live_vehicle(&mut w, SECOND_BODY, 1);
    (w, first, second)
}

/// A vehicle in the object pool that carries placement `index`.
fn live_vehicle(w: &mut World, body: u32, index: u16) -> ObjectId {
    let pool = objects::array(&w.g).unwrap().unwrap();
    let (h, header) = fake::push(&mut w.g, &pool);
    w.g.map(body, 0x400);
    header.set_body(&mut w.g, body).unwrap();
    header.set_kind(&mut w.g, objects::VEHICLE).unwrap();
    Body::at(body).set_parent(&mut w.g, None).unwrap();
    Body::at(body).set_definition(&mut w.g, Some(DatumHandle(TAG))).unwrap();
    w.g.put_u16(body + guest_layout::Object::PLACEMENT_INDEX.offset, index);
    ObjectId { kind: agent_proto::Kind::Vehicle, key: h.raw() }
}

/// The calls the game plays: the placement data, `object_new` making an
/// object that carries the index written into the placement data, and the
/// deletion.
fn game(c: &GuestCall, m: &mut dyn GuestMemory, bodies: &mut Vec<u32>) -> Regs {
    match c.target {
        addr::PLACEMENT_NEW => {}
        addr::OBJECT_NEW => {
            let index = m.u16(SCRATCH + addr::PLACEMENT_INDEX).unwrap();
            let pool = objects::array(&*m).unwrap().unwrap();
            let h = pool.array.new_datum(m).unwrap().unwrap();
            let header = pool.get(m, h).unwrap().unwrap();
            let body = bodies.remove(0);
            header.set_kind(m, objects::VEHICLE).unwrap();
            header.set_body(m, body).unwrap();
            Body::at(body).set_parent(m, None).unwrap();
            Body::at(body).set_definition(m, Some(DatumHandle(TAG))).unwrap();
            m.set_u16(body + guest_layout::Object::PLACEMENT_INDEX.offset, index).unwrap();
            return al(h.raw());
        }
        addr::OBJECT_DELETE => {
            let pool = objects::array(&*m).unwrap().unwrap();
            pool.array.delete(m, DatumHandle(c.args[0])).unwrap();
        }
        _ => panic!("unexpected guest call"),
    }
    al(0)
}

#[test]
fn two_placements_of_a_definition_are_both_remade_linked() {
    let (mut w, first, second) = two_vehicles();
    w.g.map(0x8032_0000, 0x400);
    w.g.map(0x8033_0000, 0x400);
    let mut bodies = vec![0x8032_0000, 0x8033_0000];
    let (out, calls, _) = one(w, Call::ObjectRespawnDefinition { definition: TAG }, |c, m| {
        game(c, m, &mut bodies)
    });
    let out = out.unwrap();
    assert_eq!(
        calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [
            addr::PLACEMENT_NEW, addr::OBJECT_NEW, addr::OBJECT_DELETE,
            addr::PLACEMENT_NEW, addr::OBJECT_NEW, addr::OBJECT_DELETE,
        ],
    );
    let done = out["respawned"].as_array().unwrap();
    assert_eq!(done.len(), 2);
    for (row, (index, old)) in done.iter().zip([(0, first), (1, second)]) {
        assert_eq!(row["placement"], json!({"block": "vehicles", "index": index}));
        assert_eq!(row["old"], old.to_string());
        assert_eq!(row["linked"], true, "{row}");
        assert_ne!(row["object"], row["old"]);
    }
    assert_eq!(out["skipped"], json!([]));
    assert_eq!(out["engine_made"], json!([]));
}

#[test]
fn a_held_object_is_listed_and_an_engine_made_one_is_named_and_neither_is_touched() {
    let (mut w, first, second) = two_vehicles();
    // The second placement's object is held; a third vehicle of the
    // definition has no placement.
    Body::at(SECOND_BODY)
        .set_parent(&mut w.g, Some(DatumHandle(0x0017_0001)))
        .unwrap();
    let loose = live_vehicle(&mut w, THIRD_BODY, u16::MAX);
    w.g.map(0x8032_0000, 0x400);
    let mut bodies = vec![0x8032_0000];
    let (out, calls, _) = one(w, Call::ObjectRespawnDefinition { definition: TAG }, |c, m| {
        game(c, m, &mut bodies)
    });
    let out = out.unwrap();
    assert_eq!(calls.len(), 3, "only the first placement reached the game");
    assert_eq!(calls[2].args, [first.key]);
    assert_eq!(out["respawned"].as_array().unwrap().len(), 1);
    let skipped = out["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["placement"], json!({"block": "vehicles", "index": 1}));
    assert!(skipped[0]["why"].as_str().unwrap().contains("attached"));
    assert_eq!(out["engine_made"], json!([loose.to_string()]));
    assert_ne!(second.key, loose.key);
}

#[test]
fn a_placement_without_a_live_object_is_skipped_and_a_refused_record_is_named() {
    let (mut w, _, second) = two_vehicles();
    // Placement 1's object was destroyed; placement 0 is flagged.
    let pool = objects::array(&w.g).unwrap().unwrap();
    pool.array.delete(&mut w.g, DatumHandle(second.key)).unwrap();
    w.g.put_u16(ENTRIES + 4, 0x40);
    let (out, calls, _) = one(w, Call::ObjectRespawnDefinition { definition: TAG }, |_, _| {
        panic!("nothing to remake reached the game")
    });
    assert!(calls.is_empty());
    let out = out.unwrap();
    assert_eq!(out["respawned"], json!([]));
    let skipped = out["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert!(skipped[0]["why"].as_str().unwrap().contains("flags"));
    assert!(skipped[1]["why"].as_str().unwrap().contains("no live object"));
}

#[test]
fn a_definition_no_placement_names_remakes_nothing() {
    let (w, _, _) = two_vehicles();
    let (out, calls, _) = one(w, Call::ObjectRespawnDefinition { definition: TAG ^ 0x10 }, |_, _| {
        panic!("an unrelated definition reached the game")
    });
    assert!(calls.is_empty());
    let out = out.unwrap();
    assert_eq!(out["respawned"], json!([]));
    assert_eq!(out["skipped"], json!([]));
}
