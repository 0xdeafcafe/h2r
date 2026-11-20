//! Invented BSPs only: a box-shaped piece and a floor-and-wall level.

use super::*;
use crate::surface_query::{Hit, Outcome, locate, vector};
use tags::collision::{Edge, Surface, Vertex};

/// Builds polygons into `b`: each surface its own vertices and edges, the
/// ring in the order given.
fn add_surface(b: &mut Bsp, plane: u16, ring: &[[f32; 3]]) -> u16 {
    let s = b.surfaces.len() as u16;
    let (e0, v0) = (b.edges.len(), b.vertices.len());
    let n = ring.len();
    for (i, p) in ring.iter().enumerate() {
        b.vertices.push(Vertex {
            point: *p,
            edge: (e0 + i) as u32,
        });
        b.edges.push(Edge {
            start_vertex: (v0 + i) as u16,
            end_vertex: (v0 + (i + 1) % n) as u16,
            forward_edge: (e0 + (i + 1) % n) as u16,
            reverse_edge: (e0 + (i + 1) % n) as u16,
            left_surface: s,
            right_surface: u16::MAX,
        });
    }
    b.surfaces.push(Surface {
        plane,
        first_edge: e0 as u16,
        flags: 0,
        breakable_surface: 0xff,
        material: 0,
    });
    s
}

/// A box, solid inside `[lo, hi]`: node `k` tests face `k` (`+x, -x, +y,
/// -y, +z, -z`), its front an open leaf holding one reference to the
/// face. With `split_top` the top is two surfaces either side of the
/// middle in x, behind a 2D node.
fn cube(lo: [f32; 3], hi: [f32; 3], split_top: bool) -> Bsp {
    let mut b = Bsp::default();
    for k in 0..6usize {
        let a = k / 2;
        let positive = k % 2 == 0;
        let mut n = [0.0; 3];
        n[a] = if positive { 1.0 } else { -1.0 };
        let d = if positive { hi[a] } else { -lo[a] };
        b.planes.push(Plane([n[0], n[1], n[2], d]));
        b.nodes.push(Node3d {
            plane: k as u16,
            back: if k == 5 {
                Child::None
            } else {
                Child::Node(k as u32 + 1)
            },
            front: Child::Leaf(k as u32),
        });
        let at = if positive { hi[a] } else { lo[a] };
        let (ba, ca) = ((a + 1) % 3, (a + 2) % 3);
        let corner = |u: f32, v: f32| {
            let mut p = [0.0; 3];
            p[a] = at;
            p[ba] = u;
            p[ca] = v;
            p
        };
        let quad = |u0: f32, u1: f32, v0: f32, v1: f32| {
            [
                corner(u0, v0),
                corner(u1, v0),
                corner(u1, v1),
                corner(u0, v1),
            ]
        };
        let node = if k == 4 && split_top {
            // +z: (u, v) = (x, y), and x is the first axis after z.
            let mid = (lo[0] + hi[0]) / 2.0;
            let left = add_surface(&mut b, 4, &quad(lo[0], mid, lo[1], hi[1]));
            let right = add_surface(&mut b, 4, &quad(mid, hi[0], lo[1], hi[1]));
            b.nodes_2d.push(Node2d {
                plane: [1.0, 0.0, mid],
                back: Child2d::Surface(left),
                front: Child2d::Surface(right),
            });
            Child2d::Node(b.nodes_2d.len() as u16 - 1)
        } else {
            let s = add_surface(&mut b, k as u16, &quad(lo[ba], hi[ba], lo[ca], hi[ca]));
            Child2d::Surface(s)
        };
        b.leaves.push(Leaf {
            flags: 0,
            ref_count: 1,
            first_ref: b.refs.len() as u16,
        });
        b.refs.push(Bsp2dRef {
            plane: k as u16,
            node: node.raw(),
        });
    }
    b
}

