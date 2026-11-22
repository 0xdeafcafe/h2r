//! Reading raw resource bytes. A bitmap's LOD pointers carry a file selector
//! (`map::ResourceFile`); this module maps each selector to a byte region of a
//! host file (a standalone `.map`, or an entry inside an ISO), or to bytes in
//! memory (a map built or resolved in memory), and reads ranges. A selector
//! can carry a [`ResourceOverlay`]: live edits' replacements and appended
//! extents, served over the file's bytes (docs/editor/realtime-edits.md,
//! "The edit overlay").

use crate::{Error, Result};
use map::ResourceFile;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

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

/// Bytes served over a selector's file: replacements anywhere in it, and
/// extents appended past its end.
pub trait ResourceOverlay: Send + Sync {
    /// Writes the replacements over `buf`, which holds the file's bytes
    /// from `offset` (zero past the file's end).
    fn apply(&self, offset: u64, buf: &mut [u8]);
    /// The end of the last appended extent; 0 with none.
    fn end(&self) -> u64;
}

/// The up-to-four files a map's raw pointers may select.
#[derive(Default)]
pub struct ResourceFiles {
    regions: [Option<Region>; 4],
    open: [Option<File>; 4],
    /// Selectors backed by bytes in memory instead ([`Self::set_bytes`]).
    memory: [Option<Arc<[u8]>>; 4],
    /// Served over each selector's bytes ([`Self::set_overlay`]).
    overlays: [Option<Arc<dyn ResourceOverlay>>; 4],
}

