use super::support::*;
use crate::{PieceDef, Role, Shape, units};
use sha2::{Digest, Sha256};

#[test]
fn every_shell_is_closed_and_wound_outward() {
    for def in sample_defs() {
        let solid = solid(&def);
        let edges = edge_counts(&solid.shell);
        for (&(a, b), &n) in &edges {
            assert_eq!(n, 1, "{}: an edge is used {n} times one way", def.name());
            assert_eq!(
                edges.get(&(b, a)),
                Some(&1),
                "{}: an edge has no opposite",
                def.name()
            );
        }
        // Wound outward: the divergence volume is positive, and equals the
        // hulls' where they do not overlap.
        assert!(solid.volume() > 0.0, "{} winds inward", def.name());
    }
}

#[test]
fn every_face_is_planar_and_convex() {
    for def in sample_defs() {
        for face in solid(&def).shell {
            let n = crate::newell(&face.ring);
            let n = wide(n);
            let len = dot_self(n).sqrt();
            let o = wide(face.ring[0]);
            for p in &face.ring {
                let d = wide(*p);
                let off =
                    ((d[0] - o[0]) * n[0] + (d[1] - o[1]) * n[1] + (d[2] - o[2]) * n[2]) / len;
                assert!(off.abs() < 1e-4, "{}: a face is not flat", def.name());
            }
            for i in 0..face.ring.len() {
                let (a, b, c) = (
                    wide(face.ring[i]),
                    wide(face.ring[(i + 1) % face.ring.len()]),
                    wide(face.ring[(i + 2) % face.ring.len()]),
                );
                let turn = cross3(sub3(b, a), sub3(c, b));
                let along = turn[0] * n[0] + turn[1] * n[1] + turn[2] * n[2];
                assert!(along > -1e-9, "{}: a face is not convex", def.name());
            }
        }
    }
}

