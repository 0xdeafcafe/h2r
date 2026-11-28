//! `world.placement.edit` over a fake guest: a scenario block of invented
//! records, a pool of invented objects, and the engine's callees played by
//! the tests as the real ones behave (`placement_data_new` leaves the index
//! slot alone, `object_new` copies the placement data's `+0x10` into the
//! object, `object_delete` flags the object).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_proto::sim::REFUSED;
use agent_proto::{Kind, ObjectId, RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::entry::testing::run as drive;
use guest_layout::entry::{Cx, EntryFn, EntryFuture, GuestCall, Regs, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::testing::FakeGuest;
use guest_layout::view::fake;
use guest_layout::view::objects::{self, Body, Header};
use serde_json::{Value, json};

use super::super::super::{Call, Outcome, SCRATCH_SIZE, Shared, addr, lock, run, shared};
use super::super::Entry;
use super::*;

const SCRATCH: u32 = 0x8040_0000;
const OBJECTS_HDR: u32 = 0x8010_0000;
const ROOT: u32 = 0x8100_0000;
const BLOCK: u32 = 0x8101_0000;
const PALETTE: u32 = 0x8102_0000;
const NAMES: u32 = 0x8103_0000;
const TAG_INDEX: u32 = 0x8104_0000;
const TAG_ENTRIES: u32 = 0x8105_0000;
const DEFINITION: u32 = 0x8106_0000;
const BODIES: u32 = 0x8200_0000;
const TAG: u32 = 0x4321_0001;
const SCENERY: u8 = guest_layout::ObjectType::Scenery as u8;
const NO_NAME: u16 = u16::MAX;

fn al(v: u32) -> Regs {
    Regs { eax: v, ..Regs::default() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn id(key: u32) -> String {
    ObjectId { kind: Kind::Scenery, key }.to_string()
}

fn plan(v: Value) -> Plan {
    Plan::new(serde_json::from_value(v).unwrap()).unwrap()
}

fn plan_err(v: Value) -> RpcError {
    Plan::new(serde_json::from_value(v).unwrap()).unwrap_err()
}

/// An unnamed, unflagged scenery record at `x`, as `scenario_edit` makes one.
fn record(x: f32) -> Vec<u8> {
    let stride = BlockSpec::named("scenery").unwrap().stride as usize;
    let mut b = vec![0; stride];
    b[2..4].copy_from_slice(&NO_NAME.to_le_bytes());
    b[8..12].copy_from_slice(&x.to_le_bytes());
    b
}

struct Scene {
    g: FakeGuest,
    stride: u32,
    offset: u32,
    bodies: BTreeMap<u32, u32>,
}

impl Scene {
    /// `block` holding `records` exactly (the map's loaded count), a palette
    /// of one scenery tag, two object names (scenery, vehicle) and an empty
    /// object pool.
    fn new(block: &str, records: &[Vec<u8>]) -> Scene {
        let spec = BlockSpec::named(block).unwrap();
        let stride = spec.stride;
        let mut g = FakeGuest::new();
        g.map(guest_layout::points::OBJECTS.base, 4).put_u32(guest_layout::points::OBJECTS.base, OBJECTS_HDR);
        fake::datums::<Header>(&mut g, OBJECTS_HDR, 64);
        g.map(SCRATCH, SCRATCH_SIZE);
        g.map(guest_layout::points::SCENARIO.base, 4).put_u32(guest_layout::points::SCENARIO.base, ROOT);
        g.map(ROOT, 0x800);
        g.map(BLOCK, (records.len() as u32 * stride).max(4));
        for (i, r) in records.iter().enumerate() {
            assert_eq!(r.len() as u32, stride);
            g.put(BLOCK + i as u32 * stride, r);
        }
        g.put_u32(ROOT + spec.offset, records.len() as u32).put_u32(ROOT + spec.offset + 4, BLOCK);

        // The scenery palette and its tag, as the respawn tests lay them out.
        let group = u32::from_be_bytes(*b"scen");
        g.put_u32(ROOT + 0x58, 1).put_u32(ROOT + 0x5c, PALETTE);
        g.map(PALETTE, 0x28).put_u32(PALETTE, group).put_u32(PALETTE + 4, TAG);
        for (global, ptr) in [
            (guest_layout::points::TAG_INDEX.base, TAG_INDEX),
            (guest_layout::points::TAG_ENTRIES.base, TAG_ENTRIES),
        ] {
            g.map(global, 4).put_u32(global, ptr);
        }
        g.map(TAG_INDEX, 0x20).put_u32(TAG_INDEX + guest_layout::tags::index_layout::TAG_COUNT, 2);
        g.map(TAG_ENTRIES, 0x20)
            .put_u32(TAG_ENTRIES + 0x10, group)
            .put_u32(TAG_ENTRIES + 0x14, TAG)
            .put_u32(TAG_ENTRIES + 0x18, DEFINITION);
        g.map(DEFINITION, 8).put_u16(DEFINITION, SCENERY.into());

        // Names 0 and 1: scenery 0 and vehicle 0.
        g.put_u32(ROOT + layout::OBJECT_NAMES, 2).put_u32(ROOT + layout::OBJECT_NAMES + 4, NAMES);
        g.map(NAMES, 2 * layout::OBJECT_NAME_STRIDE);
        g.put_u16(NAMES + layout::OBJECT_NAME_TYPE as u32, SCENERY.into());
        g.put_u16(NAMES + layout::OBJECT_NAME_PLACEMENT as u32, 0);
        let second = NAMES + layout::OBJECT_NAME_STRIDE;
        g.put_u16(second + layout::OBJECT_NAME_TYPE as u32, objects::VEHICLE.into());
        g.put_u16(second + layout::OBJECT_NAME_PLACEMENT as u32, 0);
        Scene { g, stride, offset: spec.offset, bodies: BTreeMap::new() }
    }

    /// A live scenery object with scenario placement `placement` (-1: none).
    fn object(&mut self, placement: i16) -> u32 {
        let pool = objects::array(&self.g).unwrap().unwrap();
        let (h, header) = fake::push(&mut self.g, &pool);
        header.set_kind(&mut self.g, SCENERY).unwrap();
        let body = BODIES + self.bodies.len() as u32 * 0x1000;
        self.g.map(body, 0x400);
        header.set_body(&mut self.g, body).unwrap();
        Body::at(body).set_parent(&mut self.g, None).unwrap();
        self.g.put_u16(body + guest_layout::Object::PLACEMENT_INDEX.offset, placement as u16);
        self.bodies.insert(h.raw(), body);
        h.raw()
    }

    fn placement_of(&self, object: u32) -> i16 {
        let body = self.bodies[&object];
        self.g.u16(body + guest_layout::Object::PLACEMENT_INDEX.offset).unwrap() as i16
    }

    fn count(&self) -> u32 {
        self.g.u32(ROOT + self.offset).unwrap()
    }

    fn slot(&self, index: u32) -> Vec<u8> {
        self.g.bytes(BLOCK + index * self.stride, self.stride as usize).unwrap()
    }

    fn name_placement(&self, name: u32) -> i16 {
        let at = NAMES + name * layout::OBJECT_NAME_STRIDE + layout::OBJECT_NAME_PLACEMENT as u32;
        self.g.u16(at).unwrap() as i16
    }
}

/// Run `plan` once. `spares` are the objects the engine hands out for
/// `object_new`, in order. A deleted object is flagged, as the game's is.
fn go(
    scene: &mut Scene,
    queue: &Shared,
    plan: &Plan,
    spares: &[u32],
) -> (Outcome, Vec<GuestCall>) {
    let call = Call::PlacementEdit { plan: Box::new(plan.clone()) };
    let got: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let sink = Arc::clone(&got);
    let q = Arc::clone(queue);
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        let (call, sink, q) = (call.clone(), Arc::clone(&sink), Arc::clone(&q));
        Box::pin(async move {
            let out = run(&cx, &call, SCRATCH, &q).await?;
            *sink.lock().unwrap() = Some(out);
            Ok(Ret::new(0, 0))
        })
    });
    let bodies = scene.bodies.clone();
    let mut spares = spares.to_vec();
    let r = drive(&*f, std::mem::take(&mut scene.g), Regs::default(), move |c, m| match c.target {
        addr::OBJECT_DELETE => {
            let key = c.args[0];
            let header = objects::array(&*m).unwrap().unwrap().get(&*m, DatumHandle(key)).unwrap().unwrap();
            m.write(
                header.addr + guest_layout::ObjectHeader::FLAGS.offset,
                &[guest_layout::object::header::DELETE],
            )
            .unwrap();
            Regs::default()
        }
        addr::PLACEMENT_NEW => Regs::default(),
        addr::OBJECT_NEW => {
            let made = spares.remove(0);
            // object_new copies the placement data's +0x10 into the object.
            let index = m.u16(SCRATCH + addr::PLACEMENT_INDEX).unwrap();
            m.write(
                bodies[&made] + guest_layout::Object::PLACEMENT_INDEX.offset,
                &index.to_le_bytes(),
            )
            .unwrap();
            al(made)
        }
        other => panic!("unexpected guest call {other:#x}"),
    });
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    scene.g = r.guest;
    let out = got.lock().unwrap().take().expect("an outcome");
    (out, r.calls)
}

fn targets(calls: &[GuestCall]) -> Vec<u32> {
    calls.iter().map(|c| c.target).collect()
}

#[test]
fn a_remove_and_its_undo_leave_the_block_as_the_save_does() {
    let recs = [record(1.0), record(2.0), record(3.0)];
    let mut s = Scene::new("scenery", &recs);
    let (o0, o1, o2) = (s.object(0), s.object(1), s.object(2));
    let spare = s.object(-1);
    let q = shared();

    // The save's swap-remove of entry 0: the last record over the slot.
    let remove = plan(json!({
        "block": "scenery", "was_count": 3, "count": 2,
        "records": [{"index": 0, "bytes": hex(&recs[2]), "was": hex(&recs[0])}],
        "delete": [0], "relink": [{"from": 2, "to": 0}],
    }));
    let (out, calls) = go(&mut s, &q, &remove, &[]);
    let out = out.unwrap();
    assert_eq!(targets(&calls), [addr::OBJECT_DELETE]);
    assert_eq!(calls[0].args, [o0]);
    assert_eq!(out["count"], 2);
    assert_eq!(out["capacity"], 3, "the loaded count is the room");
    assert_eq!(out["deleted"], json!([id(o0)]));
    assert_eq!(out["relinked"], json!([id(o2)]));
    assert_eq!(out["created"], json!([]));
    assert_eq!(s.count(), 2);
    assert_eq!((s.slot(0), s.slot(1)), (recs[2].clone(), recs[1].clone()));
    assert_eq!((s.placement_of(o1), s.placement_of(o2)), (1, 0));

    // The undo: the removed record back, the moved one home, an object made.
    let undo = plan(json!({
        "block": "scenery", "was_count": 2, "count": 3,
        "records": [
            {"index": 0, "bytes": hex(&recs[0]), "was": hex(&recs[2])},
            {"index": 2, "bytes": hex(&recs[2])},
        ],
        "relink": [{"from": 0, "to": 2}], "create": [0],
    }));
    let (out, calls) = go(&mut s, &q, &undo, &[spare]);
    let out = out.unwrap();
    assert_eq!(targets(&calls), [addr::PLACEMENT_NEW, addr::OBJECT_NEW]);
    assert_eq!(calls[0].args, [TAG, u32::MAX, 0]);
    assert_eq!(calls[1].args, [SCRATCH]);
    assert_eq!(out["count"], 3);
    assert_eq!(out["relinked"], json!([id(o2)]));
    assert_eq!(
        out["created"],
        json!([{"index": 0, "object": id(spare), "linked": true, "created": true}])
    );
    assert_eq!(s.count(), 3);
    assert_eq!([s.slot(0), s.slot(1), s.slot(2)], recs);
    assert_eq!((s.placement_of(o2), s.placement_of(spare)), (2, 0));
}

#[test]
fn an_add_into_the_slot_a_remove_left_reuses_it() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    let (o0, o1) = (s.object(0), s.object(1));
    let spare = s.object(-1);
    let q = shared();

    // Remove the last entry: only the count falls.
    let pop = plan(json!({"block": "scenery", "was_count": 2, "count": 1, "delete": [1]}));
    let (out, calls) = go(&mut s, &q, &pop, &[]);
    assert_eq!(out.unwrap()["deleted"], json!([id(o1)]));
    assert_eq!(targets(&calls), [addr::OBJECT_DELETE]);
    assert_eq!((s.count(), s.slot(1)), (1, recs[1].clone()), "the dead slot keeps its bytes");

    // Add a new record in that slot.
    let fresh = record(9.0);
    let add = plan(json!({
        "block": "scenery", "was_count": 1, "count": 2,
        "records": [{"index": 1, "bytes": hex(&fresh), "was": hex(&recs[1])}],
        "create": [1],
    }));
    let (out, _) = go(&mut s, &q, &add, &[spare]);
    let out = out.unwrap();
    assert_eq!(out["capacity"], 2);
    assert_eq!(out["created"][0]["object"], json!(id(spare)));
    assert_eq!(s.slot(1), fresh);
    assert_eq!((s.placement_of(o0), s.placement_of(spare)), (0, 1));
}

#[test]
fn an_add_past_the_loaded_count_is_refused_and_writes_nothing() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    s.object(0);
    let q = shared();
    let add = plan(json!({
        "block": "scenery", "was_count": 2, "count": 3,
        "records": [{"index": 2, "bytes": hex(&record(3.0))}], "create": [2],
    }));
    let (out, calls) = go(&mut s, &q, &add, &[]);
    let e = out.unwrap_err();
    assert_eq!(e.code, REFUSED);
    let data = e.data.unwrap();
    assert_eq!((&data["need"], &data["capacity"]), (&json!(3), &json!(2)));
    assert!(calls.is_empty());
    assert_eq!(s.count(), 2);
    assert_eq!([s.slot(0), s.slot(1)], recs);

    // The room is the activation's: a new one reads the count again.
    let pop = plan(json!({"block": "scenery", "was_count": 2, "count": 1}));
    go(&mut s, &q, &pop, &[]).0.unwrap();
    lock(&q).map_activated(None);
    let readd = plan(json!({"block": "scenery", "was_count": 1, "count": 2}));
    let e = go(&mut s, &q, &readd, &[]).0.unwrap_err();
    assert_eq!(e.data.unwrap()["capacity"], 1, "the count after the first edit is the new floor");
}

