//! Generate the engine sectors and choose drop zones on the same polygons.
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

pub(crate) struct Navigation {
    /// Elements for the pathfinding block of each structure BSP, in scenario order.
    pub bsps: Vec<(TagId, Block)>,
    pub zones: Vec<Zone>,
}

const MIN_ZONES: usize = 6;
const MAX_ZONES: usize = 12;
const SPAWN_CLEARANCE: f32 = 2.0;
const BODY_SPACING: f32 = 0.8;

pub(crate) fn prepare<R: Read + Seek>(map: &mut Map<R>) -> Result<Navigation> {
    let scenario = map.scenario().context("map has no scenario")?;
    let scnr = map.read_tag_bytes(scenario.id)?;
    let starts = metadata_block(map, &scnr, STARTING_LOCATIONS as usize, STARTING_LOCATION)?
        .into_iter()
        .map(|row| {
            std::array::from_fn(|k| f32::from_le_bytes(row[k * 4..k * 4 + 4].try_into().unwrap()))
        })
        .collect::<Vec<V3>>();
    ensure!(
        starts.iter().flatten().all(|x| x.is_finite()),
        "non-finite multiplayer start"
    );

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
        if let Some(arena) = choose_zones(&pf, &data, &starts, index.try_into()?, cluster)
            && best
                .as_ref()
                .is_none_or(|b: &Arena| arena.rank() > b.rank())
        {
            best = Some(arena);
        }
        let element = pathfind::build::element(&pf).map_err(anyhow::Error::msg)?;
        bsps.push((id, element));
    }
    ensure!(!bsps.is_empty(), "scenario has no structure BSP");
    let arena = best.context("need six distinct drop zones off the player starts with a walkable path to a common middle")?;
    Ok(Navigation {
        bsps,
        zones: arena.zones,
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

fn choose_zones(
    pf: &PathfindingData,
    data: &MapData,
    starts: &[V3],
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
    let mut visited = vec![false; graph.len()];
    let mut best: Option<Arena> = None;
    for root in 0..graph.len() {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        let mut stack = vec![root];
        let mut component = Vec::new();
        while let Some(k) = stack.pop() {
            component.push(k);
            for &next in &graph[k] {
                if !visited[next] {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
        let players = component.iter().map(|&k| spawn_counts[k]).sum::<usize>();
        // Only player-accessible geometry may supply a drop, even if an
        // isolated decorative island has more boundary polygons.
        if players == 0
            || component.len() < MIN_ZONES
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
        let mut candidates: Vec<_> = component
            .iter()
            .copied()
            .filter(|&k| edge[k] && center(k) != middle)
            .collect();
        let sample = ZoneSamples {
            data,
            starts,
            bsp,
            middle,
            cluster: &cluster,
        };
        let mut selected = vec![];
        let mut zones = vec![];
        while !candidates.is_empty() && zones.len() < MAX_ZONES {
            // Spread the drops around the outside of the component. Equal
            // scores use the lower on-disk sector index on every machine.
            let score = |k| {
                selected.iter().fold(distance2(center(k), middle), |d, &p| {
                    d.min(distance2(center(k), p))
                })
            };
            candidates
                .sort_unstable_by(|a, b| score(*a).total_cmp(&score(*b)).then_with(|| b.cmp(a)));
            // A rejected polygon cannot become eligible as more drops are
            // added. Discard it before ranking for the next drop.
            while let Some(k) = candidates.pop() {
                if selected.iter().any(|&p| distance2(center(k), p) < 1.) {
                    continue;
                }
                if let Some(zone) = sample.make_zone(k, &graph[k], zones.len()) {
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
            });
        }
    }
    best
}

struct ZoneSamples<'a, C> {
    data: &'a MapData,
    starts: &'a [V3],
    bsp: i16,
    middle: V3,
    cluster: &'a C,
}

impl<C: Fn(V3) -> Option<i16>> ZoneSamples<'_, C> {
    fn make_zone(&self, sector: usize, neighbours: &[usize], ordinal: usize) -> Option<Zone> {
        let data = self.data;
        let poly = &data.nav.polys[sector];
        let opts = Options::default();
        let mut positions: Vec<FiringPosition> = vec![];
        // Sample the interior of the drop polygon and its immediately
        // connected neighbours. Two separated points permit two actors to
        // be placed together even when the sector has no neighbour nearby.
        let sample = |k: usize, p: V3| -> Option<FiringPosition> {
            if self
                .starts
                .iter()
                .any(|&s| distance2(p, s) < SPAWN_CLEARANCE.powi(2))
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
        positions.push(sample(sector, poly.center)?);
        let mut others = poly
            .verts
            .iter()
            .map(|v| {
                (
                    sector,
                    std::array::from_fn(|j| (v[j] + poly.center[j]) * 0.5),
                )
            })
            .chain(neighbours.iter().map(|&k| (k, data.nav.polys[k].center)))
            .filter(|(_, p)| distance2(*p, poly.center) <= 4.0f32.powi(2))
            .filter_map(|(k, p)| sample(k, p))
            .collect::<Vec<_>>();
        // Prefer cover with a nearby wall on the side facing the middle.
        let cover = |p: V3| {
            let toward = sub(self.middle, p);
            let scale = 2.0 / libm::hypotf(toward[0], toward[1]).max(2.0);
            let from = [p[0], p[1], p[2] + 0.5];
            let to = [
                from[0] + toward[0] * scale,
                from[1] + toward[1] * scale,
                from[2],
            ];
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
                if positions.len() == 4 {
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
            name: "drop".into(),
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
            name: format!("ff_drop_{ordinal:02}"),
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
