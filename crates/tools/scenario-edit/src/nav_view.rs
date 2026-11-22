//! Navigation as the viewport shows it (docs/editor/editor.md,
//! "Navigation"): each structure BSP's own pathfinding data, the sectors
//! and links Halo 2's AI walks on (`sbsp+0xc4`, [`tags::pathfinding`];
//! docs/formats/pathfinding.md), and the bots' nav mesh with the
//! superbounce spots they use ([`BotNav`]; docs/game/bots.md, §3.2 and
//! "Superbounce routes").
//!
//! The pathfinding data reads with the map ([`LevelPathfinding::read`]).
//! The bots' data does not: this crate does not depend on the bots. The
//! editor reads their cache files (or builds the mesh) and hands the
//! session a [`BotNav`] (`Session::set_bot_nav`). Both are read only.
//!
//! A sector's polygon is its link ring
//! ([`tags::pathfinding::PathfindingData::sector_polygon`]); a sector whose
//! ring does not close, or names a vertex out of range, is left out and
//! counted in [`BspPathfinding::broken`]. Orphan and stale links (no sector
//! either side) are left out of [`LevelPathfinding::links`].
use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::pathfinding::PathfindingData;

use crate::geometry::{self, Ray, Vec3, add, cross, dot, normalize, scale, sub};
use crate::layers::LayerId;

/// How far above its surface a pathfinding sector is drawn, so it does
/// not fight the floor it lies on.
pub const PATHFINDING_LIFT: f32 = 0.01;
/// How far above its surface a bot polygon is drawn: over a sector on the
/// same floor.
pub const BOT_LIFT: f32 = 0.02;
/// The radius a spot is picked within (world units).
pub const SPOT_RADIUS: f32 = 0.2;
/// The least distance (world units) a ray may pass a link and pick it;
/// it grows with the distance along the ray ([`LINK_SLOPE`]).
pub const LINK_REACH: f32 = 0.05;
pub const LINK_SLOPE: f32 = 0.004;

// ---- polygons ----

fn fan(ring: &[Vec3]) -> Vec<[Vec3; 3]> {
    (1..ring.len().saturating_sub(1))
        .map(|k| [ring[0], ring[k], ring[k + 1]])
        .collect()
}

/// The ring's unit normal (Newell's method), pointing up for a ring that
/// winds counter-clockwise seen from above.
pub fn ring_normal(ring: &[Vec3]) -> Vec3 {
    let mut n = [0.0f32; 3];
    for i in 0..ring.len() {
        let (u, v) = (ring[i], ring[(i + 1) % ring.len()]);
        n[0] += (u[1] - v[1]) * (u[2] + v[2]);
        n[1] += (u[2] - v[2]) * (u[0] + v[0]);
        n[2] += (u[0] - v[0]) * (u[1] + v[1]);
    }
    normalize(n)
}

fn ring_center(ring: &[Vec3]) -> Vec3 {
    let n = ring.len().max(1) as f32;
    scale(ring.iter().fold([0.0; 3], |a, &p| add(a, p)), 1.0 / n)
}

fn ring_radius(ring: &[Vec3]) -> f32 {
    let c = ring_center(ring);
    ring.iter()
        .map(|&p| dot(sub(p, c), sub(p, c)).sqrt())
        .fold(0.0, f32::max)
}

fn ring_edges(ring: &[Vec3]) -> Vec<[Vec3; 2]> {
    let n = ring.len();
    (0..n)
        .filter(|_| n >= 2)
        .map(|i| [ring[i], ring[(i + 1) % n]])
        .collect()
}

fn ring_hit(ring: &[Vec3], ray: &Ray) -> Option<f32> {
    fan(ring)
        .iter()
        .filter_map(|t| ray.triangle(t))
        .min_by(f32::total_cmp)
}

