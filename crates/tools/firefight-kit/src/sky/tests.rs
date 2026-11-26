//! The sky test and the pods on invented collision: no input files.
use super::*;

/// A horizontal square of side 2*`half` centred on (`x`, 0) at height `z`.
fn quad(x: f32, z: f32, half: f32) -> Vec<[V3; 3]> {
    let c = |dx: f32, dy: f32| [x + dx, dy, z];
    vec![
        [c(-half, -half), c(half, -half), c(half, half)],
        [c(-half, -half), c(half, half), c(-half, half)],
    ]
}

fn walls(tris: Vec<[V3; 3]>) -> Walls {
    Walls::new(tris)
}

const START: V3 = [0.0, 0.0, 0.0];

#[test]
fn a_start_under_a_roof_has_no_fall() {
    let solid = walls([quad(0.0, 0.0, 50.0), quad(0.0, 30.0, 5.0)].concat());
    let sky = walls(Vec::new());
    assert_eq!(plan(&Roof { solid: &solid, sky: &sky }, &[START, [20.0, 0.0, 0.0]])[0], None);
    // A start beside the roof is open: the fall is the 30 up to the top.
    let open = plan(&Roof { solid: &solid, sky: &sky }, &[[20.0, 0.0, 0.0]]);
    assert_eq!(open[0].unwrap().top, [20.0, 0.0, 30.0]);
}

#[test]
fn a_start_under_sky_material_has_a_fall() {
    // Sky at 30, a solid roof above it at 60: the first hit is sky.
    let solid = walls([quad(0.0, 0.0, 50.0), quad(0.0, 60.0, 50.0)].concat());
    let sky = walls(quad(0.0, 30.0, 50.0));
    let fall = plan(&Roof { solid: &solid, sky: &sky }, &[START])[0].expect("open");
    assert_eq!(fall.ground, START);
    assert_eq!(fall.top, [0.0, 0.0, FALL_HEIGHT]);
    // Sky above a solid roof is no way out.
    let solid = walls([quad(0.0, 0.0, 50.0), quad(0.0, 20.0, 50.0)].concat());
    assert_eq!(plan(&Roof { solid: &solid, sky: &walls(quad(0.0, 60.0, 50.0)) }, &[START])[0], None);
}

#[test]
fn a_ring_ray_hit_closes_the_column() {
    let floor = quad(0.0, 0.0, 50.0);
    let sky = walls(Vec::new());
    // A plate over x 0.4 to 1.5 misses the middle ray, not the ring's.
    let plate = |lo: f32, hi: f32| -> Vec<[V3; 3]> {
        vec![[[lo, -1.0, 25.0], [hi, -1.0, 25.0], [hi, 1.0, 25.0]], [[lo, -1.0, 25.0], [hi, 1.0, 25.0], [lo, 1.0, 25.0]]]
    };
    let near = walls([floor.clone(), plate(0.4, 1.5)].concat());
    assert_eq!(plan(&Roof { solid: &near, sky: &sky }, &[START])[0], None);
    let far = walls([floor, plate(1.0, 2.0)].concat());
    assert!(plan(&Roof { solid: &far, sky: &sky }, &[START])[0].is_some());
}

#[test]
fn a_short_column_is_not_a_fall() {
    let sky = walls(Vec::new());
    let low = walls([quad(0.0, 0.0, 50.0), quad(100.0, 11.5, 1.0)].concat());
    assert_eq!(plan(&Roof { solid: &low, sky: &sky }, &[START])[0], None);
    let enough = walls([quad(0.0, 0.0, 50.0), quad(100.0, MIN_FALL, 1.0)].concat());
    assert_eq!(plan(&Roof { solid: &enough, sky: &sky }, &[START])[0].unwrap().top[2], MIN_FALL);
    // No collision at all: none.
    let nothing = walls(Vec::new());
    assert_eq!(plan(&Roof { solid: &nothing, sky: &sky }, &[START]), [None]);
}

#[test]
fn no_pod_tag_leaves_the_kit_without_pools() {
    use crate::{Config, Labels, Mode, build_with_classes, testkit};
    let built = build_with_classes(
        &testkit::court(),
        testkit::NAME,
        Mode::Kit,
        &Config::default(),
        &Labels::default(),
        None,
    )
    .unwrap();
    // The invented court is flat: no column is 12 tall, so nothing falls
    // and nothing is added.
    let report = built.pods.expect("a pod report");
    assert!(!report.kept, "{report}");
    assert_eq!(built.costs.pods, 0);
    let mut map = Map::from_reader(std::io::Cursor::new(&built.bytes)).unwrap();
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert!(ai.point_sets.iter().all(|s| !s.name.starts_with("ff_fall_")));
    let scenario = map.scenario().unwrap().id;
    let root = map.read_tag_bytes(scenario).unwrap();
    let names = metadata_block(&mut map, &root, OBJECT_NAMES as usize, OBJECT_NAME).unwrap();
    assert!(names.iter().all(|n| !n.starts_with(b"ff_pod") && !n.starts_with(b"ff_ppod")));
}
