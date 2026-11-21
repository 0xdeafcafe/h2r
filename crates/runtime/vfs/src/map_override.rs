//! A read-only backend that serves `maps\*.map` from a host directory.
//!
//! [`MapOverrideBackend`] wraps the disc. When the game names a map the disc
//! has (`maps\<name>.map`, exactly one level down) and the override directory
//! holds a file of the same name (ASCII case-insensitive, like the disc), that
//! file is served instead: its size in `stat` and in listings of `maps`, its
//! bytes in `read_at`, its path in `host_file`. Reads stream from the host
//! file; nothing is buffered, so a map of any size up to the 4 GiB XDVDFS
//! limit works. Everything else, and every map with no override, comes from
//! the inner backend unchanged.
//!
//! A directory override is chosen on first use. A writable host can also
//! bind a prepared file to a map for a reload. The current file stays pinned
//! until the last guest file object closes. Each choice is reported once. A symlink
//! that leaves the directory, anything that is not a regular file, an
//! ambiguous name (two host names differing only in case), a file too large
//! for XDVDFS, or one that cannot be opened is refused with a warning, and
//! the disc's map is served. Explicit reloads use read-only private copies. See docs/runtime/storage.md
//! ("Map overrides") and docs/editor/scenario-editor.md.
mod generation;

pub use generation::MapGeneration;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU8, Ordering}},
};

use crate::{Backend, DirectoryEntry, Error, HostFile, Metadata, Result, VolumeInfo};
use content::map;

/// Receives one line per decision: which file serves a map, or why an
/// override was refused.
pub type MapOverrideNotice = Box<dyn FnMut(&str) + Send>;

struct Override {
    file: File,
    path: PathBuf,
    len: u64,
    mapping: Option<crate::disc::MappedDisc>,
    generation: Option<MapGeneration>,
    /// Its whole-file derived identity.
    id: Option<content::MapId>,
}

impl Override {
    fn make_private(&mut self, slot: &str, cancelled: &AtomicBool) -> std::result::Result<u32, String> {
        let name = self.path.file_name().ok_or("map path has no filename")?;
        let (file, generation, hash, checksum) = MapGeneration::copy(&mut self.file, self.len, name, cancelled)
            .map_err(|e| format!("copying {} into private map storage: {e}", self.path.display()))?;
        self.file = file;
        self.generation = Some(generation);
        self.id = Some(override_id_from_hash(slot, hash));
        Ok(checksum)
    }

    fn host_file(&self) -> Option<HostFile> {
        let path = self
            .generation
            .as_ref()
            .map_or(self.path.as_path(), MapGeneration::path);
        Some(HostFile {
            path: path.to_owned(),
            offset: 0,
            len: self.len,
            generation: self.generation.clone(),
        })
    }
}

/// Supplies the DVD override layer with an explicitly selected reload file.
#[derive(Clone, Default)]
pub struct MapRebind {
    shared: Arc<SharedRebind>,
}

#[derive(Default)]
struct SharedRebind {
    state: Mutex<RebindState>,
    // Updated under state on publication, reservation and cancellation. A
    // file still being prepared has no effect on any DVD read or open.
    flags: AtomicU8,
}

const PREPARING: u8 = 1;
const RESERVED: u8 = 2;
const BOUND: u8 = 4;

#[derive(Default)]
struct RebindState {
    requests: HashMap<String, Request>,
    bound: HashMap<String, Bound>,
    shutdown: bool,
    attached: bool,
}

struct Bound {
    file: Override,
    changed: bool,
}

struct Request {
    cancelled: Arc<AtomicBool>,
    reserved: bool,
}

impl Request {
    // Once the guest options call starts, the handoff owns the reservation.
    // Until then dropping the RPC revokes the worker's claim immediately.
    fn live(&self) -> bool {
        self.reserved || !self.cancelled.load(Ordering::Acquire)
    }
}

impl SharedRebind {
    fn update_flags(&self, state: &RebindState) {
        // External Live remains blocked once any file has been published,
        // even if a service callback outlives this boot and its mounted files.
        let mut flags = self.flags.load(Ordering::Relaxed) & BOUND;
        if !state.bound.is_empty() {
            flags |= BOUND;
        }
        for request in state.requests.values() {
            flags |= if request.reserved { RESERVED } else { PREPARING };
        }
        self.flags.store(flags, Ordering::Release);
    }
}

/// Holds the validated descriptor until the game accepts its options change.
/// Dropping the preparation releases its reservation without changing the DVD.
pub struct PreparedMapRebind {
    handle: MapRebind,
    key: String,
    file: Option<Override>,
    checksum: u32,
    cancelled: Arc<AtomicBool>,
}