/// A level: solid below `z = 0` and beyond the wall `x = 3`; one open
/// leaf holding the floor's and the wall's references (the wall's
/// flipped: the leaf is on its back).
fn level() -> Bsp {
    let mut b = Bsp {
        nodes: vec![
            Node3d {
                plane: 0,
                back: Child::None,
                front: Child::Node(1),
            },
            Node3d {
                plane: 1,
                back: Child::Leaf(0),
                front: Child::None,
            },
        ],
        planes: vec![Plane([0.0, 0.0, 1.0, 0.0]), Plane([1.0, 0.0, 0.0, 3.0])],
        leaves: vec![Leaf {
            flags: 0,
            ref_count: 2,
            first_ref: 0,
        }],
        ..Bsp::default()
    };
    let floor = add_surface(
        &mut b,
        0,
        &[
            [-50.0, -50.0, 0.0],
            [3.0, -50.0, 0.0],
            [3.0, 50.0, 0.0],
            [-50.0, 50.0, 0.0],
        ],
    );
    let wall = add_surface(
        &mut b,
        0x8001,
        &[
            [3.0, -50.0, 0.0],
            [3.0, -50.0, 20.0],
            [3.0, 50.0, 20.0],
            [3.0, 50.0, 0.0],
        ],
    );
    b.refs = vec![
        Bsp2dRef {
            plane: 0,
            node: Child2d::Surface(floor).raw(),
        },
        Bsp2dRef {
            plane: 0x8001,
            node: Child2d::Surface(wall).raw(),
        },
    ];
    b
}

fn hit(b: &Bsp, from: [f32; 3], to: [f32; 3]) -> Hit {
    match vector(b, from, to).outcome {
        Outcome::Hit(h) => h,
        o => panic!("{from:?} to {to:?}: {o:?}"),
    }
}

fn near(a: [f64; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - f64::from(b[k])).abs() < 1e-5)
}

#[test]
fn a_placement_turns_yaw_then_pitch_then_roll() {
    use std::f32::consts::FRAC_PI_2;
    let x = [1.0, 0.0, 0.0];
    let y = [0.0, 1.0, 0.0];
    assert!(near(
        Pose::placement([0.0; 3], [FRAC_PI_2, 0.0, 0.0]).point(x),
        y
    ));
    // Pitch tips forward up.
    assert!(near(
        Pose::placement([0.0; 3], [0.0, FRAC_PI_2, 0.0]).point(x),
        [0.0, 0.0, 1.0]
    ));
    // Roll turns left up.
    assert!(near(
        Pose::placement([0.0; 3], [0.0, 0.0, FRAC_PI_2]).point(y),
        [0.0, 0.0, 1.0]
    ));
    // Yaw last: a pitched forward axis turns with it.
    let p = Pose::placement([1.0, 2.0, 3.0], [FRAC_PI_2, FRAC_PI_2, 0.0]);
    assert!(near(p.point(x), [1.0, 2.0, 4.0]));
    assert!(near(p.point(y), [0.0, 2.0, 3.0]));
    // `then` is composition.
    let a = Pose::placement([5.0, 0.0, 0.0], [0.3, 0.2, 0.1]);
    let b = Pose::placement([0.0, 1.0, 0.0], [0.7, -0.4, 1.2]);
    let q = [0.25, -1.5, 2.0];
    let c = a.then(&b).point(q);
    let ab = a.point(b.point(q).map(|v| v as f32));
    assert!((0..3).all(|k| (c[k] - ab[k]).abs() < 1e-4), "{c:?} {ab:?}");
}

#[test]
fn the_test_cube_is_a_valid_bsp_whose_faces_are_hit() {
    let c = cube([-1.0; 3], [1.0; 3], true);
    assert_eq!(c.validate(1), Vec::<String>::new());
    let h = hit(&c, [0.5, 0.2, 3.0], [0.5, 0.2, -3.0]);
    assert_eq!((h.t, h.surface), (1.0 / 3.0, 5));
    let h = hit(&c, [-0.5, 0.2, 3.0], [-0.5, 0.2, -3.0]);
    assert_eq!(h.surface, 4);
    assert_eq!(locate(&c, [0.0; 3]), None);
}

/// Every surface of `b` is hit where it is from just in front of it,
/// coming in along its plane's normal.
fn every_surface_is_hit_in_place(b: &Bsp) {
    for s in 0..b.surfaces.len() {
        let ring = b.surface_ring(s);
        let c: [f32; 3] = std::array::from_fn(|k| {
            ring.iter().map(|&v| b.vertices[v].point[k]).sum::<f32>() / ring.len() as f32
        });
        let sf = b.surfaces[s];
        let p = b.planes[usize::from(sf.plane & 0x7fff)].0;
        let sign = if sf.plane & 0x8000 != 0 { -1.0 } else { 1.0 };
        let n = [p[0] * sign, p[1] * sign, p[2] * sign];
        let from = std::array::from_fn(|k| c[k] + n[k]);
        let to = std::array::from_fn(|k| c[k] - 0.5 * n[k]);
        let h = hit(b, from, to);
        assert_eq!(h.surface, s, "surface {s} at {c:?}");
        assert!((h.t - 2.0 / 3.0).abs() < 1e-4, "surface {s}: t {}", h.t);
    }
}

