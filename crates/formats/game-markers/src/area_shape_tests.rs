//! The box and the sphere (docs/game/gametype-markers.md, "Areas"), on
//! invented numbers.
use std::io::Cursor;

use super::*;
use crate::record::{self, Row};

const HERE: [f32; 3] = [10.0, -4.0, 2.0];
const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

fn at(dx: f32, dy: f32, dz: f32) -> [f32; 3] {
    [HERE[0] + dx, HERE[1] + dy, HERE[2] + dz]
}

#[test]
fn a_turned_box_holds_its_corners_and_not_a_point_past_them() {
    let area = Area::boxed([3.0, 1.0]);
    // Unturned: the corners and the edges are in, a hair past is out.
    for (dx, dy) in [(3.0, 1.0), (-3.0, 1.0), (3.0, -1.0), (-3.0, -1.0), (3.0, 0.0)] {
        assert!(area.contains(HERE, at(dx, dy, 0.0)), "{dx} {dy}");
    }
    assert!(!area.contains(HERE, at(3.01, 0.0, 0.0)));
    assert!(!area.contains(HERE, at(0.0, 1.01, 0.0)));
    assert!(!area.contains(HERE, at(3.01, 1.01, 0.0)));
    // Turned a quarter, the long side lies along y: the corners turn with
    // it, and a point that was inside is now past the short side.
    for (dx, dy) in [(-1.0, 3.0), (1.0, 3.0), (-1.0, -3.0), (1.0, -3.0)] {
        assert!(area.contains_turned(HERE, QUARTER, at(dx, dy, 0.0)), "{dx} {dy}");
    }
    assert!(!area.contains_turned(HERE, QUARTER, at(2.5, 0.0, 0.0)));
    assert!(!area.contains_turned(HERE, QUARTER, at(1.01, 3.01, 0.0)));
    assert!(!area.contains_turned(HERE, QUARTER, at(0.0, 3.01, 0.0)));
    // The heights still bound it, edges included.
    assert!(area.contains(HERE, at(0.0, 0.0, -DEFAULT_BELOW)));
    assert!(area.contains(HERE, at(0.0, 0.0, DEFAULT_ABOVE)));
    assert!(!area.contains(HERE, at(0.0, 0.0, DEFAULT_ABOVE + 0.01)));
    assert!(!area.contains(HERE, at(0.0, 0.0, -DEFAULT_BELOW - 0.01)));
}

#[test]
fn a_sphere_holds_a_point_over_the_marker_that_a_flat_cylinder_does_not() {
    let ball = Area::sphere(2.0);
    let flat = Area {
        shape: Shape::Radius(2.0),
        below: 0.0,
        above: 0.0,
    };
    let over = at(0.0, 0.0, 1.5);
    assert!(ball.contains(HERE, over));
    assert!(!flat.contains(HERE, over));
    // By distance, not by height and floor: a corner of the ball's box is out.
    assert!(ball.contains(HERE, at(0.0, 0.0, -2.0)));
    assert!(ball.contains(HERE, at(2.0, 0.0, 0.0)));
    assert!(!ball.contains(HERE, at(1.5, 1.5, 0.0)));
    assert!(!ball.contains(HERE, at(1.2, 1.2, 1.2)));
    assert!(!ball.contains(HERE, at(0.0, 0.0, 2.01)));
    // It has no height of its own.
    assert_eq!((ball.below, ball.above), (0.0, 0.0));
    assert_eq!(ball.validate(), Ok(()));
    assert_eq!(ball.middle(HERE), HERE);
}

#[test]
fn the_two_shapes_are_refused_by_name() {
    assert_eq!(Area::boxed([0.1, 1.0]).validate(), Err(AreaError::Half([0.1, 1.0])));
    assert_eq!(Area::boxed([1.0, 65.0]).validate(), Err(AreaError::Half([1.0, 65.0])));
    assert_eq!(Area::boxed([f32::NAN, 1.0]).validate(), Err(AreaError::NotFinite));
    assert_eq!(Area::boxed([0.25, 64.0]).validate(), Ok(()));
    assert_eq!(Area::sphere(0.1).validate(), Err(AreaError::Radius(0.1)));
    assert_eq!(Area::sphere(100.0).validate(), Err(AreaError::Radius(100.0)));
    assert_eq!(Area::sphere(f32::INFINITY).validate(), Err(AreaError::NotFinite));
    assert_eq!(Area::sphere(0.25).validate(), Ok(()));
    // A box keeps a cylinder's heights.
    let mut tall = Area::boxed([1.0, 1.0]);
    tall.above = 40.0;
    assert_eq!(
        tall.validate(),
        Err(AreaError::Height {
            below: DEFAULT_BELOW,
            above: 40.0
        })
    );
    assert!(AreaError::Half([0.1, 1.0]).to_string().contains("half lengths"));
}

