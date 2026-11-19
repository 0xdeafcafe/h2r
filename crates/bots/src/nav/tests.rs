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
    let bsp = floors_bsp(quads);
    let pf = generate(&bsp, 0, 0, &Options::default()).unwrap();
    NavMesh::from_pathfinding(&pf, &Build::default())
}

/// [`floors`]' collision.
fn floors_bsp(quads: &[(f32, f32, f32, f32, f32)]) -> Bsp {
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
    bsp_from(&points, &refs)
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
    assert_eq!(p.last().unwrap().at, [2.2, 3.5, 0.0]);
    // It turns at the corner (2, 2), a body's width off it, not on it.
    let turns = &p[..p.len() - 1];
    assert!(!turns.is_empty(), "{p:?}");
    for w in turns {
        let d = dist_xy(w.at, [2.0, 2.0, 0.0]);
        assert!(d > CLEARANCE - 0.01 && d < 0.5, "{p:?}");
    }
}

#[test]
fn wall_corners_are_kept_clear_and_open_vertices_are_not() {
    let m = ring();
    // (2, 2) is inside the ring: every side meeting there is walked across.
    assert!(!m.is_corner([2.0, 2.0, 0.0]));
    assert!(m.is_corner([0.0, 0.0, 0.0]));
    assert!(m.is_corner([2.0, 0.0, 0.0]));
    // A portal from the outer wall to the middle is pulled in at the wall
    // end only.
    let e = m
        .edges
        .iter()
        .find(|e| e.from == 0 && e.to == 1)
        .copied()
        .unwrap();
    let (a, b) = m.portal(&e);
    let (wall, open) = if a[1] < b[1] { (a, b) } else { (b, a) };
    assert!((wall[1] - CLEARANCE).abs() < 1e-4, "{a:?} {b:?}");
    assert!((open[1] - 2.0).abs() < 1e-4, "{a:?} {b:?}");
    // Straight across the middle: nothing to go around.
    let p = m.path([0.5, 0.5, 0.0], [3.5, 3.5, 0.0]).unwrap();
    for w in &p {
        assert!((w.at[0] - w.at[1]).abs() < 1e-3, "on the diagonal: {p:?}");
    }
}

/// Two 4x4 rooms joined by a doorway one unit wide (Y 1.5 to 2.5), the
/// way the mesh builder links surfaces that do not share an edge: each
/// room's whole wall side is linked to the doorway.
fn doorway() -> NavMesh {
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Poly::new(vec![
            [x0, y0, 0.0],
            [x1, y0, 0.0],
            [x1, y1, 0.0],
            [x0, y1, 0.0],
        ])
    };
    let polys = vec![
        quad(0.0, 0.0, 4.0, 4.0),
        quad(4.0, 1.5, 5.0, 2.5),
        quad(5.0, 0.0, 9.0, 4.0),
    ];
    let e = |from, to, a: V3, b: V3| Edge {
        from,
        to,
        a,
        b,
        kind: Kind::Walk,
    };
    let edges = vec![
        e(0, 1, [4.0, 0.0, 0.0], [4.0, 4.0, 0.0]),
        e(1, 0, [4.0, 1.5, 0.0], [4.0, 2.5, 0.0]),
        e(1, 2, [5.0, 1.5, 0.0], [5.0, 2.5, 0.0]),
        e(2, 1, [5.0, 0.0, 0.0], [5.0, 4.0, 0.0]),
    ];
    NavMesh::new(polys, edges)
}

/// Where the path's legs cross the line X = `x`.
fn crossings(from: V3, p: &[Waypoint], x: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut prev = from;
    for w in p {
        let (a, b) = (prev, w.at);
        if (a[0] - x) * (b[0] - x) <= 0.0 && (b[0] - a[0]).abs() > 1e-6 {
            let t = (x - a[0]) / (b[0] - a[0]);
            out.push(a[1] + (b[1] - a[1]) * t);
        }
        prev = w.at;
    }
    out
}

