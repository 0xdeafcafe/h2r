//! The dropship a kit carries (docs/game/firefight.md, "The kit", and
//! docs/game/rules-world-api.md, "Vehicles and drop ships"): the ship's
//! tag, one parked placement named `ff_ship`, and for each drop zone a
//! point set `ff_path_NN` the host flies the ship along: run-in, hover,
//! run-out.
//!
//! The ship is the Phantom, copied in by the kit's stage two from a donor
//! campaign map (`transplant`), or, on a map built from Halo 1's campaign,
//! the Covenant dropship the map already holds. A zone gets a path only
//! when collision rays find its column clear ([`plan`]): a zone under a
//! roof, or one the ship cannot approach, spawns its squads on foot.
use std::io::{Read, Seek};

use anyhow::{Context, Result, ensure};
use bots::los::Walls;
use h1campaign::ai::h2::{Point, PointSet, ScenarioAi, Zone, layout};
use h1campaign::scenario::{OBJECT_NAME, OBJECT_NAMES, PALETTE, UNIQUE_ID_HIGH};
use map::{Map, TagId, build::{Block, TagSet}, FourCC};
use pathfind::geom::V3;
use scenario_edit::scene::metadata_block;
use serde::Serialize;

/// The parked ship's object name, which the guest reads the placement by.
pub const SHIP_NAME: &str = "ff_ship";
/// A zone's path is the point set `ff_path_NN`, NN the zone's number
/// (`ff_drop_NN`); a zone without one has no point set.
pub const PATH_PREFIX: &str = "ff_path_";
/// The points of a path, in flying order.
pub const POINTS: [&str; 3] = ["run_in", "hover", "run_out"];
/// The retail Phantom's `vehi` tag: on 03a (Outskirts) and the other
/// campaign maps that carry it (docs/game/notes/2026-11-26-firefight-class-census.md).
pub const PHANTOM: &str = r"objects\vehicles\phantom\phantom";
/// Halo 1's Covenant dropship (the Spirit) as a map built from the
/// campaign keeps it ("a scripted set piece", `h1campaign::h2assets`),
/// with and without the `objects` prefix.
pub const SPIRIT: [&str; 2] = [
    r"vehicles\c_dropship\c_dropship",
    r"objects\vehicles\c_dropship\c_dropship",
];

const VEHI: FourCC = FourCC(*b"vehi");
/// The ship hovers this far above the zone's mean while it unloads.
pub const DROP_HEIGHT: f32 = 6.0;
/// The run-in and run-out fly this far above the tallest collision.
const SKY_CLEARANCE: f32 = 10.0;
/// And start this far outside the collision's horizontal bounds.
const RUN_MARGIN: f32 = 20.0;
/// The column's ray starts this far above the zone's floor.
const FLOOR_LIFT: f32 = 0.5;
/// Run directions tried for a zone, evenly round the compass.
const DIRECTIONS: usize = 8;
/// The placement's unique ID low word: past the converter's running count.
const UNIQUE_LOW: u32 = 0xff5e;

/// Which ship a kit carries (`h2 firefight kit MAP --dropship`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dropship {
    /// The Spirit when the map holds it, else the Phantom when a donor
    /// has it, else none: a map no ship can be had for builds as it did.
    #[default]
    Auto,
    /// The Phantom, from the map or a donor; a build without one fails.
    Phantom,
    /// The map's own Halo 1 dropship; a build without one fails.
    Spirit,
    /// No ship, no placement, no paths.
    None,
}

impl Dropship {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "auto" => Self::Auto,
            "phantom" => Self::Phantom,
            "spirit" => Self::Spirit,
            "none" => Self::None,
            _ => return None,
        })
    }
}

/// What a build says of the dropship (`Package::ship`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ShipReport {
    pub kept: bool,
    /// The ship's tag name when kept.
    pub tag: String,
    /// Tags the transplant copied in (0 when the map held the ship).
    pub tags: usize,
    /// Tag memory the ship cost: its copied tags, placement, palette row,
    /// object name and point sets.
    pub bytes: usize,
    /// Zones with a path.
    pub paths: usize,
    /// Why it was cut.
    pub why: Option<String>,
}

impl ShipReport {
    pub(crate) fn cut(why: impl Into<String>) -> Self {
        Self {
            kept: false,
            tag: String::new(),
            tags: 0,
            bytes: 0,
            paths: 0,
            why: Some(why.into()),
        }
    }
}

impl std::fmt::Display for ShipReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.kept {
            write!(
                f,
                "dropship: kept ({}, {} tags, {} bytes, {} zone paths)",
                self.tag, self.tags, self.bytes, self.paths
            )
        } else {
            write!(f, "dropship: cut ({})", self.why.as_deref().unwrap_or("not built"))
        }
    }
}