#[test]
fn the_outline_of_each_shape_is_counter_clockwise_in_the_markers_axes() {
    assert_eq!(
        Area::boxed([3.0, 1.0]).outline(),
        vec![[-3.0, -1.0], [3.0, -1.0], [3.0, 1.0], [-3.0, 1.0]]
    );
    let ring = Area::sphere(2.0).outline();
    assert_eq!(ring.len(), SEGMENTS);
    assert!(ring.iter().all(|p| (p[0].hypot(p[1]) - 2.0).abs() < 1e-4));
    assert_eq!(Area::boxed([3.0, 1.0]).middle(HERE), [10.0, -4.0, 2.0 + 0.5]);
    let floor = Area::boxed([2.0, 2.0]).equal_floor_radius().unwrap();
    assert!((floor - (16.0 / std::f32::consts::PI).sqrt()).abs() < 1e-5);
    assert_eq!(Area::sphere(2.0).equal_floor_radius(), Some(2.0));
}

fn row(index: u32, area: Area) -> Row {
    Row {
        index,
        flag_type: 10,
        identifier: 1,
        at: [1.0, 2.0, 3.0],
        area: Some(area),
        show: false,
    }
}

#[test]
fn both_shapes_round_trip_the_record_bit_for_bit() {
    let rows = vec![
        row(0, Area::boxed([3.25, 0.75])),
        row(
            1,
            Area {
                shape: Shape::Box { half: [1.0, 2.0] },
                below: 0.25,
                above: 4.0,
            },
        ),
        row(2, Area::sphere(5.5)),
        row(3, Area::radius(2.0)),
    ];
    let back = record::read_tail(&mut Cursor::new(record::encode(&rows)))
        .unwrap()
        .unwrap();
    assert_eq!(back, rows);
}

#[test]
fn a_record_writes_a_spheres_heights_as_zero_and_refuses_a_bad_box() {
    let mut loud = Area::sphere(5.5);
    loud.below = 3.0;
    loud.above = 9.0;
    let back = record::read_tail(&mut Cursor::new(record::encode(&[row(0, loud)])))
        .unwrap()
        .unwrap();
    let sphere = back[0].area.clone().unwrap();
    assert_eq!(sphere, Area::sphere(5.5));
    assert_eq!((sphere.below.to_bits(), sphere.above.to_bits()), (0, 0));
    let bytes = record::encode(&[row(0, Area::boxed([100.0, 1.0]))]);
    assert!(matches!(
        record::decode(&bytes[..bytes.len() - record::TRAILER]),
        Err(record::RecordError::Area(_))
    ));
}

#[test]
fn a_box_turned_onto_the_worlds_axes_holds_what_the_turned_box_holds() {
    let boxed = Area::boxed([3.0, 1.0]);
    let yaw = 0.6;
    let flat = boxed.turned(yaw);
    assert!(matches!(&flat.shape, Shape::Path(points) if points.len() == 4));
    assert_eq!((flat.below, flat.above), (boxed.below, boxed.above));
    let (s, c) = yaw.sin_cos();
    for (x, y) in [(0.0, 0.0), (2.9, 0.9), (-2.9, -0.9), (3.1, 0.0), (0.0, 1.1), (2.5, -1.2)] {
        let p = at(x * c - y * s, x * s + y * c, 0.0);
        assert_eq!(flat.contains(HERE, p), boxed.contains_turned(HERE, yaw, p), "{x} {y}");
    }
    // Any other shape is itself.
    assert_eq!(Area::sphere(2.0).turned(yaw), Area::sphere(2.0));
    assert_eq!(Area::default().turned(yaw), Area::default());
}
