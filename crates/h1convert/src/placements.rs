//! Halo 1 multiplayer placements as Halo 2 ones, for the Halo 2 sandbox.
//!
//! [`plan`] turns the scenario's multiplayer parts into game-neutral
//! records: spawn points, netgame flags with a Halo 2 kind, and item spawns
//! with a Halo 2 role. Writing them into a base map's scenario is the
//! scenario editor's job; [`resolve`] finds the base map's item collection
//! for a role.

use h1map::H1Map;
use h1map::scenario::{FlagKind, Multiplayer};

use crate::Result;
use crate::convert::halo1::{Halo1Spawn, vehicle_spawn};
use crate::sandbox::{H2Item, h2_equivalent, h2_name_matches};

/// A Halo 2 netgame flag kind, by role; [`H2Flag::raw`] is the stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Flag {
    /// Where a team's flag spawns.
    CtfFlagSpawn,
    /// Where a team returns the enemy flag.
    CtfFlagReturn,
    /// Oddball spawn.
    OddballSpawn,
    /// Teleporter entrance; its channel is written to the identifier
    /// (`+0x14`), which Halo 2 pairs entrances and exits by.
    TeleporterSource,
    /// Teleporter exit.
    TeleporterDestination,
    /// A King of the Hill hill marker.
    Hill,
}

impl H2Flag {
    /// The stored `u16` (`scnr+0x118` element `+0x10`). The values rest on
    /// what the disc shows (`tests/h2_scenario.rs`, all 12 multiplayer
    /// maps): 0 and 1 come once per team on every map, 4 is always neutral (team
    /// 8) and two or three per map, 7 and 8 come in equal numbers on the
    /// five maps that have them, 11 and up are runs of hill boundary
    /// points. Not yet confirmed from engine code.
    pub fn raw(self) -> u16 {
        match self {
            Self::CtfFlagSpawn => 0,
            Self::CtfFlagReturn => 1,
            Self::OddballSpawn => 4,
            Self::TeleporterSource => 7,
            Self::TeleporterDestination => 8,
            // The first King of the Hill hill.
            Self::Hill => 11,
        }
    }
}

/// A spawn point.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Team.
    pub team: u16,
}

/// A netgame flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Halo 2 kind.
    pub kind: H2Flag,
    /// Team, or teleporter channel, or hill number.
    pub team: u16,
}

/// An item or vehicle spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSpawn {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// What spawns (never [`H2Item::Dropped`]).
    pub item: H2Item,
    /// Respawn seconds, 0 for the collection's default.
    pub respawn: u16,
}

/// Everything the Halo 2 sandbox variant places.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Player spawns.
    pub spawns: Vec<Spawn>,
    /// Netgame flags.
    pub flags: Vec<Flag>,
    /// Weapon, powerup and vehicle spawns.
    pub items: Vec<ItemSpawn>,
    /// Source vehicles that require a rebuilt vehicle collection.
    pub source_vehicles: Vec<Halo1Spawn>,
    /// What was left out and why, one line each.
    pub dropped: Vec<String>,
}

/// H1 0x9032d recognizes exactly -1 before indexing the palette. A valid
/// entry can itself contain a null tag (0x9038e).
pub fn placement_entry<'a, T>(
    map: &str,
    kind: &str,
    row: usize,
    index: i16,
    palette: &'a [T],
) -> Result<Option<&'a T>> {
    if index == -1 {
        return Ok(None);
    }
    palette.get(index as usize).map(Some).ok_or_else(|| {
        crate::Error::no_rebuild(
            map,
            format!("{kind} placement {row}"),
            format!("palette index {index} outside {} entries", palette.len()),
        )
    })
}

