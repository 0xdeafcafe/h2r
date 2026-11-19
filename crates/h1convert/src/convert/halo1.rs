//! The Halo 1 sandbox variant of a multiplayer map: the same structure and
//! spawns as the Halo 2 one, but every item and vehicle spawn places a
//! Halo 1 object converted to a Halo 2 tag (docs/halo1/multiplayer.md,
//! "Halo 1 sandbox").
//!
//! The objects come from outside through [`Halo1Objects`]: `h1objects`
//! converts them (it depends on this crate, so it can't be called from
//! here). The converted tags are **added** to the map (a pinned base's tag
//! index can grow), first, right after the base's own slots, so their
//! datums are known when the placements are written: slot `n` gets salt
//! `0xe174 + n` (`TagSet::from_map`); [`super::Prepared::build`] checks.
//!
//! Each spawn names a **collection** (`itmc` for items, `vehc` for
//! vehicles) built here from Halo 1's: the netgame equipment spawner
//! `0x0015aad0` reads the entry's item at `+0x58` and places something only
//! when its group is `itmc` or `vehc` (E `0x0015ab12`..`0x0015ab23`: the
//! collection is rolled by `0x0019f1a0`; any other group leaves the item
//! -1, and `0x0015ab8b` returns without placing). Through converter version
//! 49 each spawn named its `weap`/`eqip`/`vehi` directly, so nothing in the
//! Halo 1 sandbox spawned. A collection lists each converted item of the
//! Halo 1 collection with its weight (`0x0019f1a0` returns the only entry
//! of a one-entry collection whatever its weight, else rolls the weights),
//! and holds Halo 1's respawn time as its root `i16` at `+8` (read when
//! the spawn's own `+0xe` is 0, E `0x0015ac83`..`0x0015acb0`). The
//! collections are added right after the objects, so their datums are
//! known too. Type 0 (weapons) keeps a weapon the globals' weapon list
//! doesn't know (E `0x0015a640`: `0x0015eb80` returns -1, the item is
//! returned as is), and the vehicle and powerup types keep the item when
//! the variant leaves the map's default (E `0x0015a4d0`, `0x0015a5b0`).

use std::io::{Read, Seek};

use h1map::H1Map;
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use scenario_edit::{BlockKind, Scenario, Value};

use super::{COLLECTION_ENTRY, COLLECTION_ROOT, Prepared, Sandbox, prepare};
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

    /// The converted render model for Halo 1's first-person arms `id`
    /// (its globals' first-person interface), converting it the first
    /// time: its index among the tags [`ObjectTags::install`] adds.
    fn first_person_arms(&mut self, id: h1map::TagId) -> std::result::Result<usize, String>;

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
    /// Run once every tag the placements name is in (the objects and the
    /// collections after them), with the datums `install` returned: adds
    /// what the objects name but no placement does (their sounds) after
    /// them, so it moves no datum a placement names.
    #[allow(clippy::type_complexity)]
    pub after: Box<dyn Fn(&mut TagSet, &[TagId]) -> std::result::Result<(), String> + Send + Sync>,
    /// One line per converted object (what it was built on, what it
    /// keeps of the template).
    pub notes: Vec<String>,
    /// HUD pairs `boot::hud_swap` switches between (docs/halo1/hud.md): the
    /// owner, its Halo 1 HUD and its Halo 2 HUD.
    pub hud_swaps: Vec<[HudTag; 3]>,
    /// Where the player's Halo 1 HUD centres the motion sensor's blips
    /// (`h1objects::h1hud::sensor_centre`), for `boot::hud_swap`.
    pub hud_sensor: Option<[i16; 2]>,
}

/// A tag of a HUD pair: one [`ObjectTags::install`] adds (its index among
/// them), or one of the base map's (a weapon's Halo 2 HUD).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudTag {
    Added(usize),
    Base(TagId),
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

/// One item a Halo 1 collection may spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Halo1Item {
    /// `weap`, `eqip` or `vehi`.
    pub group: [u8; 4],
    /// The object tag's name.
    pub name: String,
    /// Its weight in the collection.
    pub weight: f32,
}

/// A Halo 1 sandbox spawn: its Halo 1 collection and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Halo1Spawn {
    /// The Halo 1 collection's name (the `itmc`), or the vehicle's for a
    /// vehicle spawn: spawns of the same one share a built collection.
    pub collection: String,
    /// Every item the collection may spawn, in Halo 1's order.
    pub items: Vec<Halo1Item>,
    /// The Halo 1 collection's respawn seconds, 0 for the engine's default.
    pub collection_respawn: u16,
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Respawn seconds, 0 for the collection's.
    pub respawn: u16,
}

