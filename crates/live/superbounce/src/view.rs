//! The bounce view's geometry (docs/game/superbounce.md, "The bounce
//! view"): what the collision says about the spots from where the player
//! stands, kept apart from the guest.
//!
//! - [`Geometry`] is built once a map from the scan's polygons: a grid of
//!   every polygon for the line-of-sight test ([`LineOfSight`]), and the
//!   floors and their outline for the tracker's map ([`Geometry::local_map`]).
//! - [`SightCache`] keeps one line-of-sight answer a spot and tests a few
//!   spots a tick, round the list.
//! - [`level`], [`pairs`], [`clip_convex`] and [`clip_segment`] are the
//!   small rules the view draws with.
//!
//! Coordinates are world units, Z up.

use std::collections::HashMap;

use crate::geom::{Grid, V2, cross, dot, sub};
use crate::{Kind, Poly, Spot, Vec3};

/// The line-of-sight grid's cell, world units.
pub const SIGHT_CELL: f32 = 4.0;
/// A sight line ignores a hit this close (world units) to either end, so
/// the floor under a spot or the player does not hide it.
pub const SIGHT_END: f32 = 0.05;
/// A polygon whose normal looks up more than this is floor.
pub const FLOOR_NZ: f32 = 0.7;
/// A spot this much higher or lower (world units) than the player's feet
/// is on another floor: above or below.
pub const FLOOR_STEP: f32 = 0.9;
/// A crouch spot pairs with the nearest landing this close (world units,
/// stand to stand).
pub const PAIR_REACH: f32 = 8.0;
/// The tracker's map shows floors from this far below the player's feet
/// to [`MAP_ABOVE`] above (world units).
pub const MAP_BELOW: f32 = 2.5;
pub const MAP_ABOVE: f32 = 1.5;
/// Floor outline endpoints are matched at this grain (world units).
const EDGE_GRAIN: f32 = 0.01;

/// Whether nothing solid stands between two points.
pub trait LineOfSight {
    fn clear(&self, from: Vec3, to: Vec3) -> bool;
}

struct Face {
    verts: Vec<Vec3>,
    normal: Vec3,
}

/// One floor polygon, seen from above.
#[derive(Clone, Debug, PartialEq)]
pub struct Floor {
    /// Its corners in X and Y, in winding order.
    pub verts: Vec<V2>,
    /// Its mean height.
    pub z: f32,
    min: V2,
    max: V2,
}

/// One edge of the floors' outline: a floor edge no other floor shares
/// (the foot of a wall, the lip of a drop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub edge: [Vec3; 2],
}

/// A map's collision, kept for the bounce view.
pub struct Geometry {
    faces: Vec<Face>,
    grid: Grid,
    floors: Vec<Floor>,
    floor_grid: Grid,
    outline: Vec<Outline>,
    outline_grid: Grid,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry::new(&[])
    }
}

impl std::fmt::Debug for Geometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Geometry")
            .field("faces", &self.faces.len())
            .field("floors", &self.floors.len())
            .field("outline", &self.outline.len())
            .finish()
    }
}

fn bounds2(verts: impl Iterator<Item = V2>) -> (V2, V2) {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for v in verts {
        for k in 0..2 {
            min[k] = min[k].min(v[k]);
            max[k] = max[k].max(v[k]);
        }
    }
    (min, max)
}

fn grain(v: Vec3) -> [i64; 3] {
    v.map(|c| (c / EDGE_GRAIN).round() as i64)
}

