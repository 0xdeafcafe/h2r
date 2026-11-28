use std::ops::Range;

use super::*;
use crate::light::Light;
use crate::object::ItemKey;
use crate::scene::{Instance, PartDraw, PartFlags, SceneId};
use crate::test_camera::camera;
use crate::InstanceKey;

fn close(a: V3, b: V3) -> bool {
    dist(a, b) < 1e-4
}

fn key(kind: ViewKind) -> ViewKey {
    ViewKey { kind, player: 0, view: 0 }
}

fn node(position: V3, yaw: f32) -> NodeMatrix {
    let (s, c) = yaw.sin_cos();
    NodeMatrix { scale: 1.0, forward: [c, s, 0.0], left: [-s, c, 0.0], up: [0.0, 0.0, 1.0], position }
}

fn rows_of(n: &NodeMatrix) -> [Row; 3] {
    Affine::from_node(n).to_rows()
}

fn view(kind: ViewKind, camera: Camera, instances: Range<u32>, lights: Range<u32>) -> View {
    View {
        key: key(kind),
        cut: false,
        camera,
        clusters: 0..0,
        instances,
        order: 0..0,
        world: Default::default(),
        remainder: 0,
        display_visibility: None,
        display_transparents: None,
        aux_depth: None,
        structure_parameters: None,
        lights,
        layers: 0..0,
        decorators: 0..0,
        decorator_remainder: 0,
        decals: 0..0,
        decal_remainder: 0,
        effects: Default::default(),
        fog: None,
        water: None,
        water_composite: None,
        ocean: None,
        post: None,
    }
}

fn looking(yaw: f32, position: V3) -> Camera {
    Camera { position, forward: [yaw.cos(), yaw.sin(), 0.0], ..camera() }
}

fn part_key() -> ItemKey {
    ItemKey { record: 1, shader: 2, section: 0, part: 0, implementation: 0 }
}

/// One view with one object of one node and one part of one palette row.
fn scene(kind: ViewKind, cam: Camera, n: NodeMatrix, tick: u32) -> FrameScene {
    let mut s = FrameScene::default();
    s.id = SceneId { map_epoch: 1, frame: tick as u64, tick };
    s.stamp.ticks_run = 1;
    s.views.push(view(kind, cam, 0..1, 0..0));
    s.nodes.push(n);
    s.rows.extend(rows_of(&n));
    s.instances.push(Instance {
        key: InstanceKey(7),
        definition: 1,
        model: 2,
        nodes: 0..1,
        sphere: ([0.0; 3], 1.0),
        lighting: None,
        defaults: None,
        parts: 0..1,
        teleported: false,
    });
    s.parts.push(PartDraw {
        slot: 0,
        key: part_key(),
        layer: 3,
        instance: 0,
        rows: 0..3,
        flags: PartFlags::NONE,
        subparts: None,
    });
    s
}

fn world(a: NodeMatrix, b: NodeMatrix) -> (FrameScene, FrameScene) {
    (
        scene(ViewKind::Game, camera(), a, 1),
        scene(ViewKind::Game, camera(), b, 2),
    )
}

#[test]
fn the_plan_is_alpha_over_the_publish_gap_with_one_tick_of_delay() {
    let base = Instant::now();
    let (mut a, mut b) = world(node([0.0; 3], 0.0), node([1.0; 3], 0.0));
    a.stamp.published = Some(base);
    b.stamp.published = Some(base + Duration::from_millis(33));
    let at = |ms: u64| plan(&a, &b, base + Duration::from_millis(33 + ms));
    assert_eq!(at(0), Plan::Blend(0.0));
    let Plan::Blend(half) = at(16) else { panic!() };
    assert!((half - 16.0 / 33.0).abs() < 1e-3);
    assert_eq!(at(33), Plan::Newest, "alpha 1 draws B itself");
    assert_eq!(at(80), Plan::Newest, "a late next scene holds at 1");
    assert_eq!(plan(&a, &b, base), Plan::Blend(0.0), "never before B");
}

