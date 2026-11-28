use super::*;
use crate::lights::handles;
use crate::visibility::flood::tests::camera_at;
use crate::visibility::flood::{Lens, Rect};
use crate::visibility::frusta::WindowFrusta;
use crate::visibility::frusta::tests::INPUTS;
use guest_layout::FlatMemory;

const ARRAY: u32 = 0x0058_0000;
const RECORDS: u32 = 0x0058_1000;
const USED: u32 = 0x0058_a000;
const HEADS: u32 = 0x0058_b000;
const POOL: u32 = 0x0058_c000;
const NODES: u32 = 0x0058_d000;
const LIST_AT: u32 = 0x0059_5000;
const HANDLES: u32 = 0x0059_6000;
const SALT: u32 = 0xb123_0000;

const fn handle(index: u32) -> u32 {
    SALT | index
}

/// Lights, by slot:
/// 0 in cluster 1, ahead: listed.
/// 1 in cluster 1, behind the camera: outside the view.
/// 2 in cluster 2, ahead and right (the window's half): listed.
/// 3 in cluster 2, ahead and left: outside cluster 2's window.
/// 4 in cluster 7, which the view does not reach (behind a portal).
/// 5 in cluster 1, ahead, not live.
/// 6 in clusters 1 and 2, ahead: listed once, by cluster 1.
/// 7 with no cluster, taken by the player-only tail.
/// 8 with a cluster, taken by `0x000c3950` but not by the tail's test.
const SPHERES: [(u16, [f32; 3]); 9] = [
    (1, [5.0, 0.0, 0.0]),
    (1, [-5.0, 0.0, 0.0]),
    (2, [10.0, -3.0, 0.0]),
    (2, [10.0, 3.0, 0.0]),
    (7, [5.0, 0.0, 0.0]),
    (1, [5.0, 1.0, 0.0]),
    (1, [6.0, -1.0, 0.0]),
    (u16::MAX, [-9.0, 0.0, 0.0]),
    (3, [-9.0, 0.0, 0.0]),
];
const REFS: [(u32, &[u32]); 3] = [(1, &[0, 1, 5, 6]), (2, &[2, 3, 6]), (7, &[4])];

fn guest(view_index: i32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0044_0000, 0x0016_0000);
    for (addr, value) in [(0x0045_dbdc, 0.0001), (0x0045_dc1c, 0.01)] {
        m.set_f32(addr, value).unwrap();
    }
    for (addr, value) in [
        (light::ARRAY_POINTER, ARRAY),
        (guest_layout::tags::TAG_ENTRIES_GLOBAL, 0x0059_0000),
        (ARRAY + 0x24, light::RECORD_SIZE as u32),
        (ARRAY + 0x38, SPHERES.len() as u32),
        (ARRAY + 0x44, RECORDS),
        (ARRAY + 0x48, USED),
        (USED, (1 << SPHERES.len()) - 1),
        (CLUSTER_LIGHTS.0, HEADS),
        (CLUSTER_LIGHTS.1, POOL),
        (POOL + 0x24, 16),
        (POOL + 0x38, 16),
        (POOL + 0x44, NODES),
        (VIEW_INDEX, view_index as u32),
        (MAIN_VIS + LIST, LIST_AT),
        (LIST_AT, 8),
        (LIST_AT + 0xc, HANDLES),
    ] {
        m.set_u32(addr, value).unwrap();
    }
    for cluster in 0..16 {
        m.set_u32(HEADS + 4 * cluster, u32::MAX).unwrap();
    }
    // Each cluster's nodes are a run in the pool, each pointing at the next.
    let mut node = 0;
    for (cluster, handles) in REFS {
        m.set_u32(HEADS + 4 * cluster, 0x7000_0000 | node).unwrap();
        for (i, &light) in handles.iter().enumerate() {
            let at = NODES + 16 * node;
            m.set_u32(at + 4, handle(light)).unwrap();
            let next = if i + 1 == handles.len() { u32::MAX } else { 0x7000_0000 | (node + 1) };
            m.set_u32(at + 8, next).unwrap();
            node += 1;
        }
    }
    for (i, (cluster, centre)) in SPHERES.into_iter().enumerate() {
        let at = RECORDS + i as u32 * light::RECORD_SIZE as u32;
        m.set_u16(at, (SALT >> 16) as u16).unwrap();
        m.set_u16(at + 2, if i == 5 { 0 } else { 2 }).unwrap();
        m.set_u32(at + 4, u32::MAX).unwrap();
        for (k, v) in centre.into_iter().enumerate() {
            m.set_f32(at + layout::BOUNDS_CENTRE as u32 + 4 * k as u32, v).unwrap();
        }
        m.set_f32(at + layout::BOUNDS_RADIUS as u32, 0.5).unwrap();
        m.set_u16(at + CLUSTER, cluster).unwrap();
        m.set_f32(at + layout::VISIBLE_WEIGHT as u32, 1.0).unwrap();
        m.set_f32(at + layout::COLOUR_A as u32, 1.0).unwrap();
    }
    m
}

/// Clusters 1 and 2 seen, 2 through the right half of the screen only.
fn reached() -> Reached {
    Reached { clusters: vec![1, 2], windows: vec![(1, Rect::SCREEN), (2, Rect([0.0, 1.0, -1.0, 1.0]))] }
}

fn clip(r: &Reached) -> Clip {
    let lens = Lens::new(&camera_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])).unwrap();
    Clip::Frusta(WindowFrusta::build(&r.windows, &lens, &INPUTS))
}

fn view(capacity: usize) -> GatherView {
    GatherView { flags: 0, start: 1, capacity }
}

