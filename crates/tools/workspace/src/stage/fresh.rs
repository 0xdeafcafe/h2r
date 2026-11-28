//! The stage as a package's `map` item (docs/editor/studio-redesign.md,
//! "Contents": `map`): [`StageFresh`] is the `workspace::Fresh` that turns
//! a `fresh` map's `map.toml` into the stage, through [`content_kit::stage::map::build`].
//!
//! The kit names are the stage's alone (`kits = ["stage"]`); the layout is
//! the repository's ([`content_kit::stage::SOURCE`]). The base map and the shared caches
//! it leans on are read through `workspace::BaseMaps`, so the disc is the
//! caller's (the VFS, in the game).
use std::io::Cursor;

use content::Sha256;
use map::{Map, ResourceFile};
use tags::repo::{MapSource, SharedFiles};
use tags::schema::Schema;
use crate::{BaseMaps, Fresh, FreshJob};

use content_kit::stage::map::{Base, Project, build};

/// The shared caches a map reads resources from, by file stem.
const SHARED: [(&str, ResourceFile); 3] = [
    ("mainmenu", ResourceFile::MainMenu),
    ("shared", ResourceFile::Shared),
    ("single_player_shared", ResourceFile::SinglePlayerShared),
];

/// The stage builder over the disc's maps.
pub struct StageFresh<'a> {
    /// Where the shared caches come from; the base map itself is the job's.
    pub bases: &'a dyn BaseMaps,
}

/// The project a `fresh` map item stands for.
pub fn project_of(job: &FreshJob<'_>) -> Result<Project, String> {
    let spec = crate::MapSpec {
        mode: crate::Mode::Fresh,
        base: job.base.to_owned(),
        name: job.name.to_owned(),
        scenario: Some(job.scenario.to_owned()),
        kits: job.kits.to_vec(),
        ground: None,
        title: None,
        author: None,
        description: None,
        edits: Vec::new(),
        imports: Vec::new(),
    };
    super::project_of_spec(&spec).map_err(|e| e.to_string())
}

impl Fresh for StageFresh<'_> {
    fn build(&self, job: &FreshJob<'_>) -> Result<Vec<u8>, String> {
        let project = project_of(job)?;
        let schema = Schema::builtin();
        let mut shared = SharedFiles::default();
        for (stem, file) in SHARED {
            // A map that does not lean on a cache needs none of them; one
            // that does stops the build naming the pointer.
            let Ok(bytes) = self.bases.read(stem) else {
                continue;
            };
            let map = Map::from_reader(Cursor::new(bytes))
                .map_err(|e| format!("the shared cache {stem}: {e}"))?;
            shared
                .set(schema, file, map)
                .map_err(|e| format!("the shared cache {stem}: {e}"))?;
        }
        let map = Map::from_reader(Cursor::new(job.base_bytes.to_vec()))
            .map_err(|e| format!("the base map {}: {e}", job.base))?;
        let mut src = MapSource::new(schema, map, &mut shared).map_err(|e| e.to_string())?;
        let base = Base::from_map(&mut src, job.base).map_err(|e| e.to_string())?;
        let built = build(&mut src, &base, &project).map_err(|e| e.to_string())?;
        Ok(built.bytes)
    }

    fn version(&self) -> String {
        format!(
            "stage-kit {} {}",
            content_kit::stage::version(),
            Sha256::of(content_kit::stage::SOURCE.as_bytes())
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// The shared caches of a disc that is not there.
    struct NoDisc;

    impl BaseMaps for NoDisc {
        fn read(&self, name: &str) -> Result<Vec<u8>, String> {
            Err(format!("no map {name}"))
        }
    }

    fn job<'a>(
        kits: &'a [String],
        files: &'a BTreeMap<String, Vec<u8>>,
        base_bytes: &'a [u8],
    ) -> FreshJob<'a> {
        FreshJob {
            name: "stage",
            base: "invented",
            base_bytes,
            scenario: "invented\\scenarios\\stage",
            kits,
            files,
        }
    }

    #[test]
    fn the_job_is_the_stages_project() {
        let files = BTreeMap::new();
        let kits = ["stage".to_owned()];
        let p = project_of(&job(&kits, &files, &[])).unwrap();
        assert_eq!(
            (p.name.as_str(), p.base.as_str(), p.mode.as_str()),
            ("stage", "invented", "fresh")
        );
        assert_eq!(p.kits, ["stage"]);
        assert_eq!(p.scenario, "invented\\scenarios\\stage");
    }

    #[test]
    fn another_kit_is_refused_by_name() {
        let files = BTreeMap::new();
        let kits = ["firefight".to_owned()];
        let why = StageFresh { bases: &NoDisc }
            .build(&job(&kits, &files, &[]))
            .unwrap_err();
        assert!(why.contains("firefight"), "{why}");
    }

    #[test]
    fn a_base_that_is_not_a_map_stops_the_build_naming_it() {
        let files = BTreeMap::new();
        let kits = ["stage".to_owned()];
        let why = StageFresh { bases: &NoDisc }
            .build(&job(&kits, &files, b"not a map"))
            .unwrap_err();
        assert!(why.contains("the base map invented"), "{why}");
    }

    #[test]
    fn the_version_follows_the_layout_and_the_kit() {
        let v = StageFresh { bases: &NoDisc }.version();
        assert!(v.starts_with(&format!("stage-kit {} ", content_kit::stage::version())), "{v}");
        assert!(v.ends_with(&Sha256::of(content_kit::stage::SOURCE.as_bytes()).to_string()));
    }

    /// The disc build's job and the shipped `map.toml` give one project:
    /// the same map from the same base.
    #[test]
    fn the_job_of_the_shipped_map_item_is_the_builtin_project() {
        let spec = crate::MapSpec::parse(content_kit::stage::PROJECT.as_bytes()).unwrap();
        let files = BTreeMap::new();
        let job = FreshJob {
            name: &spec.name,
            base: &spec.base,
            base_bytes: &[],
            scenario: spec.scenario.as_deref().unwrap(),
            kits: &spec.kits,
            files: &files,
        };
        assert_eq!(project_of(&job).unwrap(), crate::stage::builtin_project().unwrap());
    }
}
