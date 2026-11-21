use std::collections::HashMap;

use tags::collision::{Bsp, Edge, Plane, Surface, Vertex};

use super::*;

const NONE: u16 = u16::MAX;

fn quad(verts: [Vec3; 4], normal: Vec3) -> Poly {
    let l = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    Poly {
        verts: verts.to_vec(),
        normal: normal.map(|v| v / l),
    }
}

/// A horizontal rectangle at height `z`, looking up or down.
fn flat(x: [f32; 2], y: [f32; 2], z: f32, up: bool) -> Poly {
    quad(
        [
            [x[0], y[0], z],
            [x[1], y[0], z],
            [x[1], y[1], z],
            [x[0], y[1], z],
        ],
        [0.0, 0.0, if up { 1.0 } else { -1.0 }],
    )
}

/// The Ascension slot's shape (docs/game/superbounce.md, "The touchdown
/// tick"): floor at z 0 on both sides, a slot 0.3 deep from y 0 to y
/// `width`. The wall at y 0 looks +y and leans 2.1° up, `(0, 0.9993,
/// 0.0372)`; the one at y `width` looks −y.
fn slot(width: f32) -> Vec<Poly> {
    let n = [0.0, 0.9993, 0.0372];
    // The tilted plane through (x, 0, 0): 0.9993 y + 0.0372 z = 0.
    let y_at = |z: f32| -n[2] * z / n[1];
    vec![
        flat([0.0, 2.0], [-1.0, 0.0], 0.0, true),
        flat([0.0, 2.0], [width, width + 1.0], 0.0, true),
        quad(
            [
                [0.0, 0.0, 0.0],
                [0.0, y_at(-0.3), -0.3],
                [2.0, y_at(-0.3), -0.3],
                [2.0, 0.0, 0.0],
            ],
            n,
        ),
        quad(
            [
                [0.0, width, 0.0],
                [2.0, width, 0.0],
                [2.0, width, -0.3],
                [0.0, width, -0.3],
            ],
            [0.0, -1.0, 0.0],
        ),
        // The slot's bottom: floor, but far below the top.
        flat([0.0, 2.0], [y_at(-0.3), width], -0.3, true),
    ]
}

fn bounces(spots: &[Spot]) -> Vec<&Spot> {
    spots.iter().filter(|s| s.kind == Kind::Bounce).collect()
}
fn crouches(spots: &[Spot]) -> Vec<&Spot> {
    spots.iter().filter(|s| s.kind == Kind::Crouch).collect()
}

#[test]
fn a_17_cm_slot_with_the_ascension_tilt_is_one_bounce_spot() {
    let spots = scan(&slot(0.17));
    let b = bounces(&spots);
    assert_eq!(b.len(), 1, "{spots:?}");
    let s = b[0];
    assert!((s.launch - 17.85).abs() < 0.2, "launch {}", s.launch);
    // Drift across the slot toward +y, along the tilted wall's normal: the
    // way every traced bounce came in.
    assert!(
        (s.drift[0]).abs() < 1e-3 && (s.drift[1] - 1.0).abs() < 1e-3,
        "{:?}",
        s.drift
    );
    // The crease runs along x at floor level over the slot's length.
    let [a, e] = s.edge;
    assert!((a[2]).abs() < 1e-3 && (e[2]).abs() < 1e-3, "{:?}", s.edge);
    assert!(((a[0] - e[0]).abs() - 2.0).abs() < 1e-3, "{:?}", s.edge);
    assert!(
        a[1] > 0.0 && a[1] < 0.17 && (a[1] - e[1]).abs() < 1e-3,
        "{:?}",
        s.edge
    );
    // Standing on the floor just short of the tilted wall, origin 1 cm up.
    assert!(s.stand[1] < 0.0 && s.stand[1] > -0.1, "{:?}", s.stand);
    assert!((s.stand[2] - 0.01).abs() < 1e-3, "{:?}", s.stand);
    // Half the landings sink deep enough at about 4.1 units/s: about a
    // metre of extra fall over a standing jump.
    assert!(s.fall > 0.5 && s.fall < 1.5, "fall {}", s.fall);
    assert!(s.score > 0.5 && s.score <= 1.0);
    assert!(crouches(&spots).is_empty(), "{spots:?}");
}

#[test]
fn a_gap_wider_than_the_capsule_is_no_bounce() {
    assert!(bounces(&scan(&slot(0.5))).is_empty());
}

#[test]
fn a_slot_with_no_upward_tilt_is_no_bounce() {
    let mut polys = slot(0.17);
    // Make the tilted wall vertical: nothing is left upward.
    polys[2] = quad(
        [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, -0.3],
            [2.0, 0.0, -0.3],
            [2.0, 0.0, 0.0],
        ],
        [0.0, 1.0, 0.0],
    );
    assert!(bounces(&scan(&polys)).is_empty());
}

