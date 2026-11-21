//! What [`crate::scan`] shares between its two searches
//! (docs/game/superbounce.md, "Spots in the geometry"): polygons with
//! their bounds, the grid that buckets them, and the cap on the output.
//! The bounce search is in `bounce.rs`, the forced-crouch search in
//! `crouch.rs`.
use crate::geom::{Bounds, Grid};
use crate::{Context, CrouchRegion, Poly, Spot, Vec3};

/// Grid cell (world units) for bucketing polygons on the ground plane.
pub(crate) const CELL: f32 = 1.0;
/// Walkable floor: normal z over this.
pub(crate) const WALKABLE: f32 = 0.7;
/// A unit's origin standing on flat floor sits this far above it (−3.19
/// on the −3.2 floor at the Ascension patch).
pub(crate) const ORIGIN_LIFT: f32 = 0.01;
/// Spots of each kind at most, best first.
const MAX_SPOTS: usize = 512;

pub(crate) struct Face<'a> {
    pub poly: &'a Poly,
    pub b: Bounds,
}

pub(crate) fn scan(polys: &[Poly], ctx: &Context) -> Vec<Spot> {
    let faces = faces(polys);
    let mut out = crate::bounce::bounces(&faces);
    let crouches = crate::crouch::crouches(&faces, &out, &rules(&faces, ctx));
    out.extend(best(crouches));
    out
}

/// The forced-crouch regions near `at`, judged (`crate::explain`).
pub(crate) fn explain(polys: &[Poly], ctx: &Context, at: Vec3, r: f32) -> Vec<CrouchRegion> {
    let faces = faces(polys);
    let bounces = crate::bounce::bounces(&faces);
    crate::crouch::explain(&faces, &bounces, &rules(&faces, ctx), at, r)
}

/// What the crouch rules take from the [`Context`].
pub(crate) struct Rules {
    /// The lowest starting location over the scanned geometry's bounds;
    /// `None` turns the in-play rule off.
    pub lowest: Option<Vec3>,
    pub proven: Vec<Vec3>,
}

/// Only starting locations over these polygons count: a scenario's
/// starting locations are for all its BSPs.
fn rules(faces: &[Face], ctx: &Context) -> Rules {
    let (lo, hi) = faces.iter().fold(
        ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
        |(lo, hi), f| {
            (
                [lo[0].min(f.b.min[0]), lo[1].min(f.b.min[1])],
                [hi[0].max(f.b.max[0]), hi[1].max(f.b.max[1])],
            )
        },
    );
    let lowest = ctx
        .spawns
        .iter()
        .filter(|p| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1])
        .copied()
        .min_by(|a, b| a[2].total_cmp(&b[2]));
    Rules {
        lowest,
        proven: ctx.proven.clone(),
    }
}

fn faces(polys: &[Poly]) -> Vec<Face<'_>> {
    polys
        .iter()
        .filter(|p| p.verts.len() >= 3)
        .map(|poly| Face {
            poly,
            b: Bounds::of(&poly.verts),
        })
        .collect()
}

/// The faces `set` names, bucketed by their bounds on the ground plane.
pub(crate) fn grid_of(faces: &[Face], set: &[usize]) -> Grid {
    let mut g = Grid::new(CELL);
    for &i in set {
        let b = faces[i].b;
        g.insert(i as u32, [b.min[0], b.min[1]], [b.max[0], b.max[1]]);
    }
    g
}

/// At most [`MAX_SPOTS`], best score first.
pub(crate) fn best(mut spots: Vec<Spot>) -> Vec<Spot> {
    spots.sort_by(|a, b| b.score.total_cmp(&a.score));
    spots.truncate(MAX_SPOTS);
    spots
}
