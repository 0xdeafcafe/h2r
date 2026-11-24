//! Maps served from the tag store (docs/engine/asset-store.md, "Runtime
//! assembly").
//!
//! The store keeps each map the extractor took as a skeleton and an extent
//! table (`tags::repo::skeleton`); [`StoreFile`] gives the map file back
//! byte for byte. [`StoreMapBackend`] serves `maps\<name>.map` that way over
//! a disc backend: `stat` gives the map's length, `read_at` answers a range
//! from the skeleton or from `raw/<hh>/<sha256>` through the extent table.
//! The disc's own map files are never read: a map the store does not hold
//! is a named error ([`Error::ObjectNameNotFound`], with a line on
//! stderr), and a raw file that is missing is [`Error::FileCorrupt`] with
//! the hash named, never another file. Every other path is the inner
//! backend's.
//!
//! A map's extent table changing (an extraction while the game runs) is a
//! changed map: [`Backend::map_changed`] says so, and the new table is
//! served from the next open with no handle on the old one.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

pub use tags::repo::StoreFile;
pub use tags::repo::skeleton::{MapStatus, StoreStatus, status};

use crate::{
    ATTR_DIRECTORY, ATTR_READONLY, Backend, BasicInfo, DirectoryEntry, Error, HostFile, Metadata,
    Result, VolumeInfo,
    map_override::{is_map_path, resize},
};

enum Loaded {
    File(Box<StoreFile>),
    /// The store has no such map: why, in words.
    Missing(String),
    /// The store's files for the map do not read.
    Broken(String),
}

struct Held {
    loaded: Loaded,
    /// The store's name for the map (its file stem).
    stem: Option<String>,
    /// SHA-256 of the extent table it was loaded from.
    table: Option<String>,
    /// Handles open on it.
    opens: usize,
    /// The error was reported on stderr.
    told: bool,
}

/// Serves `maps\<name>.map` from the tag store at a root (`<data>/tags`),
/// everything else from `inner`.
pub struct StoreMapBackend<B> {
    inner: B,
    root: PathBuf,
    /// By lower-case map name (no extension).
    held: HashMap<String, Held>,
}

/// The store's stem for `key` (lower case): the map whose extent table is
/// `<key>.extents`, matched without regard to case.
fn find_stem(root: &Path, key: &str) -> Option<String> {
    if StoreFile::exists(root, key) {
        return Some(key.to_owned());
    }
    let dir = std::fs::read_dir(root.join("maps")).ok()?;
    dir.flatten().find_map(|e| {
        let name = e.file_name().to_str()?.to_owned();
        let stem = name.strip_suffix(".extents")?;
        (stem.eq_ignore_ascii_case(key) && StoreFile::exists(root, stem)).then(|| stem.to_owned())
    })
}

fn file_meta(len: u64) -> Metadata {
    let mut m = Metadata {
        basic: BasicInfo {
            attributes: ATTR_READONLY,
            ..Default::default()
        },
        end_of_file: 0,
        allocation_size: 0,
    };
    resize(&mut m, len);
    m
}

impl<B: Backend> StoreMapBackend<B> {
    /// Serve the maps of the store at `root` over `inner`.
    pub fn new(inner: B, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        eprintln!(
            "store maps: maps\\*.map served from the store at {}",
            root.display()
        );
        Self {
            inner,
            root,
            held: HashMap::new(),
        }
    }

    /// The store's root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn load(&self, key: &str) -> Held {
        let Some(stem) = find_stem(&self.root, key) else {
            return Held {
                loaded: Loaded::Missing(format!(
                    "the store at {} has no {key} (no skeleton or extent table; `h2 tags extract --update` writes them)",
                    self.root.display()
                )),
                stem: None,
                table: None,
                opens: 0,
                told: false,
            };
        };
        let table = StoreFile::table_hash(&self.root, &stem);
        let loaded = match StoreFile::open(&self.root, &stem) {
            Ok(f) => {
                eprintln!(
                    "store maps: maps\\{stem}.map from the store ({} bytes: {} in the skeleton, {} extents)",
                    f.len(),
                    f.skeleton_bytes(),
                    f.extent_count()
                );
                Loaded::File(Box::new(f))
            }
            Err(e) => Loaded::Broken(e.to_string()),
        };
        Held {
            loaded,
            stem: Some(stem),
            table,
            opens: 0,
            told: false,
        }
    }

    /// The key of a map path, loaded; `None` for any other path.
    fn ensure(&mut self, path: &str) -> Option<String> {
        let name = is_map_path(path)?;
        let key = name[..name.len() - 4].to_ascii_lowercase();
        if !self.held.contains_key(&key) {
            let held = self.load(&key);
            self.held.insert(key.clone(), held);
        }
        Some(key)
    }

    /// Whether the map's extent table differs from the one it was loaded
    /// from (or now exists or is gone).
    fn stale(&self, key: &str) -> bool {
        let Some(h) = self.held.get(key) else {
            return false;
        };
        let stem = h.stem.clone().or_else(|| find_stem(&self.root, key));
        let now = stem.and_then(|s| StoreFile::table_hash(&self.root, &s));
        now != h.table
    }

    /// The error for a map that cannot be served, said once on stderr.
    fn refuse(&mut self, path: &str, key: &str) -> Error {
        let h = self.held.get_mut(key).expect("ensured");
        let (why, e) = match &h.loaded {
            Loaded::Missing(why) => (why.clone(), Error::ObjectNameNotFound),
            Loaded::Broken(why) => (why.clone(), Error::FileCorrupt),
            Loaded::File(_) => unreachable!("served"),
        };
        if !h.told {
            eprintln!("store maps: {path}: {why}; the disc's file is not served");
            h.told = true;
        }
        e
    }

    /// Lengths of every map the store holds, by file name (`<stem>.map`).
    fn listed(&self) -> Vec<(String, u64)> {
        status(&self.root)
            .maps
            .into_iter()
            .map(|m| (format!("{}.map", m.map), m.len))
            .collect()
    }
}

