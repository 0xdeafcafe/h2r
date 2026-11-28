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

use std::collections::HashSet;
use std::io::{Cursor, Read, Seek};

use h1map::H1Map;
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use scenario_edit::{BlockKind, Scenario, Value};

use super::{Prepared, Sandbox, prepare_plan};
use crate::apply::NETGAME_ITEM;
use crate::placements::{placement_entry, plan_map_except, spawn_points_and_flags, vehicle_tag};
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

    /// A light fixture machine with any authored placement lighting.
    fn fixture(
        &mut self,
        tag: &h1map::Tag,
        lighting: Option<crate::fixture::Lighting>,
    ) -> std::result::Result<usize, String>;

    /// The converted render model for Halo 1's first-person arms `id`
    /// (its globals' first-person interface), converting it the first
    /// time: its index among the tags [`ObjectTags::install`] adds.
    fn first_person_arms(&mut self, id: h1map::TagId) -> std::result::Result<usize, String>;

    /// The collision of the Halo 1 object tag `name` of `group`, in object
    /// space, as Halo 2 BSPs: for each collision node with any, its BSP of
    /// the first permutation of its region (the one a converted
    /// placement, which names no variant, draws), placed by the render
    /// model's default pose. Each surface names a global material index.
    /// Empty when it has no collision. Every implementor reads it (no
    /// default that reads none). [`merge_scenery_collision`] puts each
    /// placed scenery object's into the level's.
    fn collision(
        &mut self,
        group: [u8; 4],
        name: &str,
    ) -> std::result::Result<Vec<tags::collision::Bsp>, String>;

    /// Everything converted so far, ready to add to a map.
    fn take(&mut self) -> ObjectTags;
}