/// The flags a Halo 1 flag becomes (a Halo 1 CTF flag is both where the
/// flag spawns and where it's captured).
pub fn h2_flags(kind: FlagKind) -> &'static [H2Flag] {
    match kind {
        FlagKind::CtfFlag => &[H2Flag::CtfFlagSpawn, H2Flag::CtfFlagReturn],
        FlagKind::OddballBall => &[H2Flag::OddballSpawn],
        FlagKind::TeleportFrom => &[H2Flag::TeleporterSource],
        FlagKind::TeleportTo => &[H2Flag::TeleporterDestination],
        FlagKind::HillFlag => &[H2Flag::Hill],
        _ => &[],
    }
}

/// Plans the Halo 2 sandbox placements of `mp`; `item_name` gives the tag
/// path of every item an item collection may spawn (see [`plan_map`]).
pub fn plan(mp: &Multiplayer, item_names: impl Fn(usize) -> Vec<String>) -> Plan {
    plan_selected(mp, |i| Some(item_names(i)))
}

fn plan_selected(mp: &Multiplayer, item_names: impl Fn(usize) -> Option<Vec<String>>) -> Plan {
    let mut out = Plan {
        spawns: mp
            .starting_locations
            .iter()
            .map(|s| Spawn {
                position: s.position,
                facing: s.facing,
                team: s.team,
            })
            .collect(),
        ..Plan::default()
    };
    for f in &mp.flags {
        let kinds = h2_flags(f.kind);
        if kinds.is_empty() {
            out.dropped
                .push(format!("flag {:?} at {:?}", f.kind, f.position));
        }
        for &kind in kinds {
            out.flags.push(Flag {
                position: f.position,
                facing: f.facing,
                kind,
                team: f.team,
            });
        }
    }
    for (i, e) in mp.equipment.iter().enumerate() {
        // An item collection with several items is rare on multiplayer maps;
        // the first one that maps to something is used.
        let Some(names) = item_names(i) else { continue };
        let item = names
            .iter()
            .filter_map(|n| h2_equivalent(n))
            .find(|i| *i != H2Item::Dropped);
        match item {
            Some(item) => out.items.push(ItemSpawn {
                position: e.position,
                facing: e.facing,
                item,
                respawn: e.spawn_time,
            }),
            None => out
                .dropped
                .push(format!("{} at {:?}", names.join(" / "), e.position)),
        }
    }
    out
}

/// [`plan`] for a map: item collections resolved through the map's tags,
/// vehicles through the vehicle palette.
pub fn plan_map(m: &H1Map) -> Result<Plan> {
    plan_map_except(m, &std::collections::HashSet::new())
}

pub(crate) fn plan_map_except(
    m: &H1Map,
    converted: &std::collections::HashSet<h1map::TagId>,
) -> Result<Plan> {
    let mp = m.multiplayer()?;
    let mut names = Vec::with_capacity(mp.equipment.len());
    for e in &mp.equipment {
        if converted.contains(&e.item.id) {
            names.push(None);
            continue;
        }
        let ic = m.item_collection(e.item.id)?;
        let mut n = Vec::new();
        for p in ic.permutations {
            n.push(m.tag(p.item.id)?.name.clone());
        }
        names.push(Some(n));
    }
    let mut out = plan_selected(&mp, |i| names[i].clone());
    for (i, v) in mp.vehicles.iter().enumerate() {
        let Some(tag) = vehicle_tag(m, &mp, i, v)? else {
            continue;
        };
        match crate::sandbox::vehicle_equivalent(&tag.name) {
            Some(item) => out.items.push(ItemSpawn {
                position: v.position,
                facing: v.rotation[0],
                item,
                respawn: 0,
            }),
            None => out.source_vehicles.push(vehicle_spawn(m, tag, v)?),
        }
    }
    Ok(out)
}

pub(crate) fn vehicle_tag<'a>(
    map: &'a H1Map,
    mp: &Multiplayer,
    row: usize,
    vehicle: &h1map::scenario::ObjectPlacement,
) -> Result<Option<&'a h1map::Tag>> {
    let entry = placement_entry(
        &map.header().name,
        "vehicle",
        row,
        vehicle.palette,
        &mp.vehicle_palette,
    )?;
    let Some(r) = entry.filter(|r| !r.id.is_none()) else {
        return Ok(None);
    };
    let tag = map.tag(r.id).map_err(|e| {
        crate::Error::no_rebuild(&map.header().name, format!("vehicle placement {row}"), e)
    })?;
    if tag.group != h1map::Group::new(b"vehi") {
        return Err(crate::Error::no_rebuild(
            &map.header().name,
            &tag.name,
            "expected vehicle",
        ));
    }
    Ok(Some(tag))
}

