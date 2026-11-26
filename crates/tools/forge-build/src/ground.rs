//! AI ground for a document that asks for it
//! (docs/game/game-types-without-carriers.md, "Maps for scripted games").
//!
//! The materialiser reads the document's ground marks from the finished
//! scenario (a flag's position, facing and team) and hands them, with the
//! recipe, to `firefight_kit::ground::build_with` over the materialised
//! map. The result is the map: nothing of it is in the document.
//!
//! **Classes.** The recipe's enemy classes (`GroundRecipe::classes`) are
//! copied from their donors, the campaign maps the recipe names by name and
//! hash. Each donor is read from the machine's own maps and checked against
//! the recipe's hash before anything is built, and refused by name when it
//! is missing or another ([`crate::Error::MissingMap`],
//! [`crate::Error::SourceHash`]). The build cuts classes from the end of the
//! list when the map is short of headroom (`Classes::cut_to_fit`), so the
//! same inputs cut alike on every machine.
use firefight_kit::{
    ClassCost, Costs, Mode, Team,
    ground::{self, BaseMarker, Labels, LineMarker, Marker, Problem, Receipt},
};
use forge_doc::{BlockKind, GroundLabel, GroundMark, GroundRecipe};
use scenario_edit::Scenario;
use tags::schema::Schema;

use crate::Error;

/// What the ground build made, besides the map: how many zones and squad
/// slots it has, the labels it could not honour, the classes it kept and
/// cut, what it cost and the receipt of its inputs.
#[derive(Clone, Debug)]
pub struct GroundReport {
    pub zones: usize,
    pub squads: usize,
    pub problems: Vec<Problem>,
    /// The classes kept past the Elite, in palette order, with what each
    /// added.
    pub classes: Vec<ClassCost>,
    /// The classes cut to fit the headroom, first cut first.
    pub cut: Vec<String>,
    pub costs: Costs,
    pub receipt: Receipt,
    /// The built map's sectors, each its polygon in ring order
    /// ([`sectors_of`]).
    pub sectors: Vec<Vec<[f32; 3]>>,
}

/// What a recipe's classes are copied by: the layouts of the tags and each
/// class's `char` tag on its donor. [`ClassTable::retail`] is the game's;
/// a test hands its invented donor's.
#[derive(Clone, Debug)]
pub struct ClassTable<'a> {
    pub schema: &'a Schema,
    /// `(class name, char path)`.
    pub paths: Vec<(String, String)>,
}

impl ClassTable<'static> {
    /// The checked-in schema and `firefight_kit::retail::CLASSES`.
    pub fn retail() -> Self {
        Self {
            schema: Schema::builtin(),
            paths: firefight_kit::retail::CLASSES
                .iter()
                .map(|c| (c.name.to_owned(), c.path.to_owned()))
                .collect(),
        }
    }
}

impl ClassTable<'_> {
    /// The `char` path of class `name`.
    pub(crate) fn path(&self, name: &str) -> Result<&str, Error> {
        self.paths
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.as_str())
            .ok_or_else(|| Error::Ground(format!("no class table entry for {name}")))
    }
}

/// The sectors of the built map `bytes`, each its polygon in ring order:
/// what the AI walks, for the AI tool's overlay. A structure BSP whose
/// pathfinding does not read gives none.
pub(crate) fn sectors_of(bytes: &[u8]) -> Result<Vec<Vec<[f32; 3]>>, Error> {
    let mut map = map::Map::from_reader(std::io::Cursor::new(bytes))?;
    let level = scenario_edit::nav_view::LevelPathfinding::read(&mut map)?;
    Ok(level.sectors.into_iter().map(|s| s.ring).collect())
}

fn team(label: GroundLabel, index: u32, value: i64) -> Result<Team, Error> {
    match value {
        0 => Ok(Team::Red),
        1 => Ok(Team::Blue),
        other => Err(Error::Ground(format!(
            "{} on netgame flag {index}: team {other} is not red (0) or blue (1)",
            label.text()
        ))),
    }
}

fn read(scenario: &Scenario, index: u32, field: &str) -> Result<scenario_edit::Value, Error> {
    scenario
        .get(BlockKind::NetgameFlags, index as usize, field)
        .map_err(|e| Error::Ground(format!("netgame flag {index}: {e}")))
}

fn position(scenario: &Scenario, index: u32) -> Result<[f32; 3], Error> {
    read(scenario, index, "position")?
        .as_vec3()
        .ok_or_else(|| Error::Ground(format!("netgame flag {index} has no position")))
}

fn facing(scenario: &Scenario, index: u32) -> Result<f32, Error> {
    read(scenario, index, "facing")?
        .as_f32()
        .ok_or_else(|| Error::Ground(format!("netgame flag {index} has no facing")))
}

fn team_of(scenario: &Scenario, mark: &GroundMark) -> Result<Team, Error> {
    let value = read(scenario, mark.index, "team")?
        .as_int()
        .ok_or_else(|| Error::Ground(format!("netgame flag {} has no team", mark.index)))?;
    team(mark.kind, mark.index, value)
}

/// The labels the ground build reads, from the marks on the finished
/// scenario's flags.
pub(crate) fn labels(scenario: &Scenario, marks: &[GroundMark]) -> Result<Labels, Error> {
    let mut out = Labels::default();
    // A barrier is a mark on an object, which the script reads by its
    // label: the ground build has no use for it.
    for mark in marks.iter().filter(|m| m.kind.on_flag()) {
        let at = position(scenario, mark.index)?;
        match mark.kind {
            GroundLabel::Drop => out.drops.push(Marker {
                position: at,
                facing: facing(scenario, mark.index)?,
            }),
            GroundLabel::Cover => out.covers.push(Marker {
                position: at,
                facing: facing(scenario, mark.index)?,
            }),
            GroundLabel::Base => out.bases.push(BaseMarker {
                position: at,
                team: team_of(scenario, mark)?,
            }),
            GroundLabel::Site => out.sites.push(at),
            GroundLabel::Goal => out.goals.push(at),
            GroundLabel::Barrier => {}
            GroundLabel::Line => out.lines.push(LineMarker {
                position: at,
                // The document's check keeps the number in 0..=65535.
                phase: u16::try_from(mark.number.unwrap_or(0)).unwrap_or(0),
                team: team_of(scenario, mark)?,
            }),
        }
    }
    Ok(out)
}

/// The kit's recipe for a document's. The kit's version is the
/// document's own, so a recipe for another kit is refused by the build.
pub(crate) fn recipe(recipe: &GroundRecipe) -> ground::Recipe {
    ground::Recipe {
        kit_version: recipe.kit_version,
        home: if recipe.home == 0 {
            Team::Red
        } else {
            Team::Blue
        },
        mode: if recipe.probe { Mode::Probe } else { Mode::Kit },
    }
}
