//! The template (`content/new-map/*.yaml`): the ground as a one-bay stage
//! layout, the block-out pieces, and per kind the base, the spawns and
//! what each mode needs. [`Template::parse`] checks everything that needs
//! no map.

use std::collections::BTreeSet;

use forge_shapes::{PieceDef, Shape};
use map::scenario_layout::ITEM_GROUPS;
use serde::{Deserialize, Serialize};
use content_kit::stage::{Layout, Player};

use crate::coverage::Census;
use crate::{Error, Result};

/// What kind of map New map makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Campaign,
    Multiplayer,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Campaign, Kind::Multiplayer];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Campaign => "campaign",
            Kind::Multiplayer => "multiplayer",
        }
    }

    /// `campaign` or `multiplayer`, any case.
    pub fn parse(s: &str) -> Result<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| Error::Template(format!("kind {s:?}: campaign or multiplayer")))
    }
}

/// A retail game type a multiplayer map claims to start, by the names
/// `game.load`'s `variant` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Slayer,
    Ctf,
    Assault,
    Oddball,
    King,
    Juggernaut,
    Territories,
}

impl Mode {
    pub const ALL: [Mode; 7] = [
        Mode::Slayer,
        Mode::Ctf,
        Mode::Assault,
        Mode::Oddball,
        Mode::King,
        Mode::Juggernaut,
        Mode::Territories,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Slayer => "slayer",
            Mode::Ctf => "ctf",
            Mode::Assault => "assault",
            Mode::Oddball => "oddball",
            Mode::King => "king",
            Mode::Juggernaut => "juggernaut",
            Mode::Territories => "territories",
        }
    }
}

/// A netgame flag, by the built-in label of its type
/// (`forge_doc::labels::builtin_label`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    CtfFlagSpawn,
    CtfFlagReturn,
    AssaultBombSpawn,
    AssaultBombReturn,
    OddballSpawn,
    Territory,
    /// A corner of hill `hill` (0 to 7): three or more make the hill.
    HillCorner,
}

impl FlagKind {
    pub const ALL: [FlagKind; 7] = [
        FlagKind::CtfFlagSpawn,
        FlagKind::CtfFlagReturn,
        FlagKind::AssaultBombSpawn,
        FlagKind::AssaultBombReturn,
        FlagKind::OddballSpawn,
        FlagKind::Territory,
        FlagKind::HillCorner,
    ];

    /// The netgame flag's `type` (`scnr +0x118`, `+0x10`); a hill corner's
    /// is 11 plus its hill.
    pub fn flag_type(self, hill: u8) -> u16 {
        match self {
            FlagKind::CtfFlagSpawn => 0,
            FlagKind::CtfFlagReturn => 1,
            FlagKind::AssaultBombSpawn => 2,
            FlagKind::AssaultBombReturn => 3,
            FlagKind::OddballSpawn => 4,
            FlagKind::Territory => 10,
            FlagKind::HillCorner => 11 + u16::from(hill),
        }
    }

    /// The label text the type reads as.
    pub fn label(self) -> &'static str {
        match self {
            FlagKind::CtfFlagSpawn => "ctf_flag_spawn",
            FlagKind::CtfFlagReturn => "ctf_flag_return",
            FlagKind::AssaultBombSpawn => "assault_bomb_spawn",
            FlagKind::AssaultBombReturn => "assault_bomb_return",
            FlagKind::OddballSpawn => "oddball_spawn",
            FlagKind::Territory => "territory",
            FlagKind::HillCorner => "hill_corner",
        }
    }
}

/// A block-out piece, placed as a crate with its lowest corner at `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Piece {
    pub name: String,
    pub shape: Shape,
    /// Block-out units.
    pub size: [u16; 3],
    #[serde(default)]
    pub param: u16,
    /// World units.
    pub at: [f32; 3],
    /// Degrees.
    #[serde(default)]
    pub facing: f32,
}

