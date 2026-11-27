//! Sculpt brushes on synthetic grids (docs/editor/modeling.md,
//! "Sculpting"): each brush's shape, refinement, the cut's clean rim, the
//! op as a replayable value, and refusals that change nothing.

mod common;

use common::*;
use mesh_edit::fixtures;
use mesh_edit::{Brush, Falloff, Mesh, MeshError, MeshOp, SelectHow, SelectMode, Stroke, VertId};

fn sculpt(m: &mut Mesh, s: Stroke) {
    ok(m, MeshOp::Sculpt(s));
}

fn flat_distance(p: [f32; 3], c: [f32; 3]) -> f32 {
    (p[0] - c[0]).hypot(p[1] - c[1])
}

/// An `n` by `n` grid of unit cells, refined nowhere (`detail` 1).
fn unit_grid(n: usize) -> Mesh {
    fixtures::grid(n, n, 1.0)
}

fn dab(brush: Brush, centre: [f32; 3], radius: f32, strength: f32) -> Stroke {
    Stroke { detail: Some(1.0), ..Stroke::dab(brush, centre, radius, strength) }
}

#[test]
fn falloffs_run_from_one_at_the_centre_to_zero_at_the_rim() {
    for f in Falloff::ALL {
        assert!(close(f.weight(0.0), 1.0), "{f:?}");
        assert_eq!(f.weight(1.0), 0.0, "{f:?}");
        assert_eq!(f.weight(2.0), 0.0, "{f:?}");
        assert_eq!(f.weight(f32::NAN), 0.0, "{f:?}");
        assert_eq!(Falloff::parse(f.name()), Some(f));
    }
    assert!(close(Falloff::Smooth.weight(0.5), 0.5));
    assert!(close(Falloff::Linear.weight(0.25), 0.75));
    assert!(close(Falloff::Sphere.weight(0.6), 0.8));
    assert!(close(Falloff::Constant.weight(0.99), 1.0));
    // The window's rings: where the weight is a half.
    assert!(close(Falloff::Linear.radius_at(0.5), 0.5));
    assert!(close(Falloff::Smooth.radius_at(0.5), 0.5));
    assert!(close(Falloff::Sphere.weight(Falloff::Sphere.radius_at(0.25)), 0.25));
    for b in Brush::ALL {
        assert_eq!(Brush::parse(b.name()), Some(b));
    }
}

#[test]
fn raise_lifts_each_vertex_by_the_falloff_of_its_distance() {
    for falloff in Falloff::ALL {
        let mut m = unit_grid(8);
        let centre = [4.0, 4.0, 0.0];
        let s = Stroke { falloff, ..dab(Brush::Raise, centre, 2.5, 1.5) };
        sculpt(&mut m, s.clone());
        assert_eq!(m.counts(), (81, 144, 64), "unit cells need no refining");
        for v in m.vert_ids() {
            let p = m.position(v);
            let want = 1.5 * falloff.weight(flat_distance(p, centre) / 2.5);
            assert!(close(p[2], want), "{falloff:?} at {p:?}: {} != {want}", p[2]);
            assert!(close(s.weight_at([p[0], p[1], 0.0]) * 1.5, want));
        }
        // The plan does not move.
        let p = m.position(VertId(4 * 9 + 5));
        assert_eq!((p[0], p[1]), (5.0, 4.0));
    }
}

#[test]
fn lower_is_raise_the_other_way_and_marks_what_moved() {
    let mut m = unit_grid(8);
    sculpt(&mut m, dab(Brush::Lower, [4.0, 4.0, 0.0], 2.0, 0.5));
    let centre = m.position(VertId(4 * 9 + 4));
    assert!(close(centre[2], -0.5));
    for v in m.vert_ids() {
        let p = m.position(v);
        assert!(p[2] <= 0.0);
        if flat_distance(p, [4.0, 4.0, 0.0]) >= 2.0 {
            assert_eq!(p[2], 0.0, "outside the radius at {p:?}");
        }
    }
    // Moved faces are lightmap-stale and their normals tilt off +z.
    let f = m.vert_faces(VertId(4 * 9 + 3))[0];
    assert_ne!(m.face_flags(f) & mesh_edit::face_flags::LIGHTMAP_STALE, 0);
    let n = m.face_corners(f)[0].normal;
    assert!(close(n[0] * n[0] + n[1] * n[1] + n[2] * n[2], 1.0));
}

