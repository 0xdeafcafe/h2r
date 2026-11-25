//! The whole computation over invented guest memory: two rooms joined by a
//! square portal (`graph::tests::two_rooms`), three objects, and the main
//! view's visibility struct; then the differential against guest lists,
//! one equal and one deliberately not.

use engine::render_loop::occlusion::guest::{
    MAIN_VIS, OBJECTS, VIEW_PLAYER, VIS_KIND, VIS_OBJECTS,
};
use frame_scene::VisibilitySource;
use frame_scene::scene::ClusterGather;

use super::flood::tests::camera_at;
use super::graph::tests::two_rooms;
use super::guest::{
    ACTIVE_STRUCTURE, ACTIVE_VIS, CAMERA_TEST_ZERO, CLUSTER_OBJECTS, FIRST_PERSON_CAMERA,
    FRUSTUM_BOUNDS, FRUSTUM_NEGATIVE_UNIT, FRUSTUM_UNIT, LOCAL_PLAYERS, MERGE,
    NORMALIZE_EPSILON, OPEN_PORTALS, PLAYER_GLOBALS, PLAYER_SIZE, PLAYER_UNIT, PLAYERS,
    PROJECTION_ROWS, SKY_CLUSTER_ENABLED, SPHERE_FLOOD_MIN, STRADDLE, USER_CAMERA_KIND,
    USER_CAMERA_TEST, USER_CAMERAS, VIS_SPHERE_CENTRE, VIS_SPHERE_RADIUS, WIDEN, view_frustum,
    vis,
};
use super::sphere::tests::ROWS;
use super::*;
use crate::testing::Mem;

const STRUCTURE: u32 = 0x8000;
const OPEN: u32 = 0xd000;
const NONE: u32 = u32::MAX;

/// A datum array header at `header` with `end` slots of `stride` at `data`.
fn pool(m: &mut Mem, header: u32, stride: u32, end: u32, data: u32) {
    m.w(header + 0x24, stride);
    m.w(header + 0x38, end);
    m.w(header + 0x44, data);
}

/// Object `index` (salt 1) with its sphere at `centre`, radius 0.5.
fn object(m: &mut Mem, index: u32, centre: [f32; 3]) {
    let (slot, body) = (0xa100 + 12 * index, 0xb000 + 0x100 * index);
    m.w(slot, 1);
    m.w(slot + 8, body);
    for (i, v) in centre.into_iter().enumerate() {
        m.f(body + 0x30 + 4 * i as u32, v);
    }
    m.f(body + 0x3c, 0.5);
    m.w(body + 0xa8, 0);
}

/// The camera in room 0 at the origin looking at the portal (x = 4).
/// Room 0 references object 1 at (2, 0, 0); room 1 references object 2 at
/// (10, 0.5, 0), seen through the portal, and object 3 at (10, 6, 0), off
/// the portal's window.
fn scene() -> Mem {
    let mut m = Mem::default();
    two_rooms(&mut m, STRUCTURE);
    m.w(ACTIVE_VIS, MAIN_VIS);
    m.w(MAIN_VIS + VIS_KIND, 0);
    m.w(MAIN_VIS + VIS_OBJECTS, 0xe000);
    m.w(0xe000, 64);
    m.w(MAIN_VIS + vis::START_CLUSTER, 0);
    m.w(MAIN_VIS + vis::FLAGS, 0);
    m.w(MAIN_VIS + vis::FRUSTA, 1);
    m.w(MAIN_VIS + vis::MODE, 0);
    m.w(ACTIVE_STRUCTURE, STRUCTURE);
    window_fields(&mut m);
    m.w(MAIN_VIS, 0xf000);
    m.w(0xf000 + view_frustum::FIRST + view_frustum::TEST_PORTAL_FAR, 1);
    m.f(0xf000 + view_frustum::FIRST + view_frustum::FAR, frusta::tests::INPUTS.far);
    m.w(0xf000 + view_frustum::FIRST + view_frustum::HAS_CLIP_PLANE, 0);
    m.w(OPEN_PORTALS, OPEN);
    m.w(OPEN, 1);
    m.w(VIEW_PLAYER, NONE);
    m.w(SKY_CLUSTER_ENABLED & !3, 0);
    // Objects.
    m.w(OBJECTS, 0xa000);
    pool(&mut m, 0xa000, 12, 16, 0xa100);
    object(&mut m, 1, [2.0, 0.0, 0.0]);
    object(&mut m, 2, [10.0, 0.5, 0.0]);
    object(&mut m, 3, [10.0, 6.0, 0.0]);
    // The first partition: room 0 lists object 1, room 1 objects 2 and 3.
    let [(heads, refs), (heads2, refs2)] = CLUSTER_OBJECTS;
    m.w(heads, 0xc000);
    m.w(refs, 0xc100);
    pool(&mut m, 0xc100, 12, 16, 0xc200);
    m.w(0xc000, 0x0002_0000);
    m.w(0xc004, 0x0002_0001);
    for (index, handle, next) in [
        (0, 0x0001_0001, NONE),
        (1, 0x0001_0002, 0x0002_0002),
        (2, 0x0001_0003, NONE),
    ] {
        let slot = 0xc200 + 12 * index;
        m.w(slot, 2);
        m.w(slot + 4, handle);
        m.w(slot + 8, next);
    }
    // The second partition is empty.
    m.w(heads2, 0xc400);
    m.w(refs2, 0xc500);
    pool(&mut m, 0xc500, 12, 0, 0xc600);
    m.w(0xc400, NONE);
    m.w(0xc404, NONE);
    m
}