impl Halo1Spawn {
    /// Whether it spawns a vehicle (a `vehc`) rather than an item (`itmc`).
    pub fn is_vehicle(&self) -> bool {
        self.items.first().is_some_and(|i| &i.group == b"vehi")
    }
}

/// Every item and vehicle spawn of a Halo 1 map, as Halo 1 objects.
pub fn plan_objects(h1: &H1Map) -> Result<Vec<Halo1Spawn>> {
    let mp = h1.multiplayer()?;
    let mut out = Vec::new();
    for e in &mp.equipment {
        let ic = h1.item_collection(e.item.id)?;
        let mut items = Vec::new();
        for p in ic.permutations {
            let t = h1.tag(p.item.id)?;
            items.push(Halo1Item {
                group: t
                    .group
                    .to_string()
                    .as_bytes()
                    .try_into()
                    .unwrap_or(*b"weap"),
                name: t.name.clone(),
                weight: p.weight,
            });
        }
        out.push(Halo1Spawn {
            collection: h1.tag(e.item.id)?.name.clone(),
            items,
            collection_respawn: ic.spawn_time,
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
        let [x, y, z] = v.position;
        out.push(Halo1Spawn {
            collection: name.clone(),
            items: vec![Halo1Item {
                group: *b"vehi",
                name: name.clone(),
                weight: 1.0,
            }],
            collection_respawn: 0,
            position: [x, y, z + vehicle_depth(h1, &name)],
            facing: v.rotation[0],
            respawn: 0,
        });
    }
    Ok(out)
}

/// How far the Halo 1 vehicle `name`'s render model reaches below its
/// origin (0 when it does not, or the model cannot be read): what a
/// vehicle spawn is raised by, so the vehicle does not start in the floor.
///
/// V: Halo 1 places a vehicle's origin on the floor (every Blood Gulch
/// vehicle placement is 0.001 above the structure's collision), and the
/// hover vehicles reach below it: the ghost's hull to 0.143 under its
/// origin (`h1objects/tests/vehicle_ground_disc.rs`). Halo 1's own mass
/// points lifted it once spawned; in Halo 2 the converted ghost started
/// 0.14 into the ground and stayed there.
pub fn vehicle_depth(h1: &H1Map, name: &str) -> f32 {
    let lowest = || -> Option<f32> {
        let t = h1.find(h1map::Group::new(b"vehi"), name)?;
        let r = h1map::TagRef::read(h1.tag_data(t).ok()?, VEHI_MODEL)?;
        let mode = h1.tag(r.id).ok()?;
        let (m, _) = h1models::model_from_map(h1, mode, &Default::default()).ok()?;
        m.geometries
            .iter()
            .flat_map(|g| &g.parts)
            .flat_map(|p| &p.vertices)
            .map(|v| v.position[2])
            .reduce(f32::min)
    };
    lowest().map_or(0.0, |z| (-z).max(0.0))
}

/// A Halo 1 object's render model reference (`obje +0x28`).
const VEHI_MODEL: usize = 0x28;

/// A collection the Halo 1 sandbox adds after its objects: one per Halo 1
/// item collection (or vehicle) some spawn uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Halo1Collection {
    /// `itmc` or `vehc`.
    pub group: [u8; 4],
    /// Halo 1's collection (or vehicle) name.
    pub name: String,
    /// `(object index among the converted tags, weight)`.
    pub entries: Vec<(usize, f32)>,
    /// Respawn seconds, root `i16` at `+8`.
    pub respawn: u16,
}

impl Halo1Collection {
    /// The collection's root: the entries block at `+0` (`0x10` each:
    /// weight `+0`, the object's reference `+4`, variant `+0xc` none) and
    /// the respawn time at `+8`. `object` gives each entry's datum.
    pub fn root(&self, object: impl Fn(usize) -> TagId) -> Block {
        let mut entries = Block::zeroed(COLLECTION_ENTRY * self.entries.len());
        for (k, (i, w)) in self.entries.iter().enumerate() {
            let at = k * COLLECTION_ENTRY;
            entries.put_f32(at, *w).tag_ref(at as u32 + 4, object(*i));
        }
        let mut root = Block::zeroed(COLLECTION_ROOT);
        root.put_u16(8, self.respawn)
            .block(0, self.entries.len() as u32, entries);
        root
    }
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
    // Each Halo 1 collection some spawn uses, built once, and the spawns
    // naming it (collection index, netgame type).
    let mut collections: Vec<Halo1Collection> = Vec::new();
    // Each collection's netgame type: its first converted item's class
    // (weapons, grenades, powerups, a vehicle's).
    let mut kinds: Vec<u8> = Vec::new();
    let mut spawns: Vec<(Halo1Spawn, usize)> = Vec::new();
    // The Halo 1 vehicles the spawns place, converted, by name.
    let mut placed_vehicles: Vec<String> = Vec::new();
    for sp in plan_objects(h1)? {
        let group = if sp.is_vehicle() { *b"vehc" } else { *b"itmc" };
        let c = match collections
            .iter()
            .position(|c| c.group == group && c.name == sp.collection)
        {
            Some(c) => c,
            None => {
                let mut entries = Vec::new();
                let mut why = Vec::new();
                let mut kind = None;
                for it in &sp.items {
                    match objects.object(it.group, &it.name) {
                        Ok(k) => {
                            if &it.group == b"vehi" && !placed_vehicles.contains(&it.name) {
                                placed_vehicles.push(it.name.clone());
                            }
                            entries.push((k, it.weight));
                            kind.get_or_insert(halo1_netgame_type(it.group, &it.name));
                        }
                        Err(w) => why.push(format!("{}: {w}", it.name)),
                    }
                }
                if entries.is_empty() {
                    applied.unresolved.push(format!(
                        "{} at {:?}: {}",
                        sp.collection,
                        sp.position,
                        why.join("; ")
                    ));
                    continue;
                }
                for w in why {
                    applied
                        .unresolved
                        .push(format!("{} leaves out {w}", sp.collection));
                }
                collections.push(Halo1Collection {
                    group,
                    name: sp.collection.clone(),
                    entries,
                    respawn: sp.collection_respawn,
                });
                kinds.push(kind.unwrap_or(0));
                collections.len() - 1
            }
        };
        spawns.push((sp, c));
    }
    // The player: Halo 1's multiplayer biped, converted like the objects.
    let player = player_biped_name(h1);
    let player_biped = match &player {
        Some(n) => match objects.object(*b"bipd", n) {
            Ok(k) => Some(added_datum(first + k)),
            Err(w) => {
                applied.unresolved.push(format!("player biped {n}: {w}"));
                None
            }
        },
        None => {
            applied
                .unresolved
                .push("player biped: the map has no cyborg biped".into());
            None
        }
    };
    let scenery = plan_scenery(h1, objects, first, &mut applied.unresolved)?;
    // The player's first-person arms: Halo 1's, which Halo 1's
    // first-person graphs name node for node (the engine binds them by
    // name; Halo 2's arms match no node of them).
    let player_hands = match crate::globals::h1_first_person_hands(h1)? {
        Some(id) => match objects.first_person_arms(id) {
            Ok(k) => Some(added_datum(first + k)),
            Err(w) => {
                applied.unresolved.push(format!("first-person arms: {w}"));
                None
            }
        },
        None => None,
    };
    // Halo 1's grenade types, converted like the objects.
    let mut grenades = Vec::new();
    for g in crate::globals::h1_grenades(h1)? {
        let mut one = |id: Option<h1map::TagId>, group: [u8; 4]| -> Option<TagId> {
            let tag = h1.tag(id?).ok()?;
            match objects.object(group, &tag.name) {
                Ok(k) => Some(added_datum(first + k)),
                Err(w) => {
                    applied.unresolved.push(format!("grenade {}: {w}", tag.name));
                    None
                }
            }
        };
        grenades.push(crate::globals::Grenade {
            maximum: g.maximum,
            equipment: one(g.equipment, *b"eqip"),
            projectile: one(g.projectile, *b"proj"),
        });
    }
    let weapon_list = plan_weapon_list(h1, base, objects, first, &mut applied.unresolved)?;
    let vehicle_list = plan_vehicle_list(
        base,
        objects,
        first,
        &placed_vehicles,
        &mut applied.unresolved,
    )?;
    let starting = plan_starting_equipment(
        h1,
        base,
        &s,
        objects,
        &mut collections,
        &mut applied.unresolved,
    )?;
    let objects_out = objects.take();
    // The collections follow the objects.
    let after = first + objects_out.count;
    for (sp, c) in &spawns {
        let i = s.add(e, template.clone())?;
        s.set(e, i, "position", Value::Vec3(sp.position))?;
        s.set(e, i, "orientation", Value::Vec3([sp.facing, 0.0, 0.0]))?;
        s.set(e, i, "respawn time", Value::Int(sp.respawn.into()))?;
        s.set(e, i, "type", Value::Int(kinds[*c].into()))?;
        // The tag is added when the map is built: the editor checks
        // references against the base's tags, so the bytes go in directly.
        let mut entry = s.block(e).entry(i).expect("added").to_vec();
        s.unadd(e, &entry)?;
        let id = added_datum(after + c);
        entry[NETGAME_ITEM..NETGAME_ITEM + 4]
            .copy_from_slice(&FourCC(collections[*c].group).as_u32().to_le_bytes());
        entry[NETGAME_ITEM + 4..NETGAME_ITEM + 8].copy_from_slice(&id.0.to_le_bytes());
        s.add(e, entry)?;
        applied.counts[2] += 1;
        if sp.is_vehicle() {
            applied.vehicles += 1;
        }
    }
    // The starting equipment names the Halo 1 collections made for it.
    let k = BlockKind::StartingEquipment;
    let mut entries: Vec<Vec<u8>> = (0..s.block(k).len())
        .map(|i| s.block(k).entry(i).expect("in range").to_vec())
        .collect();
    for (i, at, c) in &starting {
        let id = added_datum(after + c);
        entries[*i][*at..*at + 4].copy_from_slice(&FourCC(*b"itmc").as_u32().to_le_bytes());
        entries[*i][*at + 4..*at + 8].copy_from_slice(&id.0.to_le_bytes());
    }
    if !starting.is_empty() {
        while !s.block(k).is_empty() {
            let last = s.block(k).len() - 1;
            s.remove(k, last)?;
        }
        for e in entries {
            s.add(k, e)?;
        }
    }
    let tags_out = with_collections(objects_out, first, collections);
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
    p.player_biped = player_biped;
    p.scenery = Some(scenery);
    p.player_hands = player_hands;
    p.grenades = grenades;
    p.weapon_list = weapon_list;
    p.vehicle_list = vehicle_list;
    Ok(p)
}

/// The starting equipment's collection references (`scnr +0x128`,
/// `0x9c` each, five `itmc` references from `+0x3c`, 8 apart). E: with the
/// variant's starting weapon at 0 (the map's default, which most built-in
/// variants hold), `0x0019ef40` rolls them (`0x0019efef`, `0x0019f1a0`)
/// for the player's weapons.
pub const STARTING_EQUIPMENT_COLLECTIONS: [usize; 5] = [0x3c, 0x44, 0x4c, 0x54, 0x5c];

/// The Halo 1 sandbox's starting equipment: each `itmc` the base's
/// starting equipment names gets a Halo 1 collection, its weapons each the
/// Halo 1 weapon [`crate::sandbox::halo1_weapon_for`] picks at the same
/// weight (converted like the objects), appended to `collections`.
/// Returns (entry, reference offset, collection index) to point at it.
/// Entries that are not weapons are reported and left out; a collection
/// with no Halo 1 weapon is reported and left as the base's.
fn plan_starting_equipment<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    s: &Scenario,
    objects: &mut dyn Halo1Objects,
    collections: &mut Vec<Halo1Collection>,
    unresolved: &mut Vec<String>,
) -> Result<Vec<(usize, usize, usize)>> {
    let k = BlockKind::StartingEquipment;
    let names: Vec<(TagId, FourCC, String, Option<u32>)> = base
        .tags()
        .iter()
        .map(|t| {
            (
                t.id,
                t.group,
                t.name.clone().unwrap_or_default(),
                t.location.map(|l| l.address),
            )
        })
        .collect();
    let mut made: Vec<(TagId, Option<usize>)> = Vec::new();
    let mut out = Vec::new();
    let word = |b: &[u8], at: usize| {
        b.get(at..at + 4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .ok_or(Error::Invalid("a collection is too short"))
    };
    for i in 0..s.block(k).len() {
        let e = s.block(k).entry(i).expect("in range").to_vec();
        for at in STARTING_EQUIPMENT_COLLECTIONS {
            let id = TagId(word(&e, at + 4)?);
            if id.0 == u32::MAX || word(&e, at)? != FourCC(*b"itmc").as_u32() {
                continue;
            }
            let c = match made.iter().find(|m| m.0 == id) {
                Some(m) => m.1,
                None => {
                    let c = starting_collection(h1, base, &names, id, objects, collections, unresolved, &word)?;
                    made.push((id, c));
                    c
                }
            };
            if let Some(c) = c {
                out.push((i, at, c));
            }
        }
    }
    Ok(out)
}

/// One starting equipment collection rebuilt with Halo 1 weapons
/// ([`plan_starting_equipment`]); its index in `collections`.
#[allow(clippy::too_many_arguments)]
fn starting_collection<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    names: &[(TagId, FourCC, String, Option<u32>)],
    id: TagId,
    objects: &mut dyn Halo1Objects,
    collections: &mut Vec<Halo1Collection>,
    unresolved: &mut Vec<String>,
    word: &dyn Fn(&[u8], usize) -> Result<u32>,
) -> Result<Option<usize>> {
    let Some((_, _, name, Some(address))) = names.iter().find(|t| t.0 == id) else {
        return Ok(None);
    };
    let bytes = base.read_tag_bytes(id)?;
    let n = word(&bytes, 0)? as usize;
    let first = word(&bytes, 4)?
        .checked_sub(*address)
        .ok_or(Error::Invalid("a collection's entries lie outside it"))? as usize;
    let mut entries = Vec::new();
    for j in 0..n {
        let at = first + j * super::COLLECTION_ENTRY;
        let weight = f32::from_bits(word(&bytes, at)?);
        let item = TagId(word(&bytes, at + 8)?);
        let Some((_, g, item_name, _)) = names.iter().find(|t| t.0 == item) else {
            continue;
        };
        if *g != FourCC(*b"weap") {
            unresolved.push(format!("starting equipment {name}: {item_name} is not a weapon"));
            continue;
        }
        match halo1_weapon(h1, item_name) {
            Some(w) => match objects.object(*b"weap", &w) {
                Ok(k) => entries.push((k, weight)),
                Err(why) => unresolved.push(format!("starting equipment {name}: {w}: {why}")),
            },
            None => unresolved.push(format!(
                "starting equipment {name}: {item_name} has no Halo 1 weapon on the map"
            )),
        }
    }
    if entries.is_empty() {
        unresolved.push(format!("starting equipment {name}: no Halo 1 weapon; left as the base's"));
        return Ok(None);
    }
    collections.push(Halo1Collection {
        group: *b"itmc",
        name: format!("{name} (Halo 1)"),
        entries,
        respawn: 0,
    });
    Ok(Some(collections.len() - 1))
}

