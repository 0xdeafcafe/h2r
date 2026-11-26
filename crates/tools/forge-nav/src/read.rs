//! The generator's inputs from a built map (docs/game/forge-nav.md,
//! "Inputs"): the first structure BSP's collision, the starting
//! locations, the teleporter flag pairs and the solid placements the
//! caller names.
//!
//! - The structure BSP: the scenario's BSP block (`+0x210`, `0x44` each,
//!   the `sbsp` at `+0x14`), its collision at `sbsp+0x14`
//!   (`tags::pathfinding::SBSP_COLLISION_FIELD`), as the bots' generator
//!   and the Firefight kit read it.
//! - A teleporter: a netgame flag of type 7 (sender) and one of type 8
//!   (receiver) with the same identifier, as `forge-build` writes a link
//!   (docs/game/forge-design.md, "Slices", F6 in detail). Each sender pairs with each
//!   receiver of its channel.
//! - A solid: a scenery, crate or machine placement
//!   ([`SOLID_BLOCKS`]) whose object's model names a `coll`; each
//!   region's first permutation's BSPs, placed by the placement's frame.
//!   Shapes are not posed by their node (a one-node object at rest is the
//!   identity); vehicles move, so they are not solids here.
use std::io::Cursor;

use map::{Map, TagId};
use scenario_edit::geometry::{Marker, markers};
use scenario_edit::object_collision::resolve;
use scenario_edit::scene::metadata_block;
use scenario_edit::{BlockKind, Scenario, Value};
use tags::collision::{Bsp, Collision};
use tags::geometry::TagView;
use tags::object::Context;

use crate::{Frame, Input, Solid, Teleporter};

/// The blocks whose placements are solid to the nav.
pub const SOLID_BLOCKS: [BlockKind; 3] =
    [BlockKind::Scenery, BlockKind::Crates, BlockKind::Machines];

/// A teleporter sender's netgame flag type.
pub const TELEPORTER_SENDER: i64 = 7;
/// A teleporter receiver's netgame flag type.
pub const TELEPORTER_RECEIVER: i64 = 8;

const SCENARIO_BSPS: usize = 0x210;
const SCENARIO_BSP: usize = 0x44;
const BSP_TAG: usize = 0x14;
const SBSP_ROOT: usize = 0x148;

/// The inputs of a built map. `solid` says which placements join the
/// generator's input, by block and index: a session passes the ones its
/// document adds, so the base map's own scenery stays out as the bots'
/// generator leaves it.
pub fn read(bytes: &[u8], solid: impl Fn(BlockKind, usize) -> bool) -> Result<Input, String> {
    let mut map = Map::from_reader(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let level = level(&mut map)?;
    let scenario = Scenario::read(&mut map).map_err(|e| e.to_string())?;
    let all = markers(&scenario);
    let starts = all
        .iter()
        .filter(|m| m.block == BlockKind::StartingLocations)
        .map(|m| m.position)
        .collect();
    let teleporters = teleporters(&scenario);
    let placed: Vec<&Marker> = all
        .iter()
        .filter(|m| SOLID_BLOCKS.contains(&m.block) && solid(m.block, m.index))
        .collect();
    let solids = solids(&mut map, &placed);
    let lifts = lifts(&mut map)?;
    Ok(Input {
        level,
        solids,
        starts,
        teleporters,
        lifts,
    })
}

/// The scenario's lifts, read by the bots' own reader.
fn lifts<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
) -> Result<bots::autonav::Lifts, String> {
    let scenario = map.scenario().ok_or("the map has no scenario")?.clone();
    let address = scenario
        .location
        .ok_or("the scenario has no payload")?
        .address;
    let bytes = map.read_tag_bytes(scenario.id).map_err(|e| e.to_string())?;
    bots::autonav::Lifts::read(&bytes, address, map)
}

