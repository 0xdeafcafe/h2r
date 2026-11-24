//! The game's content packages (docs/editor/studio-redesign.md, "How the
//! game consumes packages"): the baked ones (`content/<id>/`, baked by
//! `build.rs`), then `<data>/packages/<id>/`, ordered and switched by
//! `<data>/packages/set.toml`, with a package whose game feature is off left
//! out whole (`workspace::GameSet`).
//!
//! The per-map items reach the game through one `vfs::Rewrite`,
//! `workspace::PackageRewrite`, cached under `<data>/packages/cache/`;
//! `packages.list` on the agent lists the set and each map read.
use std::path::Path;
use std::sync::{Arc, OnceLock};

use agent::{Access, Agent};
use agent_proto::method;
use workspace::{GameSet, PackageRewrite, Served};

mod baked {
    include!(concat!(env!("OUT_DIR"), "/packages_baked.rs"));
}

/// The set this boot read.
static SET: OnceLock<Arc<GameSet>> = OnceLock::new();

/// The baked packages, with a line for each that did not read: what
/// Studio's browser lists beside the user's.
pub fn baked() -> (Vec<workspace::Loaded>, Vec<String>) {
    workspace::baked(baked::BAKED)
}

/// Build just the baked UI package for `h2 ui inject`.
pub fn ui_rewrite(data_root: &Path) -> Result<PackageRewrite, String> {
    let (loaded, problems) = baked();
    let ui = loaded.into_iter().find(|p| p.package.name() == "ui")
        .ok_or_else(|| format!("the baked UI package is unavailable: {}", problems.join("; ")))?;
    Ok(PackageRewrite::new(vec![ui]).with_cache(data_root))
}

/// The boot's set and its rewrite.
pub struct Packages {
    pub set: Arc<GameSet>,
    pub served: Served,
    /// `None` when no package on has an item.
    pub rewrite: Option<PackageRewrite>,
}

/// Read the set: baked, then `<data_root>/packages/`, `feature_on` saying
/// which game features are on. Every problem is a line on stderr.
pub fn at_boot(data_root: &Path, feature_on: &dyn Fn(&str) -> bool) -> Packages {
    let (baked, mut problems) = workspace::baked(baked::BAKED);
    let mut set = GameSet::read(baked, data_root, feature_on);
    problems.append(&mut set.problems);
    set.problems = problems;
    for p in &set.problems {
        eprintln!("packages: {p}");
    }
    // The pogo stick's weapons are in the maps the `pogo` package built:
    // read them from what the game is handed, for the loadout.
    let rewrite = PackageRewrite::new(set.loaded())
        .with_cache(data_root)
        .observed(|stem, bytes| match bytes {
            Some(bytes) => pogo_kit::served::publish(stem, bytes),
            None => pogo_kit::served::clear(stem),
        });
    let served = rewrite.served();
    let rewrite = (!rewrite.is_empty()).then_some(rewrite);
    let set = Arc::new(set);
    let _ = SET.set(Arc::clone(&set));
    Packages {
        set,
        served,
        rewrite,
    }
}

/// Why a Live or System Link session cannot start with this boot's set:
/// an unsigned package with a `sim` item is on, named. Local games are not
/// refused.
pub fn network_refusal() -> Option<String> {
    SET.get()?.network_refusal()
}

/// Register `packages.list` (read, no params).
pub fn install(agent: &Agent, set: Arc<GameSet>, served: Served) {
    agent.add_method(method::PACKAGES_LIST, Access::Read, move |_| {
        Ok(set.list(&served))
    });
}