#[test]
fn a_stroke_is_its_path_swept_not_its_dabs_summed() {
    let mut m = unit_grid(10);
    let s = Stroke {
        points: vec![[2.0, 5.0, 0.0], [8.0, 5.0, 0.0]],
        falloff: Falloff::Linear,
        ..dab(Brush::Raise, [0.0; 3], 2.0, 1.0)
    };
    sculpt(&mut m, s);
    for v in m.vert_ids() {
        let p = m.position(v);
        let x = p[0].clamp(2.0, 8.0);
        let want = Falloff::Linear.weight(flat_distance(p, [x, 5.0, 0.0]) / 2.0);
        assert!(close(p[2], want), "at {p:?}");
    }
    // The whole ridge between the ends is at full height.
    for i in 2..=8 {
        assert!(close(m.position(VertId(5 * 11 + i))[2], 1.0));
    }
}

#[test]
fn flatten_moves_heights_toward_the_target_by_the_fraction() {
    let mut m = unit_grid(8);
    let all = Stroke { falloff: Falloff::Constant, ..dab(Brush::Raise, [4.0, 4.0, 0.0], 20.0, 2.0) };
    sculpt(&mut m, all);
    // Half way to 1 inside the radius, under constant falloff.
    let s = Stroke {
        falloff: Falloff::Constant,
        height: Some(1.0),
        ..dab(Brush::Flatten, [4.0, 4.0, 2.0], 2.5, 0.5)
    };
    sculpt(&mut m, s);
    for v in m.vert_ids() {
        let p = m.position(v);
        let want = if flat_distance(p, [4.0, 4.0, 0.0]) < 2.5 { 1.5 } else { 2.0 };
        assert!(close(p[2], want), "at {p:?}");
    }
    // With no height the first centre's is the target.
    let s = Stroke { falloff: Falloff::Constant, ..dab(Brush::Flatten, [4.0, 4.0, 3.0], 20.0, 1.0) };
    sculpt(&mut m, s);
    assert!(m.vert_ids().all(|v| close(m.position(v)[2], 3.0)));
    // Flat already: nothing to move, refused.
    let s = Stroke { falloff: Falloff::Constant, ..dab(Brush::Flatten, [4.0, 4.0, 3.0], 20.0, 1.0) };
    refused(&mut m, MeshOp::Sculpt(s), MeshError::Stroke("the flatten stroke moves no vertex".into()));
}

#[test]
fn smooth_pulls_a_spike_to_its_neighbours_mean() {
    let mut m = unit_grid(4);
    let spike = VertId(2 * 5 + 2);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Vert });
    ok(&mut m, MeshOp::Select { ids: vec![spike.0], how: SelectHow::Set });
    ok(
        &mut m,
        MeshOp::Transform {
            matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 2.0]],
            pivot: None,
        },
    );
    // Only the spike is inside a half-unit radius.
    let s = Stroke { falloff: Falloff::Constant, ..dab(Brush::Smooth, [2.0, 2.0, 0.0], 0.5, 0.25) };
    sculpt(&mut m, s);
    assert!(close(m.position(spike)[2], 1.5), "a quarter of the way to 0");
    // Each vertex reads its neighbours as they were (one pass, not in
    // place): a full-strength pass over the spike and its ring.
    let s = Stroke { falloff: Falloff::Constant, ..dab(Brush::Smooth, [2.0, 2.0, 0.0], 1.1, 1.0) };
    sculpt(&mut m, s);
    assert!(close(m.position(spike)[2], 0.0));
    for n in m.vert_neighbors(spike) {
        assert!(close(m.position(n)[2], 1.5 / 4.0), "a neighbour has the spike and three flat");
    }
    // The selection is kept.
    assert!(m.selection().has_vert(spike));
}

