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
    /// Teleporter entrance; the team field is the channel.
    TeleporterSource,
    /// Teleporter exit.
    TeleporterDestination,
    /// A King of the Hill hill marker.
    Hill,
}

impl H2Flag {
    /// The stored `u16` (`scnr+0x118` element `+0x10`). The order is the
    /// community's enum for Halo 2's netgame flags; what the disc shows
    /// (`tests/h2_scenario.rs`, all 12 multiplayer maps) agrees with it:
    /// 0 and 1 come once per team on every map, 4 is always neutral (team
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
    /// What was left out and why, one line each.
    pub dropped: Vec<String>,
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
        let names = item_names(i);
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
    // Vehicles: placed as vehicle spawns, by their palette entry's name.
    out
}

/// [`plan`] for a map: item collections resolved through the map's tags,
/// vehicles through the vehicle palette.
pub fn plan_map(m: &H1Map) -> Result<Plan> {
    let mp = m.multiplayer()?;
    let mut names = Vec::with_capacity(mp.equipment.len());
    for e in &mp.equipment {
        let ic = m.item_collection(e.item.id)?;
        let mut n = Vec::new();
        for p in ic.permutations {
            n.push(m.tag(p.item.id)?.name.clone());
        }
        names.push(n);
    }
    let mut out = plan(&mp, |i| names[i].clone());
    for v in &mp.vehicles {
        let name = mp
            .vehicle_palette
            .get(v.palette as usize)
            .and_then(|r| m.tag(r.id).ok())
            .map(|t| t.name.clone())
            .unwrap_or_default();
        match h2_equivalent(&name) {
            Some(item @ H2Item::Vehicle(_)) => out.items.push(ItemSpawn {
                position: v.position,
                facing: v.rotation[0],
                item,
                respawn: 0,
            }),
            _ => out
                .dropped
                .push(format!("vehicle {name} at {:?}", v.position)),
        }
    }
    Ok(out)
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
    tags.iter()
        .find(|(g, name)| g == group && h2_name_matches(name, role))
        .map(|(_, name)| name.as_str())
}
