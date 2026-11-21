//! Superbounce and forced-crouch spots, found in a map's collision
//! geometry (docs/game/superbounce.md, "Spots in the geometry").
//!
//! A superbounce needs two things: the player's level pair word at -3
//! (a forced crouch, [`Kind::Crouch`] spots), then a landing in a narrow
//! slot between two near-vertical faces whose normals oppose and tilt a
//! little up ([`Kind::Bounce`] spots). The scan is pure: it takes the
//! collision surfaces and returns spots. Reading them out of a `.map`
//! is [`scan_map`].
//!
//! Coordinates are world units, Z up.

mod geom;
mod read;
mod scan;

/// A point in world units.
pub type Vec3 = [f32; 3];

/// The player's capsule radius: the biped's Havok shape (`+0x0c`, 0.175 for
/// `masterchief_mp`; docs/game/superbounce.md, "Looking for state a forced
/// crouch leaves behind").
pub const RADIUS: f32 = 0.175;
/// `masterchief_mp` standing and crouching heights (docs/game/superbounce.md,
/// "The crouch height term"; `crates/live/bots/src/nav.rs` uses the same).
pub const STANDING: f32 = 0.725;
/// See [`STANDING`].
pub const CROUCHED: f32 = 0.5;

/// What a spot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A slot a primed (word -3) landing launches out of.
    Bounce,
    /// A place a standing player is forced into a crouch (ceiling between
    /// the crouched and standing capsule heights), which primes the word.
    Crouch,
}

/// One spot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub kind: Kind,
    /// The line to draw: for a bounce, the slot's floor-level crease; for
    /// a crouch spot, the edge of the low ceiling over walkable floor.
    pub edge: [Vec3; 2],
    /// Where a player stands to use it: on the floor beside the slot, or
    /// under the low ceiling. A unit's origin, standing.
    pub stand: Vec3,
    /// Bounce only: how far (world units) the player must fall onto the
    /// slot for the landing to reach it (0 for a standing jump). Crouch
    /// spots: 0.
    pub fall: f32,
    /// Bounce only: the estimated launch speed straight up (world units a
    /// second) for a primed landing after `fall`. Crouch spots: 0.
    pub launch: f32,
    /// A world direction (unit, X and Y) to drift along when landing on
    /// it, across the slot; zero for none.
    pub drift: [f32; 2],
    /// 0..1: how sure the scan is.
    pub score: f32,
}

/// One collision polygon: its vertices in winding order, and its plane's
/// unit normal (pointing out of the solid).
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    pub verts: Vec<Vec3>,
    pub normal: Vec3,
}

/// Every spot in a set of collision polygons (a map's structure BSP plus
/// its placed instanced geometry, already in world space).
///
/// Bounce spots come first, then crouch spots, each kind best score first
/// and at most 512 of it. A heuristic: the thresholds and the calibration
/// against the one traced slot are in `scan.rs`.
pub fn scan(polys: &[Poly]) -> Vec<Spot> {
    scan::scan(polys)
}

/// The polygons of a decoded collision BSP, moved by `transform`
/// (a row-major 3x4: rotation columns then translation), or as is for
/// `None`.
///
/// One polygon per surface, every surface kept whatever its flags; the
/// normal is the surface's plane, flipped when bit 15 of its plane word is
/// set. Surfaces with a broken ring or a missing plane are left out.
pub fn polys(bsp: &tags::collision::Bsp, transform: Option<[[f32; 4]; 3]>) -> Vec<Poly> {
    read::polys(bsp, transform)
}

/// The world-space collision polygons of one structure BSP of a `.map`
/// (its scenario index): the structure's collision BSP, then each
/// instanced geometry placement's collision BSP moved by its placement.
pub fn map_polys<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    bsp: usize,
) -> Result<Vec<Poly>, tags::Error> {
    read::map_polys(map, bsp)
}

/// The spots on one structure BSP of a `.map` (its scenario index), with
/// its instanced geometry.
pub fn scan_map<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    bsp: usize,
) -> Result<Vec<Spot>, tags::Error> {
    Ok(scan(&map_polys(map, bsp)?))
}

/// The nearest bounce spot to `at` (by distance to its edge) and that
/// distance.
pub fn nearest(spots: &[Spot], at: Vec3) -> Option<(&Spot, f32)> {
    spots
        .iter()
        .filter(|s| s.kind == Kind::Bounce)
        .map(|s| (s, dist_to_edge(s.edge, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

fn dist_to_edge([a, b]: [Vec3; 2], p: Vec3) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

#[cfg(test)]
mod tests;
