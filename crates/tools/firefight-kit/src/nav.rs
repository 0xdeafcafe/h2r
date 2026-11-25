//! Generate the engine sectors and choose drop zones and base zones on the
//! same polygons.
use std::io::{Read, Seek};

use anyhow::{Context, Result, ensure};
use bots::cache::{MapData, build_with_pathfinding};
use bots::nav::NavMesh;
use h1campaign::ai::{
    convert::Options,
    h2::{Area, FiringPosition, Zone},
};
use h1campaign::scenario::{BSP, BSPS, STARTING_LOCATION, STARTING_LOCATIONS};
use map::{Map, TagId, build::Block};
use pathfind::geom::{V3, dot, sub};
use scenario_edit::scene::metadata_block;
use tags::{
    collision::Bsp,
    geometry::TagView,
    pathfinding::{PathfindingData, SBSP_COLLISION_FIELD, SBSP_PATHFINDING_FIELD},
    structure::InstancedGeometryInstance,
};

use crate::bases::{self, Bases, Config, Team};
use crate::ground::{Labels, Marker, Problem};

pub(crate) struct Navigation {
    /// Elements for the pathfinding block of each structure BSP, in scenario order.
    pub bsps: Vec<(TagId, Block)>,
    /// Sectors generated and kept, summed over the BSPs.
    pub trim: Trim,
    /// The drop zones, `ff_drop_NN`.
    pub zones: Vec<Zone>,
    /// One zone per base, `ff_base_NN`, in base order.
    pub base_zones: Vec<Zone>,
    pub bases: Bases,
    /// The starting location rows, each team designator rewritten to the
    /// index of its nearest base.
    pub starts: Vec<Vec<u8>>,
    /// Placed labels the ground cannot honour.
    pub problems: Vec<Problem>,
}

const MIN_ZONES: usize = 6;
const MAX_ZONES: usize = 12;
/// A firing position keeps this far from every player start.
const SPAWN_CLEARANCE: f32 = 2.0;
/// A drop's polygon centre keeps this far from every player start.
const DROP_START_CLEARANCE: f32 = 3.0;
/// A drop's polygon centre keeps a quarter of the bases' separation from
/// each base, within these bounds.
const BASE_CLEARANCE: (f32, f32) = (2.0, 8.0);
const BODY_SPACING: f32 = 0.8;
/// A drop zone samples its polygon and its neighbours within this radius.
const DROP_RADIUS: f32 = 4.0;
const DROP_POSITIONS: usize = 4;
/// A base zone samples polygons up to two links out within this radius.
const BASE_RADIUS: f32 = 8.0;
const BASE_POSITIONS: usize = 8;