#[test]
fn a_stale_record_or_count_changes_nothing() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    s.object(0);
    let q = shared();
    let swap = |was: Value| {
        plan(json!({
            "block": "scenery", "was_count": 2, "count": 2,
            "records": [
                {"index": 0, "bytes": hex(&record(7.0)), "was": was},
            ],
            "delete": [0],
        }))
    };
    let (out, calls) = go(&mut s, &q, &swap(json!(hex(&recs[1]))), &[]);
    let e = out.unwrap_err();
    assert_eq!(e.code, code::MISMATCH);
    assert_eq!(e.data.unwrap()["records"], json!([{"index": 0}]));
    assert!(calls.is_empty(), "no object is touched before the checks pass");
    assert_eq!((s.count(), s.slot(0)), (2, recs[0].clone()));

    let wrong = plan(json!({"block": "scenery", "was_count": 5, "count": 5}));
    let e = go(&mut s, &q, &wrong, &[]).0.unwrap_err();
    assert_eq!(e.code, code::MISMATCH);
    assert_eq!(e.data.unwrap()["count"], 2);

    // The hash form: FNV-1a 64 of the bytes, 16 hex digits.
    let hash = format!("{:016x}", fnv1a(&recs[0]));
    let (out, _) = go(&mut s, &q, &swap(json!({ "hash": hash })), &[]);
    out.unwrap();
    assert_eq!(s.slot(0), record(7.0));
}