pub(crate) fn spawn_points_and_flags(mp: &Multiplayer) -> Plan {
    plan_selected(mp, |_| None)
}

/// The base map's tag for a role: the first of `tags` (group, path) whose
/// group is an item collection (`itmc`) for weapons and equipment or a
/// vehicle collection (`vehc`) for vehicles, and whose name has the role's
/// words.
pub fn resolve(item: H2Item, tags: &[(String, String)]) -> Option<&str> {
    let (group, role) = match item {
        H2Item::Weapon(r) | H2Item::Equipment(r) => ("itmc", r),
        H2Item::Vehicle(r) => ("vehc", r),
        H2Item::Dropped => return None,
    };
    // An exact last component first (`warthog`, not `warthog_gauss`).
    let exact = tags.iter().find(|(g, name)| {
        g == group
            && name
                .rsplit(['\\', '/'])
                .next()
                .is_some_and(|l| l.eq_ignore_ascii_case(role))
    });
    exact
        .or_else(|| {
            tags.iter()
                .find(|(g, name)| g == group && h2_name_matches(name, role))
        })
        .map(|(_, name)| name.as_str())
}

/// Raises each vehicle spawn of `plan` by how far its vehicle reaches
/// below its origin (`depth` of the role, [`retail_vehicle_depth`] for
/// the base's): Halo 1 places a vehicle's origin on the floor, and the
/// retail ghost's hull reaches 0.15 under its origin, so at Halo 1's spot
/// it would start in the ground (as the converted ghost did,
/// `halo1::vehicle_depth`).
pub fn lift_vehicles(plan: &mut Plan, depth: impl Fn(&str) -> Option<f32>) {
    for item in &mut plan.items {
        if let H2Item::Vehicle(role) = item.item {
            item.position[2] += depth(role).unwrap_or(0.0).max(0.0);
        }
    }
}

/// How far the base's vehicle for `role` (the `vehi` whose last name
/// component is [`crate::sandbox::vehicle_class`]'s) reaches below its
/// origin: minus the lowest z of its render model's position bounds
/// (`obje +0x38` the `hlmt` datum, `hlmt +4` the `mode` datum, `mode
/// +0x14` the compression block, element `+0x10` z min; V:
/// `tags/src/defs/{obje,hlmt,mode}.txt`). `None` when any step is missing.
pub fn retail_vehicle_depth(ctx: &tags::object::Context, role: &str) -> Option<f32> {
    let leaf = crate::sandbox::vehicle_class(role)?.vehicle;
    let cat = &ctx.catalog;
    let u32_at = |address: u32, off: u32| -> Option<u32> {
        let b = ctx.meta.slice(address.checked_add(off)?, 4).ok()?;
        Some(u32::from_le_bytes(b.try_into().ok()?))
    };
    let vehi = cat.tags.values().find(|t| {
        t.group == map::FourCC(*b"vehi")
            && t.name
                .as_deref()
                .and_then(|n| n.rsplit(['\\', '/']).next())
                .is_some_and(|l| l.eq_ignore_ascii_case(leaf))
    })?;
    let hlmt = cat.get(map::TagId(u32_at(vehi.location?.0, 0x38)?))?;
    let mode = cat.get(map::TagId(u32_at(hlmt.location?.0, 4)?))?;
    let at = mode.location?.0;
    if u32_at(at, 0x14)? == 0 {
        return None;
    }
    let z_min = f32::from_bits(u32_at(u32_at(at, 0x18)?, 0x10)?);
    Some((-z_min).max(0.0))
}
