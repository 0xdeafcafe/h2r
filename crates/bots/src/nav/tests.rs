use super::*;
use pathfind::generate::{Options, generate};
use pathfind::geom::{V3 as GV3, newell};
use tags::collision::{Bsp, Edge as CEdge, Plane, Surface, Vertex};

// The same builder as pathfind's generator tests.
/// A collision BSP from polygons, with edges shared the way the engine's
/// ring rule expects: an edge's left surface walks it start -> end.
fn bsp_from(points: &[GV3], polys: &[&[u16]]) -> Bsp {
    let mut bsp = Bsp {
        vertices: points
            .iter()
            .map(|&p| Vertex { point: p, edge: 0 })
            .collect(),
        ..Bsp::default()
    };
    let mut ring_edges: Vec<Vec<usize>> = Vec::new();
    for (s, poly) in polys.iter().enumerate() {
        let pts: Vec<GV3> = poly.iter().map(|&i| points[usize::from(i)]).collect();
        let n = newell(&pts);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = [n[0] / len, n[1] / len, n[2] / len];
        let d = n[0] * pts[0][0] + n[1] * pts[0][1] + n[2] * pts[0][2];
        bsp.planes.push(Plane([n[0], n[1], n[2], d]));
        let mut mine = Vec::new();
        for k in 0..poly.len() {
            let (u, v) = (poly[k], poly[(k + 1) % poly.len()]);
            let e = match bsp
                .edges
                .iter()
                .position(|e| e.start_vertex == v && e.end_vertex == u)
            {
                Some(e) => {
                    bsp.edges[e].right_surface = s as u16;
                    e
                }
                None => {
                    bsp.edges.push(CEdge {
                        start_vertex: u,
                        end_vertex: v,
                        forward_edge: 0,
                        reverse_edge: 0,
                        left_surface: s as u16,
                        right_surface: u16::MAX,
                    });
                    bsp.edges.len() - 1
                }
            };
            mine.push(e);
        }
        bsp.surfaces.push(Surface {
            plane: s as u16,
            first_edge: mine[0] as u16,
            flags: 0,
            breakable_surface: 0xff,
            material: 0,
        });
        ring_edges.push(mine);
    }
    for (s, ring) in ring_edges.iter().enumerate() {
        for k in 0..ring.len() {
            let (e, next) = (ring[k], ring[(k + 1) % ring.len()] as u16);
            if usize::from(bsp.edges[e].left_surface) == s {
                bsp.edges[e].forward_edge = next;
            } else {
                bsp.edges[e].reverse_edge = next;
            }
        }
    }
    for e in 0..bsp.edges.len() {
        if bsp.edges[e].right_surface == u16::MAX {
            bsp.edges[e].reverse_edge = e as u16;
        }
    }
    bsp
}

/// Axis-aligned floor quads `(x0, y0, x1, y1, z)`, counter-clockwise from
/// above, sharing vertices where they meet.
fn floors(quads: &[(f32, f32, f32, f32, f32)]) -> NavMesh {
    let mut points: Vec<GV3> = Vec::new();
    let idx = |p: GV3, points: &mut Vec<GV3>| -> u16 {
        match points.iter().position(|q| q == &p) {
            Some(i) => i as u16,
            None => {
                points.push(p);
                (points.len() - 1) as u16
            }
        }
    };
    let mut polys: Vec<Vec<u16>> = Vec::new();
    for &(x0, y0, x1, y1, z) in quads {
        let ring = [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]];
        polys.push(ring.iter().map(|&p| idx(p, &mut points)).collect());
    }
    let refs: Vec<&[u16]> = polys.iter().map(|p| p.as_slice()).collect();
    let bsp = bsp_from(&points, &refs);
    let pf = generate(&bsp, 0, 0, &Options::default()).unwrap();
    NavMesh::from_pathfinding(&pf, &Build::default())
}

fn kinds(m: &NavMesh, from: u32, to: u32) -> Vec<Kind> {
    m.out(from).filter(|e| e.to == to).map(|e| e.kind).collect()
}

#[test]
fn floors_sharing_an_edge_are_walked_both_ways() {
    let m = floors(&[(0.0, 0.0, 2.0, 2.0, 0.0), (2.0, 0.0, 4.0, 2.0, 0.0)]);
    assert_eq!(m.polys.len(), 2);
    assert_eq!(kinds(&m, 0, 1), vec![Kind::Walk]);
    assert_eq!(kinds(&m, 1, 0), vec![Kind::Walk]);
    let p = m.path([0.5, 1.0, 0.0], [3.5, 1.0, 0.0]).unwrap();
    assert_eq!(
        p,
        vec![Waypoint {
            at: [3.5, 1.0, 0.0],
            kind: Kind::Walk
        }],
        "straight across"
    );
}

#[test]
fn locate_finds_the_floor_under_a_biped() {
    let m = floors(&[(0.0, 0.0, 2.0, 2.0, 0.0), (0.0, 0.0, 2.0, 2.0, 3.0)]);
    let lower = m.locate([1.0, 1.0, 0.3], 1.0).unwrap();
    let upper = m.locate([1.0, 1.0, 3.3], 1.0).unwrap();
    assert_ne!(lower, upper);
    assert_eq!(m.polys[upper as usize].center[2], 3.0);
    assert_eq!(
        m.locate([1.0, 1.0, 2.0], 1.0),
        None,
        "in the air between floors"
    );
    assert_eq!(m.locate([9.0, 9.0, 0.0], 1.0), None);
}

