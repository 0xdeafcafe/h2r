//! The staged rename transaction used by `HostBackend::format`.

use crate::{Error, Result};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use rustix::{
    fd::OwnedFd as Directory,
    fs::{AtFlags, Dir, FileType, Mode, OFlags, Stat, fstat, openat, statat, unlinkat},
};
#[cfg(not(unix))]
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(unix)]
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

// Xbox app-container directories use the path-based Windows implementation.
#[cfg(not(unix))]
struct Directory;

pub(super) struct FormatRoot {
    parent_path: PathBuf,
    name: OsString,
    #[cfg(unix)]
    parent: Directory,
    current: Directory,
}

pub(super) struct Swap {
    name: OsString,
    staging: OsString,
    aside: OsString,
    replacement: Directory,
}

pub(super) fn swap_roots(
    [data, metadata]: &[FormatRoot; 2],
    mut before_rename: impl FnMut() -> Result<()>,
) -> Result<[Swap; 2]> {
    let data_swap = data.swap(&mut before_rename)?;
    match metadata.swap(&mut before_rename) {
        Ok(metadata_swap) => Ok([data_swap, metadata_swap]),
        Err(error) => {
            data.rollback(&data_swap, true);
            Err(error)
        }
    }
}

pub(super) fn finish(roots: &mut [FormatRoot; 2], swaps: [Swap; 2]) {
    let mut synced = true;
    for root in &*roots {
        if let Err(error) = root.sync_parent() {
            eprintln!("vfs: formatted volume, but its directory sync failed: {error}");
            synced = false;
        }
    }
    for (root, swap) in roots.iter_mut().zip(swaps) {
        let old = std::mem::replace(&mut root.current, swap.replacement);
        if synced {
            root.cleanup(&swap.aside, &old);
        }
    }
}

impl FormatRoot {
    pub(super) fn new(path: &Path) -> Result<Self> {
        let parent_path = path.parent().ok_or(Error::InvalidParameter)?.to_owned();
        let name = path.file_name().ok_or(Error::InvalidParameter)?.to_owned();
        #[cfg(unix)]
        let (parent, current) = open_root(&parent_path, &name)?;
        #[cfg(not(unix))]
        let current = Directory;
        let root = Self {
            parent_path,
            name,
            #[cfg(unix)]
            parent,
            current,
        };
        root.sweep_leftovers();
        Ok(root)
    }

    fn companion_prefix(&self) -> OsString {
        let mut prefix = OsString::from(".");
        prefix.push(&self.name);
        prefix.push(".h2format-");
        prefix
    }

