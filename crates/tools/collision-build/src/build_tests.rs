//! The compiler on invented shapes: the engine's own walks against the
//! polygons it was given.

use tags::collision::Child;

use crate::fixtures::{Brute, Rng, brute, cube, l_room, ramp, sheet};
use crate::query::{locate, vector};
use crate::{Error, Limits, Options, Polygon, TWO_SIDED, build, build_with, verify};

fn compile(polys: &[Polygon]) -> crate::Compiled {
    let c = build(polys).expect("compiles");
    let problems = verify::check(&c.bsp, 4);
    assert!(problems.is_empty(), "{problems:#?}");
    c
}

/// `count` random segments in `[lo, hi]`: every one the brute force can
/// call gives the same first hit (polygon and fraction) through the
/// engine's vector test, and no closed shape lets a segment into solid
/// unreported. Returns how many hit.
fn segments_match(polys: &[Polygon], lo: [f32; 3], hi: [f32; 3], seed: u64, count: usize) -> usize {
    let c = compile(polys);
    let mut rng = Rng(seed);
    let (mut called, mut hits) = (0, 0);
    for i in 0..count {
        let (a, b) = (rng.point(lo, hi), rng.point(lo, hi));
        let want = brute(polys, a, b);
        if want == Brute::Unsure {
            continue;
        }
        called += 1;
        let trace = vector(&c.bsp, a, b);
        match (want, trace.hit) {
            (Brute::Clear, None) => {}
            (Brute::Hit(t, poly), Some(h)) => {
                hits += 1;
                assert_eq!(
                    c.surface_source[h.surface], poly,
                    "segment {i} {a:?} -> {b:?}: surface {} is polygon {}, brute force hit {poly}",
                    h.surface, c.surface_source[h.surface]
                );
                assert!(
                    (f64::from(h.t) - t).abs() < 1e-4,
                    "segment {i}: t {} against {t}",
                    h.t
                );
            }
            (want, got) => panic!("segment {i} {a:?} -> {b:?}: brute force {want:?}, walk {got:?}"),
        }
        assert!(
            trace.missed.is_empty(),
            "segment {i} {a:?} -> {b:?} entered solid unreported: {:?}",
            trace.missed
        );
    }
    assert!(called > count / 2, "only {called} of {count} segments could be called");
    hits
}

#[test]
fn a_box_compiles_valid_and_walks() {
    let c = compile(&cube([-1.0; 3], [1.0; 3]));
    let b = &c.bsp;
    // Welded: eight corners, twelve edges each between two faces.
    assert_eq!((b.surfaces.len(), b.vertices.len(), b.edges.len()), (6, 8, 12));
    assert!(b.edges.iter().all(|e| e.right_surface != u16::MAX));
    assert_eq!(b.planes.len(), 6);
    // Inside is solid, outside open.
    assert_eq!(locate(b, [0.0; 3]), None);
    assert!(locate(b, [3.0, 0.0, 0.0]).is_some());
    // Each ring comes back in its input winding.
    for (s, poly) in cube([-1.0; 3], [1.0; 3]).iter().enumerate() {
        let ring = crate::query::ring(b, s);
        let k = ring.iter().position(|p| *p == poly.points[0]).expect("first corner on the ring");
        let turned: Vec<_> = (0..ring.len()).map(|i| ring[(k + i) % ring.len()]).collect();
        assert_eq!(turned, poly.points);
    }
}

#[test]
fn random_segments_match_brute_force_on_a_box() {
    let hits = segments_match(&cube([-1.0; 3], [1.0; 3]), [-3.0; 3], [3.0; 3], 1, 3000);
    assert!(hits > 100, "{hits}");
}

#[test]
fn random_segments_match_brute_force_in_a_concave_room() {
    let room = l_room();
    let hits = segments_match(&room, [-1.0, -1.0, -1.0], [5.0, 5.0, 4.0], 2, 3000);
    assert!(hits > 100, "{hits}");
    // The reflex corner's outside is solid, the room open.
    let c = compile(&room);
    assert_eq!(locate(&c.bsp, [3.0, 3.0, 1.0]), None);
    assert!(locate(&c.bsp, [1.0, 3.0, 1.0]).is_some());
    assert!(locate(&c.bsp, [3.0, 1.0, 1.0]).is_some());
}

