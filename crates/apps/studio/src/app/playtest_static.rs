//! Playtest's static checks (docs/editor/playtest.md, "Static checks"):
//! what can be said of a map's layout with no match, on every edit.
//!
//! [`check`] is pure. It takes a [`Layout`] (the starting locations,
//! netgame items and netgame flags with their teams, the bots' nav mesh,
//! the level collision's triangles and the start gate's census) and gives
//! a [`Report`]:
//!
//! - **off the mesh**: a starting location, item or flag that stands on
//!   no polygon of the bots' mesh ([`locate`]);
//! - **unreachable**: a starting location the mesh's links do not lead
//!   from another starting location to, per team and across;
//! - **islands**: polygons no starting location reaches, grouped by the
//!   links between them;
//! - **one way**: a link out of the reached mesh into polygons with no
//!   way back (a drop off a ledge into a pit), one per trap;
//! - **in sight**: two starting locations of opposing teams with a clear
//!   line between their eyes through the level collision;
//! - **readiness**, per built-in game type ([`::rules::builtin::ALL`]):
//!   what the start gate's check (`rules::labels::check`) finds missing in
//!   the census, enough starting locations on the mesh, and for Capture
//!   the Flag and Assault the flags and team starts a match needs
//!   ([`objectives`]).
//!
//! [`App::playtest_static`] builds the layout from the open map's
//! scenario as edited (saved or not), its collision and the bots' mesh
//! (`app::bot_nav`), and runs the check.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use ::rules::ir::Ir;
use ::rules::labels::{self, Census, Class};
use forge_doc::{CensusOf, FlagRow, MapRows};
use scenario_edit::geometry::{Ray, Vec3, cross, dot, scale, sub};
use scenario_edit::nav_view::{BotLinkKind, BotNav, BotPoly};
use scenario_edit::{BlockKind, Session};

use super::App;
use super::bot_nav::BotNavState;

/// How far over a polygon's floor (world units) a placement may stand and
/// still be on it: a starting location's marker floats over the floor.
pub const STAND_ABOVE: f32 = 1.0;
/// How far under the floor it may be (a marker sunk into it), as the
/// bots' own `NavMesh::locate` allows.
pub const STAND_BELOW: f32 = 0.1;
/// The eye's height over a starting location, for sight lines.
pub const EYE: f32 = 0.6;
/// Starting locations farther apart than this (world units) are not
/// reported in sight of each other.
pub const SIGHT_RANGE: f32 = 40.0;
/// The least area (square world units) of an island that is reported:
/// smaller slivers are the mesh builder's, not the map's.
pub const MIN_ISLAND_AREA: f32 = 1.0;
/// The least starting locations on the mesh any game type needs.
pub const MIN_STARTS: usize = 2;
/// The teams a census is checked for, and a team game type needs.
pub const TEAMS: [u8; 2] = [0, 1];

/// Capture the Flag's and Assault's retail engine indices.
const CTF: u8 = 1;
const ASSAULT: u8 = 9;

/// One placement a check reads: its index in its block, where it is and
/// its team (a designator, 0 to 7; neutral and none are `None`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub index: usize,
    pub at: Vec3,
    pub team: Option<u8>,
}

/// What the checks read of a map.
#[derive(Clone, Debug)]
pub struct Layout<'a> {
    /// Starting locations.
    pub starts: Vec<Placed>,
    /// Netgame items (`netgame_equipment`).
    pub items: Vec<Placed>,
    /// Netgame flags: objectives, teleporters and the rest.
    pub flags: Vec<Placed>,
    /// The bots' nav mesh, or why there is none.
    pub mesh: Result<&'a BotNav, String>,
    /// Triangles that block a sight line: the level collision.
    pub blockers: Vec<[Vec3; 3]>,
    /// The start gate's census of the map.
    pub census: Census,
}

/// What a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// No bots' mesh to check against.
    NoMesh,
    /// A placement on no polygon of the mesh.
    OffMesh,
    /// A starting location that cannot reach another.
    Unreachable,
    /// Polygons no starting location reaches.
    Island,
    /// A link into polygons with no way back.
    OneWay,
    /// Two opposing starting locations in sight of each other.
    InSight,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::NoMesh,
        Self::OffMesh,
        Self::Unreachable,
        Self::Island,
        Self::OneWay,
        Self::InSight,
    ];

    /// The name the agent API uses.
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoMesh => "no_mesh",
            Self::OffMesh => "off_mesh",
            Self::Unreachable => "unreachable",
            Self::Island => "island",
            Self::OneWay => "one_way",
            Self::InSight => "in_sight",
        }
    }
}