impl MapRebind {
    /// Install the editor layer on this boot's DVD at its first file request.
    /// Preserve the mount index: open file objects already refer to it.
    /// Returns true on the first attachment.
    pub fn attach(&self, vfs: &mut crate::Vfs) -> std::result::Result<bool, String> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Err("the DVD mount has shut down".into());
        }
        if state.attached {
            return Ok(false);
        }
        let index = vfs.mounts.iter()
            .position(|m| m.device.eq_ignore_ascii_case(crate::CDROM_DEVICE))
            .ok_or("the DVD is not mounted")?;
        let mut mount = vfs.mounts.remove(index);
        mount.backend = Box::new(MapOverrideBackend::from_parts(
            mount.backend,
            None,
            Some(self.clone()),
            Box::new(|line| eprintln!("map override: {line}")),
        ));
        vfs.mounts.insert(index, mount);
        state.attached = true;
        Ok(true)
    }

    /// Bindings stay mounted until the process exits.
    pub fn has_ever_bound(&self) -> bool {
        self.shared.flags.load(Ordering::Acquire) & BOUND != 0
    }

    /// A reserved reload can already be inside the retail options call.
    pub fn blocks_external_live(&self) -> bool {
        self.shared.flags.load(Ordering::Acquire) & (RESERVED | BOUND) != 0
    }

    /// Discard the mount's references even if a callback outlives the boot.
    /// An editor worker that finishes after shutdown cannot publish its copy.
    pub fn clear_on_shutdown(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutdown = true;
        state.requests.clear();
        let old = std::mem::take(&mut state.bound);
        self.shared.update_flags(&state);
        drop(state);
        drop(old);
    }

    fn snapshot_bindings(&self) -> std::result::Result<Vec<(String, String)>, crate::StateError> {
        if self.shared.flags.load(Ordering::Acquire) == 0 {
            return Ok(Vec::new());
        }
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.requests.values().any(Request::live) {
            return Err(crate::StateError::MapReloadBusy);
        }
        let mut bindings: Vec<_> = state.bound.iter().map(|(key, bound)| {
            let id = bound.file.id.expect("validated map identity");
            (key.clone(), id.to_string())
        }).collect();
        bindings.sort_unstable();
        Ok(bindings)
    }

    /// Prepare a replacement for DVD `maps\<slot>.map`. A slot can have a
    /// different name from the scenario it carries (for example a test map).
    pub fn prepare(
        &self,
        slot: &str,
        map: &str,
        file: &Path,
        cache_type: map::CacheType,
        capacity: u32,
    ) -> std::result::Result<PreparedMapRebind, String> {
        self.prepare_cancellable(slot, map, file, cache_type, capacity, Arc::default())
    }

    /// The RPC that requested the file can revoke its claim while the host
    /// is copying, hashing or waiting for the game tick to reserve the slot.
    pub fn prepare_cancellable(
        &self,
        slot: &str,
        map: &str,
        file: &Path,
        cache_type: map::CacheType,
        capacity: u32,
        cancelled: Arc<AtomicBool>,
    ) -> std::result::Result<PreparedMapRebind, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("the map preparation was cancelled".into());
        }
        if slot.is_empty() || slot == "." || slot == ".." || slot.contains(['/', '\\', ':', '\0']) {
            return Err("invalid DVD map slot".into());
        }
        if map.is_empty() {
            return Err("the active map has no name".into());
        }
        if !file.is_absolute() {
            return Err("reload file must have an absolute path".into());
        }
        let name = file.file_name().and_then(|s| s.to_str());
        if !name.is_some_and(|s| s.eq_ignore_ascii_case(&format!("{map}.map"))) {
            return Err(format!("reload file must be named {map}.map"));
        }
        let slot = slot.to_ascii_lowercase();
        let key = format!("maps\\{slot}.map");
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Err("the DVD mount has shut down".into());
        }
        if state.requests.get(&key).is_some_and(Request::live) {
            return Err(format!("a reload of {slot} is already pending"));
        }
        state.requests.insert(key.clone(), Request {
            cancelled: Arc::clone(&cancelled),
            reserved: false,
        });
        self.shared.update_flags(&state);
        drop(state);
        let mut prepared = PreparedMapRebind {
            handle: self.clone(),
            key,
            file: None,
            checksum: 0,
            cancelled,
        };
        let mut replacement = open_override(file.to_owned())?;
        if replacement.len > u64::from(capacity) {
            return Err(format!("{} bytes exceed the {capacity}-byte active cache slot", replacement.len));
        }
        let checksum = replacement.make_private(&slot, &prepared.cancelled)?;
        let parsed = map::Map::from_region(&mut replacement.file, 0, replacement.len)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        if parsed.scenario().is_none() || parsed.globals().is_none() {
            return Err("the map index is missing its scenario or globals tag".into());
        }
        let header = parsed.header();
        if header.cache_type != cache_type {
            return Err(format!("expected a {cache_type:?} map, found {:?}", header.cache_type));
        }
        if !header.name.eq_ignore_ascii_case(map) {
            return Err(format!("reload file names map {:?}, expected {map}", header.name));
        }
        if header.checksum != checksum {
            return Err("the map payload does not match the copy-time checksum in its header".into());
        }
        if prepared.cancelled.load(Ordering::Acquire) {
            return Err("the map preparation was cancelled".into());
        }
        prepared.file = Some(replacement);
        prepared.checksum = checksum;
        Ok(prepared)
    }
}

impl PreparedMapRebind {
    /// The checksum the game's public stamp reports after loading this header.
    pub fn checksum(&self) -> u32 {
        self.checksum
    }

    /// The existing VFS file-object table includes opens that predate this
    /// request and resolves aliases to the same DVD mount and lowercase key.
    /// The caller holds the VFS lock through this check and the reservation.
    pub fn reserve(self, vfs: &crate::Vfs) -> std::result::Result<ReservedMapRebind, String> {
        if vfs.objects.values().any(|f| {
            f.key == self.key && vfs.mounts[f.mount].device.eq_ignore_ascii_case(crate::CDROM_DEVICE)
        }) {
            return Err(format!("the DVD source {} is still open", self.key));
        }
        let shared = &self.handle.shared;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Err("the DVD mount has shut down".into());
        }
        let request = state.requests.get_mut(&self.key)
            .filter(|r| Arc::ptr_eq(&r.cancelled, &self.cancelled) && r.live())
            .ok_or("the map preparation was cancelled")?;
        request.reserved = true;
        shared.update_flags(&state);
        drop(state);
        Ok(ReservedMapRebind(self))
    }
}

/// Owns the reservation until the guest accepts the queue or it is cancelled.
pub struct ReservedMapRebind(PreparedMapRebind);

impl ReservedMapRebind {
    /// Publish the reserved file. Dropping the old override here releases
    /// the mount's reference even if nobody asks for this map again.
    pub fn commit(self) {
        let mut prepared = self.0;
        let file = prepared.file.take().expect("prepared map file");
        let shared = &prepared.handle.shared;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return;
        }
        state.requests.remove(&prepared.key);
        eprintln!("map override: {} rebound to {} ({} bytes)", prepared.key, file.path.display(), file.len);
        let old = state.bound.insert(prepared.key.clone(), Bound { file, changed: true });
        shared.update_flags(&state);
        drop(state);
        drop(old);
    }
}

impl Drop for PreparedMapRebind {
    fn drop(&mut self) {
        let shared = &self.handle.shared;
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.requests.get(&self.key).is_some_and(|r| Arc::ptr_eq(&r.cancelled, &self.cancelled)) {
            state.requests.remove(&self.key);
            shared.update_flags(&state);
        }
    }
}

pub struct MapOverrideBackend<B: ?Sized> {
    inner: Box<B>,
    /// Canonical directory used for startup overrides, when configured.
    dir: Option<PathBuf>,
    /// By lowercase `maps\<name>.map`: the chosen override, or `None` for a
    /// map served from the inner backend.
    files: HashMap<String, Option<Override>>,
    notice: MapOverrideNotice,
    server_maps: bool,
    rebind: Option<MapRebind>,
}

/// Whether `path` names a map directly under `maps` (any case).
pub(crate) fn is_map_path(path: &str) -> Option<&str> {
    let (dir, name) = path.split_once('\\')?;
    if !dir.eq_ignore_ascii_case("maps") || name.contains('\\') {
        return None;
    }
    let ext = name.len().checked_sub(4).map(|i| &name.as_bytes()[i..])?;
    ext.eq_ignore_ascii_case(b".map").then_some(name)
}

impl<B: Backend> MapOverrideBackend<B> {
    /// Serve maps from `dir` over `inner`. A directory that does not exist or
    /// cannot be resolved is reported through the notice and ignored.
    pub fn new(inner: B, dir: impl AsRef<Path>) -> Self {
        Self::with_notice(
            inner,
            dir,
            Box::new(|line: &str| eprintln!("map override: {line}")),
        )
    }

    /// A light-host copy must have the original header and length. This
    /// directory is operator-provisioned with verified immutable maps.
    pub fn server_maps(inner: B, dir: impl AsRef<Path>) -> Self {
        let mut backend = Self::new(inner, dir);
        backend.server_maps = true;
        backend
    }

    pub fn with_notice(inner: B, dir: impl AsRef<Path>, notice: MapOverrideNotice) -> Self {
        Self::from_parts(Box::new(inner), Some(dir.as_ref()), None, notice)
    }

    /// Mount a writable host's reload layer, with optional startup overrides.
    pub fn rebindable(inner: B, dir: Option<&Path>, handle: MapRebind) -> Self {
        Self::from_parts(
            Box::new(inner),
            dir,
            Some(handle),
            Box::new(|line| eprintln!("map override: {line}")),
        )
    }