impl std::fmt::Debug for ResourceFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceFiles")
            .field("regions", &self.regions)
            .field("memory", &self.memory.each_ref().map(|m| m.as_ref().map(|m| m.len())))
            .field("overlays", &self.overlays.each_ref().map(Option::is_some))
            .finish()
    }
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
        self.set_region(file, region, false)
    }

    /// Opens an editor generation while the caller still holds its pathname lease.
    pub fn set_pinned(&mut self, file: ResourceFile, region: Region) -> Result<()> {
        self.set_region(file, region, true)
    }

    fn set_region(&mut self, file: ResourceFile, region: Region, pin: bool) -> Result<()> {
        let end = region
            .base
            .checked_add(region.length)
            .ok_or_else(|| Error::Invalid("resource region wraps".into()))?;
        let (host, reader) = if pin {
            let reader = File::open(&region.path)?;
            (reader.metadata()?.len(), Some(reader))
        } else {
            (std::fs::metadata(&region.path)?.len(), None)
        };
        if end > host {
            return Err(Error::Invalid(format!(
                "resource region {:#x}+{:#x} exceeds host file length {host:#x}",
                region.base, region.length
            )));
        }
        self.regions[slot(file)] = Some(region);
        self.open[slot(file)] = reader;
        self.memory[slot(file)] = None;
        Ok(())
    }

    /// Supplies a selector's bytes in memory: a map that exists only in
    /// memory (a package resolved over its base, or a new map), whose raw
    /// pointers are offsets into those bytes and not into any file on disk.
    pub fn set_bytes(&mut self, file: ResourceFile, bytes: Arc<[u8]>) {
        self.regions[slot(file)] = None;
        self.open[slot(file)] = None;
        self.memory[slot(file)] = Some(bytes);
    }

    /// Serves `overlay` over the selector's bytes, for every later read.
    /// The selector keeps it when its file or bytes are set again.
    pub fn set_overlay(&mut self, file: ResourceFile, overlay: Arc<dyn ResourceOverlay>) {
        self.overlays[slot(file)] = Some(overlay);
    }

    /// The file's own length, without its overlay.
    fn own_length(&self, i: usize) -> Option<u64> {
        if let Some(m) = &self.memory[i] {
            return Some(m.len() as u64);
        }
        self.regions[i].as_ref().map(|r| r.length)
    }

    /// Length of a supplied file, extended over its overlay's appended
    /// extents.
    pub fn length(&self, file: ResourceFile) -> Option<u64> {
        let i = slot(file);
        let own = self.own_length(i)?;
        Some(self.overlays[i].as_ref().map_or(own, |o| o.end().max(own)))
    }

    /// Reads `size` bytes at map-relative `offset` of the selected file,
    /// through its overlay when it has one. Sizes above [`MAX_READ`] are
    /// rejected before allocating.
    pub fn read(&mut self, file: ResourceFile, offset: u32, size: u32) -> Result<Vec<u8>> {
        let _scope = mem_watch::scope("assets.resource");
        let i = slot(file);
        if size > MAX_READ {
            return Err(Error::Invalid(format!(
                "read of {size} bytes exceeds cap {MAX_READ}"
            )));
        }
        let Some(overlay) = self.overlays[i].clone() else {
            return self.read_own(i, file, offset, size);
        };
        let own = self.own_length(i).ok_or(Error::MissingResource(file))?;
        let limit = overlay.end().max(own);
        let end = u64::from(offset) + u64::from(size);
        if end > limit {
            return Err(Error::Map(map::Error::Bounds {
                what: "resource",
                offset: offset.into(),
                size: size.into(),
                limit,
            }));
        }
        // The file's part, then zero up to the appended extents.
        let inside = own.saturating_sub(u64::from(offset)).min(u64::from(size)) as u32;
        let mut buf = if inside == 0 {
            Vec::new()
        } else {
            self.read_own(i, file, offset, inside)?
        };
        buf.resize(size as usize, 0);
        overlay.apply(u64::from(offset), &mut buf);
        Ok(buf)
    }

    fn read_own(&mut self, i: usize, file: ResourceFile, offset: u32, size: u32) -> Result<Vec<u8>> {
        if let Some(m) = &self.memory[i] {
            let range = usize::try_from(offset).ok().and_then(|at| {
                let end = at.checked_add(size as usize)?;
                m.get(at..end)
            });
            return range
                .map(<[u8]>::to_vec)
                .ok_or(Error::Map(map::Error::Bounds {
                    what: "resource",
                    offset: offset.into(),
                    size: size.into(),
                    limit: m.len() as u64,
                }));
        }
        let region = self.regions[i]
            .as_ref()
            .ok_or(Error::MissingResource(file))?;
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
        assert!(
            f.set(
                ResourceFile::Local,
                Region {
                    path: p.clone(),
                    base: 0,
                    length: 64
                }
            )
            .is_ok()
        );
        assert!(
            f.set(
                ResourceFile::Shared,
                Region {
                    path: p.clone(),
                    base: 32,
                    length: 64
                }
            )
            .is_err()
        );
        assert!(
            f.set(
                ResourceFile::Shared,
                Region {
                    path: p.clone(),
                    base: u64::MAX - 1,
                    length: 8
                }
            )
            .is_err()
        );
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn only_a_pinned_resource_opens_before_the_path_is_replaced() {
        let path = temp("opened", &[1, 2]);
        let replacement = temp("replacement", &[3, 4]);
        let mut files = ResourceFiles::new();
        let region = Region { path: path.clone(), base: 0, length: 2 };
        files.set_pinned(ResourceFile::Local, region.clone()).unwrap();
        files.set(ResourceFile::Shared, region).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(files.read(ResourceFile::Local, 0, 2).unwrap(), [1, 2]);
        assert_eq!(files.read(ResourceFile::Shared, 0, 2).unwrap(), [3, 4]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn huge_declared_sizes_are_rejected_before_allocation() {
        let p = temp("huge", &[0u8; 16]);
        let mut f = ResourceFiles::new();
        f.set(
            ResourceFile::Local,
            Region {
                path: p.clone(),
                base: 0,
                length: 16,
            },
        )
        .unwrap();
        assert!(matches!(
            f.read(ResourceFile::Local, 0, u32::MAX),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            f.read(ResourceFile::Local, 8, 16),
            Err(Error::Map(_))
        ));
        assert_eq!(f.read(ResourceFile::Local, 4, 4).unwrap().len(), 4);
        std::fs::remove_file(p).unwrap();
    }

    struct Over(Vec<(u64, Vec<u8>)>, u64);
    impl ResourceOverlay for Over {
        fn apply(&self, offset: u64, buf: &mut [u8]) {
            for (at, bytes) in &self.0 {
                for (k, b) in bytes.iter().enumerate() {
                    let p = at + k as u64;
                    if p >= offset && p < offset + buf.len() as u64 {
                        buf[(p - offset) as usize] = *b;
                    }
                }
            }
        }
        fn end(&self) -> u64 {
            self.1
        }
    }

    /// An overlay's replacements are read over the file's bytes, and its
    /// appended extent past the file's end, zero between; the length grows.
    #[test]
    fn an_overlay_serves_replacements_and_appended_extents() {
        let p = temp("over", &[9u8; 16]);
        let mut f = ResourceFiles::new();
        f.set(ResourceFile::Local, Region { path: p.clone(), base: 0, length: 16 }).unwrap();
        f.set_overlay(
            ResourceFile::Local,
            Arc::new(Over(vec![(2, vec![1, 2]), (20, vec![7, 7, 7, 7])], 24)),
        );
        assert_eq!(f.length(ResourceFile::Local), Some(24));
        assert_eq!(f.read(ResourceFile::Local, 0, 5).unwrap(), [9, 9, 1, 2, 9]);
        assert_eq!(f.read(ResourceFile::Local, 14, 10).unwrap(), [9, 9, 0, 0, 0, 0, 7, 7, 7, 7]);
        assert_eq!(f.read(ResourceFile::Local, 20, 4).unwrap(), [7, 7, 7, 7]);
        assert!(f.read(ResourceFile::Local, 20, 5).is_err());
        // Another selector has no overlay.
        f.set(ResourceFile::Shared, Region { path: p.clone(), base: 0, length: 16 }).unwrap();
        assert_eq!(f.read(ResourceFile::Shared, 2, 2).unwrap(), [9, 9]);
        let _ = std::fs::remove_file(p);
    }

    /// A selector backed by bytes in memory reads them, bounds-checked,
    /// and replaces a file region set before it (and the reverse).
    #[test]
    fn memory_bytes_replace_a_file_region() {
        let p = temp("mem", &[9u8; 16]);
        let mut f = ResourceFiles::new();
        f.set(
            ResourceFile::Local,
            Region {
                path: p.clone(),
                base: 0,
                length: 16,
            },
        )
        .unwrap();
        f.set_bytes(ResourceFile::Local, Arc::from(&[1u8, 2, 3, 4, 5][..]));
        assert_eq!(f.length(ResourceFile::Local), Some(5));
        assert_eq!(f.read(ResourceFile::Local, 1, 3).unwrap(), [2, 3, 4]);
        assert!(f.read(ResourceFile::Local, 3, 3).is_err());
        assert!(f.read(ResourceFile::Local, u32::MAX, 2).is_err());
        f.set(
            ResourceFile::Local,
            Region {
                path: p.clone(),
                base: 0,
                length: 16,
            },
        )
        .unwrap();
        assert_eq!(f.read(ResourceFile::Local, 0, 2).unwrap(), [9, 9]);
        let _ = std::fs::remove_file(p);
    }
}