/// What a problem points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A placement (`nav.go {block, index}`).
    Placement(BlockKind, usize),
    /// A polygon of the bots' mesh (`nav.pick {kind: "poly", index}`).
    Poly(usize),
}

/// One problem and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: Kind,
    pub text: String,
    pub target: Option<Target>,
}

/// Whether a game type can be played on the map, and what it lacks.
#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    pub game_type: &'static str,
    /// The retail engine index its script rides.
    pub engine: u8,
    pub ok: bool,
    pub missing: Vec<String>,
}

/// The checks' result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub problems: Vec<Problem>,
    pub ready: Vec<Ready>,
}

impl Report {
    /// No problem was found.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The readiness of the game type named `name`.
    pub fn ready_for(&self, name: &str) -> Option<&Ready> {
        self.ready.iter().find(|r| r.game_type == name)
    }

    /// The problems of one kind.
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Problem> {
        self.problems.iter().filter(move |p| p.kind == kind)
    }
}

/// Every check over `layout`.
pub fn check(layout: &Layout) -> Report {
    let mut problems = Vec::new();
    let on_mesh = match layout.mesh {
        Err(ref why) => {
            problems.push(Problem {
                kind: Kind::NoMesh,
                text: format!("no bots' mesh to check against: {why}"),
                target: None,
            });
            None
        }
        Ok(nav) if nav.polys.is_empty() => {
            problems.push(Problem {
                kind: Kind::NoMesh,
                text: "the bots' mesh has no polygons: the level has no floor it accepts".into(),
                target: None,
            });
            None
        }
        Ok(nav) => Some(mesh_checks(layout, nav, &mut problems)),
    };
    sight(layout, &mut problems);
    let ready = readiness(layout, on_mesh);
    Report { problems, ready }
}

// ---- the mesh ----

