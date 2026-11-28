//! The object visit on invented lists and portal graphs (no game data).
use super::*;
use crate::flood::TopologyKey;
use crate::synth::{CONSTS, Image, PortalSpec, Portals, Reference, References, square};

const BASE: u32 = 0x10_000;
const CLUSTER_MARKS: u32 = BASE + 0xc000;
const OBJECT_MARKS: u32 = BASE + 0xc800;
const SLOTS: u32 = 0x100;
const GLOBALS: u32 = BASE + 0xd000;
const LISTS: [(u32, u32, u32); 2] = [
    (BASE + 0xd100, BASE + 0xd200, BASE + 0xd300),
    (BASE + 0xe100, BASE + 0xe200, BASE + 0xe300),
];
const FLOOD_EPOCH: u32 = 6;
const GENERATION: u32 = 40;

/// Clusters 0, 1, 2 in a row, walls at `x = 1` and `x = 2`.
fn row() -> Portals {
    let mut p = Portals {
        clusters: vec![Vec::new(); 3],
        ..Portals::default()
    };
    for k in 1..3i16 {
        p.planes.push([1.0, 0.0, 0.0, k as f32]);
        let mut vertices = square(0, k as f32, (0.0, 0.0), 1.0);
        vertices.reverse();
        p.link(
            k - 1,
            k,
            PortalSpec {
                plane: k as u32 - 1,
                sphere: [k as f32, 0.0, 0.0, 1.5],
                vertices,
                ..PortalSpec::default()
            },
        );
    }
    p
}

fn reference(handle: u32, kind: u8, at: [f32; 3], r: f32) -> Reference {
    Reference {
        handle,
        kind,
        bounds: [at[0], at[1], at[2], r],
    }
}

struct World {
    img: Image,
    key: TopologyKey,
}

fn world(lists: [&References; 2]) -> World {
    let mut img = Image::new(BASE, 0x10_000);
    let (_, bsp) = row().write(BASE, |a, d| img.put(a, d));
    for (k, list) in lists.into_iter().enumerate() {
        let (heads, array, data) = LISTS[k];
        img.word(GLOBALS + 8 * k as u32, heads);
        img.word(GLOBALS + 8 * k as u32 + 4, array);
        list.write(heads, array, data, |a, d| img.put(a, d));
    }
    World {
        img,
        key: TopologyKey {
            structure: BASE,
            bsp,
        },
    }
}

fn query() -> VisitQuery {
    VisitQuery {
        lists: 0,
        types: 0,
        cluster: 0,
        centre: [0.5f32, 0.0, 0.0].map(f32::to_bits),
        radius: 0.75f32.to_bits(),
        capacity: 8,
        zero: 0,
        control: 0x027f,
        x87: true,
        cluster_marks: CLUSTER_MARKS,
        flood_epoch: FLOOD_EPOCH,
        object_marks: OBJECT_MARKS,
        generation: GENERATION,
        slots: SLOTS,
        lists_at: [0, 1].map(|k| List {
            heads: GLOBALS + 8 * k,
            array: GLOBALS + 8 * k + 4,
        }),
    }
}

fn run(w: &World, q: &VisitQuery) -> Result<Visit, Bail> {
    object_visit(&w.img, q, || {
        Ok(Arc::new(Topology::decode(&w.img, w.key, &CONSTS.projection)?))
    })
}

/// Three objects in cluster 0's first list, one again in its second, one
/// in cluster 1's first list and one in cluster 2 (not flooded).
fn lists() -> [References; 2] {
    let a = References {
        clusters: vec![
            vec![
                reference(0x1234_0003, 1, [0.5, 0.5, 0.0], 0.1),
                reference(0x1111_0004, 1, [10.0, 0.0, 0.0], 1.0),
            ],
            vec![reference(0x5555_0005, 2, [1.2, 0.0, 0.0], 0.1)],
            vec![reference(0x6666_0006, 1, [2.5, 0.0, 0.0], 9.0)],
        ],
    };
    let b = References {
        clusters: vec![
            vec![
                reference(0x1234_0003, 1, [0.5, 0.5, 0.0], 0.1),
                reference(0x7777_0007, 3, [0.5, 0.0, 0.25], 0.0),
            ],
            vec![],
            vec![],
        ],
    };
    [a, b]
}

#[test]
fn the_visit_floods_then_walks_both_lists_in_cluster_order() {
    let [a, b] = lists();
    let w = world([&a, &b]);
    let v = run(&w, &query()).unwrap();
    assert_eq!(v.flood.as_ref().unwrap().clusters, [0, 1]);
    assert_eq!(v.marks, [3, 4, 7, 5], "a miss is marked; a repeat is not");
    assert_eq!(v.handles, [0x1234_0003, 0x7777_0007, 0x5555_0005]);
    assert_eq!((v.full, v.eax()), (None, 3));
    assert!(v.reads.iter().any(|&(at, _)| at == GLOBALS));
}