impl Piece {
    /// The definition: every face drawn with the dev textures.
    pub fn def(&self) -> PieceDef {
        PieceDef::dev(self.shape, self.size, self.param)
    }
}

/// A player starting location.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub at: [f32; 3],
    #[serde(default)]
    pub facing: f32,
    #[serde(default)]
    pub team: Option<i64>,
}

/// A netgame flag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub kind: FlagKind,
    pub at: [f32; 3],
    #[serde(default)]
    pub facing: f32,
    #[serde(default)]
    pub team: Option<i64>,
    /// A hill corner's hill, 0 to 7.
    #[serde(default)]
    pub hill: Option<u8>,
}

impl Flag {
    pub fn flag_type(&self) -> u16 {
        self.kind.flag_type(self.hill.unwrap_or(0))
    }
}

/// A netgame equipment spawner: `tag` is `<group>:<path>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub tag: String,
    pub at: [f32; 3],
    #[serde(default)]
    pub facing: f32,
}

impl Item {
    /// The group and path `tag` names.
    pub fn key(&self) -> Result<([u8; 4], &str)> {
        let bad = || Error::Template(format!("item {:?}: <group>:<path>", self.tag));
        let (group, path) = self.tag.split_once(':').ok_or_else(bad)?;
        let group: [u8; 4] = group.as_bytes().try_into().map_err(|_| bad())?;
        if path.is_empty() || path.contains('/') {
            return Err(bad());
        }
        if !ITEM_GROUPS.contains(&group) {
            return Err(Error::Template(format!(
                "item {:?}: netgame equipment does not place a {}",
                self.tag,
                String::from_utf8_lossy(&group)
            )));
        }
        Ok((group, path))
    }
}

/// A multiplayer map: 16 spawns over two teams, the flags of every mode
/// it claims, a weapon and a vehicle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Multiplayer {
    pub base: String,
    pub biped: String,
    pub scenario: String,
    pub modes: Vec<Mode>,
    pub spawns: Vec<Spawn>,
    pub flags: Vec<Flag>,
    pub items: Vec<Item>,
}

/// A root block of the base's scenario to empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clear {
    /// The `scnr` root offset, as hex (`0x160`).
    pub at: String,
    pub name: String,
}

impl Clear {
    pub fn offset(&self) -> Result<u32> {
        let bad = || Error::Template(format!("clear {}: at {:?} is not a hex offset", self.name, self.at));
        let hex = self.at.strip_prefix("0x").ok_or_else(bad)?;
        let at = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
        // The scenario root is 0x3e0 bytes; a block field is 8 of them.
        if at % 4 != 0 || at + 8 > 0x3e0 {
            return Err(bad());
        }
        Ok(at)
    }
}

/// The campaign map's one squad.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Squad {
    pub name: String,
    /// Actors on every difficulty.
    pub count: i16,
    pub at: [f32; 3],
    #[serde(default)]
    pub facing: f32,
}

/// The campaign map's one zone: its firing positions, one area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    pub name: String,
    pub firing_positions: Vec<[f32; 3]>,
}

/// The campaign map's AI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ai {
    /// The squad's `char` tag, held by the base.
    pub character: String,
    /// Halo 2's AI team.
    pub team: u16,
    pub squad: Squad,
    pub zone: Zone,
}

/// A campaign map: the player's start and a squad that can be placed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Campaign {
    pub base: String,
    pub biped: String,
    pub scenario: String,
    #[serde(default)]
    pub clear: Vec<Clear>,
    pub start: Spawn,
    pub ai: Ai,
}

/// A template as its YAML reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub name: String,
    pub title: String,
    pub description: String,
    /// A stage layout of one bay (`content_kit::stage::layout`).
    pub ground: serde_yaml_ng::Value,
    pub pieces: Vec<Piece>,
    pub multiplayer: Multiplayer,
    pub campaign: Campaign,
}

/// What both kinds share: the base, the biped and the scenario path.
#[derive(Clone, Copy, Debug)]
pub struct Common<'a> {
    pub base: &'a str,
    pub biped: &'a str,
    pub scenario: &'a str,
}

