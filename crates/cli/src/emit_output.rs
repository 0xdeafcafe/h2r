//! Output locations and the lock covering an entire CLI emission transaction.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// Resolve existing symlinks before interpreting `..`, including when the
/// final directories do not exist yet. Callers write through this result.
pub fn location(path: &Path, allow_repo_output: bool) -> io::Result<PathBuf> {
    let absolute = std::env::current_dir()?.join(path);
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            _ => {
                resolved.push(component.as_os_str());
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = fs::canonicalize(&resolved)?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    if !allow_repo_output {
        for ancestor in resolved.ancestors() {
            match fs::symlink_metadata(ancestor.join(".git")) {
                Ok(_) => {
                    return Err(io::Error::other(format!(
                        "refusing {}: inside a git work tree; use --allow-repo-output to explicitly permit derived game output there",
                        resolved.display()
                    )));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(resolved)
}

/// Managed output/cache trees may contain hard links, but never symlinks.
/// Check directories and leaves, including dangling aliases, before writers
/// can follow an existing src/header/ABI/pin/receipt/library destination.
pub fn reject_symlinks(root: &Path) -> io::Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // A shared cache pruner can remove entries during inspection.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "refusing symlinked output entry {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}

/// All writers sharing an output (including aliases) open the same lock
/// inode. Keep it from before receipt validation until the final link/use.
/// Closing this file releases flock, including on an error or process death.
pub struct OutputLock {
    _file: File,
}

impl OutputLock {
    pub fn acquire(out: &Path) -> io::Result<Self> {
        Self::acquire_with(out, libc::LOCK_EX)
    }

    fn acquire_with(out: &Path, operation: libc::c_int) -> io::Result<Self> {
        fs::create_dir_all(out)?;
        let out = fs::canonicalize(out)?;
        // Never unlink this file: replacing the inode would split the lock.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(out.join(".h2-emit.lock"))?;
        loop {
            // SAFETY: file owns an open fd, and operation is a flock flag.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                reject_symlinks(&out)?;
                return Ok(Self { _file: file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit_cache;
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("h2-output-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reject_worktrees_and_symlink_aliases_before_creating_directories() {
        let scratch = Scratch::new("locations");
        let repo = scratch.0.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let output = repo.join("new/deep/output");
        assert!(location(&output, false).is_err());
        assert!(!repo.join("new").exists());
        assert_eq!(location(&output, true).unwrap(), output);
        symlink(&repo, scratch.0.join("alias")).unwrap();
        assert!(location(&scratch.0.join("alias/generated"), false).is_err());
        fs::create_dir(repo.join("nested")).unwrap();
        symlink(repo.join("nested"), scratch.0.join("nested-alias")).unwrap();
        assert!(location(&scratch.0.join("nested-alias/../generated"), false).is_err());
        symlink(&scratch.0, repo.join("external")).unwrap();
        assert_eq!(
            location(&repo.join("external/output"), false).unwrap(),
            scratch.0.join("output")
        );
        let linked = scratch.0.join("linked");
        fs::create_dir(&linked).unwrap();
        fs::write(linked.join(".git"), "gitdir: ../metadata\n").unwrap();
        assert!(location(&linked.join("output"), false).is_err());
    }

    /// Astra's schedule: A is paused after writing its sources but before
    /// recording them. B must not replace those sources under A's key.
    #[test]
    fn concurrent_builds_cannot_record_another_images_sources() {
        let scratch = Scratch::new("receipt-race");
        let out = scratch.0.join("out");
        let alias = scratch.0.join("alias");
        let (attempted, received) = mpsc::channel();
        std::thread::scope(|scope| {
            let transaction_a = OutputLock::acquire(&out).unwrap();
            fs::create_dir(out.join("src")).unwrap();
            symlink(&out, &alias).unwrap();
            emit_cache::invalidate(&out).unwrap();
            fs::write(
                out.join("src/shard_00001.c"),
                "int value(void) { return 1; }\n",
            )
            .unwrap();
            scope.spawn(|| {
                let blocked = OutputLock::acquire_with(&alias, libc::LOCK_EX | libc::LOCK_NB)
                    .err()
                    .is_some_and(|e| e.kind() == io::ErrorKind::WouldBlock);
                attempted.send(blocked).unwrap();
                let _transaction_b = OutputLock::acquire(&alias).unwrap();
                emit_cache::invalidate(&alias).unwrap();
                fs::write(
                    alias.join("src/shard_00001.c"),
                    "int value(void) { return 2; }\n",
                )
                .unwrap();
                emit_cache::record(&alias, "image B\n").unwrap();
                assert!(emit_cache::valid(&alias, "image B\n"));
                assert!(!emit_cache::valid(&alias, "image A\n"));
            });
            assert!(received.recv_timeout(Duration::from_secs(10)).unwrap());
            emit_cache::record(&out, "image A\n").unwrap();
            assert!(emit_cache::valid(&out, "image A\n"));
            assert!(
                fs::read_to_string(out.join("src/shard_00001.c"))
                    .unwrap()
                    .contains("return 1")
            );
            drop(transaction_a);
        });
        assert!(!emit_cache::valid(&out, "image A\n"));
        assert!(emit_cache::valid(&out, "image B\n"));
        assert!(
            fs::read_to_string(out.join("src/shard_00001.c"))
                .unwrap()
                .contains("return 2")
        );
        // A killed/failed writer cannot leave a lock held after its fd closes.
        // Wait for the lock instead of trying it once: a child that another
        // test in this process is spawning holds a copy of every open
        // descriptor (and so of the flock) until it execs, which can take a
        // while under load. A lock the writers really leaked is never freed,
        // and the bounded wait fails.
        let (acquired, released) = mpsc::channel();
        let path = out.clone();
        std::thread::spawn(move || {
            let _ = acquired.send(OutputLock::acquire(&path).map(drop));
        });
        released
            .recv_timeout(Duration::from_secs(60))
            .expect("the lock was never released")
            .unwrap();
    }
}