pub(super) fn window_fields(m: &mut Mem) {
    let straddle = 0.01f64.to_bits();
    m.w(STRADDLE, straddle as u32);
    m.w(STRADDLE + 4, (straddle >> 32) as u32);
    m.f(WIDEN, 0.05);
    m.f(MERGE, 0.8);
    // The window frusta's values and the view frustum, no clip plane.
    let inputs = frusta::tests::INPUTS;
    m.f(FRUSTUM_UNIT, inputs.unit);
    m.f(FRUSTUM_NEGATIVE_UNIT, inputs.negative_unit);
    m.f(NORMALIZE_EPSILON, inputs.epsilon);
    m.w(FRUSTUM_BOUNDS, 0xf400);
    for (i, v) in inputs.bounds.into_iter().enumerate() {
        m.f(0xf400 + 4 * i as u32, v);
    }
}

fn run(m: &Mem) -> Result<VisibilitySets, Decline> {
    compute(
        m,
        &camera_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
        &mut GraphCache::default(),
        1,
    )
}

fn guest_lists(visible: Vec<u16>, objects: Vec<u32>) -> VisibilitySets {
    guest::guest_sets(&ClusterGather {
        mode: 0,
        visible,
        structure: Vec::new(),
        objects,
        source: VisibilitySource::Guest,
    })
}

#[test]
fn the_flood_reaches_through_the_portal_and_lists_the_objects_it_sees() {
    let m = scene();
    let rust = run(&m).unwrap();
    assert_eq!(
        rust,
        VisibilitySets {
            clusters: vec![0, 1],
            objects: vec![0x0001_0001, 0x0001_0002]
        }
    );
    // The guest's lists in its own order: equal as sets.
    let p = compare(
        &rust,
        &guest_lists(vec![1, 0], vec![0x0001_0002, 0x0001_0001]),
    );
    assert!(p.holds(), "{p:?}");
}

#[test]
fn a_deliberate_mismatch_names_the_object_the_guest_listed() {
    let rust = run(&scene()).unwrap();
    let p = compare(
        &rust,
        &guest_lists(vec![0, 1], vec![0x0001_0001, 0x0001_0002, 0x0001_0003]),
    );
    assert!(!p.holds());
    assert_eq!(p.first, Some(Mismatch::ObjectGuestOnly(0x0001_0003)));
    assert_eq!(
        (
            p.objects_guest_only,
            p.objects_rust_only,
            p.clusters_guest_only
        ),
        (1, 0, 0)
    );
}