    pub fn into_inner(self) -> B {
        *self.inner
    }
}

impl<B: Backend + ?Sized> MapOverrideBackend<B> {
    fn from_parts(
        inner: Box<B>,
        dir: Option<&Path>,
        rebind: Option<MapRebind>,
        mut notice: MapOverrideNotice,
    ) -> Self {
        let dir = dir.and_then(|dir| match dir.canonicalize() {
            Ok(d) if d.is_dir() => Some(d),
            Ok(_) => {
                notice(&format!(
                    "warning: {} is not a directory; using the disc's maps",
                    dir.display()
                ));
                None
            }
            Err(e) => {
                notice(&format!(
                    "warning: cannot use {} ({e}); using the disc's maps",
                    dir.display()
                ));
                None
            }
        });
        Self {
            inner,
            dir,
            files: HashMap::new(),
            notice,
            server_maps: false,
            rebind,
        }
    }

    /// Ordinary DVD access only tests the atomic after a path matches a map.
    fn binding<T>(&self, path: &str, f: impl FnOnce(&mut RebindState, &str) -> Option<T>) -> Option<T> {
        let handle = self.rebind.as_ref()?;
        is_map_path(path)?;
        if handle.shared.flags.load(Ordering::Acquire) & (RESERVED | BOUND) == 0 {
            return None;
        }
        let mut state = handle.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut state, &path.to_ascii_lowercase())
    }

    fn with_override<T>(&mut self, path: &str, mut f: impl FnMut(&mut Override) -> T) -> Option<T> {
        self.binding(path, |state, key| Some(f(&mut state.bound.get_mut(key)?.file)))
            .or_else(|| self.startup(path).map(f))
    }

    /// The original directory override, opened and kept on its first lookup.
    fn startup(&mut self, path: &str) -> Option<&mut Override> {
        let name = is_map_path(path)?;
        let dir = self.dir.as_ref()?;
        let key = path.to_ascii_lowercase();
        if !self.files.contains_key(&key) {
            // Only maps the disc has: a missing name stays missing.
            let on_disc = self.inner.stat(path).ok().filter(|m| !m.is_directory());
            let chosen = match on_disc {
                Some(disc) => match find(dir, name) {
                    Ok(None) => None,
                    Ok(Some(mut o)) => {
                        if self.server_maps {
                            let mut header = [0; 0x800];
                            let n = self.inner.read_at(path, 0, &mut header).ok();
                            if n != Some(header.len())
                                || !matching_server_copy(
                                    &mut o.file,
                                    o.len,
                                    disc.end_of_file,
                                    &header,
                                )
                            {
                                (self.notice)(&format!(
                                    "warning: {name}: server copy header or length differs"
                                ));
                                self.files.insert(key, None);
                                return None;
                            }
                            o.mapping = crate::disc::MappedDisc::new(&o.file);
                        }
                        (self.notice)(&format!(
                            "maps\\{name} from {} ({} bytes; disc {} bytes)",
                            o.path.display(),
                            o.len,
                            disc.end_of_file
                        ));
                        Some(o)
                    }
                    Err(why) => {
                        (self.notice)(&format!(
                            "warning: maps\\{name}: {why}; using the disc's copy"
                        ));
                        None
                    }
                },
                None => None,
            };
            self.files.insert(key.clone(), chosen);
        }
        self.files.get_mut(&key)?.as_mut()
    }
}

/// `meta` for a file of `len` bytes: its size, sector-rounded allocation.
pub(crate) fn resize(meta: &mut Metadata, len: u64) {
    meta.end_of_file = len;
    meta.allocation_size = len.div_ceil(xiso::SECTOR) * xiso::SECTOR;
}

/// The host file in `dir` named `name` (ASCII case-insensitive), checked and
/// opened. `Ok(None)` when there is none; `Err` with a reason when there is
/// one that cannot be used.
fn find(dir: &Path, name: &str) -> std::result::Result<Option<Override>, String> {
    let mut found = None;
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        let host = entry.file_name();
        if host.to_str().is_some_and(|h| h.eq_ignore_ascii_case(name)) {
            if found.is_some() {
                return Err(format!(
                    "{} holds more than one file named {name} in different case",
                    dir.display()
                ));
            }
            found = Some(entry.path());
        }
    }
    let Some(path) = found else { return Ok(None) };
    let link = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let target = if link.file_type().is_symlink() {
        let target = path
            .canonicalize()
            .map_err(|e| format!("{}: broken link ({e})", path.display()))?;
        if !target.starts_with(dir) {
            return Err(format!(
                "{} links outside {} (to {})",
                path.display(),
                dir.display(),
                target.display()
            ));
        }
        target
    } else {
        path
    };
    open_override(target).map(Some)
}

/// Open first and use this descriptor for validation, identity and all reads.
fn open_override(target: PathBuf) -> std::result::Result<Override, String> {
    let checked = std::fs::metadata(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    if !checked.is_file() {
        return Err(format!("{} is not a regular file", target.display()));
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        // Opening a named pipe for reading must not wait for a writer.
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(&target).map_err(|e| format!("cannot open {}: {e}", target.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{}: {e}", target.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", target.display()));
    }
    #[cfg(unix)]
    {
        // The object opened must be the one checked, not a later swap.
        use std::os::unix::fs::MetadataExt;
        if (checked.dev(), checked.ino()) != (meta.dev(), meta.ino()) {
            return Err(format!("{} changed while it was opened", target.display()));
        }
    }
    if meta.len() > u64::from(u32::MAX) {
        return Err(format!(
            "{} is {} bytes, over the disc's 4 GiB file limit",
            target.display(),
            meta.len()
        ));
    }
    Ok(Override {
        file,
        path: target,
        len: meta.len(),
        mapping: None,
        generation: None,
        id: None,
    })
}

pub(crate) fn matching_server_copy(
    file: &mut File,
    size: u64,
    original_size: u64,
    original_header: &[u8; 0x800],
) -> bool {
    let mut header = [0; 0x800];
    size == original_size
        && original_header.starts_with(b"daeh")
        && file.read_exact(&mut header).is_ok()
        && &header == original_header
}

/// The `MapId` of a whole-file override of map `map`.
pub(crate) fn override_id(map: &str, file: &Path) -> std::io::Result<content::MapId> {
    let mut f = File::open(file)?;
    let len = f.metadata()?.len();
    opened_override_id(map, &mut f, len)
}

fn opened_override_id(map: &str, file: &mut File, mut len: u64) -> std::io::Result<content::MapId> {
    file.rewind()?;
    let mut h = content::Hasher::default();
    let mut buf = vec![0; 1 << 20];
    while len > 0 {
        let n = len.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..n])?;
        h.update(&buf[..n]);
        len -= n as u64;
    }
    Ok(override_id_from_hash(map, h.finish()))
}

pub(crate) fn override_id_from_hash(map: &str, sha256: content::Sha256) -> content::MapId {
    let manifest = content::Manifest {
        package: content::manifest::Package {
            format: content::manifest::FORMAT,
            name: map.to_owned(),
            author: None,
            description: None,
            image: None,
        },
        base: content::manifest::Base {
            map: map.to_owned(),
            sha256: None,
            derived: true,
        },
        patches: vec![content::manifest::Layer {
            file: format!("{map}.map"),
            sha256,
        }],
        packs: Vec::new(),
        forge: Vec::new(),
        result: None,
        list: None,
        variants: Vec::new(),
    };
    content::MapId::of(&manifest)
}

