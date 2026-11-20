//! A backend that serves map files from map packages (docs/game/maps.md,
//! `crates/content`), read-only for those files.
//!
//! [`PackageBackend`] wraps one of two volumes, by its [`Scope`]:
//!
//! - **The disc** ([`PackageBackend::new`]): `maps\<name>.map`, exactly one
//!   level down, for a map the disc has.
//! - **The data partition** ([`PackageBackend::over_map_pack`]): the
//!   Multiplayer Map Pack's maps as `h2 import-mappack` installs them,
//!   `TDATA\<title>\$c\<offer>\<name>.map` for an offer in
//!   [`content::map_pack::OFFERS`]. Every other path, and every write
//!   that is not to a served map, goes to the partition untouched.
//!
//! When the game names such a map and a package in the
//! [`content::Library`] serves that name, the package's result is served in
//! its place: its size in `stat` and in listings of the map's directory,
//! its bytes in `read_at`. The base is the file the volume has at that
//! path (the disc's map, or the installed map pack map). An offset-patched map (PPF, IPS) streams from the base with
//! the patch laid over each read, so nothing is copied; a materialised map
//! (BPS) is read from the cache file. Everything else comes from the inner
//! backend unchanged.
//!
//! Each map is resolved on first use and kept; the choice is reported once
//! through the notice callback. A package that cannot be used (a base that
//! does not match, a damaged patch) is refused with a warning and the
//! volume's own map is served. A served map cannot be written or
//! truncated (`AccessDenied`); nothing is ever written to a package.
//!
//! **Changes while the game runs** (docs/game/maps.md §3, "Realtime: Now"):
//! before a map path is looked up (`stat`, so every open) and before a
//! listing of `maps`, the backend compares a cheap stamp of the maps
//! directory (its own modification time, `active.toml`'s, and each package
//! directory's and manifest's) with the one it scanned. When it differs,
//! or on an explicit [`rescan`](Backend::rescan), the library is scanned
//! again and every map whose chosen package (directory and `MapId`)
//! changed is resolved again at its next use. A map with a file object
//! open keeps what it was serving until its last handle closes, so a
//! re-resolved map takes effect at the next open of that path, never under
//! an open handle. Maps whose choice did not change keep their resolution
//! (no second pass over a 300 MB base).
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use content::{
    BaseRef, Data, Library, MapId,
    patch::{FileSource, Overlay, Source},
};

use crate::{
    Backend, BasicInfo, DirectoryEntry, Error, HostFile, MapOverrideNotice, Metadata, Result,
    VolumeInfo,
    map_override::{is_map_path, resize},
};

enum Served {
    Overlay {
        base: Base,
        overlay: std::sync::Arc<Overlay>,
    },
    File {
        file: FileSource,
        path: std::path::PathBuf,
    },
}
enum Base {
    /// The volume's own map at the path (the disc's, or the installed map
    /// pack map), of this length.
    Disc(u64),
    File(FileSource),
}

struct Entry {
    id: MapId,
    len: u64,
    served: Served,
}

/// One looked-up map path: what the library chose for it then, and what
/// is served.
struct Slot {
    /// The chosen package's directory and `MapId`, None for no package.
    choice: Option<(PathBuf, MapId)>,
    /// The resolved package, or `None` for a map served from the inner
    /// backend (no package, or one that could not be used).
    entry: Option<Entry>,
}

/// What the maps directory looks like, cheaply: modification times and
/// sizes of the directory, `active.toml`, and each package's directory and
/// manifest. Equal stamps mean nothing a scan reads has changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Stamp(Vec<StampEntry>);

/// One stamped path: its name, the directory's mtime, and the file's
/// mtime and size.
type StampEntry = (String, Option<SystemTime>, Option<(SystemTime, u64)>);

fn modified(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl Stamp {
    fn of(root: &Path) -> Self {
        let mut rows = vec![(
            String::new(),
            modified(root).map(|m| m.0),
            modified(&root.join(content::ACTIVE_FILE)),
        )];
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name == content::CACHE_DIR {
                    continue;
                }
                let dir = e.path();
                rows.push((
                    name,
                    modified(&dir).map(|m| m.0),
                    modified(&dir.join(content::manifest::MANIFEST_FILE)),
                ));
            }
        }
        rows.sort();
        Stamp(rows)
    }
}

/// Which map paths of the wrapped volume packages serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The disc: `maps\<name>.map`.
    Disc,
    /// The data partition: the map pack's installed maps,
    /// `TDATA\<title>\$c\<offer>\<name>.map`.
    MapPack {
        /// `tdata\<title as 8 hex digits>\$c\`.
        prefix: String,
        title: u32,
    },
}