/// Converted object tags, to add to a `TagSet` made from the base map.
pub struct ObjectTags {
    /// How many tags [`Self::install`] adds.
    pub count: usize,
    /// Adds the tags to the set, in order, and returns their datums.
    pub install: Box<dyn Fn(&mut TagSet) -> std::result::Result<Vec<TagId>, String> + Send + Sync>,
    /// Run once every tag the placements name is in (the objects and the
    /// collections after them), with the datums `install` returned: adds
    /// what the objects name but no placement does (their sounds) after
    /// them, so it moves no datum a placement names.
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
    source: SpawnSource,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SpawnSource {
    Equipment([h1map::scenario::GameType; 4], u16),
    Vehicle(u8),
}

impl Halo1Spawn {
    /// Whether it spawns a vehicle (a `vehc`) rather than an item (`itmc`).
    pub fn is_vehicle(&self) -> bool {
        matches!(self.source, SpawnSource::Vehicle(_))
    }
}

/// Every item and vehicle spawn of a Halo 1 map, as Halo 1 objects.
pub fn plan_objects(h1: &H1Map) -> Result<Vec<Halo1Spawn>> {
    let mp = h1.multiplayer()?;
    let mut out = mp
        .equipment
        .iter()
        .map(|e| item_spawn(h1, e))
        .collect::<Result<Vec<_>>>()?;
    for (i, v) in mp.vehicles.iter().enumerate() {
        if let Some(tag) = vehicle_tag(h1, &mp, i, v)? {
            out.push(vehicle_spawn(h1, tag, v)?);
        }
    }
    Ok(out)
}

pub(crate) fn vehicle_spawn(
    h1: &H1Map,
    tag: &h1map::Tag,
    v: &h1map::scenario::ObjectPlacement,
) -> Result<Halo1Spawn> {
    let name = tag.name.as_str();
    let named = |reason: String| Error::no_rebuild(&h1.header().name, name, reason);
    let body = h1.tag_data(tag).map_err(|e| named(e.to_string()))?;
    let controller = body
        .get(0x2f4..0x2f6)
        .ok_or_else(|| named("missing vehicle type".into()))?;
    let class = match u16::from_le_bytes([controller[0], controller[1]]) {
        0 => 3,     // tank: heavy vehicle
        1 => 1,     // jeep: primary light vehicle
        3 | 5 => 4, // aircraft
        4 => 2,     // hover scout: secondary light vehicle
        6 => 6,     // stationary: primary turret
        other => return Err(named(format!("unsupported vehicle controller {other}"))),
    };
    let [x, y, z] = v.position;
    Ok(Halo1Spawn {
        collection: name.to_owned(),
        items: vec![Halo1Item {
            group: *b"vehi",
            name: name.to_owned(),
            weight: 1.0,
        }],
        collection_respawn: 0,
        position: [x, y, z + vehicle_tag_depth(h1, tag)?],
        facing: v.rotation[0],
        respawn: 0,
        source: SpawnSource::Vehicle(class),
    })
}

fn item_spawn(h1: &H1Map, e: &h1map::scenario::NetgameEquipment) -> Result<Halo1Spawn> {
    let ic = h1.item_collection(e.item.id)?;
    let mut items = Vec::with_capacity(ic.permutations.len());
    for p in ic.permutations {
        let t = h1.tag(p.item.id)?;
        items.push(Halo1Item {
            group: t.group.0.to_be_bytes(),
            name: t.name.clone(),
            weight: p.weight,
        });
    }
    Ok(Halo1Spawn {
        collection: h1.tag(e.item.id)?.name.clone(),
        items,
        collection_respawn: ic.spawn_time,
        position: e.position,
        facing: e.facing,
        respawn: e.spawn_time,
        source: SpawnSource::Equipment(e.game_types, e.team),
    })
}

/// How far the Halo 1 vehicle `name`'s render model reaches below its
/// origin (0 when it does not, or it names no model; a model that does
/// not read is an error naming the vehicle): what a vehicle spawn is
/// raised by, so the vehicle does not start in the floor.
///
/// V: Halo 1 places a vehicle's origin on the floor (every Blood Gulch
/// vehicle placement is 0.001 above the structure's collision), and the
/// hover vehicles reach below it: the ghost's hull to 0.143 under its
/// origin (`h1objects/tests/vehicle_ground_disc.rs`). Halo 1's own mass
/// points lifted it once spawned; in Halo 2 the converted ghost started
/// 0.14 into the ground and stayed there.
pub fn vehicle_depth(h1: &H1Map, name: &str) -> Result<f32> {
    let t = h1
        .find(h1map::Group::new(b"vehi"), name)
        .ok_or_else(|| Error::no_rebuild(&h1.header().name, name, "not a vehicle"))?;
    vehicle_tag_depth(h1, t)
}

fn vehicle_tag_depth(h1: &H1Map, t: &h1map::Tag) -> Result<f32> {
    let named = |why: String| {
        Error::no_rebuild(&h1.header().name, &t.name, format!("vehicle depth: {why}"))
    };
    let r = h1map::TagRef::read(h1.tag_data(t)?, VEHI_MODEL)
        .ok_or_else(|| named("model reference does not read".into()))?;
    // A vehicle with no model has nothing to reach below its origin.
    if r.id.is_none() {
        return Ok(0.0);
    }
    let mode = h1.tag(r.id)?;
    let (m, _) = h1_assets::models::model_from_map(h1, mode, &Default::default())
        .map_err(|e| named(format!("model {}: {e}", mode.name)))?;
    let lowest = m
        .geometries
        .iter()
        .flat_map(|g| &g.parts)
        .flat_map(|p| &p.vertices)
        .map(|v| v.position[2])
        .reduce(f32::min);
    Ok(lowest.map_or(0.0, |z| (-z).max(0.0)))
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
    pub fn root(
        &self,
        object: impl Fn(usize) -> TagId,
    ) -> std::result::Result<Block, tags::defs::DefError> {
        use tags::defs::{self, Defs, Val};

        let group = FourCC(self.group);
        let d = Defs::builtin()
            .group(group)
            .filter(|_| matches!(&self.group, b"itmc" | b"vehc"))
            .ok_or_else(|| defs::DefError {
                path: self.name.clone(),
                why: format!("invalid collection group {group}"),
            })?;
        let entry = d.child("entries").expect("collection entries");
        let mut choices = Vec::with_capacity(self.entries.len());
        for &(id, weight) in &self.entries {
            let mut r = defs::default_record(entry);
            r.set("weight", Val::Float(weight))?;
            // The map builder resolves the object's group from its datum.
            r.set("object", Val::tag(FourCC([0; 4]), object(id)))?;
            choices.push(r);
        }
        let mut r = defs::default_record(d);
        r.set("entries", Val::Block(choices))?;
        r.set("respawn_time", Val::Int(i64::from(self.respawn as i16)))?;
        defs::to_built(d, &r, &mut |_| None)
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

fn collect_spawns(
    map: &str,
    source: Vec<Halo1Spawn>,
    mut object: impl FnMut([u8; 4], &str) -> std::result::Result<usize, String>,
    placed_vehicles: &mut Vec<String>,
) -> Result<(Vec<Halo1Collection>, Vec<(Halo1Spawn, usize, u8)>)> {
    // Each Halo 1 collection some spawn uses, built once, and the spawns
    // naming it (collection index, netgame type).
    let mut collections: Vec<Halo1Collection> = Vec::new();
    // Each collection's netgame type: its first converted item's class
    // (weapons, grenades, powerups, a vehicle's).
    let mut spawns: Vec<(Halo1Spawn, usize, u8)> = Vec::new();
    let mut kinds = Vec::new();
    for sp in source {
        let group = if sp.is_vehicle() { *b"vehc" } else { *b"itmc" };
        let c = match collections
            .iter()
            .position(|c| c.group == group && c.name == sp.collection)
        {
            Some(c) => c,
            None => {
                let first = sp.items.first().ok_or_else(|| {
                    Error::no_rebuild(
                        map,
                        &sp.collection,
                        format!("empty collection at {:?}", sp.position),
                    )
                })?;
                let kind = match sp.source {
                    SpawnSource::Vehicle(class) => class,
                    SpawnSource::Equipment(..) => halo1_netgame_type(first.group, &first.name),
                };
                let mut entries = Vec::with_capacity(sp.items.len());
                for it in &sp.items {
                    let k = object(it.group, &it.name)
                        .map_err(|why| Error::no_rebuild(map, &it.name, why))?;
                    if &it.group == b"vehi" && !placed_vehicles.contains(&it.name) {
                        placed_vehicles.push(it.name.clone());
                    }
                    entries.push((k, it.weight));
                }
                collections.push(Halo1Collection {
                    group,
                    name: sp.collection.clone(),
                    entries,
                    respawn: sp.collection_respawn,
                });
                kinds.push(kind);
                collections.len() - 1
            }
        };
        spawns.push((sp, c, kinds[c]));
    }
    Ok((collections, spawns))
}

fn write_spawn(
    scenario: &mut Scenario,
    mut entry: Vec<u8>,
    sp: &Halo1Spawn,
    group: [u8; 4],
    id: TagId,
    kind: u8,
) -> Result<()> {
    let e = BlockKind::NetgameEquipment;
    if kind == 9 {
        if let SpawnSource::Equipment(types, team) = sp.source {
            // The four 16-bit game-type filters and the team are in the
            // same slots as on the source netgame equipment. Type 12 is
            // "all games" on both games' retail powerup placements.
            for (i, ty) in types.iter().enumerate() {
                entry[4 + 2 * i..6 + 2 * i].copy_from_slice(&ty.0.to_le_bytes());
            }
            entry[0xc..0xe].copy_from_slice(&team.to_le_bytes());
        }
    }
    // The scenario editor can write the placement fields. The reference
    // names a collection the builder will add after the converted objects.
    entry[NETGAME_ITEM..NETGAME_ITEM + 4].copy_from_slice(&FourCC(group).as_u32().to_le_bytes());
    entry[NETGAME_ITEM + 4..NETGAME_ITEM + 8].copy_from_slice(&id.0.to_le_bytes());
    let i = scenario.add(e, entry)?;
    scenario.set(e, i, "position", Value::Vec3(sp.position))?;
    scenario.set(e, i, "orientation", Value::Vec3([sp.facing, 0.0, 0.0]))?;
    scenario.set(e, i, "respawn time", Value::Int(sp.respawn.into()))?;
    scenario.set(e, i, "type", Value::Int(kind.into()))?;
    Ok(())
}

/// [`super::prepare`] for the Halo 1 sandbox: the Halo 2 preparation (structure,
/// spawns, flags), then every item and vehicle spawn replaced by its Halo
/// 1 object from `objects`. A failed object conversion names its source tag and map.
pub fn prepare_halo1<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: content::Sha256,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
) -> Result<Prepared> {
    let plan = spawn_points_and_flags(&h1.multiplayer()?);
    let mut p = prepare_plan(h1, source_sha256, base, plan)?;
    let template = p
        .scenario
        .new_entry(BlockKind::NetgameEquipment, None, None)?;
    let mut applied = std::mem::take(&mut p.applied);
    let first = base.index_slots() as usize;
    let mut placed_vehicles = Vec::new();
    let (mut collections, spawns) = collect_spawns(
        &h1.header().name,
        plan_objects(h1)?,
        |group, name| objects.object(group, name),
        &mut placed_vehicles,
    )?;
    // The player: Halo 1's multiplayer biped, converted like the objects.
    let player = player_biped_name(h1);
    let player_name = player.as_ref().ok_or_else(|| {
        Error::no_rebuild(
            &h1.header().name,
            "player biped",
            "no multiplayer biped in source globals",
        )
    })?;
    let player_biped = Some(added_datum(
        first
            + objects
                .object(*b"bipd", player_name)
                .map_err(|e| Error::no_rebuild(&h1.header().name, player_name, e))?,
    ));
    let mut scenery = plan_scenery(h1, objects, first, &mut applied.unresolved)?;
    p.fixtures = plan_fixtures(h1, objects, first, scenery.placements.len() as u32)?;
    merge_scenery_collision(objects, &mut p, &mut scenery, &mut applied.unresolved)?;
    // The player's first-person arms: Halo 1's, which Halo 1's
    // first-person graphs name node for node (the engine binds them by
    // name; Halo 2's arms match no node of them).
    let player_hands = match crate::globals::h1_first_person_hands(h1)? {
        Some(id) => Some(added_datum(
            first
                + objects.first_person_arms(id).map_err(|e| {
                    Error::no_rebuild(
                        &h1.header().name,
                        h1.tag(id).map_or("first-person arms", |t| &t.name),
                        e,
                    )
                })?,
        )),
        None => None,
    };
    // Halo 1's grenade types, converted like the objects.
    let mut grenades = Vec::new();
    for g in crate::globals::h1_grenades(h1)? {
        let mut one = |id: Option<h1map::TagId>, group: [u8; 4]| -> Result<Option<TagId>> {
            let Some(id) = id else { return Ok(None) };
            let tag = h1.tag(id)?;
            let k = objects
                .object(group, &tag.name)
                .map_err(|e| Error::no_rebuild(&h1.header().name, &tag.name, e))?;
            Ok(Some(added_datum(first + k)))
        };
        grenades.push(crate::globals::Grenade {
            maximum: g.maximum,
            equipment: one(g.equipment, *b"eqip")?,
            projectile: one(g.projectile, *b"proj")?,
        });
    }
    let weapon_list = plan_weapon_list(h1, base, objects, first)?;
    let vehicle_list = plan_vehicle_list(
        &h1.header().name, base,
        objects,
        first,
        &placed_vehicles,
    )?;
    let starting = plan_starting_equipment(h1, objects, &mut collections)?;
    let objects_out = objects.take();
    // The collections follow the objects.
    let after = first + objects_out.count;
    let s = &mut p.scenario;
    for (sp, c, kind) in &spawns {
        write_spawn(
            s,
            template.clone(),
            sp,
            collections[*c].group,
            added_datum(after + c),
            *kind,
        )?;
        applied.counts[2] += 1;
        if sp.is_vehicle() {
            applied.vehicles += 1;
        }
    }
    // The starting equipment names the Halo 1 collections made for it.
    let k = BlockKind::StartingEquipment;
    while !s.block(k).is_empty() {
        s.remove(k, s.block(k).len() - 1)?;
    }
    for row in starting {
        let mut entry = s.new_entry(k, None, None)?;
        entry[..12].copy_from_slice(&row.filters);
        for (at, collection) in STARTING_EQUIPMENT_COLLECTIONS
            .into_iter()
            .zip(row.collections)
        {
            let id = collection.map_or(TagId::NULL, |index| added_datum(after + index));
            entry[at..at + 4].copy_from_slice(&FourCC(*b"itmc").as_u32().to_le_bytes());
            entry[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes());
        }
        s.add(k, entry)?;
    }
    let tags_out = with_collections(objects_out, first, collections);
    p.applied = applied;
    p.sandbox = Sandbox::Halo1;
    p.objects = Some(tags_out);
    p.objects_first = first;
    p.player_biped = player_biped;
    p.scenery = Some(scenery);
    p.player_hands = player_hands;
    p.grenades = grenades;
    p.weapon_list = weapon_list;
    p.vehicle_list = vehicle_list;
    p.read_pickup_text(h1)?;
    Ok(p)
}

/// The starting equipment's collection references (`scnr +0x128`,
/// `0x9c` each, five `itmc` references from `+0x3c`, 8 apart). E: with the
/// variant's starting weapon at 0 (the map's default, which most built-in
/// variants hold), `0x0019ef40` rolls them (`0x0019efef`, `0x0019f1a0`)
/// for the player's weapons.
pub const STARTING_EQUIPMENT_COLLECTIONS: [usize; 5] = [0x3c, 0x44, 0x4c, 0x54, 0x5c];

struct StartingEquipment {
    filters: [u8; 12],
    collections: [Option<usize>; 5],
}

/// H1 starting equipment rows have flags, four game-type selectors and
/// collection references at +0x3c (16 bytes apart). The H2 spawn code
/// reads the same selectors and rolls its five compact references.
fn plan_starting_equipment(
    h1: &H1Map,
    objects: &mut dyn Halo1Objects,
    collections: &mut Vec<Halo1Collection>,
) -> Result<Vec<StartingEquipment>> {
    let (at, stride) = h1map::scenario::layout::STARTING_EQUIPMENT;
    let scnr = h1.tag_data(h1.scenario())?;
    let space = h1.tag_space();
    let mut rows = Vec::new();
    for src in space.block_at(scnr, at, stride)? {
        let mut row = StartingEquipment {
            filters: src[..12].try_into().expect("starting equipment header"),
            collections: [None; 5],
        };
        for i in 0..6 {
            let r = h1map::TagRef::read(src, 0x3c + 16 * i).expect("starting equipment reference");
            if r.id.is_none() {
                continue;
            }
            let tag = h1.tag(r.id)?;
            if i >= row.collections.len() {
                return Err(Error::no_rebuild(
                    &h1.header().name,
                    &tag.name,
                    "sixth starting collection has no H2 slot",
                ));
            }
            let existing = collections
                .iter()
                .position(|c| c.group == *b"itmc" && c.name == tag.name);
            let index = if let Some(index) = existing {
                index
            } else {
                let collection = h1.item_collection(r.id)?;
                let mut entries = Vec::new();
                for p in collection.permutations {
                    let item = h1.tag(p.item.id)?;
                    let index = objects
                        .object(item.group.0.to_be_bytes(), &item.name)
                        .map_err(|why| Error::no_rebuild(&h1.header().name, &item.name, why))?;
                    entries.push((index, p.weight));
                }
                collections.push(Halo1Collection {
                    group: *b"itmc",
                    name: tag.name.clone(),
                    entries,
                    respawn: collection.spawn_time,
                });
                collections.len() - 1
            };
            row.collections[i] = Some(index);
        }
        rows.push(row);
    }
    Ok(rows)
}

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
/// unassigned. The game variant's starting weapon is a list entry
/// (`crate::globals::STARTING_WEAPON_ENTRY`), and the Halo 1 cyborg can
/// hold only Halo 1 weapons.
fn plan_weapon_list<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
    first: usize,
) -> Result<Vec<(usize, TagId)>> {
    let mut out = Vec::new();
    for (i, name) in base_list(base, crate::globals::List::Weapons)? {
        let name = name.as_str();
        let Some(h1_name) = halo1_weapon(h1, name) else {
            out.push((i, TagId::NULL));
            continue;
        };
        match objects.object(*b"weap", &h1_name) {
            Ok(k) => out.push((i, added_datum(first + k))),
            Err(w) => return Err(Error::no_rebuild(&h1.header().name, &h1_name, w)),
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
/// the rocket warthog's `warthog_gauss`); else, for a role Halo 1 has no
/// vehicle of, one of the same netgame type
/// ([`crate::sandbox::VehicleClass::netgame_type`]: the wraith takes the
/// scorpion, the other heavy vehicle the variant's heavy chooser picks
/// between). `None` when the spawns place neither, or the vehicle has no
/// class (the spectre, the turrets).
pub fn halo1_vehicle_for<'a>(h2_name: &str, placed: &'a [String]) -> Option<&'a str> {
    let last = h2_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(h2_name)
        .to_ascii_lowercase();
    let class = crate::sandbox::vehicle_class(&last)?;
    let role = |n: &str| match h2_equivalent(n) {
        Some(H2Item::Vehicle(r)) => Some(r),
        _ => None,
    };
    placed
        .iter()
        .find(|n| role(n) == Some(last.as_str()))
        .or_else(|| {
            // Only a role no Halo 1 vehicle stands for falls back.
            if crate::sandbox::TABLE
                .iter()
                .any(|(_, i)| *i == H2Item::Vehicle(class.role))
            {
                return None;
            }
            placed.iter().find(|n| {
                role(n)
                    .and_then(crate::sandbox::vehicle_class)
                    .is_some_and(|c| c.netgame_type == class.netgame_type)
            })
        })
        .map(String::as_str)
}

/// The Halo 1 sandbox's vehicle list: each entry of the base's `mulg`
/// vehicle list ([`crate::globals::UNIVERSAL_VEHICLES`]) whose vehicle
/// the spawns place a Halo 1 vehicle for ([`halo1_vehicle_for`]) gets
/// that vehicle, so a game variant's vehicle option that swaps a vehicle
/// for another of its class gives a Halo 1 one. The other entries (the
/// spectre and the turrets, which have no class; a vehicle the map
/// places none of, as Blood Gulch's banshee) are unassigned. As the list names the base's
/// vehicles, a pinned map kept every retail vehicle and all it names
/// (`fresh::prune_pinned`): pointed at the Halo 1 vehicles, those the
/// map's spawns stand in for are dropped.
fn plan_vehicle_list<R: Read + Seek>(
    map: &str,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
    first: usize,
    placed: &[String],
) -> Result<Vec<(usize, TagId)>> {
    let mut out = Vec::new();
    for (i, name) in base_list(base, crate::globals::List::Vehicles)? {
        let Some(h1_name) = halo1_vehicle_for(&name, placed) else {
            out.push((i, TagId::NULL));
            continue;
        };
        match objects.object(*b"vehi", h1_name) {
            Ok(k) => out.push((i, added_datum(first + k))),
            Err(w) => return Err(Error::no_rebuild(map, h1_name, w)),
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
                if c.group == *b"vehc" {
                    for &(k, _) in &c.entries {
                        if ids.get(k).and_then(|id| set.tag(*id)).map(|t| t.group)
                            != Some(FourCC(*b"vehi"))
                        {
                            return Err(format!(
                                "{}: vehicle collection {} needs rebuilt vehi at index {k}",
                                set.name, c.name
                            ));
                        }
                    }
                }
                set.add_group(g, None, None);
                let root = c
                    .root(|k| added_datum(first + k))
                    .map_err(|e| e.to_string())?;
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
    /// Source placements for which Halo 1 creates no object.
    pub empty_placements: usize,
    /// Each placement written, as Halo 1 has it, in order.
    pub placed: Vec<PlacedScenery>,
    /// Placements whose collision joined the level's
    /// ([`merge_scenery_collision`]).
    pub solid: usize,
}

/// A placed Halo 1 scenery object: its tag, where it stands, and whether
/// it is there from the start (Halo 1's "not placed automatically" bit,
/// `+4` bit 0, clear).
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedScenery {
    pub name: String,
    /// Position (`+8`).
    pub position: [f32; 3],
    /// Yaw, pitch and roll in radians (`+0x14`).
    pub rotation: [f32; 3],
    pub at_start: bool,
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

pub(crate) const MACHINE_PLACEMENTS: u32 = 0xa8;
pub(crate) const MACHINE_PALETTE: u32 = 0xb0;
const MACHINE_PLACEMENT: usize = 0x48;
const MACHINE_PALETTE_ENTRY: usize = 0x28;

#[derive(Default)]
pub(crate) struct Fixtures {
    pub palette: Vec<TagId>,
    pub placements: Vec<Vec<u8>>,
}

impl Fixtures {
    pub fn palette_block(&self) -> Block {
        let mut block = Block::zeroed(self.palette.len() * MACHINE_PALETTE_ENTRY);
        for (i, &id) in self.palette.iter().enumerate() {
            block.tag_ref((i * MACHINE_PALETTE_ENTRY) as u32, id);
        }
        block
    }

    pub fn patch_pinned(&self, mut map: Map<Cursor<Vec<u8>>>) -> Result<Map<Cursor<Vec<u8>>>> {
        let address = map
            .scenario()
            .and_then(|t| t.location)
            .ok_or(Error::Invalid("the pinned scenario has no payload"))?
            .address;
        let mut palette = Block::zeroed(self.palette.len() * MACHINE_PALETTE_ENTRY);
        for (i, id) in self.palette.iter().enumerate() {
            let at = i * MACHINE_PALETTE_ENTRY;
            palette.put_u32(at, FourCC(*b"mach").as_u32());
            palette.put_u32(at + 4, id.0);
        }
        let mut editor = map.edit();
        for (field, count, bytes) in [
            (MACHINE_PALETTE, self.palette.len(), palette.bytes),
            (
                MACHINE_PLACEMENTS,
                self.placements.len(),
                self.placements.concat(),
            ),
        ] {
            let offset = editor.append_metadata(&bytes, 16)?;
            let mut descriptor = Block::zeroed(8);
            descriptor.put_u32(0, count as u32);
            descriptor.put_u32(4, offset);
            editor.patch_metadata(address + field, &descriptor.bytes)?;
        }
        let mut out = Cursor::new(Vec::new());
        editor.write_to(&mut out)?;
        Ok(Map::from_reader(Cursor::new(out.into_inner()))?)
    }
}

fn plan_fixtures(
    h1: &H1Map,
    objects: &mut dyn Halo1Objects,
    first: usize,
    first_unique: u32,
) -> Result<Fixtures> {
    use crate::fixture::{Lighting, PALETTE, PLACEMENTS};

    let scnr = h1.tag_data(h1.scenario())?;
    let space = h1.tag_space();
    let palette = space.block_at(scnr, PALETTE.0, PALETTE.1)?;
    let mut out = Fixtures::default();
    for (index, row) in space
        .block_at(scnr, PLACEMENTS.0, PLACEMENTS.1)?
        .into_iter()
        .enumerate()
    {
        let slot = u16::from_le_bytes([row[0], row[1]]) as usize;
        let failure = |reason: &str| {
            Error::no_rebuild(&h1.header().name, format!("light fixture {index}"), reason)
        };
        let source = palette
            .get(slot)
            .and_then(|row| h1map::TagRef::read(row, 0))
            .filter(|r| !r.id.is_none())
            .ok_or_else(|| failure("placement has no fixture palette entry"))?;
        let tag = h1.tag(source.id)?;
        if row[2..4] != [0xff; 2] || row[0x28..0x2c] != [0xff; 4] {
            return Err(failure("named or grouped multiplayer light fixture"));
        }
        let lighting =
            Lighting::read(row).map_err(|e| Error::no_rebuild(&h1.header().name, &tag.name, e))?;
        let variant = objects
            .fixture(tag, lighting)
            .map_err(|e| Error::no_rebuild(&h1.header().name, &tag.name, e))?;
        let id = added_datum(first + variant);
        let slot = out
            .palette
            .iter()
            .position(|&existing| existing == id)
            .unwrap_or_else(|| {
                let slot = out.palette.len();
                out.palette.push(id);
                slot
            });
        let unique = first_unique + index as u32;
        let mut placement = scenery_placement(row, slot as u16, unique);
        placement.truncate(MACHINE_PLACEMENT);
        placement[0x2e] = 7;
        placement[0x34..0x3c].copy_from_slice(&row[0x28..0x30]);
        out.placements.push(placement);
    }
    Ok(out)
}

/// Converts Halo 1's scenery palette through `objects` (whose tags are
/// added from slot `first`) and plans its placements. Only H1's two
/// no-object sentinels count as empty placements.
pub fn plan_scenery(
    h1: &H1Map,
    objects: &mut dyn Halo1Objects,
    first: usize,
    _unresolved: &mut Vec<String>,
) -> Result<Scenery> {
    use h1map::scenario::layout;
    let scnr = h1.tag_data(h1.scenario())?;
    let space = h1.tag_space();
    let palette = space.block_at(scnr, layout::SCENERY_PALETTE.0, layout::SCENERY_PALETTE.1)?;
    let mut out = Scenery::default();
    // Halo 1 palette index -> Halo 2's, and the tag's name.
    let mut index = Vec::new();
    for (i, e) in palette.iter().enumerate() {
        let named =
            |why: String| Error::no_rebuild(&h1.header().name, format!("scenery palette {i}"), why);
        let r = h1map::TagRef::read(e, 0).ok_or_else(|| named("truncated reference".into()))?;
        if r.id.is_none() {
            index.push(None);
            continue;
        }
        let tag = h1.tag(r.id).map_err(|e| named(e.to_string()))?;
        if tag.group != h1map::Group::new(b"scen") {
            return Err(named(format!("{} names a {}", tag.name, tag.group)));
        }
        let object = objects
            .object(*b"scen", &tag.name)
            .map_err(|e| Error::no_rebuild(&h1.header().name, &tag.name, e))?;
        let slot =
            i16::try_from(out.palette.len()).map_err(|_| named("palette exceeds i16".into()))?;
        index.push(Some((slot as u16, tag.name.clone())));
        out.palette.push(added_datum(first + object));
    }
    let placements = space.block_at(scnr, layout::SCENERY.0, layout::SCENERY.1)?;
    for (row, e) in placements.into_iter().enumerate() {
        let p = i16::from_le_bytes([e[0], e[1]]);
        match placement_entry(&h1.header().name, "scenery", row, p, &index)?
            .cloned()
            .flatten()
        {
            Some((k, name)) => {
                let unique = out.placements.len() as u32;
                out.placements.push(scenery_placement(e, k, unique));
                let f = |at: usize| f32::from_le_bytes(e[at..at + 4].try_into().expect("4"));
                out.placed.push(PlacedScenery {
                    name,
                    position: [f(8), f(0xc), f(0x10)],
                    rotation: [f(0x14), f(0x18), f(0x1c)],
                    at_start: u16::from_le_bytes([e[4], e[5]]) & 1 == 0,
                });
            }
            None => out.empty_placements += 1,
        }
    }
    Ok(out)
}

/// Puts the collision of every placed scenery object that has one into
/// the level's ([`crate::scenery_collision`]): the engine gives scenery no
/// Havok body, so Halo 1's rocks and trees are solid only as structure,
/// the way retail Halo 2 builds its rocks. Each placement there from the
/// start has its object's collision ([`Halo1Objects::collision`]) moved
/// to its place and merged into `p.collision`, its new leaves in the
/// cluster of the level leaf each stands in (`p.sound_leaves`); its
/// surfaces keep the substance of their source object collision material. The object still
/// draws as scenery. A placement whose collision can't be read or doesn't
/// fit Halo 2's packed fields fails the map conversion.
pub fn merge_scenery_collision(
    objects: &mut dyn Halo1Objects,
    p: &mut Prepared,
    scenery: &mut Scenery,
    _unresolved: &mut Vec<String>,
) -> Result<()> {
    use crate::scenery_collision::{Pose, merge_materials, transformed};
    let mut collisions: std::collections::HashMap<String, Vec<tags::collision::Bsp>> =
        std::collections::HashMap::new();
    for placed in scenery.placed.iter().filter(|s| s.at_start) {
        let name = &placed.name;
        let bsps = match collisions.entry(name.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut bsps = objects
                    .collision(*b"scen", name)
                    .map_err(|e| Error::no_rebuild(&p.h1_name, name, e))?;
                for surface in bsps.iter_mut().flat_map(|bsp| &mut bsp.surfaces) {
                    let material = (None, surface.material);
                    let palette = &mut p.textures.collision;
                    let index = palette
                        .iter()
                        .position(|row| *row == material)
                        .unwrap_or_else(|| {
                            palette.push(material);
                            palette.len() - 1
                        });
                    surface.material = u16::try_from(index).map_err(|_| {
                        Error::no_rebuild(&p.h1_name, name, "collision material index overflow")
                    })?;
                }
                p.collision_materials = p.textures.collision.len();
                entry.insert(bsps)
            }
        };
        let pose = Pose::placement(placed.position, placed.rotation);
        let mut joined = false;
        for bsp in bsps.iter() {
            match transformed(bsp, &pose)
                .and_then(|world| merge_materials(&mut p.collision, &world, |index| index))
            {
                Ok(m) => {
                    for l in m.stand_ins {
                        let c = p.sound_leaves.get(l).copied().unwrap_or(0);
                        p.sound_leaves.push(c);
                    }
                    joined |= m.slots > 0;
                }
                Err(e) => return Err(Error::no_rebuild(
                    &p.h1_name, name, format!("scenery collision at {:?}: {e}", placed.position),
                )),
            }
        }
        scenery.solid += usize::from(joined);
    }
    // The merged level as a whole, once: each piece was checked as it joined.
    crate::collision::check_bounds(&p.collision)
        .map_err(|e| Error::no_rebuild(&p.h1_name, "scenery collision", e.to_string()))?;
    Ok(())
}

/// Prepares the Halo 2 sandbox, retaining Halo 1's scenery and native
/// pickups and vehicles without a native counterpart. A retained pickup
/// collection keeps its original weighted choices, placement and respawn,
/// even if the base has a similar collection of its own.
pub fn prepare_halo2<R: Read + Seek>(
    h1: &H1Map,
    source_sha256: content::Sha256,
    base: &mut Map<R>,
    objects: &mut dyn Halo1Objects,
) -> Result<Prepared> {
    let mut retained_spawns = Vec::new();
    let mut retained = HashSet::new();
    let mut checked = HashSet::new();
    for e in &h1.multiplayer()?.equipment {
        if checked.insert(e.item.id) {
            for item in h1.item_collection(e.item.id)?.permutations {
                let t = h1.tag(item.item.id)?;
                if t.group == h1map::Group::new(b"eqip") {
                    let b = h1.tag_data(t)?;
                    if b.get(0x308..0x30a).is_some_and(|b| {
                        crate::sandbox::native_powerup(i16::from_le_bytes([b[0], b[1]]))
                    }) {
                        retained.insert(e.item.id);
                        break;
                    }
                }
            }
        }
        if retained.contains(&e.item.id) {
            retained_spawns.push(item_spawn(h1, e)?);
        }
    }
    let mut plan = plan_map_except(h1, &retained)?;
    retained_spawns.append(&mut plan.source_vehicles);
    let mut p = prepare_plan(h1, source_sha256, base, plan)?;
    let first = base.index_slots() as usize + p.applied.collections.len();
    let mut scenery = plan_scenery(h1, objects, first, &mut p.applied.unresolved)?;
    p.fixtures = plan_fixtures(h1, objects, first, scenery.placements.len() as u32)?;
    let mut unresolved = std::mem::take(&mut p.applied.unresolved);
    merge_scenery_collision(objects, &mut p, &mut scenery, &mut unresolved)?;
    p.applied.unresolved = unresolved;
    let (collections, spawns) = collect_spawns(
        &h1.header().name,
        retained_spawns,
        |group, name| objects.object(group, name),
        &mut Vec::new(),
    )?;
    let converted = objects.take();
    let after = first + converted.count;
    for (sp, c, kind) in spawns {
        // Retained equipment uses powerup settings, as on Beaver Creek
        // and Zanzibar. Vehicles use the vehicle settings for their class.
        let entry = p
            .scenario
            .new_entry(BlockKind::NetgameEquipment, None, None)?;
        write_spawn(
            &mut p.scenario,
            entry,
            &sp,
            collections[c].group,
            added_datum(after + c),
            if sp.is_vehicle() { kind } else { 9 },
        )?;
        p.applied.counts[2] += 1;
        p.applied.vehicles += usize::from(sp.is_vehicle());
    }
    p.objects = Some(with_collections(converted, first, collections));
    p.objects_first = first;
    p.scenery = Some(scenery);
    p.read_pickup_text(h1)?;
    Ok(p)
}

impl Prepared {
    fn read_pickup_text(&mut self, map: &H1Map) -> Result<()> {
        self.pickup_text = Some(
            h1_assets::text::multiplayer::PickupText::read(map, self.sandbox == Sandbox::Halo1)
                .map_err(|e| Error::Objects(format!("{} pickup text: {e}", self.h1_name)))?,
        );
        Ok(())
    }
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
    use crate::convert::{COLLECTION_ENTRY, COLLECTION_ROOT};

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
        let root = c.root(|k| added_datum(100 + k)).unwrap();
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
    fn unmatched_vehicle_is_rebuilt_once_and_placed_at_both_source_sites() {
        use h1map::scenario::layout::{VEHICLE_PALETTE, VEHICLES};
        use h1map::testing::{BASE, SCNR_ID, Spec, build_with_extra_tags, group, put};
        use map::build::FixupKind;

        const NAME: &str = r"vehicles\custom\ghost_transport";
        let mut built = build_with_extra_tags(
            &Default::default(),
            vec![Spec {
                group: b"vehi",
                parent: Some(b"unit"),
                name: NAME,
                data: Some(vec![0; 0x600]),
            }],
        );
        let scnr = built.data_at[0].unwrap();
        let id = built.data_at.len() - 1;
        let vehicle = built.data_at[id].unwrap();
        built.file[scnr..scnr + 0x600].fill(0);
        put(&mut built.file, vehicle + VEHI_MODEL + 12, u32::MAX);
        put(&mut built.file, vehicle + 0x2f4, 3); // dropship
        let palette = vehicle + 0x340;
        let rows = palette + 2 * VEHICLE_PALETTE.1;
        for (field, count, at) in [(VEHICLE_PALETTE.0, 2, palette), (VEHICLES.0, 4, rows)] {
            put(&mut built.file, scnr + field, count);
            put(
                &mut built.file,
                scnr + field + 4,
                BASE + (at - h1map::HEADER_LEN) as u32,
            );
        }
        put(&mut built.file, palette, group(b"vehi"));
        put(&mut built.file, palette + 12, SCNR_ID + id as u32);
        put(&mut built.file, palette + VEHICLE_PALETTE.1 + 12, u32::MAX);
        for (row, index) in [0i16, -1, 1, 0].into_iter().enumerate() {
            let at = rows + row * VEHICLES.1;
            built.file[at..at + 2].copy_from_slice(&index.to_le_bytes());
            put(&mut built.file, at + 8, (row as f32).to_bits());
            put(&mut built.file, at + 0x14, (0.25 * row as f32).to_bits());
        }
        let map = H1Map::from_decompressed(built.file).unwrap();
        let plan = crate::placements::plan_map(&map).unwrap();
        assert!(plan.dropped.is_empty());
        assert!(plan.items.is_empty());
        assert_eq!(plan.source_vehicles, plan_objects(&map).unwrap());
        let mut calls = Vec::new();
        let (collections, spawns) = collect_spawns(
            "tiny",
            plan.source_vehicles,
            |group, path| {
                calls.push((group, path.to_owned()));
                Ok(5)
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(calls, [(*b"vehi", NAME.to_owned())]);
        let [collection] = collections.as_slice() else {
            panic!("expected one vehicle collection")
        };
        assert_eq!(collection.group, *b"vehc");
        let root = collection.root(added_datum).unwrap();
        let FixupKind::Block { count, child } = &root.fixups[0].kind else {
            panic!("entries")
        };
        assert_eq!(*count, 1);
        assert_eq!(child.fixups[0].kind, FixupKind::TagRef(added_datum(5)));
        assert_eq!(spawns.len(), 2);
        for ((spawn, collection_index, kind), x) in spawns.iter().zip([0.0, 3.0]) {
            assert_eq!((*collection_index, *kind), (0, 4));
            assert_eq!(spawn.position, [x, 0.0, 0.0]);
            assert_eq!(spawn.facing, x * 0.25);
        }
    }

    #[test]
    fn malformed_palette_indices_identify_the_map_and_placement() {
        for index in [-2, 2] {
            let err = placement_entry("a10", "scenery", 17, index, &[1, 2]).unwrap_err();
            let message = err.to_string();
            assert!(message.contains("a10"));
            assert!(message.contains("scenery placement 17"));
            assert!(message.contains(&format!("palette index {index}")));
        }
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
            ..Scenery::default()
        };
        assert_eq!(sc.palette_block().bytes.len(), 2 * SCENERY_PALETTE_ENTRY);
        assert_eq!(sc.palette_block().fixups.len(), 2);
        assert_eq!(sc.placements_block().bytes.len(), 2 * SCENERY_PLACEMENT);
    }

    /// Each Coagulation vehicle list entry and the Blood Gulch vehicle
    /// that stands for it: the warthog's is the plain warthog, not the
    /// rocket warthog; the wraith's the scorpion (Halo 1 has no wraith;
    /// both are heavy vehicles); the spectre and turrets have none, nor
    /// has the banshee when the spawns place none.
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
        // Halo 1 has no wraith: the other heavy vehicle stands for it.
        assert_eq!(
            v("objects\\vehicles\\wraith\\wraith"),
            Some("vehicles\\scorpion\\scorpion_mp")
        );
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