impl<B: Backend + ?Sized> Backend for MapOverrideBackend<B> {
    fn snapshot_map_bindings(&self) -> std::result::Result<Vec<(String, String)>, crate::StateError> {
        let mut bindings = self.inner.snapshot_map_bindings()?;
        if let Some(handle) = &self.rebind {
            bindings.extend(handle.snapshot_bindings()?);
        }
        Ok(bindings)
    }

    fn opened(&mut self, path: &str) {
        self.binding(path, |state, key| {
            state.bound.get_mut(key)?.changed = false;
            Some(())
        });
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    fn resolves_on_use(&mut self, path: &str) -> bool {
        self.binding(path, |state, key| state.bound.get(key).map(|_| ())).is_none()
            && self.inner.resolves_on_use(path)
    }
    fn map_changed(&mut self, path: &str) -> bool {
        if let Some(changed) = self.binding(path, |state, key| Some(state.bound.get(key)?.changed)) {
            return changed;
        }
        self.startup(path).is_none() && self.inner.map_changed(path)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let published = self.binding(path, |state, key| {
            if state.requests.get(key).is_some_and(|r| r.reserved) {
                return Some(Err(Error::SharingViolation));
            }
            Some(Ok(state.bound.get(key)?.file.len))
        }).transpose()?;
        let mut meta = self.inner.stat(path)?;
        if let Some(len) = published.or_else(|| self.startup(path).map(|o| o.len)) {
            resize(&mut meta, len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if !path.eq_ignore_ascii_case("maps") {
            return Ok(entries);
        }
        if self.dir.is_none() && !self.rebind.as_ref().is_some_and(MapRebind::has_ever_bound) {
            return Ok(entries);
        }
        for entry in &mut entries {
            if entry.metadata.is_directory() {
                continue;
            }
            let key = format!("{path}\\{}", entry.name);
            if let Some(len) = self.with_override(&key, |file| file.len) {
                resize(&mut entry.metadata, len);
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let served = self.with_override(path, |o| {
            let n = o.len.saturating_sub(offset).min(buffer.len() as u64) as usize;
            if n == 0 {
                return Ok(0);
            }
            if let Some(map) = &o.mapping {
                map.read(offset, &mut buffer[..n])?;
            } else {
                o.file.seek(SeekFrom::Start(offset))?;
                if let Err(e) = o.file.read_exact(&mut buffer[..n]) {
                    return Err(if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        Error::FileCorrupt
                    } else {
                        e.into()
                    });
                }
            }
            Ok(n)
        });
        served.unwrap_or_else(|| self.inner.read_at(path, offset, buffer))
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    fn film_source(&mut self, path: &str) -> Result<Option<crate::FilmMapSource>> {
        let derived = !self.server_maps;
        let source = self.with_override(path, |o| {
            let file = o.file.try_clone()?;
            let mut source = if derived {
                crate::FilmMapSource::region(file, 0)
            } else {
                crate::FilmMapSource::immutable_region(file, 0, o.len)
            };
            if derived && let Some(name) = is_map_path(path) {
                source.override_name = Some(name[..name.len()-4].to_ascii_lowercase());
            }
            Ok(Some(source))
        });
        source.unwrap_or_else(|| self.inner.film_source(path))
    }
    fn map_ranges(&mut self, path: &str) -> Option<Vec<(u64, u64)>> {
        if self.with_override(path, |_| ()).is_some() {
            None
        } else {
            self.inner.map_ranges(path)
        }
    }
    /// An overridden map is a user map: the identity of a derived manifest
    /// whose one layer is the whole file, so a local edit is never taken
    /// for retail. Rebinds hash their private copy during preparation;
    /// startup directory overrides hash on the first identity lookup.
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        if self.server_maps {
            return self.inner.map_id(path);
        }
        let Some(name) = is_map_path(path) else {
            return self.inner.map_id(path);
        };
        self.with_override(path, |o| {
            if o.id.is_none() {
                let map = name[..name.len() - 4].to_ascii_lowercase();
                o.id = Some(opened_override_id(&map, &mut o.file, o.len).ok()?);
            }
            o.id
        }).unwrap_or_else(|| self.inner.map_id(path))
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        self.with_override(path, |file| file.host_file())
            .unwrap_or_else(|| self.inner.host_file(path))
    }
    fn file_source(&mut self, path: &str) -> Option<crate::FileSource> {
        self.with_override(path, |file| file.host_file().map(crate::FileSource::Host))
            .unwrap_or_else(|| self.inner.file_source(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ATTR_DIRECTORY, ATTR_READONLY, BasicInfo};
    use std::sync::{Arc, Mutex};

    /// A disc with `maps\a.map`, `maps\b.map`, `maps\notes.txt` and `default.xbe`.
    struct Disc;
    fn meta(len: u64, dir: bool) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if dir { ATTR_DIRECTORY } else { 0 },
                ..Default::default()
            },
            end_of_file: len,
            allocation_size: len.div_ceil(2048) * 2048,
        }
    }
    const FILES: [(&str, &[u8]); 4] = [
        ("maps\\a.map", b"disc-a"),
        ("maps\\b.map", b"disc-bb"),
        ("maps\\notes.map.txt", b"t"),
        ("default.xbe", b"xbe"),
    ];
    impl Backend for Disc {
        fn read_only(&self) -> bool {
            true
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            if path.is_empty() || path.eq_ignore_ascii_case("maps") {
                return Ok(meta(0, true));
            }
            FILES
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .map(|f| meta(f.1.len() as u64, false))
                .ok_or(Error::ObjectNameNotFound)
        }
        fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(FILES
                .iter()
                .filter_map(|(p, d)| {
                    let (dir, name) = p.rsplit_once('\\').unwrap_or(("", p));
                    dir.eq_ignore_ascii_case(path).then(|| DirectoryEntry {
                        name: name.to_owned(),
                        metadata: meta(d.len() as u64, false),
                    })
                })
                .collect())
        }
        fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            let f = FILES
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .ok_or(Error::ObjectNameNotFound)?;
            let start = (offset as usize).min(f.1.len());
            let n = (f.1.len() - start).min(buffer.len());
            buffer[..n].copy_from_slice(&f.1[start..start + n]);
            Ok(n)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
        fn host_file(&mut self, path: &str) -> Option<HostFile> {
            Some(HostFile {
                generation: None,
                path: PathBuf::from("/disc.iso"),
                offset: 7,
                len: self.stat(path).ok()?.end_of_file,
            })
        }
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = base.join(format!("h2-map-override-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("maps")).unwrap();
            Self(p)
        }
        fn maps(&self) -> PathBuf {
            self.0.join("maps")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn backend(dir: &Path) -> (MapOverrideBackend<Disc>, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&lines);
        let b = MapOverrideBackend::with_notice(
            Disc,
            dir,
            Box::new(move |l: &str| log.lock().unwrap().push(l.to_owned())),
        );
        (b, lines)
    }
    fn read_all(b: &mut impl Backend, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4];
        loop {
            match b.read_at(path, out.len() as u64, &mut buf).unwrap() {
                0 => return out,
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    // The map writer emits a complete synthetic index and two local tags.
    fn play_copy(name: &str, payload: u8) -> Vec<u8> {
        use map::build::{Block, TagPayload, TagSet};
        let mut set = TagSet::new(map::CacheType::Multiplayer, name, "fixture");
        let globals = map::FourCC(*b"matg");
        let scenario = map::FourCC(*b"scnr");
        set.add_group(globals, None, None);
        set.add_group(scenario, None, None);
        let mut data = Block::zeroed(4);
        data.put_u32(0, u32::from(payload));
        set.globals = set.add_tag(globals, "globals", TagPayload::Built(data)).unwrap();
        set.scenario = set.add_tag(scenario, "fixture", TagPayload::Built(Block::zeroed(0x218))).unwrap();
        set.build().unwrap().0
    }

    fn prepare(
        handle: &MapRebind,
        slot: &str,
        map: &str,
        path: &Path,
    ) -> std::result::Result<PreparedMapRebind, String> {
        handle.prepare(slot, map, path, map::CacheType::Multiplayer, 0x0500_0000)
    }

    fn publish(staged: PreparedMapRebind) {
        staged.reserve(&crate::Vfs::new()).unwrap().commit();
    }

    #[test]
    fn rebind_closed_slot_serves_the_new_size_bytes_and_id() {
        let t = Temp::new("rebind-closed");
        let path = t.maps().join("Level.MAP");
        let bytes = play_copy("level", 7);
        std::fs::write(&path, &bytes).unwrap();
        let handle = MapRebind::default();
        let mut b = MapOverrideBackend::rebindable(Disc, None, handle.clone());
        let guest = "maps\\a.map";
        let before = b.host_file(guest).unwrap();
        b.opened(guest);
        b.closed(guest);
        let prepared = prepare(&handle, "A", "LEVEL", &path).unwrap();
        assert_eq!(prepared.checksum(), map::checksum::engine_xor(&bytes[0x800..]));
        assert_eq!(b.host_file(guest), Some(before));
        assert!(!b.map_changed(guest));
        publish(prepared);
        assert!(b.map_changed(guest));
        assert_eq!(b.stat(guest).unwrap().end_of_file, bytes.len() as u64);
        let entries = b.read_directory("maps").unwrap();
        assert_eq!(entries[0].metadata.end_of_file, bytes.len() as u64);
        let host = b.host_file(guest).unwrap();
        assert_ne!(host.path, path);
        assert_eq!(std::fs::read(&host.path).unwrap(), bytes);
        assert_eq!(host.offset, 0);
        assert_eq!(host.len, bytes.len() as u64);
        assert_eq!(b.map_id(guest), Some(override_id("a", &path).unwrap()));
        assert!(b.map_ranges(guest).is_none());
        assert!(b.map_changed(guest), "metadata queries leave the cache stale");
        b.opened(guest);
        assert_eq!(read_all(&mut b, guest), bytes);
        assert!(!b.map_changed(guest));
        b.closed(guest);
    }

    #[test]
    fn only_a_reserved_or_published_copy_blocks_external_live() {
        let t = Temp::new("rebind-live");
        let file = t.0.join("a.map");
        std::fs::write(&file, play_copy("a", 1)).unwrap();
        let handle = MapRebind::default();
        assert!(!handle.blocks_external_live());
        let staged = prepare(&handle, "a", "a", &file).unwrap();
        assert!(!handle.blocks_external_live());
        let reserved = staged.reserve(&crate::Vfs::new()).unwrap();
        assert!(handle.blocks_external_live());
        assert!(!handle.has_ever_bound());
        drop(reserved);
        assert!(!handle.blocks_external_live());
        publish(prepare(&handle, "a", "a", &file).unwrap());
        assert!(handle.has_ever_bound());
        assert!(handle.blocks_external_live());
    }

    #[test]
    fn idle_map_access_and_non_map_paths_bypass_the_binding_mutex() {
        let handle = MapRebind::default();
        let mut b = MapOverrideBackend::rebindable(Disc, None, handle.clone());
        fn while_locked(handle: &MapRebind, access: impl FnOnce() + Send) {
            std::thread::scope(|scope| {
                let guard = handle.shared.state.lock().unwrap();
                let (tx, rx) = std::sync::mpsc::channel();
                scope.spawn(move || {
                    access();
                    let _ = tx.send(());
                });
                let finished = rx.recv_timeout(std::time::Duration::from_secs(5));
                drop(guard);
                finished.expect("ordinary DVD access waited for an editor binding");
            });
        }
        while_locked(&handle, || {
            assert_eq!(b.stat(r"maps\a.map").unwrap().end_of_file, 6);
            assert_eq!(read_all(&mut b, r"maps\a.map"), b"disc-a");
            assert_eq!(b.read_directory("maps").unwrap().len(), 3);
        });
        let t = Temp::new("rebind-fastpath");
        let file = t.0.join("a.map");
        std::fs::write(&file, play_copy("a", 1)).unwrap();
        publish(prepare(&handle, "a", "a", &file).unwrap());
        while_locked(&handle, || {
            assert_eq!(b.stat("default.xbe").unwrap().end_of_file, 3);
            assert_eq!(read_all(&mut b, "default.xbe"), b"xbe");
            assert_eq!(b.read_directory("").unwrap().len(), 1);
        });
    }

    #[test]
    fn nested_overrides_report_inner_bindings_and_inner_busy_errors() {
        let t = Temp::new("rebind-nested");
        let file = t.0.join("a.map");
        std::fs::write(&file, play_copy("a", 1)).unwrap();
        let inner = MapRebind::default();
        let outer = MapRebind::default();
        let dvd = MapOverrideBackend::rebindable(Disc, None, inner.clone());
        let b = MapOverrideBackend::rebindable(dvd, None, outer.clone());
        let staged = prepare(&inner, "a", "a", &file).unwrap();
        assert!(matches!(b.snapshot_map_bindings(), Err(crate::StateError::MapReloadBusy)));
        publish(staged);
        publish(prepare(&outer, "b", "a", &file).unwrap());
        let keys: Vec<_> = b.snapshot_map_bindings().unwrap().into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, [r"maps\a.map", r"maps\b.map"]);
    }

    #[test]
    fn invalid_or_cancelled_rebind_preserves_the_served_file() {
        let t = Temp::new("rebind-invalid");
        let handle = MapRebind::default();
        let dir = t.maps();
        let path = dir.join("a.map");
        std::fs::write(&path, b"initial override").unwrap();
        let mut b = MapOverrideBackend::rebindable(Disc, Some(&dir), handle.clone());
        let guest = "maps\\a.map";
        let initial = b.map_id(guest).unwrap();
        let copy = t.0.join("a.map");
        let mut bytes = play_copy("a", 3);
        std::fs::write(&copy, &bytes[..0x800]).unwrap();
        assert!(prepare(&handle, "a", "a", &copy).is_err(), "declared length");
        bytes[0x7fc] = 0;
        std::fs::write(&copy, &bytes).unwrap();
        assert!(prepare(&handle, "a", "a", &copy).is_err(), "header footer");
        std::fs::write(&copy, play_copy("b", 4)).unwrap();
        assert!(prepare(&handle, "a", "a", &copy).is_err(), "header name");
        assert!(prepare(&handle, "a", "b", &copy).is_err(), "file name");
        assert!(prepare(&handle, "a", "a", Path::new("a.map")).is_err());
        std::fs::write(&copy, play_copy("a", 4)).unwrap();
        let cancelled = prepare(&handle, "a", "a", &copy).unwrap();
        assert!(prepare(&handle, "A", "a", &copy).is_err());
        drop(cancelled);
        assert!(prepare(&handle, "a", "a", &copy).is_ok());
        assert_eq!(b.map_id(guest), Some(initial));
        assert_eq!(read_all(&mut b, guest), b"initial override");
        assert!(!b.map_changed(guest));
    }

    #[test]
    fn a_revoked_preparation_releases_snapshots_and_cannot_cancel_its_replacement() {
        let t = Temp::new("revoke");
        let handle = MapRebind::default();
        let file = t.maps().join("a.map");
        std::fs::write(&file, play_copy("a", 1)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let first = handle.prepare_cancellable(
            "a", "a", &file, map::CacheType::Multiplayer, 0x0500_0000, cancelled.clone(),
        ).unwrap();
        assert!(handle.snapshot_bindings().is_err());
        cancelled.store(true, Ordering::Release);
        // The old worker still owns its descriptor. A timeout must have
        // released its claim before another request or a snapshot starts.
        assert!(handle.snapshot_bindings().unwrap().is_empty());
        let second = prepare(&handle, "a", "a", &file).unwrap();
        assert!(first.reserve(&crate::Vfs::new()).is_err());
        assert!(handle.snapshot_bindings().is_err());
        publish(second);
        assert_eq!(handle.snapshot_bindings().unwrap().len(), 1);
    }

    #[test]
    fn reserve_excludes_old_opens_before_queueing_and_drop_releases_the_source() {
        let t = Temp::new("rebind-retire");
        let copy = t.maps().join("a.map");
        let bytes = play_copy("a", 5);
        std::fs::write(&copy, &bytes).unwrap();
        let handle = MapRebind::default();
        let mut b = MapOverrideBackend::rebindable(Disc, None, handle.clone());
        let guest = "maps\\a.map";
        let staged = prepare(&handle, "a", "a", &copy).unwrap();
        let reserved = staged.reserve(&crate::Vfs::new()).unwrap();
        assert!(matches!(b.stat(guest), Err(Error::SharingViolation)));
        assert!(b.snapshot_map_bindings().is_err());
        drop(reserved);
        assert!(b.stat(guest).is_ok());
        assert!(b.snapshot_map_bindings().unwrap().is_empty());
        assert_eq!(read_all(&mut b, guest), b"disc-a");

        publish(prepare(&handle, "a", "a", &copy).unwrap());
        let id = override_id("a", &copy).unwrap();
        assert_eq!(b.snapshot_map_bindings().unwrap(), vec![(guest.into(), id.to_string())]);
        b.opened(guest);
        assert_eq!(read_all(&mut b, guest), bytes);
        assert_eq!(b.map_id(guest), Some(id));
        b.closed(guest);
    }

    #[test]
    fn adopting_the_mounted_dvd_keeps_existing_opens_and_excludes_conflicting_reloads() {
        let t = Temp::new("rebind-guest-open");
        let startup = t.maps().join("a.map");
        std::fs::write(&startup, b"chosen").unwrap();
        let copy = t.0.join("a.map");
        std::fs::write(&copy, play_copy("a", 5)).unwrap();
        let handle = MapRebind::default();
        let mut vfs = crate::Vfs::new();
        vfs.mount(crate::CDROM_DEVICE, MapOverrideBackend::new(Disc, t.maps())).unwrap();
        let guest = r"\Device\CdRom0\maps\a.map";
        let open = vfs.open(guest, Default::default()).unwrap().handle;
        let other_handle = vfs.duplicate(open).unwrap();
        // A second lookup of the startup path would select different bytes.
        std::fs::rename(&startup, t.maps().join("saved.map")).unwrap();
        std::fs::write(&startup, b"second").unwrap();
        assert!(handle.attach(&mut vfs).unwrap());
        assert!(!handle.attach(&mut vfs).unwrap());
        let mut bytes = [0; 6];
        assert_eq!(vfs.read(other_handle, crate::Offset::Absolute(0), &mut bytes), Ok(6));
        assert_eq!(&bytes, b"chosen");
        let prepared = prepare(&handle, "a", "a", &copy).unwrap();
        assert!(prepared.reserve(&vfs).is_err());
        vfs.close(open).unwrap();
        assert!(prepare(&handle, "a", "a", &copy).unwrap().reserve(&vfs).is_err());
        vfs.close(other_handle).unwrap();
        let reserved = prepare(&handle, "a", "a", &copy).unwrap().reserve(&vfs).unwrap();
        assert!(matches!(vfs.open(guest, Default::default()), Err(Error::SharingViolation)));
        reserved.commit();
        let reopened = vfs.open(guest, Default::default()).unwrap().handle;
        vfs.close(reopened).unwrap();
    }

    #[test]
    fn reaping_abandoned_generations_keeps_live_and_unrelated_files() {
        let t = Temp::new("reap");
        let root = t.maps();
        let dir = |name: &str| {
            let path = root.join(name);
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path).unwrap();
            std::fs::write(path.join("fixture.map"), [0]).unwrap();
            path
        };
        let abandoned = dir("h2-map-101-0-0000000000000001");
        let live = dir("h2-map-102-0-0000000000000002");
        let unrelated = dir("h2-map-101-other");
        let extra = dir("h2-map-101-1-0000000000000003");
        std::fs::write(extra.join("other"), [0]).unwrap();
        let multiple = dir("h2-map-101-2-0000000000000004");
        std::fs::write(multiple.join("second.map"), [0]).unwrap();
        let empty = dir("h2-map-101-3-0000000000000005");
        std::fs::remove_file(empty.join("fixture.map")).unwrap();
        generation::reap(&root, |pid| pid == 101);
        assert!(!abandoned.exists());
        assert!(!empty.exists());
        assert!(multiple.join("second.map").exists());
        for path in [live, unrelated, extra, multiple] {
            assert!(path.join("fixture.map").is_file());
        }
    }

    #[test]
    fn an_invalid_payload_checksum_refuses_before_reservation_and_allows_a_retry() {
        let t = Temp::new("rebind-checksum");
        let path = t.maps().join("a.map");
        let handle = MapRebind::default();
        let mut bytes = play_copy("a", 2);
        let good = bytes.clone();
        // Change one byte of a complete payload qword, outside the index tables.
        let meta = u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap())
            + u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap());
        bytes[meta as usize] ^= 1;
        let reference = |bytes: &[u8]| {
            map::Map::from_reader(std::io::Cursor::new(bytes)).unwrap().validate_checksum()
        };
        assert!(reference(&good).is_ok());
        assert!(reference(&bytes).is_err());
        std::fs::write(&path, bytes).unwrap();
        let error = prepare(&handle, "a", "a", &path).err().expect("invalid checksum");
        assert!(error.contains("checksum"), "{error}");
        std::fs::write(&path, good).unwrap();
        assert!(prepare(&handle, "a", "a", &path).is_ok());
    }

    #[test]
    fn reload_checks_slot_type_capacity_and_the_tag_index() {
        let t = Temp::new("rebind-invalid-index");
        let file = t.maps().join("a.map");
        let handle = MapRebind::default();
        let bytes = play_copy("a", 2);
        std::fs::write(&file, &bytes).unwrap();
        assert!(handle.prepare("a", "a", &file, map::CacheType::Campaign, u32::MAX).is_err());
        let too_small = bytes.len() as u32 - 1;
        assert!(handle.prepare("a", "a", &file, map::CacheType::Multiplayer, too_small).is_err());
        let mut header = bytes[..0x800].to_vec();
        header[8..12].copy_from_slice(&0x800u32.to_le_bytes());
        std::fs::write(&file, header).unwrap();
        assert!(prepare(&handle, "a", "a", &file).is_err());
        let mut bad = bytes;
        let index = u32::from_le_bytes(bad[0x10..0x14].try_into().unwrap()) as usize;
        bad[index + 28] ^= 1;
        std::fs::write(&file, bad).unwrap();
        assert!(prepare(&handle, "a", "a", &file).is_err());
        assert!(handle.snapshot_bindings().unwrap().is_empty());
    }

    #[test]
    fn issued_paths_and_cached_identity_stay_with_their_immutable_copy() {
        let t = Temp::new("rebind-generation");
        let file = t.maps().join("a.map");
        let handle = MapRebind::default();
        let mut b = MapOverrideBackend::rebindable(Disc, None, handle.clone());
        let guest = "maps\\a.map";
        let first = play_copy("a", 1);
        std::fs::write(&file, &first).unwrap();
        publish(prepare(&handle, "a", "a", &file).unwrap());
        let original = b.host_file(guest).unwrap();
        let id = b.map_id(guest).unwrap();
        let second = play_copy("a", 2);
        std::fs::write(&file, &second).unwrap(); // same inode, different contents
        assert_eq!(b.map_id(guest), Some(id));
        assert_eq!(read_all(&mut b, guest), first);
        let saved = t.0.join("save");
        std::fs::write(&saved, &second).unwrap();
        std::fs::rename(saved, &file).unwrap();
        publish(prepare(&handle, "a", "a", &file).unwrap());
        let replacement = b.host_file(guest).unwrap();
        assert_eq!(
            b.snapshot_map_bindings().unwrap(),
            vec![(guest.to_owned(), override_id("a", &file).unwrap().to_string())]
        );
        assert_ne!(replacement.path, original.path);
        assert_eq!(std::fs::read(&original.path).unwrap(), first);
        assert_eq!(std::fs::read(&replacement.path).unwrap(), second);
        assert_ne!(b.map_id(guest).unwrap(), id);
        let original_path = original.path.clone();
        let replacement_path = replacement.path.clone();
        drop(original);
        assert!(!original_path.exists());
        let late = prepare(&handle, "a", "a", &file).unwrap();
        handle.clear_on_shutdown();
        assert!(handle.has_ever_bound());
        assert!(late.reserve(&crate::Vfs::new()).is_err());
        assert!(prepare(&handle, "a", "a", &file).is_err());
        assert!(replacement_path.exists());
        drop(replacement);
        assert!(!replacement_path.exists());
    }

    #[test]
    fn replacing_an_unexported_generation_releases_its_private_file() {
        let t = Temp::new("rebind-release");
        let file = t.maps().join("a.map");
        let handle = MapRebind::default();
        let mut b = MapOverrideBackend::rebindable(Disc, None, handle.clone());
        let guest = r"maps\a.map";
        std::fs::write(&file, play_copy("a", 1)).unwrap();
        publish(prepare(&handle, "a", "a", &file).unwrap());
        let first = b.host_file(guest).unwrap().path;
        assert!(first.exists());
        std::fs::write(&file, play_copy("a", 2)).unwrap();
        publish(prepare(&handle, "a", "a", &file).unwrap());
        assert!(!first.exists());
    }

    #[test]
    fn server_maps_have_no_reload_binding() {
        let t = Temp::new("rebind-server");
        let copy = t.maps().join("a.map");
        std::fs::write(&copy, play_copy("a", 1)).unwrap();
        let mut b = MapOverrideBackend::server_maps(Disc, t.maps());
        assert!(b.rebind.is_none());
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
    }

    #[test]
    fn an_override_has_a_derived_id_and_other_paths_have_none() {
        let t = Temp::new("id");
        std::fs::write(t.maps().join("a.map"), b"edited map bytes").unwrap();
        let (mut b, _) = backend(&t.maps());
        let path = t.maps().join("a.map");
        let expected = override_id("a", &path).unwrap();
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"edited map bytes");
        // Replacement between first use and the first ID lookup.
        let next = t.maps().join("next.map");
        std::fs::write(&next, b"new contents").unwrap();
        std::fs::rename(next, path).unwrap();
        let id = b.map_id("maps\\A.MAP").unwrap();
        assert_eq!(b.map_id("maps\\a.map"), Some(id));
        assert_eq!(id, expected);
        // Not overridden, or not a map: the disc's, which says nothing.
        assert_eq!(b.map_id("maps\\b.map"), None);
        assert_eq!(b.map_id("default.xbe"), None);
        // Derived, so never a retail id for the same bytes' hash.
        let retail = content::MapId::retail("a", content::Sha256::of(b"edited map bytes"));
        assert_ne!(id, retail);
    }

    #[test]
    fn map_paths_are_exactly_one_level_under_maps() {
        assert_eq!(is_map_path("maps\\a.map"), Some("a.map"));
        assert_eq!(is_map_path("MAPS\\A.MAP"), Some("A.MAP"));
        assert_eq!(is_map_path("maps\\.map"), Some(".map"));
        assert_eq!(is_map_path("maps\\sub\\a.map"), None);
        assert_eq!(is_map_path("other\\a.map"), None);
        assert_eq!(is_map_path("maps\\a.mapx"), None);
        assert_eq!(is_map_path("maps\\map"), None);
        assert_eq!(is_map_path("a.map"), None);
        assert_eq!(is_map_path("maps"), None);
    }

    #[test]
    fn override_is_served_case_insensitively_with_its_size_and_logged_once() {
        let t = Temp::new("serve");
        std::fs::write(t.maps().join("A.Map"), b"edited map bytes").unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(b.stat("MAPS\\a.MAP").unwrap().end_of_file, 16);
        assert_eq!(b.stat("maps\\a.map").unwrap().allocation_size, 2048);
        assert!(b.stat("maps\\a.map").unwrap().basic.attributes & ATTR_READONLY != 0);
        assert_eq!(read_all(&mut b, "maps\\A.MAP"), b"edited map bytes");
        let mut buf = [0u8; 8];
        assert_eq!(b.read_at("maps\\a.map", 12, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"ytes");
        assert_eq!(b.read_at("maps\\a.map", 16, &mut buf).unwrap(), 0);
        assert_eq!(b.read_at("maps\\a.map", u64::MAX, &mut buf).unwrap(), 0);
        let host = b.host_file("maps\\a.map").unwrap();
        assert_eq!(host.offset, 0);
        assert_eq!(host.len, 16);
        assert!(host.path.ends_with("A.Map"));
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("maps\\a.MAP from ")
                && lines[0].contains("16 bytes; disc 6 bytes"),
            "{lines:?}"
        );
    }

    #[test]
    fn other_files_and_maps_without_an_override_fall_through() {
        let t = Temp::new("through");
        std::fs::write(t.maps().join("a.map"), b"edited").unwrap();
        std::fs::write(t.maps().join("default.xbe"), b"not me").unwrap();
        std::fs::write(t.maps().join("notes.map.txt"), b"not me").unwrap();
        std::fs::write(t.maps().join("new.map"), b"not on disc").unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\b.map"), b"disc-bb");
        assert_eq!(read_all(&mut b, "default.xbe"), b"xbe");
        assert_eq!(read_all(&mut b, "maps\\notes.map.txt"), b"t");
        assert_eq!(b.host_file("maps\\b.map").unwrap().offset, 7);
        // A map the disc lacks stays missing.
        assert_eq!(b.stat("maps\\new.map"), Err(Error::ObjectNameNotFound));
        assert!(b.read_at("maps\\new.map", 0, &mut [0; 4]).is_err());
        assert!(lines.lock().unwrap().is_empty());
        assert!(b.read_only());
        assert_eq!(
            b.write_at("maps\\a.map", 0, b"x"),
            Err(Error::MediaWriteProtected)
        );
        assert_eq!(
            b.create("maps\\c.map", false, 0),
            Err(Error::MediaWriteProtected)
        );
        assert_eq!(b.set_len("maps\\a.map", 0), Err(Error::MediaWriteProtected));
        assert_eq!(b.remove("maps\\a.map"), Err(Error::MediaWriteProtected));
        assert_eq!(std::fs::read(t.maps().join("a.map")).unwrap(), b"edited");
    }

    #[test]
    fn listing_selects_the_startup_override_and_caches_disc_misses() {
        let t = Temp::new("list");
        std::fs::write(t.maps().join("B.MAP"), vec![1u8; 5000]).unwrap();
        let (mut b, _) = backend(&t.maps());
        let sizes: Vec<_> = b
            .read_directory("MAPS")
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.metadata.end_of_file, e.metadata.allocation_size))
            .collect();
        // Publish a different pathname after enumeration. The next guest
        // open reads the descriptor selected by the listing, including its size.
        let replacement = t.maps().join("replacement");
        std::fs::write(&replacement, b"replacement").unwrap();
        std::fs::rename(replacement, t.maps().join("B.MAP")).unwrap();
        std::fs::write(t.maps().join("a.map"), b"late override").unwrap();
        assert_eq!(read_all(&mut b, "maps\\b.map"), vec![1u8; 5000]);
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert_eq!(
            sizes,
            [
                ("a.map".to_owned(), 6, 2048),
                ("b.map".to_owned(), 5000, 6144),
                ("notes.map.txt".to_owned(), 1, 2048)
            ]
        );
    }

