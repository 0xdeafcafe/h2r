//! Opens maps inside the user's own disc image and reads raw resources from
//! the current map or the shared caches. No game data is copied anywhere.
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
    pub fn open_map(&self, name: &str) -> Result<Map<BufReader<File>>> {
        let (base, len) = self.region(name)?;
        Ok(Map::from_region(
            BufReader::new(File::open(&self.path)?),
            base,
            len,
        )?)
    }
    /// A raw-resource reader for a map opened from this disc: the local map
    /// plus whichever shared caches the disc carries.
    pub fn resources(&self, local: &str) -> Result<DiscResources> {
        let mut files = Vec::new();
        for (file, name) in [
            (ResourceFile::Local, local),
            (ResourceFile::MainMenu, "mainmenu"),
            (ResourceFile::Shared, "shared"),
            (ResourceFile::SinglePlayerShared, "single_player_shared"),
        ] {
            if let Ok(r) = self.region(name) {
                files.push((file, r));
            }
        }
        Ok(DiscResources {
            file: File::open(&self.path)?,
            files,
        })
    }
}

/// Reads raw-resource byte ranges, bounded by each map file's extent.
pub struct DiscResources {
    file: File,
    files: Vec<(ResourceFile, (u64, u64))>,
}
impl tags::geometry::RawSource for DiscResources {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (file, offset) = (pointer.file, pointer.offset);
        let Some(&(_, (base, len))) = self.files.iter().find(|(f, _)| *f == file) else {
            return Err(tags::Error::MissingResource(file));
        };
        if u64::from(offset) + u64::from(size) > len {
            return Err(tags::Error::ResourceBounds {
                file,
                offset,
                size,
                length: len,
            });
        }
        let mut buf = vec![0; size as usize];
        self.file
            .seek(SeekFrom::Start(base + u64::from(offset)))
            .and_then(|_| self.file.read_exact(&mut buf))
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
