//! Private storage for the editor's map. A reader's lease pins its pathname.
use std::{
    ffi::OsStr,
    fs::{self, File},
    hash::{BuildHasher, RandomState},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Once,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use content::map::checksum::XorChecksum;

/// Pins the private pathname until its last VFS or native reader releases it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MapGeneration(Arc<PrivateFile>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct PrivateFile {
    path: PathBuf,
}

impl MapGeneration {
    pub(super) fn path(&self) -> &Path {
        &self.0.path
    }

    pub(super) fn copy(
        source: &mut File,
        len: u64,
        name: &OsStr,
        cancelled: &AtomicBool,
    ) -> io::Result<(File, Self, content::Sha256, u32)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        check_cancelled(cancelled)?;
        reap_abandoned();
        check_cancelled(cancelled)?;
        let root = std::env::temp_dir();
        let names = RandomState::new();
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        let dir = loop {
            let seq = NEXT.fetch_add(1, Ordering::Relaxed);
            let nonce = names.hash_one(seq);
            let dir = root.join(format!("h2-map-{}-{seq}-{nonce:016x}", std::process::id()));
            match builder.create(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let generation = Self(Arc::new(PrivateFile { path: dir.join(name) }));
        let before = source.metadata()?;
        let (mut out, cloned) = destination(source, generation.path(), len)?;
        let (hash, checksum) = if cloned {
            if out.metadata()?.len() != len {
                return Err(io::Error::other("the map changed size while being cloned"));
            }
            out.rewind()?;
            digest(&mut out, len, io::sink(), cancelled)?
        } else {
            // A failed clone may have extended the destination on some filesystems.
            out.set_len(0)?;
            out.rewind()?;
            source.rewind()?;
            digest(source, len, &mut out, cancelled)?
        };
        let after = source.metadata()?;
        if after.len() != len || before.modified()? != after.modified()? {
            return Err(io::Error::other("the map changed while being copied"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            out.set_permissions(fs::Permissions::from_mode(0o400))?;
        }
        #[cfg(not(unix))]
        {
            let mut permissions = out.metadata()?.permissions();
            permissions.set_readonly(true);
            out.set_permissions(permissions)?;
        }
        drop(out);
        Ok((File::open(generation.path())?, generation, hash, checksum))
    }
}

/// One pass over the pinned bytes. On a streaming copy the bytes written
/// feed both checksums; on a filesystem clone the copy itself is the input.
fn digest(
    input: &mut impl Read,
    len: u64,
    mut output: impl Write,
    cancelled: &AtomicBool,
) -> io::Result<(content::Sha256, u32)> {
    let mut hash = content::Hasher::default();
    let mut checksum = XorChecksum::default();
    let mut buf = vec![0; 1 << 20];
    let mut offset = 0;
    while offset < len {
        check_cancelled(cancelled)?;
        let n = (len - offset).min(buf.len() as u64) as usize;
        let bytes = &mut buf[..n];
        input.read_exact(bytes)?;
        output.write_all(bytes)?;
        hash.update(bytes);
        // The retail copier excludes the 0x800-byte header and includes
        // the complete payload through physical EOF (map::checksum).
        let header = 0x800u64.saturating_sub(offset).min(n as u64) as usize;
        checksum.update(&bytes[header..]);
        offset += n as u64;
    }
    check_cancelled(cancelled)?;
    Ok((hash.finish(), checksum.finish()))
}

fn check_cancelled(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "map preparation cancelled"))
    } else {
        Ok(())
    }
}