/// The Halo 1 weapon the map has for the Halo 2 weapon `h2_name`
/// ([`crate::sandbox::halo1_weapon_for`], first choice on the map).
fn halo1_weapon(h1: &H1Map, h2_name: &str) -> Option<String> {
    let last = |p: &str| p.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    let choices = crate::sandbox::halo1_weapon_for(h2_name)?;
    choices.iter().find_map(|c| {
        h1.tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"weap"))
            .find(|t| last(&t.name) == *c)
            .map(|t| t.name.clone())
    })
}

/// The Halo 1 sandbox's weapon list: each entry of the base's `mulg`
/// weapon list that names a Halo 2 weapon gets the Halo 1 weapon
/// [`crate::sandbox::halo1_weapon_for`] picks, the first of its choices
/// the map has, converted like the objects (datums from slot `first`).
/// Entries naming none stay none; an entry no Halo 1 weapon fits is
/// reported and left. The game variant's starting weapon is a list entry
/// (`crate::globals::STARTING_WEAPON_ENTRY`), and the Halo 1 cyborg can
/// hold only Halo 1 weapons.
fn plan_weapon_list<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
    first: usize,
    unresolved: &mut Vec<String>,
) -> Result<Vec<(usize, TagId)>> {
    let mut out = Vec::new();
    for (i, name) in base_list(base, crate::globals::List::Weapons)? {
        let name = name.as_str();
        let Some(h1_name) = halo1_weapon(h1, name) else {
            unresolved.push(format!("weapon list {i}: {name} has no Halo 1 weapon on the map"));
            continue;
        };
        match objects.object(*b"weap", &h1_name) {
            Ok(k) => out.push((i, added_datum(first + k))),
            Err(w) => unresolved.push(format!("weapon list {i}: {h1_name}: {w}")),
        }
    }
    Ok(out)
}