impl Scope {
    /// The map pack's maps under `title`'s `T:`.
    pub fn map_pack(title: u32) -> Self {
        Scope::MapPack {
            prefix: format!("tdata\\{title:08x}\\{}\\", crate::CONTENT_DIR),
            title,
        }
    }

    /// The map file name (`turf.map`, as the path has it) when packages
    /// may serve `path`.
    fn map_name<'a>(&self, path: &'a str) -> Option<&'a str> {
        match self {
            Scope::Disc => is_map_path(path),
            Scope::MapPack { prefix, title } => {
                let rest = path
                    .get(..prefix.len())
                    .filter(|p| p.eq_ignore_ascii_case(prefix))
                    .map(|_| &path[prefix.len()..])?;
                let (offer, name) = rest.split_once('\\')?;
                let offer = u64::from_str_radix(offer, 16)
                    .ok()
                    .filter(|_| offer.len() == 16)?;
                if !content::map_pack::is_offer(*title, offer) || name.contains('\\') {
                    return None;
                }
                let ext = name.len().checked_sub(4).map(|i| &name.as_bytes()[i..])?;
                ext.eq_ignore_ascii_case(b".map").then_some(name)
            }
        }
    }

    /// The map's stem, `turf` for `...\turf.map`.
    fn stem<'a>(&self, path: &'a str) -> Option<&'a str> {
        self.map_name(path).map(|n| &n[..n.len() - 4])
    }

    /// Whether a listing of `dir` holds maps packages may serve.
    fn lists_maps(&self, dir: &str) -> bool {
        match self {
            Scope::Disc => dir.eq_ignore_ascii_case("maps"),
            Scope::MapPack { .. } => self.map_name(&format!("{dir}\\x.map")).is_some(),
        }
    }

    /// How notices name the map at `path`.
    fn label(&self, path: &str) -> String {
        match self {
            Scope::Disc => path.to_string(),
            Scope::MapPack { .. } => format!("T:{}", &path[path.find("\\$").unwrap_or(0)..]),
        }
    }

    /// What notices call the volume's own copy.
    fn own(&self) -> &'static str {
        match self {
            Scope::Disc => "the disc's copy",
            Scope::MapPack { .. } => "the installed map pack copy",
        }
    }
}

pub struct PackageBackend<B> {
    inner: B,
    library: Library,
    scope: Scope,
    /// The maps directory as last scanned.
    stamp: Stamp,
    /// By lowercase `maps\<name>.map`.
    files: HashMap<String, Slot>,
    /// Open file objects per lowercase map path.
    open: HashMap<String, usize>,
    /// Paths whose choice changed while open: looked at again when their
    /// last handle closes.
    stale: HashSet<String>,
    /// The choice served at each map's last open.
    last_opened: HashMap<String, Option<(PathBuf, MapId)>>,
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

    pub fn with_notice(inner: B, library: Library, notice: MapOverrideNotice) -> Self {
        Self::scoped(inner, library, Scope::Disc, notice)
    }

    /// Serve `library`'s packages for the map pack's maps installed on
    /// the data partition `inner` (title `title`'s `$c`), reporting to
    /// stderr. Writes to everything else pass through.
    pub fn over_map_pack(inner: B, library: Library, title: u32) -> Self {
        Self::scoped(
            inner,
            library,
            Scope::map_pack(title),
            Box::new(|line: &str| eprintln!("map packages: {line}")),
        )
    }

