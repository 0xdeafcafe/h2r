//! The game's content packages (docs/editor/studio-redesign.md, "How the
//! game consumes packages"): the baked ones (`content/<id>/`, baked by
//! `build.rs`), then `<data>/packages/<id>/`, ordered and switched by
//! `<data>/packages/set.toml`, with a package whose game feature is off left
//! out whole (`workspace::GameSet`).
//!
//! The per-map items reach the game through one `vfs::Rewrite`,
//! `workspace::PackageRewrite`, cached under `<data>/packages/cache/`;
//! `packages.list` on the agent lists the set and each map read. With the
//! maps served from the tag store (`store-maps`), they reach it as level
//! images instead (`workspace::level::StoreLevels`, under
//! `<data>/store/images/`; docs/engine/asset-store.md, "Runtime assembly"),
//! and the rewrite rebuilds no map.
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent::{Access, Agent};
use agent_proto::method;
use vfs::Backend as _;
use workspace::{GameSet, KitInputs, Origin, PackageRewrite, Served};

mod baked {
    include!(concat!(env!("OUT_DIR"), "/packages_baked.rs"));
}

/// The boot's set admission result.
static NETWORK_REFUSAL: OnceLock<Option<String>> = OnceLock::new();

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
    pub summary: serde_json::Value,
    pub served: Served,
    /// `None` when no package on has an item.
    pub rewrite: Option<PackageRewrite>,
    /// The level images, when the maps are served from the tag store
    /// (`vfs::Config::store_levels`).
    pub levels: Option<vfs::store_map::SharedLevels>,
}

/// A single package load feeds map preparation and the runtime rewrite.
pub struct BootSet {
    pub loaded: Vec<workspace::Loaded>,
    summary: serde_json::Value,
    strict: BTreeSet<String>,
    refusal: Option<String>,
}

/// Read the set before preparing the maps that its `map` items name.
pub fn read_set(data_root: &Path, feature_on: &dyn Fn(&str) -> bool) -> BootSet {
    let (baked, mut problems) = workspace::baked(baked::BAKED);
    let mut set = GameSet::read(baked, data_root, feature_on);
    problems.append(&mut set.problems);
    set.problems = problems;
    for p in &set.problems {
        eprintln!("packages: {p}");
    }
    // Our own packages: a refusal of one for a map (a `from`, `was` or
    // `replaces` that does not match the base) stops the map's build, where
    // an installed package refused is off for that map with its reason.
    let strict = set
        .on
        .iter()
        .filter(|e| e.origin == Origin::Baked)
        .map(|e| e.loaded.package.name().to_owned())
        .collect();
    BootSet {
        summary: set.list(&Served::default()),
        refusal: set.network_refusal(),
        strict,
        loaded: set.on.into_iter().map(|entry| entry.loaded).collect(),
    }
}

/// The `map-variant` items of `loaded` as the map variant store holds
/// them. One that does not read is left out; `h2 package check` names it.
pub fn packaged_variants(loaded: &[workspace::Loaded]) -> Vec<forge::files::variants::Packaged> {
    workspace::map_variants(loaded)
        .iter()
        .filter_map(|v| forge::files::variants::Packaged::read(&v.package, &v.stem, &v.bytes))
        .collect()
}