/// The polygon `p` stands on: over its outline, from [`STAND_BELOW`]
/// under its plane to [`STAND_ABOVE`] over it; the nearest such floor.
pub fn locate(nav: &BotNav, p: Vec3) -> Option<usize> {
    nav.polys
        .iter()
        .enumerate()
        .filter_map(|(i, poly)| {
            let h = height(poly, p)?;
            (-STAND_BELOW..=STAND_ABOVE)
                .contains(&h)
                .then_some((h.abs(), i))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// How far `p` is over `poly`'s plane, when it is over the outline.
fn height(poly: &BotPoly, p: Vec3) -> Option<f32> {
    let first = *poly.verts.first()?;
    poly.distance(p)?;
    Some(dot(poly.normal, sub(p, first)))
}

fn area(poly: &BotPoly) -> f32 {
    poly.triangles()
        .iter()
        .map(|t| {
            let c = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            0.5 * dot(c, c).sqrt()
        })
        .sum()
}

/// The mesh's links as adjacency lists: out along each link, and both
/// ways (for grouping islands).
struct Graph {
    out: Vec<Vec<usize>>,
    both: Vec<Vec<usize>>,
}

impl Graph {
    fn new(nav: &BotNav) -> Self {
        let n = nav.polys.len();
        let mut out = vec![Vec::new(); n];
        let mut both = vec![Vec::new(); n];
        for l in nav.links.iter().filter(|l| l.from < n && l.to < n) {
            out[l.from].push(l.to);
            both[l.from].push(l.to);
            both[l.to].push(l.from);
        }
        Self { out, both }
    }

    /// Every polygon reached from `from` along the links.
    fn reach(&self, from: usize) -> Vec<bool> {
        let mut seen = vec![false; self.out.len()];
        seen[from] = true;
        let mut stack = vec![from];
        while let Some(v) = stack.pop() {
            for &w in &self.out[v] {
                if !seen[w] {
                    seen[w] = true;
                    stack.push(w);
                }
            }
        }
        seen
    }

    /// Each polygon's strongly connected component (Kosaraju, without
    /// recursion: a retail mesh has thousands of polygons).
    fn components(&self) -> Vec<usize> {
        let n = self.out.len();
        let mut seen = vec![false; n];
        let mut order = Vec::with_capacity(n);
        for s in 0..n {
            if seen[s] {
                continue;
            }
            seen[s] = true;
            let mut stack = vec![(s, 0usize)];
            while let Some(top) = stack.last_mut() {
                let v = top.0;
                if let Some(&w) = self.out[v].get(top.1) {
                    top.1 += 1;
                    if !seen[w] {
                        seen[w] = true;
                        stack.push((w, 0));
                    }
                } else {
                    order.push(v);
                    stack.pop();
                }
            }
        }
        let mut back = vec![Vec::new(); n];
        for (v, outs) in self.out.iter().enumerate() {
            for &w in outs {
                back[w].push(v);
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut next = 0;
        for &s in order.iter().rev() {
            if comp[s] != usize::MAX {
                continue;
            }
            comp[s] = next;
            let mut stack = vec![s];
            while let Some(v) = stack.pop() {
                for &w in &back[v] {
                    if comp[w] == usize::MAX {
                        comp[w] = next;
                        stack.push(w);
                    }
                }
            }
            next += 1;
        }
        comp
    }
}

/// "starting location 3" or "starting locations 3, 5".
fn starts_named(indices: &[usize]) -> String {
    let list: Vec<String> = indices.iter().map(usize::to_string).collect();
    match list.len() {
        1 => format!("starting location {}", list[0]),
        _ => format!("starting locations {}", list.join(", ")),
    }
}

fn point(p: Vec3) -> String {
    format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2])
}

/// The checks that need the mesh. Returns how many starting locations
/// stand on it.
fn mesh_checks(layout: &Layout, nav: &BotNav, out: &mut Vec<Problem>) -> usize {
    let mut starts = Vec::new();
    for (what, block, list) in [
        (
            "starting location",
            BlockKind::StartingLocations,
            &layout.starts,
        ),
        ("netgame item", BlockKind::NetgameEquipment, &layout.items),
        ("netgame flag", BlockKind::NetgameFlags, &layout.flags),
    ] {
        for p in list {
            match locate(nav, p.at) {
                Some(poly) if block == BlockKind::StartingLocations => starts.push((p.index, poly)),
                Some(_) => {}
                None => out.push(Problem {
                    kind: Kind::OffMesh,
                    text: format!(
                        "{what} {} at {} stands on no polygon of the bots' mesh",
                        p.index,
                        point(p.at)
                    ),
                    target: Some(Target::Placement(block, p.index)),
                }),
            }
        }
    }
    if starts.is_empty() {
        return 0;
    }
    let graph = Graph::new(nav);
    let reach: Vec<Vec<bool>> = starts.iter().map(|&(_, poly)| graph.reach(poly)).collect();
    for (i, &(a, _)) in starts.iter().enumerate() {
        let missed: Vec<usize> = starts
            .iter()
            .filter(|&&(b, poly)| b != a && !reach[i][poly])
            .map(|&(b, _)| b)
            .collect();
        if !missed.is_empty() {
            out.push(Problem {
                kind: Kind::Unreachable,
                text: format!(
                    "starting location {a} cannot reach {} along the bots' mesh",
                    starts_named(&missed)
                ),
                target: Some(Target::Placement(BlockKind::StartingLocations, a)),
            });
        }
    }
    let reached: Vec<bool> = (0..nav.polys.len())
        .map(|p| reach.iter().any(|r| r[p]))
        .collect();
    islands(nav, &graph, &reached, out);
    one_way(nav, &graph, &reached, out);
    starts.len()
}

/// The polygons no starting location reaches, grouped by the links
/// between them either way; one problem per group of at least
/// [`MIN_ISLAND_AREA`].
fn islands(nav: &BotNav, graph: &Graph, reached: &[bool], out: &mut Vec<Problem>) {
    let mut grouped = reached.to_vec();
    for s in 0..nav.polys.len() {
        if grouped[s] {
            continue;
        }
        grouped[s] = true;
        let mut members = vec![s];
        let mut stack = vec![s];
        while let Some(v) = stack.pop() {
            for &w in &graph.both[v] {
                if !grouped[w] {
                    grouped[w] = true;
                    members.push(w);
                    stack.push(w);
                }
            }
        }
        let size: f32 = members.iter().map(|&p| area(&nav.polys[p])).sum();
        if size < MIN_ISLAND_AREA {
            continue;
        }
        let first = *members.iter().min().unwrap_or(&s);
        let n = members.len();
        let out_of_map = members.iter().all(|&p| nav.polys[p].out_of_map);
        out.push(Problem {
            kind: Kind::Island,
            text: format!(
                "{n} polygon{} of the bots' mesh ({size:.1} square units{}) that no starting location reaches",
                if n == 1 { "" } else { "s" },
                if out_of_map { ", out of the map" } else { "" },
            ),
            target: Some(Target::Poly(first)),
        });
    }
}

/// Links from the reached mesh into polygons that cannot lead back to
/// where the link left: one problem per trap (the strongly connected
/// component the link lands in).
fn one_way(nav: &BotNav, graph: &Graph, reached: &[bool], out: &mut Vec<Problem>) {
    let comp = graph.components();
    let mut traps = BTreeMap::new();
    for l in &nav.links {
        if l.from >= comp.len() || l.to >= comp.len() || !reached[l.from] {
            continue;
        }
        if comp[l.from] != comp[l.to] {
            traps.entry(comp[l.to]).or_insert(l);
        }
    }
    let mut found: Vec<_> = traps.into_values().collect();
    found.sort_by_key(|l| (l.to, l.from));
    for l in found {
        let how = match l.kind {
            BotLinkKind::Drop => "a drop",
            BotLinkKind::Walk => "a walk",
            k => k.name(),
        };
        out.push(Problem {
            kind: Kind::OneWay,
            text: format!(
                "{how} link from polygon {} into polygon {} has no way back",
                l.from, l.to
            ),
            target: Some(Target::Poly(l.to)),
        });
    }
}

// ---- sight ----

/// Whether starting locations of these teams are on opposing sides: any
/// two but two of the same team.
fn opposed(a: Option<u8>, b: Option<u8>) -> bool {
    !matches!((a, b), (Some(x), Some(y)) if x == y)
}

/// Whether nothing of `blockers` lies on the segment `a`-`b`.
pub fn clear(blockers: &[[Vec3; 3]], a: Vec3, b: Vec3) -> bool {
    let d = sub(b, a);
    let len = dot(d, d).sqrt();
    if len < 1e-4 {
        return true;
    }
    let ray = Ray {
        origin: a,
        dir: scale(d, 1.0 / len),
    };
    !blockers
        .iter()
        .any(|t| ray.triangle(t).is_some_and(|at| at < len))
}

fn sight(layout: &Layout, out: &mut Vec<Problem>) {
    let eye = |p: Vec3| [p[0], p[1], p[2] + EYE];
    for (i, a) in layout.starts.iter().enumerate() {
        for b in &layout.starts[i + 1..] {
            if !opposed(a.team, b.team) {
                continue;
            }
            let (ea, eb) = (eye(a.at), eye(b.at));
            let d = sub(eb, ea);
            let len = dot(d, d).sqrt();
            if len > SIGHT_RANGE || !clear(&layout.blockers, ea, eb) {
                continue;
            }
            out.push(Problem {
                kind: Kind::InSight,
                text: format!(
                    "starting locations {} and {} of opposing sides see each other, {len:.1} units apart",
                    a.index, b.index
                ),
                target: Some(Target::Placement(BlockKind::StartingLocations, a.index)),
            });
        }
    }
}

// ---- game types ----

/// A built-in game type and its compiled script.
struct GameType {
    name: &'static str,
    engine: u8,
    ir: Result<Ir, String>,
}

/// The built-in game types, compiled once.
fn game_types() -> &'static [GameType] {
    static TYPES: OnceLock<Vec<GameType>> = OnceLock::new();
    TYPES.get_or_init(|| {
        ::rules::builtin::ALL
            .iter()
            .map(|b| GameType {
                name: b.name,
                engine: b.engine,
                ir: ::rules::compile(b.source).map_err(|e| e.to_string()),
            })
            .collect()
    })
}

/// What a match of the game type on `engine` needs past the start gate:
/// Capture the Flag a flag spawn per team; Assault a bomb spawn per team
/// (or a neutral one) and a plate per team; both a starting location
/// each team can use (its own or one with no team). The scripts declare
/// these labels with no least count, since one script serves both games,
/// so the gate's check passes a map without them.
pub fn objectives(engine: u8, layout: &Layout) -> Vec<String> {
    let has = |label: &str, team: Option<u8>| {
        layout.census.labels.get(label).is_some_and(|members| {
            members
                .iter()
                .any(|m| matches!(m.class, Class::Flag | Class::Volume) && m.team == team)
        })
    };
    let mut out = Vec::new();
    match engine {
        CTF => {
            for t in TEAMS {
                if !has("ctf_flag_spawn", Some(t)) {
                    out.push(format!("a ctf_flag_spawn flag for team {t}"));
                }
            }
        }
        ASSAULT => {
            for t in TEAMS {
                if !has("assault_bomb_spawn", Some(t)) && !has("assault_bomb_spawn", None) {
                    out.push(format!(
                        "an assault_bomb_spawn flag for team {t} (or a neutral one)"
                    ));
                }
                if !has("assault_bomb_return", Some(t)) {
                    out.push(format!("an assault_bomb_return flag for team {t}"));
                }
            }
        }
        _ => return out,
    }
    for t in TEAMS {
        if !layout.starts.iter().any(|s| s.team.is_none_or(|x| x == t)) {
            out.push(format!("a starting location for team {t}"));
        }
    }
    out
}

/// Each built-in game type's readiness. `on_mesh` is how many starting
/// locations stand on the bots' mesh, `None` with no mesh.
fn readiness(layout: &Layout, on_mesh: Option<usize>) -> Vec<Ready> {
    game_types()
        .iter()
        .map(|g| {
            let mut missing = Vec::new();
            match &g.ir {
                Err(e) => missing.push(format!("its script does not compile: {e}")),
                Ok(ir) => missing.extend(
                    labels::check(ir, &layout.census, &TEAMS)
                        .into_iter()
                        .map(|p| p.text),
                ),
            }
            match on_mesh {
                None => missing.push("the bots' mesh".into()),
                Some(n) if n < MIN_STARTS => missing.push(format!(
                    "{MIN_STARTS} starting locations on the bots' mesh (it has {n})"
                )),
                Some(_) => {}
            }
            missing.extend(objectives(g.engine, layout));
            Ready {
                game_type: g.name,
                engine: g.engine,
                ok: missing.is_empty(),
                missing,
            }
        })
        .collect()
}

// ---- the open map ----

/// The layout of the scenario `session` has open, as edited, with
/// `mesh`; an error with no map open.
pub fn layout_of<'a>(
    session: &Session,
    mesh: Result<&'a BotNav, String>,
) -> Result<Layout<'a>, String> {
    let scn = session.scenario().ok_or("no map is open")?;
    let int = |kind: BlockKind, i: usize, field: &str| {
        scn.get(kind, i, field).ok().and_then(|v| v.as_int())
    };
    let placed = |kind: BlockKind, team: bool| -> Vec<Placed> {
        (0..scn.block(kind).len())
            .filter_map(|i| {
                let at = scn.get(kind, i, "position").ok()?.as_vec3()?;
                let team = if team {
                    int(kind, i, "team").and_then(forge_doc::designator)
                } else {
                    None
                };
                Some(Placed { index: i, at, team })
            })
            .collect()
    };
    let flags = BlockKind::NetgameFlags;
    let starts = BlockKind::StartingLocations;
    let rows = MapRows {
        flags: (0..scn.block(flags).len())
            .map(|i| FlagRow {
                kind: int(flags, i, "type").unwrap_or(-1) as u16,
                team: int(flags, i, "team").unwrap_or(-1) as u16,
            })
            .collect(),
        starts: (0..scn.block(starts).len())
            .map(|i| int(starts, i, "team").unwrap_or(-1) as i16)
            .collect(),
        ..MapRows::default()
    };
    let census = Census::of_game(&rows, None).map_err(|e| e.to_string())?;
    let blockers: Vec<[Vec3; 3]> = session
        .collision()
        .map(|c| c.surfaces.iter().flat_map(|s| s.triangles()).collect())
        .unwrap_or_default();
    Ok(Layout {
        starts: placed(starts, true),
        items: placed(BlockKind::NetgameEquipment, false),
        flags: placed(flags, true),
        mesh,
        blockers,
        census,
    })
}