#[test]
fn a_slot_split_into_pieces_is_still_one_spot() {
    let mut polys = slot(0.17);
    let n = polys[3].normal;
    // The −y wall in two halves along x.
    polys[3] = quad(
        [
            [0.0, 0.17, 0.0],
            [1.0, 0.17, 0.0],
            [1.0, 0.17, -0.3],
            [0.0, 0.17, -0.3],
        ],
        n,
    );
    polys.push(quad(
        [
            [1.0, 0.17, 0.0],
            [2.0, 0.17, 0.0],
            [2.0, 0.17, -0.3],
            [1.0, 0.17, -0.3],
        ],
        n,
    ));
    let spots = scan(&polys);
    let b = bounces(&spots);
    assert_eq!(b.len(), 1, "{spots:?}");
    assert!(((b[0].edge[0][0] - b[0].edge[1][0]).abs() - 2.0).abs() < 1e-3);
}

fn room(ceiling: f32) -> Vec<Poly> {
    vec![
        flat([-2.0, 4.0], [-2.0, 4.0], 0.0, true),
        flat([0.0, 2.0], [0.0, 2.0], ceiling, false),
    ]
}

#[test]
fn a_ceiling_at_crouch_height_is_a_crouch_spot() {
    let spots = scan(&room(0.6));
    let c = crouches(&spots);
    assert_eq!(c.len(), 1, "{spots:?}");
    let s = c[0];
    // Stand inside, under the ceiling.
    assert!((s.stand[0] - 1.0).abs() < 1e-3 && (s.stand[1] - 1.0).abs() < 1e-3);
    assert!((s.stand[2] - 0.01).abs() < 1e-3);
    // The edge is one of the ceiling's sides, on the floor.
    let [a, b] = s.edge;
    let on_side = |p: Vec3| {
        (p[0].abs() < 1e-3
            || (p[0] - 2.0).abs() < 1e-3
            || p[1].abs() < 1e-3
            || (p[1] - 2.0).abs() < 1e-3)
            && p[2].abs() < 1e-3
    };
    assert!(on_side(a) && on_side(b), "{:?}", s.edge);
    assert!((((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() - 2.0).abs() < 1e-3);
    assert!((s.score - 1.0).abs() < 1e-5, "{}", s.score);
    assert_eq!((s.fall, s.launch, s.drift), (0.0, 0.0, [0.0, 0.0]));
    assert!(bounces(&spots).is_empty());
}

#[test]
fn a_ceiling_with_standing_room_is_no_crouch_spot() {
    assert!(crouches(&scan(&room(0.8))).is_empty());
}

#[test]
fn a_ceiling_too_low_to_crouch_under_is_no_crouch_spot() {
    assert!(crouches(&scan(&room(0.4))).is_empty());
}

#[test]
fn a_gap_narrower_than_the_capsule_is_no_crouch_spot() {
    let polys = vec![
        flat([-2.0, 4.0], [-2.0, 4.0], 0.0, true),
        flat([0.0, 2.0], [0.0, 0.2], 0.6, false),
    ];
    assert!(crouches(&scan(&polys)).is_empty());
}

/// A collision BSP from face loops (outward counter-clockwise), with the
/// edge records the engine's ring walk expects: the first face to use an
/// edge is its left surface (start to end, forward edge), the second its
/// right (end to start, reverse edge).
fn bsp(points: &[Vec3], faces: &[(&[u16], Plane, bool)]) -> Bsp {
    let mut out = Bsp {
        vertices: points
            .iter()
            .map(|&point| Vertex { point, edge: 0 })
            .collect(),
        ..Bsp::default()
    };
    let mut by_key: HashMap<(u16, u16), u16> = HashMap::new();
    let mut rings = Vec::new();
    for (f, (ring, plane, flip)) in faces.iter().enumerate() {
        let f = f as u16;
        out.planes.push(*plane);
        let mut edges = Vec::new();
        for k in 0..ring.len() {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            let e = *by_key.entry((a.min(b), a.max(b))).or_insert_with(|| {
                out.edges.push(Edge {
                    start_vertex: a,
                    end_vertex: b,
                    forward_edge: NONE,
                    reverse_edge: NONE,
                    left_surface: f,
                    right_surface: NONE,
                });
                (out.edges.len() - 1) as u16
            });
            if out.edges[usize::from(e)].left_surface != f {
                out.edges[usize::from(e)].right_surface = f;
            }
            edges.push(e);
        }
        out.surfaces.push(Surface {
            plane: f | if *flip { 0x8000 } else { 0 },
            first_edge: edges[0],
            flags: 0,
            breakable_surface: 0xff,
            material: 0,
        });
        rings.push(edges);
    }
    for (f, edges) in rings.iter().enumerate() {
        for k in 0..edges.len() {
            let (e, next) = (edges[k], edges[(k + 1) % edges.len()]);
            let edge = &mut out.edges[usize::from(e)];
            if usize::from(edge.right_surface) == f {
                edge.reverse_edge = next;
            } else {
                edge.forward_edge = next;
            }
        }
    }
    out
}

/// A unit cube, vertex `x + 2y + 4z`. The planes at 0 are stored as the
/// +axis plane with the flip bit, as the engine's data does for shared
/// planes, so three faces exercise bit 15.
fn cube() -> Bsp {
    let points: Vec<Vec3> = (0..8)
        .map(|i| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32])
        .collect();
    let p = |n: Vec3, d: f32| Plane([n[0], n[1], n[2], d]);
    bsp(
        &points,
        &[
            (&[4, 5, 7, 6], p([0.0, 0.0, 1.0], 1.0), false),
            (&[0, 2, 3, 1], p([0.0, 0.0, 1.0], 0.0), true),
            (&[1, 3, 7, 5], p([1.0, 0.0, 0.0], 1.0), false),
            (&[0, 4, 6, 2], p([1.0, 0.0, 0.0], 0.0), true),
            (&[2, 6, 7, 3], p([0.0, 1.0, 0.0], 1.0), false),
            (&[0, 1, 5, 4], p([0.0, 1.0, 0.0], 0.0), true),
        ],
    )
}

fn close(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
}

#[test]
fn a_cube_is_six_faces_with_outward_normals() {
    let polys = polys(&cube(), None);
    assert_eq!(polys.len(), 6);
    let want = [
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
    ];
    for (p, n) in polys.iter().zip(want) {
        assert!(close(p.normal, n), "{:?} vs {n:?}", p.normal);
        assert_eq!(p.verts.len(), 4);
        // Each vertex is on the face, and the centre is half a unit inside
        // along the normal.
        let c = p.verts.iter().fold([0.0; 3], |a, v| {
            [a[0] + v[0] / 4.0, a[1] + v[1] / 4.0, a[2] + v[2] / 4.0]
        });
        let inward = [c[0] - 0.5 * n[0], c[1] - 0.5 * n[1], c[2] - 0.5 * n[2]];
        assert!(close(inward, [0.5, 0.5, 0.5]), "{:?}", p.verts);
    }
    // The ring starts at the face's first vertex, in its order.
    assert_eq!(
        polys[0].verts,
        vec![
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0]
        ]
    );
}

