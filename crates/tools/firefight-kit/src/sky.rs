//! Drop pods the kit carries (docs/game/firefight.md, "Drop pods"): the
//! pod tag, two pools of parked pods, a fall for each drop zone with a
//! clear column and one for each player start under open sky.
//!
//! A pod is Halo 2's `insertion_pod` vehicle, copied in by the kit's stage
//! two from a donor campaign map (`transplant`) when the map lacks it. The
//! kit writes
//! - `ff_pod_00` to `ff_pod_03`, parked vehicles for squads,
//! - `ff_ppod_00` to `ff_ppod_07`, parked vehicles for players,
//! - a point set `ff_fall_NN` for each drop zone `ff_drop_NN` whose column
//!   is open, and `ff_pfall_NN` for each starting location NN whose column
//!   is open, each with the points `top` and `ground`.
//!
//! The sky test is a kit-build flag, the same on every machine ([`plan`]):
//! rays from just above the ground to the top of the collision, one straight
//! up and four on a ring round it. A ray is open when it hits nothing or
//! when the first thing it hits is sky ([`Roof`]); the fall is the lower of
//! [`FALL_HEIGHT`] and the way up to the top of the collision, and must be
//! at least [`MIN_FALL`]. A map with no open column gets no pods: nothing
//! can strand a player, since the host layers the pod on a spawn that has
//! already happened.
use std::io::{Read, Seek};

use anyhow::{Context, Result, ensure};
use bots::los::Walls;
use h1campaign::ai::h2::{Point, PointSet, ScenarioAi, layout};
use h1campaign::scenario::{OBJECT_NAME, OBJECT_NAMES, PALETTE, UNIQUE_ID_HIGH};
use map::{FourCC, Map, TagId, build::{Block, TagSet}};
use pathfind::geom::V3;
use scenario_edit::scene::metadata_block;
use serde::Serialize;
use tags::collision::Bsp;

/// Halo 2's insertion pod, one `vehi` tag for squads and players.
pub const POD: &str = r"objects\vehicles\insertion_pod\insertion_pod";
/// Parked squad pods, `ff_pod_NN`.
pub const POD_PREFIX: &str = "ff_pod_";
/// Parked player pods, `ff_ppod_NN`.
pub const PLAYER_POD_PREFIX: &str = "ff_ppod_";
/// A zone's fall, the point set `ff_fall_NN` for `ff_drop_NN`.
pub const FALL_PREFIX: &str = "ff_fall_";
/// A start's fall, the point set `ff_pfall_NN` for starting location NN.
pub const PLAYER_FALL_PREFIX: &str = "ff_pfall_";
/// The points of a fall, in this order.
pub const POINTS: [&str; 2] = ["top", "ground"];
/// Pods kept for squads and for players.
pub const SQUAD_PODS: usize = 4;
pub const PLAYER_PODS: usize = 8;

const VEHI: FourCC = FourCC(*b"vehi");
/// The rays start this far above the ground.
const LIFT: f32 = 0.2;
/// The four ring rays are this far from the middle one.
pub const RING: f32 = 0.6;
/// A fall is at most this tall.
pub const FALL_HEIGHT: f32 = 40.0;
/// A fall under this is no fall.
pub const MIN_FALL: f32 = 12.0;
/// The pods' placements' unique ID low words: past the ship's.
const UNIQUE_LOW: u32 = 0xff60;

/// A pod's way down: where it starts and where it lands (a firing
/// position, or a start).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Fall {
    pub top: V3,
    pub ground: V3,
}

/// What the collision puts over a point: the solid surfaces, and the sky
/// surfaces, which a ray passes.
pub struct Roof<'a> {
    pub solid: &'a Walls,
    pub sky: &'a Walls,
}

impl Roof<'_> {
    /// The highest point of any triangle, none for an empty collision.
    fn top(&self) -> Option<f32> {
        let top = self
            .solid
            .tris
            .iter()
            .chain(&self.sky.tris)
            .flatten()
            .map(|v| v[2])
            .fold(f32::NEG_INFINITY, f32::max);
        top.is_finite().then_some(top)
    }

    /// Whether the first thing between `a` and `b` is sky, or nothing.
    fn open(&self, a: V3, b: V3) -> bool {
        match self.solid.hit(a, b) {
            None => true,
            Some((solid, _)) => self.sky.hit(a, b).is_some_and(|(sky, _)| sky < solid),
        }
    }
}

