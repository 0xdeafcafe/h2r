//! The AI ground a `map` item asks for (docs/editor/studio-redesign.md,
//! "Contents": `map`; docs/game/game-types-without-carriers.md, "In
//! Studio"): `firefight_kit::ground::build` over the map the item makes,
//! with the labels of its ground file. `workspace` holds the file
//! (`workspace::GroundFile`) and asks for the build through
//! `workspace::Fresh::ground`; the generator is above it, so the
//! composition root supplies it here, beside the stage builder.
use std::io::Cursor;

use firefight_kit::{
    KIT_VERSION, Mode, Team,
    ground::{self, BaseMarker, Labels, LineMarker, Marker, Recipe},
};
use workspace::stage::fresh::StageFresh;
use workspace::{Fresh, FreshJob, GroundFile, GroundJob, LabelName, Side};

/// The stage as the `fresh` builder, and the Firefight kit as the ground's.
pub struct AppFresh<'a>(pub StageFresh<'a>);

impl Fresh for AppFresh<'_> {
    fn build(&self, job: &FreshJob<'_>) -> Result<Vec<u8>, String> {
        self.0.build(job)
    }

    fn version(&self) -> String {
        self.0.version()
    }

    fn ground(&self, job: &GroundJob<'_>) -> Result<Vec<u8>, String> {
        // The kit names the map it builds over by its own header.
        let map = ::map::Map::from_reader(Cursor::new(job.map))
            .map_err(|e| format!("the map {} does not read: {e}", job.name))?;
        let name = map.header().name.clone();
        // The Elite's behaviour rows come from the disc's campaign Elite;
        // a disc without the map leaves the build refused by name.
        let elite_bytes = self.0.bases.read(firefight_kit::ELITE_DONOR_MAP).ok();
        let elite = elite_bytes.as_deref().map(|bytes| firefight_kit::Donor {
            name: firefight_kit::ELITE_DONOR_MAP,
            bytes,
        });
        let built = ground::build_with_elite(
            job.map,
            &name,
            &labels_of(job.ground),
            &recipe_of(job.ground),
            None,
            elite.as_ref(),
        )
        .map_err(|e| format!("the AI ground of {}: {e:#}", job.name))?;
        for problem in &built.problems {
            eprintln!(
                "packages: the AI ground of {}: {} at {:?}: {}",
                job.name, problem.label, problem.position, problem.reason
            );
        }
        Ok(built.bytes)
    }

    fn ground_version(&self) -> String {
        format!("firefight-kit {KIT_VERSION}")
    }
}

fn team(side: Side) -> Team {
    match side {
        Side::Red => Team::Red,
        Side::Blue => Team::Blue,
    }
}

/// The generator's recipe for a ground file: this build's kit, the file's
/// home.
pub fn recipe_of(file: &GroundFile) -> Recipe {
    Recipe::new(Mode::Kit).home(team(file.home))
}

/// The labels of a ground file, as the generator reads them.
pub fn labels_of(file: &GroundFile) -> Labels {
    let mut out = Labels::default();
    for l in &file.labels {
        let at = l.position;
        let team_of = || team(l.team.unwrap_or_default());
        match l.label {
            LabelName::FfDrop => out.drops.push(Marker {
                position: at,
                facing: l.facing.unwrap_or(0.0),
            }),
            LabelName::FfCover => out.covers.push(Marker {
                position: at,
                facing: l.facing.unwrap_or(0.0),
            }),
            LabelName::FfBase => out.bases.push(BaseMarker {
                position: at,
                team: team_of(),
            }),
            LabelName::SiegeSite => out.sites.push(at),
            LabelName::SiegeGoal => out.goals.push(at),
            LabelName::SiegeLine => out.lines.push(LineMarker {
                position: at,
                phase: l.phase.unwrap_or(0),
                team: team_of(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "home: blue\nlabels:\n\
        - {label: ff_drop, position: [1.0, 2.0, 3.0], facing: 0.5}\n\
        - {label: ff_cover, position: [4.0, 5.0, 6.0], facing: 1.0}\n\
        - {label: ff_base, position: [7.0, 8.0, 9.0], team: blue}\n\
        - {label: siege_site, position: [1.0, 1.0, 1.0]}\n\
        - {label: siege_goal, position: [2.0, 2.0, 2.0]}\n\
        - {label: siege_line, position: [3.0, 3.0, 3.0], team: red, phase: 2}\n";

    #[test]
    fn a_ground_file_is_the_labels_the_generator_reads() {
        let file = GroundFile::parse(FILE.as_bytes()).unwrap();
        let labels = labels_of(&file);
        assert_eq!(labels.drops, [Marker { position: [1.0, 2.0, 3.0], facing: 0.5 }]);
        assert_eq!(labels.covers, [Marker { position: [4.0, 5.0, 6.0], facing: 1.0 }]);
        assert_eq!(
            labels.bases,
            [BaseMarker { position: [7.0, 8.0, 9.0], team: Team::Blue }]
        );
        assert_eq!(labels.sites, [[1.0, 1.0, 1.0]]);
        assert_eq!(labels.goals, [[2.0, 2.0, 2.0]]);
        assert_eq!(
            labels.lines,
            [LineMarker { position: [3.0, 3.0, 3.0], phase: 2, team: Team::Red }]
        );
        assert_eq!(recipe_of(&file).home, Team::Blue);
        assert_eq!(recipe_of(&file).kit_version, KIT_VERSION);
    }
}