/// The base `mulg`'s universal `list` (`crate::globals::List`): each
/// entry that names a tag, with the tag's name ("?" for none). Empty when
/// the base has no globals or multiplayer globals.
fn base_list<R: Read + Seek>(
    base: &mut Map<R>,
    list: crate::globals::List,
) -> Result<Vec<(usize, String)>> {
    let find = |base: &Map<R>, group: &[u8; 4]| {
        base.tags()
            .iter()
            .find(|t| t.group == FourCC(*group))
            .map(|t| t.id)
    };
    let Some(matg) = find(base, b"matg") else {
        return Ok(Vec::new());
    };
    let g = base.read_tag_bytes(matg)?;
    let at = crate::globals::MULTIPLAYER_GLOBALS + 4;
    let mulg = TagId(u32::from_le_bytes(
        g.get(at..at + 4)
            .ok_or(Error::Invalid("the globals are too short"))?
            .try_into()
            .unwrap(),
    ));
    let Some(address) = base
        .tags()
        .iter()
        .find(|t| t.id == mulg)
        .and_then(|t| t.location)
        .map(|l| l.address)
    else {
        return Ok(Vec::new());
    };
    let bytes = base.read_tag_bytes(mulg)?;
    let entries = crate::globals::universal_list(&bytes, address, list)?;
    Ok(entries
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let id = (*id)?;
            let name = base
                .tags()
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.name.clone())
                .unwrap_or_else(|| "?".into());
            Some((i, name))
        })
        .collect())
}

