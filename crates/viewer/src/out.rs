//! No-clobber, symlink-safe creation of a single output file (the `--png`
//! screenshot). Same rules as the cli's `safe_out` module: every directory
//! component of the path as given (from `/` for an absolute path, from the
//! working directory for a relative one) must be a real directory, checked
//! with `symlink_metadata`, so `link/x.png` with `link -> real` is refused
//! rather than writing `real/x.png`. `..` components are refused too. Missing
//! directories are created one level at a time ([`ensure_dir`]) under the
//! same check. The final component is never followed when it is a symlink
//! (`create_new` is `O_EXCL`), an existing file is refused unless `force` is
//! set, and `force` only replaces a regular file. A path through a system
//! symlink (macOS `/tmp`, `/var`) is refused as well; pass the resolved path
//! (`/private/tmp/...`). It does not guard against another process racing the
//! directory tree while it runs.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

fn refuse(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Walk `dir` component by component, requiring each existing prefix to be a
/// real directory (not a symlink). With `create`, missing levels are made one
/// at a time; without it a missing level is an error.
fn walk(dir: &Path, create: bool) -> io::Result<()> {
    let mut cur = PathBuf::new();
    for c in dir.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => {
                cur.push(c);
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(refuse(format!("{}: `..` in output path", dir.display())));
            }
            Component::Normal(_) => cur.push(c),
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(refuse(format!(
                    "{} is not a real directory (symlink?)",
                    cur.display()
                )));
            }
            Err(e) if create && e.kind() == io::ErrorKind::NotFound => fs::create_dir(&cur)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Create `dir` and any missing parents, refusing symlinked components.
/// Used by the `bsp` binary's render directory; the viewer writes one file.
#[allow(dead_code)]
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    walk(dir, true)
}

pub fn create(path: &Path, force: bool) -> io::Result<File> {
    if path.file_name().is_none() {
        return Err(refuse(format!("{} names no file", path.display())));
    }
    if let Some(parent) = path.parent() {
        walk(parent, false)?;
    }
    match fs::symlink_metadata(path) {
        Ok(m) if force && m.is_file() => fs::remove_file(path)?,
        Ok(m) if m.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} is a symlink; refusing to write through it",
                    path.display()
                ),
            ));
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} exists (use --force to replace a regular file)",
                    path.display()
                ),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Create `path` and fill it via `fill`; a partial file is removed on error.
pub fn write_with(
    path: &Path,
    force: bool,
    fill: impl FnOnce(&mut io::BufWriter<File>) -> io::Result<()>,
) -> io::Result<()> {
    let mut w = io::BufWriter::new(create(path, force)?);
    let result = fill(&mut w).and_then(|()| w.flush());
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-viewer-out-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn put(p: &Path, force: bool, b: &[u8]) -> io::Result<()> {
        write_with(p, force, |w| w.write_all(b))
    }

    #[test]
    fn refuses_overwrite_without_force() {
        let d = scratch("clobber");
        let f = d.join("x.png");
        put(&f, false, b"1").unwrap();
        let e = put(&f, false, b"2").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&f).unwrap(), b"1");
        put(&f, true, b"3").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"3");
        // --force never replaces a directory.
        fs::create_dir(d.join("dir")).unwrap();
        assert!(put(&d.join("dir"), true, b"x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_final_symlink() {
        use std::os::unix::fs::symlink;
        let d = scratch("links");
        let victim = d.join("victim");
        fs::write(&victim, b"keep").unwrap();
        symlink(&victim, d.join("y.png")).unwrap();
        assert!(put(&d.join("y.png"), false, b"x").is_err());
        assert!(put(&d.join("y.png"), true, b"x").is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        // A dangling link is not created through either.
        symlink(d.join("new"), d.join("z.png")).unwrap();
        assert!(put(&d.join("z.png"), true, b"x").is_err());
        assert!(!d.join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_symlinked_parent() {
        use std::os::unix::fs::symlink;
        let d = scratch("parents");
        let real = d.join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, d.join("link")).unwrap();
        // Directly under the link, and deeper below it.
        fs::create_dir(real.join("sub")).unwrap();
        for p in [d.join("link/x.png"), d.join("link/sub/x.png")] {
            assert!(put(&p, false, b"x").is_err(), "{}", p.display());
            assert!(put(&p, true, b"x").is_err(), "{}", p.display());
        }
        assert!(ensure_dir(&d.join("link/new")).is_err());
        assert_eq!(fs::read_dir(&real).unwrap().count(), 1, "only sub/ remains");
        assert_eq!(fs::read_dir(real.join("sub")).unwrap().count(), 0);
        // `..` is refused even when it would resolve to a real directory.
        assert!(put(&d.join("real/../x.png"), false, b"x").is_err());
        assert!(!d.join("x.png").exists());
        // Real directories are created one level at a time and then accepted.
        ensure_dir(&d.join("made/a/b")).unwrap();
        put(&d.join("made/a/b/x.png"), false, b"ok").unwrap();
        assert_eq!(fs::read(d.join("made/a/b/x.png")).unwrap(), b"ok");
    }

    #[test]
    fn failed_fill_leaves_no_partial_file() {
        let d = scratch("partial");
        let f = d.join("p.png");
        assert!(write_with(&f, false, |_| Err(io::Error::other("boom"))).is_err());
        assert!(!f.exists());
    }
}