#[test]
fn list_bits_and_type_masks_filter_before_the_mark() {
    let [a, b] = lists();
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { lists: 2, ..query() }).unwrap();
    assert_eq!(v.handles, [0x1234_0003, 0x7777_0007]);
    let v = run(&w, &VisitQuery { types: 1 << 2, ..query() }).unwrap();
    assert_eq!(v.marks, [5]);
    assert_eq!(v.handles, [0x5555_0005]);
    // The shift takes the type byte modulo 32.
    let [a, mut b] = lists();
    b.clusters[0][1].kind = 35;
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { types: 1 << 3, lists: 2, ..query() }).unwrap();
    assert_eq!(v.handles, [0x7777_0007]);
}

#[test]
fn a_full_output_stops_at_the_handle_that_found_it_full() {
    let [a, b] = lists();
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { capacity: 1, ..query() }).unwrap();
    assert_eq!(v.handles, [0x1234_0003]);
    assert_eq!(v.full, Some(0x7777_0007));
    assert_eq!(v.marks, [3, 4, 7], "the full handle was marked first");
    assert_eq!(v.eax(), 0x7777_0001);
    let v = run(&w, &VisitQuery { capacity: 0, ..query() }).unwrap();
    assert_eq!((v.handles.len(), v.full, v.eax()), (0, Some(0x1234_0003), 0x1234_0000));
    let v = run(&w, &VisitQuery { capacity: 0xffff, ..query() }).unwrap();
    assert_eq!(v.full, Some(0x1234_0003), "a negative capacity is always full");
}

#[test]
fn no_cluster_visits_nothing_and_no_radius_visits_one_cluster() {
    let [a, b] = lists();
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { cluster: 0xffff, ..query() }).unwrap();
    assert_eq!(v, Visit::default());
    for radius in [0.0f32, -0.0, -0.25] {
        let q = VisitQuery {
            cluster: 1,
            radius: radius.to_bits(),
            x87: false,
            ..query()
        };
        let v = run(&w, &q).unwrap();
        assert!(v.flood.is_none());
        assert_eq!(v.marks, [5]);
        assert!(v.handles.is_empty(), "nothing overlaps a point");
    }
    assert_eq!(run(&w, &VisitQuery { x87: false, ..query() }), Err(Bail::Env));
    let nan = VisitQuery {
        radius: f32::NAN.to_bits(),
        ..query()
    };
    assert_eq!(run(&w, &nan), Err(Bail::Float));
    // Under the guest's MXCSR a NaN radius is not above zero (`comiss`
    // unordered, IE), so one cluster is visited, as for no radius.
    let (v, raised) = object_visit_guest(&w.img, &nan, 0x1f80, || {
        Ok(Arc::new(Topology::decode(&w.img, w.key, &CONSTS.projection)?))
    })
    .unwrap();
    assert!(v.flood.is_none());
    assert_ne!(raised.mxcsr & 0x01, 0);
}

#[test]
fn guest_marks_and_the_topology_are_read_only_when_needed() {
    let [a, b] = lists();
    let mut w = world([&a, &b]);
    // Object 3 already visited in this generation, cluster 1 flooded in
    // this epoch.
    w.img.word(OBJECT_MARKS + 4 * 3, GENERATION + 1);
    w.img.word(CLUSTER_MARKS + 4, FLOOD_EPOCH + 1);
    let v = run(&w, &query()).unwrap();
    assert_eq!(v.flood.as_ref().unwrap().clusters, [0]);
    assert_eq!(v.handles, [0x7777_0007]);
    // A visit without a flood never asks for the graph.
    let q = VisitQuery {
        radius: 0,
        ..query()
    };
    let v = object_visit(&w.img, &q, || panic!("no flood")).unwrap();
    // Cluster 0 alone: its object 3 is already marked, and the other
    // object sits in cluster 1, which no flood reaches.
    assert!(v.handles.is_empty());
}

#[test]
fn handles_past_the_mark_bound_decline_and_a_dead_handle_ends_a_list() {
    let [mut a, b] = lists();
    a.clusters[0][0].handle = 0x1234_0000 | SLOTS;
    let w = world([&a, &b]);
    assert_eq!(run(&w, &query()), Err(Bail::Data));
    // Filtered out by type, the slot is never looked at.
    let [mut a, b] = lists();
    a.clusters[0][0].handle = 0x1234_0000 | SLOTS;
    a.clusters[0][0].kind = 9;
    let w = world([&a, &b]);
    assert!(run(&w, &VisitQuery { types: 0b1110, ..query() }).is_ok());
    // A second record whose handle is -1 ends the walk there.
    let [mut a, b] = lists();
    a.clusters[0][1].handle = u32::MAX;
    a.clusters[0].push(reference(0x2222_0002, 1, [0.5, 0.0, 0.0], 1.0));
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { lists: 1, ..query() }).unwrap();
    assert_eq!(v.marks, [3, 5]);
}

