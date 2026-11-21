//! Superbounce and forced-crouch spots, found in a map's collision
//! geometry (docs/game/superbounce.md, "Spots in the geometry").
//!
//! A superbounce needs two things: the player's level pair word at -3
//! (a forced crouch, [`Kind::Crouch`] spots), then a landing on a narrow
//! strip of floor between two parallel floor edges, at least one a
//! concave crease, that the capsule sinks onto at once ([`Kind::Bounce`]
//! spots). The scan is pure: it takes the
//! collision surfaces and returns spots. Reading them out of a `.map`
//! is [`scan_map`].
//!
//! Coordinates are world units, Z up.

mod bounce;
mod crouch;
mod geom;
pub mod ghost;
mod read;
mod scan;
pub mod verified;
pub mod view;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A strip a primed (word -3) landing launches out of.
    #[default]
    Bounce,
    /// A place where the standing capsule hits the geometry and the
    /// crouched one doesn't: a player there is forced into a crouch, which
    /// primes the word.
    Crouch,
}

/// Whether a game run has shown a spot works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Status {
    /// Found in the geometry, not measured.
    #[default]
    Likely,
    /// Within [`PROVEN_NEAR`] of a measured place ([`PROVEN`]), or shown
    /// to work by a game run ([`verified`]).
    Proven,
    /// Tried by a game run and never worked ([`verified`]): hidden from
    /// players and left alone by the bots.
    Failed,
}

impl Status {
    /// `likely` or `proven`, as `--json` and the agent API write it.
    pub fn name(self) -> &'static str {
        match self {
            Status::Likely => "likely",
            Status::Proven => "proven",
            Status::Failed => "failed",
        }
    }
}

/// A crouch spot's [`Lip`] holds at most this many edges.
pub const MAX_LIP: usize = 8;

/// The edges of the faces the standing capsule hits at a crouch spot's
/// `stand`, nearest first, at their own height: what a player gets under.
/// Fixed-size so a [`Spot`] stays `Copy`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lip {
    edges: [[Vec3; 2]; MAX_LIP],
    len: u8,
}

impl Lip {
    /// The first [`MAX_LIP`] of `edges`.
    pub fn new(edges: &[[Vec3; 2]]) -> Lip {
        let mut lip = Lip::default();
        for (slot, e) in lip.edges.iter_mut().zip(edges) {
            *slot = *e;
        }
        lip.len = edges.len().min(MAX_LIP) as u8;
        lip
    }

    pub fn edges(&self) -> &[[Vec3; 2]] {
        &self.edges[..usize::from(self.len)]
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What shape of floor a bounce spot is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A strip between two parallel floor joints, at least one a concave
    /// crease (the traced Ascension shape). Crouch spots are `Slot` too.
    #[default]
    Slot,
    /// One concave crease between two walkable floors (a flat floor at the
    /// foot of a ramp): a candidate for a game run to try, with a launch
    /// estimate from its own dihedral (docs/game/superbounce.md, "Bounce",
    /// "Single creases").
    Crease,
}

impl Shape {
    /// `slot` or `crease`, as `--json` and the agent API write it.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Slot => "slot",
            Shape::Crease => "crease",
        }
    }
}

/// One spot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub kind: Kind,
    /// The line to draw: for a bounce, the strip's midline at floor
    /// height; for a crouch spot, the side where standing becomes clear.
    pub edge: [Vec3; 2],
    /// Where a player stands to use it: on the strip, a little toward the
    /// crease, or inside the forced-crouch place. A unit's origin.
    pub stand: Vec3,
    /// Bounce only: how far (world units) the player must fall onto the
    /// strip for the landing to reach it (0 for a standing jump). Crouch
    /// spots: 0.
    pub fall: f32,
    /// Bounce only: the estimated launch speed straight up (world units a
    /// second) for a primed landing after `fall`. Crouch spots: 0.
    pub launch: f32,
    /// A world direction (unit, X and Y) to drift along when landing on
    /// it, across the strip from the crease; zero for none.
    pub drift: [f32; 2],
    /// 0..1: how sure the scan is; 1 for a proven crouch spot.
    pub score: f32,
    /// Bounce: the gap between the slot's two joints (world units), the
    /// ribbon's real width. Crouch: the widest the forced band measures
    /// across its edge.
    pub width: f32,
    /// Bounce: the downward speed (world units a second) the landing must
    /// touch down at for the capsule to sink onto both edges; `fall` is
    /// worked from it. Crouch spots: 0.
    pub need: f32,
    /// Crouch: [`Status::Proven`] near a measured priming place, else
    /// [`Status::Likely`]. Bounce spots: `Likely` (the measured bounces
    /// are the bots' list, `crates/live/bots/src/glitches/spots.rs`). A
    /// map's verified file ([`verified::apply`]) makes either kind
    /// `Proven` or `Failed`.
    pub status: Status,
    /// Crouch: the unit direction (X and Y) from `stand` to the standing
    /// room next to it, the way out. Bounce spots: zero.
    pub out: [f32; 2],
    /// Crouch: the edges the standing capsule hits at `stand`. Bounce
    /// spots: empty.
    pub lip: Lip,
    /// Bounce: a two-joint slot or a single crease. Crouch spots: `Slot`.
    pub shape: Shape,
}