#[test]
fn a_record_the_engine_cannot_make_is_written_and_reported() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    let (_, o1) = (s.object(0), s.object(1));
    let q = shared();
    let mut named = record(5.0);
    named[2..4].copy_from_slice(&0u16.to_le_bytes());
    let replace = plan(json!({
        "block": "scenery", "was_count": 2, "count": 2,
        "records": [{"index": 1, "bytes": hex(&named), "was": hex(&recs[1])}],
        "delete": [1], "create": [1],
    }));
    let (out, calls) = go(&mut s, &q, &replace, &[]);
    let out = out.unwrap();
    assert_eq!(targets(&calls), [addr::OBJECT_DELETE], "no object_new for a named record");
    assert_eq!(out["deleted"], json!([id(o1)]));
    let made = &out["created"][0];
    assert_eq!((made["index"].as_u64(), made["created"].as_bool(), made["linked"].as_bool()),
        (Some(1), Some(false), Some(false)));
    assert!(made["why"].as_str().unwrap().contains("named placement"), "{made}");
    assert!(made.get("object").is_none());
    assert_eq!(s.slot(1), named, "the record is written for the reload");
}

#[test]
fn an_object_that_does_not_take_its_index_is_reported_unlinked() {
    let mut s = Scene::new("scenery", &[record(1.0)]);
    let spare = s.object(-1);
    let q = shared();
    // Rebuild the call by hand: the callee below ignores the placement data.
    let p = plan(json!({"block": "scenery", "was_count": 1, "count": 1, "create": [0]}));
    let call = Call::PlacementEdit { plan: Box::new(p) };
    let got: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let sink = Arc::clone(&got);
    let q2 = Arc::clone(&q);
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        let (call, sink, q) = (call.clone(), Arc::clone(&sink), Arc::clone(&q2));
        Box::pin(async move {
            *sink.lock().unwrap() = Some(run(&cx, &call, SCRATCH, &q).await?);
            Ok(Ret::new(0, 0))
        })
    });
    let r = drive(&*f, std::mem::take(&mut s.g), Regs::default(), move |c, _| {
        if c.target == addr::OBJECT_NEW { al(spare) } else { Regs::default() }
    });
    let out = got.lock().unwrap().take().unwrap().unwrap();
    assert_eq!(out["created"][0]["created"], true);
    assert_eq!(out["created"][0]["linked"], false);
    assert!(out["created"][0]["why"].as_str().unwrap().contains("placement index"));
    drop(r);
}

