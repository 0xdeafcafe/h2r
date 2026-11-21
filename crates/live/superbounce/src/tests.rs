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

fn poly(verts: &[Vec3], normal: Vec3) -> Poly {
    let l = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    Poly {
        verts: verts.to_vec(),
        normal: normal.map(|v| v / l),
    }
}

fn bounces(spots: &[Spot]) -> Vec<&Spot> {
    spots.iter().filter(|s| s.kind == Kind::Bounce).collect()
}
fn crouches(spots: &[Spot]) -> Vec<&Spot> {
    spots.iter().filter(|s| s.kind == Kind::Crouch).collect()
}
fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Ascension's ramp: normal `(0, 0.5433, 0.8396)`, z falls 0.647 a unit of
/// y.
const RAMP: Vec3 = [0.0, 0.5433, 0.8396];
const RAMP_SLOPE: f32 = 0.5433 / 0.8396;

/// The bounce patch on Ascension, from `h2 superbounce scan ascension
/// --near -10.62,4.0,-3.4` (docs/game/superbounce.md, "The geometry"):
/// floor at z −3.2 in two coplanar pieces meeting at a seam along y 4.0,
/// the ramp meeting the floor's strip in a concave crease along y 3.8,
/// and the slab's undersides, the face down from y 3.8 and a side wall
/// below and beside it.
fn ascension_patch(strip: f32) -> Vec<Poly> {
    let (x0, x1) = (-11.10, -9.98);
    let seam = 3.8 + strip;
    vec![
        flat([x0, x1], [3.8, seam], -3.2, true),
        flat([x0, x1], [seam, 5.1], -3.2, true),
        quad(
            [
                [x0, 0.4, -1.0],
                [x1, 0.4, -1.0],
                [x1, 3.8, -3.2],
                [x0, 3.8, -3.2],
            ],
            RAMP,
        ),
        flat([x0, x1], [3.8, 5.1], -3.5, false),
        quad(
            [
                [x0, 0.4, -1.36],
                [x1, 0.4, -1.36],
                [x1, 3.8, -3.56],
                [x0, 3.8, -3.56],
            ],
            [0.0, -RAMP[1], -RAMP[2]],
        ),
        quad(
            [
                [x0, 3.8, -3.5],
                [x1, 3.8, -3.5],
                [x1, 3.8, -4.0],
                [x0, 3.8, -4.0],
            ],
            [0.0, -1.0, 0.0],
        ),
        quad(
            [
                [x0, 3.8, -3.2],
                [x0, 5.1, -3.2],
                [x0, 5.1, -2.0],
                [x0, 3.8, -2.0],
            ],
            [1.0, 0.0, 0.0],
        ),
    ]
}

#[test]
fn the_ascension_crease_and_seam_are_the_measured_bounce_spot() {
    let spots = scan(&ascension_patch(0.2));
    let b = bounces(&spots);
    assert_eq!(b.len(), 1, "{spots:?}");
    let s = b[0];
    // crates/live/bots/src/glitches/spots.rs: the measured spot.
    assert!(dist(s.stand, [-10.62, 3.88, -3.19]) < 0.1, "{:?}", s.stand);
    // One-point calibration: this crease is the 17.85.
    assert!((s.launch - 17.85).abs() < 0.2, "launch {}", s.launch);
    // From the crease toward the seam: +y, the way every traced bounce
    // came in.
    assert!(
        s.drift[0].abs() < 1e-3 && (s.drift[1] - 1.0).abs() < 1e-3,
        "{:?}",
        s.drift
    );
    // The midline of the strip, at floor height, over its length.
    for p in s.edge {
        assert!(
            (p[1] - 3.9).abs() < 1e-3 && (p[2] + 3.2).abs() < 1e-3,
            "{:?}",
            s.edge
        );
    }
    assert!(
        ((s.edge[0][0] - s.edge[1][0]).abs() - 1.12).abs() < 1e-3,
        "{:?}",
        s.edge
    );
    // Half the landings sink onto both edges and 4 cm more at about 4.3
    // units/s: about a unit of extra fall over a standing jump.
    assert!(s.fall > 0.5 && s.fall < 1.5, "fall {}", s.fall);
    assert!(s.score > 0.9 && s.score <= 1.0 + 1e-5, "{}", s.score);
}

