//! The stage as a step of a project build (docs/editor/tag-workspace.md,
//! "Kits are build steps"): `workspace::project_build::Kit` over
//! [`map::build`](crate::map::build), reading the base through the
//! project's stack, so its edit files and import layers apply to the tags
//! the stage takes from the base.
//!
//! [`KITS`] and [`KEEP`] are what a caller of
//! `workspace::project_build::build` passes (`h2 tags project build`,
//! Studio's `project.build`).
use workspace::project_build::{Installed, Keep, Kit, KitStep};

use crate::map::{Base, Project, build};

/// A fresh closure's roots besides the globals and the scenario: the tags
/// the engine finds by name (`h1convert::fresh`).
pub const KEEP: Keep = Keep {
    groups: &h1convert::fresh::KEEP_GROUPS,
    prefixes: &h1convert::fresh::KEEP_PREFIXES,
};

/// The kits a project can name.
pub const KITS: &[&dyn Kit] = &[&StageKit];

/// `stage`: makes the whole map, so it is a fresh project's only kit.
pub struct StageKit;

impl Kit for StageKit {
    fn name(&self) -> &str {
        "stage"
    }

    fn fingerprint(&self) -> String {
        format!(
            "stage v{} layout {}",
            crate::VERSION,
            tags::repo::hex(&tags::repo::sha256(crate::SOURCE.as_bytes()))
        )
    }

    fn run(&self, step: &mut KitStep<'_>) -> Result<Installed, String> {
        let p = step.project;
        let scenario = p
            .scenario
            .clone()
            .ok_or("the stage is a fresh project's kit: name its scenario")?;
        if step.set.is_some() || step.bytes.is_some() {
            return Err(format!(
                "the stage makes the whole map, so it is the project's only kit and no layer may hold {scenario}"
            ));
        }
        let stage = Project {
            name: p.name.clone(),
            mode: "fresh".into(),
            base: p.base.clone(),
            scenario,
            kits: vec!["stage".into()],
            // The stack has applied the project's edit files already.
            edits: Vec::new(),
            output: "package".into(),
        };
        let (base, _) = Base::from_repo(&step.roots.tree, &p.base).map_err(|e| e.to_string())?;
        let built = build(&mut *step.src, &base, &stage).map_err(|e| e.to_string())?;
        let r = &built.report;
        let notes = vec![format!(
            "{} tags, {} from the base; key {}",
            r.tags, r.from_base, r.key
        )];
        let added = r.own.clone();
        step.bytes = Some(built.bytes);
        Ok(Installed { added, notes })
    }
}
