use crate::{
    ATTR_ARCHIVE, ATTR_DIRECTORY, ATTR_NORMAL, BasicInfo, DirectoryEntry, Error, FATX_ATTRIBUTES,
    FATX_CLUSTER_SIZE, FATX_MAX_DIRECTORY_ENTRIES, FATX_MAX_FILE_SIZE, FATX_MAX_NAME, Metadata,
    Result, Timestamp, VolumeInfo,
    backend::Backend,
    path::{components, validate_fatx_name, validate_fatx_path},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone)]
struct Child {
    name: String,
    host: fs::Metadata,
}
struct DirectoryIndex {
    eligible_count: usize,
    modified: Option<SystemTime>,
    children: Arc<BTreeMap<String, Vec<Child>>>,
}
struct Reservation {
    owner: u64,
    bytes: u64,
}
#[derive(Default)]
struct Cache {
    directories: BTreeMap<PathBuf, DirectoryIndex>,
    used_bytes: Option<u64>,
    atimes: BTreeMap<String, Timestamp>,
    dirty_data: BTreeSet<String>,
    // Logical FATX preallocation, trimmed only by its owning file object's close.
    reservations: BTreeMap<String, Reservation>,
    #[cfg(test)]
    directory_scans: usize,
    #[cfg(test)]
    usage_scans: usize,
    #[cfg(test)]
    data_syncs: usize,
    #[cfg(test)]
    metadata_stores: usize,
}

/// How long write metadata may stay unstored: a crash or an exit that
/// skips the backend's drop loses at most this much (plus one
/// [`FLUSH_TICK`]) of write/change-time and archive-bit updates, never
/// file data. docs/storage.md "Write metadata".
pub const METADATA_STALE: Duration = Duration::from_secs(1);
/// How often the flusher thread looks for stale write metadata.
const FLUSH_TICK: Duration = Duration::from_millis(250);

/// Basic info that writes and truncations changed (write and change times,
/// the archive bit), not yet stored. Storing the record durably on every
/// write cost two syncs per guest write; instead it is stored at the next
/// flush, close or drop, by the write path once the oldest entry is
/// [`METADATA_STALE`], by the backend's flusher thread when no write comes,
/// and by [`flush_pending_metadata`]. Every record store holds `entries`,
/// so a flusher's store and the backend's own are never reordered.
struct Written {
    metadata_root: PathBuf,
    /// [`METADATA_STALE`]; longer in tests that count stores.
    stale: Mutex<Duration>,
    /// Lowercase path: the info and when it first became pending.
    entries: Mutex<BTreeMap<String, (BasicInfo, Instant)>>,
}

impl Written {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, (BasicInfo, Instant)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Store every entry pending since before `older_than` (all with
    /// `None`); failed ones stay pending. Returns the first error.
    fn flush(&self, older_than: Option<Instant>) -> Result<()> {
        let mut entries = self.lock();
        let due: Vec<String> = entries
            .iter()
            .filter(|(_, (_, since))| older_than.is_none_or(|t| *since <= t))
            .map(|(k, _)| k.clone())
            .collect();
        let mut first = Ok(());
        for key in due {
            let (info, _) = entries[&key];
            match write_record(&record_path(&self.metadata_root, &key), info) {
                Ok(()) => {
                    entries.remove(&key);
                }
                Err(e) => {
                    if first.is_ok() {
                        first = Err(e);
                    }
                }
            }
        }
        first
    }
    fn flush_stale(&self) -> Result<()> {
        let stale = *self.stale.lock().unwrap_or_else(|e| e.into_inner());
        match Instant::now().checked_sub(stale) {
            Some(t) => self.flush(Some(t)),
            None => Ok(()),
        }
    }
}

/// Every live backend's pending write metadata, for [`flush_pending_metadata`].
static LIVE: Mutex<Vec<Weak<Written>>> = Mutex::new(Vec::new());

/// Store the pending write metadata of every live [`HostBackend`] now,
/// including backends that were leaked and will never be dropped. For
/// exit paths: call it before `process::exit` or from a signal-handling
/// thread. Errors are reported on stderr.
pub fn flush_pending_metadata() {
    let live: Vec<Arc<Written>> = {
        let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|w| w.strong_count() > 0);
        l.iter().filter_map(Weak::upgrade).collect()
    };
    for w in live {
        if let Err(e) = w.flush(None) {
            eprintln!("vfs: storing pending write metadata failed: {e:?}");
        }
    }
}

/// Run until `written` is dropped, storing what has been pending for
/// [`METADATA_STALE`] every [`FLUSH_TICK`].
fn spawn_flusher(written: Weak<Written>) {
    let spawned = std::thread::Builder::new()
        .name("vfs-metadata".into())
        .spawn(move || {
            let mut failed = false;
            loop {
                std::thread::sleep(FLUSH_TICK);
                let Some(w) = written.upgrade() else { return };
                match w.flush_stale() {
                    Err(e) if !failed => {
                        eprintln!("vfs: storing write metadata failed (retrying): {e:?}");
                        failed = true;
                    }
                    Err(_) => {}
                    Ok(()) => failed = false,
                }
            }
        });
    if let Err(e) = spawned {
        // The write path still stores stale entries itself.
        eprintln!("vfs: no metadata flusher thread: {e}");
    }
}

fn record_path(metadata_root: &Path, path: &str) -> PathBuf {
    let mut result = metadata_root.to_owned();
    for part in path.split('\\').filter(|p| !p.is_empty()) {
        result.push(
            part.to_ascii_lowercase()
                .bytes()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        );
    }
    result.push("record");
    result
}

/// Write the 40-byte record durably: a staged file, synced, renamed over
/// the record, then the directory synced.
fn write_record(target: &Path, info: BasicInfo) -> Result<()> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(b"HVF1");
    bytes.extend_from_slice(&info.attributes.to_le_bytes());
    for time in [
        info.creation_time,
        info.last_access_time,
        info.last_write_time,
        info.change_time,
    ] {
        bytes.extend_from_slice(&time.fatx().0.to_le_bytes());
    }
    let parent = target.parent().unwrap();
    durable_create_dirs(parent)?;
    let staged = target.with_extension("new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staged)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(staged, target)?;
    sync_directory(parent)
}

/// FATX view of a private host directory. Metadata is outside the mounted tree.
/// One owner must serialize mutations, including direct Backend calls. The Vfs checks access at open.
pub struct HostBackend {
    root: PathBuf,
    metadata_root: PathBuf,
    /// The two roots as opened at construction (no symlink followed). Format
    /// traverses from these, so renaming or replacing a root or an ancestor
    /// later cannot redirect its deletions.
    #[cfg(unix)]
    anchors: [rustix::fd::OwnedFd; 2],
    capacity: u64,
    label: String,
    serial: u32,
    cache: RefCell<Cache>,
    written: Arc<Written>,
    #[cfg(test)]
    next_create_sync_error: std::cell::Cell<Option<Error>>,
    #[cfg(test)]
    next_metadata_retire_error: std::cell::Cell<Option<Error>>,
    #[cfg(test)]
    next_unlink_error: std::cell::Cell<Option<Error>>,
    #[cfg(test)]
    next_format_error: std::cell::Cell<Option<Error>>,
    /// Fail the next format after this many removals.
    #[cfg(test)]
    format_error_after_unlinks: std::cell::Cell<Option<usize>>,
}