/// Install the boot's package set. `kits` is what the set's `kit` items run on:
/// the campaigns' level pictures and the button icon sets. `store` is the
/// tag store the maps are served from (`store-maps`), if they are: the
/// set's map items then reach the game as level images.
pub fn at_boot(data_root: &Path, set: BootSet, kits: KitInputs, store: Option<&Path>) -> Packages {
    // The built-in list entries are the `variant` items of the packages
    // that are on: a package off takes its entries out of the game's list.
    // This is before the list is first read (`community_variants::install`).
    game::community_variants::install_registry(workspace::script_registry(&set.loaded));
    // The set's `map-variant` items join the map variant store: the lobby,
    // Change Map, Forge and a game variant's suggestions list and load
    // them with the ones saved here (`forge::files::variants`).
    forge::files::variants::install_packaged(packaged_variants(&set.loaded));
    let loaded: Arc<[workspace::Loaded]> = set.loaded.into();
    let mut rewrite = PackageRewrite::new(Arc::clone(&loaded)).strict(set.strict.clone());
    if store.is_some() {
        rewrite = rewrite.levels_from_store();
    } else {
        rewrite = rewrite.with_cache(data_root);
    }
    // Read the pogo stick's weapons and the superbounce HUD font from
    // the rewritten maps and fonts that the game actually receives.
    let rewrite = rewrite
        .with_kits(kits)
        .observing_fonts(game::superbounce_font::Capture::keep)
        .observed(|stem, map| {
            content_kit::served::publish(stem, std::io::BufReader::new(content::patch::Reader::new(map)))
        });
    let served = rewrite.served();
    let levels = store.map(|root| {
        workspace::level::StoreLevels::new(
            root.to_path_buf(),
            workspace::level::images_dir(data_root),
            loaded,
            served.clone(),
        )
        .strict(set.strict)
        .shared()
    });
    let rewrite = (!rewrite.is_empty()).then_some(rewrite);
    let _ = NETWORK_REFUSAL.set(set.refusal);
    Packages {
        summary: set.summary,
        served,
        rewrite,
        levels,
    }
}

/// The disc's maps, read through the VFS's disc backend, opened when one is
/// first asked for.
struct DiscMaps {
    disc: PathBuf,
    open: RefCell<Option<vfs::AnyDisc>>,
}

impl workspace::BaseMaps for DiscMaps {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        let mut slot = self.open.borrow_mut();
        if slot.is_none() {
            let disc = vfs::open_disc(&self.disc)
                .map_err(|e| format!("opening the disc {}: {e}", self.disc.display()))?;
            *slot = Some(disc);
        }
        let disc = slot.as_mut().expect("opened above");
        let path = format!("maps\\{name}.map");
        let meta = disc
            .stat(&path)
            .map_err(|e| format!("{path} on the disc: {e}"))?;
        if meta.is_directory() {
            return Err(format!("{path} on the disc is a directory"));
        }
        let len = usize::try_from(meta.end_of_file).map_err(|_| format!("{path} is too large"))?;
        let mut out = vec![0; len];
        let mut at = 0;
        while at < out.len() {
            let n = disc
                .read_at(&path, at as u64, &mut out[at..])
                .map_err(|e| format!("reading {path} on the disc: {e}"))?;
            if n == 0 {
                return Err(format!("{path} on the disc ended early"));
            }
            at += n;
        }
        Ok(out)
    }
}

/// Build the maps `loaded` holds `map` and `map-patch` items for
/// (`workspace::prepare`) into `<data_root>/maps/`, over the disc's maps,
/// with the stage as the `fresh` builder. Every item that did not build is
/// a line on stderr; the rest still did.
pub fn prepare_maps(data_root: &Path, disc: &Path, loaded: &[workspace::Loaded]) -> workspace::Prepared {
    let wants = loaded.iter().any(|l| {
        l.package.manifest.items.iter().any(|i| {
            matches!(
                i.kind,
                content::package::Kind::Map | content::package::Kind::MapPatch
            )
        })
    });
    if !wants {
        return workspace::Prepared::default();
    }
    let bases = DiscMaps {
        disc: disc.to_path_buf(),
        open: RefCell::new(None),
    };
    let stage = workspace::stage::fresh::StageFresh { bases: &bases };
    // The Firefight kit builds the ground a `map` item's ground file asks
    // for; a build without it leaves such a map failed, by name.
    #[cfg(feature = "test-maps")]
    let fresh = crate::map_ground::AppFresh(stage);
    #[cfg(not(feature = "test-maps"))]
    let fresh = stage;
    let out = workspace::prepare(&content::store::maps_dir(data_root), loaded, &bases, &fresh);
    for f in &out.failed {
        eprintln!("packages: {} item {}: {}", f.package, f.item, f.why);
    }
    for m in &out.written {
        eprintln!("packages: built the map {} ({})", m.dir, m.package);
    }
    out
}