#[test]
fn a_doorway_off_a_long_wall_is_walked_through_not_around() {
    let m = doorway();
    // The rooms' far corners on the doorway's side: a straight line would
    // go through the wall above the door.
    let (from, to) = ([1.0, 3.8, 0.0], [8.0, 3.8, 0.0]);
    let p = m.path(from, to).unwrap();
    assert_eq!(p.last().unwrap().at, to);
    for x in [4.0, 5.0] {
        let ys = crossings(from, &p, x);
        assert!(!ys.is_empty(), "{p:?}");
        for y in ys {
            assert!(
                (1.5 + CLEARANCE - 0.01..=2.5 - CLEARANCE + 0.01).contains(&y),
                "through the door at X {x}, clear of its sides: {y} {p:?}"
            );
        }
    }
    // The way in is the doorway's span, not the whole wall.
    let into = m.edges.iter().find(|e| e.from == 0).copied().unwrap();
    let (a, b) = m.portal(&into);
    assert!((a[1].min(b[1]) - 1.8).abs() < 1e-4, "{a:?} {b:?}");
    assert!((a[1].max(b[1]) - 2.2).abs() < 1e-4, "{a:?} {b:?}");
}

#[test]
fn a_start_or_goal_off_the_mesh_snaps_to_the_nearest_floor() {
    let m = ring();
    // A goal in the air (someone seen mid-jump): the path ends on the floor
    // under it.
    let p = m.path([0.5, 0.5, 0.0], [3.0, 3.0, 2.0]).unwrap();
    let end = p.last().unwrap().at;
    assert!(
        dist_xy(end, [3.0, 3.0, 0.0]) < 1e-4 && end[2].abs() < 1e-4,
        "{p:?}"
    );
    // A start just off the west side (on a crate, knocked off the edge):
    // first onto the floor, a little inside its edge.
    let p = m.path([-1.0, 1.0, 0.0], [3.5, 3.5, 0.0]).unwrap();
    let first = p[0].at;
    assert!((first[0] - CLEARANCE).abs() < 1e-3, "{p:?}");
    assert!((first[1] - 1.0).abs() < 0.35, "{p:?}");
    assert_eq!(p[0].kind, Kind::Walk);
    assert_eq!(p.last().unwrap().at, [3.5, 3.5, 0.0]);
    // Too far from any floor: no path.
    assert_eq!(m.path([-5.0, 1.0, 0.0], [3.5, 3.5, 0.0]), None);
    assert_eq!(m.path([0.5, 0.5, 0.0], [3.0, 3.0, 9.0]), None);
    // poly_near: on the mesh, the polygon under it; off it, the nearest.
    assert_eq!(m.poly_near([0.5, 0.5, 0.0]), m.locate([0.5, 0.5, 0.0], 1.0));
    assert_eq!(m.poly_near([-1.0, 1.0, 0.0]), Some(0));
    assert_eq!(m.poly_near([-9.0, 1.0, 0.0]), None);
}