/// How far `p` is from the ring's plane, when it lies over the ring.
fn ring_distance(ring: &[Vec3], normal: Vec3, p: Vec3) -> Option<f32> {
    let c = *ring.first()?;
    let h = dot(normal, sub(p, c));
    let foot = sub(p, scale(normal, h));
    let inside = fan(ring).iter().any(|t| {
        let side = |a: Vec3, b: Vec3| dot(cross(sub(b, a), sub(foot, a)), normal);
        let (a, b, c) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
        let eps = 1e-5;
        (a >= -eps && b >= -eps && c >= -eps) || (a <= eps && b <= eps && c <= eps)
    });
    inside.then_some(h.abs())
}

/// The ring raised by `lift` along +z (or along its normal for a wall).
pub fn lifted(ring: &[Vec3], normal: Vec3, lift: f32) -> Vec<Vec3> {
    let up = if normal[2].abs() > 0.3 {
        [0.0, 0.0, lift * normal[2].signum()]
    } else {
        scale(normal, lift)
    };
    ring.iter().map(|&p| add(p, up)).collect()
}

/// The ray parameter and the distance where a ray passes nearest the
/// segment `a`-`b`, for a point ahead of the ray's origin.
pub fn ray_segment(ray: &Ray, a: Vec3, b: Vec3) -> (f32, f32) {
    let u = ray.dir;
    let v = sub(b, a);
    let w = sub(ray.origin, a);
    let (aa, bb, cc) = (dot(u, u).max(1e-12), dot(u, v), dot(v, v));
    let (dd, ee) = (dot(u, w), dot(v, w));
    let along = |s: f32| {
        if cc > 1e-12 {
            ((bb * s + ee) / cc).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let den = aa * cc - bb * bb;
    // The segment's point nearest the ray's line, then the ray's point
    // nearest that; behind the origin, the origin.
    let mut t = if den.abs() < 1e-9 {
        along(0.0)
    } else {
        ((aa * ee - bb * dd) / den).clamp(0.0, 1.0)
    };
    let mut s = (t * bb - dd) / aa;
    if s < 0.0 {
        s = 0.0;
        t = along(0.0);
    }
    let p = add(ray.origin, scale(u, s));
    let q = add(a, scale(v, t));
    (s, dot(sub(p, q), sub(p, q)).sqrt())
}

fn link_hit(ray: &Ray, a: Vec3, b: Vec3) -> Option<f32> {
    let (t, d) = ray_segment(ray, a, b);
    (d <= LINK_REACH.max(LINK_SLOPE * t)).then_some(t)
}

fn point_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let v = sub(b, a);
    let cc = dot(v, v);
    let t = if cc > 1e-12 {
        (dot(sub(p, a), v) / cc).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = add(a, scale(v, t));
    dot(sub(p, q), sub(p, q)).sqrt()
}

// ---- the game's pathfinding ----

/// One sector of one structure BSP's pathfinding data.
#[derive(Clone, Debug, PartialEq)]
pub struct Sector {
    /// The scenario BSP's index (`scnr+0x210`).
    pub bsp: usize,
    /// The sector's index in that BSP's pathfinding data.
    pub sector: usize,
    pub flags: u16,
    /// The compiled hint index, `0xffff` for none.
    pub hint: u16,
    /// The links of its ring, in ring order (indices into that BSP's
    /// links).
    pub links: Vec<usize>,
    /// Its polygon, in ring order.
    pub ring: Vec<Vec3>,
    pub normal: Vec3,
}

impl Sector {
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        fan(&self.ring)
    }
    pub fn center(&self) -> Vec3 {
        ring_center(&self.ring)
    }
    pub fn radius(&self) -> f32 {
        ring_radius(&self.ring)
    }
    pub fn edges(&self) -> Vec<[Vec3; 2]> {
        ring_edges(&self.ring)
    }
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        ring_hit(&self.ring, ray)
    }
    pub fn distance(&self, p: Vec3) -> Option<f32> {
        ring_distance(&self.ring, self.normal, p)
    }
    /// A floor: its normal within 45 degrees of up, as the pathfinding
    /// generator counts walkable (docs/formats/pathfinding.md).
    pub fn floor(&self) -> bool {
        self.normal[2] >= std::f32::consts::FRAC_1_SQRT_2
    }
}

