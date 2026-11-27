//! Whether a multiplayer map's placements start every mode it claims: read
//! from the template before the build ([`Census::of_template`]) and from
//! the built scenario after it ([`Census::read`]), with one rule each
//! ([`Census::missing`]).
//!
//! | Mode | Needs |
//! |---|---|
//! | Slayer, Juggernaut | two spawns or more |
//! | Capture the Flag | a spawn of team 0 and of team 1; a flag spawn (type 0) and a flag return (type 1) of each team |
//! | Assault | a spawn of each team; a bomb spawn (type 2) and a bomb return (type 3) of each team |
//! | Oddball | a ball spawn (type 4) |
//! | King of the Hill | one hill with three corners or more (types 11 to 18, one hill a type) |
//! | Territories | two territories (type 10) or more |
//!
//! Every multiplayer map also needs a weapon and a vehicle among its
//! netgame equipment ([`Census::items`]).

use std::collections::BTreeMap;

use scenario_edit::{BlockKind, Scenario};

use crate::template::{Mode, Multiplayer};
use crate::{Error, Result};

/// The placements a mode reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    /// Each starting location's team.
    pub spawns: Vec<i64>,
    /// Each netgame flag's type and team.
    pub flags: Vec<(u16, i64)>,
    /// Each netgame equipment spawner's item group.
    pub items: Vec<[u8; 4]>,
}

const WEAPONS: [[u8; 4]; 2] = [*b"weap", *b"itmc"];
const VEHICLES: [[u8; 4]; 2] = [*b"vehi", *b"vehc"];

impl Census {
    /// What the template places.
    pub fn of_template(mp: &Multiplayer) -> Result<Census> {
        let mut items = Vec::new();
        for it in &mp.items {
            items.push(it.key()?.0);
        }
        Ok(Census {
            spawns: mp.spawns.iter().map(|s| s.team.unwrap_or(0)).collect(),
            flags: mp
                .flags
                .iter()
                .map(|f| (f.flag_type(), f.team.unwrap_or(0)))
                .collect(),
            items,
        })
    }

    /// What a scenario holds.
    pub fn read(s: &Scenario) -> Result<Census> {
        let int = |k: BlockKind, i: usize, f: &str| -> Result<i64> {
            s.get(k, i, f)
                .ok()
                .and_then(|v| v.as_int())
                .ok_or_else(|| Error::Coverage(format!("{} {i} has no {f}", k.name())))
        };
        let mut c = Census::default();
        for i in 0..s.block(BlockKind::StartingLocations).len() {
            c.spawns.push(int(BlockKind::StartingLocations, i, "team")?);
        }
        for i in 0..s.block(BlockKind::NetgameFlags).len() {
            let ty = int(BlockKind::NetgameFlags, i, "type")?;
            let ty = u16::try_from(ty)
                .map_err(|_| Error::Coverage(format!("netgame flag {i}: type {ty}")))?;
            c.flags.push((ty, int(BlockKind::NetgameFlags, i, "team")?));
        }
        for i in 0..s.block(BlockKind::NetgameEquipment).len() {
            match s.get(BlockKind::NetgameEquipment, i, "item") {
                Ok(scenario_edit::Value::Tag(r)) if !r.is_none() => c.items.push(r.group.0),
                _ => {
                    return Err(Error::Coverage(format!(
                        "netgame equipment {i} names no item"
                    )));
                }
            }
        }
        Ok(c)
    }

    fn flag(&self, ty: u16, team: i64) -> bool {
        self.flags.iter().any(|&(t, m)| t == ty && m == team)
    }

    fn of_type(&self, ty: u16) -> usize {
        self.flags.iter().filter(|&&(t, _)| t == ty).count()
    }

    fn teams(&self) -> bool {
        [0, 1].iter().all(|t| self.spawns.contains(t))
    }

    /// Why `mode` would not start, if it would not.
    pub fn missing(&self, mode: Mode) -> Option<String> {
        let need = |ok: bool, why: &str| (!ok).then(|| why.to_owned());
        match mode {
            Mode::Slayer | Mode::Juggernaut => need(self.spawns.len() >= 2, "two spawns"),
            Mode::Ctf => need(
                self.teams() && [0, 1].iter().all(|&t| self.flag(0, t) && self.flag(1, t)),
                "a spawn, a flag spawn and a flag return for team 0 and team 1",
            ),
            Mode::Assault => need(
                self.teams() && [0, 1].iter().all(|&t| self.flag(2, t) && self.flag(3, t)),
                "a spawn, a bomb spawn and a bomb return for team 0 and team 1",
            ),
            Mode::Oddball => need(self.of_type(4) >= 1, "a ball spawn"),
            Mode::King => {
                let mut hills: BTreeMap<u16, usize> = BTreeMap::new();
                for &(t, _) in &self.flags {
                    if (11..=18).contains(&t) {
                        *hills.entry(t).or_default() += 1;
                    }
                }
                need(
                    hills.values().any(|&n| n >= 3),
                    "a hill of three corners",
                )
            }
            Mode::Territories => need(self.of_type(10) >= 2, "two territories"),
        }
    }

    /// Why the map has no weapon or no vehicle, if it has not.
    pub fn items(&self) -> Option<String> {
        let weapon = self.items.iter().any(|g| WEAPONS.contains(g));
        let vehicle = self.items.iter().any(|g| VEHICLES.contains(g));
        match (weapon, vehicle) {
            (true, true) => None,
            (false, true) => Some("no weapon spawner".into()),
            (true, false) => Some("no vehicle spawner".into()),
            (false, false) => Some("no weapon or vehicle spawner".into()),
        }
    }

    /// Every mode of `modes` starts and the map has a weapon and a
    /// vehicle; else one sentence naming each gap.
    pub fn covers(&self, modes: &[Mode]) -> std::result::Result<(), String> {
        let mut gaps: Vec<String> = modes
            .iter()
            .filter_map(|&m| self.missing(m).map(|why| format!("{} needs {why}", m.name())))
            .collect();
        gaps.extend(self.items());
        if gaps.is_empty() {
            Ok(())
        } else {
            Err(gaps.join("; "))
        }
    }
}