fn host_litter(name: &str) -> bool {
    name.eq_ignore_ascii_case(".DS_Store")
        || name.starts_with("._")
        || name.eq_ignore_ascii_case(".Spotlight-V100")
        || name.eq_ignore_ascii_case(".fseventsd")
}
fn visible_name(name: &str) -> bool {
    !host_litter(name) && validate_fatx_name(name).is_ok()
}
fn representable(host: &fs::Metadata) -> bool {
    if !host.is_dir() && !host.is_file() {
        return false;
    }
    if host.is_file() && host.len() > FATX_MAX_FILE_SIZE {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if host.is_file() && host.nlink() != 1 {
            return false;
        }
    }
    true
}
fn allocated(host: &fs::Metadata) -> u64 {
    if host.is_dir() {
        FATX_CLUSTER_SIZE
    } else {
        host.len().div_ceil(FATX_CLUSTER_SIZE) * FATX_CLUSTER_SIZE
    }
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all().map_err(Into::into)
}
fn durable_create_dirs(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut parent = path;
    while !parent.try_exists()? {
        missing.push(parent.to_owned());
        parent = parent.parent().ok_or(Error::InvalidParameter)?;
    }
    for directory in missing.into_iter().rev() {
        fs::create_dir(&directory)?;
        sync_directory(directory.parent().unwrap())?;
    }
    Ok(())
}
impl HostBackend {
    pub fn new(root: impl AsRef<Path>, metadata_root: impl AsRef<Path>) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let metadata_root = std::path::absolute(metadata_root)?;
        durable_create_dirs(&root)?;
        durable_create_dirs(&metadata_root)?;
        let root = fs::canonicalize(root)?;
        let metadata_root = fs::canonicalize(metadata_root)?;
        if metadata_root.starts_with(&root) || root.starts_with(&metadata_root) {
            return Err(Error::InvalidParameter);
        }
        #[cfg(unix)]
        let anchors = [
            anchor::open_root(&root)?,
            anchor::open_root(&metadata_root)?,
        ];
        let written = Arc::new(Written {
            metadata_root: metadata_root.clone(),
            stale: Mutex::new(METADATA_STALE),
            entries: Mutex::new(BTreeMap::new()),
        });
        LIVE.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::downgrade(&written));
        spawn_flusher(Arc::downgrade(&written));
        Ok(Self {
            written,
            root,
            metadata_root,
            #[cfg(unix)]
            anchors,
            capacity: 5 * 1024 * 1024 * 1024,
            label: "Xbox Data".into(),
            serial: 1,
            cache: RefCell::new(Cache::default()),
            #[cfg(test)]
            next_create_sync_error: std::cell::Cell::new(None),
            #[cfg(test)]
            next_metadata_retire_error: std::cell::Cell::new(None),
            #[cfg(test)]
            next_unlink_error: std::cell::Cell::new(None),
            #[cfg(test)]
            next_format_error: std::cell::Cell::new(None),
            #[cfg(test)]
            format_error_after_unlinks: std::cell::Cell::new(None),
        })
    }
    pub fn with_volume(mut self, label: impl Into<String>, serial: u32, capacity: u64) -> Self {
        self.label = label.into();
        self.serial = serial;
        self.capacity = capacity / FATX_CLUSTER_SIZE * FATX_CLUSTER_SIZE;
        self
    }
    /// Cache names, not guest metadata. A directory timestamp change refreshes its index.
    fn children(&self, directory: &Path) -> Result<Arc<BTreeMap<String, Vec<Child>>>> {
        let host = fs::symlink_metadata(directory)?;
        if !host.is_dir() {
            return Err(Error::ObjectPathNotFound);
        }
        let modified = host.modified().ok();
        if let Some(index) = self.cache.borrow().directories.get(directory)
            && modified.is_some()
            && index.modified == modified
        {
            return Ok(index.children.clone());
        }
        let mut children: BTreeMap<String, Vec<Child>> = BTreeMap::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !visible_name(&name) {
                continue;
            }
            let host = fs::symlink_metadata(entry.path())?;
            children
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(Child { name, host });
        }
        let children = Arc::new(children);
        let mut cache = self.cache.borrow_mut();
        #[cfg(test)]
        {
            cache.directory_scans += 1;
        }
        cache.directories.insert(
            directory.to_owned(),
            DirectoryIndex {
                eligible_count: children
                    .values()
                    .filter(|c| c.len() == 1 && representable(&c[0].host))
                    .count(),
                modified,
                children: children.clone(),
            },
        );
        Ok(children)
    }
    fn update_parent(&self, host: &Path, exists: bool) -> Result<()> {
        let parent = host.parent().ok_or(Error::InvalidParameter)?;
        let name = host
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::ObjectNameInvalid)?;
        let metadata = if exists {
            Some(fs::symlink_metadata(host)?)
        } else {
            None
        };
        let modified = fs::symlink_metadata(parent)?.modified().ok();
        let mut cache = self.cache.borrow_mut();
        if let Some(index) = cache.directories.get_mut(parent) {
            let key = name.to_ascii_lowercase();
            let entries = Arc::make_mut(&mut index.children);
            let old = entries
                .get(&key)
                .is_some_and(|c| c.len() == 1 && representable(&c[0].host));
            if let Some(host) = metadata {
                entries.insert(
                    key.clone(),
                    vec![Child {
                        name: name.to_owned(),
                        host,
                    }],
                );
            } else {
                entries.remove(&key);
            }
            let new = entries
                .get(&key)
                .is_some_and(|c| c.len() == 1 && representable(&c[0].host));
            index.eligible_count = index.eligible_count - usize::from(old) + usize::from(new);
            index.modified = modified;
        }
        Ok(())
    }
    fn resolve(&self, path: &str, allow_missing_leaf: bool) -> Result<PathBuf> {
        validate_fatx_path(path)?;
        let parts = components(path)?;
        let mut host = self.root.clone();
        for (i, part) in parts.iter().enumerate() {
            let children = self.children(&host)?;
            let found = children.get(&part.to_ascii_lowercase());
            if let Some(found) = found {
                if found.len() != 1 {
                    return Err(Error::FileCorrupt);
                }
                let child = &found[0];
                if child.host.file_type().is_symlink() {
                    return Err(Error::AccessDenied);
                }
                host.push(&child.name);
            } else if allow_missing_leaf && i + 1 == parts.len() && !host_litter(part) {
                host.push(part);
            } else {
                return Err(if i + 1 == parts.len() {
                    Error::ObjectNameNotFound
                } else {
                    Error::ObjectPathNotFound
                });
            }
        }
        Ok(host)
    }
    fn metadata_path(&self, path: &str) -> PathBuf {
        record_path(&self.metadata_root, path)
    }
    fn retire_metadata(&self, path: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(error) = self.next_metadata_retire_error.take() {
            return Err(error);
        }
        let state = self.metadata_path(path).parent().unwrap().to_owned();
        match fs::remove_dir_all(&state) {
            Ok(()) => sync_directory(state.parent().unwrap()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn remove_litter(&self, directory: &Path) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_name().to_str().is_some_and(host_litter) {
                continue;
            }
            // Inspect the link itself: Finder litter may be a link to unrelated data.
            if fs::symlink_metadata(entry.path())?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    fn sync_created_parent(&self, parent: &Path) {
        #[cfg(test)]
        let injected = self.next_create_sync_error.take();
        #[cfg(not(test))]
        let injected: Option<Error> = None;
        let synced = injected.map_or_else(|| sync_directory(parent), Err);
        if let Err(error) = synced {
            eprintln!("vfs: created entry, but parent directory sync failed: {error}");
        }
    }
    fn host_basic(host: &fs::Metadata) -> Result<BasicInfo> {
        let modified = Timestamp::from_system(host.modified()?).fatx();
        Ok(BasicInfo {
            attributes: if host.is_dir() {
                ATTR_DIRECTORY
            } else {
                ATTR_ARCHIVE
            },
            creation_time: host
                .created()
                .map(Timestamp::from_system)
                .unwrap_or(modified)
                .fatx(),
            last_access_time: host
                .accessed()
                .map(Timestamp::from_system)
                .unwrap_or(modified)
                .fatx(),
            last_write_time: modified,
            change_time: modified,
        })
    }
    fn load_basic(&self, path: &str, host: &fs::Metadata) -> Result<BasicInfo> {
        let key = path.to_ascii_lowercase();
        let written = self.written.lock().get(&key).map(|(b, _)| *b);
        let read = match written {
            Some(basic) => Ok(Ok(basic)),
            None => fs::read(self.metadata_path(path)).map(Err),
        };
        let mut info = match read {
            Ok(Ok(basic)) => basic,
            Ok(Err(bytes))
                if bytes.len() == 40
                    && &bytes[..4] == b"HVF1"
                    && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) & !FATX_ATTRIBUTES
                        == 0 =>
            {
                let time = |at| {
                    Timestamp(u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())).fatx()
                };
                BasicInfo {
                    attributes: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                    creation_time: time(8),
                    last_access_time: time(16),
                    last_write_time: time(24),
                    change_time: time(32),
                }
            }
            Ok(Err(_)) => Self::host_basic(host)?, // Torn/old metadata must never hide intact game data.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::host_basic(host)?,
            Err(e) => return Err(e.into()),
        };
        info.attributes =
            (info.attributes & !ATTR_DIRECTORY) | if host.is_dir() { ATTR_DIRECTORY } else { 0 };
        if let Some(atime) = self.cache.borrow().atimes.get(&key) {
            info.last_access_time = *atime;
        }
        Ok(info)
    }
    fn store_basic(&self, path: &str, info: BasicInfo) -> Result<()> {
        let key = path.to_ascii_lowercase();
        // Held across the store: a flusher's store of older info for the
        // same path cannot land after this one.
        let mut written = self.written.lock();
        write_record(&self.metadata_path(path), info)?;
        written.remove(&key);
        drop(written);
        let mut cache = self.cache.borrow_mut();
        cache.atimes.remove(&key);
        #[cfg(test)]
        {
            cache.metadata_stores += 1;
        }
        Ok(())
    }
    /// Store the access time and written basic info kept in the cache.
    fn flush_metadata(&self, path: &str) -> Result<()> {
        let key = path.to_ascii_lowercase();
        if !self.cache.borrow().atimes.contains_key(&key) && !self.written.lock().contains_key(&key)
        {
            return Ok(());
        }
        let host = self.resolve(path, false)?;
        let metadata = fs::symlink_metadata(host)?;
        self.store_basic(path, self.load_basic(path, &metadata)?)
    }
    fn metadata(&self, path: &str, host_path: &Path) -> Result<Metadata> {
        let host = fs::symlink_metadata(host_path)?;
        if !representable(&host) {
            return Err(if host.is_file() && host.len() > FATX_MAX_FILE_SIZE {
                Error::FileTooLarge
            } else {
                Error::AccessDenied
            });
        }
        Ok(Metadata {
            basic: self.load_basic(path, &host)?,
            end_of_file: if host.is_dir() { 0 } else { host.len() },
            allocation_size: allocated(&host).max(
                self.cache
                    .borrow()
                    .reservations
                    .get(&path.to_ascii_lowercase())
                    .map(|reservation| reservation.bytes)
                    .unwrap_or(0),
            ),
        })
    }
    fn mutable_file(&self, path: &str) -> Result<(PathBuf, Metadata)> {
        let host = self.resolve(path, false)?;
        let info = self.metadata(path, &host)?;
        if info.is_directory() {
            return Err(Error::FileIsADirectory);
        }
        // Desired access was granted at open; later READONLY changes cannot revoke a handle's rights.
        Ok((host, info))
    }
    fn used_bytes(&self) -> Result<u64> {
        if let Some(used) = self.cache.borrow().used_bytes {
            return Ok(used);
        }
        let mut bytes = 0u64;
        let mut dirs = vec![self.root.clone()];
        while let Some(dir) = dirs.pop() {
            for entries in self.children(&dir)?.values() {
                // Ambiguous or unrepresentable host objects are outside the guest allocation model.
                if entries.len() != 1 {
                    continue;
                }
                let entry = &entries[0];
                if !representable(&entry.host) {
                    continue;
                }
                bytes = bytes.saturating_add(allocated(&entry.host));
                if entry.host.is_dir() {
                    dirs.push(dir.join(&entry.name));
                }
            }
        }
        let mut cache = self.cache.borrow_mut();
        #[cfg(test)]
        {
            cache.usage_scans += 1;
        }
        cache.used_bytes = Some(bytes);
        Ok(bytes)
    }
    fn update_usage(&self, before: u64, after: u64) {
        let mut cache = self.cache.borrow_mut();
        if let Some(used) = &mut cache.used_bytes {
            *used = used.saturating_sub(before).saturating_add(after);
        }
    }
    fn check_growth(&self, old: u64, new: u64) -> Result<()> {
        if new > FATX_MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        let growth = new
            .div_ceil(FATX_CLUSTER_SIZE)
            .saturating_sub(old.div_ceil(FATX_CLUSTER_SIZE))
            * FATX_CLUSTER_SIZE;
        if growth > 0 && self.used_bytes()?.saturating_add(growth) > self.capacity {
            return Err(Error::DiskFull);
        }
        Ok(())
    }
    /// A write or truncation: new write and change times and the archive
    /// bit, pending in [`Written`] rather than stored now. Stores whatever
    /// has been pending for [`METADATA_STALE`].
    fn touch(&self, path: &str, mut basic: BasicInfo) -> Result<()> {
        // As a stored and reloaded record would read.
        let now = Timestamp::now().fatx();
        basic.last_write_time = now;
        basic.change_time = now;
        basic.attributes |= ATTR_ARCHIVE;
        let at = Instant::now();
        self.written
            .lock()
            .entry(path.to_ascii_lowercase())
            .and_modify(|e| e.0 = basic)
            .or_insert((basic, at));
        self.written.flush_stale()
    }
    /// Pending write metadata, for tests.
    #[cfg(test)]
    fn pending_written(&self) -> usize {
        self.written.lock().len()
    }
    /// Keep write metadata pending for `stale` instead of [`METADATA_STALE`].
    #[cfg(test)]
    fn set_stale(&self, stale: Duration) {
        *self.written.stale.lock().unwrap() = stale;
    }
}
impl Backend for HostBackend {
    fn read_only(&self) -> bool {
        false
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        self.metadata(path, &self.resolve(path, false)?)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let dir = self.resolve(path, false)?;
        if !self.metadata(path, &dir)?.is_directory() {
            return Err(Error::InvalidParameter);
        }
        let mut out = Vec::new();
        for children in self.children(&dir)?.values() {
            if children.len() != 1 {
                return Err(Error::FileCorrupt);
            }
            let child = &children[0];
            if !representable(&child.host) {
                continue;
            }
            let relative = if path.is_empty() {
                child.name.clone()
            } else {
                format!("{path}\\{}", child.name)
            };
            let metadata = match self.metadata(&relative, &dir.join(&child.name)) {
                Ok(info) => info,
                Err(
                    Error::AccessDenied
                    | Error::FileTooLarge
                    | Error::FileCorrupt
                    | Error::ObjectNameNotFound,
                ) => continue,
                Err(e) => return Err(e),
            };
            out.push(DirectoryEntry {
                name: child.name.clone(),
                metadata,
            });
        }
        Ok(out)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let host = self.resolve(path, false)?;
        let info = self.metadata(path, &host)?;
        if info.is_directory() {
            return Err(Error::FileIsADirectory);
        }
        if buffer.is_empty() || offset >= info.end_of_file {
            return Ok(0);
        }
        let mut file = File::open(host)?;
        file.seek(SeekFrom::Start(offset))?;
        let read = file.read(buffer)?;
        if read != 0 {
            self.cache
                .borrow_mut()
                .atimes
                .insert(path.to_ascii_lowercase(), Timestamp::now());
        }
        Ok(read)
    }
    fn create(&mut self, path: &str, directory: bool, attributes: u32) -> Result<()> {
        if attributes & !(FATX_ATTRIBUTES | ATTR_NORMAL) != 0 {
            return Err(Error::InvalidParameter);
        }
        if path.is_empty() {
            return Err(Error::ObjectNameCollision);
        }
        let host = self.resolve(path, true)?;
        if host.try_exists()? {
            return Err(Error::ObjectNameCollision);
        }
        // Count names/kinds, not sidecar metadata, so damaged/unrepresentable neighbors cannot block create.
        let parent = host.parent().ok_or(Error::InvalidParameter)?;
        self.children(parent)?;
        let count = self.cache.borrow().directories[parent].eligible_count;
        if count >= FATX_MAX_DIRECTORY_ENTRIES {
            return Err(Error::DiskFull);
        }
        let growth = if directory { FATX_CLUSTER_SIZE } else { 0 };
        if self.used_bytes()?.saturating_add(growth) > self.capacity {
            return Err(Error::DiskFull);
        }
        // A successful directory unlink may leave metadata if its cleanup failed.
        // Never attach that orphaned subtree to a newly created object. The
        // pending lock is held from the retirement until the entry exists, so
        // no flusher stores a record for the name in between.
        {
            let mut pending = self.written.lock();
            pending.remove(&path.to_ascii_lowercase());
            self.retire_metadata(path)?;
            if directory {
                fs::create_dir(&host)?;
            } else {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&host)?;
            }
        }
        self.update_parent(&host, true)?;
        let attributes = (attributes & FATX_ATTRIBUTES & !ATTR_DIRECTORY)
            | if directory {
                ATTR_DIRECTORY
            } else {
                ATTR_ARCHIVE
            };
        if let Err(e) = self.store_basic(path, BasicInfo::new(attributes)) {
            if directory {
                let _ = fs::remove_dir(&host);
            } else {
                let _ = fs::remove_file(&host);
            }
            // A rollback failure leaves an entry: refresh its cached kind/count without hiding the original error.
            let _ = self.update_parent(&host, host.try_exists().unwrap_or(true));
            return Err(e);
        }
        self.update_usage(0, growth);
        if !directory {
            self.cache
                .borrow_mut()
                .dirty_data
                .insert(path.to_ascii_lowercase());
        }
        self.sync_created_parent(parent);
        Ok(())
    }
    fn create_allocated(
        &mut self,
        path: &str,
        directory: bool,
        attributes: u32,
        allocation: u64,
        owner: u64,
    ) -> Result<()> {
        if directory {
            return self.create(path, true, attributes);
        }
        // Capacity failures happen before publishing a new directory entry.
        self.check_growth(0, allocation)?;
        self.create(path, false, attributes)?;
        let allocation = allocation.div_ceil(FATX_CLUSTER_SIZE) * FATX_CLUSTER_SIZE;
        if allocation != 0 {
            self.cache.borrow_mut().reservations.insert(
                path.to_ascii_lowercase(),
                Reservation {
                    owner,
                    bytes: allocation,
                },
            );
            self.update_usage(0, allocation);
        }
        Ok(())
    }
    fn overwrite_allocated(&mut self, path: &str, allocation: u64, owner: u64) -> Result<()> {
        let info = self.stat(path)?;
        self.check_growth(info.allocation_size, allocation)?;
        self.set_len(path, 0)?;
        let allocation = allocation.div_ceil(FATX_CLUSTER_SIZE) * FATX_CLUSTER_SIZE;
        if allocation != 0 {
            self.cache.borrow_mut().reservations.insert(
                path.to_ascii_lowercase(),
                Reservation {
                    owner,
                    bytes: allocation,
                },
            );
            self.update_usage(0, allocation);
        }
        Ok(())
    }
    fn write_at(&mut self, path: &str, offset: u64, buffer: &[u8]) -> Result<usize> {
        let (host, info) = self.mutable_file(path)?;
        if buffer.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or(Error::FileTooLarge)?;
        self.used_bytes()?;
        self.check_growth(info.allocation_size, end.max(info.end_of_file))?;
        let mut file = OpenOptions::new().write(true).open(&host)?;
        file.seek(SeekFrom::Start(offset))?;
        let written = file.write(buffer)?;
        if written != 0 {
            self.cache
                .borrow_mut()
                .dirty_data
                .insert(path.to_ascii_lowercase());
        }
        let after = info
            .end_of_file
            .max(offset + written as u64)
            .div_ceil(FATX_CLUSTER_SIZE)
            * FATX_CLUSTER_SIZE;
        self.update_usage(info.allocation_size, after.max(info.allocation_size));
        self.update_parent(&host, true)?;
        self.touch(path, info.basic)?;
        Ok(written)
    }
    fn set_len(&mut self, path: &str, size: u64) -> Result<()> {
        let (host, info) = self.mutable_file(path)?;
        self.used_bytes()?;
        self.check_growth(info.allocation_size, size)?;
        OpenOptions::new().write(true).open(&host)?.set_len(size)?;
        self.cache
            .borrow_mut()
            .reservations
            .remove(&path.to_ascii_lowercase());
        self.cache
            .borrow_mut()
            .dirty_data
            .insert(path.to_ascii_lowercase());
        self.update_usage(
            info.allocation_size,
            size.div_ceil(FATX_CLUSTER_SIZE) * FATX_CLUSTER_SIZE,
        );
        self.update_parent(&host, true)?;
        self.touch(path, info.basic)
    }
    fn set_basic_info(&mut self, path: &str, mut basic: BasicInfo) -> Result<()> {
        let metadata = self.stat(path)?;
        if basic.attributes & !(FATX_ATTRIBUTES | ATTR_NORMAL) != 0
            || basic.is_directory() != metadata.is_directory()
        {
            return Err(Error::InvalidParameter);
        }
        basic.attributes &= FATX_ATTRIBUTES;
        self.store_basic(path, basic)
    }
    /// Empty the volume directory and its metadata directory, keeping both
    /// roots. Traversal starts from the directory handles opened at
    /// construction and never follows a symlink: entries are opened with
    /// O_NOFOLLOW|O_DIRECTORY relative to their parent's handle and removed
    /// with unlinkat, so a symlink is removed itself and a replaced root or
    /// ancestor path is never consulted.
    #[cfg(unix)]
    fn format(&mut self) -> Result<()> {
        // Held throughout so no flusher stores a record mid-format. A
        // format that fails partway keeps the pending metadata of the files
        // that survive it and drops that of the files it removed, so no
        // flusher recreates their records.
        let mut written = self.written.lock();
        #[cfg(test)]
        let mut unlinks = self.format_error_after_unlinks.take();
        let mut before_unlink = || -> Result<()> {
            #[cfg(test)]
            if let Some(n) = unlinks.as_mut() {
                if *n == 0 {
                    return Err(Error::IoDeviceError);
                }
                *n -= 1;
            }
            Ok(())
        };
        let emptied = (|| -> Result<()> {
            for fd in &self.anchors {
                #[cfg(test)]
                if let Some(error) = self.next_format_error.take() {
                    return Err(error);
                }
                anchor::empty(fd, &mut before_unlink)?;
                rustix::fs::fsync(fd).map_err(std::io::Error::from)?;
            }
            Ok(())
        })();
        if let Err(error) = emptied {
            // Listings, usage and per-path state may describe removed
            // entries; keep only what still names a surviving file.
            let (atimes, dirty_data, reservations) = {
                let mut cache = self.cache.borrow_mut();
                cache.directories.clear();
                cache.used_bytes = None;
                (
                    std::mem::take(&mut cache.atimes),
                    std::mem::take(&mut cache.dirty_data),
                    std::mem::take(&mut cache.reservations),
                )
            };
            let exists = |path: &String| self.resolve(path, false).is_ok();
            written.retain(|path, _| exists(path));
            let atimes = atimes.into_iter().filter(|(p, _)| exists(p)).collect();
            let dirty_data = dirty_data.into_iter().filter(exists).collect();
            let reservations = reservations
                .into_iter()
                .filter(|(p, _)| exists(p))
                .collect();
            let mut cache = self.cache.borrow_mut();
            cache.atimes = atimes;
            cache.dirty_data = dirty_data;
            cache.reservations = reservations;
            return Err(error);
        }
        written.clear();
        *self.cache.borrow_mut() = Cache::default();
        Ok(())
    }
    #[cfg(not(unix))]
    fn format(&mut self) -> Result<()> {
        Err(Error::NotSupported)
    }
    fn remove(&mut self, path: &str) -> Result<()> {
        if path.is_empty() {
            return Err(Error::AccessDenied);
        }
        let host = self.resolve(path, false)?;
        let info = self.metadata(path, &host)?;
        if info.is_directory() && !self.read_directory(path)?.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }
        // The host entry goes first and the record after, for files as for
        // directories: a failed unlink or rmdir leaves both the durable
        // record and any pending write metadata in place. The pending entry
        // is dropped, under the lock a flusher stores with, only once the
        // host entry is gone, so no flusher stores the record again.
        let mut pending = self.written.lock();
        if info.is_directory() {
            self.remove_litter(&host)?;
            // ENOTEMPTY or another host error must preserve the directory's metadata.
            fs::remove_dir(&host)?;
            self.cache.borrow_mut().directories.remove(&host);
        } else {
            #[cfg(test)]
            if let Some(error) = self.next_unlink_error.take() {
                return Err(error);
            }
            fs::remove_file(&host)?;
        }
        pending.remove(&path.to_ascii_lowercase());
        drop(pending);
        self.cache
            .borrow_mut()
            .atimes
            .remove(&path.to_ascii_lowercase());
        self.cache
            .borrow_mut()
            .dirty_data
            .remove(&path.to_ascii_lowercase());
        self.update_usage(info.allocation_size, 0);
        self.cache
            .borrow_mut()
            .reservations
            .remove(&path.to_ascii_lowercase());
        self.update_parent(&host, false)?;
        // A record left by a failed retirement is never read: nothing is
        // there to describe, and creation clears it before a new entry.
        if let Err(error) = self.retire_metadata(path) {
            eprintln!("vfs: removed entry, but metadata cleanup failed: {error}");
        }
        if let Err(error) = sync_directory(host.parent().unwrap()) {
            eprintln!("vfs: removed entry, but parent directory sync failed: {error}");
        }
        Ok(())
    }
    fn flush(&mut self, path: &str) -> Result<()> {
        self.flush_metadata(path)?;
        let host = self.resolve(path, false)?;
        File::open(host)?.sync_all()?;
        #[cfg(test)]
        {
            self.cache.borrow_mut().data_syncs += 1;
        }
        self.cache
            .borrow_mut()
            .dirty_data
            .remove(&path.to_ascii_lowercase());
        Ok(())
    }
    fn cleanup(&mut self, path: &str, write_access: bool) -> Result<()> {
        if write_access
            || self
                .cache
                .borrow()
                .dirty_data
                .contains(&path.to_ascii_lowercase())
        {
            self.flush(path)?;
        } else {
            self.flush_metadata(path)?;
        }
        Ok(())
    }
    fn release_allocation(&mut self, path: &str, owner: u64) -> Result<()> {
        if !self
            .cache
            .borrow()
            .reservations
            .get(&path.to_ascii_lowercase())
            .is_some_and(|reservation| reservation.owner == owner)
        {
            return Ok(());
        }
        let info = self.stat(path)?;
        self.cache
            .borrow_mut()
            .reservations
            .remove(&path.to_ascii_lowercase());
        let after = if info.is_directory() {
            FATX_CLUSTER_SIZE
        } else {
            info.end_of_file.div_ceil(FATX_CLUSTER_SIZE) * FATX_CLUSTER_SIZE
        };
        self.update_usage(info.allocation_size, after);
        Ok(())
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        let total = self.capacity / FATX_CLUSTER_SIZE;
        Ok(VolumeInfo {
            label: self.label.clone(),
            filesystem: "FATX",
            serial: self.serial,
            read_only: false,
            bytes_per_sector: 512,
            sectors_per_allocation_unit: (FATX_CLUSTER_SIZE / 512) as u32,
            total_allocation_units: total,
            available_allocation_units: total
                .saturating_sub(self.used_bytes()? / FATX_CLUSTER_SIZE),
            maximum_component_name_length: FATX_MAX_NAME as u32,
        })
    }
}
#[cfg(unix)]
mod anchor {
    use crate::{Error, Result};
    use rustix::fd::{AsFd, OwnedFd};
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat, unlinkat};
    use std::path::Path;

    const DIRECTORY: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    fn io(e: rustix::io::Errno) -> Error {
        std::io::Error::from(e).into()
    }

    /// Open a root directory without following a final symlink.
    pub(super) fn open_root(path: &Path) -> Result<OwnedFd> {
        rustix::fs::open(path, DIRECTORY, Mode::empty()).map_err(io)
    }

    /// Remove every entry below `dir`, keeping `dir` itself.
    /// `before_unlink` runs before each removal; its error stops the walk.
    pub(super) fn empty(
        dir: &impl AsFd,
        before_unlink: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut names = Vec::new();
        for entry in Dir::read_from(dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name();
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                names.push(name.to_owned());
            }
        }
        for name in names {
            let stat = statat(dir, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
            if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                // Fails (ELOOP/ENOTDIR) if the entry was swapped for a
                // symlink after the stat, instead of following it.
                let child = openat(dir, &name, DIRECTORY, Mode::empty()).map_err(io)?;
                empty(&child, before_unlink)?;
                before_unlink()?;
                unlinkat(dir, &name, AtFlags::REMOVEDIR).map_err(io)?;
            } else {
                before_unlink()?;
                unlinkat(dir, &name, AtFlags::empty()).map_err(io)?;
            }
        }
        Ok(())
    }
}

