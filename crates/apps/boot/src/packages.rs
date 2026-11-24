//! The game's content packages (docs/editor/studio-redesign.md, "How the
//! game consumes packages"): the baked ones (`content/<id>/`, baked by
//! `build.rs`), then `<data>/packages/<id>/`, ordered and switched by
//! `<data>/packages/set.toml`, with a package whose game feature is off left
//! out whole (`workspace::GameSet`).
//!
//! The per-map items reach the game through one `vfs::Rewrite`,
//! `workspace::PackageRewrite`, cached under `<data>/packages/cache/`;
//! `packages.list` on the agent lists the set and each map read.
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent::{Access, Agent};
use agent_proto::method;
use vfs::Backend as _;
use workspace::{GameSet, KitInputs, PackageRewrite, Served};

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
/// which game features are on. `kits` is what the set's `kit` items run on:
/// the campaigns' level pictures and the button icon sets. Every problem is
/// a line on stderr.
pub fn at_boot(
    data_root: &Path,
    feature_on: &dyn Fn(&str) -> bool,
    kits: KitInputs,
) -> Packages {
    let (baked, mut problems) = workspace::baked(baked::BAKED);
    let mut set = GameSet::read(baked, data_root, feature_on);
    problems.append(&mut set.problems);
    set.problems = problems;
    for p in &set.problems {
        eprintln!("packages: {p}");
    }
    // The pogo stick's weapons are in the maps the `pogo` package built:
    // read them from what the game is handed, for the loadout.
    // The superbounce world labels' HUD font is read from the fonts as the
    // game is handed them, after the `glyphs` kit.
    let rewrite = PackageRewrite::new(set.loaded())
        .with_cache(data_root)
        .with_kits(kits)
        .observing_fonts(game::superbounce_font::Capture::keep)
        .observed(|stem, bytes| match bytes {
            Some(bytes) => pogo_kit::served::publish(stem, bytes),
            None => pogo_kit::served::clear(stem),
        });
    let served = rewrite.served();
    let rewrite = (!rewrite.is_empty()).then_some(rewrite);
    // The built-in list entries are the `variant` items of the packages
    // that are on: a package off takes its entries out of the game's list.
    // This is before the list is first read (`community_variants::install`).
    game::community_variants::install_registry(workspace::script_registry(&set.loaded()));
    let set = Arc::new(set);
    let _ = SET.set(Arc::clone(&set));
    Packages {
        set,
        served,
        rewrite,
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
    let fresh = stage_kit::fresh::StageFresh { bases: &bases };
    let out = workspace::prepare(&content::store::maps_dir(data_root), loaded, &bases, &fresh);
    for f in &out.failed {
        eprintln!("packages: {} item {}: {}", f.package, f.item, f.why);
    }
    for m in &out.written {
        eprintln!("packages: built the map {} ({})", m.dir, m.package);
    }
    out
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
