//! Firefight's wave sets as `rules` items (docs/game/firefight.md, the built-in
//! sets; docs/editor/studio-redesign.md, "Contents").
//!
//! A set is a [`firefight::Def`] as JSON, in a file ending
//! [`SUFFIX`] (`content/modes/firefight/holdout.firefight.json`). The
//! director loads the set as rules script data (`firefight::script::load`):
//! the item is good when the definition is structurally valid and the
//! director verifies with its data, so the same check a lobby makes of a
//! wave set is made of the package's. It adds no tag. [`sets`] is what the
//! game reads from a set of packages; the built-in four are the same files
//! (`firefight::builtin::SETS`).
use content::package::{Item, Kind};
use firefight::Def;
use firefight::script::load;

use crate::build::{Cx, Failure, Loaded};
use crate::builders::Job;

/// The ending of a wave set's file.
pub const SUFFIX: &str = ".firefight.json";

/// Whether `item` is a wave set: a `rules` item whose source ends [`SUFFIX`].
pub fn is_set(item: &Item) -> bool {
    item.kind == Kind::Rules
        && item
            .source
            .as_deref()
            .is_some_and(|s| s.to_ascii_lowercase().ends_with(SUFFIX))
}

fn read(job: &Job<'_>) -> Result<Def, Failure> {
    if !is_set(job.item) {
        return Err(format!(
            "this build reads Firefight wave sets ({SUFFIX}) as `rules` items; a script is for the modes builders"
        )
        .into());
    }
    if job.item.tag.is_some() {
        return Err("a wave set adds no tag: drop `tag`".into());
    }
    let def: Def = serde_json::from_slice(job.source).map_err(|e| Failure {
        line: Some(e.line()),
        why: format!("not a Firefight set: {e}"),
    })?;
    def.canonical_json().map_err(|e| e.to_string())?;
    load(&def).map_err(|e| e.to_string())?;
    Ok(def)
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}

/// Changes nothing in the map: the lobby reads the set. The file is read
/// again so a build stops on a bad one.
pub fn apply(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}

/// The wave sets of `packages`, in the set's order with each package's
/// items in theirs, by package id. An item that does not read is left out
/// (`check_package` names it).
pub fn sets(packages: &[Loaded]) -> Vec<(String, Def)> {
    let mut out = Vec::new();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        for item in manifest.items.iter().filter(|i| is_set(i)) {
            let Some(bytes) = item.source.as_ref().and_then(|s| loaded.files.get(s)) else {
                continue;
            };
            let job = Job {
                item,
                source: bytes,
                files: &loaded.files,
            };
            if let Ok(def) = read(&job) {
                out.push((manifest.package.id.clone(), def));
            }
        }
    }
    out
}