#[test]
fn no_blend_on_a_pause_a_cut_a_gap_or_another_map() {
    let base = Instant::now();
    let fresh = || {
        let (mut a, mut b) = world(node([0.0; 3], 0.0), node([1.0; 3], 0.0));
        a.stamp.published = Some(base);
        b.stamp.published = Some(base + Duration::from_millis(33));
        (a, b)
    };
    let due = base + Duration::from_millis(40);
    let (a, mut b) = fresh();
    b.stamp.ticks_run = 0;
    assert_eq!(plan(&a, &b, due), Plan::Newest);
    let (a, mut b) = fresh();
    b.cut = crate::Cut::LOAD;
    assert_eq!(plan(&a, &b, due), Plan::Newest);
    let (mut a, mut b) = fresh();
    b.stamp.published = Some(base + Duration::from_millis(46));
    assert_eq!(plan(&a, &b, due), Plan::Newest, "45 ms apart is the limit");
    a.stamp.published = None;
    assert_eq!(plan(&a, &b, due), Plan::Newest, "unstamped");
    let (mut a, b) = fresh();
    a.id.map_epoch = 0;
    assert_eq!(plan(&a, &b, due), Plan::Newest);
    let (a, mut b) = fresh();
    b.stamp.published = Some(base);
    assert_eq!(plan(&a, &b, due), Plan::Newest, "no gap");
}

#[test]
fn quaternions_round_trip_and_slerp_by_angle() {
    for yaw in [0.0f32, 0.4, 1.6, 3.0, -2.5] {
        let cols = Affine::from_node(&node([0.0; 3], yaw)).columns;
        let back = Quat::from_columns(cols).columns();
        for j in 0..3 {
            assert!(close(cols[j], back[j]), "yaw {yaw} axis {j}");
        }
    }
    let q0 = Quat::from_columns(Affine::from_node(&node([0.0; 3], 0.0)).columns);
    let q1 = Quat::from_columns(Affine::from_node(&node([0.0; 3], 1.0)).columns);
    let mid = q0.slerp(q1, 0.5).columns();
    assert!(close(mid[0], [0.5f32.cos(), 0.5f32.sin(), 0.0]));
    let long = Quat::from_columns(Affine::from_node(&node([0.0; 3], 3.1)).columns);
    let wrap = Quat::from_columns(Affine::from_node(&node([0.0; 3], -3.1)).columns);
    let across = long.slerp(wrap, 0.5).columns();
    assert!(close(across[0], [-1.0, 0.0, 0.0]), "the short way, through pi: {:?}", across[0]);
}

#[test]
fn the_camera_blends_position_and_fov_linearly_and_orientation_by_slerp() {
    let a = looking(0.0, [0.0, 0.0, 1.0]);
    let b = Camera { fov: a.fov + 0.2, ..looking(std::f32::consts::FRAC_PI_2, [4.0, 0.0, 1.0]) };
    let c = blend_camera(&a, &b, 0.5).unwrap();
    assert!(close(c.position, [2.0, 0.0, 1.0]));
    assert!(close(c.forward, [0.5f32.sqrt(), 0.5f32.sqrt(), 0.0]));
    assert!(close(c.up, [0.0, 0.0, 1.0]));
    assert!((c.fov - (a.fov + 0.1)).abs() < 1e-6);
    assert_eq!((c.near, c.far, c.viewport), (b.near, b.far, b.viewport));
    assert!(c.is_usable());
    let bad = Camera { forward: [f32::NAN; 3], ..a };
    assert!(blend_camera(&a, &bad, 0.5).is_none());
}

#[test]
fn views_blend_unless_cut_or_new_and_the_rest_is_b() {
    let (mut a, mut b) = world(node([0.0; 3], 0.0), node([1.0, 0.0, 0.0], 0.0));
    a.views[0].camera = looking(0.0, [0.0; 3]);
    b.views[0].camera = looking(0.0, [2.0, 0.0, 0.0]);
    let out = blend(&a, &b, 0.25);
    assert!(close(out.cameras[0].position, [0.5, 0.0, 0.0]));
    assert_eq!(out.counts.cameras_blended, 1);
    b.views[0].cut = true;
    let out = blend(&a, &b, 0.25);
    assert_eq!(out.cameras[0], b.views[0].camera);
    assert_eq!(out.counts.cameras_snapped, 1);
    b.views[0].cut = false;
    b.views[0].key.view = 3;
    let out = blend(&a, &b, 0.25);
    assert_eq!(out.cameras[0], b.views[0].camera, "no view of the key in A");
}