/// What the guest's gather leaves in `[0x00547f88 + 0x10]` for the fixture,
/// by its walk (`0x00132220`, `0x00132a30`, `0x00132589`).
fn guest_list(m: &mut FlatMemory, list: &[u32]) {
    m.set_u16(LIST_AT + 4, list.len() as u16).unwrap();
    for (i, &h) in list.iter().enumerate() {
        m.set_u32(HANDLES + 4 * i as u32, h).unwrap();
    }
}

/// The two lights `0x000c3950` takes, as the tail finds them: their owner
/// test needs a local player's camera, which the fixture does not build.
fn with_player_only(world: &mut LightWorld, m: &FlatMemory) {
    let sphere = |light: u32| {
        let at = RECORDS + light * light::RECORD_SIZE as u32;
        LightInfo {
            active: visible::active(m, at).unwrap(),
            centre: m.vec3(at + layout::BOUNDS_CENTRE as u32).unwrap(),
            radius: 0.5,
            cluster: m.u16(at + CLUSTER).unwrap(),
        }
    };
    world.player_only = vec![handle(7), handle(8)];
    for light in [7, 8] {
        world.lights.insert(handle(light), sphere(light));
    }
}

#[test]
fn the_light_gather_equals_the_guest_list() {
    let mut m = guest(-1);
    let r = reached();
    let mut world = read_world(&m, &r.clusters).unwrap();
    assert_eq!(world.player_only, Vec::<u32>::new(), "view index -1 takes none");
    assert_eq!(world.refs[&1], [0, 1, 5, 6].map(handle), "the references in list order");
    assert!(!world.lights[&handle(5)].active, "a light that is not live");
    assert!(!world.refs.contains_key(&7), "a cluster the view does not reach is not read");
    assert_eq!(capacity(&m), Some(8));
    // With a view index the slots are walked; none has a definition here.
    assert_eq!(read_world(&guest(0), &r.clusters).unwrap().player_only, Vec::<u32>::new());

    // Lights inside the view, outside it, behind a portal, not live and
    // listed twice: cluster 1 in list order, then cluster 2.
    let inside = [handle(0), handle(6), handle(2)];
    let got = gather(&world, &view(8), &r, &clip(&r));
    assert_eq!(got, inside);
    guest_list(&mut m, &inside);
    assert_eq!(handles(&m).unwrap(), got, "the guest's list as the extraction reads it");

    // A light behind a portal is listed when its cluster is reached.
    let far = Reached { clusters: vec![1, 2, 7], ..reached() };
    let behind = read_world(&m, &far.clusters).unwrap();
    assert_eq!(gather(&behind, &view(8), &far, &Clip::All), [0, 1, 6, 2, 3, 4].map(handle));

    // The player-only tail follows, for a view inside the structure.
    with_player_only(&mut world, &m);
    let with_tail = gather(&world, &view(8), &r, &clip(&r));
    assert_eq!(with_tail, [handle(0), handle(6), handle(2), handle(7)]);
    guest_list(&mut m, &with_tail);
    assert_eq!(handles(&m).unwrap(), with_tail);
    let outside = GatherView { start: u16::MAX, ..view(8) };
    assert_eq!(gather(&world, &outside, &r, &clip(&r)), inside, "no tail outside the structure");
}

#[test]
fn the_gather_stops_at_the_guests_cap() {
    let m = guest(-1);
    let r = reached();
    let mut world = read_world(&m, &r.clusters).unwrap();
    with_player_only(&mut world, &m);
    // The list holds one fewer than its capacity.
    assert_eq!(gather(&world, &view(5), &r, &clip(&r)).len(), 4);
    assert_eq!(gather(&world, &view(4), &r, &clip(&r)), [0, 6, 2].map(handle), "the tail's light is refused");
    assert_eq!(gather(&world, &view(3), &r, &clip(&r)), [0, 6].map(handle), "light 2 is refused, not stamped");
    assert_eq!(gather(&world, &view(1), &r, &clip(&r)), Vec::<u32>::new());
    assert_eq!(gather(&world, &view(0), &r, &clip(&r)), Vec::<u32>::new());
}

#[test]
fn the_view_flags_and_modes_change_the_list() {
    let m = guest(-1);
    let r = reached();
    let world = read_world(&m, &r.clusters).unwrap();
    let none = GatherView { flags: NO_LIGHTS, ..view(8) };
    assert_eq!(gather(&world, &none, &r, &clip(&r)), Vec::<u32>::new(), "flag 0x10 lists no lights");
    let all = GatherView { flags: view_flags::ADMIT_ALL, ..view(8) };
    assert_eq!(
        gather(&world, &all, &r, &clip(&r)),
        [0, 1, 6, 2, 3].map(handle),
        "flag 0x80 takes what the clip rejects; a light not live is never taken"
    );
    let sphere = Clip::Sphere { centre: [5.0, 0.0, 0.0], radius: 1.0 };
    assert_eq!(gather(&world, &view(8), &r, &sphere), [0, 6].map(handle));
    assert_eq!(gather(&world, &view(8), &r, &Clip::All), [0, 1, 6, 2, 3].map(handle));
}

#[test]
fn the_main_view_list_is_the_world_gathered_under_the_guests_capacity() {
    let m = guest(-1);
    let r = reached();
    let v = view(capacity(&m).unwrap());
    let world = read_world(&m, &r.clusters).unwrap();
    assert_eq!(
        main_view(&m, v.flags, v.start, &r.clusters, &clip(&r)),
        Some(gather(&world, &v, &r, &clip(&r)))
    );
}
