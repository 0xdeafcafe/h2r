//! Generate nav on invented levels: a room with a ledge too high to jump,
//! a block-out ramp to it, a teleporter to it and a start in the air.
use super::*;

/// A closed box, solid inside `[lo, hi]`, faces out.
fn cube(lo: V3, hi: V3) -> Vec<Polygon> {
    let c = |i: usize| -> V3 {
        [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    let q = |is: [usize; 4]| Polygon::new(is.iter().map(|&i| c(i)).collect(), 0);
    vec![
        q([0, 4, 6, 2]),
        q([1, 3, 7, 5]),
        q([0, 1, 5, 4]),
        q([2, 6, 7, 3]),
        q([0, 2, 3, 1]),
        q([4, 5, 7, 6]),
    ]
}

/// A room open inside `[lo, hi]`: the cube's faces turned in.
fn room(lo: V3, hi: V3) -> Vec<Polygon> {
    cube(lo, hi)
        .into_iter()
        .map(|mut p| {
            p.points.reverse();
            p
        })
        .collect()
}

/// A prism: `profile` in the x-z plane, counter-clockwise seen from -y,
/// from `y0` to `y1`. Faces out.
fn prism(profile: &[[f32; 2]], y0: f32, y1: f32) -> Vec<Polygon> {
    let at = |p: [f32; 2], y: f32| [p[0], y, p[1]];
    let mut out = vec![
        Polygon::new(profile.iter().map(|&p| at(p, y0)).collect(), 0),
        Polygon::new(profile.iter().rev().map(|&p| at(p, y1)).collect(), 0),
    ];
    for k in 0..profile.len() {
        let (p, q) = (profile[k], profile[(k + 1) % profile.len()]);
        out.push(Polygon::new(vec![at(p, y0), at(p, y1), at(q, y1), at(q, y0)], 0));
    }
    out
}

/// The ledge's top: 1.5 over the floor, past any jump (0.6).
const LEDGE_TOP: f32 = 1.5;
/// A point on the ledge.
const ON_LEDGE: V3 = [5.0, 0.0, LEDGE_TOP];
/// A start on the floor.
const ON_FLOOR: V3 = [-5.0, 0.0, 0.0];

/// A room 16 by 16 with a ledge from x = 2 to 7 standing on its floor
/// (sunk into it, so no face of the ledge lies on the floor's plane).
fn level() -> Bsp {
    let mut level = collision_build::build(&room([-8.0, -8.0, 0.0], [8.0, 8.0, 6.0]))
        .expect("the room compiles")
        .bsp;
    let ledge = collision_build::build(&cube([2.0, -3.0, -0.5], [7.0, 3.0, LEDGE_TOP]))
        .expect("the ledge compiles")
        .bsp;
    merge::merge(&mut level, &ledge).expect("the ledge merges");
    level
}

/// A block-out ramp from the floor at x = -2 up to the ledge's face at
/// x = 2, in model space: its top rises 0.35 a unit (19 degrees) and
/// ends inside the ledge just under its top, so the step at the ledge's
/// lip is 0.1, under the bots' step (0.12).
fn ramp() -> Vec<Polygon> {
    prism(
        &[[-2.0, -0.5], [2.25, -0.5], [2.25, 1.4875], [-2.0, 0.0]],
        -1.0,
        1.0,
    )
}

fn input(solids: Vec<Solid>) -> Input {
    Input {
        level: level(),
        solids,
        starts: vec![ON_FLOOR],
        teleporters: vec![],
    }
}

fn ledge_poly(g: &Generated) -> u32 {
    g.data
        .nav
        .locate(ON_LEDGE, 1.0)
        .expect("the ledge's top is a sector")
}

#[test]
fn without_the_ramp_the_ledge_is_out_of_the_map() {
    let g = generate(&input(vec![])).unwrap();
    assert!(g.problems.is_empty(), "{:?}", g.problems);
    assert_eq!(g.solids, 0);
    let ledge = ledge_poly(&g);
    assert!(!g.data.nav.in_map(ledge), "no start reaches the ledge");
    assert!(g.out_of_map() > 0);
    assert!(
        g.data.nav.path(ON_FLOOR, ON_LEDGE).is_none(),
        "no path climbs the ledge"
    );
}

#[test]
fn a_block_out_ramp_gains_the_sectors_and_link_that_make_the_ledge_reachable() {
    let before = generate(&input(vec![])).unwrap();
    let ramp = Solid::placed("scenery 0 bsp 0", &ramp(), &Frame::IDENTITY);
    let after = generate(&input(vec![ramp])).unwrap();
    assert!(after.problems.is_empty(), "{:?}", after.problems);
    assert_eq!(after.solids, 1);

    // The ramp's slope is new floor: a sector over its middle.
    let on_ramp = [0.0, 0.0, 0.7];
    assert!(before.data.nav.locate(on_ramp, 1.0).is_none());
    let slope = after
        .data
        .nav
        .locate(on_ramp, 1.0)
        .expect("the ramp's slope is a sector");
    assert!(after.sectors() > before.sectors());

    // Something links the slope's top onto the ledge, and the ledge is in.
    let ledge = ledge_poly(&after);
    assert!(after.data.nav.in_map(ledge), "the ramp reaches the ledge");
    let path = after
        .data
        .nav
        .path(ON_FLOOR, ON_LEDGE)
        .expect("a path climbs the ramp");
    assert!(!path.is_empty());
    let corridor = after
        .data
        .nav
        .corridor(slope, ledge)
        .expect("the slope leads to the ledge");
    assert!(corridor.len() >= 2);
}

#[test]
fn a_ramp_placed_by_a_frame_turns_with_it() {
    // A half turn about z takes each point (x, y, z) to (-x, -y, z), in
    // the same order.
    let frame = Frame {
        position: [0.0, 0.0, 0.0],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        scale: 1.0,
    };
    let a = Solid::placed("a", &ramp(), &frame);
    let turned = Frame {
        position: [0.0, 0.0, 0.0],
        axes: [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
        scale: 1.0,
    };
    let b = Solid::placed("b", &ramp(), &turned);
    for (p, q) in a.polygons.iter().zip(&b.polygons) {
        for (u, v) in p.points.iter().zip(&q.points) {
            assert_eq!([-u[0], -u[1], u[2]], *v);
        }
        assert_eq!(p.points.len(), q.points.len());
    }
    // A mirror turns each polygon over so it still faces out.
    let mirror = Frame {
        axes: [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
        ..frame
    };
    let m = Solid::placed("m", &ramp(), &mirror);
    let first = &a.polygons[0].points;
    let mirrored = &m.polygons[0].points;
    assert_eq!(mirrored[0], {
        let p = first[first.len() - 1];
        [p[0], -p[1], p[2]]
    });
}

#[test]
fn a_teleporter_to_the_ledge_links_it_and_brings_it_into_the_map() {
    let mut i = input(vec![]);
    i.teleporters.push(Teleporter {
        channel: 3,
        sender: [-4.0, 4.0, 0.0],
        receiver: ON_LEDGE,
    });
    let g = generate(&i).unwrap();
    assert!(g.problems.is_empty(), "{:?}", g.problems);
    assert_eq!(g.links(Kind::Teleporter), 1);
    assert!(g.data.nav.in_map(ledge_poly(&g)));
    let line = g
        .overlay()
        .into_iter()
        .find(|l| l.kind == LineKind::Link(Kind::Teleporter))
        .expect("the overlay draws the teleporter");
    assert_eq!(line.a[2], OVERLAY_LIFT);
    assert_eq!(line.b[2], LEDGE_TOP + OVERLAY_LIFT);
}

#[test]
fn a_start_off_the_mesh_is_reported() {
    let mut i = input(vec![]);
    // Four units over the floor: in the air.
    i.starts.push([0.0, -5.0, 4.0]);
    let g = generate(&i).unwrap();
    assert_eq!(g.problems.len(), 1, "{:?}", g.problems);
    let p = &g.problems[0];
    assert_eq!(p.kind, ProblemKind::StartOffMesh);
    assert_eq!(p.what, "start 1");
    assert_eq!(p.position, [0.0, -5.0, 4.0]);
    assert_eq!(p.kind.name(), "start_off_mesh");
}

#[test]
fn a_teleporter_end_off_the_mesh_is_reported_and_not_linked() {
    let mut i = input(vec![]);
    i.teleporters.push(Teleporter {
        channel: 1,
        sender: [-4.0, 4.0, 0.0],
        receiver: [0.0, 0.0, 5.0],
    });
    let g = generate(&i).unwrap();
    assert_eq!(g.links(Kind::Teleporter), 0);
    assert_eq!(g.problems.len(), 1, "{:?}", g.problems);
    assert_eq!(g.problems[0].kind, ProblemKind::TeleporterOffMesh);
    assert_eq!(g.problems[0].what, "teleporter 0 receiver");
}

#[test]
fn a_solid_that_does_not_compile_is_left_out_by_name() {
    let broken = Solid {
        name: "crates 4 bsp 0".into(),
        polygons: vec![Polygon::new(vec![[0.0; 3], [0.0; 3], [0.0; 3]], 0)],
    };
    let g = generate(&input(vec![broken])).unwrap();
    assert_eq!(g.solids, 0);
    assert_eq!(g.problems.len(), 1, "{:?}", g.problems);
    assert_eq!(g.problems[0].kind, ProblemKind::SolidLeftOut);
    assert_eq!(g.problems[0].what, "crates 4 bsp 0");
    // The mesh is the level's alone.
    assert_eq!(g.sectors(), generate(&input(vec![])).unwrap().sectors());
}

#[test]
fn the_overlay_draws_every_side_and_the_links_but_walks() {
    let g = generate(&input(vec![])).unwrap();
    let lines = g.overlay();
    let sides: usize = g.data.nav.polys.iter().map(|p| p.verts.len()).sum();
    let links = g
        .data
        .nav
        .edges
        .iter()
        .filter(|e| e.kind != Kind::Walk)
        .count();
    assert_eq!(lines.len(), sides + links);
    // The ledge is out of the map, so some sides are drawn so.
    assert!(lines.iter().any(|l| l.kind == LineKind::OutOfMap));
    assert!(lines.iter().any(|l| l.kind == LineKind::Sector));
    assert_eq!(LineKind::Link(Kind::CrouchJump).name(), "crouch_jump");
}

#[test]
fn a_bsps_surfaces_come_back_as_polygons() {
    let bsp = collision_build::build(&cube([0.0; 3], [1.0; 3]))
        .unwrap()
        .bsp;
    let polygons = Solid::polygons_of(&bsp);
    assert_eq!(polygons.len(), bsp.surfaces.len());
    assert!(polygons.iter().all(|p| p.points.len() >= 3));
    // They compile again into a solid.
    collision_build::build(&polygons).expect("the surfaces compile back");
}
