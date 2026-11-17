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
use crate::sandbox::{H2Item, netgame_type, vehicle_class};

/// A vehicle collection (`vehc`) the base lacks, to be added when the map
/// is built: one entry naming the base's vehicle tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCollection {
    /// Tag name: `multiplayer\single_vehicles\<role>`.
    pub name: String,
    /// Its datum: the first of the `spare` datums [`apply`] is given (the
    /// converter gives the slots after the base's, where the build adds
    /// it).
    pub id: TagId,
    /// The vehicle (`vehi`) it spawns.
    pub vehicle: TagId,
    /// Model variant name, or the model's default.
    pub variant: Option<&'static str>,
    /// The name of the base tag whose slot it takes; empty for an added
    /// slot.
    pub replaces: String,
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    /// Entries written per block: starting locations, flags, equipment.
    pub counts: [usize; 3],
    /// Of the equipment entries, how many are vehicles (Halo 2 spawns
    /// multiplayer vehicles from netgame equipment naming a vehicle
    /// collection, `vehc`).
    pub vehicles: usize,
    /// Item spawns whose role has no collection in the base map (and, for
    /// a vehicle, no vehicle tag to build one from).
    pub unresolved: Vec<String>,
    /// Vehicle collections the base lacks, to add (their spawns already
    /// name them).
    pub collections: Vec<NewCollection>,
    /// Base object placements removed (they belong to the base's level).
    pub removed: usize,
    /// Blocks the editor refused to empty, with its reason.
    pub kept: Vec<String>,
    /// The item swaps a precision variant makes ([`precision_swaps`]):
    /// `(default, precision)` collection datums, for rows some spawn uses.
    pub precision: Vec<(TagId, TagId)>,
}

/// For each [`crate::sandbox::PRECISION`] row: the collection its Halo 1
/// item becomes by default, and the one it becomes in a precision variant
/// (the assault rifle: the SMG, the battle rifle). A pair is kept when
/// both resolve in the base and some spawn of `plan` places the default.
/// The default must be that row's alone, so every spawn of it came from
/// the row's Halo 1 item and swapping it at game start (`boot::precision`)
/// touches nothing else.
pub fn precision_swaps(plan: &Plan, tags: &[(FourCC, TagId, String)]) -> Vec<(TagId, TagId)> {
    let names: Vec<(String, String)> = tags
        .iter()
        .map(|(g, _, n)| (String::from_utf8_lossy(&g.0).into_owned(), n.clone()))
        .collect();
    let datum = |item| {
        let name = resolve(item, &names)?;
        tags.iter().find(|(_, _, n)| n == name).map(|t| t.1)
    };
    let mut out = Vec::new();
    for &(words, swap) in crate::sandbox::PRECISION {
        let Some(default) = crate::sandbox::h2_equivalent(words) else {
            continue;
        };
        if crate::sandbox::default_sources(default) != [words]
            || !plan.items.iter().any(|i| i.item == default)
        {
            continue;
        }
        if let (Some(a), Some(b)) = (datum(default), datum(swap))
            && a != b
        {
            out.push((a, b));
        }
    }
    out
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

/// Netgame equipment `item` reference (E `0x0015ac83`).
pub const NETGAME_ITEM: usize = 0x58;

/// The datum of the collection for vehicle `role`, adding it to `new` the
/// first time; `None` when the role or its vehicle tag is unknown.
fn new_collection(
    role: &str,
    tags: &[(FourCC, TagId, String)],
    spare: &mut Vec<TagId>,
    new: &mut Vec<NewCollection>,
) -> Option<TagId> {
    let name = format!("multiplayer\\single_vehicles\\{role}");
    if let Some(c) = new.iter().find(|c| c.name == name) {
        return Some(c.id);
    }
    let class = vehicle_class(role)?;
    let (_, vehicle, _) = tags.iter().find(|(g, _, n)| {
        g.0 == *b"vehi"
            && n.rsplit(['\\', '/'])
                .next()
                .is_some_and(|l| l.eq_ignore_ascii_case(class.vehicle))
    })?;
    if spare.is_empty() {
        return None;
    }
    let id = spare.remove(0);
    let replaces = tags
        .iter()
        .find(|(_, t, _)| *t == id)
        .map(|(_, _, n)| n.clone())
        .unwrap_or_default();
    new.push(NewCollection {
        name,
        id,
        vehicle: *vehicle,
        variant: class.variant,
        replaces,
    });
    Some(id)
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
/// item role's collection. A vehicle role with no collection in the base
/// gets a [`NewCollection`] in the first of the `spare` slots (taken from
/// the front), when the base has its vehicle tag (every retail multiplayer
/// map has them all) and a slot is left.
pub fn apply(
    s: &mut Scenario,
    plan: &Plan,
    tags: &[(FourCC, TagId, String)],
    spare: &mut Vec<TagId>,
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
        let target = match resolve(item.item, &names) {
            Some(name) => {
                let (group, id, _) = tags.iter().find(|(_, _, n)| n == name).expect("resolved");
                Some((*group, *id))
            }
            None => match item.item {
                H2Item::Vehicle(role) => new_collection(role, tags, spare, &mut out.collections)
                    .map(|id| (FourCC(*b"vehc"), id)),
                _ => None,
            },
        };
        let Some((group, id)) = target else {
            out.unresolved.push(format!(
                "{:?} at {:?}: the base has no collection for it",
                item.item, item.position
            ));
            continue;
        };
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
            "type",
            Value::Int(netgame_type(item.item).into()),
        )?;
        let r = TagRef { group, id };
        if !out.collections.iter().any(|c| c.id == id) {
            s.set(NetgameEquipment, i, "item", Value::Tag(r))?;
        } else {
            // A collection still to be built in another tag's slot: the
            // editor checks references against the base's tags, so the
            // bytes are written directly.
            let mut e = s.block(NetgameEquipment).entry(i).expect("added").to_vec();
            s.unadd(NetgameEquipment, &e)?;
            e[NETGAME_ITEM..NETGAME_ITEM + 4].copy_from_slice(&r.group.as_u32().to_le_bytes());
            e[NETGAME_ITEM + 4..NETGAME_ITEM + 8].copy_from_slice(&r.id.0.to_le_bytes());
            s.add(NetgameEquipment, e)?;
        }
        out.counts[2] += 1;
        if matches!(item.item, H2Item::Vehicle(_)) {
            out.vehicles += 1;
        }
    }
    out.precision = precision_swaps(plan, tags);
    Ok(out)
}