/// The structure BSP's materials block (`sbsp+0xa4`), 0x20 bytes an element:
/// the material's shader is the tag reference at `+8`, whose datum is at
/// `+0xc`. Collision surfaces index it by `Surface::material`.
pub(crate) const MATERIALS: (usize, usize) = (0xa4, 0x20);

/// The material indices whose shader is a sky: `names` is each material's
/// shader tag name, in material order. A shader is a sky when a component
/// of its path is `sky`, or its last one is `sky` or starts with `sky_`.
/// Read from the retail multiplayer maps (Coagulation, Zanzibar, Beaver
/// Creek, Lockout, Warlock): none of their collision uses such a shader, so
/// the set is empty there and open sky is a ray that hits nothing. A map
/// that does mark sky this way gets the ray's pass through it.
pub(crate) fn sky_materials<'a>(names: impl IntoIterator<Item = Option<&'a str>>) -> Vec<u16> {
    names
        .into_iter()
        .enumerate()
        .filter(|(_, name)| {
            name.is_some_and(|n| {
                let mut parts: Vec<&str> = n.split('\\').collect();
                let last = parts.pop().unwrap_or_default();
                parts.contains(&"sky") || last == "sky" || last.starts_with("sky_")
            })
        })
        .filter_map(|(i, _)| u16::try_from(i).ok())
        .collect()
}

/// `bsp`'s surfaces split by material: the solid ones and the ones whose
/// material index is in `sky`.
pub(crate) fn split(bsp: &Bsp, sky: &[u16]) -> (Walls, Walls) {
    let (mut solid, mut open) = (Vec::new(), Vec::new());
    for (s, surface) in bsp.surfaces.iter().enumerate() {
        let ring: Vec<V3> = bsp
            .surface_ring(s)
            .into_iter()
            .filter_map(|v| bsp.vertices.get(v).map(|v| v.point))
            .collect();
        if sky.contains(&surface.material) {
            open.push(ring);
        } else {
            solid.push(ring);
        }
    }
    let walls = |rings: Vec<Vec<V3>>| Walls::from_polygons(rings.iter().map(Vec::as_slice));
    (walls(solid), walls(open))
}

/// The fall over each of `grounds`, none where the column is closed or
/// short. The same collision gives the same falls.
pub(crate) fn plan(roof: &Roof, grounds: &[V3]) -> Vec<Option<Fall>> {
    let Some(world) = roof.top() else {
        return vec![None; grounds.len()];
    };
    grounds
        .iter()
        .map(|&ground| {
            if !ground.iter().all(|x| x.is_finite()) {
                return None;
            }
            let height = FALL_HEIGHT.min(world - ground[2]);
            if height < MIN_FALL {
                return None;
            }
            let offsets = [[0.0, 0.0], [RING, 0.0], [-RING, 0.0], [0.0, RING], [0.0, -RING]];
            offsets
                .iter()
                .all(|[dx, dy]| {
                    roof.open(
                        [ground[0] + dx, ground[1] + dy, ground[2] + LIFT],
                        [ground[0] + dx, ground[1] + dy, world],
                    )
                })
                .then_some(Fall {
                    top: [ground[0], ground[1], ground[2] + height],
                    ground,
                })
        })
        .collect()
}

/// The falls of a build, by drop zone and by starting location.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Falls {
    pub zones: Vec<Option<Fall>>,
    pub starts: Vec<Option<Fall>>,
}

impl Falls {
    pub(crate) fn zone_count(&self) -> usize {
        self.zones.iter().flatten().count()
    }

    pub(crate) fn start_count(&self) -> usize {
        self.starts.iter().flatten().count()
    }
}

/// What a build says of the pods (`Package::pods`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PodReport {
    pub kept: bool,
    /// The pod's tag name when kept.
    pub tag: String,
    /// Tags the transplant copied in (0 when the map held the pod).
    pub tags: usize,
    /// Tag memory the pods cost: the copied tags and what the pools,
    /// falls and their names add.
    pub bytes: usize,
    /// Drop zones with a fall.
    pub zone_falls: usize,
    /// Starting locations under open sky, and all of them.
    pub open_starts: usize,
    pub starts: usize,
    /// Why they were cut.
    pub why: Option<String>,
}

