//! Publish generated files without truncating an existing inode. Mutable
//! outputs can be hard-linked elsewhere, including into a repository.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A newly created sibling of the destination, reserved exclusively so an
/// old temporary (including a hard link) can never be truncated by a writer.
/// Drop removes unfinished output; publish replaces the destination name.
pub struct Temporary {
    path: PathBuf,
    file: File,
    published: bool,
}

impl Temporary {
    pub fn new(destination: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if destination.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output has no filename",
            ));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        loop {
            let number = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".h2-output-{}-{number}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file,
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// External tools must produce actual output before an object is pinned
    /// or published. A successful no-output driver action leaves the reserved
    /// empty file, which must not become a cache hit on the next build.
    pub fn require_nonempty(&self) -> io::Result<()> {
        if fs::metadata(&self.path)?.len() == 0 {
            return Err(io::Error::other(format!(
                "compiler produced no output in {}",
                self.path.display()
            )));
        }
        Ok(())
    }

    pub fn publish(mut self, destination: &Path) -> io::Result<()> {
        fs::rename(&self.path, destination)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Write through a fresh inode, then atomically replace the old directory
/// entry. Existing hard links retain their previous bytes and inode.
pub fn replace(destination: &Path, content: &[u8]) -> io::Result<()> {
    let mut temporary = Temporary::new(destination)?;
    temporary.file.write_all(content)?;
    temporary.publish(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn replacement_detaches_hard_links_and_failed_publication_cleans_up() {
        let root =
            std::env::temp_dir().join(format!("h2-output-publication-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let sentinel = root.join("sentinel");
        let output = root.join("output");
        fs::write(&sentinel, "original").unwrap();
        fs::hard_link(&sentinel, &output).unwrap();
        replace(&output, b"new output").unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"original");
        assert_eq!(fs::read(&output).unwrap(), b"new output");
        assert_ne!(
            fs::metadata(&sentinel).unwrap().ino(),
            fs::metadata(&output).unwrap().ino()
        );
        let temporary = Temporary::new(&output).unwrap();
        let reserved = temporary.path().to_path_buf();
        assert!(temporary.require_nonempty().is_err());
        let directory = root.join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(temporary.publish(&directory).is_err());
        assert!(!reserved.exists());
        assert!(directory.is_dir());
        assert_eq!(fs::read(&sentinel).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }
}