/// A measured place where a forced crouch primed the pair word: a unit's
/// origin, crouched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prime {
    /// The map's short name, as its `.map` file and header name it.
    pub map: &'static str,
    pub at: Vec3,
    /// The way out (unit, X and Y) a spot made from this place alone
    /// carries; zero for none.
    pub out: [f32; 2],
    /// What a player must know that the geometry doesn't say.
    pub note: &'static str,
    /// Where it was measured.
    pub source: &'static str,
}

/// Every measured priming place. A crouch region with a sample within
/// [`PROVEN_NEAR`] of one is kept whatever the other rules say, ranks
/// first, and is [`Status::Proven`]. A place no region is near (its lip
/// is an object the scan doesn't read) becomes a proven crouch spot of
/// its own, standing at `at` (docs/game/superbounce.md, "Crouch spots:
/// the stricter rule").
pub static PROVEN: &[Prime] = &[
    Prime {
        map: "ascension",
        // Where the body rested in the corner beside the prop's lip; it
        // stood up 6 cm out, at the band's way out.
        at: [-10.91, 0.72, -1.16],
        out: [0.0, 0.0],
        note: "",
        source: "docs/game/superbounce.md, \"The pair word\"; \
                 docs/game/notes/superbounce-spots.md",
    },
    Prime {
        map: "zanzibar",
        // The window sill (z 5.10) of the base's rear right inner window
        // facing the beach, under the shard of broken glass left of the
        // gap. The glass is an object, not structure: the scan sees no
        // lip here.
        at: [5.22, -26.285, 5.11],
        // The way the jump went, both tries.
        out: [-0.69, -0.72],
        note: "break the glass first; crouch under the shard on its left, \
               let go, turn right and jump: the word turns on the jump",
        source: "docs/game/notes/superbounce-spots.md, \"Zanzibar's window, \
                 played\" (lab job 0043, recording zanzibar-window)",
    },
    Prime {
        map: "zanzibar",
        // Where the body rested crouched, the same run's third place: the
        // mode's phase read `near` on the way in, so the scan has a crouch
        // spot within 1.5 of it.
        at: [8.07, -3.01, 4.51],
        // From the rest to where the word turned, both primed tries.
        out: [0.80, 0.60],
        note: "crouch in, let go, walk out (+y, then +x): the word turns \
               on the walk out, 2 tries of 3",
        source: "docs/game/notes/superbounce-spots.md, \"Zanzibar's window, \
                 played\" (lab job 0043, the same log)",
    },
];

/// A crouch region with a sample this close (world units) to a
/// [`PROVEN`] place is that place.
pub const PROVEN_NEAR: f32 = 0.3;

/// The [`PROVEN`] places on `map` (its short name, any case).
pub fn proven(map: &str) -> Vec<Prime> {
    PROVEN
        .iter()
        .filter(|p| !map.is_empty() && p.map.eq_ignore_ascii_case(map))
        .copied()
        .collect()
}

/// What the crouch rules know of the map beyond its geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// The scenario's player starting locations. Only those over the
    /// scanned polygons' bounds count. None: the in-play rule is off.
    pub spawns: Vec<Vec3>,
    /// Measured priming places ([`proven`]).
    pub proven: Vec<Prime>,
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
/// Bounce spots come first (slots and single creases together, at most
/// 512), then crouch spots (at most 12, by the stricter rules), each kind
/// best score first. With no [`Context`]: no starting locations and no
/// proven places. A heuristic: the thresholds and their calibration
/// against the two measured Ascension spots are in `bounce.rs` and
/// `crouch.rs`.
pub fn scan(polys: &[Poly]) -> Vec<Spot> {
    scan::scan(polys, &Context::default())
}