pub(crate) fn prepare<R: Read + Seek>(
    map: &mut Map<R>,
    config: &Config,
    labels: &Labels,
) -> Result<Navigation> {
    let scenario = map.scenario().context("map has no scenario")?;
    let scnr = map.read_tag_bytes(scenario.id)?;
    let start_rows = metadata_block(map, &scnr, STARTING_LOCATIONS as usize, STARTING_LOCATION)?;
    let rows: Vec<bases::StartRow> = start_rows.iter().map(|r| bases::start_row(r)).collect();
    let starts: Vec<V3> = rows.iter().map(|r| r.position).collect();
    ensure!(
        starts.iter().flatten().all(|x| x.is_finite()),
        "non-finite multiplayer start"
    );
    let flags: Vec<bases::FlagRow> = metadata_block(
        map,
        &scnr,
        bases::NETGAME_FLAGS as usize,
        bases::NETGAME_FLAG,
    )?
    .iter()
    .map(|r| bases::flag_row(r))
    .collect();
    let found = if labels.bases.is_empty() {
        bases::find(&rows, &flags, config.home).context(
            "the map has no two bases: no CTF flag stand or starting location for each of teams 0 and 1, and no two distinct starting locations",
        )?
    } else {
        let team = |t: Team| {
            labels
                .bases
                .iter()
                .find(|b| b.team == t)
                .map(|b| b.position)
        };
        let (Some(red), Some(blue)) = (team(Team::Red), team(Team::Blue)) else {
            anyhow::bail!("ff_base: place one base for each of the two teams, or none");
        };
        bases::from_labels(red, blue, config.home)
            .context("the two ff_base labels are less than a unit apart")?
    };
    let teams = bases::start_teams(&found.points, &rows);
    let rewritten = start_rows
        .into_iter()
        .zip(teams)
        .map(|(mut row, team)| {
            row[bases::START_TEAM..bases::START_TEAM + 2].copy_from_slice(&team.to_le_bytes());
            row
        })
        .collect();

    let mut generated = Vec::new();
    let mut best = None;
    let mut why = Vec::new();
    for (index, row) in metadata_block(map, &scnr, BSPS as usize, BSP)?
        .into_iter()
        .enumerate()
    {
        let id = TagId(u32::from_le_bytes(row[0x14..0x18].try_into().unwrap()));
        let loc = map.tag(id)?.location.context("BSP missing its payload")?;
        let bytes = map.read_tag_bytes(id)?;
        let v = TagView::new(&bytes, loc.address);
        let root = v.at(0, 0x148)?;
        ensure!(
            root[SBSP_PATHFINDING_FIELD..SBSP_PATHFINDING_FIELD + 4] == [0; 4],
            "BSP {index} already has pathfinding"
        );
        let coll = v.block(root, SBSP_COLLISION_FIELD, Bsp::STRUCTURE_SIZE)?;
        let [record] = coll.as_slice() else {
            anyhow::bail!("expected one collision BSP in structure {index}");
        };
        let at = record.as_ptr() as usize - bytes.as_ptr() as usize;
        let collision = Bsp::decode_structure(&bytes, loc.address, at)?;
        let checksum = u32::from_le_bytes(root[8..12].try_into().unwrap());
        let instances = v.block(root, 0x140, InstancedGeometryInstance::SIZE)?.len();
        let (pf, data) = build_with_pathfinding(&collision, instances, checksum)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("generating sectors for BSP {index}"))?;
        let leaves = v.block(root, 0x2c, 8)?;
        let cluster = |mut p: V3| {
            p[2] += 0.01;
            let leaf = h1convert::query::leaf_at(&collision, p)?;
            let row = leaves.get(leaf)?;
            let cluster = i16::from_le_bytes(row[..2].try_into().unwrap());
            (0..512).contains(&cluster).then_some(cluster)
        };
        if let Some(arena) = choose_zones(
            &pf,
            &data,
            &starts,
            &found.points,
            index.try_into()?,
            cluster,
            labels,
            &mut |w| why.push(format!("BSP {index}: {w}")),
        ) && best
            .as_ref()
            .is_none_or(|b: &Arena| arena.rank() > b.rank())
        {
            best = Some(arena);
        }
        generated.push((id, pf, data));
    }
    ensure!(!generated.is_empty(), "scenario has no structure BSP");
    let mut arena = best.with_context(|| {
        format!(
            "need six distinct drop zones off the player starts and both bases ({:?} and {:?}, from {:?}), with a walkable path to each base: {}",
            found.points[0],
            found.points[1],
            found.source,
            if why.is_empty() { "no walkable component holds a player start".into() } else { why.join("; ") }
        )
    })?;
    // Keep only the sectors the AI can use, then encode each BSP's.
    let mut bsps = Vec::with_capacity(generated.len());
    let mut trim = Trim::default();
    let mut problems = std::mem::take(&mut arena.problems);
    for (index, (id, pf, mut data)) in generated.into_iter().enumerate() {
        bots::autonav::mark_in_map(map, &mut data)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("marking BSP {index}'s walkable area"))?;
        let on_arena = usize::try_from(arena.bsp).ok() == Some(index);
        let seeds = if on_arena {
            arena.sectors()
        } else {
            Vec::new()
        };
        let keep = usable_sectors(&pf, &data.nav, &seeds);
        let (kept, sector_of) =
            pathfind::trim::keep_sectors(&pf, &keep).map_err(anyhow::Error::msg)?;
        if on_arena {
            arena.renumber(&sector_of)?;
            problems.extend(siege_problems(labels, &data.nav, &sector_of));
        }
        trim.generated += pf.sectors.len();
        trim.kept += kept.sectors.len();
        let element = pathfind::build::element(&kept).map_err(anyhow::Error::msg)?;
        bsps.push((id, element));
    }
    Ok(Navigation {
        bsps,
        trim,
        zones: arena.zones,
        base_zones: arena.bases,
        bases: found,
        starts: rewritten,
        problems,
    })
}