    fn companion(&self, kind: &str) -> OsString {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut name = self.companion_prefix();
        name.push(format!(
            "{kind}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        name
    }

    fn sweep_leftovers(&self) {
        let prefix = self.companion_prefix();
        let Ok(names) = self.names() else { return };
        for name in names {
            if name
                .as_encoded_bytes()
                .starts_with(prefix.as_encoded_bytes())
            {
                match self.open_directory(&name) {
                    Ok(tree) => self.cleanup(&name, &tree),
                    Err(error) => self.log_leftover(&name, &error),
                }
            }
        }
    }

    fn swap(&self, before_rename: &mut impl FnMut() -> Result<()>) -> Result<Swap> {
        let name = self.locate()?;
        let staging = self.companion("new");
        let aside = self.companion("old");
        let replacement = self.create_directory(&staging)?;
        let swap = Swap {
            name,
            staging,
            aside,
            replacement,
        };
        if let Err(error) = self.install(&swap, before_rename) {
            self.discard_empty(&swap.staging, &swap.replacement);
            return Err(error);
        }
        Ok(swap)
    }

    fn install(&self, swap: &Swap, before_rename: &mut impl FnMut() -> Result<()>) -> Result<()> {
        before_rename()?;
        self.verify_entry(&swap.name, &self.current)?;
        self.move_aside(&swap.name, &swap.aside)?;

        let install_replacement = (|| -> Result<()> {
            self.verify_entry(&swap.aside, &self.current)?;
            before_rename()?;
            self.verify_entry(&swap.staging, &swap.replacement)?;
            self.require_vacant(&swap.name)?;
            self.rename(&swap.staging, &swap.name)?;
            Ok(())
        })();
        if let Err(error) = install_replacement {
            self.rollback(swap, false);
            return Err(error);
        }
        if let Err(error) = self.verify_entry(&swap.name, &swap.replacement) {
            self.rollback(swap, true);
            return Err(error.into());
        }
        Ok(())
    }

    fn rollback(&self, swap: &Swap, installed: bool) {
        let restore = || -> io::Result<()> {
            self.verify_entry(&swap.aside, &self.current)?;
            if installed {
                self.verify_entry(&swap.name, &swap.replacement)?;
                // Unix rename replaces the verified empty directory in one step.
                #[cfg(not(unix))]
                self.remove_empty(&swap.name)?;
            } else {
                self.require_vacant(&swap.name)?;
            }
            self.rename(&swap.aside, &swap.name)
        };
        if let Err(error) = restore() {
            eprintln!(
                "vfs: format rollback of {} failed, its old tree may remain at {}: {error}",
                self.parent_path.join(&swap.name).display(),
                self.parent_path.join(&swap.aside).display()
            );
        }
    }

    fn discard_empty(&self, name: &OsStr, expected: &Directory) {
        let _ = self
            .verify_entry(name, expected)
            .and_then(|()| self.remove_empty(name));
    }

    fn cleanup(&self, name: &OsStr, tree: &Directory) {
        if let Err(error) = self.remove_tree(name, tree) {
            self.log_leftover(name, &error);
        }
    }

    fn log_leftover(&self, name: &OsStr, error: &io::Error) {
        eprintln!(
            "vfs: cannot remove format leftover {}: {error}",
            self.parent_path.join(name).display()
        );
    }

    fn create_directory(&self, name: &OsStr) -> io::Result<Directory> {
        #[cfg(unix)]
        rustix::fs::mkdirat(&self.parent, name, Mode::from_raw_mode(0o777))?;
        #[cfg(not(unix))]
        fs::create_dir(self.parent_path.join(name))?;
        self.open_directory(name).inspect_err(|_| {
            let _ = self.remove_empty(name);
        })
    }

    fn move_aside(&self, name: &OsStr, aside: &OsStr) -> io::Result<()> {
        // On Unix an empty directory at the destination also makes rename
        // reject a symlink substituted for the source after verification.
        #[cfg(unix)]
        let slot = self.create_directory(aside)?;
        #[cfg(not(unix))]
        self.require_vacant(aside)?;
        let moved = self.rename(name, aside);
        #[cfg(unix)]
        if moved.is_err() {
            self.discard_empty(aside, &slot);
        }
        moved
    }

    fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        return Ok(rustix::fs::renameat(&self.parent, from, &self.parent, to)?);
        #[cfg(not(unix))]
        fs::rename(self.parent_path.join(from), self.parent_path.join(to))
    }

    fn remove_empty(&self, name: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        return Ok(unlinkat(&self.parent, name, AtFlags::REMOVEDIR)?);
        #[cfg(not(unix))]
        fs::remove_dir(self.parent_path.join(name))
    }

    fn sync_parent(&self) -> Result<()> {
        #[cfg(unix)]
        return Ok(rustix::fs::fsync(&self.parent).map_err(io::Error::from)?);
        #[cfg(not(unix))]
        super::sync_directory(&self.parent_path)
    }

    fn require_vacant(&self, name: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        let entry = statat(&self.parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from);
        #[cfg(not(unix))]
        let entry = fs::symlink_metadata(self.parent_path.join(name));
        match entry {
            Ok(_) => Err(io::ErrorKind::AlreadyExists.into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(unix)]
impl FormatRoot {
    fn locate(&self) -> io::Result<OsString> {
        // Both identities come from the mount's descriptors. The original
        // path might resolve to a different tree after an ancestor rename.
        let parent = fstat(&self.parent)?;
        let actual_parent = statat(&self.current, "..", AtFlags::SYMLINK_NOFOLLOW)?;
        if !same_inode(&parent, &actual_parent) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mounted = fstat(&self.current)?;
        if entry_matches(&self.parent, &self.name, &mounted)? {
            return Ok(self.name.clone());
        }
        for name in self.names()? {
            if entry_matches(&self.parent, &name, &mounted)? {
                return Ok(name);
            }
        }
        Err(io::ErrorKind::NotFound.into())
    }

    fn verify_entry(&self, name: &OsStr, directory: &Directory) -> io::Result<()> {
        verify_entry(&self.parent, name, directory)
    }

    fn names(&self) -> io::Result<Vec<OsString>> {
        directory_names(&self.parent)
    }

    fn open_directory(&self, name: &OsStr) -> io::Result<Directory> {
        Ok(openat(&self.parent, name, DIRECTORY, Mode::empty())?)
    }

    fn remove_tree(&self, name: &OsStr, tree: &Directory) -> io::Result<()> {
        remove_tree(&self.parent, name, tree)
    }
}

#[cfg(unix)]
fn open_root(parent: &Path, name: &OsStr) -> io::Result<(Directory, Directory)> {
    // Resolve the canonical mount path once. Format uses the held parent
    // and opens its children with single-component, no-follow lookups.
    let dir = rustix::fs::open(parent, DIRECTORY, Mode::empty())?;
    let root = openat(&dir, name, DIRECTORY, Mode::empty())?;
    Ok((dir, root))
}

#[cfg(unix)]
fn directory_names(dir: &Directory) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in Dir::read_from(dir)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsStr::from_bytes(bytes).to_owned());
        }
    }
    Ok(names)
}

