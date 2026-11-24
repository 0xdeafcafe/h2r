//! **Generate nav** for a Forge map (docs/game/forge-nav.md;
//! docs/game/forge-design.md, slice F9).
//!
//! The bots walk a nav mesh made from the structure BSP's collision
//! (`bots::cache::build`). A Forge map adds solid objects the BSP does not
//! have: a block-out ramp, a crate wall. [`generate`] merges each solid's
//! collision into a copy of the level's (`collision_build::merge`), builds
//! the bots' mesh over the result, links each teleporter pair, marks what
//! the player starts do not reach, and names what is wrong as
//! [`Problem`]s: a start off the mesh, a teleporter end off it, a solid
//! that did not compile or merge.
//!
//! [`read`] takes the inputs from a built map: the first structure BSP,
//! the starting locations, the teleporter flags and the solid placements
//! (scenery, crates and machines) the caller names.
//!
//! The result is the bots' own [`MapData`], so a map that loads it walks
//! what the overlay showed. The overlay is [`Generated::overlay`]: one
//! line per polygon side and one per link, each with what it is.
use bots::cache::MapData;
use bots::nav::{Edge, Kind, NavMesh, V3};
use collision_build::{Polygon, TWO_SIDED, merge};
use tags::collision::Bsp;

pub mod read;

/// How far up a point may be over the floor it stands on and still be on
/// it, as the bots' starts snap ([`NavMesh::mark_out_of_map`]).
pub const ON_MESH_REACH: f32 = 0.8;
pub const ON_MESH_UP: f32 = 0.5;
pub const ON_MESH_DOWN: f32 = 2.0;

/// A solid object, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    /// What it is, as a problem names it: `scenery 12 (crate_large)`.
    pub name: String,
    /// Closed, convex, planar polygons, counter-clockwise seen from
    /// outside.
    pub polygons: Vec<Polygon>,
}

/// A placement's frame: position, forward, left and up axes, uniform scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub position: V3,
    /// Forward, left, up.
    pub axes: [V3; 3],
    pub scale: f32,
}

impl Frame {
    pub const IDENTITY: Frame = Frame {
        position: [0.0; 3],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        scale: 1.0,
    };

    /// A model-space point in world space.
    pub fn apply(&self, p: V3) -> V3 {
        let [f, l, u] = self.axes;
        let s = self.scale;
        std::array::from_fn(|k| {
            self.position[k] + (f[k] * p[0] + l[k] * p[1] + u[k] * p[2]) * s
        })
    }
}

impl Solid {
    /// `polygons` in model space, placed at `frame`. A frame whose axes
    /// mirror (a negative determinant) turns each polygon over so it still
    /// faces out.
    pub fn placed(name: impl Into<String>, polygons: &[Polygon], frame: &Frame) -> Self {
        let [f, l, u] = frame.axes;
        let det = f[0] * (l[1] * u[2] - l[2] * u[1]) - f[1] * (l[0] * u[2] - l[2] * u[0])
            + f[2] * (l[0] * u[1] - l[1] * u[0]);
        let flip = det * frame.scale.signum() < 0.0;
        let polygons = polygons
            .iter()
            .map(|p| {
                let mut out = p.clone();
                out.points = p.points.iter().map(|&q| frame.apply(q)).collect();
                if flip {
                    out.points.reverse();
                }
                out
            })
            .collect();
        Self {
            name: name.into(),
            polygons,
        }
    }

    /// Every surface of a collision BSP as a polygon, in the BSP's space:
    /// a `coll` permutation's BSP, to place with [`Solid::placed`].
    pub fn polygons_of(bsp: &Bsp) -> Vec<Polygon> {
        (0..bsp.surfaces.len())
            .filter_map(|s| {
                let points: Vec<V3> = bsp
                    .surface_ring(s)
                    .into_iter()
                    .filter_map(|v| bsp.vertices.get(v).map(|v| v.point))
                    .collect();
                (points.len() >= 3).then(|| {
                    let p = Polygon::new(points, 0);
                    if bsp.surfaces[s].flags & TWO_SIDED != 0 {
                        p.two_sided()
                    } else {
                        p
                    }
                })
            })
            .collect()
    }
}