/// Where the build gets its ship.
pub(crate) enum Pick {
    /// A tag the map has.
    Have(TagId, String),
    /// A tag to copy from a donor, by name.
    Copy(&'static str),
}

/// The ship `want` asks for on `set`, or an error when it cannot be had
/// there. [`Dropship::None`] is not asked here.
pub(crate) fn pick(set: &TagSet, want: Dropship) -> Result<Pick> {
    let spirit = SPIRIT
        .iter()
        .find_map(|p| set.find(VEHI, p).map(|id| (id, (*p).to_owned())));
    let phantom = set.find(VEHI, PHANTOM).map(|id| (id, PHANTOM.to_owned()));
    let have = match want {
        Dropship::None => None,
        Dropship::Spirit => Some(spirit.context(
            "--dropship spirit needs the Halo 1 Covenant dropship in the map: build it from a Halo 1 campaign map",
        )?),
        Dropship::Phantom => phantom,
        Dropship::Auto => spirit.or(phantom),
    };
    Ok(match have {
        Some((id, name)) => Pick::Have(id, name),
        None => Pick::Copy(PHANTOM),
    })
}

/// A ship's way into a zone: the three points of its path, in flying order,
/// and the yaw it flies at (radians, the run-in's direction of travel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Run {
    pub points: [V3; 3],
    pub yaw: f32,
}

/// Each zone's run, none where the collision leaves no clear column.
///
/// A zone's point is its first area's mean. The ship hovers
/// [`DROP_HEIGHT`] above it. A zone gets a run only when
/// - a ray from just above its floor to the sky (the tallest collision plus
///   [`SKY_CLEARANCE`]) is clear, so no roof is over it, and
/// - for one of eight compass directions, the ray from the run-in down to the hover point and the ray from
///   the hover point up to the run-out are clear.
///
/// Run-in and run-out lie on opposite sides at the sky height, outside the
/// collision's horizontal bounds by [`RUN_MARGIN`]. Directions are tried
/// from the one pointing away from the map's middle, so the same zone on
/// the same map always gets the same run.
pub(crate) fn plan(walls: &Walls, zones: &[Zone]) -> Vec<Option<Run>> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in walls.tris.iter().flatten() {
        for j in 0..3 {
            lo[j] = lo[j].min(v[j]);
            hi[j] = hi[j].max(v[j]);
        }
    }
    if !lo.iter().chain(&hi).all(|x| x.is_finite()) {
        return vec![None; zones.len()];
    }
    let middle = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let half = libm::hypotf(hi[0] - lo[0], hi[1] - lo[1]) * 0.5;
    let sky = hi[2] + SKY_CLEARANCE;
    zones
        .iter()
        .map(|zone| {
            let mean = zone.areas.first()?.mean;
            if !mean.iter().all(|x| x.is_finite()) {
                return None;
            }
            let hover = [mean[0], mean[1], mean[2] + DROP_HEIGHT];
            if hover[2] >= sky
                || !walls.clear([mean[0], mean[1], mean[2] + FLOOR_LIFT], [mean[0], mean[1], sky])
            {
                return None;
            }
            // Far enough that the run-in and run-out start outside.
            let reach = half
                + libm::hypotf(mean[0] - middle[0], mean[1] - middle[1])
                + RUN_MARGIN;
            let away = libm::atan2f(mean[1] - middle[1], mean[0] - middle[0]);
            (0..DIRECTIONS).find_map(|k| {
                let angle = away + k as f32 * std::f32::consts::TAU / DIRECTIONS as f32;
                let (dy, dx) = libm::sincosf(angle);
                let end = |sign: f32| [hover[0] + sign * dx * reach, hover[1] + sign * dy * reach, sky];
                let (run_in, run_out) = (end(1.0), end(-1.0));
                (walls.clear(run_in, hover) && walls.clear(hover, run_out)).then_some(Run {
                    points: [run_in, hover, run_out],
                    yaw: libm::atan2f(-dy, -dx),
                })
            })
        })
        .collect()
}

/// The scenario blocks a ship adds, `(field, count, block)`.
pub(crate) struct Authored {
    pub fields: Vec<(usize, u32, Block)>,
    /// Zones with a path.
    pub paths: usize,
}

fn name32(name: &str, size: usize) -> Block {
    let mut b = Block::zeroed(size);
    let n = name.len().min(31);
    b.bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
    b
}