#[test]
fn random_segments_match_brute_force_on_a_ramp() {
    let hits = segments_match(&ramp(), [-1.0, -1.0, -1.0], [5.0, 3.0, 3.0], 3, 3000);
    assert!(hits > 100, "{hits}");
}

#[test]
fn random_segments_match_brute_force_on_a_two_sided_sheet() {
    let hits = segments_match(&sheet(), [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0], 4, 3000);
    assert!(hits > 100, "{hits}");
    let c = compile(&sheet());
    // No solid anywhere; both sides' leaves carry the flag.
    assert!(locate(&c.bsp, [1.0, 1.0, 0.5]).is_some());
    assert!(locate(&c.bsp, [1.0, 1.0, 1.5]).is_some());
    assert!(c.bsp.leaves.iter().all(|l| l.flags & 1 == 1));
    assert!(c.bsp.surfaces.iter().all(|s| s.flags & TWO_SIDED != 0));
}

#[test]
fn every_reference_is_on_its_leafs_side_and_every_centroid_walks_home() {
    // `compile` runs `verify::check`; here a broken BSP must fail it.
    let mut c = build(&l_room()).unwrap();
    assert!(verify::check(&c.bsp, 4).is_empty());
    let r = c.bsp.refs.first_mut().unwrap();
    r.plane ^= 0x8000;
    let problems = verify::check(&c.bsp, 4);
    assert!(problems.iter().any(|p| p.contains("not on its path's side")), "{problems:#?}");

    let mut c = build(&sheet()).unwrap();
    c.bsp.leaves[0].flags = 0;
    let problems = verify::check(&c.bsp, 4);
    assert!(problems.iter().any(|p| p.contains("flag bit 0")), "{problems:#?}");
}

#[test]
fn a_ring_of_twelve_is_split_into_rings_of_eight_or_fewer() {
    // A twelve-sided prism: two twelve-gon caps and twelve sides.
    let n = 12;
    let at = |k: usize, z: f32| {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        [a.cos() * 2.0, a.sin() * 2.0, z]
    };
    let mut polys = vec![
        Polygon::new((0..n).map(|k| at(k, 1.0)).collect(), 0),
        Polygon::new((0..n).rev().map(|k| at(k, -1.0)).collect(), 0),
    ];
    for k in 0..n {
        let j = (k + 1) % n;
        polys.push(Polygon::new(vec![at(k, -1.0), at(j, -1.0), at(j, 1.0), at(k, 1.0)], 0));
    }
    let c = compile(&polys);
    assert_eq!(c.report.split, vec![0, 1]);
    assert_eq!(c.bsp.surfaces.len(), 2 * 2 + n);
    assert!((0..c.bsp.surfaces.len()).all(|s| c.bsp.surface_ring(s).len() <= 8));
    segments_match(&polys, [-3.0; 3], [3.0; 3], 5, 1500);
}

#[test]
fn a_bent_or_concave_polygon_is_triangulated_and_a_degenerate_one_dropped() {
    let mut polys = cube([-1.0; 3], [1.0; 3]);
    // The top bent: one corner lifted.
    polys[5].points[2][2] += 0.1;
    polys.push(Polygon::new(vec![[5.0, 0.0, 0.0], [5.0, 0.0, 0.0], [6.0, 0.0, 0.0]], 0));
    let c = build(&polys).unwrap();
    assert_eq!(c.report.triangulated, vec![5]);
    assert_eq!(c.report.dropped, vec![6]);
    assert_eq!(c.bsp.surfaces.len(), 5 + 2);
    assert!(verify::check(&c.bsp, 1).is_empty());
}

#[test]
fn nothing_to_compile_is_refused() {
    assert_eq!(build(&[]).unwrap_err(), Error::Empty);
    let flat = Polygon::new(vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], 0);
    assert_eq!(build(&[flat]).unwrap_err(), Error::Empty);
    let bad = Polygon::new(vec![[0.0; 3], [f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]], 0);
    assert_eq!(build(&[bad]).unwrap_err(), Error::NonFinite(0));
}

