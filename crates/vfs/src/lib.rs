//! Xbox object-manager storage, independent of guest memory and kernel ABI packing.
//!
//! See `docs/storage.md` for mappings, supported semantics, and the integration contract.
#![forbid(unsafe_code)]

mod backend;
mod disc;
mod host;
mod path;
mod status;
mod types;

pub use backend::Backend;
pub use disc::DiscBackend;
pub use host::HostBackend;
pub use path::{Namespace, validate_fatx_name, validate_fatx_path, wildcard_matches};
pub use status::{Error, Result};
pub use types::*;

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

pub const CDROM_DEVICE: &str = "\\Device\\CdRom0";
pub const DATA_DEVICE: &str = "\\Device\\Harddisk0\\Partition1";

#[derive(Debug, Clone)]
pub struct Config {
    pub data_root: PathBuf,
    pub title_id: u32,
    /// Retail utility area is selected from cache partitions 3..=5.
    pub utility_partition: u8,
}
impl Config {
    pub fn new(data_root: impl Into<PathBuf>, title_id: u32) -> Self {
        Self {
            data_root: data_root.into(),
            title_id,
            utility_partition: 5,
        }
    }
    pub fn for_title(title_id: u32) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|p| !p.is_empty())
            .ok_or(Error::InvalidParameter)?;
        Ok(Self::new(
            PathBuf::from(home).join("Library/Application Support/h2recomp"),
            title_id,
        ))
    }
}

struct Mount {
    device: String,
    backend: Box<dyn Backend>,
}
struct Enumeration {
    entries: Vec<DirectoryEntry>,
    next: usize,
    pattern: String,
}
struct OpenFile {
    mount: usize,
    path: String,
    key: String,
    options: OpenOptions,
    position: u64,
    enumeration: Option<Enumeration>,
    delete_requested: bool,
    directory: bool,
}