#[test]
fn a_stroke_refines_coarse_faces_where_the_brush_needs_vertices() {
    let mut m = fixtures::grid(2, 2, 4.0);
    assert_eq!(m.counts(), (9, 12, 4));
    let s = Stroke { detail: Some(1.0), ..Stroke::dab(Brush::Raise, [4.0, 4.0, 0.0], 3.0, 1.0) };
    sculpt(&mut m, s);
    let (v, _, f) = m.counts();
    assert!(v > 9 && f > 4, "refined: {:?}", m.counts());
    assert_eq!(euler(&m), 1, "still a disc");
    // Every edge of a face the footprint reaches is at most the detail.
    for f in m.face_ids() {
        let ps = m.face_positions(f);
        if ps.iter().any(|&p| flat_distance(p, [4.0, 4.0, 0.0]) < 3.0) {
            for e in m.face_edges(f) {
                let [a, b] = m.edge_verts(e);
                let (pa, pb) = (m.position(a), m.position(b));
                assert!(flat_distance(pa, pb) <= 1.0 + 1e-4, "edge {pa:?} {pb:?}");
            }
        }
    }
    // Refined quads round the brush: the 4 by 4 cells became unit quads.
    assert!(m.face_ids().filter(|&f| m.face_len(f) == 4).count() >= 36);
    let top = m.vert_ids().map(|v| m.position(v)[2]).fold(f32::MIN, f32::max);
    assert!(close(top, 1.0), "the centre is a vertex and is raised fully");
    // New corners interpolate the coordinates: u = x / 8 on this grid.
    for f in m.face_ids() {
        for (k, &x) in m.face_verts(f).iter().enumerate() {
            let c = m.face_corners(f)[k];
            assert!(close(c.uv[0], m.position(x)[0] / 8.0), "uv {:?} at {:?}", c.uv, m.position(x));
        }
    }
}

#[test]
fn a_cut_makes_a_hole_whose_rim_is_on_the_brush_circle() {
    let mut m = unit_grid(10);
    let centre = [5.0, 5.0, 0.0];
    let s = Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Cut, centre, 2.3, 1.0) };
    sculpt(&mut m, s);
    assert_eq!(euler(&m), 0, "a disc with a hole: V - E + F = 0");
    // Nothing is left inside the circle.
    for f in m.face_ids() {
        let c = m.face_center(f);
        assert!(flat_distance(c, centre) > 2.3 - 1e-3, "face at {c:?}");
    }
    // The boundary edges off the grid's outer edge are the hole's rim, and
    // every rim vertex is on the circle: a clean edge, no stair steps.
    let outer = |p: [f32; 3]| p[0] == 0.0 || p[0] == 10.0 || p[1] == 0.0 || p[1] == 10.0;
    let rim: Vec<_> = m
        .edge_ids()
        .filter(|&e| m.edge_face_count(e) == 1)
        .filter(|&e| !m.edge_verts(e).iter().all(|&v| outer(m.position(v))))
        .collect();
    assert!(rim.len() >= 16, "the rim has {} edges", rim.len());
    for &e in &rim {
        for v in m.edge_verts(e) {
            let d = flat_distance(m.position(v), centre);
            assert!((d - 2.3).abs() < 1e-3, "rim vertex at {d}");
        }
    }
    // The rim is one closed loop: every rim vertex has two rim edges.
    for &e in &rim {
        for v in m.edge_verts(e) {
            let n = m.vert_edges(v).iter().filter(|x| rim.contains(x)).count();
            assert_eq!(n, 2);
        }
    }
}

#[test]
fn a_cut_along_a_path_goes_through_every_layer_under_it() {
    // A slab: the closed box cut straight through, top and bottom.
    let mut m = fixtures::cube(4.0);
    let s = Stroke {
        points: vec![[-1.0, 0.0, 2.0], [1.0, 0.0, 2.0]],
        detail: Some(0.5),
        ..Stroke::dab(Brush::Cut, [0.0; 3], 0.75, 1.0)
    };
    sculpt(&mut m, s);
    let boundary: Vec<_> = m.edge_ids().filter(|&e| m.edge_face_count(e) == 1).collect();
    let on = |z: f32| boundary.iter().filter(|&&e| m.edge_verts(e).iter().all(|&v| close(m.position(v)[2], z))).count();
    assert!(on(2.0) > 0 && on(-2.0) > 0, "a hole in the top and in the bottom");
    assert_eq!(on(2.0) + on(-2.0), boundary.len(), "the sides are whole");
    assert_eq!(euler(&m), 0, "a sphere with two holes");
}

