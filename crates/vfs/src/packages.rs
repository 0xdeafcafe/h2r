//! A read-only backend that serves `maps\*.map` from map packages
//! (docs/maps.md, `crates/content`).
//!
//! [`PackageBackend`] wraps the disc. When the game names a map the disc has
//! (`maps\<name>.map`, exactly one level down) and a package in the
//! [`content::Library`] serves that name, the package's result is served in
//! its place: its size in `stat` and in listings of `maps`, its bytes in
//! `read_at`. An offset-patched map (PPF, IPS) streams from the base with
//! the patch laid over each read, so nothing is copied; a materialised map
//! (BPS) is read from the cache file. Everything else comes from the inner
//! backend unchanged.
//!
//! Each map is resolved once, on first use, and kept for the session; the
//! choice is reported once through the notice callback. A package that
//! cannot be used (a base that does not match, a damaged patch) is refused
//! with a warning and the disc's map is served. Nothing is ever written
//! through the VFS.
use std::{collections::HashMap, io};

use content::{
    BaseRef, Data, Library, MapId,
    patch::{FileSource, Overlay, Source},
};

use crate::{
    Backend, DirectoryEntry, Error, HostFile, MapOverrideNotice, Metadata, Result, VolumeInfo,
    map_override::{is_map_path, resize},
};

enum Served {
    Overlay {
        base: Base,
        overlay: Overlay,
    },
    File {
        file: FileSource,
        path: std::path::PathBuf,
    },
}
enum Base {
    /// The disc's map, of this length.
    Disc(u64),
    File(FileSource),
}

struct Entry {
    id: MapId,
    len: u64,
    served: Served,
}

pub struct PackageBackend<B> {
    inner: B,
    library: Library,
    /// By lowercase `maps\<name>.map`: the resolved package, or `None` for
    /// a map served from the inner backend.
    files: HashMap<String, Option<Entry>>,
    notice: MapOverrideNotice,
}

/// The inner backend's file at `path` as a [`Source`].
struct InnerFile<'a, B> {
    inner: &'a mut B,
    path: &'a str,
    len: u64,
}
impl<B: Backend> Source for InnerFile<'_, B> {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.inner
            .read_at(self.path, offset, buf)
            .map_err(|e| io::Error::other(format!("{e:?}")))
    }
}

fn io_error(e: io::Error) -> Error {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => Error::FileCorrupt,
        _ => e.into(),
    }
}

impl<B: Backend> PackageBackend<B> {
    /// Serve `library`'s packages over `inner`, reporting to stderr.
    pub fn new(inner: B, library: Library) -> Self {
        Self::with_notice(
            inner,
            library,
            Box::new(|line: &str| eprintln!("map packages: {line}")),
        )
    }

