//! Open file objects as plain data, for machine snapshots
//! (docs/runtime/snapshots.md, phase 3). A save records every open object
//! by device and path with its handle numbers, options, position and
//! search cursor; a load re-opens them on the booted VFS's mounts under the
//! same numbers. Only read-only objects are covered: while a file is open
//! for writing, or a delete is pending, the host disk holds state the
//! snapshot does not, so the save is refused.
use crate::{
    Access, CreateDisposition, Enumeration, Error, Handle, OpenFile, OpenOptions, ShareAccess, Vfs,
    path, wildcard_matches,
};

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
}

/// Why the VFS cannot be saved or loaded.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("{0} is open for writing or deletion")]
    Writable(String),
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
    /// The open objects, or why they cannot be saved.
    pub fn state(&self) -> Result<VfsState, StateError> {
        let pending = self.pending.len() + self.cleanup_pending.len();
        if pending != 0 {
            return Err(StateError::DeletePending(pending));
        }
        let mut objects = Vec::with_capacity(self.objects.len());
        for (&id, f) in &self.objects {
            let device = &self.mounts[f.mount].device;
            let writes = Access::WRITE.0 | Access::APPEND.0 | Access::DELETE.0;
            if f.options.access.0 & writes != 0
                || f.options.delete_on_close
                || f.delete_requested
                || f.changed
            {
                return Err(StateError::Writable(format!("{device}\\{}", f.path)));
            }
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
            });
        }
        Ok(VfsState {
            next_handle: self.next_handle,
            handles: self.handles.iter().map(|(&h, &o)| (h, o)).collect(),
            objects,
        })
    }

    /// Close every open object and re-open the saved ones under their own
    /// handle numbers. Every saved file is looked up first: on an error
    /// nothing has changed.
    pub fn restore_state(&mut self, state: &VfsState) -> Result<(), StateError> {
        if state
            .handles
            .iter()
            .any(|&(h, o)| h > state.next_handle || !state.objects.iter().any(|f| f.id == o))
        {
            return Err(StateError::Invalid);
        }
        let mut reopened = Vec::with_capacity(state.objects.len());
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
            let info = backend.stat(&f.path).map_err(reopen)?;
            if info.is_directory() != f.directory {
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
                    changed: false,
                },
            ));
        }
        let open: Vec<_> = self.handles.keys().copied().collect();
        for handle in open {
            let _ = self.close(Handle(handle));
        }
        for (id, file) in reopened {
            if !file.directory {
                self.mounts[file.mount].backend.opened(&file.path);
            }
            self.objects.insert(id, file);
        }
        self.handles = state.handles.iter().copied().collect();
        self.next_handle = state.next_handle;
        Ok(())
    }
}