#[test]
fn a_side_portal_and_its_object_enter_the_wider_view_together() {
    let mut m = scene();
    object(&mut m, 2, [10.0, -9.0, 0.0]);
    let mut camera = camera_at([0.0, 7.0, 0.0], [1.0, 0.0, 0.0]);
    // The nearest portal edge is six units sideways at a depth of four.
    // At a vertical FOV of 90 degrees its tangent lies between 4:3 and 16:9.
    let mut cache = GraphCache::default();
    let mut visible = |m: &Mem, aspect: f32| {
        camera.projection_xy = Some([1.0 / aspect, 1.0, 0.0, 0.0]);
        compute(m, &camera, &mut cache, 1).unwrap()
    };
    let narrow = visible(&m, 4.0 / 3.0);
    assert_eq!(narrow.clusters, [0]);
    assert!(!narrow.objects.contains(&0x0001_0002));
    let wide = visible(&m, 16.0 / 9.0);
    assert_eq!(wide.clusters, [0, 1]);
    assert!(wide.objects.contains(&0x0001_0002));
    assert!(!wide.objects.contains(&0x0001_0003));
    m.w(OPEN, 0);
    assert_eq!(visible(&m, 16.0 / 9.0).clusters, [0]);
}

#[test]
fn a_shut_portal_hides_the_room_behind_it_and_its_objects() {
    let mut m = scene();
    m.w(OPEN, 0);
    assert_eq!(
        run(&m).unwrap(),
        VisibilitySets {
            clusters: vec![0],
            objects: vec![0x0001_0001]
        }
    );
}

/// Local user 0 (the word at `VIEW_PLAYER`) mapped to player `0x0003_0002`,
/// whose unit is `unit`, with a camera of `kind`.
fn local_player(m: &mut Mem, unit: u32, kind: u32) {
    const GLOBALS: u32 = 0x2_0000;
    const HEADER: u32 = 0x2_1000;
    const DATA: u32 = 0x2_2000;
    m.w(VIEW_PLAYER, 0);
    m.w(PLAYER_GLOBALS, GLOBALS);
    m.w(GLOBALS + LOCAL_PLAYERS, 0x0003_0002);
    for user in 1..4 {
        m.w(GLOBALS + LOCAL_PLAYERS + 4 * user, NONE);
    }
    m.w(PLAYERS, HEADER);
    m.w(HEADER + 0x44, DATA);
    m.w(DATA + 2 * PLAYER_SIZE + PLAYER_UNIT, unit);
    m.w(USER_CAMERAS + USER_CAMERA_KIND, kind);
    m.f(USER_CAMERAS + USER_CAMERA_TEST, 0.0);
    m.f(CAMERA_TEST_ZERO, 0.0);
}

#[test]
fn the_player_unit_is_listed_even_when_no_cluster_references_it() {
    let mut m = scene();
    local_player(&mut m, 0x0001_0009, FIRST_PERSON_CAMERA);
    assert_eq!(
        run(&m).unwrap().objects,
        [0x0001_0001, 0x0001_0002, 0x0001_0009]
    );
}

/// The word at `VIEW_PLAYER` is the view's local user, 0 for the first.
/// Read as a unit it put object `0x00000000` in every Rust set, and every
/// main view on 05b_deltatowers differed by that object alone
/// (`object 0x00000000 rust only`). The guest appends the user's unit, here
/// already listed through room 0.
#[test]
fn the_first_local_user_is_not_listed_as_object_zero() {
    let mut m = scene();
    local_player(&mut m, 0x0001_0001, FIRST_PERSON_CAMERA);
    let rust = run(&m).unwrap();
    assert_eq!(rust.objects, [0x0001_0001, 0x0001_0002]);
    let p = compare(&rust, &guest_lists(vec![0, 1], vec![0x0001_0002, 0x0001_0001]));
    assert!(p.holds(), "{p:?}");
}

