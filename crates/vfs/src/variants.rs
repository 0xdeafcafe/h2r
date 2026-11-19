//! Converted Halo 1 maps served in the sandbox chosen for the next game
//! (docs/halo1/multiplayer.md, "Setting, Halo 1 maps sandbox").
//!
//! A converted multiplayer map has one whole map per sandbox
//! (`<data>/maps/h1_<map>/{h1,h2}/h1_<map>.map`). 1.0 opens a new map from
//! its content package, `T:\$c\<offer>\<map>.map` (docs/game/map-pack.md
//! "Loading"), which `boot::new_maps` writes at boot. [`VariantFiles`] sits
//! over the data partition and serves that path from the variant the
//! [`SandboxChoice`] names when the file is opened, so the choice made on
//! the pregame screen applies at the map's next open with nothing copied:
//!
//! - a file open now keeps the variant it was opened with until its last
//!   handle closes (`opened`, `closed`);
//! - [`Backend::map_changed`] says when the map would now be served from
//!   the other variant than at its last open, so the boot drops the game's
//!   cached copy (`boot::map_slots`, docs/runtime/cache-slots.md);
//! - writes go to the inner file (the materialised copy), which the game
//!   never writes.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{Backend, BasicInfo, DirectoryEntry, Error, HostFile, Metadata, Result, VolumeInfo};
use crate::{content_maps::ContentFiles, map_override::resize};

/// Halo 1's assets or Halo 2's, shared between whoever sets it (the
/// pregame screen's rows) and the backends that serve by it. Halo 1's by
/// default. Clones share the one value.
#[derive(Clone, Debug)]
pub struct SandboxChoice(Arc<AtomicBool>);

impl Default for SandboxChoice {
    fn default() -> Self {
        SandboxChoice(Arc::new(AtomicBool::new(true)))
    }
}

impl SandboxChoice {
    pub fn set_halo1(&self, halo1: bool) {
        self.0.store(halo1, Ordering::SeqCst);
    }
    pub fn halo1(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// `h1` or `h2`, as manifests and the settings name the sandboxes.
    pub fn name(&self) -> &'static str {
        if self.halo1() { "h1" } else { "h2" }
    }
}

/// One sandbox's map file, and its package `MapId` when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantFile {
    pub file: PathBuf,
    pub id: Option<content::MapId>,
}

/// A map's two variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantPair {
    pub h1: VariantFile,
    pub h2: VariantFile,
}

impl VariantPair {
    fn pick(&self, halo1: bool) -> &VariantFile {
        if halo1 { &self.h1 } else { &self.h2 }
    }
}

/// The maps served by the choice, by their path on the partition
/// (`TDATA\<title>\$c\<offer>\<map>.map`, any case), and the choice.
/// Clones share both, so the boot can register maps after the VFS is
/// mounted.
#[derive(Clone, Debug, Default)]
pub struct ContentVariants {
    maps: Arc<Mutex<BTreeMap<String, VariantPair>>>,
    choice: SandboxChoice,
}