impl Geometry {
    /// The polygons gridded for the sight test, and their floors and
    /// outline for the map. Polygons with fewer than three corners or a
    /// corner that is not finite are left out.
    pub fn new(polys: &[Poly]) -> Geometry {
        let mut g = Geometry {
            faces: Vec::new(),
            grid: Grid::new(SIGHT_CELL),
            floors: Vec::new(),
            floor_grid: Grid::new(SIGHT_CELL),
            outline: Vec::new(),
            outline_grid: Grid::new(SIGHT_CELL),
        };
        // Each floor edge, by its ends at the grain, either way round:
        // how many floors have it, and the edge itself.
        let mut edges: HashMap<([i64; 3], [i64; 3]), (u32, [Vec3; 2])> = HashMap::new();
        for p in polys {
            if p.verts.len() < 3 || !p.verts.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            let (min, max) = bounds2(p.verts.iter().map(|v| [v[0], v[1]]));
            let i = g.faces.len() as u32;
            g.grid.insert(i, min, max);
            g.faces.push(Face {
                verts: p.verts.clone(),
                normal: p.normal,
            });
            if p.normal[2] < FLOOR_NZ {
                continue;
            }
            let n = p.verts.len();
            let z = p.verts.iter().map(|v| v[2]).sum::<f32>() / n as f32;
            let f = g.floors.len() as u32;
            g.floor_grid.insert(f, min, max);
            g.floors.push(Floor {
                verts: p.verts.iter().map(|v| [v[0], v[1]]).collect(),
                z,
                min,
                max,
            });
            for k in 0..n {
                let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
                let (ka, kb) = (grain(a), grain(b));
                if ka == kb {
                    continue;
                }
                let key = if ka < kb { (ka, kb) } else { (kb, ka) };
                edges.entry(key).or_insert((0, [a, b])).0 += 1;
            }
        }
        let mut lone: Vec<_> = edges
            .into_iter()
            .filter(|(_, (count, _))| *count == 1)
            .collect();
        // The map's order, not the hash's.
        lone.sort_by_key(|a| a.0);
        for (_, (_, edge)) in lone {
            let (min, max) = bounds2(edge.iter().map(|v| [v[0], v[1]]));
            g.outline_grid.insert(g.outline.len() as u32, min, max);
            g.outline.push(Outline { edge });
        }
        g
    }

    /// No polygons.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub fn floors(&self) -> &[Floor] {
        &self.floors
    }

    pub fn outline(&self) -> &[Outline] {
        &self.outline
    }

    /// The floors and outline within `radius` (world units, across the
    /// ground) of `centre`, from [`MAP_BELOW`] under its height to
    /// [`MAP_ABOVE`] over it, relative to `centre`: lowest floor first,
    /// so a higher floor is drawn over a lower one.
    pub fn local_map(&self, centre: Vec3, radius: f32) -> LocalMap {
        let mut out = LocalMap::default();
        if !centre.iter().all(|v| v.is_finite()) || radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return out;
        }
        let min = [centre[0] - radius, centre[1] - radius];
        let max = [centre[0] + radius, centre[1] + radius];
        let band = |z: f32| {
            let rise = z - centre[2];
            (-MAP_BELOW..=MAP_ABOVE).contains(&rise).then_some(rise)
        };
        let near = |lo: V2, hi: V2| {
            // The box's nearest point to the centre is within the radius.
            let dx = (lo[0] - centre[0]).max(0.0).max(centre[0] - hi[0]);
            let dy = (lo[1] - centre[1]).max(0.0).max(centre[1] - hi[1]);
            dx * dx + dy * dy <= radius * radius
        };
        let mut found = Vec::new();
        self.floor_grid.query(min, max, &mut found);
        for &i in &found {
            let f = &self.floors[i as usize];
            let Some(rise) = band(f.z) else { continue };
            if !near(f.min, f.max) {
                continue;
            }
            out.floors.push(LocalFloor {
                verts: f
                    .verts
                    .iter()
                    .map(|v| [v[0] - centre[0], v[1] - centre[1]])
                    .collect(),
                rise,
            });
        }
        out.floors.sort_by(|a, b| a.rise.total_cmp(&b.rise));
        self.outline_grid.query(min, max, &mut found);
        for &i in &found {
            let [a, b] = self.outline[i as usize].edge;
            if band((a[2] + b[2]) * 0.5).is_none() {
                continue;
            }
            let (lo, hi) = bounds2([[a[0], a[1]], [b[0], b[1]]].into_iter());
            if !near(lo, hi) {
                continue;
            }
            out.edges.push([
                [a[0] - centre[0], a[1] - centre[1]],
                [b[0] - centre[0], b[1] - centre[1]],
            ]);
        }
        out
    }
}

