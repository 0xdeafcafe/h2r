//! Where snapshots live: the user's cache directory, never the repository
//! (they hold game data), one directory per build.
//!
//! ```text
//! <root>/<BuildKey::id>/<name>.h2snap
//! ```
//!
//! `<root>` is `$H2_SNAPSHOTS`, else `~/Library/Caches/h2recomp/snapshots`
//! on Apple hosts, `%LOCALAPPDATA%\h2recomp\cache\snapshots` on Windows and
//! `$XDG_CACHE_HOME/h2recomp/snapshots` (`~/.cache/...`) elsewhere: the
//! same directory the pipeline cache and the agent socket use. A build's
//! snapshots are useless to any other build, so a rebuilt image starts with
//! an empty list and old directories can be deleted wholesale.
use crate::Error;
use crate::container::{BuildKey, Header, read_header};
use std::path::{Path, PathBuf};

pub const ENV: &str = "H2_SNAPSHOTS";
pub const EXTENSION: &str = "h2snap";

/// The default root from the environment; `None` without a home directory.
pub fn default_root() -> Option<PathBuf> {
    root_from(
        std::env::var_os(ENV),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("LOCALAPPDATA"),
    )
}

fn root_from(
    var: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    local_app_data: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let nonempty = |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty());
    if let Some(v) = nonempty(var) {
        return Some(PathBuf::from(v));
    }
    let cache = if cfg!(target_vendor = "apple") {
        PathBuf::from(nonempty(home)?).join("Library/Caches/h2recomp")
    } else if cfg!(windows) {
        PathBuf::from(nonempty(local_app_data)?)
            .join("h2recomp")
            .join("cache")
    } else {
        nonempty(xdg)
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(nonempty(home)?).join(".cache")))?
            .join("h2recomp")
    };
    Some(cache.join("snapshots"))
}

/// A snapshot name: 1 to 64 of `a-z 0-9 . _ -`, not starting with a dot.
/// `oldmombasa.courtyard_fight` is one. Names never form paths.
pub fn check_name(name: &str) -> Result<(), Error> {
    let ok = (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(Error::BadName(name.to_owned()))
    }
}

/// One listed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub bytes: u64,
    pub header: Header,
}

/// The snapshots of one build.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(root: &Path, build: &BuildKey) -> Self {
        Self {
            dir: root.join(build.id()),
        }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn path(&self, name: &str) -> Result<PathBuf, Error> {
        check_name(name)?;
        Ok(self.dir.join(format!("{name}.{EXTENSION}")))
    }
    /// Write atomically: a crash leaves the old file or the new one, never
    /// half of one. Replaces a snapshot of the same name.
    pub fn save(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, Error> {
        let path = self.path(name)?;
        std::fs::create_dir_all(&self.dir).map_err(Error::Io)?;
        let tmp = self.dir.join(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(Error::Io)?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::Io(e)
        })?;
        Ok(path)
    }
    pub fn load(&self, name: &str) -> Result<Vec<u8>, Error> {
        let path = self.path(name)?;
        std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NoSuchSnapshot(name.to_owned()),
            _ => Error::Io(e),
        })
    }
    pub fn remove(&self, name: &str) -> Result<(), Error> {
        let path = self.path(name)?;
        std::fs::remove_file(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NoSuchSnapshot(name.to_owned()),
            _ => Error::Io(e),
        })
    }
    /// Snapshots by name. Files that are not snapshots, or whose header
    /// does not read, are skipped; temporary files are never listed.
    pub fn list(&self) -> Result<Vec<Listed>, Error> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(Error::Io)?;
            let path = entry.path();
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(&format!(".{EXTENSION}")))
            else {
                continue;
            };
            if check_name(name).is_err() {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(header) = read_header(&bytes) {
                out.push(Listed {
                    name: name.to_owned(),
                    bytes: bytes.len() as u64,
                    header,
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{Codec, SnapshotWriter, Tag};

    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("h2-snapshot-store-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn build() -> BuildKey {
        BuildKey {
            stamp: "xbe 0 generator 0".into(),
            switches: String::new(),
        }
    }

    fn file(name: &str) -> Vec<u8> {
        let mut w = SnapshotWriter::new(Header {
            build: build(),
            name: name.into(),
            tick: 9,
            presents: 18,
            notes: vec![],
        });
        w.section(Tag::new("x"), 1, Codec::Stored, vec![1, 2, 3])
            .unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn names_are_plain() {
        for good in ["a", "oldmombasa.courtyard_fight", "x-1"] {
            check_name(good).unwrap();
        }
        for bad in ["", ".hidden", "../up", "a/b", "A", "a b", &"x".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn save_list_load_remove() {
        let root = scratch("cycle");
        let store = Store::new(&root, &build());
        assert_eq!(store.list().unwrap(), vec![]);
        store.save("b.two", &file("b.two")).unwrap();
        let path = store.save("a.one", &file("a.one")).unwrap();
        assert!(path.starts_with(root.join(build().id())));
        // Stray files are not snapshots.
        std::fs::write(store.dir().join("notes.txt"), "x").unwrap();
        std::fs::write(store.dir().join("junk.h2snap"), "x").unwrap();
        let listed = store.list().unwrap();
        assert_eq!(
            listed.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            vec!["a.one", "b.two"]
        );
        assert_eq!(listed[0].header.tick, 9);
        assert_eq!(store.load("a.one").unwrap(), file("a.one"));
        // Saving again replaces.
        store.save("a.one", &file("a.one.v2")).unwrap();
        assert_eq!(store.load("a.one").unwrap(), file("a.one.v2"));
        store.remove("a.one").unwrap();
        assert!(matches!(store.load("a.one"), Err(Error::NoSuchSnapshot(_))));
        assert!(matches!(
            store.remove("a.one"),
            Err(Error::NoSuchSnapshot(_))
        ));
        assert!(matches!(store.load("../a"), Err(Error::BadName(_))));
        // No temporary file is left behind.
        assert!(
            std::fs::read_dir(store.dir()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn builds_do_not_share_a_directory() {
        let root = scratch("builds");
        let mut other = build();
        other.switches = "-agent".into();
        let a = Store::new(&root, &build());
        let b = Store::new(&root, &other);
        a.save("x", &file("x")).unwrap();
        assert_eq!(b.list().unwrap(), vec![]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_root_follows_the_platform_cache() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            root_from(os("/x/snaps"), None, os("/h"), None),
            Some(PathBuf::from("/x/snaps"))
        );
        let home = root_from(os(""), os("/xdg"), os("/h"), os("C:\\L"));
        if cfg!(target_vendor = "apple") {
            assert_eq!(
                home,
                Some(PathBuf::from("/h/Library/Caches/h2recomp/snapshots"))
            );
        } else if cfg!(windows) {
            assert_eq!(
                home,
                Some(PathBuf::from("C:\\L").join("h2recomp/cache/snapshots"))
            );
        } else {
            assert_eq!(home, Some(PathBuf::from("/xdg/h2recomp/snapshots")));
        }
        if !cfg!(windows) {
            assert_eq!(root_from(None, None, None, None), None);
        }
    }
}