impl PodReport {
    pub(crate) fn cut(why: impl Into<String>) -> Self {
        Self {
            kept: false,
            tag: String::new(),
            tags: 0,
            bytes: 0,
            zone_falls: 0,
            open_starts: 0,
            starts: 0,
            why: Some(why.into()),
        }
    }
}

impl std::fmt::Display for PodReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.kept {
            write!(
                f,
                "pods: kept ({}, {} tags, {} bytes, {} zone falls); pod starts: {} of {} open",
                self.tag, self.tags, self.bytes, self.zone_falls, self.open_starts, self.starts
            )
        } else {
            write!(f, "pods: cut ({})", self.why.as_deref().unwrap_or("not built"))
        }
    }
}

/// Where the build gets its pod.
pub(crate) enum Pick {
    /// A tag the map has.
    Have(TagId, String),
    /// A tag to copy from a donor, by name.
    Copy(&'static str),
}

pub(crate) fn pick(set: &TagSet) -> Pick {
    match set.find(VEHI, POD) {
        Some(id) => Pick::Have(id, POD.to_owned()),
        None => Pick::Copy(POD),
    }
}

/// The scenario blocks the pods add, `(field, count, block)` replacing the
/// ship's of the same field, and what they weigh.
pub(crate) struct Authored {
    pub bytes: usize,
}

fn name32(name: &str) -> Block {
    let mut b = Block::zeroed(OBJECT_NAME);
    let n = name.len().min(31);
    b.bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
    b
}

/// The block at `offset`: the one already in `fields` (the ship's, taken
/// out), or the map's own.
fn current<R: Read + Seek>(
    map: &mut Map<R>,
    root: &[u8],
    fields: &mut Vec<(usize, u32, Block)>,
    offset: usize,
    stride: usize,
) -> Result<(usize, Block)> {
    if let Some(at) = fields.iter().position(|f| f.0 == offset) {
        let (_, count, block) = fields.remove(at);
        return Ok((count as usize, block));
    }
    let rows = metadata_block(map, root, offset, stride)?;
    Ok((rows.len(), Block::new(rows.concat())))
}

/// Author the pools and the falls into `fields` (the ship's authored
/// blocks, if any, which they extend) and return what they weigh, or none
/// when nothing falls. `ship_sets` are the ship's path point sets, which
/// go into the same scripting block. `bsp` is the BSP the falls were
/// planned on.
pub(crate) fn author<R: Read + Seek>(
    map: &mut Map<R>,
    pod: TagId,
    squad_pod: Option<TagId>,
    falls: &Falls,
    bsp: i16,
    fields: &mut Vec<(usize, u32, Block)>,
    ship_sets: Vec<PointSet>,
) -> Result<Option<Authored>> {
    let squad_first = falls.zones.iter().flatten().next();
    let player_first = falls.starts.iter().flatten().next();
    if squad_first.is_none() && player_first.is_none() {
        return Ok(None);
    }
    let scenario = map.scenario().context("map has no scenario")?.id;
    let root = map.read_tag_bytes(scenario)?;
    let (placements_at, palette_at, stride) = tags::object::OBJECT_TYPES[1]
        .placement
        .context("vehicles have no placement block")?;
    let (placements_at, palette_at, stride) = (usize::from(placements_at), usize::from(palette_at), usize::from(stride));
    let (name_count, mut names) = current(map, &root, fields, OBJECT_NAMES as usize, OBJECT_NAME)?;
    ensure!(
        !names.bytes.chunks(OBJECT_NAME).any(|n| {
            [POD_PREFIX, PLAYER_POD_PREFIX]
                .iter()
                .any(|p| n.starts_with(p.as_bytes()))
        }),
        "the base already has {POD_PREFIX}NN or {PLAYER_POD_PREFIX}NN objects"
    );
    let (place_count, mut placements) = current(map, &root, fields, placements_at, stride)?;
    let (palette_count, mut palette) = current(map, &root, fields, palette_at, PALETTE)?;
    let mut row_of = |tag: TagId| match palette
        .bytes
        .chunks(PALETTE)
        .position(|r| r[4..8] == tag.0.to_le_bytes())
    {
        Some(row) => row,
        None => {
            let at = palette.bytes.len();
            palette.bytes.extend(std::iter::repeat_n(0, PALETTE));
            palette.tag_ref(at as u32, tag);
            at / PALETTE
        }
    };
    // The squads' pool is the Flood pod when the kit has one.
    let player_row = row_of(pod);
    let squad_row = squad_pod.map_or(player_row, &mut row_of);
    let palette_len = palette.bytes.len() / PALETTE;
    let index = |len: usize, what: &str| {
        i16::try_from(len).map_err(|_| anyhow::anyhow!("{what} index overflow"))
    };

    let mut added = 0;
    let mut next_name = name_count;
    let mut next_place = place_count;
    let mut pool = |prefix: &str, count: usize, at: V3, row: usize| -> Result<()> {
        for n in 0..count {
            let mut name = name32(&format!("{prefix}{n:02}"));
            name.put_u16(0x20, 1);
            name.put_u16(0x22, index(next_place, "placement")? as u16);
            names.bytes.extend(&name.bytes);
            let mut place = Block::zeroed(stride);
            place.put_u16(0, index(row, "palette")? as u16);
            place.put_u16(2, index(next_name, "object name")? as u16);
            place.put_u32(4, 0);
            for (j, v) in [at[0], at[1], at[2], 0.0, 0.0, 0.0].into_iter().enumerate() {
                place.put_f32(8 + 4 * j, v);
            }
            place.put_u32(0x28, UNIQUE_ID_HIGH << 16 | (UNIQUE_LOW + next_place as u32 & 0xffff));
            place.put_u16(0x2c, 0xffff);
            place.bytes[0x2e] = 1;
            place.bytes[0x2f] = 1;
            place.put_u16(0x32, 0xffff);
            placements.bytes.extend(&place.bytes);
            next_name += 1;
            next_place += 1;
            added += OBJECT_NAME + stride;
        }
        Ok(())
    };
    if let Some(first) = squad_first {
        pool(POD_PREFIX, SQUAD_PODS, first.top, squad_row)?;
    }
    if let Some(first) = player_first {
        pool(PLAYER_POD_PREFIX, PLAYER_PODS, first.top, player_row)?;
    }
    fields.push((OBJECT_NAMES as usize, next_name as u32, names));
    fields.push((placements_at, next_place as u32, placements));
    if palette_len > palette_count {
        added += PALETTE;
    }
    fields.push((palette_at, palette_len as u32, palette));

    // The scripting data holds every point set, the ship's with ours.
    let mut sets = ScenarioAi::read_map(map)
        .map_err(|e| anyhow::anyhow!("reading the scenario's point sets: {e}"))?
        .point_sets;
    ensure!(
        !sets
            .iter()
            .any(|s| s.name.starts_with(FALL_PREFIX) || s.name.starts_with(PLAYER_FALL_PREFIX)),
        "the base already has {FALL_PREFIX}NN or {PLAYER_FALL_PREFIX}NN point sets"
    );
    sets.extend(ship_sets);
    let mut set_of = |prefix: &str, number: usize, fall: &Fall| {
        added += 0x30 + 0x3c * POINTS.len();
        PointSet {
            name: format!("{prefix}{number:02}"),
            points: POINTS
                .iter()
                .zip([fall.top, fall.ground])
                .map(|(name, position)| Point {
                    name: (*name).to_owned(),
                    position,
                    reference_frame: -1,
                    // No collision surface: the top is in the air.
                    surface: u32::MAX,
                    facing: [0.0, 0.0],
                })
                .collect(),
            bsp,
            manual_reference_frame: -1,
            flags: 0,
        }
    };
    let mut mine = Vec::new();
    for (number, fall) in falls.zones.iter().enumerate() {
        if let Some(fall) = fall {
            mine.push(set_of(FALL_PREFIX, number, fall));
        }
    }
    for (number, fall) in falls.starts.iter().enumerate() {
        if let Some(fall) = fall {
            mine.push(set_of(PLAYER_FALL_PREFIX, number, fall));
        }
    }
    sets.extend(mine);
    let authored = ScenarioAi {
        point_sets: sets,
        ..Default::default()
    }
    .blocks();
    let (at, count, block) = authored
        .fields
        .into_iter()
        .find(|(at, _, _)| *at as usize == layout::SCRIPTING_DATA.0)
        .context("no scripting data authored for the point sets")?;
    // The ship's scripting block is replaced by this one, which holds its
    // sets too.
    fields.retain(|f| f.0 != at as usize);
    fields.push((at as usize, count, block));
    Ok(Some(Authored { bytes: added }))
}

#[cfg(test)]
mod tests;
