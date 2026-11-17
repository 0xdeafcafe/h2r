//! The Halo 1 sandbox variant of a multiplayer map: the same structure and
//! spawns as the Halo 2 one, but every item and vehicle spawn places a
//! Halo 1 object converted to a Halo 2 tag (docs/game/h1-multiplayer.md,
//! "Halo 1 sandbox").
//!
//! The objects come from outside through [`Halo1Objects`]: `h1objects`
//! converts them (it depends on this crate, so it can't be called from
//! here). The converted tags are **added** to the map (a pinned base's tag
//! index can grow), first, right after the base's own slots, so their
//! datums are known when the placements are written: slot `n` gets salt
//! `0xe174 + n` (`TagSet::from_map`); [`super::Prepared::build`] checks.
//!
//! Each spawn names its object directly, not through a collection: the
//! netgame equipment spawner rolls an `itmc`/`vehc` and otherwise places
//! the tag it names (E `0x0015aad0`); a zero respawn time on a non-
//! collection falls back to the engine's default (E `0x0015ac83`); type 0
//! (weapons) keeps a weapon the globals' weapon list doesn't know (E
//! `0x0015a640`: `0x0015eb80` returns -1, the item is returned as is), and
//! the vehicle and powerup types keep the item when the variant leaves
//! the map's default (E `0x0015a4d0`, `0x0015a5b0`).

use std::io::{Read, Seek};

use h1map::H1Map;
use map::build::TagSet;
use map::{FourCC, Map, TagId};
use scenario_edit::{BlockKind, Scenario, Value};

use super::{Prepared, Sandbox, prepare};
use crate::apply::{NETGAME_ITEM, apply};
use crate::placements::{Plan, plan_map};
use crate::sandbox::{H2Item, h2_equivalent, netgame_type};
use crate::{Error, Result};

/// What converts Halo 1 objects into Halo 2 tags for the Halo 1 sandbox
/// (implemented by `h1objects::sandbox`).
pub trait Halo1Objects {
    /// The converted tag for the Halo 1 object tag `name` of `group`
    /// (`weap`, `eqip`, `vehi`), converting it the first time: its index
    /// among the tags [`ObjectTags::install`] adds. An error says why it
    /// can't be converted.
    fn object(&mut self, group: [u8; 4], name: &str) -> std::result::Result<usize, String>;

    /// Everything converted so far, ready to add to a map.
    fn take(&mut self) -> ObjectTags;
}

/// Converted object tags, to add to a `TagSet` made from the base map.
pub struct ObjectTags {
    /// How many tags [`Self::install`] adds.
    pub count: usize,
    /// Adds the tags to the set, in order, and returns their datums.
    #[allow(clippy::type_complexity)]
    pub install: Box<dyn Fn(&mut TagSet) -> std::result::Result<Vec<TagId>, String> + Send + Sync>,
    /// One line per converted object (what it was built on, what it
    /// keeps of the template).
    pub notes: Vec<String>,
}

impl std::fmt::Debug for ObjectTags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectTags")
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// Salt base of tags added to a set made from a map (`TagSet::from_map`).
pub const SALT_BASE: u16 = 0xe174;

/// The datum of the tag added in index slot `slot`.
pub fn added_datum(slot: usize) -> TagId {
    TagId(u32::from(SALT_BASE.wrapping_add(slot as u16)) << 16 | slot as u32)
}

/// A Halo 1 sandbox spawn: the Halo 1 object (group, tag name) and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Halo1Spawn {
    /// `weap`, `eqip` or `vehi`.
    pub group: [u8; 4],
    /// The object tag's name, and the other items its collection may
    /// spawn (tried in order).
    pub names: Vec<String>,
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Respawn seconds, 0 for the engine's default.
    pub respawn: u16,
}

/// Every item and vehicle spawn of a Halo 1 map, as Halo 1 objects.
pub fn plan_objects(h1: &H1Map) -> Result<Vec<Halo1Spawn>> {
    let mp = h1.multiplayer()?;
    let mut out = Vec::new();
    for e in &mp.equipment {
        let ic = h1.item_collection(e.item.id)?;
        let mut names = Vec::new();
        let mut group = *b"weap";
        for p in ic.permutations {
            let t = h1.tag(p.item.id)?;
            if names.is_empty() {
                group = t
                    .group
                    .to_string()
                    .as_bytes()
                    .try_into()
                    .unwrap_or(*b"weap");
            }
            names.push(t.name.clone());
        }
        out.push(Halo1Spawn {
            group,
            names,
            position: e.position,
            facing: e.facing,
            respawn: e.spawn_time,
        });
    }
    for v in &mp.vehicles {
        let name = mp
            .vehicle_palette
            .get(v.palette as usize)
            .and_then(|r| h1.tag(r.id).ok())
            .map(|t| t.name.clone())
            .unwrap_or_default();
        out.push(Halo1Spawn {
            group: *b"vehi",
            names: vec![name],
            position: v.position,
            facing: v.rotation[0],
            respawn: 0,
        });
    }
    Ok(out)
}

/// The netgame equipment type for a Halo 1 object: its Halo 2
/// equivalent's ([`netgame_type`]); equipment with none (the health pack)
/// is a powerup, 9.
pub fn halo1_netgame_type(group: [u8; 4], name: &str) -> u8 {
    match h2_equivalent(name) {
        Some(H2Item::Dropped) | None if &group == b"eqip" => 9,
        Some(H2Item::Dropped) | None if &group == b"vehi" => 1,
        Some(item) => netgame_type(item),
        None => 0,
    }
}

