//! The Halo 2 Multiplayer Map Pack: its maps are ordinary Halo 2 cache maps
//! (they lean on the base game's `shared` and `mainmenu` caches) but sit in
//! content packages, `/demos/dlc/<8 hex>/<name>.map`, not under `/maps/`.
//! The layout reader is `xiso::mappack`, the one `h2 import-mappack`'s
//! disc layout is described by (docs/game/map-pack.md section 1).
//!
//! A map pack is an XISO, or a folder holding the same tree (an extracted
//! disc, or just its `dlc` directory). Read-only; nothing is copied.
//! Whether a path is one is decided by its contents, never its name.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::Map;

/// The environment variable naming the map pack's path (as `H2_DISC` does
/// the Halo 2 disc's). `H2_MAPPACK` itself is `h2 play`'s on/off switch
/// (`0`, `false`, `no`, `off` skip the map pack), and switches this
/// auto-discovery off too.
pub const ENV: &str = "H2_MAPPACK_ISO";

/// The file name the docs tell people to give the disc.
pub const FILE_NAME: &str = "mappack.iso";

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackedMap {
    name: String,
    /// The file holding the map: the ISO, or the loose `.map`.
    file: PathBuf,
    /// `(offset, length)` inside an ISO; `None` for a loose file.
    region: Option<(u64, u64)>,
}

/// The maps of an opened map pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapPack {
    path: PathBuf,
    maps: Vec<PackedMap>,
}

impl MapPack {
    /// Opens `path` (an XISO or a folder) as a map pack. Fails, naming why,
    /// for anything whose contents are not the map pack's.
    pub fn open(path: &Path) -> Result<Self> {
        let maps = if path.is_dir() {
            folder_maps(path)
        } else {
            iso_maps(path)?
        };
        if maps.is_empty() {
            bail!(
                "{} is not the Halo 2 Multiplayer Map Pack: no content packages with maps",
                path.display()
            );
        }
        Ok(Self {
            path: path.to_owned(),
            maps,
        })
    }

    /// Whether `path`'s contents are the map pack's.
    pub fn detect(path: &Path) -> bool {
        Self::open(path).is_ok()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the pack is a folder rather than a disc image.
    pub fn is_folder(&self) -> bool {
        self.path.is_dir()
    }

    /// Map names (without `.map`), sorted case-insensitively.
    pub fn map_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.maps.iter().map(|m| m.name.clone()).collect();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    fn find(&self, name: &str) -> Option<&PackedMap> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        self.maps
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(want))
    }

    /// Where map `name`'s bytes are: the file to read and, in an ISO, the
    /// `(offset, length)` range inside it (as `MapSource::location`).
    pub fn location(&self, name: &str) -> Result<(PathBuf, Option<(u64, u64)>)> {
        let m = self
            .find(name)
            .with_context(|| format!("no {name}.map in the map pack"))?;
        Ok((m.file.clone(), m.region))
    }

    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        let (path, region) = self.location(name)?;
        let reader = BufReader::new(File::open(&path)?);
        Ok(match region {
            Some((base, len)) => Map::from_region(reader, base, len)?,
            None => {
                let len = std::fs::metadata(&path)?.len();
                Map::from_region(reader, 0, len)?
            }
        })
    }
}

fn iso_maps(path: &Path) -> Result<Vec<PackedMap>> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path)?))
        .with_context(|| format!("open {}", path.display()))?;
    let entries = iso.entries()?;
    let partition = iso.partition_offset();
    Ok(xiso::mappack::maps(&entries)
        .into_iter()
        .map(|m| PackedMap {
            name: m.name,
            file: path.to_owned(),
            region: Some((
                partition + u64::from(m.entry.sector) * xiso::SECTOR,
                u64::from(m.entry.size),
            )),
        })
        .collect())
}