/// The Halo 1 vehicle of `placed` (the Halo 1 vehicles the spawns place)
/// that stands for the Halo 2 vehicle `h2_name`: one whose Halo 2
/// equivalent ([`crate::sandbox::h2_equivalent`]) is the role named as
/// the vehicle (`warthog` for `objects\vehicles\warthog\warthog`, not
/// the rocket warthog's `warthog_gauss`). `None` when the spawns place
/// none of that role.
pub fn halo1_vehicle_for<'a>(h2_name: &str, placed: &'a [String]) -> Option<&'a str> {
    let last = h2_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(h2_name)
        .to_ascii_lowercase();
    crate::sandbox::vehicle_class(&last)?;
    placed
        .iter()
        .find(|n| matches!(h2_equivalent(n), Some(H2Item::Vehicle(r)) if r == last))
        .map(String::as_str)
}

/// The Halo 1 sandbox's vehicle list: each entry of the base's `mulg`
/// vehicle list ([`crate::globals::UNIVERSAL_VEHICLES`]) whose vehicle
/// the spawns place a Halo 1 vehicle for ([`halo1_vehicle_for`]) gets
/// that vehicle, so a game variant's vehicle option that swaps a vehicle
/// for another of its class gives a Halo 1 one. The other entries (the
/// wraith, the spectre, the turrets: Halo 1 has none, or the map places
/// none) keep the base's and are reported. As the list names the base's
/// vehicles, a pinned map kept every retail vehicle and all it names
/// (`fresh::prune_pinned`): pointed at the Halo 1 vehicles, those the
/// map's spawns stand in for are dropped.
fn plan_vehicle_list<R: Read + Seek>(
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
    first: usize,
    placed: &[String],
    unresolved: &mut Vec<String>,
) -> Result<Vec<(usize, TagId)>> {
    let mut out = Vec::new();
    for (i, name) in base_list(base, crate::globals::List::Vehicles)? {
        let Some(h1_name) = halo1_vehicle_for(&name, placed) else {
            unresolved.push(format!(
                "vehicle list {i}: {name} has no Halo 1 vehicle the map places; the base's stays"
            ));
            continue;
        };
        match objects.object(*b"vehi", h1_name) {
            Ok(k) => out.push((i, added_datum(first + k))),
            Err(w) => unresolved.push(format!("vehicle list {i}: {h1_name}: {w}")),
        }
    }
    Ok(out)
}