impl Drop for HostBackend {
    fn drop(&mut self) {
        let paths: BTreeSet<_> = self
            .cache
            .borrow()
            .atimes
            .keys()
            .cloned()
            .chain(self.written.lock().keys().cloned())
            .chain(self.cache.borrow().dirty_data.iter().cloned())
            .collect();
        for path in paths {
            let _ = self.cleanup(&path, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ATTR_HIDDEN;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "vfs-host-regression-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn backend(&self) -> HostBackend {
            HostBackend::new(self.0.join("root"), self.0.join("metadata")).unwrap()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn initial_allocation_reserves_capacity_without_exposing_bytes() {
        let temp = Temp::new();
        let mut backend = temp.backend().with_volume("test", 1, 3 * FATX_CLUSTER_SIZE);
        backend
            .create_allocated("meta", false, 0, FATX_CLUSTER_SIZE + 1, 1)
            .unwrap();
        let info = backend.stat("meta").unwrap();
        assert_eq!(info.end_of_file, 0);
        assert_eq!(info.allocation_size, 2 * FATX_CLUSTER_SIZE);
        assert_eq!(backend.read_at("meta", 0, &mut [0; 1]), Ok(0));
        backend.create_allocated("other", false, 0, 1, 2).unwrap();
        assert_eq!(backend.volume_info().unwrap().available_allocation_units, 0);
        // Writes and EOF growth inside the reservation consume no more space.
        assert_eq!(backend.write_at("meta", 0, b"Name=synthetic"), Ok(14));
        assert_eq!(
            backend.stat("meta").unwrap().allocation_size,
            2 * FATX_CLUSTER_SIZE
        );
        assert_eq!(
            backend.create_allocated("failed", false, 0, 1, 3),
            Err(Error::DiskFull)
        );
        assert_eq!(backend.stat("failed"), Err(Error::ObjectNameNotFound));
        assert_eq!(
            backend.overwrite_allocated("meta", 3 * FATX_CLUSTER_SIZE, 4),
            Err(Error::DiskFull)
        );
        let mut data = [0; 14];
        assert_eq!(backend.read_at("meta", 0, &mut data), Ok(14));
        assert_eq!(&data, b"Name=synthetic");
        backend.set_len("meta", FATX_CLUSTER_SIZE + 1).unwrap();
        backend.cleanup("other", true).unwrap();
        backend.release_allocation("other", 2).unwrap();
        assert_eq!(backend.stat("other").unwrap().allocation_size, 0);
        assert_eq!(backend.volume_info().unwrap().available_allocation_units, 1);
        backend.cleanup("meta", true).unwrap();
        drop(backend);
        let mut backend = temp.backend().with_volume("test", 1, 3 * FATX_CLUSTER_SIZE);
        assert_eq!(
            backend.stat("meta").unwrap().end_of_file,
            FATX_CLUSTER_SIZE + 1
        );
        assert_eq!(backend.volume_info().unwrap().available_allocation_units, 1);
    }

    #[test]
    fn writes_keep_their_metadata_until_flush_close_or_drop() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        // Nothing goes stale while this test counts stores.
        backend.set_stale(Duration::from_secs(3600));
        backend.create("data", false, 0).unwrap();
        backend.flush("data").unwrap();
        let mut basic = backend.stat("data").unwrap().basic;
        basic.attributes = 0;
        basic.last_write_time = Timestamp(125_911_584_000_000_000);
        backend.set_basic_info("data", basic).unwrap();
        let record = backend.metadata_path("data");
        let before = fs::read(&record).unwrap();
        let stores = backend.cache.borrow().metadata_stores;
        for i in 0..50u64 {
            assert_eq!(backend.write_at("data", i, b"x"), Ok(1));
        }
        backend.set_len("data", 64).unwrap();
        // No record was stored for any of them, but stat sees the change.
        assert_eq!(backend.cache.borrow().metadata_stores, stores);
        assert_eq!(fs::read(&record).unwrap(), before);
        let seen = backend.stat("data").unwrap().basic;
        assert!(seen.last_write_time > basic.last_write_time);
        assert_ne!(seen.attributes & ATTR_ARCHIVE, 0);
        // Flush stores it once.
        backend.flush("data").unwrap();
        assert_eq!(backend.cache.borrow().metadata_stores, stores + 1);
        assert_ne!(fs::read(&record).unwrap(), before);
        assert!(backend.pending_written() == 0);
        assert_eq!(backend.stat("data").unwrap().basic, seen);
        // Close (cleanup) stores what a later write left; so does drop.
        backend.write_at("data", 0, b"y").unwrap();
        backend.cleanup("data", false).unwrap();
        assert!(backend.pending_written() == 0);
        backend.write_at("data", 0, b"z").unwrap();
        let pending = backend.stat("data").unwrap().basic;
        drop(backend);
        let mut reopened = temp.backend();
        assert_eq!(
            reopened.stat("data").unwrap().basic.last_write_time,
            pending.last_write_time
        );
        // A removed file's pending metadata goes with it.
        reopened.write_at("data", 0, b"w").unwrap();
        reopened.remove("data").unwrap();
        assert!(reopened.pending_written() == 0);
    }

    #[test]
    fn write_metadata_pending_past_the_stale_window_is_stored_without_a_flush() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.set_stale(Duration::from_secs(3600));
        backend.create("data", false, 0).unwrap();
        backend.flush("data").unwrap();
        // An old record, so storing a write's times changes its bytes.
        let mut basic = backend.stat("data").unwrap().basic;
        basic.attributes = 0;
        basic.last_write_time = Timestamp(125_911_584_000_000_000);
        basic.change_time = basic.last_write_time;
        backend.set_basic_info("data", basic).unwrap();
        let record = backend.metadata_path("data");
        let old = fs::read(&record).unwrap();
        // The write path: with no window, the write that makes the entry
        // finds it stale and stores it.
        backend.set_stale(Duration::ZERO);
        backend.write_at("data", 0, b"x").unwrap();
        assert_eq!(backend.pending_written(), 0);
        assert_ne!(fs::read(&record).unwrap(), old, "stored by the write");
        // The flusher thread: an entry made under a long window, with no
        // write after the window shrinks, is stored by the thread alone.
        backend.set_basic_info("data", basic).unwrap();
        backend.set_stale(Duration::from_secs(3600));
        backend.write_at("data", 1, b"y").unwrap();
        assert_eq!(backend.pending_written(), 1);
        assert_eq!(fs::read(&record).unwrap(), old);
        backend.set_stale(Duration::from_millis(50));
        let deadline = Instant::now() + Duration::from_secs(10);
        while backend.pending_written() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(backend.pending_written(), 0, "the flusher stored it");
        assert_ne!(fs::read(&record).unwrap(), old);
    }

    /// `cargo test -p vfs --release bench_write_metadata -- --ignored --nocapture`:
    /// a write with its metadata pending against a write followed by the
    /// durable record store every write used to make.
    #[test]
    #[ignore]
    fn bench_write_metadata() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("data", false, 0).unwrap();
        let n = 200u64;
        let t = Instant::now();
        for i in 0..n {
            backend.write_at("data", i * 512, &[1; 512]).unwrap();
        }
        let deferred = t.elapsed() / n as u32;
        let t = Instant::now();
        for i in 0..n {
            backend.write_at("data", i * 512, &[2; 512]).unwrap();
            let basic = backend.stat("data").unwrap().basic;
            backend.store_basic("data", basic).unwrap();
        }
        let stored = t.elapsed() / n as u32;
        eprintln!("write_at, metadata pending: {deferred:?}; stored every write: {stored:?}");
        assert!(deferred < stored);
    }

