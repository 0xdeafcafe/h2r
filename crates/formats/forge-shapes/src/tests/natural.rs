//! The natural set, hull and noise: invented sizes and seeds, no files but
//! the golden hashes of `shapes.rs`.

use super::support::*;
use crate::hull::{IPoint, convex_hull, orientation};
use crate::noise;
use crate::{Face, PieceDef, Role, Shape, Solid, units};
use std::collections::HashSet;

const NATURAL: [Shape; 5] = [
    Shape::Boulder,
    Shape::Slab,
    Shape::Cliff,
    Shape::TerrainWedge,
    Shape::Spire,
];

const SIZES: [[u16; 3]; 5] = [
    [96, 96, 64],
    [256, 192, 32],
    [256, 64, 256],
    [256, 256, 96],
    [64, 64, 256],
];

/// Every shape at its starting size over a run of seeds.
fn rocks(seeds: impl Iterator<Item = u16> + Clone) -> Vec<(PieceDef, Solid)> {
    let mut out = Vec::new();
    for (shape, size) in NATURAL.into_iter().zip(SIZES) {
        for seed in seeds.clone() {
            let def = PieceDef::dev(shape, size, seed);
            let rock = solid(&def);
            out.push((def, rock));
        }
    }
    out
}

fn normal64(face: &Face) -> [f64; 3] {
    wide(face.normal())
}

fn area(face: &Face) -> f64 {
    let n = normal64(face);
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() * 0.5
}

#[test]
fn every_vertex_is_on_or_behind_every_faces_plane() {
    for (def, rock) in rocks(0..8) {
        for face in &rock.shell {
            let n = normal64(face);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let o = wide(face.ring[0]);
            for p in rock.shell.iter().flat_map(|f| &f.ring) {
                let d = wide(*p);
                let off =
                    ((d[0] - o[0]) * n[0] + (d[1] - o[1]) * n[1] + (d[2] - o[2]) * n[2]) / len;
                assert!(off < 1e-4, "{}: a vertex is {off} in front", def.name());
            }
        }
    }
}

#[test]
fn a_rock_is_one_closed_convex_hull() {
    for (def, rock) in rocks(0..8) {
        assert_eq!(rock.hulls.len(), 1, "{}", def.name());
        let edges = edge_counts(&rock.shell);
        for (&(a, b), &n) in &edges {
            assert_eq!(n, 1, "{}: an edge is used {n} times", def.name());
            assert_eq!(edges.get(&(b, a)), Some(&1), "{}: an open edge", def.name());
        }
        assert!(rock.volume() > 0.0, "{} winds inward", def.name());
        let hull = rock.hulls[0].volume();
        assert!((hull - rock.volume()).abs() <= rock.volume() * 1e-5);
    }
}

#[test]
fn seeded_points_are_inside_the_shell_exactly_when_inside_the_hull() {
    for (def, rock) in rocks(0..2) {
        let (lo, hi) = rock.bounds();
        let mut rng = Rng::new(0x70c5_0001 ^ u64::from(def.param) ^ u64::from(def.size[2]));
        let (mut inside, mut outside) = (0, 0);
        for _ in 0..2_000 {
            let mut p = [0.0f64; 3];
            for k in 0..3 {
                let (l, h) = (f64::from(lo[k]), f64::from(hi[k]));
                let pad = (h - l) * 0.05;
                p[k] = l - pad + rng.next_f64() * (h - l + 2.0 * pad);
            }
            let in_shell = inside_shell(&rock.shell, p);
            // Away from a face, where float noise could differ between the tests.
            let near = rock.shell.iter().any(|f| {
                let n = normal64(f);
                let o = wide(f.ring[0]);
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                (((p[0] - o[0]) * n[0] + (p[1] - o[1]) * n[1] + (p[2] - o[2]) * n[2]) / len).abs()
                    < 1e-6
            });
            if near {
                continue;
            }
            assert_eq!(
                in_shell,
                rock.hulls[0].contains(p),
                "{} at {p:?}",
                def.name()
            );
            if in_shell {
                inside += 1;
            } else {
                outside += 1;
            }
        }
        assert!(
            inside > 100 && outside > 100,
            "{}: a one-sided sample",
            def.name()
        );
    }
}