#[test]
fn only_a_mapped_user_in_the_first_person_camera_appends_a_unit() {
    let listed = |m: &Mem| run(m).unwrap().objects;
    let mut m = scene();
    local_player(&mut m, 0x0001_0009, 0x0023_cbb0);
    assert_eq!(listed(&m), [0x0001_0001, 0x0001_0002], "another camera kind");
    local_player(&mut m, 0x0001_0009, FIRST_PERSON_CAMERA);
    m.f(USER_CAMERAS + USER_CAMERA_TEST, 0.5);
    assert_eq!(listed(&m), [0x0001_0001, 0x0001_0002], "the camera's test float is not zero");
    m.f(USER_CAMERAS + USER_CAMERA_TEST, f32::NAN);
    assert_eq!(listed(&m), [0x0001_0001, 0x0001_0002], "unordered is not equal");
    local_player(&mut m, NONE, FIRST_PERSON_CAMERA);
    assert_eq!(listed(&m), [0x0001_0001, 0x0001_0002], "a player without a unit");
    local_player(&mut m, 0x0001_0009, FIRST_PERSON_CAMERA);
    m.w(0x2_0000 + LOCAL_PLAYERS, NONE);
    assert_eq!(listed(&m), [0x0001_0001, 0x0001_0002], "an unmapped user");
    m.w(VIEW_PLAYER, 4);
    assert_eq!(run(&m), Err(Decline::Layout), "no fifth local user");
}

#[test]
fn mode_two_takes_the_visible_set_and_does_not_clip_objects() {
    let mut m = scene();
    m.w(MAIN_VIS + vis::MODE, 2);
    let sets = run(&m).unwrap();
    assert_eq!(sets.clusters, [0, 1]);
    assert_eq!(sets.objects, [0x0001_0001, 0x0001_0002, 0x0001_0003]);
    // Room 1's set is computed at run time: from its own row (the
    // structure has one table) through portals that join computed sets,
    // of which the fixture's one portal is not.
    m.w(MAIN_VIS + vis::START_CLUSTER, 1);
    let sets = run(&m).unwrap();
    assert_eq!(sets.clusters, [1]);
    assert_eq!(sets.objects, [0x0001_0002, 0x0001_0003]);
}

#[test]
fn the_sphere_mode_floods_through_the_portal_and_lists_the_objects_its_sphere_reaches() {
    let mut m = scene();
    m.w(MAIN_VIS + vis::MODE, 1);
    for (i, v) in [3.5, 0.0, 0.0].into_iter().enumerate() {
        m.f(MAIN_VIS + VIS_SPHERE_CENTRE + 4 * i as u32, v);
    }
    m.f(MAIN_VIS + VIS_SPHERE_RADIUS, 1.2);
    for (r, [a, b]) in ROWS.into_iter().enumerate() {
        let at = PROJECTION_ROWS + 6 * r as u32;
        // Rows of three halves: `a` and `b` lead each, the third is unused.
        let word = |m: &Mem, w: u32| m.0.get(&w).copied().unwrap_or(0);
        for (k, v) in [a, b].into_iter().enumerate() {
            let addr = at + 2 * k as u32;
            let shift = (addr & 2) * 8;
            let old = word(&m, addr & !3);
            m.w(addr & !3, (old & !(0xffff << shift)) | (u32::from(v) << shift));
        }
    }
    m.f(SPHERE_FLOOD_MIN, 0.0);
    // Room 0's object 1 at (2, 0, 0) is within 1.2 + 0.5 of the centre;
    // room 1's are not.
    assert_eq!(
        run(&m).unwrap(),
        VisibilitySets {
            clusters: vec![0, 1],
            objects: vec![0x0001_0001]
        }
    );
    // A deliberate mismatch: the guest listed object 2 as well.
    let p = compare(
        &run(&m).unwrap(),
        &guest_lists(vec![0, 1], vec![0x0001_0001, 0x0001_0002]),
    );
    assert_eq!(p.first, Some(Mismatch::ObjectGuestOnly(0x0001_0002)));
    // Not above the flood's minimum: the start cluster alone.
    m.f(SPHERE_FLOOD_MIN, 2.0);
    assert_eq!(run(&m).unwrap().clusters, [0]);
}

#[test]
fn views_it_does_not_cover_decline_by_name() {
    let mut m = scene();
    m.w(MAIN_VIS + vis::MODE, 3);
    assert_eq!(run(&m), Err(Decline::Mode));
    let mut m = scene();
    m.w(MAIN_VIS + vis::FRUSTA, 2);
    assert_eq!(run(&m), Err(Decline::Frusta));
    let mut m = scene();
    m.w(ACTIVE_VIS, MAIN_VIS + 8);
    assert_eq!(run(&m), Err(Decline::NotMain));
    let mut m = scene();
    m.w(OPEN_PORTALS, 0);
    assert_eq!(run(&m), Err(Decline::Unread));
    assert_eq!(Decline::ALL.map(Decline::name).len(), 7);
}