/// [`scan`] with what the crouch rules know of the map: its starting
/// locations and its measured priming places.
pub fn scan_in(polys: &[Poly], ctx: &Context) -> Vec<Spot> {
    scan::scan(polys, ctx)
}

/// One concave floor joint near a point, as the bounce search judged it
/// for a single-crease candidate (`--explain`).
#[derive(Clone, Debug, PartialEq)]
pub struct CreaseNote {
    /// The joint, at floor height.
    pub edge: [Vec3; 2],
    /// The angle between the two floors, degrees.
    pub dihedral: f32,
    /// The crease spot it makes on its own, before merging along its line.
    pub spot: Option<Spot>,
    /// Why it makes none.
    pub rejected: Option<String>,
}

/// The bounce side of `--explain` near a point: every concave joint
/// judged as a single crease, and the bounce spots (slots and creases, as
/// [`scan`] returns them) whose edge is within the radius.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BounceExplain {
    pub creases: Vec<CreaseNote>,
    pub spots: Vec<Spot>,
}

/// [`BounceExplain`] for the joints and spots within `r` of `at`.
pub fn explain_bounces(polys: &[Poly], at: Vec3, r: f32) -> BounceExplain {
    scan::explain_bounces(polys, at, r)
}

/// One crouch rule's verdict on a region (`--explain`).
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// `in play`, `by a strip`, `spread out`, `proven` or `stand at the
    /// way out`.
    pub rule: &'static str,
    pub pass: bool,
    /// What it measured.
    pub note: String,
}

/// One forced-crouch region as the scan judged it, before and through its
/// filters: for tuning the scan against a real map
/// (`h2 superbounce scan --explain`).
#[derive(Clone, Debug, PartialEq)]
pub struct CrouchRegion {
    /// Its forced samples.
    pub samples: usize,
    /// The bounds of their origins.
    pub min: Vec3,
    pub max: Vec3,
    /// The band measured across its edge at the probed exits: the
    /// sample's origin, the lattice way out, the width.
    pub widths: Vec<(Vec3, [i32; 2], f32)>,
    /// Its extent along its principal axis.
    pub length: f32,
    /// Whether a walk from its exits reaches open floor, and how it ended.
    pub reachable: bool,
    pub reach: String,
    /// Why it isn't a spot; `None` when it is one.
    pub rejected: Option<String>,
    /// The spot it makes when it passes the filters, before suppression.
    pub spot: Option<Spot>,
    /// The stand of the better spot within the spacing that dropped it.
    pub suppressed_by: Option<Vec3>,
    /// Whether it is among the spots [`scan`] returns.
    pub kept: bool,
    /// Each crouch rule's verdict, in order, for a region that made a
    /// spot; empty for one dropped before.
    pub rules: Vec<Verdict>,
    /// `Proven` near a measured priming place.
    pub status: Status,
}

/// Every forced-crouch region with a sample within `r` of `at`, with how
/// the scan's filters judged it.
pub fn explain(polys: &[Poly], at: Vec3, r: f32) -> Vec<CrouchRegion> {
    scan::explain(polys, &Context::default(), at, r)
}

/// [`explain`] with the map's [`Context`].
pub fn explain_in(polys: &[Poly], ctx: &Context, at: Vec3, r: f32) -> Vec<CrouchRegion> {
    scan::explain(polys, ctx, at, r)
}

/// [`explain`] on one structure BSP of a `.map`, with its
/// [`map_context`].
pub fn explain_map<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    bsp: usize,
    at: Vec3,
    r: f32,
) -> Result<Vec<CrouchRegion>, tags::Error> {
    let ctx = map_context(map)?;
    Ok(explain_in(&map_polys(map, bsp)?, &ctx, at, r))
}

/// A `.map`'s [`Context`]: the scenario's player starting locations
/// (`scnr+0x100`) and the [`PROVEN`] places for its header name.
pub fn map_context<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
) -> Result<Context, tags::Error> {
    let header = map.header();
    let short = header
        .scenario_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    let mut places = proven(&header.name);
    if places.is_empty() {
        places = proven(&short);
    }
    Ok(Context {
        spawns: read::spawns(map)?,
        proven: places,
    })
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
/// its instanced geometry and its [`map_context`].
pub fn scan_map<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    bsp: usize,
) -> Result<Vec<Spot>, tags::Error> {
    let ctx = map_context(map)?;
    Ok(scan_in(&map_polys(map, bsp)?, &ctx))
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