/// `objects` followed by `collections`, whose entries name the objects
/// by their datums once added from slot `first`.
fn with_collections(
    objects: ObjectTags,
    first: usize,
    collections: Vec<Halo1Collection>,
) -> ObjectTags {
    let ObjectTags {
        count,
        install,
        after,
        mut notes,
        hud_swaps,
        hud_sensor,
    } = objects;
    for c in &collections {
        notes.push(format!(
            "{} {}: {} item(s), respawn {} s",
            String::from_utf8_lossy(&c.group),
            c.name,
            c.entries.len(),
            c.respawn
        ));
    }
    ObjectTags {
        // The collections come after the objects: the pairs' indices hold.
        hud_swaps,
        hud_sensor,
        count: count + collections.len(),
        install: Box::new(move |set| {
            let mut ids = install(set)?;
            for c in &collections {
                let g = FourCC(c.group);
                set.add_group(g, None, None);
                let root = c.root(|k| added_datum(first + k));
                ids.push(
                    set.add_tag(g, &c.name, TagPayload::Built(root))
                        .map_err(|e| e.to_string())?,
                );
            }
            Ok(ids)
        }),
        after,
        notes,
    }
}

/// `scnr +0x50`: scenery placements (`0x5c` each, E: the object type
/// table `0x00468630`; `scenario_edit::layout`).
pub const SCENERY_PLACEMENTS: u32 = 0x50;
pub const SCENERY_PLACEMENT: usize = 0x5c;
/// `scnr +0x58`: the scenery palette (`0x28` each, the `scen` reference
/// first).
pub const SCENERY_PALETTE: u32 = 0x58;
pub const SCENERY_PALETTE_ENTRY: usize = 0x28;
/// The high word of a converted placement's unique ID (`+0x28`), as
/// `h1campaign` writes them.
pub const UNIQUE_ID_HIGH: u32 = 0x4831;
/// Halo 2's scenery object type (`+0x2e` of a placement).
const SCENERY_TYPE: u8 = 6;

/// Halo 1's scenery as Halo 2 scenario blocks: the palette (each a
/// converted `scen`'s datum once added) and the placements. Written into
/// the scenario of a fresh build ([`crate::fresh`]), which lays the
/// scenario out anew; the pinned build can't grow its palette.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scenery {
    /// The palette: one converted `scen` per Halo 1 palette entry that
    /// converted, in Halo 1's order.
    pub palette: Vec<TagId>,
    /// The placements, [`SCENERY_PLACEMENT`] bytes each.
    pub placements: Vec<Vec<u8>>,
    /// Halo 1 placements left out (their palette entry didn't convert).
    pub dropped: usize,
}

