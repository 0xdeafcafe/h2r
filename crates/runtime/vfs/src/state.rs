//! Open file objects as plain data, for machine snapshots
//! (docs/runtime/snapshots.md, phase 3). A save records every open object
//! by device and path with its handle numbers, options, position and
//! search cursor; a load re-opens them on the booted VFS's mounts under the
//! same numbers. A file open for writing also carries its whole contents
//! (up to [`MAX_CARRIED_BYTES`]), and a load writes them back before it
//! re-opens the file: the title keeps `z:\preferences.dat` open for
//! writing all the time it runs. A larger one (a map cache slot) is
//! carried by reference and put back from the VFS's journal of the bytes
//! changes overwrote since (`journal.rs`). While a delete is pending, or
//! an object may delete, the host disk holds state the snapshot does not,
//! so the save is refused.
use crate::{
    Access, Backend, CreateDisposition, Enumeration, Error, FileReference, Handle, OpenFile,
    OpenOptions, ShareAccess, Vfs, journal::Plan, path, wildcard_matches,
};

/// The largest file a snapshot carries the bytes of, for an object open
/// for writing. The title's own (`preferences.dat`) is 0x1fc bytes; a map
/// cache slot (180 MB) is carried by reference ([`FileReference`]).
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
    /// The file's bytes when the object can write to it, or wrote to it,
    /// and they are at most [`MAX_CARRIED_BYTES`]: written back at load,
    /// before the object is re-opened.
    pub contents: Option<Vec<u8>>,
    /// Such a file when it is larger: put back at load from the journal
    /// of the capturing VFS, or checked by its fingerprint.
    pub reference: Option<FileReference>,
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
    /// Changed since the capture by more than the journal keeps
    /// ([`crate::MAX_JOURNAL_BYTES`]), or captured by another VFS, and its
    /// fingerprint differs.
    #[error(
        "{path} no longer holds the bytes the snapshot saw, and the VFS journal \
         no longer reaches back to them"
    )]
    Unjournaled { path: String },
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
    /// writing is read whole into its [`SavedFile::contents`], or, past
    /// [`MAX_CARRIED_BYTES`], recorded as a [`SavedFile::reference`]: its
    /// changes from here on are journaled.
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
            let (contents, reference) =
                if !f.directory && (f.options.access.0 & writes != 0 || f.changed) {
                    let read = |error| StateError::Read {
                        path: format!("{device}\\{}", f.path),
                        error,
                    };
                    let bytes = mount.backend.stat(&f.path).map_err(read)?.end_of_file;
                    if bytes > MAX_CARRIED_BYTES {
                        let reference = self
                            .journals
                            .capture(mount.backend.as_mut(), f.mount, &f.key, &f.path, bytes)
                            .map_err(read)?;
                        (None, Some(reference))
                    } else {
                        let contents = read_whole(mount.backend.as_mut(), &f.path, bytes);
                        (Some(contents.map_err(read)?), None)
                    }
                } else {
                    (None, None)
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
                reference,
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
    /// creating it if it is gone, and rewinds each referenced one
    /// ([`SavedFile::reference`]) through the journal, after the open
    /// objects close. A referenced file that can be neither rewound nor
    /// matched by its fingerprint refuses the load.
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
        // Referenced files, one per path: mount, key, path, what to do.
        let mut rewinds: Vec<(usize, String, String, Plan)> = Vec::new();
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
            let carried = f.contents.is_some() || f.reference.is_some();
            let (is_directory, length) = match backend.stat(&f.path) {
                Ok(info) => (info.is_directory(), Some(info.end_of_file)),
                // A carried file is created again at apply; a referenced
                // one, if the journal saw it removed.
                Err(Error::ObjectNameNotFound) if carried => (false, None),
                Err(error) => return Err(reopen(error)),
            };
            if carried {
                if f.directory || (f.contents.is_some() && f.reference.is_some()) {
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
            let key = f.path.to_ascii_lowercase();
            if let Some(reference) = &f.reference
                && !rewinds.iter().any(|(m, k, _, _)| *m == mount && *k == key)
            {
                let plan = self
                    .journals
                    .plan(backend.as_mut(), mount, &key, &f.path, reference, length)
                    .ok_or_else(|| StateError::Unjournaled {
                        path: format!("{}\\{}", f.device, f.path),
                    })?;
                rewinds.push((mount, key.clone(), f.path.clone(), plan));
            }
            let disposition = CreateDisposition::try_from(f.disposition).map_err(reopen)?;
            reopened.push((
                f.id,
                OpenFile {
                    mount,
                    path: f.path.clone(),
                    key,
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
                match write_whole(m.backend.as_mut(), &path, &bytes) {
                    Ok(false) => {}
                    // Small again: a journal of it no longer applies.
                    Ok(true) => vfs.journals.forget(mount, &path.to_ascii_lowercase()),
                    Err(error) => eprintln!(
                        "vfs: snapshot load: {}\\{path} not written back: {error}",
                        m.device
                    ),
                }
            }
            for (mount, key, path, plan) in rewinds {
                let backend = vfs.mounts[mount].backend.as_mut();
                vfs.journals.apply(backend, mount, &key, &path, plan);
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
/// the guest's own writes, durability is the guest's flush. True if it
/// wrote.
fn write_whole(backend: &mut dyn Backend, path: &str, bytes: &[u8]) -> Result<bool, Error> {
    match backend.stat(path) {
        Ok(info) if info.end_of_file == bytes.len() as u64 => {
            if read_whole(backend, path, info.end_of_file)? == bytes {
                return Ok(false);
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
    Ok(true)
}
