//! Authored layouts for the built-in kit maps (docs/game/firefight.md,
//! "Built-in layouts"). No map data is in the repo: a layout names places
//! by the map's own data (its netgame flags, starting locations and weapon
//! pads) and the builder resolves each on the player's own disc, adds the
//! offset and snaps to the nearest usable sector (`nav::choose_zones`). A
//! drop that resolves nowhere, has no path to both bases, or sits inside
//! the kit's base exclusion is refused by name; the kit never quietly
//! picks another. [`ensure`] builds each shipped kit's package on the
//! player's machine when it is missing or stale.
//!
//! The positions a layout cannot know until a census of the disc map has
//! been read are named constants in its file, each marked `V`;
//! `h2 firefight kit MAP --layout-report` prints what settles them.
use std::fmt::{self, Write as _};
use std::io::{Read, Seek};

use map::{Map, TagId};
use pathfind::geom::V3;
use scenario_edit::scene::metadata_block;

use crate::bases::{FlagRow, StartRow, Team, distance2};

pub mod coagulation;
mod ensure;
mod report;
pub mod zanzibar;

pub use ensure::{
    DiscMaps, MAPS, Outcome, REFUSED, Report, Target, ensure, ensure_with, pending, pending_with,
    refusal, refusal_line, targets,
};
pub use report::layout_report;

/// The kit maps that have an authored layout.
pub static ALL: [&Layout; 2] = [&coagulation::LAYOUT, &zanzibar::LAYOUT];

/// The authored layout of the map named `name`, if it has one.
pub fn for_map(name: &str) -> Option<&'static Layout> {
    ALL.iter()
        .copied()
        .find(|l| l.map.eq_ignore_ascii_case(name))
}

/// The kinds of squad a drop zone takes (`firefight::roles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles(pub u8);

impl Roles {
    pub const LINE: Roles = Roles(firefight::roles::LINE);
    pub const FLANK: Roles = Roles(firefight::roles::FLANK);
    pub const HEAVY: Roles = Roles(firefight::roles::HEAVY);
    pub const ALL: Roles = Roles(firefight::roles::ALL);

    pub const fn with(self, other: Roles) -> Roles {
        Roles(self.0 | other.0)
    }

    pub fn has(self, role: Roles) -> bool {
        self.0 & role.0 == role.0
    }
}

/// Which team's hunt side a drop is on: the drops each side's players
/// meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    Both,
    Only(Team),
}

impl Sides {
    pub fn serves(self, team: Team) -> bool {
        match self {
            Sides::Both => true,
            Sides::Only(t) => t == team,
        }
    }
}

/// The netgame flag types a layout names (`scnr+0x118`, type at `+0x10`;
/// `forge_doc::labels::builtin_label`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagKind {
    CtfStand,
    OddballSpawn,
    TeleporterSource,
    TeleporterExit,
    Territory,
    /// Any of the eight hill corners (types 11 to 18).
    Hill,
}

impl FlagKind {
    pub fn matches(self, flag_type: u16) -> bool {
        match self {
            FlagKind::CtfStand => flag_type == 0,
            FlagKind::OddballSpawn => flag_type == 4,
            FlagKind::TeleporterSource => flag_type == 7,
            FlagKind::TeleporterExit => flag_type == 8,
            FlagKind::Territory => flag_type == 10,
            FlagKind::Hill => (11..=18).contains(&flag_type),
        }
    }

    fn name(self) -> &'static str {
        match self {
            FlagKind::CtfStand => "ctf",
            FlagKind::OddballSpawn => "oddball",
            FlagKind::TeleporterSource => "teleporter_source",
            FlagKind::TeleporterExit => "teleporter_exit",
            FlagKind::Territory => "territory",
            FlagKind::Hill => "hill",
        }
    }
}

