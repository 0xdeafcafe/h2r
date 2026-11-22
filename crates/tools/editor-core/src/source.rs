//! Where maps come from: the user's own disc image (an XISO,
//! read here), a loose `.map`
//! file, or a folder of `.map` files (an extracted `maps` directory).
//! Read-only; nothing is copied anywhere.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{Header, Map, ResourceFile};

use crate::map_groups::MapGroup;

/// A disc image's `/maps/*.map` files.
pub struct Disc {
    path: PathBuf,
    partition: u64,
    entries: Vec<xiso::Entry>,
}

impl Disc {
    pub fn open(path: &Path) -> Result<Self> {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(path)?))
            .with_context(|| format!("open {}", path.display()))?;
        let entries = iso.entries()?;
        Ok(Self {
            path: path.to_owned(),
            partition: iso.partition_offset(),
            entries,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Map names (without `.map`) in on-disc order: only files directly
    /// under `/maps/`, the set [`Disc::open_map`] accepts.
    pub fn map_names(&self) -> Vec<String> {
        self.map_regions().map(|(name, _)| name.to_owned()).collect()
    }

    /// The byte range `(offset, length)` of `/maps/<name>.map` in the image.
    pub fn region(&self, name: &str) -> Result<(u64, u64)> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        self.map_regions()
            .find(|(name, _)| name.eq_ignore_ascii_case(want))
            .map(|(_, region)| region)
            .with_context(|| format!("no /maps/{want}.map on disc"))
    }

    fn map_regions(&self) -> impl Iterator<Item = (&str, (u64, u64))> {
        self.entries.iter().filter(|e| !e.is_dir).filter_map(|e| {
            let name = map_name(&e.path)?;
            let base = self.partition + u64::from(e.sector) * xiso::SECTOR;
            Some((name, (base, u64::from(e.size))))
        })
    }

    /// A file in the disc's root directory (`default.xbe`), at most `max`
    /// bytes. Names match without regard to case.
    pub fn read_root_file(&self, name: &str, max: u64) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|e| !e.is_dir && e.path.trim_start_matches('/').eq_ignore_ascii_case(name))
            .with_context(|| format!("no {name} on the disc"))?;
        if u64::from(entry.size) > max {
            bail!("{name} is larger than {max} bytes");
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(
            self.partition + u64::from(entry.sector) * xiso::SECTOR,
        ))?;
        let mut bytes = vec![0; entry.size as usize];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        let (base, len) = self.region(name)?;
        Ok(Map::from_region(
            BufReader::new(File::open(&self.path)?),
            base,
            len,
        )?)
    }
}

/// The opened map source: a disc image, one loose `.map` (its siblings
/// in the same directory serve as the shared caches), or a folder of maps.
pub enum MapSource {
    Disc(Disc),
    Loose(PathBuf),
    /// Every `*.map` directly in this directory.
    Folder(PathBuf),
}