impl App {
    /// The static checks of the open map as edited, starting the bots'
    /// mesh load if nothing has (`wait` blocks until it is done). `None`
    /// with no map open.
    pub fn playtest_static(&mut self, wait: bool) -> Option<Result<Report, String>> {
        if !self.session.is_open() {
            return None;
        }
        self.want_bot_nav();
        if wait {
            self.wait_bot_nav();
        } else {
            self.poll_bot_nav();
        }
        let state = self.bot_nav_state();
        let mesh = match (state, self.session.bot_nav()) {
            (BotNavState::Ready, Some(nav)) => Ok(&**nav),
            (BotNavState::Loading, _) => Err("it is loading (pass wait to wait for it)".to_owned()),
            (BotNavState::Failed, _) => {
                Err(self.bot_nav_error().unwrap_or("its load failed").to_owned())
            }
            _ => Err("it is not loaded".to_owned()),
        };
        Some(layout_of(&self.session, mesh).map(|l| check(&l)))
    }
}

#[cfg(test)]
mod tests {
    //! The checks on invented layouts. The stage's layout (`content-kit` (its `stage` module),
    //! docs/editor/content-stage.md) joins these as
    //! `the_stage_is_ready_for_slayer` once that crate is on master: its
    //! starting locations, items and flags as [`Placed`], its mesh through
    //! `bot_nav::add_mesh`, and [`check`] asserted ready for Slayer with no
    //! off-mesh, unreachable or island problem.
    use super::*;
    use scenario_edit::nav_view::BotLink;

