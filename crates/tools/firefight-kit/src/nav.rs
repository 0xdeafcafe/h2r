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

use crate::bases::{self, Bases, Config};

pub(crate) struct Navigation {
    /// Elements for the pathfinding block of each structure BSP, in scenario order.
    pub bsps: Vec<(TagId, Block)>,
    /// The drop zones, `ff_drop_NN`.
    pub zones: Vec<Zone>,
    /// One zone per base, `ff_base_NN`, in base order.
    pub base_zones: Vec<Zone>,
    pub bases: Bases,
    /// The starting location rows, each team designator rewritten to the
    /// index of its nearest base.
    pub starts: Vec<Vec<u8>>,
}

const MIN_ZONES: usize = 6;
const MAX_ZONES: usize = 12;
/// A firing position keeps this far from every player start.
const SPAWN_CLEARANCE: f32 = 2.0;
/// A drop's polygon centre keeps this far from every player start.
const DROP_START_CLEARANCE: f32 = 4.0;
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

pub(crate) fn prepare<R: Read + Seek>(map: &mut Map<R>, config: &Config) -> Result<Navigation> {
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
    let found = bases::find(&rows, &flags, config.home).context(
        "the map has no two bases: no CTF flag stand or starting location for each of teams 0 and 1, and no two distinct starting locations",
    )?;
    let teams = bases::start_teams(&found.points, &rows);
    let rewritten = start_rows
        .into_iter()
        .zip(teams)
        .map(|(mut row, team)| {
            row[bases::START_TEAM..bases::START_TEAM + 2].copy_from_slice(&team.to_le_bytes());
            row
        })
        .collect();

    let mut bsps = Vec::new();
    let mut best = None;
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
        ) && best
            .as_ref()
            .is_none_or(|b: &Arena| arena.rank() > b.rank())
        {
            best = Some(arena);
        }
        let element = pathfind::build::element(&pf).map_err(anyhow::Error::msg)?;
        bsps.push((id, element));
    }
    ensure!(!bsps.is_empty(), "scenario has no structure BSP");
    let arena = best.with_context(|| {
        format!(
            "need six distinct drop zones off the player starts and both bases ({:?} and {:?}, from {:?}), with a walkable path to each base",
            found.points[0], found.points[1], found.source
        )
    })?;
    Ok(Navigation {
        bsps,
        zones: arena.zones,
        base_zones: arena.bases,
        bases: found,
        starts: rewritten,
    })
}

fn distance2(a: V3, b: V3) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}

struct Arena {
    starts: usize,
    sectors: usize,
    zones: Vec<Zone>,
    bases: Vec<Zone>,
}

impl Arena {
    fn rank(&self) -> (usize, usize) {
        (self.starts, self.sectors)
    }
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
) -> Option<Arena> {
    let (graph, edge) = walk_graph(pf, &data.nav);
    let mut spawn_counts = vec![0usize; graph.len()];
    for &start in starts {
        if let Some(k) = data.nav.poly_near(start) {
            spawn_counts[k as usize] += 1;
        }
    }
    let base_polys = bases
        .iter()
        .map(|&b| {
            data.nav
                .poly_near(b)
                .map(|k| k as usize)
                .filter(|&k| k < graph.len())
        })
        .collect::<Option<Vec<usize>>>()?;
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
        let sample = ZoneSamples {
            data,
            starts,
            bsp,
            cluster: &cluster,
        };
        let Some(base_zones) = base_polys
            .iter()
            .enumerate()
            .map(|(i, &k)| sample.base_zone(i, k, bases[i], &graph))
            .collect::<Option<Vec<Zone>>>()
        else {
            continue;
        };
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
        let side = |k: usize| bases::nearest(bases, center(k));
        let mut selected = vec![];
        let mut zones = vec![];
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
        if zones.len() >= MIN_ZONES {
            best = Some(Arena {
                starts: players,
                sectors: component.len(),
                zones,
                bases: base_zones,
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
        let centre = sample(sector, poly.center);
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

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::test_floor;