/// [`prepare`] for the Halo 1 sandbox: the Halo 2 preparation (structure,
/// spawns, flags), then every item and vehicle spawn replaced by its Halo
/// 1 object from `objects`. Spawns whose object doesn't convert are listed
/// in `applied.unresolved`.
pub fn prepare_halo1<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: content::Sha256,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
) -> Result<Prepared> {
    let mut p = prepare(h1, source_sha256, base, Sandbox::Halo2)?;
    let tags: Vec<_> = base
        .tags()
        .iter()
        .map(|t| (t.group, t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut s = Scenario::read(base)?;
    let template = s.new_entry(
        BlockKind::NetgameEquipment,
        (!s.block(BlockKind::NetgameEquipment).is_empty()).then_some(0),
        None,
    )?;
    // Spawns and flags as the Halo 2 variant has them; no Halo 2 items.
    let plan = plan_map(h1)?;
    let spawns_only = Plan {
        spawns: plan.spawns,
        flags: plan.flags,
        items: vec![],
        dropped: vec![],
    };
    let mut applied = apply(&mut s, &spawns_only, &tags, &mut vec![])?;
    let first = base.index_slots() as usize;
    let e = BlockKind::NetgameEquipment;
    for sp in plan_objects(h1)? {
        let mut why = Vec::new();
        let mut found = None;
        for n in &sp.names {
            match objects.object(sp.group, n) {
                Ok(k) => {
                    found = Some((n.clone(), k));
                    break;
                }
                Err(w) => why.push(format!("{n}: {w}")),
            }
        }
        let Some((name, k)) = found else {
            applied.unresolved.push(format!(
                "{} at {:?}: {}",
                sp.names.join(" / "),
                sp.position,
                why.join("; ")
            ));
            continue;
        };
        let i = s.add(e, template.clone())?;
        s.set(e, i, "position", Value::Vec3(sp.position))?;
        s.set(e, i, "orientation", Value::Vec3([sp.facing, 0.0, 0.0]))?;
        s.set(e, i, "respawn time", Value::Int(sp.respawn.into()))?;
        let kind = halo1_netgame_type(sp.group, &name);
        s.set(e, i, "type", Value::Int(kind.into()))?;
        // The tag is added when the map is built: the editor checks
        // references against the base's tags, so the bytes go in directly.
        let mut entry = s.block(e).entry(i).expect("added").to_vec();
        s.unadd(e, &entry)?;
        let id = added_datum(first + k);
        entry[NETGAME_ITEM..NETGAME_ITEM + 4]
            .copy_from_slice(&FourCC(sp.group).as_u32().to_le_bytes());
        entry[NETGAME_ITEM + 4..NETGAME_ITEM + 8].copy_from_slice(&id.0.to_le_bytes());
        s.add(e, entry)?;
        applied.counts[2] += 1;
        if &sp.group == b"vehi" {
            applied.vehicles += 1;
        }
    }
    let tags_out = objects.take();
    p.applied = applied;
    p.dropped = plan
        .dropped
        .into_iter()
        .filter(|d| d.starts_with("flag "))
        .collect();
    p.sandbox = Sandbox::Halo1;
    p.scenario = s;
    p.objects = Some(tags_out);
    p.objects_first = first;
    Ok(p)
}

/// [`super::convert`] for the Halo 1 sandbox.
pub fn convert_halo1<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: content::Sha256,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
) -> Result<super::Converted> {
    let p = prepare_halo1(h1, source_sha256, base, objects)?;
    let mopp = super::structure_mopp(&p.collision)?;
    p.build(base, &mopp)
}

/// Adds `objects` to `set` and checks they landed where the placements
/// expect them.
pub(super) fn install(objects: &ObjectTags, first: usize, set: &mut TagSet) -> Result<()> {
    if set.slots.len() != first {
        return Err(Error::Invalid(
            "the Halo 1 objects must be the first tags added",
        ));
    }
    let ids = (objects.install)(set).map_err(Error::Objects)?;
    let want: Vec<TagId> = (0..objects.count).map(|k| added_datum(first + k)).collect();
    if ids != want {
        return Err(Error::Invalid(
            "the Halo 1 objects' datums moved from what the placements name",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_tags_take_the_salt_of_their_slot() {
        // TagSet::from_map: slot n gets salt 0xe174 + n.
        assert_eq!(added_datum(0), TagId(0xe174_0000));
        assert_eq!(added_datum(0x1234), TagId(0xf3a8_1234));
    }

    #[test]
    fn halo1_objects_take_their_halo2_equivalents_netgame_type() {
        let t = |g: &[u8; 4], n: &str| halo1_netgame_type(*g, n);
        assert_eq!(t(b"weap", "weapons\\assault rifle\\assault rifle"), 0);
        assert_eq!(t(b"weap", "weapons\\pistol\\pistol"), 0);
        assert_eq!(t(b"vehi", "vehicles\\warthog\\warthog"), 1);
        assert_eq!(t(b"vehi", "vehicles\\ghost\\ghost_mp"), 2);
        assert_eq!(t(b"vehi", "vehicles\\scorpion\\scorpion_mp"), 3);
        assert_eq!(t(b"eqip", "weapons\\frag grenade\\frag grenade"), 8);
        assert_eq!(t(b"eqip", "powerups\\health pack"), 9);
        assert_eq!(t(b"eqip", "powerups\\over shield"), 9);
        assert_eq!(t(b"eqip", "powerups\\active camouflage"), 9);
    }
}