    /// A flat square polygon, `size` on a side, its low corner at `at`.
    fn square(at: Vec3, size: f32) -> BotPoly {
        let [x, y, z] = at;
        BotPoly {
            bsp: 0,
            poly: 0,
            verts: vec![
                [x, y, z],
                [x + size, y, z],
                [x + size, y + size, z],
                [x, y + size, z],
            ],
            normal: [0.0, 0.0, 1.0],
            out_of_map: false,
        }
    }

    fn link(from: usize, to: usize, kind: BotLinkKind) -> BotLink {
        BotLink {
            bsp: 0,
            link: 0,
            from,
            to,
            a: [0.0; 3],
            b: [0.0; 3],
            kind,
            one_way: false,
        }
    }

    fn walk(a: usize, b: usize) -> [BotLink; 2] {
        [link(a, b, BotLinkKind::Walk), link(b, a, BotLinkKind::Walk)]
    }

    /// Three 10-unit squares in a row at z 0, walked between both ways:
    /// polygons 0, 1 and 2, x from 0 to 30, y from 0 to 10.
    fn hall() -> BotNav {
        let mut nav = BotNav {
            polys: (0..3)
                .map(|i| square([10.0 * i as f32, 0.0, 0.0], 10.0))
                .collect(),
            ..BotNav::default()
        };
        nav.links.extend(walk(0, 1));
        nav.links.extend(walk(1, 2));
        nav
    }