#[test]
fn a_name_follows_its_placement_and_must_be_of_the_blocks_type() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    let q = shared();
    let repoint = plan(json!({
        "block": "scenery", "was_count": 2, "count": 2,
        "names": [{"name": 0, "placement": 1}],
    }));
    go(&mut s, &q, &repoint, &[]).0.unwrap();
    assert_eq!(s.name_placement(0), 1);
    let clear = plan(json!({
        "block": "scenery", "was_count": 2, "count": 2, "names": [{"name": 0, "placement": -1}],
    }));
    go(&mut s, &q, &clear, &[]).0.unwrap();
    assert_eq!(s.name_placement(0), -1);

    // Name 1 names a vehicle, and name 9 does not exist.
    for name in [1, 9] {
        let wrong = plan(json!({
            "block": "scenery", "was_count": 2, "count": 2, "names": [{"name": name, "placement": 0}],
        }));
        let e = go(&mut s, &q, &wrong, &[]).0.unwrap_err();
        assert_eq!(e.code, REFUSED);
    }
    assert_eq!(s.name_placement(1), 0, "a refused repoint writes nothing");
}

#[test]
fn a_block_the_engine_walks_takes_records_and_a_count_only() {
    let stride = BlockSpec::named("netgame_equipment").unwrap().stride as usize;
    let recs = [vec![1; stride], vec![2; stride]];
    let mut s = Scene::new("netgame_equipment", &recs);
    let q = shared();
    let pop = plan(json!({"block": "netgame_equipment", "was_count": 2, "count": 1}));
    let (out, calls) = go(&mut s, &q, &pop, &[]);
    assert!(calls.is_empty());
    assert_eq!(out.unwrap()["created"], json!([]));
    let add = plan(json!({
        "block": "netgame_equipment", "was_count": 1, "count": 2,
        "records": [{"index": 1, "bytes": hex(&vec![7; stride])}],
    }));
    let (out, calls) = go(&mut s, &q, &add, &[]);
    assert!(calls.is_empty(), "the engine's own spawner reads the new entry");
    assert_eq!(out.unwrap()["count"], 2);
    assert_eq!((s.count(), s.slot(1)), (2, vec![7; stride]));
    let past = plan(json!({"block": "netgame_equipment", "was_count": 2, "count": 3}));
    assert_eq!(go(&mut s, &q, &past, &[]).0.unwrap_err().code, REFUSED);
}

