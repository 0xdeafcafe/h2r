//! Where maps come from: the user's own disc image (an XISO, as
//! `crates/viewer/src/disc.rs` reads it; the logic is copied here so the
//! editor does not depend on the viewer binary crate) or a loose `.map`
//! file. Read-only; nothing is copied anywhere.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{Map, ResourceFile};

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
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| map_name(&e.path))
            .map(str::to_owned)
            .collect()
    }

    /// The byte range `(offset, length)` of `/maps/<name>.map` in the image.
    pub fn region(&self, name: &str) -> Result<(u64, u64)> {
        let want = name.strip_suffix(".map").unwrap_or(name);
        let e = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .find(|e| map_name(&e.path).is_some_and(|n| n.eq_ignore_ascii_case(want)))
            .with_context(|| format!("no /maps/{want}.map on disc"))?;
        Ok((
            self.partition + u64::from(e.sector) * xiso::SECTOR,
            u64::from(e.size),
        ))
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

/// The opened map source: a disc image, or one loose `.map` (its siblings
/// in the same directory serve as the shared caches).
pub enum MapSource {
    Disc(Disc),
    Loose(PathBuf),
}

impl MapSource {
    /// Opens `path`: a `.map` file as a loose map, anything else as a disc
    /// image.
    pub fn open(path: &Path) -> Result<Self> {
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
            MapSource::Loose(p) => p,
        }
    }

    /// What the sidebar lists: every map on a disc, or the loose map's stem.
    pub fn map_names(&self) -> Vec<String> {
        match self {
            MapSource::Disc(d) => d.map_names(),
            MapSource::Loose(p) => p
                .file_stem()
                .map(|s| vec![s.to_string_lossy().into_owned()])
                .unwrap_or_default(),
        }
    }

    /// Opens a map by name: from the disc, or (loose) the file itself or a
    /// sibling `<name>.map` in its directory.
    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        match self {
            MapSource::Disc(d) => d.open_map(name),
            MapSource::Loose(_) => {
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
            MapSource::Loose(_) => Ok((
                self.loose_file(name)
                    .with_context(|| format!("no {name}.map"))?,
                None,
            )),
        }
    }

    fn loose_file(&self, name: &str) -> Option<PathBuf> {
        let MapSource::Loose(p) = self else {
            return None;
        };
        let want = name.strip_suffix(".map").unwrap_or(name);
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
                MapSource::Loose(_) => {
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
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso");
    p.exists().then_some(p)
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
    fn explicit_disc_wins() {
        assert_eq!(
            default_disc(Some("x.iso".into())),
            Some(PathBuf::from("x.iso"))
        );
    }
}