#[test]
fn nodes_and_rows_blend_translation_linear_and_rotation_by_slerp() {
    let (a, b) = world(node([0.0, 0.0, 0.0], 0.0), node([2.0, 2.0, 0.0], 1.0));
    let out = blend(&a, &b, 0.5);
    let n = out.nodes[0];
    assert!(close(n.position, [1.0, 1.0, 0.0]));
    assert!(close(n.forward, [0.5f32.cos(), 0.5f32.sin(), 0.0]));
    assert!((n.scale - 1.0).abs() < 1e-5);
    let r = Affine::from_rows(&out.rows[0..3]).unwrap();
    assert!(close(r.t, n.position));
    assert!(close(r.columns[0], n.forward));
    assert_eq!((out.counts.instances_blended, out.counts.parts_blended), (1, 1));
}

#[test]
fn scale_blends_linearly() {
    let mut big = node([0.0; 3], 0.0);
    big.scale = 3.0;
    let (a, b) = world(node([0.0; 3], 0.0), big);
    let out = blend(&a, &b, 0.5);
    assert!((out.nodes[0].scale - 2.0).abs() < 1e-5);
}

#[test]
fn blending_at_zero_gives_a_and_the_ends_are_not_extrapolated() {
    let (a, b) = world(node([0.0, 0.0, 0.0], 0.2), node([1.0, 0.0, 0.0], 0.9));
    let out = blend(&a, &b, 0.0);
    assert!(close(out.nodes[0].position, a.nodes[0].position));
    assert!(close(out.nodes[0].forward, a.nodes[0].forward));
    let past = blend(&a, &b, 7.0);
    assert!(close(past.nodes[0].position, b.nodes[0].position), "alpha is clamped");
}

#[test]
fn an_instance_snaps_to_b_when_teleported_moved_far_changed_or_new() {
    let far = |a: &FrameScene, b: &FrameScene| blend(a, b, 0.5);
    let (a, b) = world(node([0.0; 3], 0.0), node([MAX_MOVE + 1.0, 0.0, 0.0], 0.0));
    let out = far(&a, &b);
    assert_eq!(out.nodes, b.nodes);
    assert_eq!(out.rows, b.rows);
    assert_eq!((out.counts.instances_snapped, out.counts.parts_snapped), (1, 1));

    let (a, mut b) = world(node([0.0; 3], 0.0), node([1.0, 0.0, 0.0], 0.0));
    b.instances[0].teleported = true;
    assert_eq!(far(&a, &b).nodes, b.nodes);

    let (a, mut b) = world(node([0.0; 3], 0.0), node([1.0, 0.0, 0.0], 0.0));
    b.instances[0].model = 99;
    let out = far(&a, &b);
    assert_eq!((out.nodes, out.rows), (b.nodes.clone(), b.rows.clone()));

    let (a, mut b) = world(node([0.0; 3], 0.0), node([1.0, 0.0, 0.0], 0.0));
    b.instances[0].key = InstanceKey(8);
    assert_eq!(far(&a, &b).nodes, b.nodes, "no match in A");

    let (mut a, b) = world(node([0.0; 3], 0.0), node([1.0, 0.0, 0.0], 0.0));
    a.nodes.push(node([0.0; 3], 0.0));
    a.instances[0].nodes = 0..2;
    assert_eq!(far(&a, &b).nodes, b.nodes, "another node count");
}

#[test]
fn a_part_snaps_alone_when_its_rows_do_not_match() {
    let (mut a, b) = world(node([0.0; 3], 0.0), node([2.0, 0.0, 0.0], 0.0));
    a.parts[0].key.part = 5;
    let out = blend(&a, &b, 0.5);
    assert_eq!(out.rows, b.rows, "no part of the key");
    assert_eq!(out.nodes[0].position, [1.0, 0.0, 0.0], "the nodes still blend");
    let (mut a, b) = world(node([0.0; 3], 0.0), node([2.0, 0.0, 0.0], 0.0));
    a.rows.pop();
    a.parts[0].rows = 0..2;
    assert_eq!(blend(&a, &b, 0.5).rows, b.rows, "another row count");
}