/// A place named by the map's own data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// The `nth` netgame flag of `kind` (of `team`, when given), in row
    /// order.
    Flag {
        kind: FlagKind,
        team: Option<Team>,
        nth: usize,
    },
    /// The mean of a team's starting locations (the scenario's own team
    /// designators, before the kit rewrites them).
    Starts { team: Team },
    /// The `nth` weapon placement whose tag name ends with `suffix`.
    Item { suffix: &'static str, nth: usize },
    /// `a` plus `t` of the way to `b`.
    Between(&'static Anchor, &'static Anchor, f32),
    /// The first that resolves.
    Or(&'static Anchor, &'static Anchor),
}

/// A weapon placement of the map: its tag name and place.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub name: String,
    pub position: V3,
}

/// What a map has that an anchor can name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Census {
    pub flags: Vec<FlagRow>,
    pub starts: Vec<StartRow>,
    pub items: Vec<Item>,
}

/// The scenario's weapon placements (`scnr+0x90`, `0x54` bytes each, the
/// palette index at `+0` and the position at `+8`) and their palette
/// (`scnr+0x98`, `0x28` bytes, the tag at `+4`): the same offsets
/// `tags::object::OBJECT_TYPES` gives the weapon type.
const WEAPONS: usize = 0x90;
const WEAPON: usize = 0x54;
const WEAPON_PALETTE: usize = 0x98;
const PALETTE: usize = 0x28;
/// The netgame equipment block (`scnr+0x120`, `0x90` bytes each): what a
/// multiplayer map spawns at its pads. The position is at `+0x40`, the item
/// (a weapon or an item collection) as group then tag id at `+0x58`. Retail
/// multiplayer maps place their weapons here and have no weapon placements.
const NETGAME_EQUIPMENT: usize = 0x120;
const NETGAME_EQUIPMENT_ROW: usize = 0x90;

impl Census {
    /// The census of a map whose scenario root is `scnr`: the flags and
    /// starts the caller already read, and the weapon placements named by
    /// their tag. A weapon block that cannot be read is empty.
    pub(crate) fn read<R: Read + Seek>(
        map: &mut Map<R>,
        scnr: &[u8],
        starts: Vec<StartRow>,
        flags: Vec<FlagRow>,
    ) -> Census {
        let f = |row: &[u8], at: usize| f32::from_le_bytes(row[at..at + 4].try_into().unwrap());
        let palette: Vec<TagId> = metadata_block(map, scnr, WEAPON_PALETTE, PALETTE)
            .unwrap_or_default()
            .iter()
            .map(|row| TagId(u32::from_le_bytes(row[4..8].try_into().unwrap())))
            .collect();
        let mut items = Vec::new();
        for row in metadata_block(map, scnr, WEAPONS, WEAPON).unwrap_or_default() {
            let index = i16::from_le_bytes([row[0], row[1]]);
            let Some(&id) = usize::try_from(index).ok().and_then(|i| palette.get(i)) else {
                continue;
            };
            let Some(name) = map.tag(id).ok().and_then(|t| t.name.clone()) else {
                continue;
            };
            items.push(Item {
                name,
                position: [f(&row, 8), f(&row, 12), f(&row, 16)],
            });
        }
        for row in metadata_block(map, scnr, NETGAME_EQUIPMENT, NETGAME_EQUIPMENT_ROW)
            .unwrap_or_default()
        {
            let id = TagId(u32::from_le_bytes(row[0x5c..0x60].try_into().unwrap()));
            if id == TagId::NULL {
                continue;
            }
            let Some(name) = map.tag(id).ok().and_then(|t| t.name.clone()) else {
                continue;
            };
            items.push(Item {
                name,
                position: [f(&row, 0x40), f(&row, 0x44), f(&row, 0x48)],
            });
        }
        Census {
            flags,
            starts,
            items,
        }
    }