#[test]
fn deleting_a_placement_deletes_its_respawn_preview_and_renumbers_the_rest() {
    let recs = [record(1.0), record(2.0), record(3.0)];
    let mut s = Scene::new("scenery", &recs);
    let (o0, _, o2) = (s.object(0), s.object(1), s.object(2));
    let (stale, moved) = (s.object(-1), s.object(-1));
    let q = shared();
    {
        let mut queue = lock(&q);
        for (index, object) in [(0, stale), (2, moved)] {
            let entry = Entry {
                old: None, kind: Kind::Scenery, index, tag: TAG, pos: [0.0; 3],
                axes: ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            };
            queue.previews.creating(&entry);
            queue.previews.remember(&entry, ObjectId { kind: Kind::Scenery, key: object });
        }
    }
    let remove = plan(json!({
        "block": "scenery", "was_count": 3, "count": 2,
        "records": [{"index": 0, "bytes": hex(&recs[2])}],
        "delete": [0], "relink": [{"from": 2, "to": 0}],
    }));
    let (out, calls) = go(&mut s, &q, &remove, &[]);
    let out = out.unwrap();
    assert_eq!(out["deleted"], json!([id(o0), id(stale)]));
    assert_eq!(calls.iter().map(|c| c.args[0]).collect::<Vec<_>>(), [o0, stale]);
    assert_eq!(s.placement_of(o2), 0);
    let queue = lock(&q);
    assert!(queue.previews.objects.contains_key(&(Kind::Scenery, 0)), "the moved preview is at 0");
    assert!(!queue.previews.objects.contains_key(&(Kind::Scenery, 2)));
    assert_eq!(
        queue.previews.objects[&(Kind::Scenery, 0)].current,
        Some(moved),
        "the preview that stood for entry 2 now stands for entry 0"
    );
}

