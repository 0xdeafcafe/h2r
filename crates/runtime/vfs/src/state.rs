//! Open file objects as plain data, for machine snapshots
//! (docs/runtime/snapshots.md, phase 3). A save records every open object
//! by device and path with its handle numbers, options, position and
//! search cursor; a load re-opens them on the booted VFS's mounts under the
//! same numbers. A file open for writing also carries its whole contents
//! (up to [`MAX_CARRIED_BYTES`]), and a load writes them back before it
//! re-opens the file: the title keeps `z:\preferences.dat` open for
//! writing all the time it runs. While a delete is pending, or an object
//! may delete, the host disk holds state the snapshot does not, so the
//! save is refused.
use crate::{
    Access, Backend, CreateDisposition, Enumeration, Error, Handle, OpenFile, OpenOptions,
    ShareAccess, Vfs, path, wildcard_matches,
};

/// The largest file a snapshot carries for an object open for writing.
/// The title's own (`preferences.dat`) is 0x1fc bytes; a cache slot being
/// copied is hundreds of MB and refuses the save.
pub const MAX_CARRIED_BYTES: u64 = 1 << 20;

/// The VFS's file objects and handle numbering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VfsState {
    pub next_handle: u64,
    /// Handle number to file object identity (duplicates share one).
    pub handles: Vec<(u64, u64)>,
    pub objects: Vec<SavedFile>,
}

/// One open file object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFile {
    pub id: u64,
    /// The mount's device name (`\Device\CdRom0`, ...).
    pub device: String,
    pub path: String,
    pub access: u32,
    pub share: u32,
    pub disposition: u32,
    /// The open's FILE_DIRECTORY_FILE / FILE_NON_DIRECTORY_FILE request.
    pub want_directory: Option<bool>,
    pub attributes: u32,
    pub position: u64,
    /// The object is a directory.
    pub directory: bool,
    /// A directory search in progress: its pattern and next entry. The
    /// entries are read again at load.
    pub search: Option<(String, u64)>,
    /// The object wrote to the file since it was opened (reported as
    /// written at its last close).
    pub changed: bool,
    /// The file's bytes when the object can write to it, or wrote to it:
    /// written back at load, before the object is re-opened.
    pub contents: Option<Vec<u8>>,
}

/// Why the VFS cannot be saved or loaded.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("an editor map rebind is in progress")]
    MapReloadBusy,
    #[error("snapshots cannot reproduce the editor play copy at {0}")]
    MapBinding(String),
    #[error("{0} is open for deletion")]
    Deletable(String),
    #[error("{path} is open for writing and holds {bytes} bytes, more than a snapshot carries")]
    TooLarge { path: String, bytes: u64 },
    #[error("{path} cannot be read for the snapshot: {error}")]
    Read { path: String, error: Error },
    #[error("a delete is pending on {0} paths")]
    DeletePending(usize),
    #[error("device {0} is not mounted")]
    NoMount(String),
    #[error("{path} cannot be re-opened: {error}")]
    Reopen { path: String, error: Error },
    #[error("saved handles are inconsistent")]
    Invalid,
}

impl Vfs {
    /// Machine snapshots identify the boot mounts, but cannot reproduce a
    /// play copy published later (even with its DVD file already closed).
    pub fn check_snapshot_maps(&self) -> Result<(), StateError> {
        for mount in &self.mounts {
            if let Some((path, _)) = mount.backend.snapshot_map_bindings()?.first() {
                return Err(StateError::MapBinding(format!("{}\\{path}", mount.device)));
            }
        }
        Ok(())
    }

    /// The open objects, or why they cannot be saved. A file open for
    /// writing is read whole into its [`SavedFile::contents`].
    pub fn state(&mut self) -> Result<VfsState, StateError> {
        self.check_snapshot_maps()?;
        let pending = self.pending.len() + self.cleanup_pending.len();
        if pending != 0 {
            return Err(StateError::DeletePending(pending));
        }
        let mut objects = Vec::with_capacity(self.objects.len());
        for (&id, f) in &self.objects {
            let mount = &mut self.mounts[f.mount];
            let device = &mount.device;
            if f.options.access.contains(Access::DELETE)
                || f.options.delete_on_close
                || f.delete_requested
            {
                return Err(StateError::Deletable(format!("{device}\\{}", f.path)));
            }
            let writes = Access::WRITE.0 | Access::APPEND.0;
            let contents = if !f.directory && (f.options.access.0 & writes != 0 || f.changed) {
                let named = || format!("{device}\\{}", f.path);
                let read = |error| StateError::Read {
                    path: named(),
                    error,
                };
                let bytes = mount.backend.stat(&f.path).map_err(read)?.end_of_file;
                if bytes > MAX_CARRIED_BYTES {
                    return Err(StateError::TooLarge {
                        path: named(),
                        bytes,
                    });
                }
                Some(read_whole(mount.backend.as_mut(), &f.path, bytes).map_err(read)?)
            } else {
                None
            };
            objects.push(SavedFile {
                id,
                device: device.clone(),
                path: f.path.clone(),
                access: f.options.access.0,
                share: f.options.share.0,
                disposition: f.options.disposition as u32,
                want_directory: f.options.directory,
                attributes: f.options.attributes,
                position: f.position,
                directory: f.directory,
                search: f
                    .enumeration
                    .as_ref()
                    .map(|e| (e.pattern.clone(), e.next as u64)),
                changed: f.changed,
                contents,
            });
        }
        Ok(VfsState {
            next_handle: self.next_handle,
            handles: self.handles.iter().map(|(&h, &o)| (h, o)).collect(),
            objects,
        })
    }

