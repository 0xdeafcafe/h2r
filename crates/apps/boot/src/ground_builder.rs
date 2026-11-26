//! The AI ground builder Forge's **Build AI ground** and **Test waves** rows
//! run (docs/game/forge-map-variants.md, "AI ground"): `forge_build::build`
//! over the document's play form, with the ground recipe, over the maps of
//! this machine's disc. `game` may not use `tools`, so the composition root
//! supplies this, through `forge_glue::ground::install`.
//!
//! The build runs on the worker the editor gives it
//! (`forge_glue::ground`), never on the game thread. What a build makes
//! beyond its report, the map, goes to the `after` hook: the composition
//! root hands it to `forge_glue::reload::keep_ground_copy`, and the game
//! reloads the running map onto it. With no hook the map is dropped.
//!
//! **Rebuild map** builds its copy the same way ([`AppCopy`]), with the
//! session's kit and without the ground (docs/game/forge-design.md,
//! "Rebuild map").
use std::path::PathBuf;

use forge::{GroundBuilder, GroundProblem, GroundReceipt, GroundRecipe, GroundReport};
use forge_doc::ForgeDoc;
use scenario_edit::MapSource;

/// The maps the build reads, by name.
pub type Maps = Box<dyn FnMut() -> Result<Vec<(String, MapSource)>, String> + Send>;

/// What happens with a map the build made, once its report is made: the
/// hook that loads it. An `Err` is the build's.
pub type After = Box<dyn FnMut(&forge_build::Built) -> Result<(), String> + Send>;

pub struct AppGround {
    maps: Maps,
    after: Option<After>,
}

impl AppGround {
    pub fn new(maps: Maps) -> Self {
        Self { maps, after: None }
    }

    /// The maps of the disc image at `disc`.
    pub fn disc(disc: PathBuf) -> Self {
        Self::new(disc_maps(disc))
    }

    /// Hand each map built to `after`.
    pub fn after(mut self, after: After) -> Self {
        self.after = Some(after);
        self
    }
}

impl GroundBuilder for AppGround {
    /// This build's kit version, red home, the kit's mode.
    fn recipe(&self) -> GroundRecipe {
        GroundRecipe {
            kit_version: firefight_kit::KIT_VERSION,
            home: 0,
            probe: false,
        }
    }

    fn build(&mut self, play: &ForgeDoc) -> Result<GroundReport, String> {
        let built = materialise(&mut self.maps, play)?;
        let report = built
            .ground
            .as_ref()
            .map(report_of)
            .ok_or("The document asks for no AI ground.")?;
        if let Some(after) = &mut self.after {
            after(&built)?;
        }
        Ok(report)
    }
}

/// `forge_build::build` of `doc` over the maps `maps` reads.
fn materialise(maps: &mut Maps, doc: &ForgeDoc) -> Result<forge_build::Built, String> {
    let maps = maps()?;
    forge_build::build(doc, |name| {
        maps.iter()
            .find(|(map, _)| map.eq_ignore_ascii_case(name))
            .map(|(_, source)| source.clone())
    })
    .map_err(|e| e.to_string())
}

/// The play copy **Rebuild map** reloads onto: the session's live form
/// (`forge::live_form`) materialised over the disc's maps.
pub struct AppCopy {
    maps: Maps,
}

impl AppCopy {
    pub fn new(maps: Maps) -> Self {
        Self { maps }
    }

    /// The maps of the disc image at `disc`.
    pub fn disc(disc: PathBuf) -> Self {
        Self::new(disc_maps(disc))
    }
}

impl game::forge_glue::reload::CopyBuilder for AppCopy {
    fn build(&mut self, doc: &ForgeDoc) -> Result<Vec<u8>, String> {
        materialise(&mut self.maps, doc).map(|built| built.map_bytes().to_vec())
    }
}

fn disc_maps(disc: PathBuf) -> Maps {
    Box::new(move || {
        MapSource::disc_maps(&disc).map_err(|e| format!("The disc's maps do not read ({e})."))
    })
}

/// The session's report of a build: its zones, squad slots, the labels it
/// could not honour and the digests of its inputs.
pub fn report_of(report: &forge_build::GroundReport) -> GroundReport {
    GroundReport {
        zones: report.zones,
        squads: report.squads,
        problems: report
            .problems
            .iter()
            .map(|p| GroundProblem {
                label: p.label.to_owned(),
                position: p.position,
                reason: p.reason.to_owned(),
            })
            .collect(),
        receipt: Some(GroundReceipt {
            base: report.receipt.base.to_string(),
            labels: report.receipt.labels.to_string(),
            kit_version: report.receipt.recipe.kit_version,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::Sha256;
    use firefight_kit::{
        Mode,
        ground::{Labels, Problem, Receipt, Recipe},
    };
    use forge_doc::{BuilderVersions, FORMAT, MapRef};

    fn document() -> ForgeDoc {
        ForgeDoc {
            format: FORMAT,
            builder: BuilderVersions::CURRENT,
            base: MapRef {
                map: "court".into(),
                sha256: Sha256::of(b"an invented court"),
            },
            sources: vec![],
            title: "Waves".into(),
            author: "Builder".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
        }
    }

    #[test]
    fn the_recipe_is_this_builds_kit_red_home_and_the_kits_mode() {
        let builder = AppGround::new(Box::new(|| Ok(vec![])));
        assert_eq!(
            builder.recipe(),
            GroundRecipe {
                kit_version: firefight_kit::KIT_VERSION,
                home: 0,
                probe: false,
            }
        );
    }

    #[test]
    fn a_report_carries_the_counts_the_problems_and_the_receipt() {
        let recipe = Recipe::new(Mode::Kit);
        let made = forge_build::GroundReport {
            zones: 4,
            squads: 16,
            problems: vec![Problem {
                label: "ff_drop",
                index: 1,
                position: [1., 2., 3.],
                reason: "off the walkable area",
            }],
            receipt: Receipt::of(Sha256::of(b"base"), &Labels::default(), &recipe),
        };
        let report = report_of(&made);
        assert_eq!((report.zones, report.squads), (4, 16));
        assert_eq!(
            report.problems,
            vec![GroundProblem {
                label: "ff_drop".into(),
                position: [1., 2., 3.],
                reason: "off the walkable area".into(),
            }]
        );
        let receipt = report.receipt.unwrap();
        assert_eq!(receipt.base, Sha256::of(b"base").to_string());
        assert_eq!(receipt.labels, Labels::default().digest().to_string());
        assert_eq!(receipt.kit_version, firefight_kit::KIT_VERSION);
    }

    #[test]
    fn a_map_the_disc_lacks_or_cannot_read_fails_the_build_with_the_reason() {
        let mut none = AppGround::new(Box::new(|| Ok(vec![])));
        let error = none.build(&document()).unwrap_err();
        assert!(error.contains("court.map is missing"), "{error}");

        let mut broken = AppGround::new(Box::new(|| Err("The disc is not there.".into())));
        assert_eq!(
            broken.build(&document()).unwrap_err(),
            "The disc is not there."
        );
    }

    #[test]
    fn a_rebuild_copy_fails_by_the_builds_reason() {
        use game::forge_glue::reload::CopyBuilder;
        let mut none = AppCopy::new(Box::new(|| Ok(vec![])));
        let error = none.build(&document()).unwrap_err();
        assert!(error.contains("court.map is missing"), "{error}");
        let mut broken = AppCopy::new(Box::new(|| Err("The disc is not there.".into())));
        assert_eq!(broken.build(&document()).unwrap_err(), "The disc is not there.");
    }
}