#[test]
fn the_op_is_a_value_that_replays_to_the_same_mesh() {
    let start = fixtures::grid(3, 3, 2.0);
    let ops = vec![
        MeshOp::Sculpt(Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Raise, [3.0, 3.0, 0.0], 2.0, 1.0) }),
        MeshOp::Sculpt(Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Smooth, [3.0, 3.0, 0.0], 2.0, 0.5) }),
        MeshOp::Sculpt(Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Cut, [1.0, 1.0, 0.0], 0.8, 1.0) }),
    ];
    let json = serde_json::to_string(&ops).unwrap();
    assert!(json.contains(r#""op":"sculpt","brush":"raise""#), "{json}");
    let back: Vec<MeshOp> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ops);
    // The fields with defaults may be left out.
    let short: MeshOp =
        serde_json::from_str(r#"{"op":"sculpt","brush":"lower","points":[[1,1,0]],"radius":1}"#).unwrap();
    let MeshOp::Sculpt(s) = &short else { panic!("{short:?}") };
    assert_eq!((s.strength, s.falloff, s.axis, s.detail), (1.0, Falloff::Smooth, None, None));

    let mut a = start.clone();
    let mut b = start.clone();
    for op in &ops {
        ok(&mut a, op.clone());
    }
    for op in &back {
        ok(&mut b, op.clone());
    }
    assert_eq!(a, b, "same ops, same mesh, ids included");
    assert_ne!(a, start);
}

#[test]
fn a_stroke_on_a_wall_follows_its_axis() {
    // The grid stood up facing +x: a raise along +x pushes it out.
    let mut m = unit_grid(4);
    ok(&mut m, MeshOp::Mode { mode: SelectMode::Vert });
    ok(&mut m, MeshOp::SelectAll);
    ok(
        &mut m,
        MeshOp::Transform {
            matrix: [[0.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0]],
            pivot: Some([0.0; 3]),
        },
    );
    let s = Stroke { axis: Some([2.0, 0.0, 0.0]), ..dab(Brush::Raise, [0.0, 2.0, -2.0], 1.5, 1.0) };
    sculpt(&mut m, s);
    let x = m.vert_ids().map(|v| m.position(v)[0]).fold(f32::MIN, f32::max);
    assert!(close(x, 1.0));
    // A brush facing away from the faces moves nothing.
    let s = Stroke { axis: Some([-1.0, 0.0, 0.0]), ..dab(Brush::Raise, [0.0, 0.0, -2.0], 1.5, 1.0) };
    refused(&mut m, MeshOp::Sculpt(s), MeshError::Stroke("the raise stroke moves no vertex".into()));
}

#[test]
fn bad_strokes_are_refused_by_name_and_change_nothing() {
    let mut m = unit_grid(4);
    let base = dab(Brush::Raise, [2.0, 2.0, 0.0], 1.0, 1.0);
    let cases = [
        (Stroke { radius: 0.0, ..base.clone() }, "the radius must be above 0"),
        (Stroke { points: vec![], ..base.clone() }, "a stroke needs at least one point"),
        (Stroke { points: vec![[f32::NAN, 0.0, 0.0]], ..base.clone() }, "a stroke point is not finite"),
        (Stroke { strength: 0.0, ..base.clone() }, "the strength must be above 0"),
        (Stroke { axis: Some([0.0; 3]), ..base.clone() }, "the axis must be a finite direction"),
        (Stroke { detail: Some(0.001), ..base.clone() }, "the detail must be at least a 64th of the radius"),
        (Stroke { brush: Brush::Smooth, strength: 2.0, ..base.clone() }, "smooth's strength is a fraction, 0 to 1"),
        (Stroke { points: vec![[50.0, 50.0, 0.0]], ..base.clone() }, "the raise stroke moves no vertex"),
        (Stroke { brush: Brush::Cut, points: vec![[50.0, 50.0, 0.0]], ..base.clone() }, "the cut reaches no face"),
    ];
    for (s, why) in cases {
        refused(&mut m, MeshOp::Sculpt(s), MeshError::Stroke(why.into()));
    }
}

#[test]
fn a_stroke_past_the_vertex_budget_is_refused() {
    // 32,761 vertices; refining the footprint once would add about 35,000.
    let mut m = fixtures::grid(180, 180, 1.0);
    let s = Stroke { detail: Some(0.95), ..Stroke::dab(Brush::Raise, [90.0, 90.0, 0.0], 60.0, 1.0) };
    let before = m.clone();
    let e = m.apply(&MeshOp::Sculpt(s)).unwrap_err();
    let MeshError::Stroke(why) = &e else { panic!("{e:?}") };
    assert!(why.starts_with("the stroke needs more than 65535 vertices"), "{why}");
    assert_eq!(m, before);
}