#[cfg_attr(target_vendor = "apple", allow(unsafe_code))]
fn destination(source: &File, path: &Path, len: u64) -> io::Result<(File, bool)> {
    #[cfg(target_vendor = "apple")]
    {
        use std::{ffi::CString, os::fd::AsRawFd, os::unix::ffi::OsStrExt};
        let dst = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: The source descriptor stays open; the absolute destination is new
        // inside our private directory. APFS clones both data and length.
        if unsafe { libc::fclonefileat(source.as_raw_fd(), libc::AT_FDCWD, dst.as_ptr(), 0) } == 0 {
            return Ok((File::open(path)?, true));
        }
        // The directory was empty before the clone. Discard any partial
        // entry before opening the fallback with create_new.
        if let Err(e) = fs::remove_file(path)
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(e);
        }
    }
    // create_new opens a fresh read-write descriptor, so a successful
    // descriptor-based clone can also be read for validation.
    let out = File::create_new(path)?;
    #[cfg(target_os = "linux")]
    if rustix::fs::ioctl_ficlone(&out, source).is_ok() {
        return Ok((out, true));
    }
    #[cfg(windows)]
    if clone_windows(source, &out, len)? {
        return Ok((out, true));
    }
    let _ = (source, len);
    Ok((out, false))
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win32 {
    use std::ffi::c_void;
    use std::os::windows::io::RawHandle;

    // winioctl.h: FILE_DEVICE_FILE_SYSTEM, function 209, METHOD_BUFFERED,
    // FILE_WRITE_DATA. Offsets and lengths are LARGE_INTEGER values.
    pub(super) const FSCTL_DUPLICATE_EXTENTS_TO_FILE: u32 = 0x0009_8344;
    #[repr(C)]
    pub(super) struct DuplicateExtents {
        pub source: RawHandle,
        pub source_offset: i64,
        pub target_offset: i64,
        pub byte_count: i64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn DeviceIoControl(
            handle: RawHandle,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        pub(super) fn OpenProcess(access: u32, inherit: i32, pid: u32) -> RawHandle;
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn clone_windows(source: &File, mut out: &File, len: u64) -> io::Result<bool> {
    use std::{mem::size_of, os::windows::io::AsRawHandle, ptr};
    use win32::{DeviceIoControl, DuplicateExtents, FSCTL_DUPLICATE_EXTENTS_TO_FILE};
    // ReFS uses 4 or 64 KiB clusters. Clone the aligned prefix and copy the
    // final partial cluster from the same open source descriptor.
    let aligned = len & !0xffff;
    if aligned == 0 {
        return Ok(false);
    }
    out.set_len(len)?;
    let request = DuplicateExtents {
        source: source.as_raw_handle(),
        source_offset: 0,
        target_offset: 0,
        byte_count: aligned as i64,
    };
    let mut returned = 0;
    // SAFETY: Both handles outlive this synchronous operation. The single input is
    // the documented extent request and the call has no output buffer.
    let ok = unsafe {
        DeviceIoControl(
            out.as_raw_handle(),
            FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            ptr::from_ref(&request).cast(),
            size_of::<DuplicateExtents>() as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Ok(false);
    }
    let mut source = source;
    source.seek(io::SeekFrom::Start(aligned))?;
    out.seek(io::SeekFrom::Start(aligned))?;
    if io::copy(&mut source.take(len - aligned), &mut out)? != len - aligned {
        return Err(io::Error::other("the map changed size while being cloned"));
    }
    Ok(true)
}

fn remove_file(path: &Path) {
    #[cfg(windows)]
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.is_file()
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
    let _ = fs::remove_file(path);
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        remove_file(&self.path);
        if let Some(dir) = self.path.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// Clean up after an interrupted process on the first private map preparation.
fn reap_abandoned() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| reap(&std::env::temp_dir(), process_is_dead));
}

#[cfg_attr(unix, allow(unsafe_code))]
pub(super) fn reap(root: &Path, dead: impl Fn(u32) -> bool) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((pid, rest)) = name.to_str().and_then(|n| n.strip_prefix("h2-map-"))
            .and_then(|n| n.split_once('-')) else { continue };
        let Some((seq, nonce)) = rest.split_once('-') else { continue };
        if seq.parse::<u64>().is_err() || nonce.len() != 16
            || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
        {
            continue;
        }
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid == 0 || !dead(pid) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Shared temporary roots can hold another user's directories.
            // SAFETY: geteuid reads the effective uid and accepts no pointers.
            let uid = unsafe { libc::geteuid() };
            if !fs::symlink_metadata(&dir).is_ok_and(|m| {
                m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0
            }) {
                continue;
            }
        }
        let Ok(mut files) = fs::read_dir(&dir) else { continue };
        // A generation contains at most its one regular map. A directory
        // with any other contents is not ours to remove.
        if let Some(entry) = files.next() {
            let Ok(file) = entry else { continue };
            let path = file.path();
            if files.next().is_some() || !file.file_type().is_ok_and(|t| t.is_file())
                || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("map"))
            {
                continue;
            }
            remove_file(&path);
        }
        drop(files);
        let _ = fs::remove_dir(dir);
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn process_is_dead(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: pid is positive and signal zero only queries its existence.
    let missing = unsafe { libc::kill(pid, 0) } != 0;
    missing && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn process_is_dead(pid: u32) -> bool {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    // SAFETY: This call takes only integers and gives us ownership of its handle.
    let handle = unsafe { win32::OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: The returned handle is valid and is closed exactly once.
    drop(unsafe { OwnedHandle::from_raw_handle(handle) });
    false
}

#[cfg(not(any(unix, windows)))]
fn process_is_dead(_: u32) -> bool {
    false
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_streaming_copy_stops_before_the_next_chunk() {
        struct FirstChunk<'a> {
            cancelled: &'a AtomicBool,
            written: usize,
        }

        impl Write for FirstChunk<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.written += bytes.len();
                self.cancelled.store(true, Ordering::Release);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let cancelled = AtomicBool::new(false);
        let mut out = FirstChunk { cancelled: &cancelled, written: 0 };
        let err = digest(&mut io::repeat(1), (1 << 20) + 1, &mut out, &cancelled).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(out.written, 1 << 20);
    }
}