/// The siege labels that are not on the ground the AI keeps. The scripts
/// read these places; the ground only has to hold them.
fn siege_problems(labels: &Labels, nav: &NavMesh, sector_of: &[u16]) -> Vec<Problem> {
    let on_ground = |p: V3| {
        nav.poly_near(p)
            .and_then(|k| sector_of.get(k as usize))
            .is_some_and(|&k| k != pathfind::trim::NONE)
    };
    let mut problems = Vec::new();
    let mut check = |label: &'static str, index: usize, position: V3| {
        if !on_ground(position) {
            problems.push(Problem {
                label,
                index,
                position,
                reason: "not on the AI ground",
            });
        }
    };
    for (i, &p) in labels.sites.iter().enumerate() {
        check("siege_site", i, p);
    }
    for (i, &p) in labels.goals.iter().enumerate() {
        check("siege_goal", i, p);
    }
    for (i, l) in labels.lines.iter().enumerate() {
        check("siege_line", i, l.position);
    }
    problems
}

fn distance2(a: V3, b: V3) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}

struct Arena {
    /// The structure BSP the zones are on.
    bsp: i16,
    starts: usize,
    sectors: usize,
    zones: Vec<Zone>,
    bases: Vec<Zone>,
    /// Placed drops and covers this arena cannot honour.
    problems: Vec<Problem>,
}

impl Arena {
    fn rank(&self) -> (usize, usize) {
        (self.starts, self.sectors)
    }

    fn firing_positions(&mut self) -> impl Iterator<Item = &mut FiringPosition> {
        self.zones
            .iter_mut()
            .chain(&mut self.bases)
            .flat_map(|z| z.firing_positions.iter_mut())
    }

    /// Every sector a firing position is on.
    fn sectors(&mut self) -> Vec<usize> {
        let mut sectors: Vec<usize> = self.firing_positions().map(|p| p.sector as usize).collect();
        sectors.sort_unstable();
        sectors.dedup();
        sectors
    }

    /// Give each firing position its sector's index after trimming.
    fn renumber(&mut self, sector_of: &[u16]) -> Result<()> {
        for p in self.firing_positions() {
            let new = sector_of
                .get(p.sector as usize)
                .copied()
                .filter(|&k| k != pathfind::trim::NONE)
                .context("a firing position's sector was trimmed")?;
            p.sector = u32::from(new);
        }
        Ok(())
    }
}

/// Sectors generated and kept for the AI ([`usable_sectors`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trim {
    pub generated: usize,
    pub kept: usize,
}

/// The sectors the AI can use on one BSP: those the arena's sectors reach
/// over two-sided links (the only links the engine's path search follows)
/// without leaving the map, where the map is what the player starts and
/// weapon pads reach (`bots::autonav::mark_in_map`). Floors the players
/// never reach, decorative islands, sky and terrain past the boundary are
/// out of the map; in-map floors with no engine link to the arena are
/// never on an AI path. With no arena on this BSP, every in-map sector.
/// The arena's own sectors are always kept.
fn usable_sectors(pf: &PathfindingData, nav: &NavMesh, arena: &[usize]) -> Vec<bool> {
    let n = pf.sectors.len();
    let in_map = |k: usize| nav.in_map(k as u32);
    if arena.is_empty() {
        return (0..n).map(in_map).collect();
    }
    let mut graph = vec![vec![]; n];
    for link in &pf.links {
        let (a, b) = (
            usize::from(link.left_sector),
            usize::from(link.right_sector),
        );
        if a < n && b < n && in_map(a) && in_map(b) {
            graph[a].push(b);
            graph[b].push(a);
        }
    }
    let mut keep = vec![false; n];
    let mut stack: Vec<usize> = arena.iter().copied().filter(|&k| k < n).collect();
    for &k in &stack {
        keep[k] = true;
    }
    while let Some(k) = stack.pop() {
        for &next in &graph[k] {
            if !keep[next] {
                keep[next] = true;
                stack.push(next);
            }
        }
    }
    keep
}