fn dot_self(a: [f64; 3]) -> f64 {
    a[0] * a[0] + a[1] * a[1] + a[2] * a[2]
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[test]
fn seeded_points_are_inside_the_shell_exactly_when_inside_a_hull() {
    for def in sample_defs() {
        let solid = solid(&def);
        let (lo, hi) = solid.bounds();
        let mut rng = Rng::new(0x5eed_0001 ^ u64::from(def.size[0]));
        let (mut inside, mut outside) = (0, 0);
        for _ in 0..10_000 {
            let mut p = [0.0f64; 3];
            for k in 0..3 {
                let (l, h) = (f64::from(lo[k]), f64::from(hi[k]));
                let pad = (h - l) * 0.05;
                p[k] = l - pad + rng.next_f64() * (h - l + 2.0 * pad);
            }
            let in_shell = inside_shell(&solid.shell, p);
            let in_hull = solid.hulls.iter().any(|h| h.contains(p));
            assert_eq!(in_shell, in_hull, "{} at {p:?}", def.name());
            if in_shell {
                inside += 1;
            } else {
                outside += 1;
            }
        }
        assert!(
            inside > 500 && outside > 500,
            "{}: a one-sided sample",
            def.name()
        );
    }
}

fn cubic(units: [u16; 3]) -> f64 {
    units.iter().map(|&u| f64::from(u) / 100.0).product()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= b.abs() * 1e-5
}

#[test]
fn block_ramp_and_wedge_volumes_equal_their_formulas() {
    for size in [[96, 160, 224], [4, 2048, 33], [517, 64, 1000]] {
        let block = solid(&PieceDef::dev(Shape::Block, size, 0));
        assert!(close(block.volume(), cubic(size)), "block {size:?}");
        for shape in [Shape::Ramp, Shape::Wedge] {
            let v = solid(&PieceDef::dev(shape, size, 0)).volume();
            assert!(close(v, cubic(size) / 2.0), "{shape} {size:?}: {v}");
        }
    }
}

#[test]
fn hulls_fill_the_shell_without_overlap() {
    for def in sample_defs() {
        let solid = solid(&def);
        let hulls: f64 = solid.hulls.iter().map(|h| h.volume()).sum();
        assert!(
            close(hulls, solid.volume()),
            "{}: {hulls} vs {}",
            def.name(),
            solid.volume()
        );
    }
}

#[test]
fn hull_counts_follow_the_table() {
    for (def, count) in [
        (PieceDef::dev(Shape::Block, [64; 3], 0), 1),
        (PieceDef::dev(Shape::Ramp, [64; 3], 0), 1),
        (PieceDef::dev(Shape::Wedge, [64; 3], 0), 1),
        (PieceDef::dev(Shape::Stairs, [64; 3], 9), 9),
        (PieceDef::dev(Shape::Cylinder, [64; 3], 12), 1),
        (PieceDef::dev(Shape::Arch, [128, 32, 128], 6), 8),
    ] {
        assert_eq!(solid(&def).hulls.len(), count, "{}", def.name());
    }
}

#[test]
fn bounds_are_the_size() {
    for def in sample_defs() {
        // An odd count of voussoirs leaves the top of an arch below the bounds.
        let odd_arch = def.shape == Shape::Arch && def.param % 2 == 1;
        let (lo, hi) = solid(&def).bounds();
        assert_eq!(lo, [0.0; 3], "{}", def.name());
        for k in 0..3 {
            let want = units::world(def.size[k]);
            if odd_arch && k == 2 {
                assert!(hi[k] < want && hi[k] > want * 0.9, "{}", def.name());
            } else if def.shape == Shape::Arch && k == 2 {
                assert!((hi[k] - want).abs() < 1e-5, "{}", def.name());
            } else {
                assert_eq!(hi[k], want, "{} axis {k}", def.name());
            }
        }
    }
}

#[test]
fn a_block_has_the_expected_coordinates_and_roles() {
    let solid = solid(&PieceDef::dev(Shape::Block, [100, 200, 300], 0));
    let mut coords: Vec<u32> = solid
        .shell
        .iter()
        .flat_map(|f| f.ring.iter().flatten().map(|v| v.to_bits()))
        .collect();
    coords.sort_unstable();
    coords.dedup();
    let want: Vec<u32> = [0.0f32, 1.0, 2.0, 3.0]
        .iter()
        .map(|v| v.to_bits())
        .collect();
    assert_eq!(coords, want);
    assert_eq!(solid.shell.len(), 6);
    let roles = |r: Role| solid.shell.iter().filter(|f| f.role == r).count();
    assert_eq!(
        (
            roles(Role::Floor),
            roles(Role::Ceiling),
            roles(Role::Wall),
            roles(Role::Trim)
        ),
        (1, 1, 4, 0)
    );
}

#[test]
fn roles_follow_the_faces() {
    let ramp = solid(&PieceDef::dev(Shape::Ramp, [200, 80, 120], 0));
    let slope = ramp
        .shell
        .iter()
        .find(|f| f.ring.len() == 4 && f.normal()[0] != 0.0 && f.normal()[2] != 0.0)
        .unwrap();
    assert_eq!(slope.role, Role::Floor);
    let wedge = solid(&PieceDef::dev(Shape::Wedge, [150, 90, 60], 0));
    assert_eq!(
        wedge.shell.iter().filter(|f| f.role == Role::Wall).count(),
        3
    );

    let stairs = solid(&PieceDef::dev(Shape::Stairs, [240, 100, 128], 8));
    let count = |r: Role| stairs.shell.iter().filter(|f| f.role == r).count();
    assert_eq!(count(Role::Trim), 8, "a riser a step");
    assert_eq!(count(Role::Floor), 8, "a tread a step");
    assert_eq!(count(Role::Ceiling), 1);

    let arch = solid(&PieceDef::dev(Shape::Arch, [256, 32, 200], 8));
    assert_eq!(
        arch.shell.iter().filter(|f| f.role == Role::Trim).count(),
        8,
        "the soffit"
    );
    assert!(arch.shell.iter().any(|f| f.role == Role::Floor));
}

#[test]
fn a_role_is_that_of_the_piece_space_normal() {
    assert_eq!(Role::of_normal([0.0, 0.0, 1.0]), Role::Floor);
    assert_eq!(Role::of_normal([0.0, 0.0, -2.0]), Role::Ceiling);
    assert_eq!(Role::of_normal([1.0, 0.0, 0.1]), Role::Wall);
    assert_eq!(Role::of_normal([1.0, 0.0, 1.1]), Role::Floor);
    assert_eq!(Role::of_normal([0.0, 1.0, -1.1]), Role::Ceiling);
}

#[test]
fn two_blocks_a_tile_apart_share_coordinates_along_their_seam() {
    let tile = 256u16;
    let a = solid(&PieceDef::dev(Shape::Block, [tile, 192, 64], 0));
    let b = solid(&PieceDef::dev(Shape::Block, [128, 192, 64], 0));
    // The +x face of a and the -x face of b are one wall of texture: equal
    // coordinates for equal y and z.
    let face = |s: &crate::Solid, sign: f32| {
        s.shell
            .iter()
            .find(|f| f.normal()[0] * sign > 0.0)
            .unwrap()
            .clone()
    };
    let (fa, fb) = (face(&a, 1.0), face(&b, -1.0));
    let mut uv_a: Vec<[u32; 2]> = fa
        .uv
        .iter()
        .map(|p| [p[0].to_bits(), p[1].to_bits()])
        .collect();
    let mut uv_b: Vec<[u32; 2]> = fb
        .uv
        .iter()
        .map(|p| [p[0].to_bits(), p[1].to_bits()])
        .collect();
    uv_a.sort_unstable();
    uv_b.sort_unstable();
    assert_eq!(uv_a, uv_b);
    // On the top, b at one tile continues a: u differs by one whole tile.
    let top = |s: &crate::Solid| {
        s.shell
            .iter()
            .find(|f| f.role == Role::Floor)
            .unwrap()
            .clone()
    };
    let (ta, tb) = (top(&a), top(&b));
    let u_end_a = ta.uv.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    let u_start_b = tb.uv.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    assert!(
        (u_end_a - (u_start_b + 1.0)).abs() < 1e-4,
        "{u_end_a} {u_start_b}"
    );
    // A unit is 1/256 of a tile on both axes.
    let widest = ta.uv.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    assert!((widest - 192.0 / 256.0).abs() < 1e-4);
}

#[test]
fn generation_is_repeatable() {
    for def in sample_defs() {
        assert_eq!(solid(&def), solid(&def));
        assert_eq!(solid(&def).bits(), solid(&def).bits());
    }
}

/// The golden hashes: a SHA-256 of each sample's vertex bits.
fn golden_lines() -> String {
    let mut out = String::new();
    for def in sample_defs().into_iter().chain(natural_defs()) {
        let sum = Sha256::digest(solid(&def).bits());
        let hex: String = sum.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{} {hex}\n", def.name()));
    }
    out
}

#[test]
fn each_shapes_vertex_bits_match_the_golden_hash() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/tests/golden.txt");
    let now = golden_lines();
    if std::env::var_os("H2_BLESS_GOLDEN").is_some() {
        std::fs::write(path, &now).expect("golden.txt is writable");
        return;
    }
    let want = include_str!("golden.txt");
    assert_eq!(
        now, want,
        "the generators' bits changed; if that is meant, rerun with H2_BLESS_GOLDEN=1"
    );
}

#[test]
fn the_rational_circle_is_symmetric_and_round() {
    for sides in (8..=24).step_by(2) {
        for k in 0..sides {
            let (x, y) = crate::circle::point(sides, k);
            let r = f64::from(x).hypot(f64::from(y));
            assert!((r - 65536.0).abs() < 2.0, "{sides} {k}: {r}");
            assert_eq!(crate::circle::point(sides, (sides - k) % sides), (x, -y));
            if k <= sides / 2 {
                assert_eq!(crate::circle::point(sides, sides / 2 - k).0, -x);
            }
        }
    }
}

#[test]
fn a_cylinder_fits_its_bounds_and_an_arch_has_an_opening() {
    let cyl = solid(&PieceDef::dev(Shape::Cylinder, [100, 140, 180], 12));
    assert_eq!(cyl.bounds().1, [1.0, 1.4, 1.8]);
    assert_eq!(cyl.shell.len(), 12 + 2);
    let arch = solid(&PieceDef::dev(Shape::Arch, [256, 32, 200], 8));
    // The middle of the opening is outside, a point in a pier inside.
    assert!(!inside_shell(&arch.shell, [1.28, 0.16, 0.5]));
    assert!(inside_shell(&arch.shell, [0.1, 0.16, 0.5]));
}
