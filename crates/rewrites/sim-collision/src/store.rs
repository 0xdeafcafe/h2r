//! Publication protocol for content-addressed collision files. Mappers hold
//! an immutable inode even while another process repairs the cache entry.
use bytemuck::Pod;
use memmap2::{Mmap, MmapOptions};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    ops::{Deref, Range},
    path::Path,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

const MIN_BYTES: usize = 64 << 10;

/// A decoded array backed by a private allocation or a checked read-only file range.
#[derive(Debug)]
pub struct Column<T: Pod>(Backing<T>);

#[derive(Debug)]
enum Backing<T: Pod> {
    Owned(Vec<T>),
    Mapped(Arc<Mmap>, Range<usize>),
}

impl<T: Pod> From<Vec<T>> for Column<T> {
    fn from(value: Vec<T>) -> Self {
        Self(Backing::Owned(value))
    }
}

impl<T: Pod> FromIterator<T> for Column<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl<T: Pod> Deref for Column<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match &self.0 {
            Backing::Owned(values) => values,
            Backing::Mapped(file, span) => bytemuck::cast_slice(&file[span.clone()]),
        }
    }
}

impl<T: Pod> Column<T> {
    pub(crate) fn mapped(map: &Arc<Mmap>, span: Range<usize>) -> Option<Self> {
        bytemuck::try_cast_slice::<u8, T>(map.get(span.clone())?).ok()?;
        Some(Self(Backing::Mapped(map.clone(), span)))
    }

    /// Share a decoded column once its contents are known. The caller's
    /// watch receipts, pointers and transient data never enter the file.
    pub(crate) fn share(&mut self, directory: &Path) {
        if !matches!(self.0, Backing::Owned(_)) || std::mem::size_of_val(&**self) < MIN_BYTES {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(self);
        let key = Sha256::digest(bytes);
        let path = directory.join(format!("{key:x}.column"));
        let open = || {
            let file = File::open(&path).ok()?;
            if file.metadata().ok()?.len() != bytes.len() as u64 {
                return None;
            }
            // Every writer uses atomic_write; published inodes are immutable.
            let map = Arc::new(unsafe { MmapOptions::new().map(&file).ok()? });
            if &map[..] != bytes {
                return None;
            }
            Self::mapped(&map, 0..bytes.len())
        };
        let column = open().or_else(|| {
            let _ = atomic_write(&path, bytes);
            open()
        });
        if let Some(column) = column {
            *self = column;
        }
    }

    #[cfg(test)]
    pub(crate) fn is_mapped(&self) -> bool {
        matches!(self.0, Backing::Mapped(..))
    }

    pub(crate) fn capacity(&self) -> usize {
        match &self.0 {
            Backing::Owned(values) => values.capacity(),
            Backing::Mapped(_, range) => range.len() / size_of::<T>(),
        }
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(path.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // PID and sequence distinguish simultaneous publishers. Skip leftovers
    // from terminated processes that had the same PID.
    let (tmp, mut file) = loop {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}.{number}.tmp", std::process::id()));
        match options.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = file.write_all(bytes);
    drop(file);
    let result = result.and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
pub(crate) struct TestDir(std::path::PathBuf);

#[cfg(test)]
impl TestDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("h2-collision-store-{}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_column_publications_return_identical_read_only_rows() {
        let scratch = crate::store::TestDir::new();
        let dir = scratch.path();
        let rows: Vec<i16> = (0..=(i16::MAX as usize)).map(|i| -(i as i16)).collect();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let mut column: Column<_> = rows.clone().into();
                    column.share(dir);
                    assert!(column.is_mapped());
                    assert_eq!(&*column, rows);
                });
            }
        });
        assert_eq!(fs::read_dir(dir).unwrap().count(), 1);
        let path = fs::read_dir(dir).unwrap().next().unwrap().unwrap().path();
        let mut damaged = fs::read(&path).unwrap();
        damaged[7] ^= 1;
        atomic_write(&path, &damaged).unwrap();
        let mut recovered: Column<_> = rows.clone().into();
        recovered.share(dir);
        assert!(recovered.is_mapped());
        assert_eq!(&*recovered, rows);
    }
}