#[test]
fn a_hull_fills_its_bounds_on_each_axis() {
    for (def, rock) in rocks(0..8) {
        let (lo, hi) = rock.bounds();
        for k in 0..3 {
            let want = units::world(def.size[k]);
            assert_eq!(lo[k], 0.0, "{} axis {k}", def.name());
            assert!(
                hi[k] >= want * 0.9 && hi[k] <= want * 1.0001,
                "{} axis {k}: {} of {want}",
                def.name(),
                hi[k]
            );
        }
    }
}

#[test]
fn the_lowest_face_lies_on_the_ground() {
    for (def, rock) in rocks(0..8) {
        let (lo, _) = rock.bounds();
        assert_eq!(lo[2], 0.0, "{}", def.name());
        let base = rock
            .shell
            .iter()
            .find(|f| f.ring.iter().all(|p| p[2] == 0.0))
            .unwrap_or_else(|| panic!("{}: no face on z = 0", def.name()));
        assert_eq!(base.role, Role::Ceiling, "{}", def.name());
        assert!(base.ring.len() >= 3);
    }
}

#[test]
fn a_hull_has_at_most_sixty_four_faces_and_a_plausible_outline() {
    for (def, rock) in rocks(0..64) {
        assert!(
            (6..=64).contains(&rock.shell.len()),
            "{}: {} faces",
            def.name(),
            rock.shell.len()
        );
    }
    // A rock is neither a box nor a ball: it fills part of its bounds, with
    // many facets.
    for (def, rock) in rocks(0..8) {
        let (_, hi) = rock.bounds();
        let fill = rock.volume() / (f64::from(hi[0]) * f64::from(hi[1]) * f64::from(hi[2]));
        match def.shape {
            Shape::Boulder => {
                assert!((0.2..0.7).contains(&fill), "{}: fills {fill}", def.name());
                assert!(rock.shell.len() >= 12, "{}", def.name());
            }
            Shape::Spire => {
                assert!((0.1..0.5).contains(&fill), "{}: fills {fill}", def.name());
                assert!(rock.shell.len() >= 12, "{}", def.name());
            }
            Shape::Slab => assert!(rock.shell.len() >= 12, "{}", def.name()),
            _ => assert!(fill < 0.97, "{}: a box", def.name()),
        }
    }
}

#[test]
fn a_seed_gives_the_same_bits_and_eight_seeds_eight_hulls() {
    for (shape, size) in NATURAL.into_iter().zip(SIZES) {
        let mut seen = HashSet::new();
        for seed in 0..8 {
            let def = PieceDef::dev(shape, size, seed);
            assert_eq!(solid(&def).bits(), solid(&def).bits());
            seen.insert(solid(&def).bits());
        }
        assert_eq!(seen.len(), 8, "{shape}: two seeds gave one hull");
    }
}

#[test]
fn every_natural_shape_generates_at_its_extremes() {
    for shape in NATURAL {
        for size in [[4, 4, 4], [2048, 2048, 2048], [4, 2048, 33]] {
            for seed in [0, 1, 65535] {
                let def = PieceDef::dev(shape, size, seed);
                let rock = def.generate().expect("a natural piece generates");
                assert!(rock.volume() > 0.0, "{}", def.name());
            }
        }
    }
}

#[test]
fn a_slabs_top_and_a_wedges_slope_are_floor() {
    for seed in 0..16 {
        let slab = solid(&PieceDef::dev(Shape::Slab, [256, 192, 32], seed));
        let (_, hi) = slab.bounds();
        let top: Vec<&Face> = slab
            .shell
            .iter()
            .filter(|f| f.ring.iter().any(|p| p[2] == hi[2]))
            .collect();
        assert!(!top.is_empty());
        assert!(top.iter().all(|f| f.role == Role::Floor), "slab {seed}");

        let wedge = solid(&PieceDef::dev(Shape::TerrainWedge, [256, 256, 96], seed));
        let slope = wedge
            .shell
            .iter()
            .filter(|f| f.normal()[2] > 0.0)
            .max_by(|a, b| area(a).total_cmp(&area(b)))
            .expect("a wedge has an upward face");
        assert!(slope.normal()[0] < 0.0, "wedge {seed}: it rises along x");
        assert_eq!(slope.role, Role::Floor, "wedge {seed}");
    }
}