/// The same tree on the host: `<root>/demos/dlc/<pkg>/`, `<root>/dlc/<pkg>/`
/// or `<root>/<pkg>/`, the first that holds packages with maps.
fn folder_maps(root: &Path) -> Vec<PackedMap> {
    for base in [root.join("demos").join("dlc"), root.join("dlc"), root.to_owned()] {
        let found = packages_in(&base);
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn packages_in(base: &Path) -> Vec<PackedMap> {
    let Ok(rd) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(xiso::mappack::is_package_dir)
        })
        .collect();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs {
        let files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        let named = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
        if !files.iter().filter_map(named).any(|n| {
            n.eq_ignore_ascii_case(xiso::mappack::CONTENT_META)
        }) {
            continue;
        }
        let mut maps: Vec<PackedMap> = files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")))
            .filter_map(|p| {
                Some(PackedMap {
                    name: p.file_stem()?.to_str()?.to_owned(),
                    file: p.clone(),
                    region: None,
                })
            })
            .collect();
        maps.sort_by_key(|m| m.name.to_ascii_lowercase());
        out.extend(maps);
    }
    out
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// The map pack beside `disc`: `mappack.iso` (any case), else the first
/// other `.iso` there (by name) whose contents are the map pack's. A folder
/// `disc` is looked in, then its parent. This is where `h2 play` looks to
/// install the map pack (`mappack_cmd::discover`), so a pack found here is
/// one the game installs by itself.
pub fn beside(disc: &Path) -> Option<PathBuf> {
    let parent = match disc.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dirs: Vec<&Path> = if disc.is_dir() {
        vec![disc, parent]
    } else {
        vec![parent]
    };
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut isos: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
            .filter(|p| p.is_file() && !same_file(p, disc))
            .collect();
        isos.sort();
        let named = |p: &PathBuf| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(FILE_NAME))
        };
        if let Some(i) = isos.iter().position(named) {
            return Some(isos.swap_remove(i));
        }
        if let Some(p) = isos.into_iter().find(|p| MapPack::detect(p)) {
            return Some(p);
        }
    }
    None
}

/// The map pack to open at start: `explicit` (`--mappack`), else
/// `$H2_MAPPACK_ISO`, else the map pack beside the Halo 2 disc that was
/// opened (`mappack.iso`, so it works outside the repository), else
/// `disc/mappack.iso` in the repository. With `H2_MAPPACK` set to `0`,
/// `false`, `no` or `off` only the first two apply.
pub fn default_source(explicit: Option<PathBuf>, disc: Option<&Path>) -> Option<PathBuf> {
    let off = std::env::var("H2_MAPPACK").is_ok_and(|v| {
        ["0", "false", "no", "off"].contains(&v.trim().to_ascii_lowercase().as_str())
    });
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../disc")
        .join(FILE_NAME);
    find(
        explicit,
        std::env::var_os(ENV).filter(|v| !v.is_empty()).map(PathBuf::from),
        disc,
        Some(repo).filter(|_| !off),
        off,
    )
}