impl MapSource {
    /// Opens `path`: a directory as a folder of maps, a `.map` file as a
    /// loose map, anything else as a disc image.
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            if folder_maps(path).is_empty() {
                bail!("no .map files in {}", path.display());
            }
            return Ok(MapSource::Folder(path.to_owned()));
        }
        let is_map = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("map"));
        if is_map {
            if !path.is_file() {
                bail!("{} is not a file", path.display());
            }
            // Fail now rather than on first use.
            Map::open(path).with_context(|| format!("open {}", path.display()))?;
            Ok(MapSource::Loose(path.to_owned()))
        } else {
            Ok(MapSource::Disc(Disc::open(path)?))
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            MapSource::Disc(d) => d.path(),
            MapSource::Loose(p) | MapSource::Folder(p) => p,
        }
    }

    /// Every map stem in the source. Disc maps follow on-disc order, and
    /// folder maps are sorted by stem.
    pub fn map_names(&self) -> Vec<String> {
        match self {
            MapSource::Disc(d) => d.map_names(),
            MapSource::Folder(dir) => folder_maps(dir),
            MapSource::Loose(p) => p
                .file_stem()
                .map(|s| vec![s.to_string_lossy().into_owned()])
                .unwrap_or_default(),
        }
    }

    /// Sections keyed by map stem, read from the cache headers.
    pub fn map_groups(&self) -> HashMap<String, MapGroup> {
        match self {
            Self::Disc(disc) => {
                let mut file = File::open(disc.path()).ok();
                disc.map_regions()
                    .map(|(name, (base, length))| {
                        let header = file.as_mut().and_then(|file| {
                            Header::read_region(file, base, length).ok()
                        });
                        (name.to_owned(), map_group(name, header))
                    })
                    .collect()
            }
            Self::Folder(dir) => folder_entries(dir)
                .into_iter()
                .map(|(name, path)| {
                    let group = map_group(&name, file_header(&path).ok());
                    (name, group)
                })
                .collect(),
            Self::Loose(path) => self.map_names()
                .into_iter()
                .map(|name| {
                    let group = map_group(&name, file_header(path).ok());
                    (name, group)
                })
                .collect(),
        }
    }

    /// Opens a map by name: from the disc, or (loose) the file itself or a
    /// sibling `<name>.map` in its directory.
    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        match self {
            MapSource::Disc(d) => d.open_map(name),
            MapSource::Loose(_) | MapSource::Folder(_) => {
                let p = self
                    .loose_file(name)
                    .with_context(|| format!("no {name}.map"))?;
                let len = std::fs::metadata(&p)?.len();
                Ok(Map::from_region(BufReader::new(File::open(&p)?), 0, len)?)
            }
        }
    }

    /// Where map `name`'s bytes are: the file to read and, for a disc, the
    /// `(offset, length)` range inside it. For tools that open maps
    /// through their own readers.
    pub fn location(&self, name: &str) -> Result<(PathBuf, Option<(u64, u64)>)> {
        match self {
            MapSource::Disc(d) => Ok((d.path().to_owned(), Some(d.region(name)?))),
            MapSource::Loose(_) | MapSource::Folder(_) => Ok((
                self.loose_file(name)
                    .with_context(|| format!("no {name}.map"))?,
                None,
            )),
        }
    }

    /// The game's executable, `default.xbe`, for tools that read the
    /// game's own tables from it (the Scripts tool,
    /// docs/editor/rich-editors.md, "Scripts"): the disc image's root;
    /// for a folder of maps or a loose map, the folder it is in, then the
    /// one above (an extracted disc's `maps`). Read when asked, never
    /// copied. The error names every place it looked. Returns where it
    /// was read and its bytes.
    pub fn executable(&self) -> Result<(String, Vec<u8>)> {
        let dir = match self {
            MapSource::Disc(d) => {
                let mut iso = xiso::Xiso::open(BufReader::new(File::open(d.path())?))
                    .with_context(|| format!("open {}", d.path().display()))?;
                let entry = iso
                    .find("default.xbe")
                    .with_context(|| format!("no default.xbe in {}", d.path().display()))?;
                return Ok((format!("{} (default.xbe)", d.path().display()), iso.read(&entry)?));
            }
            MapSource::Folder(dir) => Some(dir.as_path()),
            MapSource::Loose(p) => p.parent(),
        };
        let dirs: Vec<&Path> = dir.into_iter().chain(dir.and_then(Path::parent)).collect();
        for d in &dirs {
            let found = std::fs::read_dir(d).ok().and_then(|entries| {
                entries.flatten().map(|e| e.path()).find(|p| {
                    p.is_file()
                        && p.file_name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("default.xbe"))
                })
            });
            if let Some(p) = found {
                let bytes = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
                return Ok((p.display().to_string(), bytes));
            }
        }
        let places: Vec<String> = dirs
            .iter()
            .map(|d| d.join("default.xbe").display().to_string())
            .collect();
        bail!("no default.xbe at {}", places.join(" or "))
    }

    fn loose_file(&self, name: &str) -> Option<PathBuf> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        let p = match self {
            MapSource::Disc(_) => return None,
            MapSource::Folder(dir) => {
                return folder_entries(dir)
                    .into_iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(want))
                    .map(|(_, p)| p);
            }
            MapSource::Loose(p) => p,
        };
        if p.file_stem()
            .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(want))
        {
            return Some(p.clone());
        }
        let sibling = p.parent()?.join(format!("{want}.map"));
        sibling.is_file().then_some(sibling)
    }

    /// A raw-resource reader for map `local`: the map itself plus whichever
    /// shared caches (`mainmenu`, `shared`, `single_player_shared`) the
    /// source carries.
    pub fn resources(&self, local: &str) -> Result<Resources> {
        let mut files = Vec::new();
        for (file, name) in [
            (ResourceFile::Local, local),
            (ResourceFile::MainMenu, "mainmenu"),
            (ResourceFile::Shared, "shared"),
            (ResourceFile::SinglePlayerShared, "single_player_shared"),
        ] {
            match self {
                MapSource::Disc(d) => {
                    if let Ok((base, len)) = d.region(name) {
                        files.push((file, d.path().to_owned(), base, len));
                    }
                }
                MapSource::Loose(_) | MapSource::Folder(_) => {
                    if let Some(p) = self.loose_file(name) {
                        let len = std::fs::metadata(&p)?.len();
                        files.push((file, p, 0, len));
                    }
                }
            }
        }
        Ok(Resources { files })
    }
}

fn file_header(path: &Path) -> Result<Header> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    Ok(Header::read_region(&mut file, 0, length)?)
}

fn map_group(name: &str, header: Option<Header>) -> MapGroup {
    MapGroup::classify(name, header.map_or(MapGroup::Other, |h| h.cache_type.into()))
}

/// Reads raw-resource byte ranges, bounded by each map file's extent.
pub struct Resources {
    files: Vec<(ResourceFile, PathBuf, u64, u64)>,
}