#[test]
fn a_sheared_or_mirrored_row_snaps() {
    let mut sheared = rows_of(&node([0.0; 3], 0.0));
    sheared[0][1] = 0.5;
    let ok = rows_of(&node([1.0, 0.0, 0.0], 0.0));
    assert!(blend_rows(&sheared, &ok, 0.5, None).is_none());
    let mut mirrored = rows_of(&node([0.0; 3], 0.0));
    mirrored[0][0] = -1.0;
    assert!(blend_rows(&ok, &mirrored, 0.5, None).is_none());
    assert!(blend_rows(&ok[..2], &ok[..2], 0.5, None).is_none(), "not whole rows");
}

#[test]
fn the_first_person_stays_locked_to_the_blended_camera() {
    // The weapon is one unit ahead of its camera in both scenes; the
    // camera turns a quarter circle. In world space the midpoint would
    // sit 0.71 from the eye; in camera space it stays 1 ahead.
    let ahead = |yaw: f32| node([yaw.cos(), yaw.sin(), 0.0], yaw);
    let mut a = scene(ViewKind::FirstPerson, looking(0.0, [0.0; 3]), ahead(0.0), 1);
    let mut b = scene(
        ViewKind::FirstPerson,
        looking(std::f32::consts::FRAC_PI_2, [0.0; 3]),
        ahead(std::f32::consts::FRAC_PI_2),
        2,
    );
    for s in [&mut a, &mut b] {
        s.views[0].camera.position = [0.0; 3];
    }
    let out = blend(&a, &b, 0.5);
    let cam = out.cameras[0];
    let n = out.nodes[0];
    assert!((len(n.position) - 1.0).abs() < 1e-4, "{:?}", n.position);
    assert!(close(n.position, cam.forward), "ahead of the blended camera");
    assert!(close(n.forward, cam.forward));
    let r = Affine::from_rows(&out.rows[0..3]).unwrap();
    assert!(close(r.t, cam.forward), "the rows follow too");

    // The same pair as a world view blends in world space.
    let (mut a, mut b) = (a, b);
    a.views[0].key.kind = ViewKind::Game;
    b.views[0].key.kind = ViewKind::Game;
    let out = blend(&a, &b, 0.5);
    assert!((len(out.nodes[0].position) - 0.5f32.sqrt()).abs() < 1e-4);
}

#[test]
fn lights_blend_by_datum_and_definition() {
    let candidate = |x: f32, intensity: f32| LightCandidate {
        position: [x, 0.0, 0.0],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        colour_a: [0.0; 3],
        colour_b: [1.0; 3],
        intensity,
        bounds: ([0.0; 3], 1.0),
        owner: None,
        definition_kind: 0,
        volume: None,
        view_eligible: None,
        priority: None,
        weight: 1.0,
        gel_bitmap: None,
        gel_shader: None,
        constants: None,
    };
    let light = |handle, definition, c| Light {
        handle,
        definition,
        candidate: Some(c),
        ..Light::default()
    };
    let (mut a, mut b) = world(node([0.0; 3], 0.0), node([0.0; 3], 0.0));
    a.lights.push(light(5, 9, candidate(0.0, 1.0)));
    b.lights.push(light(5, 9, candidate(4.0, 3.0)));
    b.lights.push(light(6, 9, candidate(4.0, 3.0)));
    b.views[0].lights = 0..2;
    a.views[0].lights = 0..1;
    let out = blend(&a, &b, 0.5);
    let l = out.lights[0].as_ref().unwrap();
    assert!(close(l.position, [2.0, 0.0, 0.0]));
    assert_eq!(l.intensity, 2.0);
    assert!(close(l.colour_b, [1.0; 3]));
    assert!(out.lights[1].is_none(), "a new light has no earlier value");
    assert_eq!(out.counts.lights_blended, 1);
    b.lights[0].definition = 10;
    assert!(blend(&a, &b, 0.5).lights[0].is_none(), "another definition");
}

#[test]
fn the_blended_camera_is_found_by_key() {
    let (a, b) = world(node([0.0; 3], 0.0), node([0.0; 3], 0.0));
    let out = blend(&a, &b, 0.5);
    assert!(out.camera(&b, key(ViewKind::Game)).is_some());
    assert!(out.camera(&b, key(ViewKind::Alternate)).is_none());
}