    pub fn with_notice(inner: B, mut library: Library, mut notice: MapOverrideNotice) -> Self {
        for line in library.take_notices() {
            notice(&line);
        }
        Self {
            inner,
            library,
            files: HashMap::new(),
            notice,
        }
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    /// The resolved package for `path`, resolving it on first use.
    fn chosen(&mut self, path: &str) -> Option<&mut Entry> {
        let name = is_map_path(path)?;
        let key = path.to_ascii_lowercase();
        if !self.files.contains_key(&key) {
            let entry = self.resolve(path, name);
            for line in self.library.take_notices() {
                (self.notice)(&line);
            }
            self.files.insert(key.clone(), entry);
        }
        self.files.get_mut(&key)?.as_mut()
    }

    fn resolve(&mut self, path: &str, name: &str) -> Option<Entry> {
        // Only maps the disc has: a missing name stays missing.
        let disc = self.inner.stat(path).ok().filter(|m| !m.is_directory())?;
        let stem = &name[..name.len() - 4];
        let mut base = InnerFile {
            inner: &mut self.inner,
            path,
            len: disc.end_of_file,
        };
        let resolved = match self.library.resolve(stem, &mut base) {
            Ok(r) => r?,
            Err(e) => {
                (self.notice)(&format!(
                    "warning: maps\\{name}: {e}; using the disc's copy"
                ));
                return None;
            }
        };
        let len = resolved.len();
        if len > u64::from(u32::MAX) {
            (self.notice)(&format!(
                "warning: maps\\{name}: {len} bytes, over the disc's 4 GiB file limit; using the disc's copy"
            ));
            return None;
        }
        let (how, served) = match resolved.data {
            Data::Overlay { base, overlay } => {
                let runs = overlay.runs().len();
                let base = match base {
                    BaseRef::Disc => Base::Disc(disc.end_of_file),
                    BaseRef::File { file, .. } => match FileSource::new(file) {
                        Ok(f) => Base::File(f),
                        Err(e) => {
                            (self.notice)(&format!(
                                "warning: maps\\{name}: {e}; using the disc's copy"
                            ));
                            return None;
                        }
                    },
                };
                let over = if matches!(base, Base::Disc(_)) {
                    "the disc"
                } else {
                    "its map file"
                };
                (
                    format!("{runs} patched runs over {over}"),
                    Served::Overlay { base, overlay },
                )
            }
            Data::File { path: p, file, .. } => match FileSource::new(file) {
                Ok(file) => (
                    format!("built at {}", p.display()),
                    Served::File { file, path: p },
                ),
                Err(e) => {
                    (self.notice)(&format!(
                        "warning: maps\\{name}: {e}; using the disc's copy"
                    ));
                    return None;
                }
            },
        };
        (self.notice)(&format!(
            "maps\\{name} from package {} (map id {}; {how}; {len} bytes; disc {} bytes)",
            resolved.package.display(),
            resolved.id,
            disc.end_of_file
        ));
        Some(Entry {
            id: resolved.id,
            len,
            served,
        })
    }

}

impl<B: Backend> Backend for PackageBackend<B> {
    /// The `MapId` of the package serving `path` (resolving it if needed),
    /// else whatever the inner backend says.
    fn map_id(&mut self, path: &str) -> Option<MapId> {
        match self.chosen(path) {
            Some(e) => Some(e.id),
            None => self.inner.map_id(path),
        }
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let mut meta = self.inner.stat(path)?;
        if let Some(e) = self.chosen(path) {
            let len = e.len;
            resize(&mut meta, len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if self.library.is_empty() || !path.eq_ignore_ascii_case("maps") {
            return Ok(entries);
        }
        for e in &mut entries {
            if e.metadata.is_directory() {
                continue;
            }
            if let Some(o) = self.chosen(&format!("{path}\\{}", e.name)) {
                let len = o.len;
                resize(&mut e.metadata, len);
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if self.chosen(path).is_none() {
            return self.inner.read_at(path, offset, buffer);
        }
        let Self { inner, files, .. } = self;
        let entry = files
            .get_mut(&path.to_ascii_lowercase())
            .and_then(Option::as_mut)
            .expect("chosen above");
        match &mut entry.served {
            Served::File { file, .. } => file.read_at(offset, buffer).map_err(io_error),
            Served::Overlay { base, overlay } => match base {
                Base::Disc(len) => {
                    let mut src = InnerFile {
                        inner,
                        path,
                        len: *len,
                    };
                    overlay.read_at(&mut src, offset, buffer)
                }
                Base::File(f) => overlay.read_at(f, offset, buffer),
            }
            .map_err(io_error),
        }
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    /// A materialised map is all of its cache file. An overlaid map is not
    /// one region of any host file, so it has none.
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        if let Some(e) = self.chosen(path) {
            return match &e.served {
                Served::File { path, .. } => Some(HostFile {
                    path: path.clone(),
                    offset: 0,
                    len: e.len,
                }),
                Served::Overlay { .. } => None,
            };
        }
        self.inner.host_file(path)
    }
}