#[test]
fn a_cliffs_back_and_ends_are_flat() {
    for seed in 0..16 {
        let cliff = solid(&PieceDef::dev(Shape::Cliff, [256, 64, 256], seed));
        let (_, hi) = cliff.bounds();
        for (axis, at) in [(1, hi[1]), (0, 0.0), (0, hi[0])] {
            let flat = cliff.shell.iter().any(|f| {
                f.ring.len() >= 3 && f.ring.iter().all(|p| p[axis] == at) && f.role == Role::Wall
            });
            assert!(flat, "cliff {seed}: no flat face on axis {axis} at {at}");
        }
    }
}

#[test]
fn the_hull_of_a_cube_and_its_inside_points_is_the_cube() {
    let mut points: Vec<IPoint> = Vec::new();
    for x in [0, 8] {
        for y in [0, 8] {
            for z in [0, 8] {
                points.push([x, y, z]);
            }
        }
    }
    // Inside, on a face, on an edge, and a repeat.
    points.extend([[4, 4, 4], [2, 3, 5], [4, 4, 0], [0, 8, 4], [0, 0, 0]]);
    let cube = convex_hull(&points).expect("a cube");
    assert_eq!(cube.points.len(), 8);
    assert_eq!(cube.faces.len(), 6);
    assert!(cube.faces.iter().all(|f| f.len() == 4));
    // Each face is wound outward: the cube's middle is behind it.
    for face in &cube.faces {
        let p = |i: usize| cube.points[face[i] as usize];
        assert!(orientation(p(0), p(1), p(2), [4, 4, 4]) < 0);
    }
}

#[test]
fn a_flat_or_short_set_has_no_hull() {
    assert!(convex_hull(&[[0, 0, 0], [1, 0, 0], [0, 1, 0]]).is_none());
    assert!(convex_hull(&[[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [3, 3, 0]]).is_none());
    assert!(convex_hull(&[[0, 0, 0], [1, 1, 1], [2, 2, 2], [3, 3, 3]]).is_none());
    let tetra = convex_hull(&[[0, 0, 0], [4, 0, 0], [0, 4, 0], [0, 0, 4]]).expect("a tetrahedron");
    assert_eq!((tetra.points.len(), tetra.faces.len()), (4, 4));
}

#[test]
fn orientation_is_the_sign_of_the_volume() {
    let (a, b, c) = ([0, 0, 0], [1, 0, 0], [0, 1, 0]);
    assert_eq!(orientation(a, b, c, [0, 0, 5]), 1);
    assert_eq!(orientation(a, b, c, [3, 3, -1]), -1);
    assert_eq!(orientation(a, b, c, [7, -2, 0]), 0);
}

#[test]
fn noise_is_repeatable_bounded_and_seeded() {
    let mut differ = 0;
    for i in -20..20 {
        let p = [i as f32 * 0.37, i as f32 * -0.21 + 0.5, i as f32 * 0.11];
        let (a, b) = (noise::value(7, p), noise::value(7, p));
        assert_eq!(a.to_bits(), b.to_bits());
        assert!((-1.0..1.0).contains(&a), "{a}");
        let f = noise::fbm(7, p, 4);
        assert!((-1.0..1.0).contains(&f), "{f}");
        if noise::value(8, p) != a {
            differ += 1;
        }
    }
    assert!(differ > 30, "another seed gave the same noise");
}

#[test]
fn noise_at_a_lattice_point_is_its_hash_and_is_continuous() {
    for (x, y, z) in [(0, 0, 0), (3, -2, 5), (-7, 1, 1)] {
        let at = noise::value(9, [x as f32, y as f32, z as f32]);
        assert_eq!(at, noise::signed(noise::hash(9, x, y, z)));
    }
    let p = [1.3f32, 2.7, -0.4];
    let q = [1.3001, 2.7, -0.4];
    assert!((noise::value(3, p) - noise::value(3, q)).abs() < 0.01);
}

#[test]
fn a_stream_is_repeatable_and_inside_its_range() {
    let mut a = noise::Stream::new(42);
    let mut b = noise::Stream::new(42);
    for _ in 0..100 {
        let v = a.range(2.0, 5.0);
        assert_eq!(v.to_bits(), b.range(2.0, 5.0).to_bits());
        assert!((2.0..5.0).contains(&v));
    }
}
