//! Package maps mounted at their own paths on the data partition.

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use content::{
    BaseRef, Data, Library, MapId, Package,
    patch::{FileSource, Overlay, Source},
};

use crate::{Error, HostFile, Result};

/// A package's immutable load inputs. The base is a region of the retail
/// ISO or an imported Map Pack file; a whole-map package needs neither.
#[derive(Clone, Debug)]
pub struct ContentMap {
    pub package: Package,
    pub base: Option<HostFile>,
    /// The name written into the served cache header, when the source
    /// header could identify the base or another package.
    pub alias: Option<String>,
}

impl ContentMap {
    fn same_source(&self, other: &Self) -> bool {
        self.package.id == other.package.id
            && self.package.dir == other.package.dir
            && self.base == other.base
            && self.alias == other.alias
    }
}

/// Generated content paths, replaced as a set when packages change.
#[derive(Clone, Debug, Default)]
pub struct ContentMaps(Arc<Mutex<BTreeMap<String, ContentMap>>>);

impl ContentMaps {
    pub fn replace(&self, maps: impl IntoIterator<Item = (String, ContentMap)>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = maps
            .into_iter()
            .map(|(path, map)| (path.to_ascii_lowercase(), map))
            .collect();
    }

    pub fn has_id(&self, id: MapId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|map| map.package.id == id)
    }

    fn contains_key(&self, lower_path: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(lower_path)
    }

    pub(crate) fn get(&self, path: &str) -> Option<ContentMap> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&path.to_ascii_lowercase())
            .cloned()
    }

    pub(crate) fn path_for_name(&self, name: &str) -> Option<String> {
        let file = format!("{}.map", name.to_ascii_lowercase());
        let maps = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut matching = maps
            .keys()
            .filter(|path| path.rsplit('\\').next() == Some(file.as_str()));
        let path = matching.next()?;
        matching.next().is_none().then(|| path.clone())
    }
}

struct Region {
    file: File,
    offset: u64,
    len: u64,
}

impl Region {
    fn open(file: &HostFile) -> io::Result<Self> {
        Ok(Self {
            file: File::open(&file.path)?,
            offset: file.offset,
            len: file.len,
        })
    }
}

impl Source for Region {
    fn size(&self) -> u64 {
        self.len
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        self.file
            .seek(SeekFrom::Start(self.offset + offset.min(self.len)))?;
        self.file.read_exact(&mut buf[..n])?;
        Ok(n)
    }
}

enum Reader {
    Overlay {
        base: Region,
        overlay: Arc<Overlay>,
    },
    File(FileSource, HostFile),
    Mapped(memmap2::Mmap, HostFile, File),
}

pub(crate) struct Served {
    map: ContentMap,
    reader: Reader,
    pub(crate) len: u64,
    alias: Option<[u8; 32]>,
}

impl Served {
    fn new(library: &Library, map: ContentMap, server_maps: Option<&Path>) -> Result<Self> {
        let mut base = map.base.as_ref().map(Region::open).transpose()?;
        let mut empty: &[u8] = &[];
        let source: &mut dyn Source = match &mut base {
            Some(base) => base,
            None => &mut empty,
        };
        let resolved = library.resolve_package(&map.package, source).map_err(|e| {
            eprintln!("map packages: {}: {e}", map.package.dir.display());
            Error::FileCorrupt
        })?;
        let len = resolved.len();
        if len > u64::from(u32::MAX) {
            return Err(Error::FileCorrupt);
        }
        let reader = match resolved.data {
            Data::Overlay {
                base: source,
                overlay,
            } => {
                let base = match source {
                    BaseRef::Disc => base.ok_or(Error::FileCorrupt)?,
                    BaseRef::File { file, .. } => Region { len: file.metadata()?.len(), file, offset: 0 },
                };
                Reader::Overlay { base, overlay: Arc::new(overlay) }
            }
            Data::File { path, file, .. } => Reader::File(
                FileSource::new(file)?,
                HostFile {
                    path,
                    offset: 0,
                    len,
                },
            ),
        };
        let alias = map.alias.as_ref().map(|name| {
            let mut field = [0; 32];
            let target = field
                .get_mut(..name.len().min(31))
                .expect("a bounded header name");
            target.copy_from_slice(&name.as_bytes()[..target.len()]);
            field
        });
        let mut served = Self {
            map,
            reader,
            len,
            alias,
        };
        if let Some(dir) = server_maps {
            let path = dir.join(format!("{}.map", served.id()));
            if let Ok(mut file) = File::open(&path) {
                // Compare the source before apply_alias edits the name.
                let mut header = [0; 0x800];
                let n = served.read_source(0, &mut header)?;
                let size = file.metadata()?.len();
                if n == header.len()
                    && crate::map_override::matching_server_copy(&mut file, size, len, &header)
                {
                    if let Some(mapping) = crate::disc::map_read_only(&file) {
                        served.reader = Reader::Mapped(mapping, HostFile { path, offset: 0, len }, file);
                    } else {
                        served.reader = Reader::File(FileSource::new(file)?, HostFile { path, offset: 0, len });
                    }
                } else {
                    eprintln!("map packages: {}: server copy header or length differs", path.display());
                }
            }
        }
        let mut magic = [0; 4];
        if served.alias.is_some()
            && (len < 0x800 || served.read_at(0, &mut magic)? != 4 || &magic != b"daeh")
        {
            served.alias = None;
        }
        Ok(served)
    }