    /// Close every open object, write back the files the saved ones carry
    /// and re-open them under their own handle numbers. Every saved file is
    /// looked up first: on an error nothing has changed.
    pub fn restore_state(&mut self, state: &VfsState) -> Result<(), StateError> {
        let apply = self.prepare_restore(state)?;
        apply(self);
        Ok(())
    }

    /// Resolve the saved objects on this mount before the machine writes guest RAM.
    /// Apply the returned file table to this VFS at the same slice boundary.
    /// The apply writes back each carried file ([`SavedFile::contents`]),
    /// creating it if it is gone, after the open objects close.
    pub fn prepare_restore(
        &mut self,
        state: &VfsState,
    ) -> Result<impl FnOnce(&mut Vfs) + use<>, StateError> {
        self.check_snapshot_maps()?;
        if state
            .handles
            .iter()
            .any(|&(h, o)| h > state.next_handle || !state.objects.iter().any(|f| f.id == o))
        {
            return Err(StateError::Invalid);
        }
        let mut reopened = Vec::with_capacity(state.objects.len());
        // Carried files, one per path (duplicate objects carry the same bytes).
        let mut writes: Vec<(usize, String, Vec<u8>)> = Vec::new();
        for f in &state.objects {
            let mount = self
                .mounts
                .iter()
                .position(|m| m.device == f.device)
                .ok_or_else(|| StateError::NoMount(f.device.clone()))?;
            let reopen = |error| StateError::Reopen {
                path: format!("{}\\{}", f.device, f.path),
                error,
            };
            path::components(&f.path).map_err(reopen)?;
            let backend = &mut self.mounts[mount].backend;
            let is_directory = match backend.stat(&f.path) {
                Ok(info) => info.is_directory(),
                // A carried file is created again at apply.
                Err(Error::ObjectNameNotFound) if f.contents.is_some() => false,
                Err(error) => return Err(reopen(error)),
            };
            if f.contents.is_some() {
                if f.directory {
                    return Err(StateError::Invalid);
                }
                if backend.read_only() {
                    return Err(reopen(Error::MediaWriteProtected));
                }
            }
            if is_directory != f.directory {
                return Err(reopen(if f.directory {
                    Error::NotADirectory
                } else {
                    Error::FileIsADirectory
                }));
            }
            let enumeration = match &f.search {
                None => None,
                Some((pattern, next)) => {
                    let mut entries = backend.read_directory(&f.path).map_err(reopen)?;
                    entries.retain(|e| wildcard_matches(pattern, &e.name));
                    entries.sort_by_key(|e| e.name.to_ascii_lowercase());
                    Some(Enumeration {
                        entries,
                        next: *next as usize,
                        pattern: pattern.clone(),
                    })
                }
            };
            let disposition = CreateDisposition::try_from(f.disposition).map_err(reopen)?;
            reopened.push((
                f.id,
                OpenFile {
                    mount,
                    path: f.path.clone(),
                    key: f.path.to_ascii_lowercase(),
                    options: OpenOptions {
                        access: Access(f.access),
                        share: ShareAccess(f.share),
                        disposition,
                        directory: f.want_directory,
                        attributes: f.attributes,
                        delete_on_close: false,
                    },
                    position: f.position,
                    enumeration,
                    delete_requested: false,
                    directory: f.directory,
                    changed: f.changed,
                },
            ));
            if let Some(bytes) = &f.contents
                && !writes
                    .iter()
                    .any(|(m, p, _)| *m == mount && p.eq_ignore_ascii_case(&f.path))
            {
                writes.push((mount, f.path.clone(), bytes.clone()));
            }
        }
        let handles = state.handles.iter().copied().collect();
        let next_handle = state.next_handle;
        Ok(move |vfs: &mut Vfs| {
            let open: Vec<_> = vfs.handles.keys().copied().collect();
            for handle in open {
                let _ = vfs.close(Handle(handle));
            }
            for (mount, path, bytes) in writes {
                let m = &mut vfs.mounts[mount];
                if let Err(error) = write_whole(m.backend.as_mut(), &path, &bytes) {
                    eprintln!(
                        "vfs: snapshot load: {}\\{path} not written back: {error}",
                        m.device
                    );
                }
            }
            for (id, file) in reopened {
                if !file.directory {
                    vfs.mounts[file.mount].backend.opened(&file.path);
                }
                vfs.objects.insert(id, file);
            }
            vfs.handles = handles;
            vfs.next_handle = next_handle;
        })
    }
}

/// The first `len` bytes of the file at `path`.
fn read_whole(backend: &mut dyn Backend, path: &str, len: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; len as usize];
    let mut at = 0;
    while at < bytes.len() {
        match backend.read_at(path, at as u64, &mut bytes[at..])? {
            0 => return Err(Error::EndOfFile),
            n => at += n,
        }
    }
    Ok(bytes)
}

/// Make the file at `path` hold exactly `bytes`, creating it if it is gone.
/// A file that holds them already is left alone (a rewind that the game
/// did not write past costs a read). The bytes are not synced: as with
/// the guest's own writes, durability is the guest's flush.
fn write_whole(backend: &mut dyn Backend, path: &str, bytes: &[u8]) -> Result<(), Error> {
    match backend.stat(path) {
        Ok(info) if info.end_of_file == bytes.len() as u64 => {
            if read_whole(backend, path, info.end_of_file)? == bytes {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(Error::ObjectNameNotFound) => backend.create(path, false, 0)?,
        Err(error) => return Err(error),
    }
    backend.set_len(path, bytes.len() as u64)?;
    let mut at = 0;
    while at < bytes.len() {
        match backend.write_at(path, at as u64, &bytes[at..])? {
            0 => return Err(Error::DiskFull),
            n => at += n,
        }
    }
    Ok(())
}