impl Scenery {
    /// The palette block (`+0x58`).
    pub fn palette_block(&self) -> Block {
        let mut b = Block::zeroed(SCENERY_PALETTE_ENTRY * self.palette.len());
        for (k, id) in self.palette.iter().enumerate() {
            b.tag_ref((k * SCENERY_PALETTE_ENTRY) as u32, *id);
        }
        b
    }

    /// The placements block (`+0x50`).
    pub fn placements_block(&self) -> Block {
        Block::new(self.placements.concat())
    }
}

/// One Halo 2 scenery placement from Halo 1's (`0x48`: `i16` palette
/// index `+0`, `u16` flags `+4`, position `+8`, rotation `+0x14`), on
/// palette entry `palette`: no name, Halo 1's "not placed automatically"
/// bit, unique ID `unique`, the fields `h1campaign` writes on every
/// placement.
pub fn scenery_placement(h1: &[u8], palette: u16, unique: u32) -> Vec<u8> {
    let mut b = vec![0u8; SCENERY_PLACEMENT];
    b[0..2].copy_from_slice(&palette.to_le_bytes());
    b[2..4].copy_from_slice(&u16::MAX.to_le_bytes());
    let flags = u32::from(u16::from_le_bytes([h1[4], h1[5]]) & 1);
    b[4..8].copy_from_slice(&flags.to_le_bytes());
    b[8..0x20].copy_from_slice(&h1[8..0x20]);
    b[0x28..0x2c].copy_from_slice(&(UNIQUE_ID_HIGH << 16 | unique).to_le_bytes());
    b[0x2c..0x2e].copy_from_slice(&u16::MAX.to_le_bytes());
    b[0x2e] = SCENERY_TYPE;
    b[0x2f] = 1;
    b[0x32..0x34].copy_from_slice(&u16::MAX.to_le_bytes());
    b
}

/// Converts Halo 1's scenery palette through `objects` (whose tags are
/// added from slot `first`) and plans its placements. Entries that don't
/// convert are listed in `unresolved`.
pub fn plan_scenery(
    h1: &H1Map,
    objects: &mut dyn Halo1Objects,
    first: usize,
    unresolved: &mut Vec<String>,
) -> Result<Scenery> {
    use h1map::scenario::layout;
    let scnr = h1.tag_data(h1.scenario())?;
    let space = h1.tag_space();
    let palette = space.block_at(scnr, layout::SCENERY_PALETTE.0, layout::SCENERY_PALETTE.1)?;
    let mut out = Scenery::default();
    // Halo 1 palette index -> Halo 2's.
    let mut index = Vec::new();
    for e in &palette {
        let name = h1map::TagRef::read(e, 0)
            .and_then(|r| h1.tag(r.id).ok())
            .map(|t| t.name.clone());
        let Some(name) = name else {
            index.push(None);
            continue;
        };
        match objects.object(*b"scen", &name) {
            Ok(k) => {
                index.push(Some(out.palette.len() as u16));
                out.palette.push(added_datum(first + k));
            }
            Err(w) => {
                unresolved.push(format!("scenery {name}: {w}"));
                index.push(None);
            }
        }
    }
    for e in space.block_at(scnr, layout::SCENERY.0, layout::SCENERY.1)? {
        let p = i16::from_le_bytes([e[0], e[1]]);
        match usize::try_from(p).ok().and_then(|p| index.get(p).copied().flatten()) {
            Some(k) => {
                let unique = out.placements.len() as u32;
                out.placements.push(scenery_placement(e, k, unique));
            }
            None => out.dropped += 1,
        }
    }
    Ok(out)
}

/// [`prepare`] for the Halo 2 sandbox with Halo 1's scenery: its `scen`
/// tags converted through `objects` and added after the vehicle
/// collections the placements build.
pub fn prepare_halo2<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: content::Sha256,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
) -> Result<Prepared> {
    let mut p = prepare(h1, source_sha256, base, Sandbox::Halo2)?;
    let first = base.index_slots() as usize + p.applied.collections.len();
    let scenery = plan_scenery(h1, objects, first, &mut p.applied.unresolved)?;
    p.objects = Some(objects.take());
    p.objects_first = first;
    p.scenery = Some(scenery);
    Ok(p)
}