/// One link (a sector edge) of one structure BSP's pathfinding data.
#[derive(Clone, Debug, PartialEq)]
pub struct PathLink {
    pub bsp: usize,
    /// The link's index in that BSP's pathfinding data.
    pub link: usize,
    pub a: Vec3,
    pub b: Vec3,
    pub flags: u16,
    /// The sector on each side, if any.
    pub left: Option<usize>,
    pub right: Option<usize>,
}

/// What one structure BSP carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BspPathfinding {
    pub bsp: usize,
    pub tag: TagId,
    /// Whether it has pathfinding data at all (`sbsp+0xc4` count 1).
    pub present: bool,
    pub sectors: usize,
    pub links: usize,
    pub nodes: usize,
    pub vertices: usize,
    pub refs: usize,
    /// Links with no sector either side.
    pub orphan_links: usize,
    /// Sectors left out: their ring does not close or names a vertex out
    /// of range.
    pub broken: Vec<usize>,
}

/// Every scenario BSP's pathfinding data, as polygons and lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelPathfinding {
    pub bsps: Vec<BspPathfinding>,
    pub sectors: Vec<Sector>,
    pub links: Vec<PathLink>,
    /// BSPs whose pathfinding did not read: (scenario BSP index, why).
    pub errors: Vec<(usize, String)>,
}

impl LevelPathfinding {
    /// Read every scenario BSP's pathfinding data
    /// ([`PathfindingData::read`]). A BSP that does not read is named in
    /// [`Self::errors`]; the others still read.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> crate::Result<Self> {
        let mut out = Self::default();
        for (i, id) in geometry::scenario_bsps(map)?.into_iter().enumerate() {
            match PathfindingData::read(map, id) {
                Ok(data) => out.add(i, id, data.as_ref()),
                Err(e) => {
                    out.add(i, id, None);
                    out.errors.push((i, e.to_string()));
                }
            }
        }
        Ok(out)
    }

    /// Add scenario BSP `bsp`'s data, or none.
    pub fn add(&mut self, bsp: usize, tag: TagId, data: Option<&PathfindingData>) {
        let mut row = BspPathfinding {
            bsp,
            tag,
            present: data.is_some(),
            ..BspPathfinding::default()
        };
        if let Some(pf) = data {
            row.sectors = pf.sectors.len();
            row.links = pf.links.len();
            row.nodes = pf.bsp2d_nodes.len();
            row.vertices = pf.vertices.len();
            row.refs = pf.refs.len();
            row.orphan_links = pf.orphan_links();
            let vertex = |v: usize| pf.vertices.get(v).copied();
            for (i, s) in pf.sectors.iter().enumerate() {
                let ring = pf.sector_ring(i);
                let poly: Option<Vec<Vec3>> = pf
                    .sector_polygon(i)
                    .and_then(|vs| vs.into_iter().map(vertex).collect());
                match (ring, poly) {
                    (Some(links), Some(ring)) if ring.len() >= 3 => {
                        self.sectors.push(Sector {
                            bsp,
                            sector: i,
                            flags: s.flags,
                            hint: s.hint,
                            links,
                            normal: ring_normal(&ring),
                            ring,
                        });
                    }
                    _ => row.broken.push(i),
                }
            }
            let side = |s: u16| {
                (s != u16::MAX && usize::from(s) < pf.sectors.len()).then_some(usize::from(s))
            };
            for (i, l) in pf.links.iter().enumerate() {
                let (left, right) = (side(l.left_sector), side(l.right_sector));
                let (Some(a), Some(b)) = (vertex(l.vertex1.into()), vertex(l.vertex2.into()))
                else {
                    continue;
                };
                if left.is_none() && right.is_none() {
                    continue;
                }
                self.links.push(PathLink {
                    bsp,
                    link: i,
                    a,
                    b,
                    flags: l.flags,
                    left,
                    right,
                });
            }
        }
        self.bsps.push(row);
    }

    /// Whether no BSP carries a drawable sector.
    pub fn is_empty(&self) -> bool {
        self.sectors.is_empty()
    }

    /// Why the layer shows nothing, when it does not.
    pub fn note(&self) -> Option<String> {
        if !self.is_empty() {
            return None;
        }
        Some(if self.bsps.is_empty() {
            "the map has no structure BSP".to_owned()
        } else if self.bsps.iter().all(|b| !b.present) {
            "no structure BSP carries pathfinding data (retail multiplayer maps have none)"
                .to_owned()
        } else {
            "the pathfinding data has no sector that reads".to_owned()
        })
    }

    /// The sector index (into [`Self::sectors`]) of BSP `bsp`'s sector
    /// `sector`.
    pub fn find(&self, bsp: usize, sector: usize) -> Option<usize> {
        self.sectors
            .iter()
            .position(|s| s.bsp == bsp && s.sector == sector)
    }

    /// The nearest sector the ray meets, either side.
    pub fn pick(&self, ray: &Ray) -> Option<(f32, usize)> {
        self.sectors
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.hit(ray).map(|t| (t, i)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The sector nearest `p` within `within` of its plane, over it.
    pub fn at_point(&self, p: Vec3, within: f32) -> Option<(f32, usize)> {
        self.sectors
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.distance(p).map(|d| (d, i)))
            .filter(|(d, _)| *d <= within)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }
}

