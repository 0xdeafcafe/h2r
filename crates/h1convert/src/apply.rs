//! Writing a [`Plan`] into a Halo 2 base map's scenario, through the
//! scenario editor (`scenario-edit`): every starting location, netgame flag
//! and netgame equipment entry of the base is replaced by the plan's.
//!
//! Each new entry starts as a copy of the base's first entry of its block,
//! so fields this converter doesn't set (player type, equipment respawn
//! mode, ...) keep retail values; then position, facing, team, kind and item
//! are written.

use map::{FourCC, TagId};
use scenario_edit::{BlockKind, Scenario, TagRef, Value};

use crate::placements::{Plan, resolve};

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    /// Entries written per block: starting locations, flags, equipment.
    pub counts: [usize; 3],
    /// Of the equipment entries, how many are vehicles (Halo 2 spawns
    /// multiplayer vehicles from netgame equipment naming a vehicle
    /// collection, `vehc`).
    pub vehicles: usize,
    /// Item spawns whose role has no collection in the base map.
    pub unresolved: Vec<String>,
    /// Base object placements removed (they belong to the base's level).
    pub removed: usize,
    /// Blocks the editor refused to empty, with its reason.
    pub kept: Vec<String>,
}

/// The base's object placement blocks: its level's scenery, vehicles,
/// equipment, weapons, machines, sound scenery and crates, which would
/// float in the converted geometry.
pub const BASE_OBJECTS: [BlockKind; 7] = [
    BlockKind::Scenery,
    BlockKind::Vehicles,
    BlockKind::Equipment,
    BlockKind::Weapons,
    BlockKind::Machines,
    BlockKind::SoundScenery,
    BlockKind::Crates,
];

/// Why a plan couldn't be written.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The scenario editor refused an edit.
    #[error(transparent)]
    Edit(#[from] scenario_edit::Error),
    /// The base has no entry of a block to copy as a template.
    #[error("the base scenario has no {0:?} entry to start from")]
    NoTemplate(BlockKind),
}

fn template(s: &Scenario, kind: BlockKind) -> Result<Vec<u8>, ApplyError> {
    s.new_entry(kind, (!s.block(kind).is_empty()).then_some(0), None)
        .map_err(Into::into)
        .and_then(|e| {
            if s.block(kind).is_empty() {
                Err(ApplyError::NoTemplate(kind))
            } else {
                Ok(e)
            }
        })
}

fn clear(s: &mut Scenario, kind: BlockKind) -> Result<(), ApplyError> {
    while !s.block(kind).is_empty() {
        let last = s.block(kind).len() - 1;
        s.remove(kind, last)?;
    }
    Ok(())
}

/// Removes the base level's object placements ([`BASE_OBJECTS`]) and
/// replaces its spawns, flags and equipment with `plan`'s.
/// `tags` is the base map's tags as (group, ID, name), used to find each
/// item role's collection.
pub fn apply(
    s: &mut Scenario,
    plan: &Plan,
    tags: &[(FourCC, TagId, String)],
) -> Result<Applied, ApplyError> {
    use BlockKind::*;
    let templates = [
        template(s, StartingLocations)?,
        template(s, NetgameFlags)?,
        template(s, NetgameEquipment)?,
    ];
    for kind in [StartingLocations, NetgameFlags, NetgameEquipment] {
        clear(s, kind)?;
    }
    let mut out = Applied::default();
    for kind in BASE_OBJECTS {
        if let Some(why) = s.remove_refusal(kind) {
            out.kept.push(format!("{kind:?}: {why}"));
            continue;
        }
        out.removed += s.block(kind).len();
        clear(s, kind)?;
    }
    for sp in &plan.spawns {
        let i = s.add(StartingLocations, templates[0].clone())?;
        s.set(StartingLocations, i, "position", Value::Vec3(sp.position))?;
        s.set(StartingLocations, i, "facing", Value::Float(sp.facing))?;
        s.set(StartingLocations, i, "team", Value::Int(sp.team.into()))?;
        out.counts[0] += 1;
    }
    for f in &plan.flags {
        let i = s.add(NetgameFlags, templates[1].clone())?;
        s.set(NetgameFlags, i, "position", Value::Vec3(f.position))?;
        s.set(NetgameFlags, i, "facing", Value::Float(f.facing))?;
        s.set(NetgameFlags, i, "type", Value::Int(f.kind.raw().into()))?;
        s.set(NetgameFlags, i, "team", Value::Int(f.team.into()))?;
        out.counts[1] += 1;
    }
    let names: Vec<(String, String)> = tags
        .iter()
        .map(|(g, _, n)| (String::from_utf8_lossy(&g.0).into_owned(), n.clone()))
        .collect();
    for item in &plan.items {
        let Some(name) = resolve(item.item, &names) else {
            out.unresolved
                .push(format!("{:?} at {:?}", item.item, item.position));
            continue;
        };
        let (group, id, _) = tags.iter().find(|(_, _, n)| n == name).expect("resolved");
        let i = s.add(NetgameEquipment, templates[2].clone())?;
        s.set(NetgameEquipment, i, "position", Value::Vec3(item.position))?;
        s.set(
            NetgameEquipment,
            i,
            "orientation",
            Value::Vec3([item.facing, 0.0, 0.0]),
        )?;
        s.set(
            NetgameEquipment,
            i,
            "respawn time",
            Value::Int(item.respawn.into()),
        )?;
        s.set(
            NetgameEquipment,
            i,
            "item",
            Value::Tag(TagRef {
                group: *group,
                id: *id,
            }),
        )?;
        out.counts[2] += 1;
        if matches!(item.item, crate::sandbox::H2Item::Vehicle(_)) {
            out.vehicles += 1;
        }
    }
    Ok(out)
}