    #[test]
    fn repeated_reads_defer_atime_record_writes_until_flush() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("data", false, 0).unwrap();
        backend.write_at("data", 0, b"contents").unwrap();
        let mut basic = backend.stat("data").unwrap().basic;
        basic.last_access_time = Timestamp(125_911_584_000_000_000);
        backend.set_basic_info("data", basic).unwrap();
        let record = backend.metadata_path("data");
        let before = fs::read(&record).unwrap();
        for _ in 0..20 {
            assert_eq!(backend.read_at("DATA", 0, &mut [0]), Ok(1));
        }
        assert_eq!(fs::read(&record).unwrap(), before);
        assert!(backend.stat("data").unwrap().basic.last_access_time > basic.last_access_time);
        assert_eq!(backend.cache.borrow().atimes.len(), 1);
        backend.flush("data").unwrap();
        assert_ne!(fs::read(record).unwrap(), before);
        assert!(backend.cache.borrow().atimes.is_empty());
    }

    #[test]
    fn growth_and_volume_queries_reuse_usage_and_directory_indexes() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("data", false, 0).unwrap();
        backend.write_at("data", 0, b"x").unwrap();
        let (usage_scans, directory_scans) = {
            let cache = backend.cache.borrow();
            (cache.usage_scans, cache.directory_scans)
        };
        for n in 1..12 {
            backend
                .write_at("DATA", n * FATX_CLUSTER_SIZE, b"x")
                .unwrap();
            let volume = backend.volume_info().unwrap();
            assert_eq!(
                volume.total_allocation_units - volume.available_allocation_units,
                n + 1
            );
        }
        for n in 0..8 {
            let name = format!("extra{n}");
            backend.create(&name, false, 0).unwrap();
            backend.remove(&name).unwrap();
        }
        let cache = backend.cache.borrow();
        assert_eq!(cache.usage_scans, usage_scans);
        assert_eq!(cache.directory_scans, directory_scans);
        drop(cache);
        backend.set_len("data", 0).unwrap();
        assert_eq!(
            backend.volume_info().unwrap().available_allocation_units,
            backend.volume_info().unwrap().total_allocation_units
        );
        backend.remove("data").unwrap();
        backend.create("folder", true, 0).unwrap();
        let volume = backend.volume_info().unwrap();
        assert_eq!(
            volume.total_allocation_units - volume.available_allocation_units,
            1
        );
        backend.remove("folder").unwrap();
        let volume = backend.volume_info().unwrap();
        assert_eq!(
            volume.total_allocation_units,
            volume.available_allocation_units
        );
    }

    #[test]
    fn metadata_retirement_failure_after_unlink_leaves_an_orphan_recreation_clears() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("save", false, ATTR_HIDDEN).unwrap();
        backend.write_at("save", 0, b"payload").unwrap();
        let record = backend.metadata_path("save");
        let state = record.parent().unwrap();
        let moved = temp.0.join("old-state");
        fs::rename(state, &moved).unwrap();
        fs::write(state, b"blocked metadata directory").unwrap();
        // The file goes first; the record's retirement then fails, which is
        // logged, not an error: the remove happened.
        assert_eq!(backend.remove("save"), Ok(()));
        assert!(!backend.root.join("save").exists());
        fs::remove_file(state).unwrap();
        fs::rename(moved, state).unwrap();
        assert!(record.exists(), "an orphan record");
        backend.create("save", false, 0).unwrap();
        assert_eq!(backend.stat("save").unwrap().basic.attributes, ATTR_ARCHIVE);
        let bytes = fs::read(&record).unwrap();
        assert_eq!(bytes.len(), 40);
        assert_eq!(&bytes[..4], b"HVF1");
        assert!(!state.join("record.new").exists());
    }
    #[test]
    fn failed_directory_unlink_preserves_metadata_and_litter_cleanup_never_follows_links() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("folder", true, ATTR_HIDDEN).unwrap();
        let folder = backend.root.join("folder");
        fs::write(folder.join(".DS_Store"), b"finder").unwrap();
        fs::write(folder.join("unaddressable:"), b"keep").unwrap();
        let record = backend.metadata_path("folder");
        let before = fs::read(&record).unwrap();
        assert!(backend.read_directory("folder").unwrap().is_empty());
        assert_eq!(backend.remove("folder"), Err(Error::DirectoryNotEmpty));
        assert_eq!(fs::read(&record).unwrap(), before);
        assert_eq!(
            backend.stat("folder").unwrap().basic.attributes,
            ATTR_HIDDEN | ATTR_DIRECTORY
        );
        assert_eq!(fs::read(folder.join("unaddressable:")).unwrap(), b"keep");
        fs::remove_file(folder.join("unaddressable:")).unwrap();
        #[cfg(unix)]
        {
            let outside = temp.0.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("payload"), b"private").unwrap();
            std::os::unix::fs::symlink(&outside, folder.join("._link")).unwrap();
        }
        backend.remove("folder").unwrap();
        assert!(!folder.exists());
        assert!(!record.parent().unwrap().exists());
        #[cfg(unix)]
        assert_eq!(
            fs::read(temp.0.join("outside/payload")).unwrap(),
            b"private"
        );
    }

    #[test]
    fn read_only_cleanup_persists_atime_without_syncing_file_data_or_directories() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("file", false, 0).unwrap();
        backend.write_at("file", 0, b"data").unwrap();
        backend.flush("file").unwrap();
        backend.create("folder", true, 0).unwrap();
        let before = backend.cache.borrow().data_syncs;
        backend.cleanup("file", false).unwrap();
        backend.cleanup("folder", false).unwrap();
        assert_eq!(backend.cache.borrow().data_syncs, before);
        backend.read_at("file", 0, &mut [0]).unwrap();
        assert_eq!(backend.cache.borrow().atimes.len(), 1);
        backend.cleanup("file", false).unwrap();
        assert!(backend.cache.borrow().atimes.is_empty());
        assert_eq!(backend.cache.borrow().data_syncs, before);
        backend.cleanup("file", true).unwrap();
        assert_eq!(backend.cache.borrow().data_syncs, before + 1);
        backend.write_at("file", 0, b"x").unwrap();
        backend.cleanup("file", false).unwrap();
        assert_eq!(backend.cache.borrow().data_syncs, before + 2);
    }

    #[test]
    fn parent_sync_failure_does_not_turn_successful_create_into_an_error() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        for (name, directory) in [("file", false), ("folder", true)] {
            backend
                .next_create_sync_error
                .set(Some(Error::IoDeviceError));
            assert_eq!(backend.create(name, directory, ATTR_HIDDEN), Ok(()));
            assert_eq!(backend.next_create_sync_error.get(), None);
            assert!(backend.root.join(name).exists());
            assert_eq!(backend.stat(name).unwrap().is_directory(), directory);
            assert!(backend.stat(name).unwrap().basic.attributes & ATTR_HIDDEN != 0);
            assert_eq!(
                backend.create(name, directory, 0),
                Err(Error::ObjectNameCollision)
            );
        }
        let volume = backend.volume_info().unwrap();
        assert_eq!(
            volume.total_allocation_units - volume.available_allocation_units,
            1
        );
    }

    #[test]
    fn a_failed_remove_keeps_the_writes_pending_metadata() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.set_stale(Duration::from_secs(3600));
        backend.create("data", false, 0).unwrap();
        backend.flush("data").unwrap();
        let mut basic = backend.stat("data").unwrap().basic;
        basic.attributes = 0;
        basic.last_write_time = Timestamp(125_911_584_000_000_000);
        backend.set_basic_info("data", basic).unwrap();
        backend.write_at("data", 0, b"x").unwrap();
        let written = backend.stat("data").unwrap().basic;
        assert!(written.last_write_time > basic.last_write_time);
        let record = fs::read(backend.metadata_path("data")).unwrap();
        // Unlink fails: the file, its durable record and the pending write
        // all stay.
        backend.next_unlink_error.set(Some(Error::AccessDenied));
        assert_eq!(backend.remove("data"), Err(Error::AccessDenied));
        assert!(backend.root.join("data").exists());
        assert_eq!(fs::read(backend.metadata_path("data")).unwrap(), record);
        assert_eq!(backend.pending_written(), 1);
        assert_eq!(backend.stat("data").unwrap().basic, written);
        // An exit right then (no drop, no flush) keeps the old record: the
        // reopened file has its old times, not host defaults.
        std::mem::forget(backend);
        let mut reopened = temp.backend();
        assert_eq!(reopened.stat("data").unwrap().basic, basic);
        // A remove that succeeds drops the entry and the record with the
        // file; a retirement that fails after the unlink only leaves an
        // orphan record, cleared when the name is created again.
        reopened.set_stale(Duration::from_secs(3600));
        reopened.write_at("data", 0, b"y").unwrap();
        assert_eq!(reopened.remove("data"), Ok(()));
        assert_eq!(reopened.pending_written(), 0);
        assert!(!reopened.metadata_path("data").exists());
        reopened.create("data", false, ATTR_HIDDEN).unwrap();
        reopened
            .next_metadata_retire_error
            .set(Some(Error::IoDeviceError));
        assert_eq!(reopened.remove("data"), Ok(()));
        assert!(!reopened.root.join("data").exists());
        assert!(reopened.metadata_path("data").exists(), "orphan");
        reopened.create("data", false, 0).unwrap();
        assert_eq!(
            reopened.stat("data").unwrap().basic.attributes,
            ATTR_ARCHIVE
        );
    }

    #[test]
    fn a_failed_format_keeps_pending_metadata() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.set_stale(Duration::from_secs(3600));
        backend.create("data", false, 0).unwrap();
        backend.write_at("data", 0, b"x").unwrap();
        assert_eq!(backend.pending_written(), 1);
        backend.next_format_error.set(Some(Error::IoDeviceError));
        assert_eq!(backend.format(), Err(Error::IoDeviceError));
        assert!(backend.root.join("data").exists());
        assert_eq!(backend.pending_written(), 1);
        backend.format().unwrap();
        assert_eq!(backend.pending_written(), 0);
        assert!(!backend.root.join("data").exists());
    }

    #[test]
    fn a_partly_failed_format_drops_pending_metadata_of_the_files_it_removed() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.set_stale(Duration::from_secs(3600));
        for name in ["one", "two"] {
            backend.create(name, false, 0).unwrap();
            backend.write_at(name, 0, b"x").unwrap();
        }
        assert_eq!(backend.pending_written(), 2);
        // The first removal succeeds, the second fails: one file is gone and
        // one survives, whichever order the host lists them in.
        backend.format_error_after_unlinks.set(Some(1));
        assert_eq!(backend.format(), Err(Error::IoDeviceError));
        let [gone, kept] = if backend.root.join("one").exists() {
            ["two", "one"]
        } else {
            ["one", "two"]
        };
        assert!(!backend.root.join(gone).exists());
        assert!(backend.root.join(kept).exists());
        assert_eq!(backend.pending_written(), 1);
        assert_eq!(
            backend.stat(gone).map(|_| ()),
            Err(Error::ObjectNameNotFound)
        );
        let kept_basic = backend.stat(kept).unwrap().basic;
        // A flush stores only the survivor's pending metadata. The removed
        // file's record may outlive the failed format as an orphan, which
        // creating the name clears.
        backend.written.flush(None).unwrap();
        assert_eq!(backend.pending_written(), 0);
        backend.create(gone, false, ATTR_HIDDEN).unwrap();
        assert_eq!(
            backend.stat(gone).unwrap().basic.attributes,
            ATTR_HIDDEN | ATTR_ARCHIVE
        );
        drop(backend);
        let mut reopened = temp.backend();
        assert_eq!(reopened.stat(kept).unwrap().basic, kept_basic);
    }

    #[test]
    fn orphaned_directory_metadata_is_not_attached_to_recreated_directory() {
        let temp = Temp::new();
        let mut backend = temp.backend();
        backend.create("folder", true, ATTR_HIDDEN).unwrap();
        let record = backend.metadata_path("folder");
        backend
            .next_metadata_retire_error
            .set(Some(Error::IoDeviceError));
        assert_eq!(backend.remove("folder"), Ok(()));
        assert!(!backend.root.join("folder").exists());
        assert!(record.exists());
        backend
            .next_metadata_retire_error
            .set(Some(Error::IoDeviceError));
        assert_eq!(backend.create("folder", true, 0), Err(Error::IoDeviceError));
        assert!(!backend.root.join("folder").exists());
        backend.create("folder", true, 0).unwrap();
        assert_eq!(
            backend.stat("folder").unwrap().basic.attributes,
            ATTR_DIRECTORY
        );
    }
}