    fn start(index: usize, x: f32, y: f32, team: Option<u8>) -> Placed {
        Placed {
            index,
            at: [x, y, 0.0],
            team,
        }
    }

    fn census(flags: &[(u16, u16)], starts: &[Placed]) -> Census {
        let rows = MapRows {
            flags: flags
                .iter()
                .map(|&(kind, team)| FlagRow { kind, team })
                .collect(),
            starts: starts.iter().map(|s| s.team.map_or(8, i16::from)).collect(),
            ..MapRows::default()
        };
        Census::of_game(&rows, None).unwrap()
    }

    /// A wall across the hall at x 15, 4 high: two triangles.
    fn wall() -> Vec<[Vec3; 3]> {
        let (a, b, c, d) = (
            [15.0, -1.0, 0.0],
            [15.0, 11.0, 0.0],
            [15.0, 11.0, 4.0],
            [15.0, -1.0, 4.0],
        );
        vec![[a, b, c], [a, c, d]]
    }

    fn layout<'a>(nav: &'a BotNav, starts: Vec<Placed>, flags: &'a [(u16, u16)]) -> Layout<'a> {
        Layout {
            census: census(flags, &starts),
            starts,
            items: Vec::new(),
            flags: Vec::new(),
            mesh: Ok(nav),
            blockers: wall(),
        }
    }

    fn kinds(r: &Report) -> Vec<Kind> {
        r.problems.iter().map(|p| p.kind).collect()
    }