    /// Serve `library`'s packages for the paths of `scope` over `inner`.
    pub fn scoped(
        inner: B,
        mut library: Library,
        scope: Scope,
        mut notice: MapOverrideNotice,
    ) -> Self {
        for line in library.take_notices() {
            notice(&line);
        }
        Self {
            inner,
            scope,
            stamp: Stamp::of(library.root()),
            library,
            files: HashMap::new(),
            open: HashMap::new(),
            stale: HashSet::new(),
            last_opened: HashMap::new(),
            notice,
        }
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    /// Scan the maps directory again if its stamp changed.
    fn refresh(&mut self) {
        if Stamp::of(self.library.root()) != self.stamp {
            self.reload();
        }
    }

    /// Scan the maps directory again and forget every map whose choice
    /// changed, except those open now (until their last close).
    fn reload(&mut self) {
        self.stamp = Stamp::of(self.library.root());
        self.library = self.library.reopen();
        (self.notice)(&format!(
            "{} changed: {} package(s)",
            self.library.root().display(),
            self.library.packages().len()
        ));
        let keys: Vec<String> = self.files.keys().cloned().collect();
        for key in keys {
            if self.open.contains_key(&key) {
                self.stale.insert(key);
            } else {
                self.forget_if_changed(&key);
            }
        }
        self.flush_notices();
    }

    /// Drop `key`'s resolution if the library now chooses another package
    /// (or none, or one where there was none) for it.
    fn forget_if_changed(&mut self, key: &str) {
        let Some(slot) = self.files.get(key) else {
            return;
        };
        let stem = self.scope.stem(key);
        let now = stem
            .and_then(|s| self.library.choose(s))
            .map(|p| (p.dir.clone(), p.id));
        if now != slot.choice {
            self.files.remove(key);
        }
    }

    /// Report the library's notices, each line once (choosing a map again
    /// to compare repeats its notice).
    fn flush_notices(&mut self) {
        let mut seen = HashSet::new();
        for line in self.library.take_notices() {
            if seen.insert(line.clone()) {
                (self.notice)(&line);
            }
        }
    }

    /// The resolved package for `path`, resolving it on first use.
    fn chosen(&mut self, path: &str) -> Option<&mut Entry> {
        let stem = self.scope.stem(path)?;
        let key = path.to_ascii_lowercase();
        if !self.files.contains_key(&key) {
            let choice = self.library.choose(stem).map(|p| (p.dir.clone(), p.id));
            let entry = choice.as_ref().and_then(|_| self.resolve(path, stem));
            self.flush_notices();
            self.files.insert(key.clone(), Slot { choice, entry });
        }
        self.files.get_mut(&key)?.entry.as_mut()
    }

    fn resolve(&mut self, path: &str, stem: &str) -> Option<Entry> {
        // Only maps the volume has: a missing name stays missing.
        let disc = self.inner.stat(path).ok().filter(|m| !m.is_directory())?;
        let name = self.scope.label(path);
        let own = self.scope.own();
        let mut base = InnerFile {
            inner: &mut self.inner,
            path,
            len: disc.end_of_file,
        };
        let resolved = match self.library.resolve(stem, &mut base) {
            Ok(r) => r?,
            Err(e) => {
                (self.notice)(&format!("warning: {name}: {e}; using {own}"));
                return None;
            }
        };
        let len = resolved.len();
        if len > u64::from(u32::MAX) {
            (self.notice)(&format!(
                "warning: {name}: {len} bytes, over the 4 GiB file limit; using {own}"
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
                            (self.notice)(&format!("warning: {name}: {e}; using {own}"));
                            return None;
                        }
                    },
                };
                let over = if matches!(base, Base::Disc(_)) {
                    match self.scope {
                        Scope::Disc => "the disc",
                        Scope::MapPack { .. } => "the installed map",
                    }
                } else {
                    "its map file"
                };
                (
                    format!("{runs} patched runs over {over}"),
                    Served::Overlay { base, overlay: std::sync::Arc::new(overlay) },
                )
            }
            Data::File { path: p, file, .. } => match FileSource::new(file) {
                Ok(file) => (
                    format!("built at {}", p.display()),
                    Served::File { file, path: p },
                ),
                Err(e) => {
                    (self.notice)(&format!("warning: {name}: {e}; using {own}"));
                    return None;
                }
            },
        };
        (self.notice)(&format!(
            "{name} from package {} (map id {}; {how}; {len} bytes; {} {} bytes)",
            resolved.package.display(),
            resolved.id,
            match self.scope {
                Scope::Disc => "disc",
                Scope::MapPack { .. } => "installed",
            },
            disc.end_of_file
        ));
        Some(Entry {
            id: resolved.id,
            len,
            served,
        })
    }

    /// `AccessDenied` for a path a package serves now: its bytes are the
    /// package's, not the volume's.
    fn refuse_served(&mut self, path: &str) -> Result<()> {
        if self.scope.map_name(path).is_some() && self.chosen(path).is_some() {
            return Err(Error::AccessDenied);
        }
        Ok(())
    }
}