/// The Halo 1 map's multiplayer player biped: the `bipd` named
/// `cyborg_mp`, else one named `cyborg`.
pub fn player_biped_name(h1: &H1Map) -> Option<String> {
    let bipeds = || h1.tags().iter().filter(|t| t.group.to_string() == "bipd");
    let last = |n: &str| n.rsplit('\\').next().unwrap_or(n).to_ascii_lowercase();
    bipeds()
        .find(|t| last(&t.name) == "cyborg_mp")
        .or_else(|| bipeds().find(|t| last(&t.name).contains("cyborg")))
        .map(|t| t.name.clone())
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
    (objects.after)(set, &ids).map_err(Error::Objects)
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
    fn a_collection_lists_each_object_at_its_weight_with_its_respawn_time() {
        use map::build::FixupKind;
        let c = Halo1Collection {
            group: *b"itmc",
            name: "weapons\\multiplayer\\random".into(),
            entries: vec![(3, 0.25), (7, 0.75)],
            respawn: 45,
        };
        let root = c.root(|k| added_datum(100 + k));
        assert_eq!(root.bytes.len(), COLLECTION_ROOT);
        assert_eq!(u16::from_le_bytes([root.bytes[8], root.bytes[9]]), 45);
        let [f] = root.fixups.as_slice() else {
            panic!("one block")
        };
        let FixupKind::Block { count, child } = &f.kind else {
            panic!("a block")
        };
        assert_eq!((f.offset, *count), (0, 2));
        assert_eq!(child.bytes.len(), 2 * COLLECTION_ENTRY);
        let w = |at: usize| f32::from_le_bytes(child.bytes[at..at + 4].try_into().unwrap());
        assert_eq!([w(0), w(COLLECTION_ENTRY)], [0.25, 0.75]);
        let refs: Vec<_> = child
            .fixups
            .iter()
            .map(|f| match f.kind {
                FixupKind::TagRef(id) => (f.offset, id),
                ref k => panic!("{k:?}"),
            })
            .collect();
        assert_eq!(refs, [(4, added_datum(103)), (0x14, added_datum(107))]);
    }

    #[test]
    fn a_scenery_placement_keeps_halo1_s_place_on_its_new_palette_entry() {
        let mut h1 = vec![0u8; 0x48];
        h1[0..2].copy_from_slice(&7i16.to_le_bytes());
        h1[4..6].copy_from_slice(&0x0003u16.to_le_bytes());
        for (k, v) in [1.0f32, -2.0, 3.5, 0.25, 0.0, -0.5].iter().enumerate() {
            h1[8 + 4 * k..12 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        let b = scenery_placement(&h1, 2, 9);
        assert_eq!(b.len(), SCENERY_PLACEMENT);
        let u16_at = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(u16_at(0), 2, "the Halo 2 palette entry");
        assert_eq!(u16_at(2), 0xffff, "no name");
        assert_eq!(u32_at(4), 1, "only Halo 1's not-placed-automatically bit");
        assert_eq!(b[8..0x20], h1[8..0x20], "position and rotation");
        assert_eq!(u32_at(0x28), UNIQUE_ID_HIGH << 16 | 9);
        assert_eq!((b[0x2e], b[0x2f]), (SCENERY_TYPE, 1));
        let sc = Scenery {
            palette: vec![added_datum(5), added_datum(6)],
            placements: vec![b.clone(), b],
            dropped: 0,
        };
        assert_eq!(sc.palette_block().bytes.len(), 2 * SCENERY_PALETTE_ENTRY);
        assert_eq!(sc.palette_block().fixups.len(), 2);
        assert_eq!(sc.placements_block().bytes.len(), 2 * SCENERY_PLACEMENT);
    }

    /// Each Coagulation vehicle list entry and the Blood Gulch vehicle
    /// that stands for it: the warthog's is the plain warthog, not the
    /// rocket warthog; the wraith, spectre and turrets have none, nor does
    /// a vehicle the spawns don't place.
    #[test]
    fn a_vehicle_list_entry_takes_the_placed_halo1_vehicle_of_its_role() {
        let placed: Vec<String> = [
            "vehicles\\rwarthog\\rwarthog",
            "vehicles\\warthog\\mp_warthog",
            "vehicles\\ghost\\ghost_mp",
            "vehicles\\scorpion\\scorpion_mp",
        ]
        .map(String::from)
        .to_vec();
        let v = |n: &str| halo1_vehicle_for(n, &placed);
        assert_eq!(
            v("objects\\vehicles\\warthog\\warthog"),
            Some("vehicles\\warthog\\mp_warthog")
        );
        assert_eq!(
            v("objects\\vehicles\\ghost\\ghost"),
            Some("vehicles\\ghost\\ghost_mp")
        );
        assert_eq!(
            v("objects\\vehicles\\scorpion\\scorpion"),
            Some("vehicles\\scorpion\\scorpion_mp")
        );
        assert_eq!(v("objects\\vehicles\\banshee\\banshee"), None);
        assert_eq!(v("objects\\vehicles\\wraith\\wraith"), None);
        assert_eq!(v("objects\\vehicles\\spectre\\spectre"), None);
        assert_eq!(v("objects\\vehicles\\c_turret_ap\\c_turret_ap"), None);
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