/// The retail AI graph has no step, jump or drop links across seams. Keep
/// shared sector portals that also passed the bot mesh's headroom check.
fn walk_graph(pf: &PathfindingData, nav: &NavMesh) -> (Vec<Vec<usize>>, Vec<bool>) {
    let mut graph = vec![vec![]; pf.sectors.len()];
    let mut edge = vec![false; graph.len()];
    for link in &pf.links {
        let (a, b) = (link.left_sector, link.right_sector);
        match (a == u16::MAX, b == u16::MAX) {
            (false, false) => {
                let portal = pf.vertices[usize::from(link.vertex1)];
                let end = pf.vertices[usize::from(link.vertex2)];
                if nav
                    .out(u32::from(a))
                    .any(|e| e.to == u32::from(b) && e.a == portal && e.b == end)
                {
                    graph[usize::from(a)].push(usize::from(b));
                    graph[usize::from(b)].push(usize::from(a));
                }
            }
            (false, true) => edge[usize::from(a)] = true,
            (true, false) => edge[usize::from(b)] = true,
            _ => {}
        }
    }
    (graph, edge)
}

/// How far a drop keeps from each base: a quarter of the closest two
/// bases' separation, within [`BASE_CLEARANCE`].
fn base_clearance(bases: &[V3]) -> f32 {
    let mut closest = f32::INFINITY;
    for (i, &a) in bases.iter().enumerate() {
        for &b in &bases[i + 1..] {
            closest = closest.min(distance2(a, b).sqrt());
        }
    }
    (closest * 0.25).clamp(BASE_CLEARANCE.0, BASE_CLEARANCE.1)
}

/// The point a zone at `p` takes cover from: the nearer base, or the
/// arena's middle on a map without bases.
fn facing(bases: &[V3], middle: V3, p: V3) -> V3 {
    if bases.is_empty() {
        middle
    } else {
        bases[bases::nearest(bases, p)]
    }
}

/// The cover probe from a firing position at `p`: from half a unit up,
/// up to two units level toward `toward`.
fn cover_ray(p: V3, toward: V3) -> (V3, V3) {
    let d = sub(toward, p);
    let scale = 2.0 / libm::hypotf(d[0], d[1]).max(2.0);
    let from = [p[0], p[1], p[2] + 0.5];
    let to = [from[0] + d[0] * scale, from[1] + d[1] * scale, from[2]];
    (from, to)
}