    /// Every flag, start cluster and pad, one line each, for the census.
    pub(crate) fn describe(&self) -> String {
        let mut out = String::new();
        for (i, f) in self.flags.iter().enumerate() {
            let kind = [
                FlagKind::CtfStand,
                FlagKind::OddballSpawn,
                FlagKind::TeleporterSource,
                FlagKind::TeleporterExit,
                FlagKind::Territory,
                FlagKind::Hill,
            ]
            .into_iter()
            .find(|k| k.matches(f.kind))
            .map_or("other", FlagKind::name);
            let _ = writeln!(
                out,
                "flag {i}: type {} ({kind}) team {} at {}",
                f.kind,
                f.team as i16,
                point(f.position)
            );
        }
        for team in [Team::Red, Team::Blue] {
            let rows: Vec<&StartRow> = self
                .starts
                .iter()
                .filter(|s| s.team == team.index() as i16)
                .collect();
            if let Some(mean) = mean(rows.iter().map(|s| s.position)) {
                let _ = writeln!(
                    out,
                    "starts team {}: {} at mean {}",
                    team.index(),
                    rows.len(),
                    point(mean)
                );
            }
        }
        let other = self
            .starts
            .iter()
            .filter(|s| !(0..2).contains(&s.team))
            .count();
        if other > 0 {
            let _ = writeln!(out, "starts of no team 0 or 1: {other}");
        }
        for (i, item) in self.items.iter().enumerate() {
            let _ = writeln!(out, "pad {i}: {} at {}", item.name, point(item.position));
        }
        out
    }
}

pub(crate) fn point(p: V3) -> String {
    format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2])
}

fn mean(points: impl Iterator<Item = V3>) -> Option<V3> {
    let (mut sum, mut n) = ([0f64; 3], 0usize);
    for p in points {
        n += 1;
        for k in 0..3 {
            sum[k] += f64::from(p[k]);
        }
    }
    (n > 0).then(|| std::array::from_fn(|k| (sum[k] / n as f64) as f32))
}

impl Anchor {
    /// The place this names on `census`, or why the map has none.
    pub(crate) fn resolve(&self, census: &Census) -> Result<V3, String> {
        match *self {
            Anchor::Flag { kind, team, nth } => census
                .flags
                .iter()
                .filter(|f| kind.matches(f.kind))
                .filter(|f| team.is_none_or(|t| f.team == t.index() as u16))
                .nth(nth)
                .map(|f| f.position)
                .ok_or_else(|| format!("the map has no {self}")),
            Anchor::Starts { team } => mean(
                census
                    .starts
                    .iter()
                    .filter(|s| s.team == team.index() as i16)
                    .map(|s| s.position),
            )
            .ok_or_else(|| format!("the map has no {self}")),
            Anchor::Item { suffix, nth } => census
                .items
                .iter()
                .filter(|i| i.name.ends_with(suffix))
                .nth(nth)
                .map(|i| i.position)
                .ok_or_else(|| format!("the map has no {self}")),
            Anchor::Between(a, b, t) => {
                let (a, b) = (a.resolve(census)?, b.resolve(census)?);
                Ok(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t))
            }
            Anchor::Or(first, second) => first
                .resolve(census)
                .or_else(|first| second.resolve(census).map_err(|s| format!("{first}; {s}"))),
        }
    }
}

impl fmt::Display for Anchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Anchor::Flag { kind, team, nth } => {
                write!(f, "{} flag #{nth}", kind.name())?;
                if let Some(t) = team {
                    write!(f, " of team {}", t.index())?;
                }
                Ok(())
            }
            Anchor::Starts { team } => write!(f, "starting locations of team {}", team.index()),
            Anchor::Item { suffix, nth } => write!(f, "weapon #{nth} named '*{suffix}'"),
            Anchor::Between(a, b, t) => write!(f, "{t} of the way from ({a}) to ({b})"),
            Anchor::Or(a, b) => write!(f, "({a}) or ({b})"),
        }
    }
}