/// One storage session and its NT file objects. Use one Vfs for all guest threads;
/// put a mutex around it in the kernel. Share checks span every alias within this instance.
#[derive(Default)]
pub struct Vfs {
    pub namespace: Namespace,
    mounts: Vec<Mount>,
    handles: BTreeMap<u64, u64>,
    objects: BTreeMap<u64, OpenFile>,
    pending: HashSet<(usize, String)>,
    cleanup_pending: HashSet<(usize, String)>,
    next_handle: u64,
}
impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Standard hard disk layout: mounts the device objects only (`\\Device\\Harddisk0\\PartitionN`)
    /// and creates no drive-letter links. The kernel provides devices; XAPI creates D:/T:/U:/Z: itself
    /// and treats an existing T: as a fatal collision. Host tools that want the links call
    /// [`Vfs::create_standard_links`]. Callers may mount CDROM_DEVICE separately for a disc-less session.
    pub fn with_storage(config: Config) -> Result<Self> {
        if !(3..=5).contains(&config.utility_partition) {
            return Err(Error::InvalidParameter);
        }
        let mut vfs = Self::new();
        for partition in 1..=5 {
            let name = format!("Partition{partition}");
            let mut backend = HostBackend::new(
                config.data_root.join("disk").join(&name),
                config.data_root.join("metadata").join(&name),
            )?
            .with_volume(
                name.clone(),
                partition,
                match partition {
                    1 => 5 * 1024 * 1024 * 1024,
                    2 => 500 * 1024 * 1024,
                    _ => 750 * 1024 * 1024,
                },
            );
            if partition == 1 {
                for base in ["TDATA", "UDATA"] {
                    for dir in [base.to_owned(), format!("{base}\\{:08X}", config.title_id)] {
                        match backend.stat(&dir) {
                            Ok(m) if m.is_directory() => {}
                            Ok(_) => return Err(Error::NotADirectory),
                            Err(Error::ObjectNameNotFound) => backend.create(&dir, true, 0)?,
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            vfs.mount(&format!("\\Device\\Harddisk0\\{name}"), backend)?;
        }
        Ok(vfs)
    }
    /// Creates D:, T:, U: and Z: for host tools that have no XAPI. Not for the boot or game path:
    /// XAPI creates these links and fails if T: already exists. Pass the `Config` given to
    /// `with_storage`; errors with `ObjectNameCollision` if any link exists already. Not atomic: links
    /// created before a failure are not rolled back.
    pub fn create_standard_links(&mut self, config: &Config) -> Result<()> {
        if !(3..=5).contains(&config.utility_partition) {
            return Err(Error::InvalidParameter);
        }
        for (drive, target) in [
            ('D', CDROM_DEVICE.to_owned()),
            (
                'Z',
                format!("\\Device\\Harddisk0\\Partition{}", config.utility_partition),
            ),
            (
                'T',
                format!("{DATA_DEVICE}\\TDATA\\{:08X}", config.title_id),
            ),
            (
                'U',
                format!("{DATA_DEVICE}\\UDATA\\{:08X}", config.title_id),
            ),
        ] {
            self.namespace
                .create_symbolic_link(&format!("\\??\\{drive}:"), &target)?;
        }
        Ok(())
    }
    pub fn with_disc(config: Config, disc: impl AsRef<std::path::Path>) -> Result<Self> {
        // Fail a bad image before creating persistent storage directories.
        let disc = DiscBackend::open(disc)?;
        let mut vfs = Self::with_storage(config)?;
        vfs.mount(CDROM_DEVICE, disc)?;
        Ok(vfs)
    }
    pub fn mount(&mut self, device: &str, backend: impl Backend + 'static) -> Result<()> {
        let device = path::object_name(device)?;
        if !device.to_ascii_lowercase().starts_with("\\device\\") {
            return Err(Error::ObjectNameInvalid);
        }
        if self.mounts.iter().any(|m| {
            path::prefix(&device, &m.device).is_some() || path::prefix(&m.device, &device).is_some()
        }) {
            return Err(Error::ObjectNameCollision);
        }
        self.mounts.push(Mount {
            device,
            backend: Box::new(backend),
        });
        Ok(())
    }
    fn resolve(&self, name: &str) -> Result<(usize, String)> {
        let resolved = self.namespace.resolve(name)?;
        for (index, mount) in self.mounts.iter().enumerate() {
            if let Some(rest) = path::prefix(&resolved, &mount.device) {
                path::components(rest)?;
                return Ok((index, rest.to_owned()));
            }
        }
        Err(Error::ObjectPathNotFound)
    }
    fn object(&self, handle: Handle) -> Result<&OpenFile> {
        let id = self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        self.objects.get(id).ok_or(Error::InvalidHandle)
    }
    fn object_mut(&mut self, handle: Handle) -> Result<&mut OpenFile> {
        let id = self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        self.objects.get_mut(id).ok_or(Error::InvalidHandle)
    }
    fn allocate_handle(&mut self) -> Result<u64> {
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(Error::InvalidHandle)?;
        Ok(self.next_handle)
    }
    pub fn open(&mut self, name: &str, mut options: OpenOptions) -> Result<OpenResult> {
        if name.ends_with('\\') {
            if options.directory == Some(false) {
                return Err(Error::InvalidParameter);
            }
            options.directory = Some(true);
        }
        self.open_resolved(self.resolve(name)?, options)
    }
    /// OBJECT_ATTRIBUTES.RootDirectory support. Absolute names must use open().
    pub fn open_relative(
        &mut self,
        directory: Handle,
        name: &str,
        options: OpenOptions,
    ) -> Result<OpenResult> {
        path::components(name)?;
        let root = self.object(directory)?;
        let (mount, root_path) = (root.mount, root.path.clone());
        if !self.mounts[mount].backend.stat(&root_path)?.is_directory() {
            return Err(Error::NotADirectory);
        }
        let joined = if root_path.is_empty() {
            name.to_owned()
        } else if name.is_empty() {
            root_path
        } else {
            format!("{root_path}\\{name}")
        };
        self.open_resolved((mount, joined), options)
    }
    fn open_resolved(
        &mut self,
        (mount, path): (usize, String),
        mut options: OpenOptions,
    ) -> Result<OpenResult> {
        options.access = Access::from_nt(options.access.0)?;
        if options.share.0 & !7 != 0 || options.attributes & !(FATX_ATTRIBUTES | ATTR_NORMAL) != 0 {
            return Err(Error::InvalidParameter);
        }
        if options.delete_on_close
            && (path.is_empty()
                || options.attributes & ATTR_READONLY != 0
                || !options.access.contains(Access::DELETE))
        {
            return Err(Error::AccessDenied);
        }
        let key = path.to_ascii_lowercase();
        if self
            .pending
            .iter()
            .any(|(m, p)| *m == mount && path::prefix(&key, p).is_some())
        {
            return Err(Error::DeletePending);
        }
        let overwriting = matches!(
            options.disposition,
            CreateDisposition::Supersede
                | CreateDisposition::Overwrite
                | CreateDisposition::OverwriteIf
        );
        if overwriting && options.directory == Some(true) {
            return Err(Error::InvalidParameter);
        }
        if matches!(
            options.disposition,
            CreateDisposition::Overwrite | CreateDisposition::OverwriteIf
        ) && !options.access.contains(Access::WRITE)
        {
            return Err(Error::AccessDenied);
        }
        let existing = match self.mounts[mount].backend.stat(&path) {
            Ok(m) => Some(m),
            Err(Error::ObjectNameNotFound) => None,
            Err(e) => return Err(e),
        };
        if let Some(info) = &existing {
            if options.disposition == CreateDisposition::Create {
                return Err(Error::ObjectNameCollision);
            }
            if options.directory == Some(true) && !info.is_directory() {
                return Err(Error::NotADirectory);
            }
            if options.directory == Some(false) && info.is_directory() {
                return Err(Error::FileIsADirectory);
            }
            if overwriting && info.is_directory() {
                return Err(Error::FileIsADirectory);
            }
        } else if matches!(
            options.disposition,
            CreateDisposition::Open | CreateDisposition::Overwrite
        ) {
            return Err(Error::ObjectNameNotFound);
        }
        let read_only = self.mounts[mount].backend.read_only();
        if read_only
            && (existing.is_none()
                || overwriting
                || options.delete_on_close
                || options.access.0
                    & (Access::WRITE.0
                        | Access::APPEND.0
                        | Access::WRITE_ATTRIBUTES.0
                        | Access::DELETE.0)
                    != 0)
        {
            return Err(Error::MediaWriteProtected);
        }
        if existing
            .as_ref()
            .is_some_and(|info| info.basic.attributes & ATTR_READONLY != 0)
            && (options.access.0 & (Access::WRITE.0 | Access::APPEND.0 | Access::DELETE.0) != 0
                || overwriting)
        {
            return Err(Error::AccessDenied);
        }
        // Bidirectional checks: old share must permit new access, and new share must permit old access.
        // Overwrite/supersede also modify data even when a caller passes unusual rights.
        for old in self
            .objects
            .values()
            .filter(|f| f.mount == mount && f.key == key)
        {
            let checked_access =
                if existing.is_some() && options.disposition == CreateDisposition::Supersede {
                    options.access | Access::DELETE
                } else {
                    options.access
                };
            if checked_access.sharing() & !old.options.share.0 != 0
                || old.options.access.sharing() & !options.share.0 != 0
            {
                return Err(Error::SharingViolation);
            }
        }
        if options.delete_on_close
            && existing.as_ref().is_some_and(Metadata::is_directory)
            && !self.mounts[mount].backend.read_directory(&path)?.is_empty()
        {
            return Err(Error::DirectoryNotEmpty);
        }
        let information = if existing.is_some() {
            if overwriting {
                self.mounts[mount].backend.set_len(&path, 0)?;
                let mut basic = self.mounts[mount].backend.stat(&path)?.basic;
                basic.attributes = if options.disposition == CreateDisposition::Supersede {
                    (options.attributes & FATX_ATTRIBUTES & !ATTR_DIRECTORY) | ATTR_ARCHIVE
                } else {
                    basic.attributes
                        | (options.attributes & FATX_ATTRIBUTES & !ATTR_DIRECTORY)
                        | ATTR_ARCHIVE
                };
                self.mounts[mount].backend.set_basic_info(&path, basic)?;
                if options.disposition == CreateDisposition::Supersede {
                    CreateInformation::Superseded
                } else {
                    CreateInformation::Overwritten
                }
            } else {
                CreateInformation::Opened
            }
        } else {
            self.mounts[mount].backend.create(
                &path,
                options.directory == Some(true),
                options.attributes,
            )?;
            CreateInformation::Created
        };
        let handle = self.allocate_handle()?;
        self.handles.insert(handle, handle);
        self.objects.insert(
            handle,
            OpenFile {
                mount,
                path,
                key: key.clone(),
                options,
                position: 0,
                enumeration: None,
                delete_requested: false,
                directory: existing
                    .as_ref()
                    .map_or(options.directory == Some(true), Metadata::is_directory),
            },
        );
        Ok(OpenResult {
            handle: Handle(handle),
            information,
        })
    }
    /// NtDuplicateObject: same file object, including position, search cursor and share reservation.
    pub fn duplicate(&mut self, handle: Handle) -> Result<Handle> {
        let object = *self.handles.get(&handle.0).ok_or(Error::InvalidHandle)?;
        let duplicate = self.allocate_handle()?;
        self.handles.insert(duplicate, object);
        Ok(Handle(duplicate))
    }
    pub fn close(&mut self, handle: Handle) -> Result<()> {
        let id = self.handles.remove(&handle.0).ok_or(Error::InvalidHandle)?;
        if self.handles.values().any(|object| *object == id) {
            return Ok(());
        }
        let object = self.objects.remove(&id).ok_or(Error::InvalidHandle)?;
        let key = (object.mount, object.key.clone());
        let write_access = !object.directory
            && object.options.access.0 & (Access::WRITE.0 | Access::APPEND.0) != 0;
        if let Err(error) = self.mounts[object.mount]
            .backend
            .cleanup(&object.path, write_access)
        {
            // The handle is already closed. Preserve close success; explicit flush is
            // the API for callers that need a synchronous durability error.
            eprintln!("vfs: file-object cleanup sync failed: {error}");
        }
        if object.options.delete_on_close || object.delete_requested {
            self.pending.insert(key.clone());
            self.cleanup_pending.insert(key.clone());
        }
        if self.pending.contains(&key)
            && !self
                .objects
                .values()
                .any(|f| f.mount == object.mount && f.key == object.key)
        {
            let result = self.mounts[object.mount].backend.remove(&object.path);
            self.pending.remove(&key);
            self.cleanup_pending.remove(&key);
            result?;
        }
        Ok(())
    }
    pub fn query_information(&mut self, handle: Handle) -> Result<FileInformation> {
        let file = self.object(handle)?;
        let (mount, path, position, access, pending) = (
            file.mount,
            file.path.clone(),
            file.position,
            file.options.access,
            self.pending.contains(&(file.mount, file.key.clone())),
        );
        Ok(FileInformation {
            metadata: self.mounts[mount].backend.stat(&path)?,
            position,
            access,
            delete_pending: pending,
            device_path: if path.is_empty() {
                self.mounts[mount].device.clone()
            } else {
                format!("{}\\{path}", self.mounts[mount].device)
            },
        })
    }
    pub fn query_path(&mut self, path: &str) -> Result<Metadata> {
        let (mount, path) = self.resolve(path)?;
        self.mounts[mount].backend.stat(&path)
    }
    pub fn query_volume_information(&mut self, handle: Handle) -> Result<VolumeInfo> {
        let mount = self.object(handle)?.mount;
        self.mounts[mount].backend.volume_info()
    }
    pub fn set_position(&mut self, handle: Handle, position: u64) -> Result<()> {
        if position > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        self.object_mut(handle)?.position = position;
        Ok(())
    }
    /// Synchronous I/O: both current and explicit successful reads update the file-object position.
    pub fn read(&mut self, handle: Handle, offset: Offset, buffer: &mut [u8]) -> Result<usize> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::READ) {
            return Err(Error::AccessDenied);
        }
        let at = match offset {
            Offset::Current => file.position,
            Offset::Absolute(n) => n,
            Offset::EndOfFile => return Err(Error::InvalidParameter),
        };
        if at > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        let (mount, path) = (file.mount, file.path.clone());
        let len = self.mounts[mount].backend.read_at(&path, at, buffer)?;
        if len == 0 && !buffer.is_empty() {
            return Err(Error::EndOfFile);
        }
        self.object_mut(handle)?.position = at + len as u64;
        Ok(len)
    }
    pub fn write(&mut self, handle: Handle, offset: Offset, buffer: &[u8]) -> Result<usize> {
        let file = self.object(handle)?;
        if file.options.access.0 & (Access::WRITE.0 | Access::APPEND.0) == 0 {
            return Err(Error::AccessDenied);
        }
        let (mount, path, position) = (file.mount, file.path.clone(), file.position);
        let append_only = !file.options.access.contains(Access::WRITE);
        let at = if append_only || offset == Offset::EndOfFile {
            self.mounts[mount].backend.stat(&path)?.end_of_file
        } else {
            match offset {
                Offset::Current => position,
                Offset::Absolute(n) => n,
                Offset::EndOfFile => unreachable!(),
            }
        };
        if at > i64::MAX as u64 {
            return Err(Error::InvalidParameter);
        }
        let len = self.mounts[mount].backend.write_at(&path, at, buffer)?;
        self.object_mut(handle)?.position = at + len as u64;
        Ok(len)
    }
    pub fn set_end_of_file(&mut self, handle: Handle, size: u64) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::WRITE) {
            return Err(Error::AccessDenied);
        }
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.set_len(&path, size)
    }
    /// Full logical metadata replacement. Kernel merges zero/-1 NT sentinel fields first.
    pub fn set_basic_information(&mut self, handle: Handle, basic: BasicInfo) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::WRITE_ATTRIBUTES) {
            return Err(Error::AccessDenied);
        }
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.set_basic_info(&path, basic)
    }
    pub fn set_delete_pending(&mut self, handle: Handle, delete: bool) -> Result<()> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::DELETE) {
            return Err(Error::AccessDenied);
        }
        let (mount, path, key) = (file.mount, file.path.clone(), file.key.clone());
        if delete {
            if path.is_empty() {
                return Err(Error::AccessDenied);
            }
            let metadata = self.mounts[mount].backend.stat(&path)?;
            if metadata.basic.attributes & ATTR_READONLY != 0 {
                return Err(Error::AccessDenied);
            }
            if metadata.is_directory()
                && !self.mounts[mount].backend.read_directory(&path)?.is_empty()
            {
                return Err(Error::DirectoryNotEmpty);
            }
        }
        self.object_mut(handle)?.delete_requested = delete;
        if delete {
            self.pending.insert((mount, key));
        } else if !self.cleanup_pending.contains(&(mount, key.clone()))
            && !self
                .objects
                .values()
                .any(|f| f.mount == mount && f.key == key && f.delete_requested)
        {
            self.pending.remove(&(mount, key));
        }
        Ok(())
    }
    pub fn flush(&mut self, handle: Handle) -> Result<()> {
        let file = self.object(handle)?;
        let (mount, path) = (file.mount, file.path.clone());
        self.mounts[mount].backend.flush(&path)
    }
    /// Returns one entry, so kernel buffer overflow never consumes additional entries.
    /// The first expression is captured for the file object; later expressions are ignored.
    /// Restart refreshes the snapshot and rewinds using that captured expression.
    pub fn query_directory(
        &mut self,
        handle: Handle,
        pattern: Option<&str>,
        restart: bool,
    ) -> Result<DirectoryEntry> {
        self.peek_directory(handle, pattern, restart)?;
        self.consume_directory(handle)
    }
    /// Pack first, then call consume_directory only after the entry fits the guest buffer.
    pub fn peek_directory(
        &mut self,
        handle: Handle,
        pattern: Option<&str>,
        restart: bool,
    ) -> Result<DirectoryEntry> {
        let file = self.object(handle)?;
        if !file.options.access.contains(Access::READ) {
            return Err(Error::AccessDenied);
        }
        let first = file.enumeration.is_none();
        if first || restart {
            let expression = file
                .enumeration
                .as_ref()
                .map(|e| e.pattern.as_str())
                .unwrap_or(pattern.filter(|p| !p.is_empty()).unwrap_or("*"));
            if expression.is_empty()
                || !expression.is_ascii()
                || expression
                    .bytes()
                    .any(|b| !(0x20..0x7f).contains(&b) || b"\\/:\"<>|".contains(&b))
            {
                return Err(Error::ObjectNameInvalid);
            }
            let (mount, path, expression) = (file.mount, file.path.clone(), expression.to_owned());
            let mut entries = self.mounts[mount].backend.read_directory(&path)?;
            entries.retain(|e| wildcard_matches(&expression, &e.name));
            entries.sort_by_key(|e| e.name.to_ascii_lowercase());
            self.object_mut(handle)?.enumeration = Some(Enumeration {
                entries,
                next: 0,
                pattern: expression,
            });
        }
        let search = self.object(handle)?.enumeration.as_ref().unwrap();
        search.entries.get(search.next).cloned().ok_or(if first {
            Error::NoSuchFile
        } else {
            Error::NoMoreFiles
        })
    }
    pub fn consume_directory(&mut self, handle: Handle) -> Result<DirectoryEntry> {
        let search = self
            .object_mut(handle)?
            .enumeration
            .as_mut()
            .ok_or(Error::InvalidParameter)?;
        let entry = search
            .entries
            .get(search.next)
            .cloned()
            .ok_or(Error::NoMoreFiles)?;
        search.next += 1;
        Ok(entry)
    }
}

impl Drop for Vfs {
    fn drop(&mut self) {
        // Normal shutdown closes file objects, including delete-on-close requests.
        // Explicit close() is required when the caller needs to observe a cleanup failure.
        let handles: Vec<_> = self.handles.keys().copied().collect();
        for handle in handles {
            let _ = self.close(Handle(handle));
        }
    }
}