/// The drop zones and, when `bases` are given, one zone per base. A
/// component qualifies only if it holds every base's polygon, so each drop
/// has a walkable path to each base. Drops keep off the starts and off both
/// bases, alternate between the bases' sides, and take cover facing the
/// nearer base.
fn choose_zones(
    pf: &PathfindingData,
    data: &MapData,
    starts: &[V3],
    bases: &[V3],
    bsp: i16,
    cluster: impl Fn(V3) -> Option<i16>,
    placed: &Labels,
    why: &mut dyn FnMut(String),
) -> Option<Arena> {
    let (graph, edge) = walk_graph(pf, &data.nav);
    let mut spawn_counts = vec![0usize; graph.len()];
    for &start in starts {
        if let Some(k) = data.nav.poly_near(start) {
            spawn_counts[k as usize] += 1;
        }
    }
    let Some(base_polys) = bases
        .iter()
        .map(|&b| {
            data.nav
                .poly_near(b)
                .map(|k| k as usize)
                .filter(|&k| k < graph.len())
        })
        .collect::<Option<Vec<usize>>>()
    else {
        why("a base is on no walkable polygon".into());
        return None;
    };
    let clearance = base_clearance(bases);
    let mut owner = vec![usize::MAX; graph.len()];
    let mut best: Option<Arena> = None;
    for root in 0..graph.len() {
        if owner[root] != usize::MAX {
            continue;
        }
        owner[root] = root;
        let mut stack = vec![root];
        let mut component = Vec::new();
        while let Some(k) = stack.pop() {
            component.push(k);
            for &next in &graph[k] {
                if owner[next] == usize::MAX {
                    owner[next] = root;
                    stack.push(next);
                }
            }
        }
        let players = component.iter().map(|&k| spawn_counts[k]).sum::<usize>();
        // Only player-accessible geometry may supply a drop, even if an
        // isolated decorative island has more boundary polygons. Every base
        // must be on it, so each drop has a walkable path to each base.
        if players > 0 && component.len() >= MIN_ZONES {
            let off = base_polys
                .iter()
                .enumerate()
                .filter(|(_, k)| owner[**k] != root)
                .map(|(i, _)| i.to_string())
                .collect::<Vec<_>>();
            if !off.is_empty() {
                why(format!(
                    "the area of {} polygons with {players} starts has no walkable path to base {}",
                    component.len(),
                    off.join(" or ")
                ));
            }
        }
        if players == 0
            || component.len() < MIN_ZONES
            || base_polys.iter().any(|&k| owner[k] != root)
            || best
                .as_ref()
                .is_some_and(|b| b.rank() >= (players, component.len()))
        {
            continue;
        }

        let center = |k: usize| data.nav.polys[k].center;
        let mut lo = center(root);
        let mut hi = lo;
        for p in component.iter().map(|&k| center(k)) {
            for j in 0..3 {
                lo[j] = lo[j].min(p[j]);
                hi[j] = hi[j].max(p[j]);
            }
        }
        let mid = std::array::from_fn(|j| (lo[j] + hi[j]) * 0.5);
        let middle = component
            .iter()
            .map(|&k| center(k))
            .min_by(|a, b| distance2(*a, mid).total_cmp(&distance2(*b, mid)))?;
        let mut problems = Vec::new();
        // Placed covers on this area: the sector each stands on.
        let on_area = |p: V3| {
            data.nav
                .poly_near(p)
                .map(|k| k as usize)
                .filter(|&k| k < graph.len() && owner[k] == root)
        };
        let mut covers = Vec::new();
        for (index, m) in placed.covers.iter().enumerate() {
            match on_area(m.position) {
                Some(k) => covers.push((k, m.position)),
                None => problems.push(Problem {
                    label: "ff_cover",
                    index,
                    position: m.position,
                    reason: "not on the walkable area the bases share",
                }),
            }
        }
        let sample = ZoneSamples {
            data,
            starts,
            bsp,
            cluster: &cluster,
            covers: &covers,
        };
        let Some(base_zones) = base_polys
            .iter()
            .enumerate()
            .map(|(i, &k)| sample.base_zone(i, k, bases[i], &graph))
            .collect::<Option<Vec<Zone>>>()
        else {
            why("a base zone found no firing positions".into());
            continue;
        };
        let mut selected = vec![];
        let mut zones = vec![];
        if !placed.drops.is_empty() {
            // The author's drops, one zone each in label order, the zone's
            // first position the label's own point.
            for (index, m) in placed.drops.iter().enumerate() {
                let reason = match on_area(m.position) {
                    None => "not on the walkable area the bases share",
                    Some(k) => {
                        let toward = [
                            m.position[0] + 2.0 * libm::cosf(m.facing),
                            m.position[1] + 2.0 * libm::sinf(m.facing),
                            m.position[2],
                        ];
                        match sample.placed_drop(k, &graph[k], zones.len(), m, toward) {
                            Some(zone) => {
                                zones.push(zone);
                                continue;
                            }
                            None => "no room for two firing positions",
                        }
                    }
                };
                problems.push(Problem {
                    label: "ff_drop",
                    index,
                    position: m.position,
                    reason,
                });
            }
        }
        let mut candidates: Vec<_> = component
            .iter()
            .copied()
            .filter(|&k| {
                let c = center(k);
                edge[k]
                    && c != middle
                    && starts
                        .iter()
                        .all(|&s| distance2(c, s) >= DROP_START_CLEARANCE.powi(2))
                    && bases.iter().all(|&b| distance2(c, b) >= clearance.powi(2))
            })
            .collect();
        if !placed.drops.is_empty() {
            candidates.clear();
        }
        let side = |k: usize| bases::nearest(bases, center(k));
        while !candidates.is_empty() && zones.len() < MAX_ZONES {
            // Alternate between the bases' sides so each base gets drops,
            // and spread the drops around the outside of the component.
            // Equal scores use the lower on-disk sector index on every machine.
            let wanted = zones.len() % bases.len().max(1);
            let on_side = |k: usize| bases.is_empty() || side(k) == wanted;
            let score = |k| {
                selected.iter().fold(distance2(center(k), middle), |d, &p| {
                    d.min(distance2(center(k), p))
                })
            };
            candidates.sort_unstable_by(|a, b| {
                on_side(*a)
                    .cmp(&on_side(*b))
                    .then_with(|| score(*a).total_cmp(&score(*b)))
                    .then_with(|| b.cmp(a))
            });
            // A rejected polygon cannot become eligible as more drops are
            // added. Discard it before ranking for the next drop.
            while let Some(k) = candidates.pop() {
                if selected.iter().any(|&p| distance2(center(k), p) < 1.) {
                    continue;
                }
                let toward = facing(bases, middle, center(k));
                if let Some(zone) = sample.drop_zone(k, &graph[k], zones.len(), toward) {
                    selected.push(center(k));
                    zones.push(zone);
                    break;
                }
            }
        }
        let wanted = if placed.drops.is_empty() {
            MIN_ZONES
        } else {
            1
        };
        if zones.len() < wanted {
            why(format!(
                "the bases' area has room for {} drop zones of {wanted}",
                zones.len()
            ));
        } else {
            best = Some(Arena {
                bsp,
                starts: players,
                sectors: component.len(),
                zones,
                bases: base_zones,
                problems,
            });
        }
    }
    best
}