impl LineOfSight for Geometry {
    /// Whether the segment from `from` to `to` passes through no polygon,
    /// [`SIGHT_END`] clear of each end. Without polygons everything is in
    /// sight.
    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        if !from.iter().chain(&to).all(|v| v.is_finite()) {
            return true;
        }
        let d = sub(to, from);
        let length = dot(d, d).sqrt();
        if length <= 2.0 * SIGHT_END {
            return true;
        }
        let flat = d[0].hypot(d[1]);
        let steps = ((flat / SIGHT_CELL).ceil() as usize).clamp(1, 4096);
        let at = |t: f32| [from[0] + d[0] * t, from[1] + d[1] * t];
        let mut candidates = Vec::new();
        let mut found = Vec::new();
        for s in 0..steps {
            let (a, b) = (
                at(s as f32 / steps as f32),
                at((s + 1) as f32 / steps as f32),
            );
            let min = [a[0].min(b[0]), a[1].min(b[1])];
            let max = [a[0].max(b[0]), a[1].max(b[1])];
            self.grid.query(min, max, &mut found);
            candidates.extend_from_slice(&found);
        }
        candidates.sort_unstable();
        candidates.dedup();
        let end = SIGHT_END / length;
        !candidates.iter().any(|&i| {
            let f = &self.faces[i as usize];
            segment_hits(&f.verts, f.normal, from, d, end)
        })
    }
}

/// Whether the segment `from + t d`, `t` in (`end`, 1 - `end`), crosses
/// the convex polygon `verts` (either winding) in the plane of `normal`.
fn segment_hits(verts: &[Vec3], normal: Vec3, from: Vec3, d: Vec3, end: f32) -> bool {
    let denom = dot(normal, d);
    if denom.abs() < 1e-9 {
        return false;
    }
    let t = dot(normal, sub(verts[0], from)) / denom;
    if !(t > end && t < 1.0 - end) {
        return false;
    }
    let p = [from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t];
    let n = verts.len();
    let (mut pos, mut neg) = (false, false);
    for k in 0..n {
        let (a, b) = (verts[k], verts[(k + 1) % n]);
        let s = dot(cross(sub(b, a), sub(p, a)), normal);
        let tol = 1e-5 * dot(sub(b, a), sub(b, a)).sqrt().max(1.0);
        if s > tol {
            pos = true;
        } else if s < -tol {
            neg = true;
        }
        if pos && neg {
            return false;
        }
    }
    true
}

/// The floors near a point, relative to it ([`Geometry::local_map`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalMap {
    /// Lowest first.
    pub floors: Vec<LocalFloor>,
    /// The outline edges, X and Y from the centre.
    pub edges: Vec<[V2; 2]>,
}

/// A floor of a [`LocalMap`].
#[derive(Clone, Debug, PartialEq)]
pub struct LocalFloor {
    /// X and Y from the centre.
    pub verts: Vec<V2>,
    /// Its height over the centre's.
    pub rise: f32,
}

/// One line-of-sight answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// Something solid is between the eye and the spot.
    pub hidden: bool,
    /// Where the eye was when it was tested.
    pub from: Vec3,
}

/// One line-of-sight answer a spot, tested a few a tick in turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SightCache {
    seen: Vec<Option<Seen>>,
    next: usize,
}

impl SightCache {
    pub const fn new() -> SightCache {
        SightCache {
            seen: Vec::new(),
            next: 0,
        }
    }

    /// Tests up to `budget` of `targets` from `eye`, starting where the
    /// last call stopped and going round. A list of another length starts
    /// the answers again.
    pub fn step(&mut self, eye: Vec3, targets: &[Vec3], budget: usize, sight: &dyn LineOfSight) {
        if self.seen.len() != targets.len() {
            self.seen = vec![None; targets.len()];
            self.next = 0;
        }
        if targets.is_empty() {
            return;
        }
        for _ in 0..budget.min(targets.len()) {
            let i = self.next % targets.len();
            self.seen[i] = Some(Seen {
                hidden: !sight.clear(eye, targets[i]),
                from: eye,
            });
            self.next = (i + 1) % targets.len();
        }
    }

    /// Spot `i`'s answer, if it has been tested.
    pub fn get(&self, i: usize) -> Option<Seen> {
        self.seen.get(i).copied().flatten()
    }

