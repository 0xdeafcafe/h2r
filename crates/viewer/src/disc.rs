//! Opens maps inside the user's own disc image (or a converted Halo 1 map
//! under the runtime's data root, or any map file by path) and reads raw
//! resources from the current map or the disc's shared caches. No game data
//! is copied anywhere.
use anyhow::{Context, Result, bail};
use map::{Map, ResourceFile};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

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
    /// Map names (without `.map`) in on-disc order.
    /// Only files directly under `/maps/`, the same set `open_map` accepts.
    pub fn map_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| map_name(&e.path))
            .map(str::to_owned)
            .collect()
    }
    fn region(&self, name: &str) -> Result<(u64, u64)> {
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
    /// Where a map's bytes are: on the disc by name; else a `.map` file by
    /// path; else a Halo 1 map `h2 import halo1` converted under the data
    /// root ([`converted`]). Converted maps read the disc's shared caches.
    fn locate(&self, name: &str) -> Result<(PathBuf, u64, u64)> {
        if let Ok((base, len)) = self.region(name) {
            return Ok((self.path.clone(), base, len));
        }
        let file = Path::new(name);
        let found = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")) && file.is_file() {
            Some(file.to_path_buf())
        } else {
            converted(&data_root(), name)
        };
        let path = found.with_context(|| {
            format!(
                "no /maps/{name}.map on disc, no such map file, and no converted map {name} under {}",
                data_root().display()
            )
        })?;
        let len = std::fs::metadata(&path)?.len();
        Ok((path, 0, len))
    }
    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        let (path, base, len) = self.locate(name)?;
        Ok(Map::from_region(BufReader::new(File::open(&path)?), base, len)?)
    }
    /// A raw-resource reader for a map opened with [`Disc::open_map`]: the
    /// local map plus whichever shared caches the disc carries.
    pub fn resources(&self, local: &str) -> Result<DiscResources> {
        let mut files = Vec::new();
        let (path, base, len) = self.locate(local)?;
        files.push((ResourceFile::Local, File::open(&path)?, (base, len)));
        for (file, name) in [
            (ResourceFile::MainMenu, "mainmenu"),
            (ResourceFile::Shared, "shared"),
            (ResourceFile::SinglePlayerShared, "single_player_shared"),
        ] {
            if let Ok(r) = self.region(name) {
                files.push((file, File::open(&self.path)?, r));
            }
        }
        Ok(DiscResources { files })
    }
}

/// The runtime's data root, where `h2 import halo1` writes: `$H2_DATA`,
/// else `h2boot-data` in the temporary directory (as `h2 play` defaults).
pub fn data_root() -> PathBuf {
    std::env::var_os("H2_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("h2boot-data"))
}

/// A converted Halo 1 map under `data` by the name the game knows it by
/// (docs/game/campaigns.md "Importing"): a campaign level at
/// `campaigns/halo1/<name>.map`, a multiplayer map at
/// `maps/<name>/h1/<name>.map` (the Halo 1 sandbox), else its Halo 2
/// sandbox's `h2/`. The level geometry is the same in both sandboxes.
pub fn converted(data: &Path, name: &str) -> Option<PathBuf> {
    let name = name.strip_suffix(".map").unwrap_or(name);
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let file = format!("{name}.map");
    [
        data.join("campaigns").join("halo1").join(&file),
        data.join("maps").join(name).join("h1").join(&file),
        data.join("maps").join(name).join("h2").join(&file),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Reads raw-resource byte ranges, bounded by each map file's extent.
pub struct DiscResources {
    files: Vec<(ResourceFile, File, (u64, u64))>,
}
impl tags::geometry::RawSource for DiscResources {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (file, offset) = (pointer.file, pointer.offset);
        let Some((_, handle, (base, len))) = self.files.iter_mut().find(|(f, _, _)| *f == file) else {
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
        handle
            .seek(SeekFrom::Start(*base + u64::from(offset)))
            .and_then(|_| handle.read_exact(&mut buf))
            .map_err(|e| tags::Error::Map(map::Error::from(e)))?;
        Ok(buf)
    }
}

/// `--disc`, else `H2_DISC`, else `disc/halo2.iso` in the repository.
pub fn disc_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Some(p) = std::env::var_os("H2_DISC") {
        return Ok(p.into());
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso");
    if p.exists() {
        return Ok(p);
    }
    bail!("no disc: pass --disc <iso> or set H2_DISC")
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
    #[test]
    fn converted_halo1_maps_are_found_under_the_data_root() {
        use super::converted;
        let root = std::env::temp_dir().join(format!("viewer-h1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let camp = root.join("campaigns/halo1");
        std::fs::create_dir_all(&camp).unwrap();
        std::fs::write(camp.join("x10.map"), b"x").unwrap();
        for s in ["h1", "h2"] {
            std::fs::create_dir_all(root.join("maps/h1_alpha").join(s)).unwrap();
            std::fs::write(root.join("maps/h1_alpha").join(s).join("h1_alpha.map"), b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("maps/h1_beta/h2")).unwrap();
        std::fs::write(root.join("maps/h1_beta/h2/h1_beta.map"), b"x").unwrap();
        assert_eq!(converted(&root, "x10"), Some(camp.join("x10.map")));
        assert_eq!(converted(&root, "x10.map"), Some(camp.join("x10.map")));
        assert_eq!(
            converted(&root, "h1_alpha"),
            Some(root.join("maps/h1_alpha/h1/h1_alpha.map")),
            "the Halo 1 sandbox's first"
        );
        assert_eq!(converted(&root, "h1_beta"), Some(root.join("maps/h1_beta/h2/h1_beta.map")));
        for bad in ["x20", "../x10", "", "maps/x"] {
            assert_eq!(converted(&root, bad), None, "{bad}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn map_names_come_only_from_the_maps_directory() {
        use super::map_name;
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
}