/// Author the ship's placement, `ff_ship`, parked at the first run's
/// run-in with its yaw, and a point set `ff_path_NN` for each zone with a
/// run, on `ship`'s `vehi` tag. The base keeps its own vehicles, names and
/// point sets; one of ours already there is refused. `bsp` is the BSP the
/// runs were planned on. None when no zone has a run.
pub(crate) fn author<R: Read + Seek>(
    map: &mut Map<R>,
    ship: TagId,
    runs: &[Option<Run>],
    bsp: i16,
) -> Result<Option<Authored>> {
    let Some(first) = runs.iter().flatten().next() else {
        return Ok(None);
    };
    let scenario = map.scenario().context("map has no scenario")?.id;
    let root = map.read_tag_bytes(scenario)?;
    let (placements_at, palette_at, stride) = tags::object::OBJECT_TYPES[1]
        .placement
        .context("vehicles have no placement block")?;
    let (placements_at, palette_at, stride) = (usize::from(placements_at), usize::from(palette_at), usize::from(stride));
    let names = metadata_block(map, &root, OBJECT_NAMES as usize, OBJECT_NAME)?;
    ensure!(
        !names.iter().any(|n| n.starts_with(SHIP_NAME.as_bytes()) && n[SHIP_NAME.len()] == 0),
        "the base already has an object named {SHIP_NAME}"
    );
    let mut palette = metadata_block(map, &root, palette_at, PALETTE)?;
    let placements = metadata_block(map, &root, placements_at, stride)?;
    let index = |len: usize, what: &str| {
        i16::try_from(len).map_err(|_| anyhow::anyhow!("{what} index overflow"))
    };
    let had = palette.len();
    let row = match palette.iter().position(|r| r[4..8] == ship.0.to_le_bytes()) {
        Some(row) => row,
        None => {
            let mut entry = Block::zeroed(PALETTE);
            entry.tag_ref(0, ship);
            palette.push(entry.bytes);
            palette.len() - 1
        }
    };
    let palette_new = palette.len() > had;

    let mut fields: Vec<(usize, u32, Block)> = Vec::new();
    // The object name, then the placement that carries its index.
    let name_row = {
        let mut b = name32(SHIP_NAME, OBJECT_NAME);
        b.put_u16(0x20, 1);
        b.put_u16(0x22, index(placements.len(), "placement")? as u16);
        b
    };
    let name_index = index(names.len(), "object name")?;
    let mut name_bytes = names.concat();
    name_bytes.extend(&name_row.bytes);
    fields.push((OBJECT_NAMES as usize, names.len() as u32 + 1, Block::new(name_bytes)));

    let mut place = Block::zeroed(stride);
    place.put_u16(0, index(row, "palette")? as u16);
    place.put_u16(2, name_index as u16);
    // Placed at the start: parked, waiting for its first squad.
    place.put_u32(4, 0);
    let start = first.points[0];
    for (j, v) in [start[0], start[1], start[2], first.yaw, 0.0, 0.0].into_iter().enumerate() {
        place.put_f32(8 + 4 * j, v);
    }
    place.put_u32(0x28, UNIQUE_ID_HIGH << 16 | UNIQUE_LOW);
    place.put_u16(0x2c, 0xffff);
    place.bytes[0x2e] = 1;
    place.bytes[0x2f] = 1;
    place.put_u16(0x32, 0xffff);
    let mut place_bytes = placements.concat();
    place_bytes.extend(&place.bytes);
    fields.push((placements_at, placements.len() as u32 + 1, Block::new(place_bytes)));

    if palette_new {
        let count = palette.len() as u32;
        let mut block = Block::new(palette.concat());
        block.tag_ref(((palette.len() - 1) * PALETTE) as u32, ship);
        fields.push((palette_at, count, block));
    }

    let mut sets = ScenarioAi::read_map(map)
        .map_err(|e| anyhow::anyhow!("reading the scenario's point sets: {e}"))?
        .point_sets;
    ensure!(
        !sets.iter().any(|s| s.name.starts_with(PATH_PREFIX)),
        "the base already has {PATH_PREFIX}NN point sets"
    );
    let mut paths = 0;
    for (zone, run) in runs.iter().enumerate() {
        let Some(run) = run else { continue };
        sets.push(PointSet {
            name: format!("{PATH_PREFIX}{zone:02}"),
            points: POINTS
                .iter()
                .zip(run.points)
                .map(|(name, position)| Point {
                    name: (*name).to_owned(),
                    position,
                    reference_frame: -1,
                    // No collision surface: the points are in the air.
                    surface: u32::MAX,
                    facing: [run.yaw, 0.0],
                })
                .collect(),
            bsp,
            manual_reference_frame: -1,
            flags: 0,
        });
        paths += 1;
    }
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
    fields.push((at as usize, count, block));
    Ok(Some(Authored { fields, paths }))
}

#[cfg(test)]
mod tests;