    /// How many spots have an answer.
    pub fn tested(&self) -> usize {
        self.seen.iter().filter(|s| s.is_some()).count()
    }

    pub fn clear(&mut self) {
        *self = SightCache::new();
    }
}

/// Which floor a spot is on, from the player's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Level {
    #[default]
    Same,
    Above,
    Below,
}

impl Level {
    /// `same`, `above` or `below`, as the agent API writes it.
    pub fn name(self) -> &'static str {
        match self {
            Level::Same => "same",
            Level::Above => "above",
            Level::Below => "below",
        }
    }
}

/// A spot standing at height `spot` from a player whose feet are at
/// `player`: more than [`FLOOR_STEP`] apart is another floor.
pub fn level(player: f32, spot: f32) -> Level {
    let rise = spot - player;
    if rise > FLOOR_STEP {
        Level::Above
    } else if rise < -FLOOR_STEP {
        Level::Below
    } else {
        Level::Same
    }
}

/// Each crouch spot with the nearest bounce spot within [`PAIR_REACH`] of
/// it, stand to stand: (crouch index, bounce index), in the crouch spots'
/// order.
pub fn pairs(spots: &[Spot]) -> Vec<(usize, usize)> {
    let d = |a: Vec3, b: Vec3| {
        let v = sub(a, b);
        dot(v, v).sqrt()
    };
    spots
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == Kind::Crouch)
        .filter_map(|(i, c)| {
            spots
                .iter()
                .enumerate()
                .filter(|(_, b)| b.kind == Kind::Bounce)
                .map(|(j, b)| (j, d(c.stand, b.stand)))
                .filter(|&(_, dist)| dist <= PAIR_REACH)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(j, _)| (i, j))
        })
        .collect()
}