/// A teleporter pair: a sender flag and the receiver it goes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Teleporter {
    pub channel: i64,
    pub sender: V3,
    pub receiver: V3,
}

/// What the generator works from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    /// The level's collision: the structure BSP's.
    pub level: Bsp,
    pub solids: Vec<Solid>,
    /// The player starts: the mesh is in the map where they reach.
    pub starts: Vec<V3>,
    pub teleporters: Vec<Teleporter>,
}

/// What a problem is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProblemKind {
    /// A player start with no floor of the mesh under it.
    StartOffMesh,
    /// A teleporter's sender or receiver with no floor under it: no bot
    /// takes it.
    TeleporterOffMesh,
    /// A solid whose collision did not compile or merge: the mesh is made
    /// without it.
    SolidLeftOut,
}

impl ProblemKind {
    /// The name `forge.nav` gives it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::StartOffMesh => "start_off_mesh",
            Self::TeleporterOffMesh => "teleporter_off_mesh",
            Self::SolidLeftOut => "solid_left_out",
        }
    }
}

/// Something the generated nav cannot do, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: ProblemKind,
    /// What it is about: `start 3`, `teleporter 2 receiver`, a solid's
    /// name.
    pub what: String,
    pub position: V3,
    pub reason: String,
}

/// What one overlay line shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// A side of a polygon the starts reach.
    Sector,
    /// A side of a polygon out of the map: no start reaches it.
    OutOfMap,
    /// A link that is not a walk between neighbours: a step, drop, jump,
    /// crouch jump, teleporter, lift or elevator, from where it is taken
    /// to where it lands.
    Link(Kind),
}

impl LineKind {
    /// The name `forge.nav` gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sector => "sector",
            Self::OutOfMap => "out_of_map",
            Self::Link(kind) => link_name(kind),
        }
    }
}

/// A link kind as `forge.nav` names it.
pub fn link_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Walk => "walk",
        Kind::Drop => "drop",
        Kind::Jump => "jump",
        Kind::CrouchJump => "crouch_jump",
        Kind::Teleporter => "teleporter",
        Kind::Lift => "lift",
        Kind::Elevator => "elevator",
    }
}

/// One line of the overlay, lifted a little off the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub kind: LineKind,
    pub a: V3,
    pub b: V3,
}

/// How far the overlay's lines sit over the floor, in world units.
pub const OVERLAY_LIFT: f32 = 0.03;

/// The generated nav.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The bots' data for the level with the solids in.
    pub data: MapData,
    /// The solids merged in.
    pub solids: usize,
    pub problems: Vec<Problem>,
}

impl Generated {
    /// How many polygons (the sectors) the mesh has.
    pub fn sectors(&self) -> usize {
        self.data.nav.polys.len()
    }

    /// How many links of `kind` the mesh has.
    pub fn links(&self, kind: Kind) -> usize {
        self.data.nav.edges.iter().filter(|e| e.kind == kind).count()
    }

    /// How many polygons no start reaches.
    pub fn out_of_map(&self) -> usize {
        self.data.nav.out_of_map_count()
    }