/// `Ok` when every package item built or was in place; otherwise an error
/// that names each one that did not (package, item and why), for a run that
/// cannot go on without them (`h2 sim`).
pub fn staged_or_named(out: &workspace::Prepared) -> anyhow::Result<()> {
    if out.failed.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = out
        .failed
        .iter()
        .map(|f| format!("package {} item {}: {}", f.package, f.item, f.why))
        .collect();
    anyhow::bail!("packages did not stage: {}", lines.join("; "))
}

/// Why a Live or System Link session cannot start with this boot's set:
/// an unsigned package with a `sim` item is on, named. Local games are not
/// refused.
pub fn network_refusal() -> Option<String> {
    NETWORK_REFUSAL.get()?.clone()
}

/// Register `packages.list` (read, no params).
pub fn install(agent: &Agent, summary: serde_json::Value, served: Served) {
    agent.add_method(method::PACKAGES_LIST, Access::Read, move |_| {
        let mut result = summary.clone();
        let maps = served.lock().unwrap_or_else(|e| e.into_inner());
        result["maps"] = serde_json::json!(*maps);
        Ok(result)
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use content::Sha256;
    use content::package::{Lock, Manifest, Package};
    use workspace::{BaseMaps, Loaded};

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("boot-prepare-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A package of one `map-patch` item over `patch`, for an invented map.
    fn patch_package(patch: &[u8], base: &Sha256) -> Loaded {
        let text = "[package]\nformat = 1\nid = \"mp\"\nname = \"mp\"\n\
            [[item]]\nkind = \"map-patch\"\nsource = \"tiny.ppf\"\ninputs = [\"tiny.base.toml\"]\n";
        let sidecar = format!("map = \"tiny\"\nsha256 = \"{base}\"\n");
        let files: BTreeMap<String, Vec<u8>> = [
            ("tiny.ppf".to_owned(), patch.to_vec()),
            ("tiny.base.toml".to_owned(), sidecar.into_bytes()),
        ]
        .into();
        let lock = Lock::new(
            files
                .iter()
                .map(|(n, b)| (n.clone(), Sha256::of(b)))
                .collect(),
        );
        Loaded {
            package: Package::new(Manifest::from_toml(text).unwrap(), lock).unwrap(),
            files,
        }
    }

    #[test]
    fn a_boot_with_no_map_items_writes_nothing() {
        let data = temp("none");
        let out = prepare_maps(&data, &data.join("no-disc"), &[]);
        assert!(out.written.is_empty() && out.failed.is_empty());
        assert!(!data.join("maps").exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_package_that_did_not_stage_is_named() {
        let mut out = workspace::Prepared::default();
        assert!(staged_or_named(&out).is_ok());
        out.failed.push(workspace::Failed {
            package: "stage".into(),
            item: 0,
            why: "Directory not empty (os error 66)".into(),
        });
        let e = staged_or_named(&out).unwrap_err().to_string();
        assert_eq!(e, "packages did not stage: package stage item 0: Directory not empty (os error 66)");
    }

    #[test]
    fn a_map_patch_is_written_once_without_the_disc() {
        let data = temp("patch");
        let original = vec![7u8; 4096];
        let mut edited = original.clone();
        edited[0x100] ^= 1;
        let patch = content::patch::diff(&original, &edited, "invented").bytes;
        let p = patch_package(&patch, &Sha256::of(&original));
        let disc = data.join("no-disc");
        let first = prepare_maps(&data, &disc, std::slice::from_ref(&p));
        assert!(first.failed.is_empty(), "{first:?}");
        assert_eq!(first.written.len(), 1);
        assert!(data.join("maps").join(&first.written[0].dir).is_dir());
        let second = prepare_maps(&data, &disc, std::slice::from_ref(&p));
        assert_eq!((second.written.len(), second.unchanged.len()), (0, 1));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn the_disc_maps_read_from_a_disc_directory_and_a_missing_one_is_named() {
        let disc = temp("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        std::fs::write(disc.join("maps").join("tiny.map"), b"invented bytes").unwrap();
        let maps = DiscMaps {
            disc: disc.clone(),
            open: RefCell::new(None),
        };
        assert_eq!(maps.read("tiny").unwrap(), b"invented bytes");
        let why = maps.read("absent").unwrap_err();
        assert!(why.contains("absent"), "{why}");
        let _ = std::fs::remove_dir_all(&disc);
    }
}