#[test]
fn overlapping_coplanar_polygons_are_refused() {
    let mut polys = cube([-1.0; 3], [1.0; 3]);
    let top = polys[5].clone();
    polys.push(top);
    assert!(matches!(build(&polys).unwrap_err(), Error::Overlap { .. }));
}

#[test]
fn counts_past_the_packed_fields_are_refused() {
    let limited = |polys: &[Polygon], f: fn(&mut Limits)| {
        let mut opts = Options::default();
        f(&mut opts.limits);
        build_with(polys, &opts).unwrap_err()
    };
    let room = l_room();
    let n = room.len();
    let too_big = |e: Error| match e {
        Error::TooBig { what, .. } => what,
        e => panic!("{e:?}"),
    };
    assert_eq!(
        limited(&room, |l| l.surfaces = 3),
        Error::TooBig { what: "surface", value: n, max: 3 }
    );
    assert_eq!(too_big(limited(&room, |l| l.vertices = 4)), "vertex");
    assert_eq!(too_big(limited(&room, |l| l.planes = 2)), "plane");
    assert_eq!(too_big(limited(&room, |l| l.edges = 5)), "edge");
    assert_eq!(too_big(limited(&room, |l| l.nodes = 2)), "3D node");
    assert_eq!(too_big(limited(&room, |l| l.leaves = 1)), "leaf");
    assert_eq!(too_big(limited(&room, |l| l.refs = 1)), "2D reference");
    assert_eq!(too_big(limited(&room, |l| l.leaf_refs = 0)), "leaf 2D reference");
    // The sheet's open leaves each reach its four pieces through 2D nodes.
    assert!(!build(&sheet()).unwrap().bsp.nodes_2d.is_empty());
    assert_eq!(too_big(limited(&sheet(), |l| l.nodes_2d = 0)), "2D node");
    // Halo 2's limits are what its fields hold: the last leaf is not the
    // 24-bit "none", the last surface not the 2D tree's.
    let h = Limits::HALO2;
    assert_ne!(Child::Leaf(h.leaves as u32 - 1).raw24(), Child::None.raw24());
    assert_ne!(Child::Node(h.nodes as u32 - 1).raw24(), Child::None.raw24());
    assert_eq!(h.surfaces - 1, 0x7ffe);
    assert_eq!(h.planes - 1, 0x7fff);
    assert_eq!(h.leaf_refs, u8::MAX as usize);
}

#[test]
fn one_plane_serves_both_faces_of_a_thin_wall() {
    // Two boxes face to face share the plane x = 1 from both sides.
    let mut polys = cube([-1.0; 3], [1.0; 3]);
    polys.extend(cube([1.0, -1.0, -1.0], [3.0, 1.0, 1.0]));
    let c = build(&polys).unwrap();
    let on = |s: usize| (c.bsp.surfaces[s].plane & 0x7fff, c.bsp.surfaces[s].plane & 0x8000 != 0);
    let (p1, f1) = on(1); // first box's +x
    let (p2, f2) = on(6); // second box's -x
    assert_eq!(p1, p2);
    assert_ne!(f1, f2);
    assert!(verify::check(&c.bsp, 1).is_empty());
}

#[test]
fn stage_blocks_are_the_encoded_blocks_by_name() {
    let c = compile(&ramp());
    let staged = crate::coll::stage_blocks(&c.bsp);
    let names: Vec<_> = staged.iter().map(|b| b.name).collect();
    assert_eq!(names, crate::coll::BLOCK_NAMES);
    let enc = c.bsp.encode_blocks(0);
    let body: Vec<u8> = staged.iter().flat_map(|b| b.bytes.iter().copied()).collect();
    assert_eq!(&enc[tags::collision::BSP_BLOCKS_SIZE..], &body[..]);
    assert_eq!(staged[5].count as usize, c.bsp.surfaces.len());
    for (b, stride) in staged.iter().zip(tags::collision::BSP_STRIDES) {
        assert_eq!(b.bytes.len(), b.count as usize * stride, "{}", b.name);
    }
    // And they decode back to the BSP.
    let enc = c.bsp.encode_blocks(0x1000_0000);
    assert_eq!(
        tags::collision::Bsp::decode_blocks(&enc, 0x1000_0000, 0).unwrap(),
        c.bsp
    );
}