#[test]
fn an_l_shaped_corridor_turns_at_the_inner_corner() {
    // (0..2, 0..2) -> (2..4, 0..2) -> (2..4, 2..4): around the corner at (2, 2).
    let m = floors(&[
        (0.0, 0.0, 2.0, 2.0, 0.0),
        (2.0, 0.0, 4.0, 2.0, 0.0),
        (2.0, 2.0, 4.0, 4.0, 0.0),
    ]);
    let p = m.path([0.5, 1.8, 0.0], [2.2, 3.5, 0.0]).unwrap();
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(dist_xy(p[0].at, [2.0, 2.0, 0.0]) < 1e-4, "{p:?}");
    assert_eq!(p[1].at, [2.2, 3.5, 0.0]);
}

#[test]
fn a_ledge_is_dropped_off_but_not_climbed() {
    // A platform 1.0 high beside the floor, not sharing an edge.
    let m = floors(&[(0.0, 0.0, 2.0, 2.0, 0.0), (2.0, 0.0, 4.0, 2.0, 1.0)]);
    assert_eq!(kinds(&m, 1, 0), vec![Kind::Drop]);
    assert!(kinds(&m, 0, 1).is_empty());
    let down = m.path([3.5, 1.0, 1.0], [0.5, 1.0, 0.0]).unwrap();
    assert!(down.iter().any(|w| w.kind == Kind::Drop), "{down:?}");
    assert_eq!(m.path([0.5, 1.0, 0.0], [3.5, 1.0, 1.0]), None);
}

#[test]
fn a_low_ledge_is_jumped_onto_and_a_seam_is_walked() {
    let m = floors(&[
        (0.0, 0.0, 2.0, 2.0, 0.0),
        (2.0, 0.0, 4.0, 2.0, 0.25),
        (-2.0, 0.0, 0.0, 2.0, 0.05),
    ]);
    // Polygons follow the walkable surfaces' order.
    assert_eq!(kinds(&m, 0, 1), vec![Kind::Jump]);
    assert_eq!(kinds(&m, 1, 0), vec![Kind::Drop]);
    assert_eq!(kinds(&m, 0, 2), vec![Kind::Walk]);
    assert_eq!(kinds(&m, 2, 0), vec![Kind::Walk]);
}

#[test]
fn a_narrow_gap_is_jumped_and_a_wide_one_is_not() {
    let m = floors(&[
        (0.0, 0.0, 2.0, 2.0, 0.0),
        (2.6, 0.0, 4.0, 2.0, 0.0),
        (5.5, 0.0, 7.0, 2.0, 0.0),
    ]);
    assert_eq!(kinds(&m, 0, 1), vec![Kind::Jump]);
    assert_eq!(kinds(&m, 1, 0), vec![Kind::Jump]);
    assert!(kinds(&m, 1, 2).is_empty());
    let p = m.path([0.5, 1.0, 0.0], [3.5, 1.0, 0.0]).unwrap();
    let jump = p.iter().position(|w| w.kind == Kind::Jump).expect("a jump");
    assert!(
        p[jump - 1].at[0] >= 1.99,
        "the jump starts at the edge: {p:?}"
    );
}

#[test]
fn the_file_round_trips_and_refuses_damage() {
    let m = floors(&[(0.0, 0.0, 2.0, 2.0, 0.0), (2.0, 0.0, 4.0, 2.0, 1.0)]);
    let b = m.to_bytes();
    assert_eq!(NavMesh::from_bytes(&b), Some(m));
    assert_eq!(NavMesh::from_bytes(&b[..b.len() - 1]), None);
    let mut bad = b.clone();
    bad[0] = b'X';
    assert_eq!(NavMesh::from_bytes(&bad), None);
    let mut extra = b;
    extra.push(0);
    assert_eq!(NavMesh::from_bytes(&extra), None);
}

#[test]
fn the_follower_walks_the_waypoints_in_turn() {
    let path = vec![
        Waypoint {
            at: [2.0, 0.0, 0.0],
            kind: Kind::Walk,
        },
        Waypoint {
            at: [2.0, 2.0, 0.0],
            kind: Kind::Walk,
        },
    ];
    let mut f = Follower::new(path, 0);
    let s = f.steer([0.0, 0.0, 0.0], 1);
    assert_eq!((s.dir, s.jump, s.replan), ([1.0, 0.0], false, false));
    let s = f.steer([1.9, 0.0, 0.0], 2);
    assert!((s.dir[1] - 1.0).abs() < 0.01, "on to the second: {s:?}");
    let s = f.steer([2.0, 1.9, 0.0], 3);
    assert!(s.replan && f.done());
}

#[test]
fn a_stuck_follower_jumps_once_then_asks_for_a_new_path() {
    let mut f = Follower::new(
        vec![Waypoint {
            at: [5.0, 0.0, 0.0],
            kind: Kind::Walk,
        }],
        0,
    );
    for frame in 0..STUCK_FRAMES {
        let s = f.steer([0.0, 0.0, 0.0], frame);
        assert!(!s.jump && !s.replan, "frame {frame}");
    }
    assert!(f.steer([0.0, 0.0, 0.0], STUCK_FRAMES).jump);
    let s = f.steer([0.0, 0.0, 0.0], 2 * STUCK_FRAMES);
    assert!(s.replan);
}

#[test]
fn the_follower_jumps_at_a_jump() {
    let mut f = Follower::new(
        vec![Waypoint {
            at: [0.8, 0.0, 0.0],
            kind: Kind::Jump,
        }],
        0,
    );
    assert!(f.steer([0.0, 0.0, 0.0], 1).jump);
}