/// The first structure BSP's collision.
fn level<R: std::io::Read + std::io::Seek>(map: &mut Map<R>) -> Result<Bsp, String> {
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let scenario = map.scenario().ok_or("the map has no scenario")?.clone();
    let scnr = map.read_tag_bytes(scenario.id).map_err(|x| e(&x))?;
    let rows = metadata_block(map, &scnr, SCENARIO_BSPS, SCENARIO_BSP).map_err(|x| e(&x))?;
    let row = rows.first().ok_or("the scenario has no structure BSP")?;
    let id = TagId(u32::from_le_bytes(
        row[BSP_TAG..BSP_TAG + 4].try_into().unwrap(),
    ));
    let loc = map
        .tag(id)
        .map_err(|x| e(&x))?
        .location
        .ok_or("the structure BSP has no payload")?;
    let bytes = map.read_tag_bytes(id).map_err(|x| e(&x))?;
    let v = TagView::new(&bytes, loc.address);
    let root = v.at(0, SBSP_ROOT).map_err(|x| e(&x))?;
    let coll = v
        .block(
            root,
            tags::pathfinding::SBSP_COLLISION_FIELD,
            Bsp::STRUCTURE_SIZE,
        )
        .map_err(|x| e(&x))?;
    let record = coll.first().ok_or("the structure BSP has no collision")?;
    let at = record.as_ptr() as usize - bytes.as_ptr() as usize;
    Bsp::decode_structure(&bytes, loc.address, at).map_err(|x| e(&x))
}

/// Each sender with each receiver of its channel, in flag order.
fn teleporters(scenario: &Scenario) -> Vec<Teleporter> {
    let kind = BlockKind::NetgameFlags;
    let int = |i: usize, f: &str| scenario.get(kind, i, f).ok().and_then(Value::as_int);
    let flags: Vec<(i64, i64, [f32; 3])> = (0..scenario.block(kind).len())
        .filter_map(|i| {
            let position = scenario.get(kind, i, "position").ok()?.as_vec3()?;
            Some((int(i, "type")?, int(i, "identifier")?, position))
        })
        .collect();
    let mut out = Vec::new();
    for &(t, channel, sender) in &flags {
        if t != TELEPORTER_SENDER {
            continue;
        }
        for &(r, c, receiver) in &flags {
            if r == TELEPORTER_RECEIVER && c == channel {
                out.push(Teleporter {
                    channel,
                    sender,
                    receiver,
                });
            }
        }
    }
    out
}

/// The named placements' collision, each placement one solid per BSP.
fn solids<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, placed: &[&Marker]) -> Vec<Solid> {
    if placed.is_empty() {
        return Vec::new();
    }
    let Ok(ctx) = Context::read(map) else {
        return Vec::new();
    };
    let mut cache: Vec<(TagId, Vec<Vec<collision_build::Polygon>>)> = Vec::new();
    let mut out = Vec::new();
    for m in placed {
        let Some(object) = m.object else { continue };
        let shapes = match cache.iter().find(|(id, _)| *id == object) {
            Some((_, shapes)) => shapes.clone(),
            None => {
                let shapes = object_bsps(map, &ctx, object);
                cache.push((object, shapes.clone()));
                shapes
            }
        };
        let frame = Frame {
            position: m.position,
            axes: m.axes,
            scale: m.scale,
        };
        for (k, polygons) in shapes.iter().enumerate() {
            let name = format!("{} {} bsp {k}", m.block.name(), m.index);
            out.push(Solid::placed(name, polygons, &frame));
        }
    }
    out
}

/// The polygons of each BSP of an object's `coll`, in model space: each
/// region's first permutation. None for an object with no collision.
fn object_bsps<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    ctx: &Context,
    object: TagId,
) -> Vec<Vec<collision_build::Polygon>> {
    let Ok((_, Some(model))) = resolve(ctx, object) else {
        return Vec::new();
    };
    let Some(coll) = model.collision_model.as_ref().map(|l| l.id) else {
        return Vec::new();
    };
    let Ok(coll) = Collision::read(map, coll) else {
        return Vec::new();
    };
    coll.regions
        .iter()
        .filter_map(|r| r.permutations.first())
        .flat_map(|p| p.bsps.iter())
        .map(Solid::polygons_of)
        .filter(|p| !p.is_empty())
        .collect()
}
