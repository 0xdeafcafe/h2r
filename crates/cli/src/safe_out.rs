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
use std::io::{self, Write};
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
                Ok(_) => return Err(bad(format!("{} is not a real directory (symlink?)", cur.display()))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&cur)?,
                Err(e) => return Err(e),
            }
        }
        cur.push(file);
        match fs::symlink_metadata(&cur) {
            Ok(m) if self.force && m.is_file() => fs::remove_file(&cur)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists (use --force to replace a regular file)", cur.display()),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut f: File = OpenOptions::new().write(true).create_new(true).open(&cur)?;
        f.write_all(bytes)?;
        Ok(cur)
    }
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
