//! Copy-on-write file copies for prepared data: the sim batch's map cache
//! templates (`crates/apps/cli/src/sim_cmd/cache_template.rs`) and the N:
//! seed after a format ([`crate::Vfs::seed_device`],
//! docs/runtime/cache-slots.md "Sim batches: a warm N:").

use std::fs;
use std::io;
use std::path::Path;

/// Clones `src` to the new file `dst` (APFS `fclonefileat`, so the two
/// share blocks until one is written), else copies it. True if it was
/// cloned. `dst` must not exist.
#[cfg_attr(target_vendor = "apple", allow(unsafe_code))]
pub fn clone_or_copy(src: &Path, dst: &Path) -> io::Result<bool> {
    #[cfg(target_vendor = "apple")]
    {
        use std::{ffi::CString, os::fd::AsRawFd, os::unix::ffi::OsStrExt};
        let source = fs::File::open(src)?;
        let target = CString::new(dst.as_os_str().as_bytes())?;
        // SAFETY: the source descriptor stays open for the call and the
        // destination is a NUL-terminated path that outlives it.
        if unsafe { libc::fclonefileat(source.as_raw_fd(), libc::AT_FDCWD, target.as_ptr(), 0) }
            == 0
        {
            return Ok(true);
        }
        match fs::remove_file(dst) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    // Linux's copy_file_range shares extents where the filesystem can.
    fs::copy(src, dst)?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_a_file_of_its_own() {
        let d = std::env::temp_dir().join(format!("h2-vfs-clone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a"), d.join("b"));
        fs::write(&a, b"template").unwrap();
        clone_or_copy(&a, &b).unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"template");
        fs::write(&b, b"changed").unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"template");
        let _ = fs::remove_dir_all(&d);
    }
}