#[test]
fn the_camera_outside_the_structure_sees_no_clusters() {
    let mut m = scene();
    m.w(MAIN_VIS + vis::START_CLUSTER, 0xffff);
    assert_eq!(run(&m).unwrap(), VisibilitySets::default());
}

#[test]
fn the_cache_keeps_a_graph_until_the_map_or_its_shape_changes() {
    let m = scene();
    let mut cache = GraphCache::default();
    assert_eq!(cache.graph(&m, 1, STRUCTURE).unwrap().shape(), (2, 1));
    let mut m = m;
    // Bad portal clusters: a new map decodes again and declines.
    m.w(0x9200, 5 << 16);
    assert_eq!(
        cache.graph(&m, 1, STRUCTURE).unwrap().shape(),
        (2, 1),
        "same map, same shape: kept"
    );
    assert_eq!(cache.graph(&m, 2, STRUCTURE).err(), Some(Decline::Layout));
    assert_eq!(
        cache.graph(&m, 2, STRUCTURE).err(),
        Some(Decline::Layout),
        "kept as declined"
    );
    assert_eq!(cache.graph(&m, 2, 0x4_0000).err(), Some(Decline::Unread));
}

#[test]
fn the_arm_takes_the_rust_sets_whenever_it_computes_them() {
    let rust = run(&scene()).unwrap();
    let gather = |visible: Vec<u16>, objects: Vec<u32>| ClusterGather {
        mode: 0,
        visible,
        structure: vec![1],
        objects,
        source: VisibilitySource::Guest,
    };
    let (taken, p) = arm(
        gather(vec![1, 0], vec![0x0001_0002, 0x0001_0001]),
        rust.clone(),
    );
    assert!(p.holds());
    assert_eq!(taken.source, VisibilitySource::Rust);
    assert_eq!(
        (taken.visible, taken.objects),
        (vec![0, 1], vec![0x0001_0001, 0x0001_0002])
    );
    assert_eq!(taken.structure, [1], "the structure list stays the guest's");
    // A deliberate mismatch: the guest saw a cluster Rust did not. The
    // Rust sets stand — the default — and the difference stays counted
    // and named; a view whose computation declined never reaches the arm.
    let guest_gather = gather(vec![1, 0, 7], vec![0x0001_0002, 0x0001_0001]);
    let (stood, p) = arm(guest_gather, rust);
    assert_eq!(p.first, Some(Mismatch::ClusterGuestOnly(7)));
    assert_eq!(stood.source, VisibilitySource::Rust);
    assert_eq!(
        (stood.visible, stood.objects),
        (vec![0, 1], vec![0x0001_0001, 0x0001_0002])
    );
    assert_eq!(stood.structure, [1]);
}

#[test]
fn the_window_far_distance_can_end_before_the_render_camera_far_plane() {
    let mut m = scene();
    m.f(0xf000 + view_frustum::FIRST + view_frustum::FAR, 6.0);
    let sets = run(&m).unwrap();
    assert_eq!(sets.clusters, [0, 1]);
    assert_eq!(sets.objects, [0x0001_0001]);
    let parity = compare(&sets, &guest_lists(vec![0, 1], vec![0x0001_0001, 0x0001_0002]));
    assert_eq!(parity.first, Some(Mismatch::ObjectGuestOnly(0x0001_0002)));
}

#[test]
fn the_portal_far_test_is_optional_even_with_a_finite_window_distance() {
    let mut m = scene();
    let view = 0xf000 + view_frustum::FIRST;
    m.f(view + view_frustum::FAR, 2.5);
    assert_eq!(run(&m).unwrap().clusters, [0]);
    m.w(view + view_frustum::TEST_PORTAL_FAR, 0);
    let sets = run(&m).unwrap();
    assert_eq!(sets.clusters, [0, 1]);
    assert_eq!(sets.objects, [0x0001_0001]);
}