#[test]
fn a_held_object_refuses_the_whole_transaction() {
    let recs = [record(1.0), record(2.0)];
    let mut s = Scene::new("scenery", &recs);
    let o0 = s.object(0);
    let holder = s.object(-1);
    Body::at(s.bodies[&o0]).set_parent(&mut s.g, Some(DatumHandle(holder))).unwrap();
    let q = shared();
    let remove = plan(json!({
        "block": "scenery", "was_count": 2, "count": 1,
        "records": [{"index": 0, "bytes": hex(&recs[1])}], "delete": [0],
    }));
    let (out, calls) = go(&mut s, &q, &remove, &[]);
    assert_eq!(out.unwrap_err().code, REFUSED);
    assert!(calls.is_empty());
    assert_eq!((s.count(), s.slot(0)), (2, recs[0].clone()));
}

#[test]
fn the_wire_is_checked_against_the_blocks_shape() {
    let ok = json!({"block": "scenery", "was_count": 1, "count": 1});
    assert!(!plan(ok).changes_count());
    assert!(plan(json!({"block": "scenery", "was_count": 1, "count": 2})).changes_count());
    let stride = BlockSpec::named("scenery").unwrap().stride as usize;
    let rec = hex(&vec![0; stride]);
    let cases = [
        json!({"block": "bipeds", "was_count": 0, "count": 0}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "records": [{"index": 0, "bytes": "00"}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "records": [{"index": 1, "bytes": rec}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "records": [{"index": 0, "bytes": rec}, {"index": 0, "bytes": rec}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "records": [{"index": 0, "bytes": "zz"}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "records": [{"index": 0, "bytes": rec, "was": {"hash": "12"}}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1, "delete": [1]}),
        json!({"block": "scenery", "was_count": 1, "count": 1, "create": [1]}),
        json!({"block": "scenery", "was_count": 2, "count": 2, "delete": [0, 0]}),
        json!({"block": "scenery", "was_count": 2, "count": 2,
               "delete": [0], "relink": [{"from": 0, "to": 1}]}),
        json!({"block": "scenery", "was_count": 2, "count": 2,
               "relink": [{"from": 0, "to": 1}, {"from": 1, "to": 1}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1,
               "names": [{"name": 0, "placement": 1}]}),
        json!({"block": "netgame_equipment", "was_count": 1, "count": 1, "create": [0]}),
        json!({"block": "netgame_equipment", "was_count": 1, "count": 1, "delete": [0]}),
        json!({"block": "netgame_equipment", "was_count": 1, "count": 1,
               "names": [{"name": 0, "placement": 0}]}),
        json!({"block": "scenery", "was_count": 1, "count": 1, "extra": 1}),
    ];
    for case in cases {
        let parsed = serde_json::from_value::<agent_proto::sim::PlacementEditParams>(case.clone());
        let rejected = parsed.map_or(true, |p| Plan::new(p).is_err());
        assert!(rejected, "{case}");
    }
    // The per-type limit: netgame equipment holds 100.
    let e = plan_err(json!({"block": "netgame_equipment", "was_count": 100, "count": 101}));
    assert_eq!(e.code, REFUSED);
    assert_eq!(e.data.unwrap()["limit"], 100);
}

#[test]
fn fnv1a_is_the_64_bit_variant() {
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
}

/// The edit arena for the fake game: a window mapped in the fake guest,
/// and pages that map anything while `free` physical bytes allow.
const ARENA: u32 = 0x8300_0000;

struct FakePages {
    free: u64,
}

impl arena::Pages for FakePages {
    fn map(&mut self, _: u32, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn unmap(&mut self, _: u32, _: u32) {}
    fn physical_free(&mut self) -> u64 {
        self.free
    }
}

fn arena_link(scene: &mut Scene, len: u32, free: u64) -> arena::Link {
    scene.g.map(ARENA, len);
    arena::Link {
        arena: Arc::new(Mutex::new(arena::Arena::new(ARENA..ARENA + len))),
        pages: Arc::new(move |f: &mut dyn FnMut(&mut dyn arena::Pages)| {
            f(&mut FakePages { free })
        }),
    }
}

#[test]
fn a_ninth_entry_on_a_block_of_eight_grows_it_into_the_arena() {
    let recs: Vec<Vec<u8>> = (0..8).map(|i| record(i as f32)).collect();
    let mut s = Scene::new("scenery", &recs);
    let spare = s.object(-1);
    let link = arena_link(&mut s, 0x1_0000, 64 << 20);
    let q = shared();
    lock(&q).set_arena(link.clone());
    let stride = s.stride;
    let root = ROOT + s.offset;

    let ninth = record(9.0);
    let add = plan(json!({
        "block": "scenery", "was_count": 8, "count": 9,
        "records": [{"index": 8, "bytes": hex(&ninth), "was": hex(&vec![0; stride as usize])}],
        "create": [8],
    }));
    let (out, calls) = go(&mut s, &q, &add, &[spare]);
    let out = out.unwrap();
    assert_eq!(out["moved"], json!({"address": ARENA, "capacity": 16}), "{out}");
    assert_eq!((out["count"].as_u64(), out["capacity"].as_u64()), (Some(9), Some(16)));
    assert_eq!(targets(&calls), [addr::PLACEMENT_NEW, addr::OBJECT_NEW]);
    assert_eq!(out["created"][0]["linked"], true, "{out}");
    // The root names the arena block; the old records were copied, the new
    // one written after them, the rest of the room zeroed.
    assert_eq!((s.count(), s.g.u32(root + 4).unwrap()), (9, ARENA));
    for (i, r) in recs.iter().enumerate() {
        assert_eq!(&s.g.bytes(ARENA + i as u32 * stride, stride as usize).unwrap(), r, "record {i}");
    }
    assert_eq!(s.g.bytes(ARENA + 8 * stride, stride as usize).unwrap(), ninth);
    assert_eq!(
        s.g.bytes(ARENA + 9 * stride, 7 * stride as usize).unwrap(),
        vec![0; 7 * stride as usize]
    );
    // The object was made from the entry in its new place.
    assert_eq!(s.placement_of(spare), 8);
    let e = *arena::lock(&link.arena).extent(ARENA).unwrap();
    assert_eq!((e.root, e.stride, e.capacity), (root, stride, 16));

    // A tenth fits the room the move made.
    let tenth = plan(json!({"block": "scenery", "was_count": 9, "count": 10,
        "records": [{"index": 9, "bytes": hex(&record(10.0))}]}));
    let out = go(&mut s, &q, &tenth, &[]).0.unwrap();
    assert_eq!(out["capacity"], 16);
    assert!(out.get("moved").is_none(), "{out}");
    assert_eq!(s.g.bytes(ARENA + 9 * stride, stride as usize).unwrap(), record(10.0));

    // Past sixteen it moves again, and the extent it left is freed.
    let more = plan(json!({"block": "scenery", "was_count": 10, "count": 17}));
    let out = go(&mut s, &q, &more, &[]).0.unwrap();
    let address = out["moved"]["address"].as_u64().unwrap() as u32;
    assert_ne!(address, ARENA);
    assert_eq!(out["moved"]["capacity"], 32);
    assert_eq!(s.g.u32(root + 4).unwrap(), address);
    assert_eq!(s.g.bytes(address + 8 * stride, stride as usize).unwrap(), ninth);
    let held = arena::lock(&link.arena);
    assert!(held.extent(ARENA).is_none());
    assert_eq!(held.extents().count(), 1);
}

#[test]
fn a_full_arena_refuses_the_growth_and_writes_nothing() {
    let recs: Vec<Vec<u8>> = (0..8).map(|i| record(i as f32)).collect();
    let mut s = Scene::new("scenery", &recs);
    let spare = s.object(-1);
    // Nothing mapped yet and no physical memory past the floor to map.
    let link = arena_link(&mut s, 0x1_0000, arena::PHYSICAL_FLOOR);
    let q = shared();
    lock(&q).set_arena(link.clone());
    let add = plan(json!({
        "block": "scenery", "was_count": 8, "count": 9,
        "records": [{"index": 8, "bytes": hex(&record(9.0))}], "create": [8],
    }));
    let (out, calls) = go(&mut s, &q, &add, &[spare]);
    let e = out.unwrap_err();
    assert_eq!(e.code, REFUSED);
    let data = e.data.unwrap();
    assert_eq!(
        (&data["need"], &data["capacity"], &data["largest_free"]),
        (&json!(9), &json!(8), &json!(0))
    );
    assert!(calls.is_empty());
    assert_eq!((s.count(), s.g.u32(ROOT + s.offset + 4).unwrap()), (8, BLOCK));
    assert!(arena::lock(&link.arena).is_empty());
}