impl tags::geometry::RawSource for Resources {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (file, offset) = (pointer.file, pointer.offset);
        let Some((_, path, base, len)) = self.files.iter().find(|(f, ..)| *f == file) else {
            return Err(tags::Error::MissingResource(file));
        };
        if u64::from(offset) + u64::from(size) > *len {
            return Err(tags::Error::ResourceBounds {
                file,
                offset,
                size,
                length: *len,
            });
        }
        let mut buf = vec![0; size as usize];
        File::open(path)
            .and_then(|mut f| {
                f.seek(SeekFrom::Start(base + u64::from(offset)))?;
                f.read_exact(&mut buf)
            })
            .map_err(|e| tags::Error::Map(map::Error::from(e)))?;
        Ok(buf)
    }
}

/// The disc to open at start: `explicit`, else `H2_DISC`, else
/// `disc/halo2.iso` in the repository when it exists.
pub fn default_disc(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(p) = std::env::var_os("H2_DISC") {
        return Some(p.into());
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso");
    p.exists().then_some(p)
}

/// The stems of the `*.map` files directly in `dir`, sorted
/// case-insensitively.
pub fn folder_maps(dir: &Path) -> Vec<String> {
    folder_entries(dir).into_iter().map(|(n, _)| n).collect()
}

/// `(stem, path)` of each `*.map` file directly in `dir`, sorted by stem.
pub(super) fn folder_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_owned(), p)))
        .collect();
    out.sort_by_key(|(n, _)| n.to_ascii_lowercase());
    out
}

/// `/maps/<name>.map` (case-insensitive, no deeper directories) -> `<name>`.
fn map_name(path: &str) -> Option<&str> {
    let rest = path
        .get(..6)?
        .eq_ignore_ascii_case("/maps/")
        .then(|| &path[6..])?;
    let cut = rest.len().checked_sub(4)?;
    if !rest.get(cut..)?.eq_ignore_ascii_case(".map") || rest.contains('/') || cut == 0 {
        return None;
    }
    Some(&rest[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_names_come_only_from_the_maps_directory() {
        assert_eq!(map_name("/maps/mainmenu.map"), Some("mainmenu"));
        assert_eq!(map_name("/MAPS/Shared.MAP"), Some("Shared"));
        for bad in [
            "/other/x.map",
            "/maps/sub/x.map",
            "/maps/.map",
            "/maps/x.mapx",
            "/x.map",
            "/maps/",
        ] {
            assert_eq!(map_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_loose_map_finds_its_siblings() {
        let dir = std::env::temp_dir().join(format!("h2-editor-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("ascension.map");
        std::fs::write(&a, b"not a real map").unwrap();
        std::fs::write(dir.join("shared.map"), b"x").unwrap();
        let src = MapSource::Loose(a.clone());
        assert_eq!(src.map_names(), vec!["ascension".to_string()]);
        assert_eq!(src.loose_file("ASCENSION"), Some(a.clone()));
        assert_eq!(src.loose_file("shared.map"), Some(dir.join("shared.map")));
        assert_eq!(src.loose_file("mainmenu"), None);
        // Not a cache file: open refuses it up front.
        assert!(MapSource::open(&a).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_lists_its_maps_sorted() {
        let dir = std::env::temp_dir().join(format!("h2-editor-folder-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["b.map", "A.MAP", "notes.txt", "sub/c.map"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let src = MapSource::open(&dir).unwrap();
        assert!(matches!(src, MapSource::Folder(_)));
        assert_eq!(src.map_names(), vec!["A".to_string(), "b".to_string()]);
        assert_eq!(src.loose_file("a"), Some(dir.join("A.MAP")));
        assert_eq!(src.location("b").unwrap(), (dir.join("b.map"), None));
        assert!(
            src.location("c").is_err(),
            "only maps directly in the folder"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let empty = std::env::temp_dir().join(format!("h2-editor-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(MapSource::open(&empty).is_err());
        std::fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn the_executable_is_beside_the_maps_or_one_folder_up() {
        let root = std::env::temp_dir().join(format!("h2-editor-xbe-{}", std::process::id()));
        let maps = root.join("maps");
        std::fs::create_dir_all(&maps).unwrap();
        let map = maps.join("example.map");
        std::fs::write(&map, b"x").unwrap();
        let src = MapSource::Loose(map.clone());
        let e = src.executable().unwrap_err().to_string();
        assert!(e.contains(&maps.join("default.xbe").display().to_string()), "{e}");
        assert!(e.contains(&root.join("default.xbe").display().to_string()), "{e}");
        // An extracted disc: the executable is above `maps`, any case.
        std::fs::write(root.join("DEFAULT.XBE"), b"invented").unwrap();
        assert_eq!(src.executable().unwrap().1, b"invented");
        assert_eq!(MapSource::Folder(maps.clone()).executable().unwrap().1, b"invented");
        // One beside the maps wins.
        std::fs::write(maps.join("default.xbe"), b"beside").unwrap();
        assert_eq!(src.executable().unwrap().1, b"beside");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn explicit_disc_wins() {
        assert_eq!(
            default_disc(Some("x.iso".into())),
            Some(PathBuf::from("x.iso"))
        );
    }
}
