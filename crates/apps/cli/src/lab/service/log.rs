//! Bound both sides of the LaunchAgent's log rotation. The running job and its
//! children write to pipes, so the file descriptors survive a rotation.

use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread::{Builder, JoinHandle};

const LIMIT: u64 = 8 << 20;

pub(super) struct Log {
    path: PathBuf,
    archive: PathBuf,
    file: File,
    len: u64,
    limit: u64,
}

impl Log {
    pub(super) fn open(path: PathBuf) -> io::Result<Self> {
        Self::with_limit(path, LIMIT)
    }

    fn with_limit(path: PathBuf, limit: u64) -> io::Result<Self> {
        let archive = archive_path(&path);
        match open(&archive, false) {
            Ok(file) => {
                keep_tail(&file, limit)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let mut file = open(&path, true)?;
        let len = keep_tail(&file, limit)?;
        file.seek(SeekFrom::Start(len))?;
        Ok(Self {
            path,
            archive,
            file,
            len,
            limit,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        fs::rename(&self.path, &self.archive)?;
        self.file = open(&self.path, true)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.len == self.limit {
            self.rotate()?;
        }
        let count = bytes.len().min((self.limit - self.len) as usize);
        let written = self.file.write(&bytes[..count])?;
        self.len += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open(path: &Path, create: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn archive_path(path: &Path) -> PathBuf {
    let mut filename = OsString::from(path.file_name().unwrap());
    filename.push(".1");
    path.with_file_name(filename)
}

/// An earlier version may have left oversized files. Save their newest bytes
/// without an allocation proportional to the original file.
fn keep_tail(file: &File, limit: u64) -> io::Result<u64> {
    let len = file.metadata()?.len();
    if len <= limit {
        return Ok(len);
    }
    let tail = len - limit;
    let mut buf = [0; 64 << 10];
    let mut copied = 0;
    while copied < limit {
        let count = buf.len().min((limit - copied) as usize);
        let block = &mut buf[..count];
        file.read_exact_at(block, tail + copied)?;
        file.write_all_at(block, copied)?;
        copied += count as u64;
    }
    file.set_len(limit)?;
    Ok(limit)
}

pub(super) struct Drain {
    fd: RawFd,
    thread: JoinHandle<()>,
}

impl Drain {
    pub(super) fn finish(self) -> io::Result<()> {
        let sink = OpenOptions::new().write(true).open("/dev/null")?;
        replace_fd(sink.as_raw_fd(), self.fd)?;
        self.thread
            .join()
            .map_err(|_| io::Error::other("lab log drainer panicked"))
    }
}

pub(super) fn redirect(fd: RawFd, path: PathBuf) -> io::Result<Drain> {
    let mut log = Log::open(path)?;
    let (mut reader, writer) = io::pipe()?;
    let thread = Builder::new().name("lab host log".into()).spawn(move || {
        if let Err(e) = io::copy(&mut reader, &mut log) {
            let message = format!("h2 lab: writing {}: {e}", log.path.display());
            if let Ok(message) = CString::new(message) {
                // SAFETY: both strings are NUL-terminated. The single
                // argument corresponds to the %s in the static format.
                unsafe { libc::syslog(libc::LOG_ERR, c"%s".as_ptr(), message.as_ptr()) };
            }
            // A storage failure should leave the host running.
            let _ = io::copy(&mut reader, &mut io::sink());
        }
    })?;
    replace_fd(writer.as_raw_fd(), fd)?;
    Ok(Drain { fd, thread })
}

fn replace_fd(from: RawFd, to: RawFd) -> io::Result<()> {
    // SAFETY: the callers own a valid source descriptor and specify either
    // stdout or stderr as the destination in this managed process.
    if unsafe { libc::dup2(from, to) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::service::tests::Scratch;

    #[test]
    fn rolling_log_caps_both_files_during_large_writes() {
        let scratch = Scratch::new();
        let path = scratch.0.join("host.log");
        let archive = archive_path(&path);
        let mut log = Log::with_limit(path.clone(), 5).unwrap();

        log.write_all(b"abcdefghijkl").unwrap();
        assert_eq!(fs::read(&archive).unwrap(), b"fghij");
        assert_eq!(fs::read(&path).unwrap(), b"kl");
        log.write_all(b"mnop").unwrap();
        assert_eq!(fs::read(&archive).unwrap(), b"klmno");
        assert_eq!(fs::read(&path).unwrap(), b"p");
    }

    #[test]
    fn rolling_log_bounds_existing_files_and_keeps_their_tails() {
        let scratch = Scratch::new();
        let path = scratch.0.join("host.log");
        let archive = archive_path(&path);
        fs::write(&path, b"first-last").unwrap();
        fs::write(&archive, b"before-archive").unwrap();

        let mut log = Log::with_limit(path.clone(), 4).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"last");
        assert_eq!(fs::read(&archive).unwrap(), b"hive");
        log.write_all(b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read(&archive).unwrap(), b"last");
    }
}