#[test]
fn a_strip_wider_than_the_capsule_is_no_bounce() {
    assert!(bounces(&scan(&ascension_patch(0.5))).is_empty());
}

#[test]
fn two_creases_are_a_v_groove_with_both_tilts() {
    let polys = vec![
        flat([0.0, 2.0], [0.0, 0.2], 0.0, true),
        // Rising toward −y from y 0, and toward +y from y 0.2.
        quad(
            [
                [0.0, -1.0, RAMP_SLOPE],
                [2.0, -1.0, RAMP_SLOPE],
                [2.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            RAMP,
        ),
        quad(
            [
                [0.0, 0.2, 0.0],
                [2.0, 0.2, 0.0],
                [2.0, 1.2, RAMP_SLOPE],
                [0.0, 1.2, RAMP_SLOPE],
            ],
            [0.0, -RAMP[1], RAMP[2]],
        ),
    ];
    let spots = scan(&polys);
    let b = bounces(&spots);
    assert_eq!(b.len(), 1, "{spots:?}");
    assert!(
        (b[0].launch - 2.0 * 17.85).abs() < 0.5,
        "launch {}",
        b[0].launch
    );
    assert!((b[0].drift[1].abs() - 1.0).abs() < 1e-3, "{:?}", b[0].drift);
    assert!(
        b[0].score < 0.9,
        "a V-groove isn't the traced shape: {}",
        b[0].score
    );
}

#[test]
fn a_seam_on_a_ramp_beside_its_foot_is_a_bounce_spot() {
    let up = [0.0, -RAMP[1], RAMP[2]];
    let z = |y: f32| y * RAMP_SLOPE;
    let polys = vec![
        flat([0.0, 2.0], [-1.0, 0.0], 0.0, true),
        quad(
            [
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 0.15, z(0.15)],
                [0.0, 0.15, z(0.15)],
            ],
            up,
        ),
        quad(
            [
                [0.0, 0.15, z(0.15)],
                [2.0, 0.15, z(0.15)],
                [2.0, 2.0, z(2.0)],
                [0.0, 2.0, z(2.0)],
            ],
            up,
        ),
    ];
    let spots = scan(&polys);
    let b = bounces(&spots);
    assert_eq!(b.len(), 1, "{spots:?}");
    assert!((b[0].launch - 17.85).abs() < 0.2, "launch {}", b[0].launch);
    assert!((b[0].drift[1] - 1.0).abs() < 1e-3, "{:?}", b[0].drift);
    // The midline is on the ramp, halfway up the strip.
    assert!((b[0].edge[0][1] - 0.075).abs() < 1e-3, "{:?}", b[0].edge);
    assert!((b[0].edge[0][2] - z(0.075)).abs() < 1e-3, "{:?}", b[0].edge);
}

#[test]
fn seams_alone_are_no_bounce() {
    let polys = vec![
        flat([0.0, 2.0], [-1.0, 0.0], 0.0, true),
        flat([0.0, 2.0], [0.0, 0.2], 0.0, true),
        flat([0.0, 2.0], [0.2, 1.0], 0.0, true),
    ];
    assert!(bounces(&scan(&polys)).is_empty());
}

#[test]
fn a_convex_edge_beside_a_seam_is_no_bounce() {
    let polys = vec![
        // Falling away toward −y from y 0.
        quad(
            [
                [0.0, -1.0, -RAMP_SLOPE],
                [2.0, -1.0, -RAMP_SLOPE],
                [2.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            [0.0, -RAMP[1], RAMP[2]],
        ),
        flat([0.0, 2.0], [0.0, 0.2], 0.0, true),
        flat([0.0, 2.0], [0.2, 1.0], 0.0, true),
    ];
    assert!(bounces(&scan(&polys)).is_empty());
}

/// The forced-crouch corner on Ascension, from `--near -10.91,0.72,-1.16`:
/// floor flat at z −1.0 to y 0.4, then the ramp; a prop box x −11.4..−11.0,
/// y 0.11..0.61, z −1.31..−0.25 whose upper part overhangs its lower by
/// 0.05 in y, with a 45° lip between (the faces toward the corner only).
fn ascension_corner() -> Vec<Poly> {
    let (x0, x1) = (-12.0, -10.0);
    let ramp_z = -1.0 - 3.4 * RAMP_SLOPE;
    let lip = [0.0, 0.7071, -0.7071];
    vec![
        flat([x0, x1], [-0.5, 0.4], -1.0, true),
        quad(
            [
                [x0, 0.4, -1.0],
                [x1, 0.4, -1.0],
                [x1, 3.8, ramp_z],
                [x0, 3.8, ramp_z],
            ],
            RAMP,
        ),
        // +x, the lower part and the overhanging upper part.
        poly(
            &[
                [-11.0, 0.11, -1.31],
                [-11.0, 0.56, -1.31],
                [-11.0, 0.56, -0.73],
                [-11.0, 0.11, -0.73],
            ],
            [1.0, 0.0, 0.0],
        ),
        poly(
            &[
                [-11.0, 0.11, -0.73],
                [-11.0, 0.56, -0.73],
                [-11.0, 0.61, -0.68],
                [-11.0, 0.61, -0.25],
                [-11.0, 0.11, -0.25],
            ],
            [1.0, 0.0, 0.0],
        ),
        // +y: lower at 0.56, the lip, upper at 0.61.
        quad(
            [
                [-11.4, 0.56, -1.31],
                [-11.0, 0.56, -1.31],
                [-11.0, 0.56, -0.73],
                [-11.4, 0.56, -0.73],
            ],
            [0.0, 1.0, 0.0],
        ),
        quad(
            [
                [-11.4, 0.56, -0.73],
                [-11.0, 0.56, -0.73],
                [-11.0, 0.61, -0.68],
                [-11.4, 0.61, -0.68],
            ],
            lip,
        ),
        quad(
            [
                [-11.4, 0.61, -0.68],
                [-11.0, 0.61, -0.68],
                [-11.0, 0.61, -0.25],
                [-11.4, 0.61, -0.25],
            ],
            [0.0, 1.0, 0.0],
        ),
    ]
}

/// The corner and the bounce patch together, as on the map: the corner's
/// ramp is the one that comes down to the patch's crease (the patch's own
/// copy of it left out).
fn ascension_corner_and_patch() -> Vec<Poly> {
    let mut polys = ascension_corner();
    let mut patch = ascension_patch(0.2);
    patch.remove(2);
    polys.extend(patch);
    polys
}

#[test]
fn the_ascension_prop_lip_is_the_measured_crouch_spot() {
    // The lip's band runs the 0.4 front of the prop and round both ends;
    // its stand is the end nearest the bounce strip 3 units away, where
    // the player stood.
    let spots = scan(&ascension_corner_and_patch());
    let c = crouches(&spots);
    let target = [-10.91, 0.72, -1.16];
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(dist(c[0].stand, target) < 0.15, "{:?}", c[0].stand);
    assert_eq!((c[0].fall, c[0].launch, c[0].drift), (0.0, 0.0, [0.0, 0.0]));
    // Scored up for the bounce strip nearby.
    assert!(c[0].score > 0.7, "{}", c[0].score);
    assert_eq!(bounces(&spots).len(), 1, "{spots:?}");
}

#[test]
fn the_ascension_lip_alone_is_still_a_crouch_spot() {
    // With no bounce strip near, it stands mid-band.
    let spots = scan(&ascension_corner());
    let c = crouches(&spots);
    assert_eq!(c.len(), 1, "{c:?}");
    let [x, y, _] = c[0].stand;
    assert!(
        (-11.6..=-10.8).contains(&x) && (0.6..=0.8).contains(&y),
        "{:?}",
        c[0].stand
    );
    // No bounce strip near: 0.4, plus about 0.07 for a 5 cm band and 0.07
    // for its length, well under the 0.3 a strip nearby adds.
    assert!(c[0].score < 0.7, "{}", c[0].score);
    assert!(bounces(&spots).is_empty(), "{spots:?}");
}

/// The corner as it is on the map, from every surface `--near` printed
/// there: the floor flat at z −1.0 to y 0.4, then the ramp, both from the
/// wall at x −11.10 to the ramp's side at x −9.98; the wall, faces at
/// x −11.10 looking +x and x −11.30 looking −x; the prop box set into it
/// with its lip; the ramp's side face; and the bounce patch below.
fn ascension_corner_as_on_the_map() -> Vec<Poly> {
    let (x0, x1) = (-11.10, -9.98);
    let ramp_z = -1.0 - 3.4 * RAMP_SLOPE;
    let mut polys = vec![
        flat([x0, x1], [-0.5, 0.4], -1.0, true),
        quad(
            [
                [x0, 0.4, -1.0],
                [x1, 0.4, -1.0],
                [x1, 3.8, ramp_z],
                [x0, 3.8, ramp_z],
            ],
            RAMP,
        ),
        quad(
            [
                [x0, -0.5, -3.6],
                [x0, 3.8, -3.6],
                [x0, 3.8, 0.5],
                [x0, -0.5, 0.5],
            ],
            [1.0, 0.0, 0.0],
        ),
        quad(
            [
                [-11.30, -0.5, -3.6],
                [-11.30, -0.5, 0.5],
                [-11.30, 3.8, 0.5],
                [-11.30, 3.8, -3.6],
            ],
            [-1.0, 0.0, 0.0],
        ),
        // The ramp's side, 0.36 deep under its edge at x −9.98.
        quad(
            [
                [x1, 0.4, -1.0],
                [x1, 3.8, ramp_z],
                [x1, 3.8, ramp_z - 0.36],
                [x1, 0.4, -1.36],
            ],
            [1.0, 0.0, 0.0],
        ),
    ];
    // The prop's faces (as in `ascension_corner`, past its floor and ramp)
    // and its top.
    polys.extend(ascension_corner().into_iter().skip(2));
    polys.push(flat([-11.4, -11.0], [0.11, 0.61], -0.25, true));
    let mut patch = ascension_patch(0.2);
    patch.remove(2);
    polys.extend(patch);
    polys
}

#[test]
fn the_ascension_corner_against_its_wall_is_the_measured_crouch_spot() {
    // The wall 0.18 from the measured axis leaves only the pocket between
    // it and the lip's corner: two lattice samples, 4 cm long, its band
    // 5 cm across. No length rule may drop it.
    let polys = ascension_corner_as_on_the_map();
    let spots = scan(&polys);
    let c = crouches(&spots);
    let target = [-10.91, 0.72, -1.16];
    assert!(
        c.iter().any(|s| dist(s.stand, target) < 0.15),
        "{:?}",
        explain(&polys, target, 1.0)
    );
    // --explain sees it, kept.
    let near = explain(&polys, target, 0.2);
    assert!(near.iter().any(|r| r.kept), "{near:?}");
    assert_eq!(bounces(&spots).len(), 1, "{spots:?}");
}

#[test]
fn explain_says_why_a_region_is_dropped() {
    // The thin band under a 0.4 ledge: a region, with its widths, dropped.
    let polys = room(0.4);
    let regions = explain(&polys, [2.16, 1.0, 0.0], 0.1);
    assert!(!regions.is_empty());
    for r in &regions {
        assert!(!r.kept && r.spot.is_none(), "{r:?}");
        let why = r.rejected.as_deref().unwrap_or("");
        assert!(
            why.starts_with("band too thin") || why.contains("speck"),
            "{why}"
        );
    }
    assert!(regions.iter().any(|r| !r.widths.is_empty() && r.reachable));
}

#[test]
fn a_thin_band_under_a_low_ledge_is_no_spot() {
    // Clearance 0.4: only the crouched capsule's rounded top slips under,
    // 0.157 to 0.17 from the edge, a band 1.3 cm wide.
    assert!(crouches(&scan(&room(0.4))).is_empty());
}

#[test]
fn a_crouch_place_inside_a_prop_is_no_spot() {
    // A closed box 0.9 high, with a shelf at 0.6 inside over half of it:
    // forced under the shelf, but walled in and roofed.
    let (h, s) = (0.9, 0.6);
    let polys = vec![
        flat([-2.0, 3.0], [-2.0, 3.0], 0.0, true),
        quad(
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, h],
                [0.0, 0.0, h],
            ],
            [0.0, -1.0, 0.0],
        ),
        quad(
            [
                [0.0, 1.0, 0.0],
                [0.0, 1.0, h],
                [1.0, 1.0, h],
                [1.0, 1.0, 0.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        quad(
            [
                [0.0, 0.0, 0.0],
                [0.0, 0.0, h],
                [0.0, 1.0, h],
                [0.0, 1.0, 0.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        quad(
            [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, h],
                [1.0, 0.0, h],
            ],
            [1.0, 0.0, 0.0],
        ),
        flat([0.0, 1.0], [0.0, 1.0], h, true),
        flat([0.0, 1.0], [0.0, 0.5], s, false),
    ];
    assert!(
        crouches(&scan(&polys)).is_empty(),
        "{:?}",
        crouches(&scan(&polys))
    );
}

#[test]
fn two_crouch_places_close_together_are_one_spot() {
    let polys = vec![
        flat([-2.0, 4.0], [-2.0, 3.0], 0.0, true),
        flat([0.0, 0.8], [0.0, 0.8], 0.6, false),
        flat([1.4, 2.2], [0.0, 0.8], 0.6, false),
    ];
    let c = crouches(&scan(&polys)).len();
    assert_eq!(c, 1);
}

#[test]
fn a_crouch_spot_near_a_bounce_strip_outranks_a_lone_one() {
    let mut polys = ascension_patch(0.2);
    // Low ceilings over floor 3.5 units from the strip, and 31 units.
    for x in [-7.0, 21.0] {
        polys.push(flat([x - 1.0, x + 1.0], [3.0, 5.0], -3.2, true));
        polys.push(flat([x - 0.4, x + 0.4], [3.6, 4.4], -2.6, false));
    }
    let spots = scan(&polys);
    let c = crouches(&spots);
    assert_eq!(c.len(), 2, "{c:?}");
    // Its stand is the end nearest the strip.
    assert!((c[0].stand[0] + 7.0).abs() < 0.8, "{:?}", c[0].stand);
    assert!((c[1].stand[0] - 21.0).abs() < 0.5, "{:?}", c[1].stand);
    assert!(
        c[0].score > c[1].score + 0.1,
        "{} {}",
        c[0].score,
        c[1].score
    );
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
    // Stand in the middle, under the ceiling.
    assert!(dist(s.stand, [1.0, 1.0, 0.01]) < 0.05, "{:?}", s.stand);
    // The edge runs along one side, just outside the ceiling where the
    // standing capsule still clips it, on the floor.
    let [a, b] = s.edge;
    let off_side = |p: Vec3| {
        [
            p[0].abs(),
            (p[0] - 2.0).abs(),
            p[1].abs(),
            (p[1] - 2.0).abs(),
        ]
        .into_iter()
        .fold(f32::INFINITY, f32::min)
    };
    assert!(off_side(a) < 0.25 && off_side(b) < 0.25, "{:?}", s.edge);
    assert!(a[2].abs() < 1e-3 && b[2].abs() < 1e-3, "{:?}", s.edge);
    assert!(dist(a, b) > 2.0, "{:?}", s.edge);
    // No bounce strip near: 0.4 + 0.2 (wide) + 0.1 (long).
    assert!((s.score - 0.7).abs() < 1e-4, "{}", s.score);
    assert!(bounces(&spots).is_empty());
}

#[test]
fn a_ceiling_with_standing_room_is_no_crouch_spot() {
    assert!(crouches(&scan(&room(0.8))).is_empty());
}

/// A ceiling too low to crouch under forces a crouch only in the thin band
/// outside its edge where the crouched capsule's rounded top slips under:
/// the edge more than `sqrt(r^2 - (r - 0.1)^2)` (0.158) from the axis. The
/// Ascension lip is this case (its clearance is 0.39). Nothing is under it.
#[test]
fn a_ceiling_too_low_to_crouch_under_forces_only_at_its_edge() {
    for s in crouches(&scan(&room(0.4))) {
        let [x, y, _] = s.stand;
        let outside = x < 0.0 || x > 2.0 || y < 0.0 || y > 2.0;
        let gap = (-x).max(x - 2.0).max(-y).max(y - 2.0);
        assert!(outside && gap < RADIUS + 0.01, "{:?}", s.stand);
    }
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