#[cfg(unix)]
fn same_inode(a: &Stat, b: &Stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}

#[cfg(unix)]
fn entry_matches(dir: &Directory, name: &OsStr, expected: &Stat) -> io::Result<bool> {
    match statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(entry) => Ok(same_inode(&entry, expected)),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn verify_entry(parent: &Directory, name: &OsStr, directory: &Directory) -> io::Result<()> {
    if entry_matches(parent, name, &fstat(directory)?)? {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

#[cfg(unix)]
fn remove_tree(parent: &Directory, name: &OsStr, tree: &Directory) -> io::Result<()> {
    verify_entry(parent, name, tree)?;
    for child_name in directory_names(tree)? {
        let stat = statat(tree, &child_name, AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let child = openat(tree, &child_name, DIRECTORY, Mode::empty())?;
            if !same_inode(&stat, &fstat(&child)?) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            remove_tree(tree, &child_name, &child)?;
        } else {
            unlinkat(tree, &child_name, AtFlags::empty())?;
        }
    }
    verify_entry(parent, name, tree)?;
    Ok(unlinkat(parent, name, AtFlags::REMOVEDIR)?)
}

#[cfg(not(unix))]
impl FormatRoot {
    fn locate(&self) -> io::Result<OsString> {
        Ok(self.name.clone())
    }

    fn verify_entry(&self, name: &OsStr, _: &Directory) -> io::Result<()> {
        if fs::symlink_metadata(self.parent_path.join(name))?.is_dir() {
            Ok(())
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    fn names(&self) -> io::Result<Vec<OsString>> {
        Ok(fs::read_dir(&self.parent_path)?
            .flatten()
            .map(|entry| entry.file_name())
            .collect())
    }

    fn open_directory(&self, _: &OsStr) -> io::Result<Directory> {
        Ok(Directory)
    }

    fn remove_tree(&self, name: &OsStr, _: &Directory) -> io::Result<()> {
        fs::remove_dir_all(self.parent_path.join(name))
    }
}