/// The part of the convex polygon `poly` inside the convex polygon `clip`
/// (counter-clockwise), by Sutherland and Hodgman. Empty when they do not
/// meet.
pub fn clip_convex(poly: &[V2], clip: &[V2]) -> Vec<V2> {
    let mut out: Vec<V2> = poly.to_vec();
    let m = clip.len();
    for k in 0..m {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[k], clip[(k + 1) % m]);
        let inside = |p: V2| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= 0.0;
        let cut = |p: V2, q: V2| {
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let dp = ex * (p[1] - a[1]) - ey * (p[0] - a[0]);
            let dq = ex * (q[1] - a[1]) - ey * (q[0] - a[0]);
            let t = dp / (dp - dq);
            [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]
        };
        let input = std::mem::take(&mut out);
        let n = input.len();
        for i in 0..n {
            let (p, q) = (input[i], input[(i + 1) % n]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cut(p, q)),
                (false, true) => {
                    out.push(cut(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
    }
    if out.len() < 3 { Vec::new() } else { out }
}

/// The part of the segment `a`-`b` inside the circle of `radius` about the
/// origin, or `None` outside it.
pub fn clip_segment(a: V2, b: V2, radius: f32) -> Option<[V2; 2]> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let qa = d[0] * d[0] + d[1] * d[1];
    let qb = 2.0 * (a[0] * d[0] + a[1] * d[1]);
    let qc = a[0] * a[0] + a[1] * a[1] - radius * radius;
    if qa < 1e-12 {
        return (qc <= 0.0).then_some([a, b]);
    }
    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return None;
    }
    let r = disc.sqrt();
    let t0 = ((-qb - r) / (2.0 * qa)).max(0.0);
    let t1 = ((-qb + r) / (2.0 * qa)).min(1.0);
    (t0 < t1).then(|| {
        let at = |t: f32| [a[0] + d[0] * t, a[1] + d[1] * t];
        [at(t0), at(t1)]
    })
}

/// A regular polygon of `sides` corners on the circle of `radius` about
/// the origin, counter-clockwise from +X: the clip for [`clip_convex`].
pub fn circle(radius: f32, sides: usize) -> Vec<V2> {
    (0..sides)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / sides as f32;
            [radius * a.cos(), radius * a.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(verts: [Vec3; 4], normal: Vec3) -> Poly {
        Poly {
            verts: verts.to_vec(),
            normal,
        }
    }

    /// A room: a floor at 0 from -10 to 10, a wall across x = 0 from y -2
    /// to 2 and z 0 to 3, a ledge at z 1 from x 4 to 6, and a floor far up
    /// at z 10.
    fn patch() -> Vec<Poly> {
        vec![
            // Two floor halves sharing the edge x = 0.
            quad(
                [
                    [-10.0, -10.0, 0.0],
                    [0.0, -10.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [-10.0, 10.0, 0.0],
                ],
                [0.0, 0.0, 1.0],
            ),
            quad(
                [
                    [0.0, -10.0, 0.0],
                    [10.0, -10.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                [0.0, 0.0, 1.0],
            ),
            // The wall.
            quad(
                [
                    [0.0, -2.0, 0.0],
                    [0.0, 2.0, 0.0],
                    [0.0, 2.0, 3.0],
                    [0.0, -2.0, 3.0],
                ],
                [1.0, 0.0, 0.0],
            ),
            // The ledge.
            quad(
                [
                    [4.0, 4.0, 1.0],
                    [6.0, 4.0, 1.0],
                    [6.0, 6.0, 1.0],
                    [4.0, 6.0, 1.0],
                ],
                [0.0, 0.0, 1.0],
            ),
            // Far up.
            quad(
                [
                    [-1.0, -1.0, 10.0],
                    [1.0, -1.0, 10.0],
                    [1.0, 1.0, 10.0],
                    [-1.0, 1.0, 10.0],
                ],
                [0.0, 0.0, 1.0],
            ),
        ]
    }

    #[test]
    fn a_wall_hides_what_is_behind_it_and_nothing_else() {
        let g = Geometry::new(&patch());
        let eye = [-5.0, 0.0, 0.6];
        assert!(!g.clear(eye, [5.0, 0.0, 0.25]), "through the wall");
        assert!(g.clear(eye, [5.0, 5.0, 0.25]), "round the wall's end");
        assert!(g.clear(eye, [-5.0, 8.0, 0.25]), "on this side");
        // Over the wall's top.
        assert!(g.clear([-5.0, 0.0, 3.5], [5.0, 0.0, 3.5]));
        // A spot on the floor itself is not hidden by its own floor.
        assert!(g.clear(eye, [-5.0, 3.0, 0.0]));
        // Seen from under the floor, it is.
        assert!(!g.clear([-5.0, 0.0, -1.0], [-5.0, 3.0, 0.5]));
        // Nothing to test against: in sight.
        assert!(Geometry::new(&[]).clear(eye, [5.0, 0.0, 0.25]));
        // A long line still finds the wall cells away.
        assert!(!g.clear([-9.9, 0.1, 1.0], [9.9, -0.1, 1.0]));
    }

    #[test]
    fn the_floors_outline_leaves_out_the_shared_edge() {
        let g = Geometry::new(&patch());
        // Four floors (the wall is not one).
        assert_eq!(g.floors().len(), 4);
        // Each floor half has three lone edges, the ledge and the far floor
        // four each; x = 0 is shared.
        assert_eq!(g.outline().len(), 3 + 3 + 4 + 4);
        assert!(
            !g.outline()
                .iter()
                .any(|o| o.edge[0][0] == 0.0 && o.edge[1][0] == 0.0 && o.edge[0][2] == 0.0)
        );
    }

    #[test]
    fn the_local_map_keeps_floors_near_the_players_height_lowest_first() {
        let g = Geometry::new(&patch());
        let m = g.local_map([5.0, 5.0, 0.0], 3.0);
        // The ground's right half and the ledge (1 up), not the floor 10 up
        // nor the left half 5 away.
        assert_eq!(m.floors.len(), 2);
        assert_eq!(m.floors[0].rise, 0.0);
        assert_eq!(m.floors[1].rise, 1.0);
        // Relative to the centre.
        assert!(m.floors[1].verts.contains(&[-1.0, -1.0]));
        // The ledge's outline is in; the ground's far edges are not.
        assert_eq!(m.edges.len(), 4);
        // From the ledge: the ground is 1 below, in the band.
        let m = g.local_map([5.0, 5.0, 1.0], 3.0);
        assert_eq!(m.floors.len(), 2);
        assert_eq!(m.floors[0].rise, -1.0);
        // From 2 above the ledge the ground is 3 below: out of the band.
        assert_eq!(g.local_map([5.0, 5.0, 3.0], 3.0).floors.len(), 1);
        // From far below, nothing.
        assert!(g.local_map([5.0, 5.0, -20.0], 3.0).floors.is_empty());
        assert!(g.local_map([5.0, 5.0, 0.0], 0.0).floors.is_empty());
    }

    #[test]
    fn the_sight_cache_tests_a_few_a_step_round_the_list() {
        struct Wall;
        impl LineOfSight for Wall {
            fn clear(&self, _from: Vec3, to: Vec3) -> bool {
                to[0] < 0.0
            }
        }
        let targets = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [-2.0, 0.0, 0.0]];
        let mut c = SightCache::default();
        c.step([0.0; 3], &targets, 2, &Wall);
        assert_eq!(c.tested(), 2);
        assert_eq!(c.get(0).map(|s| s.hidden), Some(false));
        assert_eq!(c.get(1).map(|s| s.hidden), Some(true));
        assert_eq!(c.get(2), None);
        c.step([0.0; 3], &targets, 2, &Wall);
        assert_eq!(c.tested(), 3);
        assert_eq!(c.get(2).map(|s| s.hidden), Some(false));
        // Round again: spot 1 is tested from the new eye.
        c.step([0.0, 1.0, 0.0], &targets, 1, &Wall);
        assert_eq!(c.get(1).unwrap().from, [0.0, 1.0, 0.0]);
        // A budget over the list tests each once.
        let mut c = SightCache::default();
        c.step([0.0; 3], &targets, 10, &Wall);
        assert_eq!(c.tested(), 3);
        // Another list starts again.
        c.step([0.0; 3], &targets[..1], 0, &Wall);
        assert_eq!(c.tested(), 0);
    }

    #[test]
    fn a_floor_step_is_above_or_below() {
        assert_eq!(level(0.0, FLOOR_STEP), Level::Same);
        assert_eq!(level(0.0, FLOOR_STEP + 0.01), Level::Above);
        assert_eq!(level(2.0, 0.0), Level::Below);
        assert_eq!(level(2.0, 1.5), Level::Same);
    }

    #[test]
    fn a_crouch_spot_pairs_with_the_nearest_landing_in_reach() {
        let s = |kind, x: f32| Spot {
            kind,
            stand: [x, 0.0, 0.0],
            ..Spot::default()
        };
        let spots = [
            s(Kind::Bounce, 3.0),
            s(Kind::Crouch, 0.0),
            s(Kind::Bounce, -2.0),
            s(Kind::Crouch, 100.0),
        ];
        assert_eq!(pairs(&spots), [(1, 2)]);
    }

    #[test]
    fn clipping_keeps_what_is_inside() {
        let square = [[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]];
        let clip = circle(1.0, 32);
        let c = clip_convex(&square, &clip);
        // The square holds the circle: the clip is the circle.
        assert!(c.len() >= 32);
        assert!(c.iter().all(|p| (p[0].hypot(p[1]) - 1.0).abs() < 1e-4));
        // A square inside is kept whole.
        let small = [[-0.1, -0.1], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]];
        let kept = clip_convex(&small, &clip);
        assert_eq!(kept.len(), 4);
        assert!(small.iter().all(|p| kept.contains(p)));
        // One outside is gone.
        let far = small.map(|p| [p[0] + 5.0, p[1]]);
        assert!(clip_convex(&far, &clip).is_empty());

        let [a, b] = clip_segment([-2.0, 0.0], [2.0, 0.0], 1.0).unwrap();
        assert!((a[0] + 1.0).abs() < 1e-5 && (b[0] - 1.0).abs() < 1e-5);
        assert_eq!(
            clip_segment([0.0, 0.0], [0.5, 0.0], 1.0),
            Some([[0.0, 0.0], [0.5, 0.0]])
        );
        assert_eq!(clip_segment([2.0, 2.0], [3.0, 2.0], 1.0), None);
    }
}
