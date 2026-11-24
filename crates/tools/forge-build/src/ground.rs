//! AI ground for a document that asks for it
//! (docs/game/game-types-without-carriers.md, "Maps for scripted games").
//!
//! The materialiser reads the document's ground marks from the finished
//! scenario (a flag's position, facing and team) and hands them, with the
//! recipe, to `firefight_kit::ground::build` over the materialised map. The
//! result is the map: nothing of it is in the document.
use firefight_kit::{
    Mode, Team,
    ground::{self, BaseMarker, Labels, LineMarker, Marker, Problem, Receipt},
};
use forge_doc::{BlockKind, GroundLabel, GroundMark, GroundRecipe};
use scenario_edit::Scenario;

use crate::Error;

/// What the ground build made, besides the map: how many zones and squad
/// slots it has, the labels it could not honour, and the receipt of its
/// inputs.
#[derive(Clone, Debug)]
pub struct GroundReport {
    pub zones: usize,
    pub squads: usize,
    pub problems: Vec<Problem>,
    pub receipt: Receipt,
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
    for mark in marks {
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
pub(crate) fn recipe(recipe: GroundRecipe) -> ground::Recipe {
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