#[test]
fn a_moved_bsp_finds_every_surface_where_it_went() {
    let c = cube([-1.0, -0.5, -0.25], [1.0, 0.5, 0.75], true);
    every_surface_is_hit_in_place(&c);
    for rotation in [
        [0.7, 0.4, 1.1],
        [std::f32::consts::FRAC_PI_2, 0.0, 0.0],
        [2.5, -1.2, 0.3],
    ] {
        let pose = Pose::placement([10.0, -5.0, 3.0], rotation);
        let m = transformed(&c, &pose).unwrap();
        assert_eq!(m.validate(1), Vec::<String>::new(), "{rotation:?}");
        assert_eq!(m.nodes_2d.len(), 1, "one reference, one 2D node");
        every_surface_is_hit_in_place(&m);
        // Solid where the box went.
        let inside = pose.point([0.5, 0.2, 0.1]).map(|v| v as f32);
        assert_eq!(locate(&m, inside), None, "{rotation:?}");
        let outside = pose.point([0.5, 0.2, 1.0]).map(|v| v as f32);
        assert!(locate(&m, outside).is_some(), "{rotation:?}");
    }
}

/// A rock: a box turned 45 degrees about z, its centre at
/// `(2.5, 0, 0.5)`, reaching past the wall at `x = 3` and below the floor.
fn rock() -> (Bsp, Pose) {
    let pose = Pose::placement([2.5, 0.0, 0.5], [std::f32::consts::FRAC_PI_4, 0.0, 0.0]);
    (
        transformed(&cube([-1.0; 3], [1.0; 3], true), &pose).unwrap(),
        pose,
    )
}

#[test]
fn a_merged_rock_is_solid_and_hit_and_the_level_is_unchanged_around_it() {
    let before = level();
    let mut b = before.clone();
    let (rock, pose) = rock();
    let n = b.surfaces.len();
    let merged = merge(&mut b, &rock, 0).unwrap();
    assert_eq!(b.validate(1), Vec::<String>::new());
    // The box reaches the level's one open leaf.
    assert_eq!(merged.slots, 1);
    assert_eq!(merged.stand_ins.len(), b.leaves.len() - before.leaves.len());
    assert!(merged.stand_ins.iter().all(|&l| l == 0));
    // The level's own blocks come first, unchanged.
    assert_eq!(b.surfaces[..n], before.surfaces[..]);
    assert_eq!(b.planes[..2], before.planes[..]);
    assert!(b.surfaces[n..].iter().all(|s| s.material == 0));

    // Inside the rock: solid. Beside it, above the floor: open.
    assert!(locate(&before, [2.5, 0.0, 1.0]).is_some());
    assert_eq!(locate(&b, [2.5, 0.0, 1.0]), None);
    assert!(locate(&b, [1.3, 1.0, 1.0]).is_some());
    assert!(locate(&b, [-5.0, 0.0, 1.0]).is_some());

    // From above, the rock's top, not the floor under it.
    let top = pose.point([0.3, 0.2, 1.0]).map(|v| v as f32);
    let h = hit(&b, [top[0], top[1], 5.0], [top[0], top[1], -5.0]);
    assert!(h.surface >= n, "{h:?}");
    assert!((h.t - 0.35).abs() < 1e-4, "{h:?}");
    assert_eq!(
        hit(&before, [top[0], top[1], 5.0], [top[0], top[1], -5.0]).surface,
        0
    );
    // Inside the rock's box but off the rock: the floor (the level leaf's
    // reference carried into the piece's open leaf).
    let h = hit(&b, [1.3, 1.0, 5.0], [1.3, 1.0, -5.0]);
    assert_eq!((h.surface, h.t), (0, 0.5));
    // ... and the wall, from between the rock and the wall.
    let h = hit(&b, [2.7, 1.3, 1.0], [5.0, 1.3, 1.0]);
    assert_eq!(h.surface, 1, "{h:?}");
    assert!((h.t - 0.3 / 2.3).abs() < 1e-5);
    // A side of the rock, from the open side.
    let h = hit(&b, [-2.0, 0.3, 1.0], [5.0, 0.3, 1.0]);
    assert!(h.surface >= n, "{h:?}");
    // Away from the rock, every hit as before.
    for (from, to) in [
        ([-5.0, 3.0, 5.0], [-5.0, 3.0, -5.0]),
        ([0.0, -10.0, 1.0], [10.0, -10.0, 1.0]),
        ([-20.0, 20.0, 2.0], [1.0, -1.0, -1.0]),
    ] {
        assert_eq!(vector(&b, from, to), vector(&before, from, to), "{from:?}");
    }
}

