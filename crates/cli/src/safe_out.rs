//! Symlink-safe, no-clobber output directory for exporters.
//!
//! Every directory component below the chosen root is created one level at a
//! time and verified with `symlink_metadata` to be a real directory (never a
//! symlink), and files are created with `create_new` (`O_EXCL`, which does not
//! follow a symlink at the final component). Existing files are refused unless
//! `force` is set, in which case only a regular file is replaced. Components
//! are validated: no empty, `.`, `..`, separators, drive colons or NUL. This
//! stops a pre-planted link or a hostile tag name from leaving the root. It
//! does not defend against another process racing the directory tree while the
//! export runs. The root itself is the caller's choice and may be a symlink.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;

pub struct OutDir {
    root: PathBuf,
    force: bool,
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

impl OutDir {
    pub fn new(root: impl Into<PathBuf>, force: bool) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root, force })
    }

    /// Write `bytes` to `root/<components...>`; returns the final path.
    pub fn write(&self, components: &[String], bytes: &[u8]) -> io::Result<PathBuf> {
        self.write_from(components, &mut &*bytes)
    }

    /// Like `write`, but streams from `src` (no whole-file buffer).
    ///
    /// The data goes to a temporary name in the destination directory and is
    /// published only once fully written, so a failing `src` never leaves a
    /// partial file and never destroys an existing one (under `force` the old
    /// file is replaced by an atomic rename). Without `force` publishing is a
    /// hard link, which fails if something appeared at the name meanwhile.
    pub fn write_from(&self, components: &[String], src: &mut impl io::Read) -> io::Result<PathBuf> {
        let dest = self.resolve(components)?;
        let tmp = dest.with_file_name(format!(
            ".{}.tmp-{}",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
            std::process::id()
        ));
        // create_new: never follows a planted symlink at the temp name either.
        let mut f: File = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let published = io::copy(src, &mut f).and_then(|_| f.sync_all()).and_then(|()| {
            if self.force {
                // Re-check right before replacing: only a regular file goes.
                match fs::symlink_metadata(&dest) {
                    Ok(m) if !m.is_file() => Err(exists(&dest)),
                    Ok(_) | Err(_) => fs::rename(&tmp, &dest),
                }
            } else {
                fs::hard_link(&tmp, &dest)
                    .map_err(|e| if e.kind() == io::ErrorKind::AlreadyExists { exists(&dest) } else { e })
            }
        });
        if !self.force || published.is_err() {
            let _ = fs::remove_file(&tmp); // after a rename the temp name is already gone
        }
        published.map(|()| dest)
    }

    /// Validate `components`, create parent directories and return the final
    /// path after checking what already sits there.
    fn resolve(&self, components: &[String]) -> io::Result<PathBuf> {
        let (file, dirs) = components.split_last().ok_or_else(|| bad("empty output path".into()))?;
        for c in components {
            if c.is_empty() || c == "." || c == ".." || c.contains(['/', '\\', ':', '\0']) {
                return Err(bad(format!("unsafe output path component {c:?}")));
            }
        }
        let mut cur = self.root.clone();
        for d in dirs {
            cur.push(d);
            match fs::symlink_metadata(&cur) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => {
                    return Err(bad(format!("{} is not a real directory (symlink?)", cur.display())));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&cur)?,
                Err(e) => return Err(e),
            }
        }
        cur.push(file);
        match fs::symlink_metadata(&cur) {
            Ok(m) if self.force && m.is_file() => {}
            Ok(_) => return Err(exists(&cur)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(cur)
    }
}

fn exists(p: &std::path::Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} exists (use --force to replace a regular file)", p.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-safe-out-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn parts(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn writes_nested_and_refuses_overwrite_without_force() {
        let d = scratch("nested");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let p = out.write(&parts(&["a", "b", "x.wav"]), b"1").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"1");
        let e = out.write(&parts(&["a", "b", "x.wav"]), b"2").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&p).unwrap(), b"1");
        let forced = OutDir::new(d.join("out"), true).unwrap();
        forced.write(&parts(&["a", "b", "x.wav"]), b"3").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"3");
    }

    #[test]
    fn write_from_streams_and_refuses_overwrite() {
        let d = scratch("stream");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let p = out.write_from(&parts(&["m.mp4"]), &mut &b"abc"[..]).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"abc");
        assert!(out.write_from(&parts(&["m.mp4"]), &mut &b"x"[..]).is_err());
    }

    struct Failing(usize);
    impl io::Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("boom"));
            }
            let n = self.0.min(buf.len());
            buf[..n].fill(9);
            self.0 -= n;
            Ok(n)
        }
    }

    #[test]
    fn failed_copy_leaves_nothing_and_keeps_old_file() {
        let d = scratch("failcopy");
        let root = d.join("out");
        let out = OutDir::new(&root, false).unwrap();
        assert!(out.write_from(&parts(&["m.mp4"]), &mut Failing(5000)).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "partial or temp file left behind");
        let forced = OutDir::new(&root, true).unwrap();
        forced.write(&parts(&["m.mp4"]), b"good").unwrap();
        assert!(forced.write_from(&parts(&["m.mp4"]), &mut Failing(5000)).is_err());
        assert_eq!(fs::read(root.join("m.mp4")).unwrap(), b"good");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn rejects_unsafe_components() {
        let d = scratch("unsafe");
        let out = OutDir::new(d.join("out"), true).unwrap();
        for bad in ["", ".", "..", "a/b", "a\\b", "c:", "a\0b"] {
            assert!(out.write(&parts(&[bad, "f"]), b"x").is_err(), "{bad:?}");
            assert!(out.write(&parts(&["d", bad]), b"x").is_err(), "{bad:?}");
        }
        assert!(out.write(&[], b"x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_and_file_cannot_escape() {
        use std::os::unix::fs::symlink;
        let d = scratch("links");
        let outside = d.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let root = d.join("out");
        let out = OutDir::new(&root, true).unwrap();
        symlink(&outside, root.join("sound")).unwrap();
        assert!(out.write(&parts(&["sound", "x.wav"]), b"x").is_err());
        assert!(!outside.join("x.wav").exists());
        // A final-component symlink is neither followed nor replaced by --force.
        let target = outside.join("victim");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, root.join("y.wav")).unwrap();
        assert!(out.write(&parts(&["y.wav"]), b"x").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        // A dangling final symlink is not created through either.
        symlink(outside.join("new"), root.join("z.wav")).unwrap();
        assert!(out.write(&parts(&["z.wav"]), b"x").is_err());
        assert!(!outside.join("new").exists());
    }
}