struct ZoneSamples<'a, C> {
    data: &'a MapData,
    starts: &'a [V3],
    bsp: i16,
    cluster: &'a C,
    /// Placed `ff_cover` labels on the area: the sector and the point.
    covers: &'a [(usize, V3)],
}

/// How one zone is sampled.
struct Spec {
    name: String,
    area: &'static str,
    /// Where cover faces.
    toward: V3,
    radius: f32,
    positions: usize,
    /// A drop zone: its polygon's centre must be a firing position, and no
    /// position is near a player start. A base zone's are around the starts.
    drop: bool,
    /// A placed drop's own point, in place of its polygon's centre.
    anchor: Option<V3>,
}

impl<C: Fn(V3) -> Option<i16>> ZoneSamples<'_, C> {
    fn drop_zone(
        &self,
        sector: usize,
        neighbours: &[usize],
        ordinal: usize,
        toward: V3,
    ) -> Option<Zone> {
        self.zone(
            sector,
            neighbours,
            &Spec {
                name: format!("ff_drop_{ordinal:02}"),
                area: "drop",
                toward,
                radius: DROP_RADIUS,
                positions: DROP_POSITIONS,
                drop: true,
                anchor: None,
            },
        )
    }

    /// The zone of a placed `ff_drop` at `marker`, on polygon `sector`:
    /// its first position is the marker's point, cover faces `toward`.
    fn placed_drop(
        &self,
        sector: usize,
        neighbours: &[usize],
        ordinal: usize,
        marker: &Marker,
        toward: V3,
    ) -> Option<Zone> {
        self.zone(
            sector,
            neighbours,
            &Spec {
                name: format!("ff_drop_{ordinal:02}"),
                area: "drop",
                toward,
                radius: DROP_RADIUS,
                positions: DROP_POSITIONS,
                drop: true,
                anchor: Some(marker.position),
            },
        )
    }

    /// A zone around base `index` at `point`: its polygon and the polygons
    /// up to two links out, cover facing the base point.
    fn base_zone(
        &self,
        index: usize,
        sector: usize,
        point: V3,
        graph: &[Vec<usize>],
    ) -> Option<Zone> {
        let mut near = vec![];
        for &a in &graph[sector] {
            for k in std::iter::once(a).chain(graph[a].iter().copied()) {
                if k != sector && !near.contains(&k) {
                    near.push(k);
                }
            }
        }
        self.zone(
            sector,
            &near,
            &Spec {
                name: format!("ff_base_{index:02}"),
                area: "base",
                toward: point,
                radius: BASE_RADIUS,
                positions: BASE_POSITIONS,
                drop: false,
                anchor: None,
            },
        )
    }

    fn zone(&self, sector: usize, neighbours: &[usize], spec: &Spec) -> Option<Zone> {
        let data = self.data;
        let poly = &data.nav.polys[sector];
        let opts = Options::default();
        let mut positions: Vec<FiringPosition> = vec![];
        // Sample the interior of the polygon and its connected neighbours.
        // Two separated points permit two actors to be placed together even
        // when the sector has no neighbour nearby.
        let sample = |k: usize, p: V3| -> Option<FiringPosition> {
            if (spec.drop
                && self
                    .starts
                    .iter()
                    .any(|&s| distance2(p, s) < SPAWN_CLEARANCE.powi(2)))
                || !data
                    .walls
                    .clear([p[0], p[1], p[2] + 0.02], [p[0], p[1], p[2] + 0.9])
            {
                return None;
            }
            let n = data.nav.polys[k].normal;
            Some(FiringPosition {
                position: p,
                reference_frame: -1,
                flags: opts.firing_position_flags,
                area: 0,
                cluster: (self.cluster)(p)?,
                sector: k as u32,
                normal: [libm::atan2f(n[1], n[0]), libm::asinf(n[2].clamp(-1., 1.))],
            })
        };
        let mut others = vec![];
        let centre = sample(sector, spec.anchor.unwrap_or(poly.center));
        if spec.drop {
            positions.push(centre?);
        } else {
            others.extend(centre);
        }
        others.extend(
            poly.verts
                .iter()
                .map(|v| {
                    (
                        sector,
                        std::array::from_fn(|j| (v[j] + poly.center[j]) * 0.5),
                    )
                })
                .chain(neighbours.iter().map(|&k| (k, data.nav.polys[k].center)))
                .filter(|(_, p)| distance2(*p, poly.center) <= spec.radius.powi(2))
                .filter_map(|(k, p)| sample(k, p)),
        );
        // Prefer cover: a nearby wall on the side facing `toward`.
        let cover = |p: V3| {
            let (from, to) = cover_ray(p, spec.toward);
            data.walls
                .hit(from, to)
                .is_some_and(|(_, n)| n[2].abs() < 0.7)
        };
        others.sort_by_cached_key(|f| !cover(f.position));
        // A placed cover within reach goes first: the author chose it.
        let placed: Vec<FiringPosition> = self
            .covers
            .iter()
            .filter(|(_, p)| distance2(*p, poly.center) <= spec.radius.powi(2))
            .filter_map(|&(k, p)| sample(k, p))
            .collect();
        others.splice(0..0, placed);
        for p in others {
            if positions
                .iter()
                .all(|q| distance2(q.position, p.position) >= BODY_SPACING.powi(2))
            {
                positions.push(p);
                if positions.len() == spec.positions {
                    break;
                }
            }
        }
        if positions.len() < 2 {
            return None;
        }
        let count = positions.len() as f32;
        let mean =
            std::array::from_fn(|j| positions.iter().map(|p| p.position[j]).sum::<f32>() / count);
        let deviation = (positions
            .iter()
            .map(|p| distance2(p.position, mean))
            .sum::<f32>()
            / count)
            .sqrt();
        let mut clusters = [0; 16];
        for p in &positions {
            let c = p.cluster as usize;
            clusters[c / 32] |= 1 << (c % 32);
        }
        let area = Area {
            name: spec.area.into(),
            flags: 0,
            mean,
            reference_frame: -1,
            deviation,
            first: 0,
            count: positions.len() as i16,
            clusters,
            manual_reference_frame: -1,
        };
        Some(Zone {
            name: spec.name.clone(),
            flags: opts.zone_flags,
            manual_bsp: self.bsp,
            firing_positions: positions,
            areas: vec![area],
        })
    }
}

#[cfg(any(test, feature = "testkit"))]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