    pub(crate) fn id(&self) -> MapId {
        self.map.package.id
    }

    pub(crate) fn film_source(&self) -> Result<crate::FilmMapSource> {
        let mut source = match &self.reader {
            Reader::File(f, _) => crate::FilmMapSource::region(f.file().try_clone()?, 0),
            Reader::Mapped(_, region, file) => crate::FilmMapSource::immutable_region(file.try_clone()?, 0, region.len),
            Reader::Overlay { base, overlay } => crate::FilmMapSource::overlay(
                crate::FilmMapSource::region(base.file.try_clone()?, base.offset),
                base.len, overlay.clone(), self.id(),
            ),
        };
        source.alias = self.alias;
        Ok(source)
    }

    pub(crate) fn host_file(&self) -> Option<HostFile> {
        match (&self.reader, self.alias) {
            (Reader::File(_, file) | Reader::Mapped(_, file, _), None) => Some(file.clone()),
            _ => None,
        }
    }

    fn read_source(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        match &mut self.reader {
            Reader::File(file, _) => Ok(file.read_at(offset, buf)?),
            Reader::Overlay { base, overlay } => Ok(overlay.read_at(base, offset, buf)?),
            Reader::Mapped(map, _, _) => {
                let n = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
                if n > 0 {
                    crate::disc::copy_mapped(map, offset, &mut buf[..n])?;
                }
                Ok(n)
            }
        }
    }

    pub(crate) fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = self.read_source(offset, buf)?;
        // The game's checksum covers the body from 0x800 onward.
        if let Some(alias) = &self.alias {
            let start = offset.max(0x198);
            let end = (offset + n as u64).min(0x198 + 32);
            if start < end {
                buf[(start - offset) as usize..(end - offset) as usize]
                    .copy_from_slice(&alias[(start - 0x198) as usize..(end - 0x198) as usize]);
            }
        }
        Ok(n)
    }
}

/// Holds resolved streams while owners have them open and reuses a
/// resolution for successive opens of the same package.
pub(crate) struct ContentFiles {
    maps: ContentMaps,
    library: Library,
    server_maps: Option<PathBuf>,
    files: HashMap<String, Served>,
    open: HashMap<String, usize>,
    removed: std::collections::HashSet<String>,
}

impl ContentFiles {
    pub(crate) fn new(maps: ContentMaps, root: &Path, server_maps: Option<PathBuf>) -> Self {
        Self {
            maps,
            library: Library::open(root),
            server_maps,
            files: HashMap::new(),
            open: HashMap::new(),
            removed: Default::default(),
        }
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        let key = path.to_ascii_lowercase();
        self.open.contains_key(&key) || self.maps.contains_key(&key)
    }

    pub(crate) fn id(&self, path: &str) -> Option<MapId> {
        let key = path.to_ascii_lowercase();
        if self.open.contains_key(&key) {
            return self.files.get(&key).map(Served::id);
        }
        self.maps.get(path).map(|map| map.package.id)
    }

    pub(crate) fn served(&mut self, path: &str) -> Result<&mut Served> {
        let key = path.to_ascii_lowercase();
        if !self.open.contains_key(&key) {
            let map = self.maps.get(path).ok_or(Error::ObjectNameNotFound)?;
            if self
                .files
                .get(&key)
                .is_none_or(|file| !file.map.same_source(&map))
            {
                self.files
                    .insert(key.clone(), Served::new(&self.library, map, self.server_maps.as_deref())?);
            }
        }
        self.files.get_mut(&key).ok_or(Error::ObjectNameNotFound)
    }

    pub(crate) fn opened(&mut self, path: &str) {
        let key = path.to_ascii_lowercase();
        if self.files.contains_key(&key) && self.maps.contains_key(&key) {
            self.removed.remove(&key);
            *self.open.entry(key).or_default() += 1;
        }
    }

    pub(crate) fn closed(&mut self, path: &str) {
        let key = path.to_ascii_lowercase();
        if let Some(count) = self.open.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                self.open.remove(&key);
                self.prune();
            }
        }
    }

    pub(crate) fn changed(&self, path: &str) -> bool {
        let key = path.to_ascii_lowercase();
        !self.open.contains_key(&key)
            && (self.removed.contains(&key)
                || self.files.get(&key).is_some_and(|file| {
                    self.maps
                        .get(path)
                        .is_none_or(|now| !file.map.same_source(&now))
                }))
    }

    pub(crate) fn prune(&mut self) {
        self.files.retain(|path, _| {
            let keep = self.open.contains_key(path) || self.maps.contains_key(path);
            if !keep {
                self.removed.insert(path.clone());
            }
            keep
        });
    }
}