/// The home base: its anchor, replacing the point the kit finds from the
/// map's flag stands and starts, and how many firing positions its zone
/// keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaseSpec {
    pub anchor: Anchor,
    pub positions: usize,
}

/// A drop zone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropSpec {
    /// The name refusals use.
    pub name: &'static str,
    pub anchor: Anchor,
    /// Added to the anchor's point.
    pub offset: [f32; 3],
    /// The zone snaps to the nearest usable sector within this many units.
    pub radius: f32,
    pub roles: Roles,
    pub sides: Sides,
}

/// A firing position the author wants taken first (`ff_cover`'s
/// counterpart).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverSpec {
    pub anchor: Anchor,
    pub offset: [f32; 3],
}

/// The authored kit of one map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub map: &'static str,
    /// The map team whose base is the home base.
    pub home: Team,
    /// Two teams each hold a base (Crossfire is offered), or one.
    pub two_teams: bool,
    pub base: BaseSpec,
    pub drops: &'static [DropSpec],
    pub cover: &'static [CoverSpec],
}

// Offsets and radii are floats, but never not-a-number.
impl Eq for Layout {}

impl Layout {
    /// The teams whose hunt side the layout serves.
    pub fn served(&self) -> &'static [Team] {
        if self.two_teams {
            &[Team::Red, Team::Blue]
        } else {
            match self.home {
                Team::Red => &[Team::Red],
                Team::Blue => &[Team::Blue],
            }
        }
    }

    /// A short digest of everything the layout says: the package label
    /// carries it, so the package id changes with the layout.
    pub fn digest(&self) -> String {
        let sha = content::Sha256::of(format!("{self:?}").as_bytes());
        sha.to_string().chars().take(12).collect()
    }

    /// Each anchor on `census`, with what could not be resolved named.
    pub(crate) fn resolve(&self, census: &Census) -> Resolved {
        let mut out = Resolved {
            base_point: None,
            base_positions: self.base.positions,
            drops: Vec::new(),
            covers: Vec::new(),
            refused: Vec::new(),
        };
        match self.base.anchor.resolve(census) {
            Ok(p) => out.base_point = Some(p),
            Err(why) => out.refused.push(format!("home base: {why}")),
        }
        for d in self.drops {
            match d.anchor.resolve(census) {
                Ok(p) => out.drops.push(ResolvedDrop {
                    name: d.name,
                    anchor: p,
                    point: std::array::from_fn(|k| p[k] + d.offset[k]),
                    radius: d.radius,
                    roles: d.roles.0,
                }),
                Err(why) => out.refused.push(format!("drop '{}': {why}", d.name)),
            }
        }
        for c in self.cover {
            if let Ok(p) = c.anchor.resolve(census) {
                out.covers.push(std::array::from_fn(|k| p[k] + c.offset[k]));
            }
        }
        out
    }
}

/// A layout resolved on one map.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolved {
    /// The home base's point, when its anchor resolved.
    pub base_point: Option<V3>,
    pub base_positions: usize,
    pub drops: Vec<ResolvedDrop>,
    pub covers: Vec<V3>,
    /// What did not resolve, named.
    pub refused: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedDrop {
    pub name: &'static str,
    /// The anchor's own point.
    pub anchor: V3,
    /// The anchor plus the offset: where the zone is wanted.
    pub point: V3,
    pub radius: f32,
    pub roles: u8,
}

/// Where a resolved drop snapped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snap {
    pub name: &'static str,
    pub anchor: V3,
    pub wanted: V3,
    pub sector: usize,
    pub centre: V3,
    pub radius: f32,
}

/// The distance between two points, for the report.
pub(crate) fn flat(a: V3, b: V3) -> f32 {
    distance2(a, b).sqrt()
}

/// The distance from `p` to each base, for the report.
pub(crate) fn from_bases(bases: &[V3], p: V3) -> Vec<f32> {
    bases.iter().map(|&b| distance2(p, b).sqrt()).collect()
}

#[cfg(test)]
mod tests;