#[test]
fn a_dead_first_handle_ends_the_list_regardless_of_its_next_reference() {
    let [mut a, b] = lists();
    a.clusters[0][0].handle = u32::MAX;
    let w = world([&a, &b]);
    let v = run(&w, &VisitQuery { lists: 1, radius: 0, ..query() }).unwrap();
    assert!(v.handles.is_empty());
    assert!(v.marks.is_empty());
}

#[test]
fn a_cycle_in_the_live_reference_list_declines() {
    let [a, b] = lists();
    let mut w = world([&a, &b]);
    w.img.word(LISTS[0].2 + References::SIZE + layout::NEXT, 0);
    assert_eq!(run(&w, &query()), Err(Bail::Data));
}

#[test]
fn visit_batch_advances_the_flood_epoch_only_on_flooding_visits() {
    let [a, b] = lists();
    let mut w = world([&a, &b]);
    let q = VisitQuery { flood_epoch: u32::MAX - 1, ..query() };
    let no_flood = VisitQuery { radius: 0, ..q };
    let queries = [q, no_flood, q];
    let topology = Arc::new(Topology::decode(&w.img, w.key, &CONSTS.projection).unwrap());
    let answers = crate::batch::visit_batch(&w.img, &queries, || Ok(topology.clone())).unwrap();
    assert_eq!(answers[1].flood_epoch, None);
    assert_eq!(answers[2].flood_epoch, Some(0));
    assert_eq!(answers[2].visit.flood.as_ref().unwrap().clusters, [0, 1]);
    let (mut epoch, mut generation) = (q.flood_epoch, q.generation);
    for (query, answer) in queries.iter().zip(answers) {
        let serial = run(&w, &VisitQuery { flood_epoch: epoch, generation, ..*query }).unwrap();
        assert_eq!(serial, answer.visit);
        if let Some(f) = serial.flood {
            epoch = epoch.wrapping_add(1);
            for c in f.clusters {
                w.img.word(CLUSTER_MARKS + 4 * u32::from(c), epoch);
            }
        }
        generation = generation.wrapping_add(1);
        for mark in serial.marks {
            w.img.word(OBJECT_MARKS + 4 * u32::from(mark), generation);
        }
    }
}

#[test]
fn reference_packets_keep_each_list_rounding_order_and_fall_back_on_wide_values() {
    let minor = 1.0f32 / 4096.0;
    let refs = |base: u32| References {
        clusters: vec![(1..=4).map(|i| reference(base + i, 1, [1.0, minor, minor], 1.0)).collect()],
    };
    let (a, b) = (refs(0x1234_0000), refs(0x5678_0004));
    let w = world([&a, &b]);
    let q = VisitQuery { centre: [0; 3], radius: 0, ..query() };
    // (2^-24 + 2^-24) + 1.0 rounds up. (2^-24 + 1.0) + 2^-24
    // rounds to 1 twice. Both sets are marked; only list two touches.
    let v = run(&w, &q).unwrap();
    assert_eq!(v.marks, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(v.handles, [0x5678_0005, 0x5678_0006, 0x5678_0007, 0x5678_0008]);

    let mut a = a;
    a.clusters[0][3].bounds[0] = f32::from_bits(0x1c80_0000);
    let w = world([&a, &b]);
    let q = VisitQuery { centre: [1.0f32.to_bits(), 0, 0], lists: 1, ..q };
    let v = run(&w, &q).unwrap();
    assert_eq!(v.handles, [0x1234_0001, 0x1234_0002, 0x1234_0003]);
}

#[test]
fn a_full_visit_stops_before_a_bad_or_unreadable_later_reference() {
    let a = References { clusters: vec![(1..=8).map(|i| reference(i, 0, [0.0; 3], 1.0)).collect()] };
    let b = References { clusters: vec![vec![]] };
    let mut w = world([&a, &b]);
    // Four hits in a packet, then the fifth hit ends a full output.
    let q = VisitQuery { capacity: 4, radius: 0, centre: [0; 3], ..query() };
    let (_, _, data) = LISTS[0];
    w.img.word(data + 4 * References::SIZE + layout::NEXT, u32::MAX - 1);
    let v = run(&w, &q).unwrap();
    assert_eq!(v.handles, [1, 2, 3, 4]);
    assert_eq!(v.marks, [1, 2, 3, 4, 5]);
    assert_eq!(v.full, Some(5));
}