impl<B: Backend> Backend for StoreMapBackend<B> {
    fn snapshot_map_bindings(
        &self,
    ) -> std::result::Result<Vec<(String, String)>, crate::StateError> {
        self.inner.snapshot_map_bindings()
    }
    fn opened(&mut self, path: &str) {
        let Some(key) = self.ensure(path) else {
            return self.inner.opened(path);
        };
        let idle = self.held.get(&key).is_some_and(|h| h.opens == 0);
        if idle && self.stale(&key) {
            let held = self.load(&key);
            self.held.insert(key.clone(), held);
        }
        if let Some(h) = self.held.get_mut(&key) {
            h.opens += 1;
        }
    }
    fn closed(&mut self, path: &str) {
        let Some(key) = self.ensure(path) else {
            return self.inner.closed(path);
        };
        if let Some(h) = self.held.get_mut(&key) {
            h.opens = h.opens.saturating_sub(1);
        }
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        if path.eq_ignore_ascii_case("maps") {
            return match self.inner.stat(path) {
                Ok(m) => Ok(m),
                Err(_) if !self.listed().is_empty() => Ok(Metadata {
                    basic: BasicInfo {
                        attributes: ATTR_READONLY | ATTR_DIRECTORY,
                        ..Default::default()
                    },
                    end_of_file: 0,
                    allocation_size: 0,
                }),
                Err(e) => Err(e),
            };
        }
        let Some(key) = self.ensure(path) else {
            return self.inner.stat(path);
        };
        let len = match &self.held[&key].loaded {
            Loaded::File(f) => f.len(),
            _ => return Err(self.refuse(path, &key)),
        };
        match self.inner.stat(path) {
            Ok(mut m) => {
                resize(&mut m, len);
                Ok(m)
            }
            Err(_) => Ok(file_meta(len)),
        }
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        if !path.eq_ignore_ascii_case("maps") {
            return self.inner.read_directory(path);
        }
        let listed = self.listed();
        let mut entries = match self.inner.read_directory(path) {
            Ok(e) => e,
            Err(_) if !listed.is_empty() => Vec::new(),
            Err(e) => return Err(e),
        };
        // The disc's map files are not served: only the store's are listed.
        let mut seen = BTreeSet::new();
        entries.retain_mut(|e| {
            if e.metadata.is_directory() || is_map_path(&format!("maps\\{}", e.name)).is_none() {
                return true;
            }
            let lower = e.name.to_ascii_lowercase();
            match listed.iter().find(|(n, _)| n.eq_ignore_ascii_case(&e.name)) {
                Some((_, len)) => {
                    resize(&mut e.metadata, *len);
                    seen.insert(lower);
                    true
                }
                None => false,
            }
        });
        for (name, len) in listed {
            if !seen.contains(&name.to_ascii_lowercase()) {
                entries.push(DirectoryEntry {
                    name,
                    metadata: file_meta(len),
                });
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let Some(key) = self.ensure(path) else {
            return self.inner.read_at(path, offset, buffer);
        };
        let r = match &mut self.held.get_mut(&key).expect("ensured").loaded {
            Loaded::File(f) => f.read_at(offset, buffer),
            _ => return Err(self.refuse(path, &key)),
        };
        r.map_err(|e| {
            eprintln!("store maps: {path}: {e}");
            Error::FileCorrupt
        })
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    /// A store-served map is not one host region.
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        if is_map_path(path).is_some() {
            return None;
        }
        self.inner.host_file(path)
    }
    /// A film's digest of a store-served map reads it through the backend.
    fn film_source(&mut self, path: &str) -> Result<Option<crate::FilmMapSource>> {
        if is_map_path(path).is_some() {
            return Ok(None);
        }
        self.inner.film_source(path)
    }
    /// The whole file is there.
    fn map_ranges(&mut self, path: &str) -> Option<Vec<(u64, u64)>> {
        if is_map_path(path).is_some() {
            return None;
        }
        self.inner.map_ranges(path)
    }
    /// The store serves the retail map, whose identity is the disc's.
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        if is_map_path(path).is_some() {
            return None;
        }
        self.inner.map_id(path)
    }
    fn resolves_on_use(&mut self, path: &str) -> bool {
        is_map_path(path).is_none() && self.inner.resolves_on_use(path)
    }
    /// A map whose extent table changed since it was loaded. With no handle
    /// open it is loaded again at once, so the next open serves the new
    /// table.
    fn map_changed(&mut self, path: &str) -> bool {
        let Some(key) = self.ensure(path) else {
            return self.inner.map_changed(path);
        };
        if !self.stale(&key) {
            return false;
        }
        if self.held.get(&key).is_some_and(|h| h.opens == 0) {
            let held = self.load(&key);
            self.held.insert(key, held);
        }
        true
    }
}
