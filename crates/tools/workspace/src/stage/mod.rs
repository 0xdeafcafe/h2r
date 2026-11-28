//! The content stage as a project's kit and as a package's `map` item: the
//! adapters from `content_kit::stage` (the stage's own build) to this
//! crate's [`Fresh`](crate::Fresh) and `project_build::Kit`.
//!
//! - [`fresh`]: the stage as a package's `map` item.
//! - [`project_kit`]: the stage as a step of a project build.

pub mod fresh;
pub mod project_kit;

use content_kit::stage::map::{Project, Spec};
use content_kit::stage::Error;

/// A `map.toml`'s text as the stage's project ([`Project::from_spec`]).
pub fn project(text: &str) -> Result<Project, Error> {
    let spec = crate::MapSpec::parse(text.as_bytes()).map_err(Error::Project)?;
    project_of_spec(&spec)
}

/// A parsed `map.toml` as the stage's project.
pub fn project_of_spec(spec: &crate::MapSpec) -> Result<Project, Error> {
    Project::from_spec(&Spec {
        fresh: spec.mode == crate::Mode::Fresh,
        base: spec.base.clone(),
        name: spec.name.clone(),
        scenario: spec.scenario.clone(),
        kits: spec.kits.clone(),
    })
}

/// The project shipped with the repository (`content_kit::stage::PROJECT`).
pub fn builtin_project() -> Result<Project, Error> {
    project(content_kit::stage::PROJECT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_parses_and_refuses_what_it_cannot_build() {
        let p = builtin_project().unwrap();
        assert_eq!((p.name.as_str(), p.mode.as_str()), ("stage", "fresh"));
        assert_eq!(p.kits, ["stage"]);
        for (from, to) in [
            ("mode = \"fresh\"", "mode = \"onto\""),
            ("kits = [\"stage\"]", "kits = [\"stage\", \"pogo\"]"),
            (
                "kits = [\"stage\"]",
                "kits = [\"stage\"]\nedits = [\"x\", \"x\"]",
            ),
            ("name = \"stage\"", "name = \"a stage\""),
        ] {
            let text = content_kit::stage::PROJECT.replace(from, to);
            assert_ne!(text, content_kit::stage::PROJECT, "{from}");
            assert!(
                matches!(project(&text), Err(Error::Project(_))),
                "{to}"
            );
        }
    }
}