#[test]
fn a_transform_moves_vertices_and_turns_normals() {
    // A quarter turn about z, then (10, 20, 30): x -> y, y -> -x.
    let m = [
        [0.0, -1.0, 0.0, 10.0],
        [1.0, 0.0, 0.0, 20.0],
        [0.0, 0.0, 1.0, 30.0],
    ];
    let polys = polys(&cube(), Some(m));
    assert_eq!(polys.len(), 6);
    // The +x face looks +y, its first vertex (1, 0, 0) is at (10, 21, 30).
    assert!(
        close(polys[2].normal, [0.0, 1.0, 0.0]),
        "{:?}",
        polys[2].normal
    );
    assert!(
        close(polys[2].verts[0], [10.0, 21.0, 30.0]),
        "{:?}",
        polys[2].verts
    );
    // Scale doesn't change a normal's length.
    let scaled = [
        [2.0, 0.0, 0.0, 0.0],
        [0.0, 2.0, 0.0, 0.0],
        [0.0, 0.0, 2.0, 0.0],
    ];
    let polys = super::polys(&cube(), Some(scaled));
    assert!(close(polys[0].normal, [0.0, 0.0, 1.0]));
    assert!(close(polys[0].verts[2], [2.0, 2.0, 2.0]));
}

#[test]
fn a_surface_with_a_broken_ring_is_left_out() {
    let mut b = cube();
    b.surfaces[0].first_edge = 999;
    b.surfaces[1].plane = 77;
    assert_eq!(polys(&b, None).len(), 4);
}

#[test]
fn a_cube_on_its_own_has_no_spots() {
    assert!(scan(&polys(&cube(), None)).is_empty());
}

fn bounce_at(x: f32) -> Spot {
    Spot {
        kind: Kind::Bounce,
        edge: [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0]],
        stand: [x, -0.1, 0.01],
        fall: 0.0,
        launch: 17.85,
        drift: [0.0, 1.0],
        score: 1.0,
    }
}

#[test]
fn nearest_is_the_closest_bounce_edge() {
    let crouch = Spot {
        kind: Kind::Crouch,
        ..bounce_at(4.9)
    };
    let spots = [bounce_at(0.0), bounce_at(10.0), crouch];
    let (s, d) = nearest(&spots, [5.0, 3.0, 0.0]).unwrap();
    // (1, 0, 0) is 5 away from (5, 3); (10, 0, 0) is about 5.83.
    assert_eq!(s.edge[0][0], 0.0);
    assert!((d - 5.0).abs() < 1e-5, "{d}");
    // Closest point inside the segment.
    let (s, d) = nearest(&spots, [10.5, 2.0, 0.0]).unwrap();
    assert_eq!(s.edge[0][0], 10.0);
    assert!((d - 2.0).abs() < 1e-5);
    assert!(nearest(&spots[2..], [0.0; 3]).is_none());
}