#[test]
fn a_start_beside_and_below_the_floor_jumps_up_onto_it() {
    let m = ring();
    // Half a unit under the floor's edge, beside it: the floor is higher,
    // so the first step is a jump.
    let p = m.path([-0.5, 1.0, -0.4], [1.5, 1.5, 0.0]).unwrap();
    assert_eq!(p[0].kind, Kind::Jump, "{p:?}");
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
fn the_follower_bends_toward_the_next_leg_near_a_corner() {
    let walk = |x: f32, y: f32| Waypoint {
        at: [x, y, 0.0],
        kind: Kind::Walk,
    };
    // Far from the corner: straight at it.
    let mut f = Follower::new(vec![walk(2.0, 0.0), walk(2.0, 2.0)], 0);
    assert_eq!(f.steer([0.0, 0.0, 0.0], 1).dir, [1.0, 0.0]);
    // Near it: bending left, toward the next leg, still mostly on.
    let s = f.steer([1.6, 0.0, 0.0], 2);
    assert!(s.dir[1] > 0.1 && s.dir[0] > 0.5, "{s:?}");
    assert!((s.dir[0].hypot(s.dir[1]) - 1.0).abs() < 1e-4);
    // Not into a jump: those start at their edge.
    let mut j = Follower::new(
        vec![
            walk(2.0, 0.0),
            Waypoint {
                at: [2.0, 2.0, 0.0],
                kind: Kind::Jump,
            },
        ],
        0,
    );
    let d = j.steer([1.6, 0.0, 0.0], 1).dir;
    assert!((d[0] - 1.0).abs() < 1e-5 && d[1].abs() < 1e-5, "{d:?}");
}

#[test]
fn a_follower_that_fell_below_its_path_plans_again() {
    let up_there = |z: f32| {
        Follower::new(
            vec![Waypoint {
                at: [5.0, 0.0, z],
                kind: Kind::Walk,
            }],
            0,
        )
    };
    // Under a waypoint 3 up: fell off; plan again at once.
    assert!(up_there(3.0).steer([4.5, 0.0, 0.0], 1).replan);
    assert!(up_there(3.0).steer([5.0, 0.0, 0.0], 1).replan);
    // Up a ramp: 1.5 higher 5 away is a climb, not a fall.
    let s = up_there(1.5).steer([0.0, 0.0, 0.0], 1);
    assert!(!s.replan && s.dir == [1.0, 0.0], "{s:?}");
    // The goal it was asked for is remembered over the path's end.
    let f = up_there(0.0).toward([5.0, 0.0, 2.0]);
    assert_eq!(f.goal(), Some([5.0, 0.0, 2.0]));
    assert_eq!(up_there(0.0).goal(), Some([5.0, 0.0, 0.0]));
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

/// The generator's output on a small scene (a seam, a low ledge, a high
/// ledge, a narrow gap and a wide one), as counts, links and coordinates
/// to the millimetre. When this changes, the maps' `.bots` files built
/// before are wrong: bump `cache::GENERATOR` (so every map is built again,
/// docs/game/bots.md §10 stage 2) and record the new pair here.
#[test]
fn generator_output_is_pinned() {
    let bsp = floors_bsp(&[
        (0.0, 0.0, 2.0, 2.0, 0.0),
        (2.0, 0.0, 4.0, 2.0, 0.25),
        (-2.0, 0.0, 0.0, 2.0, 0.05),
        (4.0, 0.0, 6.0, 2.0, 1.2),
        (6.6, 0.0, 8.0, 2.0, 1.2),
        (9.5, 0.0, 11.0, 2.0, 1.2),
    ]);
    let d = crate::cache::build(&bsp).unwrap();
    let mut s = Vec::new();
    let mm = |v: &[f32; 3], s: &mut Vec<u8>| {
        for f in v {
            s.extend_from_slice(&((f * 1000.0).round() as i64).to_le_bytes());
        }
    };
    s.extend_from_slice(&(d.nav.polys.len() as u64).to_le_bytes());
    for p in &d.nav.polys {
        s.extend_from_slice(&(p.verts.len() as u64).to_le_bytes());
        p.verts.iter().for_each(|v| mm(v, &mut s));
    }
    for e in &d.nav.edges {
        s.extend_from_slice(&e.from.to_le_bytes());
        s.extend_from_slice(&e.to.to_le_bytes());
        s.push(e.kind as u8);
        mm(&e.a, &mut s);
        mm(&e.b, &mut s);
    }
    s.extend_from_slice(&(d.walls.tris.len() as u64).to_le_bytes());
    d.walls.tris.iter().flatten().for_each(|v| mm(v, &mut s));
    let got = (crate::cache::GENERATOR, crate::autonav::fingerprint(&s));
    assert_eq!(
        got,
        (2, 0x96880c12653c7760),
        "the nav output changed: bump cache::GENERATOR and record the new pair"
    );
}

/// Four 2x2 squares in a ring around (2, 2): 0 at the origin, 1 east of
/// it, 2 north of it, 3 north-east; walkable both ways across each shared
/// side.
fn ring() -> NavMesh {
    let sq = |x: f32, y: f32| {
        Poly::new(vec![
            [x, y, 0.0],
            [x + 2.0, y, 0.0],
            [x + 2.0, y + 2.0, 0.0],
            [x, y + 2.0, 0.0],
        ])
    };
    let mut edges = Vec::new();
    let mut both = |from: u32, to: u32, a: V3, b: V3| {
        for (f, t) in [(from, to), (to, from)] {
            edges.push(Edge {
                from: f,
                to: t,
                a,
                b,
                kind: Kind::Walk,
            });
        }
    };
    both(0, 1, [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]);
    both(0, 2, [0.0, 2.0, 0.0], [2.0, 2.0, 0.0]);
    both(1, 3, [2.0, 2.0, 0.0], [4.0, 2.0, 0.0]);
    both(2, 3, [2.0, 2.0, 0.0], [2.0, 4.0, 0.0]);
    NavMesh::new(
        vec![sq(0.0, 0.0), sq(2.0, 0.0), sq(0.0, 2.0), sq(2.0, 2.0)],
        edges,
    )
}

#[test]
fn a_corridor_can_avoid_an_edge() {
    let m = ring();
    let via = |c: &[u32]| m.edges[c[0] as usize].to;
    let any = m.corridor(0, 3).unwrap();
    assert_eq!(any.len(), 2);
    // Ruling out the way through 1 goes through 2, and the other way.
    let not_1 = m.corridor_avoiding(0, 3, &|e| e.to == 1).unwrap();
    assert_eq!(via(&not_1), 2);
    let not_2 = m.corridor_avoiding(0, 3, &|e| e.to == 2).unwrap();
    assert_eq!(via(&not_2), 1);
    // Both ruled out: no way.
    assert_eq!(m.corridor_avoiding(0, 3, &|e| e.to == 1 || e.to == 2), None);
    let p = m
        .path_avoiding([0.5, 0.5, 0.0], [3.5, 3.5, 0.0], &|e| e.to == 1)
        .unwrap();
    assert_eq!(p.last().unwrap().at, [3.5, 3.5, 0.0]);
    assert!(
        p.iter()
            .all(|w| w.at[0] <= 2.0 + 1e-4 || w.at[1] >= 2.0 - 1e-4),
        "{p:?}"
    );
}

#[test]
fn the_follower_previews_a_turn_and_checks_the_foot_line() {
    let walk = |at| Waypoint {
        at,
        kind: Kind::Walk,
    };
    let path = vec![walk([2.0, 0.0, 0.0]), walk([2.0, 2.0, 0.0])];
    let mut follower = Follower::new(path, 0);
    let at = [1.6, 0.0, 0.0];
    assert_eq!(follower.corner(at), Some([2.0, 2.0, 0.0]));
    assert!(follower.pace(at) < follower.pace([0.0; 3]));
    let blocked = follower.steer_with_clearance(at, 1, |d| {
        assert!(d[1] > 0.1, "{d:?}");
        false
    });
    assert_eq!(blocked.dir, [1.0, 0.0]);
    let clear = follower.steer_with_clearance(at, 2, |_| true);
    assert!(clear.dir[1] > 0.1);
    let mut gap = Follower::new(
        vec![
            walk([2.0, 0.0, 0.0]),
            Waypoint {
                at: [3.0, 0.0, 0.0],
                kind: Kind::Jump,
            },
        ],
        0,
    );
    assert_eq!(gap.corner(at), None);
    assert_eq!(gap.pace(at), 1.0);
    assert_eq!(
        gap.steer_with_clearance(at, 1, |_| panic!("no blend into a jump"))
            .dir,
        [1.0, 0.0]
    );
}

fn floor_quad(x: f32, y: f32, z: f32, w: f32, h: f32) -> Poly {
    Poly::new(vec![
        [x, y, z],
        [x + w, y, z],
        [x + w, y + h, z],
        [x, y + h, z],
    ])
}

#[test]
fn short_traversals_win_over_a_longer_walking_route() {
    let polys = vec![
        floor_quad(0.0, 0.0, 0.0, 2.0, 2.0),
        floor_quad(1.0, -20.0, 0.0, 2.0, 2.0),
        floor_quad(10.0, 0.0, 0.0, 2.0, 2.0),
        floor_quad(25.0, 0.0, 0.0, 2.0, 2.0),
    ];
    for kind in [Kind::Teleporter, Kind::Lift] {
        let mesh = NavMesh::new(
            polys.clone(),
            vec![
                Edge {
                    from: 0,
                    to: 2,
                    a: polys[0].center,
                    b: polys[0].center,
                    kind: Kind::Walk,
                },
                Edge {
                    from: 2,
                    to: 3,
                    a: polys[2].center,
                    b: polys[2].center,
                    kind: Kind::Walk,
                },
                Edge {
                    from: 0,
                    to: 1,
                    a: polys[0].center,
                    b: polys[0].center,
                    kind: Kind::Walk,
                },
                Edge {
                    from: 1,
                    to: 3,
                    a: polys[1].center,
                    b: polys[3].center,
                    kind,
                },
            ],
        );
        if kind == Kind::Teleporter {
            // Walking to the entrance initially moves further from the
            // goal. An unmodified Euclidean heuristic misses the shortcut.
            assert_eq!(mesh.corridor(0, 3).unwrap(), [2, 3]);
        } else {
            let path = mesh.corridor(1, 3).unwrap();
            assert_eq!(path, [3]);
            let mut edges = mesh.edges.clone();
            // The same two corridors with a longer walking route.
            edges[0].a = [0.0, 40.0, 0.0];
            edges[0].b = edges[0].a;
            assert_eq!(
                NavMesh::new(polys.clone(), edges).corridor(0, 3).unwrap(),
                [2, 3]
            );
        }
        let bytes = mesh.to_bytes();
        assert_eq!(NavMesh::from_bytes(&bytes).unwrap().edges, mesh.edges);
    }
}

#[test]
fn high_ledge_builds_a_crouch_jump_and_waits_for_takeoff_before_crouching() {
    let bsp = floors_bsp(&[(0.0, 0.0, 2.0, 2.0, 0.0), (2.0, 0.0, 4.0, 2.0, 0.48)]);
    let m = crate::cache::build(&bsp).unwrap();
    let edge = m
        .nav
        .edges
        .iter()
        .find(|e| e.kind == Kind::CrouchJump)
        .unwrap();
    let path = m.nav.path([1.0, 1.0, 0.0], [3.0, 1.0, 0.48]).unwrap();
    let mut f = Follower::new(path, 0);
    assert!(!f.steer([1.0, 1.0, 0.0], 1).jump);
    let start = f.steer(edge.a, 2);
    assert!(start.jump && !start.crouch);
    let airborne = [edge.a[0] + 0.1, edge.a[1], edge.a[2] + 0.1];
    assert!(!f.steer(airborne, 3).crouch);
    assert!(f.steer(airborne, 5).crouch);
    assert!(!f.steer(edge.b, 6).traversing);
}

#[test]
fn the_jump_sweep_uses_crouch_height_and_rejects_solid_obstructions() {
    let roof = floor_quad(0.42, -1.0, 0.99, 0.25, 2.0);
    let walls = Walls::from_polygons([roof.verts.as_slice()]);
    let a = [0.0; 3];
    let b = [1.0, 0.0, 0.0];
    assert!(!jump_clear(&walls, a, b, false));
    assert!(jump_clear(&walls, a, b, true));
    let wall = [
        [0.5, -1.0, 0.0],
        [0.5, 1.0, 0.0],
        [0.5, 1.0, 2.0],
        [0.5, -1.0, 2.0],
    ];
    let walls = Walls::from_polygons([wall.as_slice()]);
    assert!(!jump_clear(&walls, a, b, true));
}

#[test]
fn overhead_geometry_removes_the_standing_walk_link() {
    let bsp = floors_bsp(&[(0.0, 0.0, 2.0, 2.0, 0.0), (2.0, 0.0, 4.0, 2.0, 0.05)]);
    let pf =
        pathfind::generate::generate(&bsp, 0, 0, &pathfind::generate::Options::default()).unwrap();
    let roof = floor_quad(2.0, 0.0, 0.6, 2.0, 2.0);
    let walls = Walls::from_polygons([roof.verts.as_slice()]);
    let nav = NavMesh::from_collision(&pf, &Build::default(), &walls);
    assert!(nav.path([1.0, 1.0, 0.0], [3.0, 1.0, 0.05]).is_none());
}

#[test]
fn a_vehicle_uses_the_wide_route_around_a_pedestrian_gap() {
    let polys = vec![
        floor_quad(0.0, 0.0, 0.0, 4.0, 4.0),
        floor_quad(4.0, 1.6, 0.0, 2.0, 0.8),
        floor_quad(6.0, 0.0, 0.0, 4.0, 4.0),
        floor_quad(0.0, 4.0, 0.0, 10.0, 4.0),
    ];
    let edge = |from, to, a, b| Edge {
        from,
        to,
        a,
        b,
        kind: Kind::Walk,
    };
    let nav = NavMesh::new(
        polys,
        vec![
            edge(0, 1, [4.0, 1.6, 0.0], [4.0, 2.4, 0.0]),
            edge(1, 2, [6.0, 1.6, 0.0], [6.0, 2.4, 0.0]),
            edge(0, 3, [0.0, 4.0, 0.0], [4.0, 4.0, 0.0]),
            edge(3, 2, [6.0, 4.0, 0.0], [10.0, 4.0, 0.0]),
        ],
    );
    let start = [2.0, 2.0, 0.0];
    let end = [8.0, 2.0, 0.0];
    assert_eq!(nav.corridor(0, 2).unwrap(), [0, 1]);
    let m = crate::cache::MapData {
        nav,
        walls: Walls::default(),
    };
    let spec = crate::vehicle::Class::Jeep.spec();
    let path = m
        .nav
        .vehicle_path(start, end, spec.half_width, spec.height, &m.walls)
        .unwrap();
    assert_eq!(path, [[2.0, 4.0, 0.0], [8.0, 4.0, 0.0], end]);
    assert!(!crate::vehicle::drive::clear(&m, spec, start, end));
    let mut route = crate::vehicle::drive::Route::new(path, end);
    assert_eq!(
        route.carrot_clear(start, |to| crate::vehicle::drive::clear(
            &m, spec, start, to
        )),
        Some([2.0, 4.0, 0.0])
    );
}