impl ContentVariants {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, VariantPair>> {
        self.maps.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Serve `path` (partition-relative) from `pair` by the choice.
    pub fn register(&self, path: &str, pair: VariantPair) {
        self.lock().insert(path.to_ascii_lowercase(), pair);
    }
    /// Replace all sandbox paths together.
    pub fn replace(&self, maps: impl IntoIterator<Item = (String, VariantPair)>) {
        *self.lock() = maps
            .into_iter()
            .map(|(path, pair)| (path.to_ascii_lowercase(), pair))
            .collect();
    }
    /// Forget every map (a new install replaces the table).
    pub fn clear(&self) {
        self.lock().clear();
    }
    pub fn has_id(&self, id: content::MapId) -> bool {
        self.lock()
            .values()
            .any(|pair| pair.h1.id == Some(id) || pair.h2.id == Some(id))
    }
    pub fn choice(&self) -> &SandboxChoice {
        &self.choice
    }
    pub fn len(&self) -> usize {
        self.lock().len()
    }
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
    fn get(&self, path: &str) -> Option<VariantPair> {
        let maps = self.lock();
        if maps.is_empty() {
            return None;
        }
        maps.get(&path.to_ascii_lowercase()).cloned()
    }
}

struct Served {
    halo1: bool,
    file: File,
    len: u64,
}

/// [`ContentVariants`] over `inner` (the data partition's host backend).
pub struct VariantFiles<B> {
    inner: B,
    variants: ContentVariants,
    packages: Option<ContentFiles>,
    /// Per lower-case path: handles open, and the variant they read.
    open: BTreeMap<String, (usize, bool)>,
    /// The variant each path was last opened with.
    last_opened: BTreeMap<String, bool>,
    /// The host file being read, per lower-case path.
    files: BTreeMap<String, Served>,
}

impl<B: Backend> VariantFiles<B> {
    pub fn new(inner: B, variants: ContentVariants) -> Self {
        Self {
            inner,
            variants,
            packages: None,
            open: BTreeMap::new(),
            last_opened: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }

    pub(crate) fn with_packages(
        mut self,
        maps: crate::ContentMaps,
        root: &std::path::Path,
    ) -> Self {
        self.packages = Some(ContentFiles::new(maps, root));
        self
    }

    fn package(&mut self, path: &str) -> Option<&mut ContentFiles> {
        self.packages.as_mut().filter(|maps| maps.contains(path))
    }

    fn refuse_package(&mut self, path: &str) -> Result<()> {
        if self.package(path).is_some() {
            return Err(Error::AccessDenied);
        }
        Ok(())
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    /// The variant `path` serves now: the one its open handles have, else
    /// the choice's. `None` when `path` is not a registered map.
    fn variant(&self, path: &str) -> Option<(String, bool, VariantPair)> {
        let pair = self.variants.get(path)?;
        let key = path.to_ascii_lowercase();
        let halo1 = match self.open.get(&key) {
            Some(&(_, pinned)) => pinned,
            None => self.variants.choice.halo1(),
        };
        Some((key, halo1, pair))
    }

    /// The host file `path` serves now, opened.
    fn served(&mut self, path: &str) -> Option<Result<&mut Served>> {
        let (key, halo1, pair) = self.variant(path)?;
        if self.files.get(&key).is_some_and(|s| s.halo1 != halo1) {
            self.files.remove(&key);
        }
        if !self.files.contains_key(&key) {
            let opened = File::open(&pair.pick(halo1).file).and_then(|file| {
                let len = file.metadata()?.len();
                Ok(Served { halo1, file, len })
            });
            match opened {
                Ok(s) => {
                    self.files.insert(key.clone(), s);
                }
                Err(e) => return Some(Err(e.into())),
            }
        }
        self.files.get_mut(&key).map(Ok)
    }
}

impl<B: Backend> Backend for VariantFiles<B> {
    fn read_only(&self) -> bool {
        self.inner.read_only()
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        if self.variant(path).is_none()
            && let Some(p) = self.package(path)
        {
            let len = p.served(path)?.len;
            let mut meta = Metadata {
                basic: BasicInfo {
                    attributes: crate::ATTR_READONLY,
                    ..BasicInfo::default()
                },
                end_of_file: 0,
                allocation_size: 0,
            };
            resize(&mut meta, len);
            return Ok(meta);
        }
        let mut meta = self.inner.stat(path)?;
        if meta.is_directory() {
            return Ok(meta);
        }
        if let Some(served) = self.served(path) {
            resize(&mut meta, served?.len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if self.variants.is_empty() {
            return Ok(entries);
        }
        for e in &mut entries {
            if e.metadata.is_directory() {
                continue;
            }
            let child = format!("{path}\\{}", e.name);
            if let Some(served) = self.served(&child) {
                resize(&mut e.metadata, served?.len);
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if self.variant(path).is_none()
            && let Some(p) = self.package(path)
        {
            return p.served(path)?.read_at(offset, buffer);
        }
        let Some(served) = self.served(path) else {
            return self.inner.read_at(path, offset, buffer);
        };
        let s = served?;
        let n = s.len.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        s.file.seek(SeekFrom::Start(offset))?;
        match s.file.read_exact(&mut buffer[..n]) {
            Ok(()) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::FileCorrupt),
            Err(e) => Err(e.into()),
        }
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        let Some((_, halo1, pair)) = self.variant(path) else {
            return match self.package(path) {
                Some(p) => p.served(path).ok()?.host_file(),
                None => self.inner.host_file(path),
            };
        };
        let len = self.served(path)?.ok()?.len;
        Some(HostFile {
            path: pair.pick(halo1).file.clone(),
            offset: 0,
            len,
        })
    }
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        match self.variant(path) {
            Some((_, halo1, pair)) => pair.pick(halo1).id,
            None => match self.package(path) {
                Some(p) => p.id(path),
                None => self.inner.map_id(path),
            },
        }
    }
    fn opened(&mut self, path: &str) {
        if let Some((key, halo1, _)) = self.variant(path) {
            let e = self.open.entry(key.clone()).or_insert((0, halo1));
            e.0 += 1;
            self.last_opened.insert(key, e.1);
        }
        if let Some(p) = &mut self.packages {
            p.opened(path);
        }
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        let key = path.to_ascii_lowercase();
        if let Some(e) = self.open.get_mut(&key) {
            e.0 = e.0.saturating_sub(1);
            if e.0 == 0 {
                self.open.remove(&key);
            }
        }
        if let Some(p) = &mut self.packages {
            p.closed(path);
        }
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        if let Some(p) = &mut self.packages {
            p.prune();
        }
        self.inner.rescan()
    }
    fn map_changed(&mut self, path: &str) -> bool {
        if self.variants.get(path).is_none() {
            return self.packages.as_ref().is_some_and(|p| p.changed(path))
                || self.inner.map_changed(path);
        }
        let key = path.to_ascii_lowercase();
        if self.open.contains_key(&key) {
            return false;
        }
        self.last_opened
            .get(&key)
            .is_some_and(|&was| was != self.variants.choice.halo1())
    }
    fn create(&mut self, path: &str, directory: bool, attributes: u32) -> Result<()> {
        self.inner.create(path, directory, attributes)
    }
    fn create_allocated(
        &mut self,
        path: &str,
        directory: bool,
        attributes: u32,
        allocation: u64,
        owner: u64,
    ) -> Result<()> {
        self.inner
            .create_allocated(path, directory, attributes, allocation, owner)
    }
    fn overwrite_allocated(&mut self, path: &str, allocation: u64, owner: u64) -> Result<()> {
        self.refuse_package(path)?;
        self.inner.overwrite_allocated(path, allocation, owner)
    }
    fn release_allocation(&mut self, path: &str, owner: u64) -> Result<()> {
        self.inner.release_allocation(path, owner)
    }
    fn write_at(&mut self, path: &str, offset: u64, buffer: &[u8]) -> Result<usize> {
        self.refuse_package(path)?;
        self.inner.write_at(path, offset, buffer)
    }
    fn set_len(&mut self, path: &str, size: u64) -> Result<()> {
        self.refuse_package(path)?;
        self.inner.set_len(path, size)
    }
    fn set_basic_info(&mut self, path: &str, basic: BasicInfo) -> Result<()> {
        self.refuse_package(path)?;
        self.inner.set_basic_info(path, basic)
    }
    fn remove(&mut self, path: &str) -> Result<()> {
        self.refuse_package(path)?;
        self.files.remove(&path.to_ascii_lowercase());
        self.inner.remove(path)
    }
    fn flush(&mut self, path: &str) -> Result<()> {
        self.inner.flush(path)
    }
    fn format(&mut self) -> Result<()> {
        self.files.clear();
        self.inner.format()
    }
    fn cleanup(&mut self, path: &str, write_access: bool) -> Result<()> {
        self.inner.cleanup(path, write_access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ATTR_DIRECTORY;

    const MAP: &str = "TDATA\\4D530064\\$c\\4D53006400000101\\h1_gulch.map";

    /// A partition holding `MAP` (the copy written at boot) and a save.
    struct Fake;
    fn meta(len: u64, dir: bool) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: if dir { ATTR_DIRECTORY } else { 0 },
                ..Default::default()
            },
            end_of_file: len,
            allocation_size: len.div_ceil(512) * 512,
        }
    }
    impl Backend for Fake {
        fn read_only(&self) -> bool {
            false
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            if path.eq_ignore_ascii_case(MAP) {
                Ok(meta(4, false))
            } else if path.eq_ignore_ascii_case("save.dat") {
                Ok(meta(3, false))
            } else if path.eq_ignore_ascii_case("TDATA\\4D530064\\$c\\4D53006400000101") {
                Ok(meta(0, true))
            } else {
                Err(Error::ObjectNameNotFound)
            }
        }
        fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(vec![DirectoryEntry {
                name: "h1_gulch.map".into(),
                metadata: meta(4, false),
            }])
        }
        fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            let bytes: &[u8] = if path.eq_ignore_ascii_case(MAP) {
                b"boot"
            } else {
                b"sav"
            };
            let start = (offset as usize).min(bytes.len());
            let n = (bytes.len() - start).min(buffer.len());
            buffer[..n].copy_from_slice(&bytes[start..start + n]);
            Ok(n)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-vfs-variants-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(b: &mut VariantFiles<Fake>, path: &str) -> Vec<u8> {
        let len = b.stat(path).unwrap().end_of_file as usize;
        let mut buf = vec![0u8; len];
        assert_eq!(b.read_at(path, 0, &mut buf).unwrap(), len);
        buf
    }

    fn id(b: u8) -> Option<content::MapId> {
        Some(content::MapId([b; 32]))
    }

    /// The choice picks the variant at the map's next open; an open map
    /// keeps the one it was opened with, and the change is reported once it
    /// is closed so the game's cached copy can be dropped.
    #[test]
    fn the_served_variant_follows_the_choice_at_the_next_open() {
        let d = dir();
        std::fs::write(d.join("h1.map"), b"halo one sandbox").unwrap();
        std::fs::write(d.join("h2.map"), b"halo two").unwrap();
        let variants = ContentVariants::default();
        let mut b = VariantFiles::new(Fake, variants.clone());
        // Nothing registered: the partition's own file.
        assert_eq!(read(&mut b, MAP), b"boot");
        variants.register(
            MAP,
            VariantPair {
                h1: VariantFile { file: d.join("h1.map"), id: id(1) },
                h2: VariantFile { file: d.join("h2.map"), id: id(2) },
            },
        );
        let lower = MAP.to_ascii_lowercase();
        assert_eq!(read(&mut b, &lower), b"halo one sandbox", "any case");
        assert_eq!(b.map_id(MAP), id(1));
        b.opened(MAP);
        variants.choice().set_halo1(false);
        assert_eq!(read(&mut b, MAP), b"halo one sandbox", "open: keeps its variant");
        assert_eq!(b.map_id(MAP), id(1));
        assert!(!b.map_changed(MAP), "not while open");
        b.closed(MAP);
        assert!(b.map_changed(MAP), "opened in Halo 1's, Halo 2's now");
        assert_eq!(read(&mut b, MAP), b"halo two");
        assert_eq!(b.map_id(MAP), id(2));
        assert_eq!(b.host_file(MAP).unwrap().path, d.join("h2.map"));
        let listed = b.read_directory("TDATA\\4D530064\\$c\\4D53006400000101").unwrap();
        assert_eq!(listed[0].metadata.end_of_file, 8, "listed at the served size");
        b.opened(MAP);
        b.closed(MAP);
        assert!(!b.map_changed(MAP));
        // Anything else is the partition's.
        assert_eq!(read(&mut b, "save.dat"), b"sav");
        assert!(!b.map_changed("save.dat"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