#[test]
fn the_structure_mopp_returns_the_rock_where_it_stands() {
    let mut b = level();
    let n = b.surfaces.len();
    let (rock, pose) = rock();
    merge(&mut b, &rock, 0).unwrap();
    let code = mopp::Code::parse(&mopp::build_structure(&b)).unwrap();
    let top = pose.point([0.3, 0.2, 1.0]).map(|v| v as f32);
    let keys = mopp::query_aabb(
        &code,
        &mopp::Aabb {
            min: [top[0] - 0.1, top[1] - 0.1, top[2] - 0.1],
            max: [top[0] + 0.1, top[1] + 0.1, top[2] + 0.1],
        },
    )
    .unwrap();
    let rock_keys: Vec<_> = (n..b.surfaces.len())
        .map(mopp::geom::structure_key)
        .filter(|k| keys.contains(k))
        .collect();
    assert!(!rock_keys.is_empty(), "{keys:x?}");
}

#[test]
fn merged_surfaces_keep_their_source_material_assignment() {
    let mut level = level();
    let count = level.surfaces.len();
    let (mut rock, _) = rock();
    for (i, surface) in rock.surfaces.iter_mut().enumerate() {
        surface.material = (i % 2 + 2) as u16;
    }
    super::merge_materials(&mut level, &rock, |i| i).unwrap();
    let expected: Vec<_> = rock.surfaces.iter().map(|s| s.material).collect();
    let actual: Vec<_> = level.surfaces[count..].iter().map(|s| s.material).collect();
    assert_eq!(actual, expected);
}

#[test]
fn two_overlapping_rocks_are_both_solid() {
    let mut b = level();
    let (first, _) = rock();
    let second = transformed(
        &cube([-1.0; 3], [1.0; 3], false),
        &Pose::placement([1.0, 0.5, 0.8], [0.3, 0.2, -0.4]),
    )
    .unwrap();
    merge(&mut b, &first, 0).unwrap();
    let leaves = b.leaves.len();
    let again = merge(&mut b, &second, 0).unwrap();
    assert!(again.slots > 1, "the second rock meets the first's leaves");
    assert!(again.stand_ins.iter().all(|&l| l < leaves));
    assert_eq!(b.validate(1), Vec::<String>::new());
    assert_eq!(locate(&b, [2.5, 0.0, 1.0]), None);
    assert_eq!(locate(&b, [1.0, 0.5, 0.8]), None);
    assert!(locate(&b, [-3.0, 0.0, 1.0]).is_some());
    // The floor between and beyond them still found.
    let h = hit(&b, [-3.0, -3.0, 5.0], [-3.0, -3.0, -5.0]);
    assert_eq!(h.surface, 0);
}

#[test]
fn a_rock_in_solid_ground_adds_nothing() {
    let mut b = level();
    let before = b.clone();
    let deep = transformed(
        &cube([-1.0; 3], [1.0; 3], false),
        &Pose::placement([0.0, 0.0, -10.0], [0.0; 3]),
    )
    .unwrap();
    assert_eq!(merge(&mut b, &deep, 0).unwrap(), Merged::default());
    assert_eq!(b, before);
}

#[test]
fn a_merge_that_does_not_fit_changes_nothing() {
    let mut b = level();
    b.planes
        .resize(MAX_FLAGGED - 4, Plane([0.0, 0.0, 1.0, -100.0]));
    let before = b.clone();
    let (rock, _) = rock();
    assert!(matches!(
        merge(&mut b, &rock, 0),
        Err(Error::TooBig { what: "plane", .. })
    ));
    assert_eq!(b, before);
}