    #[test]
    fn a_walled_hall_with_two_starts_is_ready_for_slayer() {
        let nav = hall();
        let l = layout(
            &nav,
            vec![start(0, 5.0, 5.0, None), start(1, 25.0, 5.0, None)],
            &[],
        );
        let r = check(&l);
        assert!(r.ok(), "{:?}", r.problems);
        let slayer = r.ready_for("Slayer").unwrap();
        assert!(slayer.ok, "{slayer:?}");
        assert_eq!(slayer.engine, 2);
        assert_eq!(r.ready.len(), ::rules::builtin::ALL.len());
    }

    #[test]
    fn an_off_mesh_start_item_and_flag_are_each_found() {
        let nav = hall();
        let mut l = layout(
            &nav,
            vec![start(0, 5.0, 5.0, None), start(1, 50.0, 5.0, None)],
            &[],
        );
        l.items.push(Placed {
            index: 0,
            at: [5.0, 5.0, 3.0],
            team: None,
        });
        l.flags.push(Placed {
            index: 2,
            at: [5.0, -4.0, 0.0],
            team: None,
        });
        // On the floor, just over it and just under it: on the mesh.
        l.items.push(Placed {
            index: 1,
            at: [12.0, 5.0, 0.5],
            team: None,
        });
        l.items.push(Placed {
            index: 2,
            at: [12.0, 5.0, -0.05],
            team: None,
        });
        let r = check(&l);
        let off: Vec<_> = r.of(Kind::OffMesh).map(|p| p.target).collect();
        assert_eq!(
            off,
            [
                Some(Target::Placement(BlockKind::StartingLocations, 1)),
                Some(Target::Placement(BlockKind::NetgameEquipment, 0)),
                Some(Target::Placement(BlockKind::NetgameFlags, 2)),
            ]
        );
        // One start on the mesh is too few for any game type.
        let slayer = r.ready_for("Slayer").unwrap();
        assert!(!slayer.ok);
        assert!(slayer.missing[0].contains("it has 1"), "{slayer:?}");
    }

    #[test]
    fn an_island_and_a_start_on_it_are_found() {
        let mut nav = hall();
        // Polygon 3: a square off to the side, linked to nothing.
        nav.polys.push(square([0.0, 40.0, 0.0], 5.0));
        // Polygon 4: a sliver too small to report.
        nav.polys.push(square([0.0, 60.0, 0.0], 0.5));
        let l = layout(
            &nav,
            vec![start(0, 5.0, 5.0, Some(0)), start(1, 25.0, 5.0, Some(0))],
            &[],
        );
        let r = check(&l);
        assert_eq!(kinds(&r), [Kind::Island]);
        assert_eq!(r.problems[0].target, Some(Target::Poly(3)));
        assert!(
            r.problems[0].text.contains("25.0 square units"),
            "{}",
            r.problems[0].text
        );

        // A third start on the island: it reaches no one, no one reaches
        // it, and the island is reached.
        let l = layout(
            &nav,
            vec![
                start(0, 5.0, 5.0, Some(0)),
                start(1, 25.0, 5.0, Some(0)),
                start(2, 2.0, 42.0, Some(0)),
            ],
            &[],
        );
        let r = check(&l);
        let unreachable: Vec<_> = r.of(Kind::Unreachable).map(|p| p.text.as_str()).collect();
        assert_eq!(unreachable.len(), 3, "{unreachable:?}");
        assert!(unreachable[0].contains("cannot reach starting location 2"));
        assert!(unreachable[2].contains("cannot reach starting locations 0, 1"));
        assert_eq!(r.of(Kind::Island).count(), 0);
    }

    #[test]
    fn a_drop_with_no_way_back_is_found_once() {
        let mut nav = hall();
        // A pit under polygon 2 that two drops lead into.
        nav.polys.push(square([20.0, 0.0, -3.0], 10.0));
        nav.links.push(link(2, 3, BotLinkKind::Drop));
        nav.links.push(link(1, 3, BotLinkKind::Drop));
        // A ledge dropped from and jumped back to is no trap.
        nav.polys.push(square([0.0, 10.0, 1.0], 10.0));
        nav.links.push(link(4, 0, BotLinkKind::Drop));
        nav.links.push(link(0, 4, BotLinkKind::Jump));
        let l = layout(
            &nav,
            vec![start(0, 5.0, 5.0, None), start(1, 25.0, 5.0, None)],
            &[],
        );
        let r = check(&l);
        let traps: Vec<_> = r.of(Kind::OneWay).collect();
        assert_eq!(traps.len(), 1, "{traps:?}");
        assert_eq!(traps[0].target, Some(Target::Poly(3)));
        assert!(
            traps[0].text.starts_with("a drop link"),
            "{}",
            traps[0].text
        );
    }

