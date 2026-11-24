//! Publication protocol for content-addressed collision files. Mappers hold
//! an immutable inode even while another process repairs the cache entry.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

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