    /// The overlay: each polygon's sides, then each link but a walk, in
    /// mesh order.
    pub fn overlay(&self) -> Vec<Line> {
        let nav = &self.data.nav;
        let lift = |p: V3| [p[0], p[1], p[2] + OVERLAY_LIFT];
        let mut out = Vec::new();
        for (i, poly) in nav.polys.iter().enumerate() {
            let kind = if nav.in_map(i as u32) {
                LineKind::Sector
            } else {
                LineKind::OutOfMap
            };
            let n = poly.verts.len();
            for k in 0..n {
                out.push(Line {
                    kind,
                    a: lift(poly.verts[k]),
                    b: lift(poly.verts[(k + 1) % n]),
                });
            }
        }
        for e in nav.edges.iter().filter(|e| e.kind != Kind::Walk) {
            let (a, b) = if e.kind.traversal() {
                (e.a, e.b)
            } else {
                (mid(e.a, e.b), nav.polys[e.to as usize].center)
            };
            out.push(Line {
                kind: LineKind::Link(e.kind),
                a: lift(a),
                b: lift(b),
            });
        }
        out
    }
}

fn mid(a: V3, b: V3) -> V3 {
    std::array::from_fn(|k| (a[k] + b[k]) * 0.5)
}

/// The polygon a point stands on, as the bots' starts snap.
fn on_mesh(nav: &NavMesh, p: V3) -> Option<u32> {
    nav.locate(p, 1.0).or_else(|| {
        nav.nearest(p, ON_MESH_REACH, ON_MESH_UP, ON_MESH_DOWN)
            .map(|(i, _)| i)
    })
}

/// Merge each solid into the level, build the bots' mesh over it, link the
/// teleporters and mark what the starts reach. A solid that does not
/// compile or merge is left out and named; the level failing to make a
/// mesh at all is the error.
pub fn generate(input: &Input) -> Result<Generated, String> {
    let mut level = input.level.clone();
    let mut problems = Vec::new();
    let mut merged = 0;
    for solid in &input.solids {
        let at = solid
            .polygons
            .first()
            .and_then(|p| p.points.first())
            .copied()
            .unwrap_or_default();
        let left_out = |reason: String| Problem {
            kind: ProblemKind::SolidLeftOut,
            what: solid.name.clone(),
            position: at,
            reason,
        };
        let piece = match collision_build::build(&solid.polygons) {
            Ok(piece) => piece,
            Err(e) => {
                problems.push(left_out(format!("its collision does not compile: {e}")));
                continue;
            }
        };
        // A failed merge leaves the level as it was: merge into a copy.
        let mut next = level.clone();
        match merge::merge(&mut next, &piece.bsp) {
            Ok(_) => {
                level = next;
                merged += 1;
            }
            Err(e) => problems.push(left_out(format!("it does not merge into the level: {e}"))),
        }
    }

    let mut data = bots::cache::build(&level)?;
    let mut edges = data.nav.edges.clone();
    for (n, t) in input.teleporters.iter().enumerate() {
        let from = on_mesh(&data.nav, t.sender);
        let to = on_mesh(&data.nav, t.receiver);
        for (end, at, on) in [("sender", t.sender, from), ("receiver", t.receiver, to)] {
            if on.is_none() {
                problems.push(Problem {
                    kind: ProblemKind::TeleporterOffMesh,
                    what: format!("teleporter {n} {end}"),
                    position: at,
                    reason: "no floor under it: bots do not take this teleporter".into(),
                });
            }
        }
        if let (Some(from), Some(to)) = (from, to) {
            edges.push(Edge {
                from,
                to,
                a: t.sender,
                b: t.receiver,
                kind: Kind::Teleporter,
            });
        }
    }
    if edges.len() != data.nav.edges.len() {
        data.nav = NavMesh::new(std::mem::take(&mut data.nav.polys), edges);
    }

    for (i, &p) in input.starts.iter().enumerate() {
        if on_mesh(&data.nav, p).is_none() {
            problems.push(Problem {
                kind: ProblemKind::StartOffMesh,
                what: format!("start {i}"),
                position: p,
                reason: "no floor under it: a player who spawns here falls or sticks".into(),
            });
        }
    }
    data.nav.mark_out_of_map(&input.starts);

    Ok(Generated {
        data,
        solids: merged,
        problems,
    })
}

#[cfg(test)]
mod tests;