/// [`default_source`] with its inputs given.
fn find(
    explicit: Option<PathBuf>,
    env: Option<PathBuf>,
    disc: Option<&Path>,
    repo: Option<PathBuf>,
    beside_off: bool,
) -> Option<PathBuf> {
    if let Some(p) = explicit.or(env) {
        return Some(p);
    }
    if beside_off {
        return None;
    }
    disc.and_then(beside)
        .or_else(|| repo.filter(|p| p.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("h2-mappack-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
        fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A folder shaped like the disc: two packages, three maps.
    fn pack(t: &Tmp, under: &str) -> PathBuf {
        for (rel, bytes) in [
            ("00000001/ContentMeta.xbx", &b"m"[..]),
            ("00000001/Warlock.map", b"w"),
            ("00000001/containment.map", b"c"),
            ("00000001/patch.lvl", b"p"),
            ("00000002/contentmeta.xbx", b"m"),
            ("00000002/turf.map", b"t"),
        ] {
            t.file(&format!("{under}{rel}"), bytes);
        }
        t.0.clone()
    }

    #[test]
    fn a_folder_with_the_discs_packages_is_a_map_pack() {
        for under in ["demos/dlc/", "dlc/", ""] {
            let t = Tmp::new(&format!("folder{}", under.len()));
            let root = pack(&t, under);
            assert!(MapPack::detect(&root), "{under:?}");
            let p = MapPack::open(&root).unwrap();
            assert_eq!(p.map_names(), ["containment", "turf", "Warlock"], "{under:?}");
            assert!(p.is_folder());
            let (file, region) = p.location("WARLOCK").unwrap();
            assert!(file.ends_with("00000001/Warlock.map"));
            assert_eq!(region, None);
            assert!(p.location("lockout").is_err());
        }
    }

    #[test]
    fn contents_decide_not_the_name() {
        // Maps with no content packages: a plain maps folder, not a pack.
        let t = Tmp::new("plain");
        t.file("a.map", b"x");
        t.file("00000001/a.map", b"x");
        assert!(!MapPack::detect(&t.0), "no ContentMeta.xbx");
        // A file named mappack.iso that is not a disc.
        let iso = t.file("mappack.iso", b"not a disc");
        assert!(!MapPack::detect(&iso));
        assert!(MapPack::open(&iso).is_err());
        // A map pack folder under any name.
        let t2 = Tmp::new("renamed");
        t2.file("whatever/demos/dlc/00000009/ContentMeta.xbx", b"m");
        t2.file("whatever/demos/dlc/00000009/gemini.map", b"g");
        assert!(MapPack::detect(&t2.0.join("whatever")));
    }

    #[test]
    fn discovery_prefers_explicit_then_env_then_beside_then_repo() {
        let t = Tmp::new("find");
        let disc = t.file("game/halo2.iso", b"d");
        let beside = t.file("game/MapPack.ISO", b"p");
        let repo = t.file("repo/mappack.iso", b"r");
        let other = PathBuf::from("/elsewhere/pack.iso");
        let missing = t.0.join("no.iso");
        let find = |e: Option<&Path>, v: Option<&Path>, repo: Option<&Path>| {
            find(
                e.map(Path::to_owned),
                v.map(Path::to_owned),
                Some(disc.as_path()),
                repo.map(Path::to_owned),
                false,
            )
        };
        let (other_p, repo_p) = (other.as_path(), repo.as_path());
        assert_eq!(find(Some(other_p), Some(repo_p), Some(repo_p)), Some(other.clone()));
        assert_eq!(find(None, Some(other_p), Some(repo_p)), Some(other.clone()));
        assert_eq!(find(None, None, Some(repo_p)), Some(beside.clone()));
        std::fs::remove_file(&beside).unwrap();
        assert_eq!(find(None, None, Some(repo_p)), Some(repo.clone()));
        assert_eq!(find(None, None, Some(missing.as_path())), None);
    }

    #[test]
    fn discovery_beside_the_disc_skips_the_disc_and_other_games() {
        let t = Tmp::new("beside");
        let disc = t.file("halo2.iso", b"d");
        t.file("halo1.iso", b"not a map pack");
        assert_eq!(beside(&disc), None);
        let pack = t.file("mappack.iso", b"p");
        assert_eq!(beside(&disc), Some(pack));
        // Switched off: only an explicit path counts.
        assert_eq!(
            find(None, None, Some(disc.as_path()), Some(t.0.join("mappack.iso")), true),
            None
        );
    }

    /// The real disc, read only: nine maps, none of them the base game's.
    /// `H2_MAPPACK_ISO=disc/mappack.iso cargo test -p editor-core real_map_pack -- --ignored`
    #[test]
    #[ignore = "needs the map pack ISO ($H2_MAPPACK_ISO, else disc/mappack.iso)"]
    fn real_map_pack_lists_its_nine_maps() {
        let path = std::env::var_os(ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc").join(FILE_NAME));
        assert!(MapPack::detect(&path));
        let p = MapPack::open(&path).unwrap();
        let names = p.map_names();
        assert_eq!(names.len(), 9, "{names:?}");
        for n in ["backwash", "containment", "dune", "gemini", "turf", "warlock"] {
            assert!(p.has(n), "{n} in {names:?}");
        }
        for n in &names {
            let m = p.open_map(n).unwrap();
            assert!(m.scenario().is_some(), "{n} has a scenario");
        }
    }
}
