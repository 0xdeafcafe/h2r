//! Reading raw resource bytes. A bitmap's LOD pointers carry a file selector
//! (`map::ResourceFile`); this module maps each selector to a byte region of a
//! host file (a standalone `.map`, or an entry inside an ISO) and reads ranges.

use crate::{Error, Result};
use map::ResourceFile;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// Largest single read, in bytes. Declared sizes come from untrusted tags.
pub const MAX_READ: u32 = 256 << 20;

/// A byte region of a host file: a whole `.map`, or an ISO entry.
#[derive(Clone, Debug)]
pub struct Region {
    /// Host file.
    pub path: PathBuf,
    /// Offset of the map's first byte in the host file.
    pub base: u64,
    /// Length of the map in bytes.
    pub length: u64,
}

/// The up-to-four files a map's raw pointers may select.
#[derive(Debug, Default)]
pub struct ResourceFiles {
    regions: [Option<Region>; 4],
    open: [Option<File>; 4],
}

fn slot(file: ResourceFile) -> usize {
    match file {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}

impl ResourceFiles {
    /// An empty set; add regions with [`ResourceFiles::set`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Supplies the region backing a selector, checking that `base + length`
    /// neither overflows nor exceeds the host file.
    pub fn set(&mut self, file: ResourceFile, region: Region) -> Result<()> {
        let end = region
            .base
            .checked_add(region.length)
            .ok_or_else(|| Error::Invalid("resource region wraps".into()))?;
        let host = std::fs::metadata(&region.path)?.len();
        if end > host {
            return Err(Error::Invalid(format!(
                "resource region {:#x}+{:#x} exceeds host file length {host:#x}",
                region.base, region.length
            )));
        }
        self.regions[slot(file)] = Some(region);
        self.open[slot(file)] = None;
        Ok(())
    }

    /// Length of a supplied file.
    pub fn length(&self, file: ResourceFile) -> Option<u64> {
        self.regions[slot(file)].as_ref().map(|r| r.length)
    }

    /// Reads `size` bytes at map-relative `offset` of the selected file.
    /// Sizes above [`MAX_READ`] are rejected before allocating.
    pub fn read(&mut self, file: ResourceFile, offset: u32, size: u32) -> Result<Vec<u8>> {
        let i = slot(file);
        let region = self.regions[i].as_ref().ok_or(Error::MissingResource(file))?;
        if size > MAX_READ {
            return Err(Error::Invalid(format!("read of {size} bytes exceeds cap {MAX_READ}")));
        }
        if u64::from(offset) + u64::from(size) > region.length {
            return Err(Error::Map(map::Error::Bounds {
                what: "resource",
                offset: offset.into(),
                size: size.into(),
                limit: region.length,
            }));
        }
        // base + length was checked in `set`, so this cannot wrap.
        let start = region.base + u64::from(offset);
        if self.open[i].is_none() {
            self.open[i] = Some(File::open(&region.path)?);
        }
        let f = self.open[i].as_mut().expect("just opened");
        f.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; size as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, bytes: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("assets-res-{name}-{}.bin", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn region_must_fit_host_file_and_not_wrap() {
        let p = temp("region", &[0u8; 64]);
        let mut f = ResourceFiles::new();
        assert!(f.set(ResourceFile::Local, Region { path: p.clone(), base: 0, length: 64 }).is_ok());
        assert!(f.set(ResourceFile::Shared, Region { path: p.clone(), base: 32, length: 64 }).is_err());
        assert!(
            f.set(ResourceFile::Shared, Region { path: p.clone(), base: u64::MAX - 1, length: 8 })
                .is_err()
        );
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn huge_declared_sizes_are_rejected_before_allocation() {
        let p = temp("huge", &[0u8; 16]);
        let mut f = ResourceFiles::new();
        f.set(ResourceFile::Local, Region { path: p.clone(), base: 0, length: 16 }).unwrap();
        assert!(matches!(f.read(ResourceFile::Local, 0, u32::MAX), Err(Error::Invalid(_))));
        assert!(matches!(f.read(ResourceFile::Local, 8, 16), Err(Error::Map(_))));
        assert_eq!(f.read(ResourceFile::Local, 4, 4).unwrap().len(), 4);
        std::fs::remove_file(p).unwrap();
    }
}