impl Common<'_> {
    /// The scenario path of a map called `name`.
    pub fn scenario_path(&self, name: &str) -> String {
        self.scenario.replace("{name}", name)
    }
}

/// A stand-in player: the layout's checks do not depend on the size.
const STAND_IN: Player = Player {
    height: 0.7,
    radius: 0.2,
};

fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 31
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn finite(what: &str, v: &[f32]) -> Result<()> {
    if v.iter().all(|x| x.is_finite() && x.abs() < 1.0e4) {
        Ok(())
    } else {
        Err(Error::Template(format!("{what}: not a finite point")))
    }
}

fn tag_path(what: &str, s: &str) -> Result<()> {
    if s.is_empty() || s.contains('/') || s.len() >= 256 {
        return Err(Error::Template(format!("{what} {s:?} is not a tag path")));
    }
    Ok(())
}

/// A 31-character inline name.
fn name31(what: &str, s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 31 || !s.is_ascii() {
        return Err(Error::Template(format!("{what} {s:?}: 1 to 31 ASCII characters")));
    }
    Ok(())
}

/// The map name a title gives: lower case, a space or a dot an
/// underscore, letters, digits, `_` and `-` only, 1 to 31 of them.
pub fn map_name(title: &str) -> Result<String> {
    let name: String = title
        .trim()
        .chars()
        .map(|c| match c {
            ' ' | '.' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    if !plain(&name) || !name.as_bytes()[0].is_ascii_alphabetic() {
        return Err(Error::Name(format!(
            "{title:?}: start with a letter; letters, digits, spaces, '_' and '-', at most 31"
        )));
    }
    Ok(name)
}

impl Template {
    /// Parses and checks a template (module docs).
    pub fn parse(yaml: &str) -> Result<Template> {
        let t: Template =
            serde_yaml_ng::from_str(yaml).map_err(|e| Error::Template(e.to_string()))?;
        t.check()?;
        Ok(t)
    }

    /// The template shipped with the repository ([`crate::SOURCE`]).
    pub fn builtin() -> Result<Template> {
        Self::parse(crate::SOURCE)
    }

    /// The ground as the YAML `content_kit::stage::Layout::parse` takes.
    pub fn ground_yaml(&self) -> Result<String> {
        serde_yaml_ng::to_string(&self.ground).map_err(|e| Error::Template(format!("ground: {e}")))
    }

    /// The ground resolved against `player`.
    pub fn layout(&self, player: Player) -> Result<Layout> {
        let layout = Layout::parse(&self.ground_yaml()?, player)?;
        if layout.bays.len() != 1 {
            return Err(Error::Template(format!(
                "ground: {} bays; a new map's floor is one",
                layout.bays.len()
            )));
        }
        Ok(layout)
    }

    pub fn common(&self, kind: Kind) -> Common<'_> {
        match kind {
            Kind::Multiplayer => Common {
                base: &self.multiplayer.base,
                biped: &self.multiplayer.biped,
                scenario: &self.multiplayer.scenario,
            },
            Kind::Campaign => Common {
                base: &self.campaign.base,
                biped: &self.campaign.biped,
                scenario: &self.campaign.scenario,
            },
        }
    }

    /// The pieces' definitions, once each, in name order.
    pub fn defs(&self) -> Vec<PieceDef> {
        let mut by_name = std::collections::BTreeMap::new();
        for p in &self.pieces {
            let d = p.def();
            by_name.entry(d.name()).or_insert(d);
        }
        by_name.into_values().collect()
    }

    fn check(&self) -> Result<()> {
        if !plain(&self.name) {
            return Err(Error::Template(format!("name {:?} is not a plain name", self.name)));
        }
        if self.title.trim().is_empty() {
            return Err(Error::Template("title is empty".into()));
        }
        self.layout(STAND_IN)?;
        if self.pieces.is_empty() {
            return Err(Error::Template("no pieces".into()));
        }
        let mut names = BTreeSet::new();
        for p in &self.pieces {
            if !names.insert(p.name.as_str()) {
                return Err(Error::Template(format!("two pieces are named {:?}", p.name)));
            }
            p.def()
                .validate()
                .map_err(|e| Error::Template(format!("piece {}: {e}", p.name)))?;
            finite(&format!("piece {}", p.name), &p.at)?;
            finite(&format!("piece {}: facing", p.name), &[p.facing])?;
        }
        for kind in Kind::ALL {
            let c = self.common(kind);
            if !plain(c.base) {
                return Err(Error::Template(format!(
                    "{}: base {:?} is not a map name",
                    kind.name(),
                    c.base
                )));
            }
            tag_path(&format!("{}: biped", kind.name()), c.biped)?;
            if !c.scenario.contains("{name}") {
                return Err(Error::Template(format!(
                    "{}: scenario {:?} does not hold {{name}}",
                    kind.name(),
                    c.scenario
                )));
            }
            tag_path(&format!("{}: scenario", kind.name()), &c.scenario_path("x"))?;
        }
        self.check_multiplayer()?;
        self.check_campaign()
    }

    fn check_multiplayer(&self) -> Result<()> {
        let mp = &self.multiplayer;
        if mp.spawns.is_empty() {
            return Err(Error::Template("multiplayer: no spawns".into()));
        }
        for (i, s) in mp.spawns.iter().enumerate() {
            finite(&format!("multiplayer spawn {i}"), &s.at)?;
        }
        for (i, f) in mp.flags.iter().enumerate() {
            finite(&format!("multiplayer flag {i}"), &f.at)?;
            match (f.kind, f.hill) {
                (FlagKind::HillCorner, Some(h)) if h <= 7 => {}
                (FlagKind::HillCorner, _) => {
                    return Err(Error::Template(format!(
                        "multiplayer flag {i}: a hill corner names its hill, 0 to 7"
                    )));
                }
                (_, Some(_)) => {
                    return Err(Error::Template(format!(
                        "multiplayer flag {i}: only a hill corner has a hill"
                    )));
                }
                _ => {}
            }
        }
        for (i, it) in mp.items.iter().enumerate() {
            finite(&format!("multiplayer item {i}"), &it.at)?;
            it.key()?;
        }
        if mp.modes.is_empty() {
            return Err(Error::Template("multiplayer: no modes".into()));
        }
        let modes: BTreeSet<Mode> = mp.modes.iter().copied().collect();
        if modes.len() != mp.modes.len() {
            return Err(Error::Template("multiplayer: a mode is named twice".into()));
        }
        Census::of_template(mp)?.covers(&mp.modes).map_err(Error::Template)
    }

    fn check_campaign(&self) -> Result<()> {
        let c = &self.campaign;
        let mut seen = BTreeSet::new();
        for clear in &c.clear {
            let at = clear.offset()?;
            if !seen.insert(at) {
                return Err(Error::Template(format!("campaign: {} is cleared twice", clear.at)));
            }
        }
        finite("campaign start", &c.start.at)?;
        tag_path("campaign: ai.character", &c.ai.character)?;
        name31("campaign: the squad's name", &c.ai.squad.name)?;
        name31("campaign: the zone's name", &c.ai.zone.name)?;
        if !(1..=16).contains(&c.ai.squad.count) {
            return Err(Error::Template(format!(
                "campaign: a squad of {}; 1 to 16",
                c.ai.squad.count
            )));
        }
        finite("campaign squad", &c.ai.squad.at)?;
        let n = c.ai.zone.firing_positions.len();
        if !(1..=32).contains(&n) {
            return Err(Error::Template(format!(
                "campaign: {n} firing positions; 1 to 32"
            )));
        }
        for (i, p) in c.ai.zone.firing_positions.iter().enumerate() {
            finite(&format!("campaign firing position {i}"), p)?;
        }
        Ok(())
    }
}