#[test]
fn the_stored_gather_frustum_adds_only_the_side_room_to_the_display() {
    let mut m = scene();
    let frustum = 0xf000 + view_frustum::FIRST;
    fn floats(m: &mut Mem, base: u32, values: &[f32]) {
        for (i, &value) in values.iter().enumerate() {
            m.f(base + 4 * i as u32, value);
        }
    }
    fn operand(m: &mut Mem, base: u32, value: f32) {
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            let addr = base + i as u32;
            let shift = 8 * (addr % 4);
            let w = m.0.entry(addr & !3).or_default();
            *w = (*w & !(0xff << shift)) | (u32::from(byte) << shift);
        }
    }
    // An eye at (0, 7, 0), facing +x: local right is -y and up is +z.
    floats(&mut m, frustum + 4, &[1., 0.,0.,-1., -1.,0.,0., 0.,1.,0., 7.,0.,0.]);
    floats(&mut m, frustum + 0x38, &[1., 0.,-1.,0., 0.,0.,1., -1.,0.,0., 0.,7.,0.]);
    let root = flood::Rect([-4./3., 4./3., -1., 1.]);
    floats(&mut m, frustum + 0xa0, &root.0);
    m.w(frustum + 0x198, 4);
    for (i, p) in root.corners().iter().enumerate() {
        floats(&mut m, frustum + 0x19c + 8*i as u32, p);
    }
    operand(&mut m, 0x0017_35a9, 0.03);
    operand(&mut m, 0x0017_3acc, 0.02);
    operand(&mut m, 0x0017_338a, 0.0002);
    m.f(0x0045_ddfc, -0.0001);
    m.f(0x0044_aed4, 5000.);
    m.w(0x0046_87dc, 0xf500);
    floats(&mut m, 0xf500, &[10000., -10000., 10000., -10000.]);
    for r in 0..6 {
        for (k, v) in [0u32, 1, 2].into_iter().enumerate() {
            let a = PROJECTION_ROWS + 6*r + 2*k as u32;
            *m.0.entry(a & !3).or_default() |= v << (8 * (a % 4));
        }
    }
    object(&mut m, 2, [10., -9., 0.]);
    let mut cache = GraphCache::default();
    let (retail,_) = portal_walk::scene_visibility(&m, &mut cache, 1, 1.).unwrap();
    assert_eq!(retail.clusters, [0]);
    let (wide,volumes) = portal_walk::scene_visibility(&m, &mut cache, 1, 0.75).unwrap();
    assert_eq!(wide.clusters, [0,1]);
    assert!(volumes.admits(1, [10.,-9.,0.], 0.5));
    assert!(wide.objects.contains(&0x0001_0002));
    m.w(OPEN,0);
    assert_eq!(portal_walk::scene_visibility(&m,&mut cache,1,0.75).unwrap().0.clusters,[0]);
}

/// The display supplement (`render_bridge::scene_step::display_additions`)
/// keeps what the wide lens reaches beyond both retail sets, whether or not
/// the guest-lens sets equal the guest's lists. Here they are equal, the
/// arm takes the Rust sets, and the side room and its object still come
/// through as display-only additions.
#[test]
fn display_additions_survive_equal_guest_lens_sets() {
    let mut m = scene();
    object(&mut m, 2, [10.0, -9.0, 0.0]);
    local_player(&mut m, 0x0001_0001, FIRST_PERSON_CAMERA);
    let mut camera = camera_at([0.0, 7.0, 0.0], [1.0, 0.0, 0.0]);
    let mut cache = GraphCache::default();
    camera.projection_xy = Some([3.0 / 4.0, 1.0, 0.0, 0.0]);
    let narrow = compute(&m, &camera, &mut cache, 1).unwrap();
    let gather = ClusterGather {
        mode: 0,
        visible: vec![0],
        structure: Vec::new(),
        objects: vec![0x0001_0001],
        source: VisibilitySource::Guest,
    };
    let (taken, parity) = arm(gather.clone(), narrow.clone());
    assert!(parity.holds(), "{parity:?}");
    assert_eq!(taken.source, VisibilitySource::Rust);
    camera.projection_xy = Some([9.0 / 16.0, 1.0, 0.0, 0.0]);
    let mut wide = compute(&m, &camera, &mut cache, 1).unwrap();
    wide.retain_display_additions(&narrow, &gather);
    assert_eq!(wide, VisibilitySets::new(vec![1], vec![0x0001_0002]));
}