// ---- the bots' nav mesh ----

/// How a bot link is crossed (`bots::nav::Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BotLinkKind {
    Walk,
    Drop,
    Jump,
    CrouchJump,
    Teleporter,
    Lift,
    Elevator,
}

impl BotLinkKind {
    pub const ALL: [Self; 7] = [
        Self::Walk,
        Self::Drop,
        Self::Jump,
        Self::CrouchJump,
        Self::Teleporter,
        Self::Lift,
        Self::Elevator,
    ];

    /// The name the agent API and the legend use.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Walk => "walk",
            Self::Drop => "drop",
            Self::Jump => "jump",
            Self::CrouchJump => "crouch_jump",
            Self::Teleporter => "teleporter",
            Self::Lift => "lift",
            Self::Elevator => "elevator",
        }
    }

    /// A link whose ends are a takeoff (or trigger) and a separate
    /// landing, drawn from `a` to `b`.
    pub const fn traversal(self) -> bool {
        matches!(
            self,
            Self::CrouchJump | Self::Teleporter | Self::Lift | Self::Elevator
        )
    }
}

/// One walkable polygon of the bots' mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct BotPoly {
    /// The structure BSP its file is for.
    pub bsp: usize,
    /// Its index in that BSP's mesh.
    pub poly: usize,
    /// Its outline, counter-clockwise seen from above.
    pub verts: Vec<Vec3>,
    pub normal: Vec3,
}

impl BotPoly {
    pub fn center(&self) -> Vec3 {
        ring_center(&self.verts)
    }
    pub fn radius(&self) -> f32 {
        ring_radius(&self.verts)
    }
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        fan(&self.verts)
    }
    pub fn edges(&self) -> Vec<[Vec3; 2]> {
        ring_edges(&self.verts)
    }
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        ring_hit(&self.verts, ray)
    }
    pub fn distance(&self, p: Vec3) -> Option<f32> {
        ring_distance(&self.verts, self.normal, p)
    }
}

/// One directed link of the bots' mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct BotLink {
    pub bsp: usize,
    /// Its index in that BSP's mesh.
    pub link: usize,
    /// The polygons it joins, as indices into [`BotNav::polys`].
    pub from: usize,
    pub to: usize,
    /// The crossing on the `from` side; for a traversal, the takeoff
    /// (`a`) and the landing (`b`).
    pub a: Vec3,
    pub b: Vec3,
    pub kind: BotLinkKind,
    /// No link of any kind leads back from `to` to `from`.
    pub one_way: bool,
}

impl BotLink {
    /// The line the viewport draws: the takeoff to the landing for a
    /// traversal, else the crossing's middle to the far polygon's centre.
    pub fn line(&self, polys: &[BotPoly]) -> [Vec3; 2] {
        if self.kind.traversal() {
            return [self.a, self.b];
        }
        let mid = scale(add(self.a, self.b), 0.5);
        let to = polys.get(self.to).map_or(self.b, BotPoly::center);
        [mid, to]
    }
}

