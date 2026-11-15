//! [`SaveStore`] over the shared VFS: guest paths such as `U:\ABC\SaveMeta.xbx`
//! become `\??\U:\ABC\SaveMeta.xbx`, so saves live under the host directory the
//! VFS maps U: to (`<data root>/disk/Partition1/UDATA/<title id>`) and carry
//! its metadata like any file the game creates itself.
use crate::{
    Error, Result, SaveDirectory, SaveStore, VfsStorage, saves::ERROR_PATH_NOT_FOUND,
    vfs_storage::win32,
};
use vfs::{Access, CreateDisposition, Handle, Offset, OpenOptions, ShareAccess, Timestamp, Vfs};

/// `\??\X:\rest` for a drive-letter path; anything else is not a save path.
fn nt_name(path: &str) -> Result<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return Err(Error::Win32(ERROR_PATH_NOT_FOUND));
    }
    Ok(format!("\\??\\{path}"))
}

fn open(vfs: &mut Vfs, path: &str, options: OpenOptions) -> Result<Handle> {
    vfs.open(&nt_name(path)?, options)
        .map(|opened| opened.handle)
        .map_err(win32)
}

/// Close `handle` and report the first error of `result` or the close.
fn finish<T>(vfs: &mut Vfs, handle: Handle, result: Result<T>) -> Result<T> {
    let closed = vfs.close(handle).map_err(win32);
    let value = result?;
    closed?;
    Ok(value)
}

fn directory_options(access: Access, disposition: CreateDisposition) -> OpenOptions {
    OpenOptions {
        access,
        share: ShareAccess::ALL,
        disposition,
        directory: Some(true),
        ..OpenOptions::default()
    }
}

fn file_options(access: Access, disposition: CreateDisposition) -> OpenOptions {
    OpenOptions {
        directory: Some(false),
        ..directory_options(access, disposition)
    }
}

fn delete_tree(vfs: &mut Vfs, path: &str) -> Result<()> {
    let is_directory = {
        let handle = open(
            vfs,
            path,
            OpenOptions {
                directory: None,
                ..OpenOptions::default()
            },
        )?;
        let info = vfs.query_information(handle).map_err(win32);
        finish(vfs, handle, info)?.metadata.is_directory()
    };
    if is_directory {
        let children = list(vfs, path, false)?;
        for child in children {
            delete_tree(
                vfs,
                &format!("{}\\{}", path.trim_end_matches('\\'), child.0),
            )?;
        }
    }
    let handle = open(
        vfs,
        path,
        OpenOptions {
            access: Access::DELETE | Access::READ_ATTRIBUTES,
            directory: None,
            ..OpenOptions::default()
        },
    )?;
    let pending = vfs.set_delete_pending(handle, true).map_err(win32);
    finish(vfs, handle, pending)
}

/// Every entry of directory `path`: name, directory flag and times.
fn list(vfs: &mut Vfs, path: &str, directories_only: bool) -> Result<Vec<(String, SaveDirectory)>> {
    let handle = open(
        vfs,
        path,
        directory_options(
            Access::READ | Access::READ_ATTRIBUTES,
            CreateDisposition::Open,
        ),
    )?;
    let mut entries = Vec::new();
    let result = loop {
        match vfs.query_directory(handle, Some("*"), false) {
            Ok(entry) => {
                if entry.metadata.is_directory() || !directories_only {
                    let basic = entry.metadata.basic;
                    entries.push((
                        entry.name.clone(),
                        SaveDirectory {
                            name: entry.name,
                            creation_time: basic.creation_time.0,
                            access_time: basic.last_access_time.0,
                            write_time: basic.last_write_time.0,
                        },
                    ));
                }
            }
            Err(vfs::Error::NoSuchFile | vfs::Error::NoMoreFiles) => break Ok(()),
            Err(error) => break Err(win32(error)),
        }
    };
    finish(vfs, handle, result)?;
    Ok(entries)
}

impl SaveStore for VfsStorage {
    fn create_directory(&mut self, path: &str) -> Result<()> {
        let mut vfs = self.lock();
        let handle = open(
            &mut vfs,
            path,
            directory_options(Access::READ_ATTRIBUTES, CreateDisposition::Create),
        )?;
        finish(&mut vfs, handle, Ok(()))
    }

    fn read_file(&mut self, path: &str, limit: usize) -> Result<Vec<u8>> {
        let mut vfs = self.lock();
        let handle = open(
            &mut vfs,
            path,
            file_options(
                Access::READ | Access::READ_ATTRIBUTES,
                CreateDisposition::Open,
            ),
        )?;
        let result = (|| {
            let size = vfs
                .query_information(handle)
                .map_err(win32)?
                .metadata
                .end_of_file;
            let mut bytes = vec![0; (size.min(limit as u64)) as usize];
            let mut filled = 0;
            while filled < bytes.len() {
                let n = vfs
                    .read(
                        handle,
                        Offset::Absolute(filled as u64),
                        &mut bytes[filled..],
                    )
                    .map_err(win32)?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            bytes.truncate(filled);
            Ok(bytes)
        })();
        finish(&mut vfs, handle, result)
    }

    fn create_file(&mut self, path: &str, contents: &[u8]) -> Result<()> {
        let mut vfs = self.lock();
        let handle = open(
            &mut vfs,
            path,
            file_options(
                Access::WRITE | Access::READ_ATTRIBUTES,
                CreateDisposition::Create,
            ),
        )?;
        let result = (|| {
            let mut written = 0;
            while written < contents.len() {
                let n = vfs
                    .write(
                        handle,
                        Offset::Absolute(written as u64),
                        &contents[written..],
                    )
                    .map_err(win32)?;
                if n == 0 {
                    return Err(Error::Win32(112));
                }
                written += n;
            }
            Ok(())
        })();
        finish(&mut vfs, handle, result)
    }

    fn touch_directory(&mut self, path: &str) -> Result<()> {
        let mut vfs = self.lock();
        let handle = open(
            &mut vfs,
            path,
            directory_options(
                Access::READ_ATTRIBUTES | Access::WRITE_ATTRIBUTES,
                CreateDisposition::Open,
            ),
        )?;
        let result = (|| {
            let mut basic = vfs.query_information(handle).map_err(win32)?.metadata.basic;
            basic.last_write_time = Timestamp::now();
            vfs.set_basic_information(handle, basic).map_err(win32)
        })();
        finish(&mut vfs, handle, result)
    }

    fn remove_tree(&mut self, path: &str) -> Result<()> {
        delete_tree(&mut self.lock(), path.trim_end_matches('\\'))
    }

    fn list_directories(&mut self, path: &str) -> Result<Vec<SaveDirectory>> {
        let mut entries = list(&mut self.lock(), path, true)?;
        entries.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        Ok(entries.into_iter().map(|(_, entry)| entry).collect())
    }
}