impl<B: Backend> Backend for PackageBackend<B> {
    fn opened(&mut self, path: &str) {
        if self.scope.map_name(path).is_some() {
            let key = path.to_ascii_lowercase();
            *self.open.entry(key.clone()).or_default() += 1;
            self.chosen(path);
            if let Some(slot) = self.files.get(&key) {
                self.last_opened.insert(key, slot.choice.clone());
            }
        }
        self.inner.opened(path)
    }
    fn map_changed(&mut self, path: &str) -> bool {
        let Some(stem) = self.scope.stem(path) else {
            return self.inner.map_changed(path);
        };
        let key = path.to_ascii_lowercase();
        if self.open.contains_key(&key) {
            return false;
        }
        self.refresh();
        let Some(then) = self.last_opened.get(&key).cloned() else {
            return self.inner.map_changed(path);
        };
        let now = self.library.choose(stem).map(|p| (p.dir.clone(), p.id));
        self.flush_notices();
        now != then || self.inner.map_changed(path)
    }
    fn closed(&mut self, path: &str) {
        let key = path.to_ascii_lowercase();
        if let Some(n) = self.open.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                self.open.remove(&key);
                if self.stale.remove(&key) {
                    self.forget_if_changed(&key);
                    self.flush_notices();
                }
            }
        }
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.reload();
        self.inner.rescan()
    }
    /// The `MapId` of the package serving `path` (resolving it if needed),
    /// else whatever the inner backend says.
    fn map_id(&mut self, path: &str) -> Option<MapId> {
        match self.chosen(path) {
            Some(e) => Some(e.id),
            None => self.inner.map_id(path),
        }
    }
    /// The disc is read-only; the data partition stays writable, all
    /// but its served maps.
    fn read_only(&self) -> bool {
        self.inner.read_only()
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        // Every open stats first: the moment to notice a changed folder,
        // unless this map is open now.
        if self.scope.map_name(path).is_some()
            && !self.open.contains_key(&path.to_ascii_lowercase())
        {
            self.refresh();
        }
        let mut meta = self.inner.stat(path)?;
        if let Some(e) = self.chosen(path) {
            let len = e.len;
            resize(&mut meta, len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if !self.scope.lists_maps(path) {
            return Ok(entries);
        }
        self.refresh();
        if self.library.is_empty() {
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
            .and_then(|s| s.entry.as_mut())
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
    fn film_source(&mut self, path: &str) -> Result<Option<crate::FilmMapSource>> {
        let Some(entry) = self.chosen(path) else { return self.inner.film_source(path) };
        let id = entry.id;
        match &entry.served {
            Served::File { file, .. } => Ok(Some(crate::FilmMapSource::region(file.file().try_clone()?, 0))),
            Served::Overlay { base, overlay } => {
                let patch = overlay.clone();
                let (base, len) = match base {
                    Base::Disc(len) => {
                        let len = *len;
                        let Some(source) = self.inner.film_source(path)? else { return Ok(None) };
                        (source, len)
                    }
                    Base::File(file) => (crate::FilmMapSource::region(file.file().try_clone()?, 0), file.size()),
                };
                Ok(Some(crate::FilmMapSource::overlay(base, len, patch, id)))
            }
        }
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
    // Writes: a served map is the package's, read-only; everything else
    // (the data partition's saves, content, the map pack's other files)
    // goes to the volume as it would without packages.
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
        self.refuse_served(path)?;
        self.inner.overwrite_allocated(path, allocation, owner)
    }
    fn release_allocation(&mut self, path: &str, owner: u64) -> Result<()> {
        self.inner.release_allocation(path, owner)
    }
    fn write_at(&mut self, path: &str, offset: u64, buffer: &[u8]) -> Result<usize> {
        self.refuse_served(path)?;
        self.inner.write_at(path, offset, buffer)
    }
    fn set_len(&mut self, path: &str, size: u64) -> Result<()> {
        self.refuse_served(path)?;
        self.inner.set_len(path, size)
    }
    fn set_basic_info(&mut self, path: &str, basic: BasicInfo) -> Result<()> {
        self.inner.set_basic_info(path, basic)
    }
    /// Removing a served map removes the volume's own copy (the base);
    /// the path is resolved again at its next use.
    fn remove(&mut self, path: &str) -> Result<()> {
        self.inner.remove(path)?;
        let key = path.to_ascii_lowercase();
        if self.scope.map_name(path).is_some() && !self.open.contains_key(&key) {
            self.files.remove(&key);
        }
        Ok(())
    }
    fn flush(&mut self, path: &str) -> Result<()> {
        self.inner.flush(path)
    }
    fn format(&mut self) -> Result<()> {
        self.inner.format()?;
        self.files.clear();
        Ok(())
    }
    fn cleanup(&mut self, path: &str, write_access: bool) -> Result<()> {
        self.inner.cleanup(path, write_access)
    }
}