/// A superbounce spot's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpotKind {
    /// A two-joint bounce slot.
    Bounce,
    /// A single-crease bounce strip.
    Strip,
    /// A forced-crouch place a bot primes at.
    Crouch,
}

impl SpotKind {
    pub const ALL: [Self; 3] = [Self::Bounce, Self::Strip, Self::Crouch];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bounce => "bounce",
            Self::Strip => "strip",
            Self::Crouch => "crouch",
        }
    }
}

/// What the data directory's files say of a spot, the strongest first:
/// a game run proved it (or it is a hand-measured spot), a game run
/// failed it (`.verified`), training found it dead (`.learned`), training
/// learned it (a `learned` or `partial` line), training is under way (an
/// `open` or `blocked` line), or nothing yet (the scan's guess).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpotState {
    Proven,
    Failed,
    Dead,
    Learned,
    Training,
    Likely,
}

impl SpotState {
    pub const ALL: [Self; 6] = [
        Self::Proven,
        Self::Failed,
        Self::Dead,
        Self::Learned,
        Self::Training,
        Self::Likely,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Proven => "proven",
            Self::Failed => "failed",
            Self::Dead => "dead",
            Self::Learned => "learned",
            Self::Training => "training",
            Self::Likely => "likely",
        }
    }
}

/// One superbounce or crouch spot, with what the files say of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BotSpot {
    pub kind: Option<SpotKind>,
    /// `measured` (a hand-measured spot) or `scan`.
    pub source: &'static str,
    /// Where a player stands to use it.
    pub stand: Vec3,
    /// The strip's midline (bounce), or the side standing becomes clear
    /// (crouch).
    pub edge: [Vec3; 2],
    /// A crouch spot's lip edges.
    pub lip: Vec<[Vec3; 2]>,
    /// A crouch spot's way out (unit, X and Y), zero when not known.
    pub out: [f32; 2],
    pub state: Option<SpotState>,
    pub score: f32,
    /// Estimated launch speed (units a second up) and the fall it needs.
    pub launch: f32,
    pub fall: f32,
    /// The learned line's state name, when the spot has one.
    pub line: Option<&'static str>,
    /// The learned line's scored tries and bounces.
    pub tries: u32,
    pub hits: u32,
    /// The verified file's trials and successes.
    pub trials: u32,
    pub ok: u32,
    /// The ghosts file keeps a run of it.
    pub ghost: bool,
}

impl BotSpot {
    pub fn kind(&self) -> SpotKind {
        self.kind.unwrap_or(SpotKind::Bounce)
    }
    pub fn state(&self) -> SpotState {
        self.state.unwrap_or(SpotState::Likely)
    }
}

/// The bots' data for the open map: every structure BSP's nav mesh, and
/// the map's spots with their state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BotNav {
    /// The map's internal name, what the files are named by.
    pub map: String,
    /// Where the mesh came from, per BSP: a cache file's path or `built`.
    pub origins: Vec<String>,
    pub polys: Vec<BotPoly>,
    pub links: Vec<BotLink>,
    /// Line-of-sight wall triangles, per BSP total.
    pub walls: usize,
    pub spots: Vec<BotSpot>,
    /// What the loader wants the user to know: a stale cache built again,
    /// a spots file missing, a file that did not read.
    pub notes: Vec<String>,
}

impl BotNav {
    /// Mark each link that no link leads back along.
    pub fn mark_one_way(&mut self) {
        let back: std::collections::HashSet<(usize, usize)> =
            self.links.iter().map(|l| (l.from, l.to)).collect();
        for l in &mut self.links {
            l.one_way = !back.contains(&(l.to, l.from));
        }
    }

    pub fn link_count(&self, kind: BotLinkKind) -> usize {
        self.links.iter().filter(|l| l.kind == kind).count()
    }

    pub fn spot_count(&self, state: SpotState) -> usize {
        self.spots.iter().filter(|s| s.state() == state).count()
    }

    pub fn spot_kind_count(&self, kind: SpotKind) -> usize {
        self.spots.iter().filter(|s| s.kind() == kind).count()
    }