    #[test]
    fn missing_directory_and_unusable_files_fall_back_to_the_disc() {
        let t = Temp::new("fallback");
        let (mut b, lines) = backend(&t.0.join("absent"));
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert_eq!(lines.lock().unwrap().len(), 1);
        assert!(lines.lock().unwrap()[0].starts_with("warning: cannot use"));

        // A directory where the map should be, and names that differ only in case.
        std::fs::create_dir(t.maps().join("a.map")).unwrap();
        std::fs::write(t.maps().join("b.map"), b"one").unwrap();
        std::fs::write(t.maps().join("B.map"), b"two").unwrap();
        let case_sensitive = std::fs::read_dir(t.maps()).unwrap().count() == 3;
        let (mut b, lines) = backend(&t.maps());
        for _ in 0..2 {
            assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
            assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 6);
        }
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "warned once: {lines:?}");
        assert!(lines[0].contains("not a regular file"), "{lines:?}");
        drop(lines);
        if case_sensitive {
            assert_eq!(read_all(&mut b, "maps\\b.map"), b"disc-bb");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_are_followed_and_escaping_ones_refused() {
        let t = Temp::new("links");
        std::fs::write(t.0.join("outside.map"), b"outside").unwrap();
        std::fs::write(t.maps().join("real.bin"), b"inside").unwrap();
        std::os::unix::fs::symlink(t.0.join("outside.map"), t.maps().join("a.map")).unwrap();
        std::os::unix::fs::symlink("real.bin", t.maps().join("b.map")).unwrap();
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 6);
        assert_eq!(read_all(&mut b, "maps\\b.map"), b"inside");
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("links outside")),
            "{lines:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_override_falls_back_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("perm");
        let p = t.maps().join("a.map");
        std::fs::write(&p, b"secret").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read it anyway; then there is nothing to test.
        if File::open(&p).is_ok() {
            return;
        }
        let (mut b, lines) = backend(&t.maps());
        assert_eq!(read_all(&mut b, "maps\\a.map"), b"disc-a");
        assert!(lines.lock().unwrap()[0].contains("cannot open"));
    }

    #[test]
    fn override_streams_past_64_mib_from_a_sparse_file() {
        let t = Temp::new("sparse");
        let len = 300 * 1024 * 1024u64;
        let f = File::create(t.maps().join("a.map")).unwrap();
        f.set_len(len).unwrap();
        let mut f = f;
        use std::io::Write;
        for (at, mark) in [
            (0u64, b"head"),
            (100 * 1024 * 1024, b"mark"),
            (len - 4, b"foot"),
        ] {
            f.seek(SeekFrom::Start(at)).unwrap();
            f.write_all(mark).unwrap();
        }
        drop(f);
        let (mut b, _) = backend(&t.maps());
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, len);
        let mut buf = [9u8; 8];
        assert_eq!(
            b.read_at("maps\\a.map", 100 * 1024 * 1024 - 2, &mut buf)
                .unwrap(),
            8
        );
        assert_eq!(&buf, b"\0\0mark\0\0");
        assert_eq!(b.read_at("maps\\a.map", len - 6, &mut buf).unwrap(), 6);
        assert_eq!(&buf[..6], b"\0\0foot");
        assert_eq!(b.read_at("maps\\a.map", 0, &mut buf).unwrap(), 8);
        assert_eq!(&buf[..4], b"head");
    }

    #[test]
    fn startup_override_streams_the_original_file_without_a_private_copy() {
        let t = Temp::new("trunc");
        let p = t.maps().join("a.map");
        std::fs::write(&p, vec![1u8; 4096]).unwrap();
        let (mut b, _) = backend(&t.maps());
        assert_eq!(b.stat("maps\\a.map").unwrap().end_of_file, 4096);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(10)
            .unwrap();
        let host = b.host_file("maps\\a.map").unwrap();
        assert!(host.generation.is_none());
        assert_eq!(host.path, p.canonicalize().unwrap());
        assert_eq!(b.read_at("maps\\a.map", 0, &mut [0; 4096]), Err(Error::FileCorrupt));
    }
}