    #[test]
    fn opposing_starts_in_sight_are_found_and_a_wall_or_a_team_hides_them() {
        let nav = hall();
        // 0 and 1 are on one side of the wall at x 15, 2 on the other.
        let starts = vec![
            start(0, 5.0, 5.0, Some(0)),
            start(1, 12.0, 5.0, Some(1)),
            start(2, 25.0, 5.0, Some(1)),
        ];
        let r = check(&layout(&nav, starts.clone(), &[]));
        let seen: Vec<_> = r.of(Kind::InSight).map(|p| p.target).collect();
        assert_eq!(
            seen,
            [Some(Target::Placement(BlockKind::StartingLocations, 0))]
        );
        assert!(
            r.problems[0].text.contains("0 and 1"),
            "{}",
            r.problems[0].text
        );

        // The same team: no problem.
        let mut same = starts.clone();
        same[1].team = Some(0);
        assert_eq!(check(&layout(&nav, same, &[])).of(Kind::InSight).count(), 0);

        // With no wall, 0 sees 2 as well; 1 and 2 are one team.
        let mut l = layout(&nav, starts, &[]);
        l.blockers.clear();
        assert_eq!(check(&l).of(Kind::InSight).count(), 2);
        assert!(clear(&[], [0.0; 3], [1.0, 0.0, 0.0]));
        assert!(!clear(&wall(), [5.0, 5.0, 1.0], [25.0, 5.0, 1.0]));
    }

    #[test]
    fn a_missing_flag_keeps_ctf_and_assault_from_ready() {
        let nav = hall();
        let starts = vec![start(0, 5.0, 5.0, Some(0)), start(1, 25.0, 5.0, Some(1))];
        // One flag: an Assault bomb spawn (type 2) of team 1.
        let r = check(&layout(&nav, starts.clone(), &[(2, 1)]));
        let ctf = r.ready_for("Capture the Flag").unwrap();
        assert!(!ctf.ok);
        assert_eq!(
            ctf.missing,
            [
                "a ctf_flag_spawn flag for team 0",
                "a ctf_flag_spawn flag for team 1"
            ]
        );
        let assault = r.ready_for("Assault").unwrap();
        assert!(!assault.ok);
        assert!(
            assault
                .missing
                .iter()
                .any(|m| m.contains("assault_bomb_spawn flag for team 0"))
        );
        assert!(
            !assault
                .missing
                .iter()
                .any(|m| m.contains("assault_bomb_spawn flag for team 1"))
        );
        assert!(r.ready_for("Slayer").unwrap().ok);

        // Each team's flag spawn (type 0): CTF is ready.
        let r = check(&layout(&nav, starts.clone(), &[(0, 0), (0, 1)]));
        assert!(r.ready_for("Capture the Flag").unwrap().ok, "{:?}", r.ready);

        // With every start on team 1, team 0 has none.
        let mut one_side = starts;
        one_side[0].team = Some(1);
        let r = check(&layout(&nav, one_side, &[(0, 0), (0, 1)]));
        assert_eq!(
            r.ready_for("Capture the Flag").unwrap().missing,
            ["a starting location for team 0"]
        );
    }

    #[test]
    fn with_no_mesh_nothing_is_ready_and_it_says_why() {
        let nav = BotNav::default();
        let mut l = layout(&nav, vec![start(0, 5.0, 5.0, None)], &[]);
        assert_eq!(kinds(&check(&l)), [Kind::NoMesh]);
        l.mesh = Err("it is loading".into());
        let r = check(&l);
        assert!(r.problems[0].text.ends_with("it is loading"));
        assert!(
            r.ready
                .iter()
                .all(|g| !g.ok && g.missing.iter().any(|m| m == "the bots' mesh"))
        );
    }

    #[test]
    fn kind_names_are_the_apis() {
        let names: Vec<_> = Kind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(
            names,
            [
                "no_mesh",
                "off_mesh",
                "unreachable",
                "island",
                "one_way",
                "in_sight"
            ]
        );
    }
}