    fn pick_poly(&self, ray: &Ray) -> Option<(f32, usize)> {
        self.polys
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                ring_hit(&lifted(&p.verts, p.normal, BOT_LIFT), ray).map(|t| (t, i))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    fn pick_link(&self, ray: &Ray) -> Option<(f32, usize)> {
        self.links
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let [a, b] = self.lifted_line(l);
                link_hit(ray, a, b).map(|t| (t, i))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    fn pick_spot(&self, ray: &Ray) -> Option<(f32, usize)> {
        self.spots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| ray.sphere(s.stand, SPOT_RADIUS).map(|t| (t, i)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// A link's line, raised with the polygons.
    pub fn lifted_line(&self, l: &BotLink) -> [Vec3; 2] {
        l.line(&self.polys)
            .map(|p| add(p, [0.0, 0.0, BOT_LIFT * 2.0]))
    }
}

/// What a navigation pick selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavPick {
    /// An index into [`LevelPathfinding::sectors`].
    Sector(usize),
    /// An index into [`BotNav::polys`].
    BotPoly(usize),
    /// An index into [`BotNav::links`].
    BotLink(usize),
    /// An index into [`BotNav::spots`].
    Spot(usize),
}

impl NavPick {
    /// The name the agent API uses.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Sector(_) => "sector",
            Self::BotPoly(_) => "poly",
            Self::BotLink(_) => "link",
            Self::Spot(_) => "spot",
        }
    }
    pub const fn index(self) -> usize {
        match self {
            Self::Sector(i) | Self::BotPoly(i) | Self::BotLink(i) | Self::Spot(i) => i,
        }
    }
    /// The layer that draws it.
    pub const fn layer(self) -> LayerId {
        match self {
            Self::Sector(_) => LayerId::Pathfinding,
            Self::BotPoly(_) | Self::BotLink(_) => LayerId::BotNav,
            Self::Spot(_) => LayerId::BotSpots,
        }
    }
    /// From the API's kind name and index.
    pub fn from_name(kind: &str, index: usize) -> Option<Self> {
        Some(match kind {
            "sector" => Self::Sector(index),
            "poly" => Self::BotPoly(index),
            "link" => Self::BotLink(index),
            "spot" => Self::Spot(index),
            _ => return None,
        })
    }
}

/// The nearest navigation thing a ray meets among the layers `shown`
/// accepts: a spot or a link first (small targets) when one is in reach,
/// else the nearer of a bot polygon and a pathfinding sector.
pub fn pick(
    path: Option<&LevelPathfinding>,
    bots: Option<&BotNav>,
    ray: &Ray,
    shown: impl Fn(LayerId) -> bool,
) -> Option<(f32, NavPick)> {
    if let Some(b) = bots {
        if shown(LayerId::BotSpots)
            && let Some((t, i)) = b.pick_spot(ray)
        {
            return Some((t, NavPick::Spot(i)));
        }
        if shown(LayerId::BotNav)
            && let Some((t, i)) = b.pick_link(ray)
        {
            return Some((t, NavPick::BotLink(i)));
        }
    }
    let poly = bots
        .filter(|_| shown(LayerId::BotNav))
        .and_then(|b| b.pick_poly(ray))
        .map(|(t, i)| (t, NavPick::BotPoly(i)));
    let sector = path
        .filter(|_| shown(LayerId::Pathfinding))
        .and_then(|p| {
            p.sectors
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    ring_hit(&lifted(&s.ring, s.normal, PATHFINDING_LIFT), ray).map(|t| (t, i))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        })
        .map(|(t, i)| (t, NavPick::Sector(i)));
    match (poly, sector) {
        // A bot polygon over a sector on the same floor wins a near tie.
        (Some(p), Some(s)) => Some(if p.0 <= s.0 + BOT_LIFT { p } else { s }),
        (p, s) => p.or(s),
    }
}

/// The navigation thing at a point: a spot within `within` of its stand,
/// a link within `within` of its line, else the polygon or sector the
/// point lies over within `within` of its plane (a bot polygon first).
pub fn at_point(
    path: Option<&LevelPathfinding>,
    bots: Option<&BotNav>,
    p: Vec3,
    within: f32,
    shown: impl Fn(LayerId) -> bool,
) -> Option<(f32, NavPick)> {
    let best = |it: &mut dyn Iterator<Item = (f32, usize)>| {
        it.filter(|(d, _)| *d <= within)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    if let Some(b) = bots {
        if shown(LayerId::BotSpots)
            && let Some((d, i)) = best(&mut b.spots.iter().enumerate().map(|(i, s)| {
                let d = sub(p, s.stand);
                (dot(d, d).sqrt(), i)
            }))
        {
            return Some((d, NavPick::Spot(i)));
        }
        if shown(LayerId::BotNav) {
            if let Some((d, i)) = best(&mut b.links.iter().enumerate().map(|(i, l)| {
                let [a, c] = l.line(&b.polys);
                (point_segment(p, a, c), i)
            })) {
                return Some((d, NavPick::BotLink(i)));
            }
            if let Some((d, i)) = best(
                &mut b
                    .polys
                    .iter()
                    .enumerate()
                    .filter_map(|(i, q)| q.distance(p).map(|d| (d, i))),
            ) {
                return Some((d, NavPick::BotPoly(i)));
            }
        }
    }
    path.filter(|_| shown(LayerId::Pathfinding))
        .and_then(|l| l.at_point(p, within))
        .map(|(d, i)| (d, NavPick::Sector(i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32, z: f32) -> Vec<Vec3> {
        vec![
            [x, 0.0, z],
            [x + 1.0, 0.0, z],
            [x + 1.0, 1.0, z],
            [x, 1.0, z],
        ]
    }

    fn nav() -> BotNav {
        let polys: Vec<BotPoly> = [quad(0.0, 0.0), quad(1.0, 0.0), quad(2.0, -1.0)]
            .into_iter()
            .enumerate()
            .map(|(i, verts)| BotPoly {
                bsp: 0,
                poly: i,
                normal: ring_normal(&verts),
                verts,
            })
            .collect();
        let link = |link, from, to, a, b, kind| BotLink {
            bsp: 0,
            link,
            from,
            to,
            a,
            b,
            kind,
            one_way: false,
        };
        let mut nav = BotNav {
            map: "test".into(),
            polys,
            links: vec![
                link(0, 0, 1, [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], BotLinkKind::Walk),
                link(1, 1, 0, [1.0, 1.0, 0.0], [1.0, 0.0, 0.0], BotLinkKind::Walk),
                link(2, 1, 2, [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], BotLinkKind::Drop),
            ],
            ..BotNav::default()
        };
        nav.mark_one_way();
        nav
    }

    #[test]
    fn a_drop_with_no_way_back_is_one_way_and_walks_are_not() {
        let nav = nav();
        assert_eq!(
            nav.links.iter().map(|l| l.one_way).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(nav.link_count(BotLinkKind::Walk), 2);
        assert_eq!(nav.link_count(BotLinkKind::Drop), 1);
        assert!(ring_normal(&nav.polys[0].verts)[2] > 0.99);
    }

    #[test]
    fn a_ray_picks_a_poly_from_above_and_a_link_it_passes() {
        let nav = nav();
        let all = |_| true;
        let down = Ray {
            origin: [0.5, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(
            pick(None, Some(&nav), &down, all).map(|h| h.1),
            Some(NavPick::BotPoly(0))
        );
        // The drop's line runs from x 2 to the low quad's centre at x 2.5.
        let [a, b] = nav.lifted_line(&nav.links[2]);
        let mid = scale(add(a, b), 0.5);
        let at_link = Ray {
            origin: add(mid, [0.0, 0.0, 3.0]),
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(
            pick(None, Some(&nav), &at_link, all).map(|h| h.1),
            Some(NavPick::BotLink(2))
        );
        assert_eq!(
            pick(None, Some(&nav), &down, |_| false),
            None,
            "hidden layers pick nothing"
        );
        let below = at_point(None, Some(&nav), [2.5, 0.75, -0.9], 0.2, |id| {
            id == LayerId::BotNav
        });
        assert_eq!(below.map(|h| h.1), Some(NavPick::BotPoly(2)));
    }
}
