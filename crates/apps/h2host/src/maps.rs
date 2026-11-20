//! Which maps an app bundle carries (`maps=demo|all`, docs/runtime/building.md),
//! and the hard-disk seed that brings the Multiplayer Map Pack along.
//!
//! * `demo` is [`crate::layout::DISC_FILES`]: the menu, the shared maps, The
//!   Heretic and Zanzibar, about 1 GB.
//! * `all` is the whole disc, every campaign and multiplayer map, plus the
//!   map pack when a `mappack.iso` is given.
//!
//! The map pack is not on the game disc. On the Mac, `h2 play` installs it
//! into the data root's hard disk as nine 1.0 content packages
//! (`h2 import-mappack`, docs/game/map-pack.md section 8):
//! `<data>/disk/Partition1/TDATA/4D530064/$c/<offer id>/`. A device's data
//! root is writable but empty at install, so `build.sh` runs the same import
//! into `bootcache/`[`SEED_DIR`] in the bundle, and the app copies that tree
//! into its data root at launch ([`seed`], from `Layout::prepare`). The seed
//! rides in `bootcache/` because that folder is already in both Xcode
//! projects; `Layout::prepare` copies only the top level's files into the
//! boot cache, so the seed directory never lands there.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

/// The data-root seed's directory inside the bundled boot cache.
pub const SEED_DIR: &str = "h2data";

/// What `pack --maps` copies from the disc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Maps {
    /// [`crate::layout::DISC_FILES`] only.
    #[default]
    Demo,
    /// Every file on the disc.
    All,
}

impl Maps {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "demo" => Some(Maps::Demo),
            "all" => Some(Maps::All),
            _ => None,
        }
    }

    /// Whether the disc entry at `path` (as `xiso` names it, from the root)
    /// goes in the bundle.
    pub fn wants(self, path: &str, is_dir: bool) -> bool {
        match self {
            Maps::Demo => xiso::subset_matches(path, is_dir, crate::layout::DISC_FILES),
            Maps::All => true,
        }
    }
}

/// Copies every file under `from` that is missing from `to`, or there with
/// another size, keeping the tree. Returns how many were copied. A missing
/// `from` is no seed (zero). Symlinks in `from` are skipped, never
/// followed. Files already in `to` and not in `from` are left alone: they
/// are the game's own writes.
///
/// On APFS (the app bundle and its data container share a volume),
/// `std::fs::copy` clones, so a seeded map pack takes no extra space until
/// one side changes.
pub fn seed(from: &Path, to: &Path) -> io::Result<usize> {
    if !from.is_dir() {
        return Ok(0);
    }
    let mut copied = 0;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        std::fs::create_dir_all(to.join(&rel))?;
        for e in std::fs::read_dir(from.join(&rel))? {
            let e = e?;
            let kind = e.file_type()?;
            let rel = rel.join(e.file_name());
            if kind.is_dir() {
                stack.push(rel);
            } else if kind.is_file() {
                let dest = to.join(&rel);
                let want = e.metadata()?.len();
                if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == want) {
                    continue;
                }
                let mut tmp = dest.clone().into_os_string();
                tmp.push(".seeding");
                std::fs::copy(e.path(), &tmp)?;
                std::fs::rename(&tmp, &dest)?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

/// Removes the files under `root` whose path relative to it is not in
/// `keep`, then the directories left empty (never `root` itself). This is
/// how a `maps=demo` pack drops what an earlier `maps=all` pack copied.
/// Returns the files removed and their bytes. Symlinks are removed, not
/// followed.
pub fn prune(root: &Path, keep: &BTreeSet<PathBuf>) -> io::Result<(usize, u64)> {
    fn walk(
        root: &Path,
        rel: &Path,
        keep: &BTreeSet<PathBuf>,
        gone: &mut (usize, u64),
    ) -> io::Result<bool> {
        let mut empty = true;
        for e in std::fs::read_dir(root.join(rel))? {
            let e = e?;
            let kind = e.file_type()?;
            let rel = rel.join(e.file_name());
            if kind.is_dir() {
                if walk(root, &rel, keep, gone)? && !keep.contains(&rel) {
                    std::fs::remove_dir(root.join(&rel))?;
                } else {
                    empty = false;
                }
            } else if keep.contains(&rel) {
                empty = false;
            } else {
                gone.0 += 1;
                gone.1 += e.metadata()?.len();
                std::fs::remove_file(root.join(&rel))?;
            }
        }
        Ok(empty)
    }
    let mut gone = (0, 0);
    if root.is_dir() {
        walk(root, Path::new(""), keep, &mut gone)?;
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("h2-host-maps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn demo_is_the_bundled_subset_and_all_is_every_file() {
        assert_eq!(Maps::parse("demo"), Some(Maps::Demo));
        assert_eq!(Maps::parse("all"), Some(Maps::All));
        assert_eq!(Maps::parse("ALL"), None);
        assert_eq!(Maps::parse(""), None);
        assert_eq!(Maps::default(), Maps::Demo);
        for (p, dir) in [
            ("/default.xbe", false),
            ("/maps/zanzibar.map", false),
            ("/maps/fonts/font_table.txt", false),
            ("/maps", true),
        ] {
            assert!(Maps::Demo.wants(p, dir) && Maps::All.wants(p, dir), "{p}");
        }
        for (p, dir) in [
            ("/maps/ascension.map", false),
            ("/maps/01a_tutorial.map", false),
            ("/bink/credits_60.bik", false),
            ("/extras", true),
        ] {
            assert!(!Maps::Demo.wants(p, dir), "{p}");
            assert!(Maps::All.wants(p, dir), "{p}");
        }
    }

    /// The seed is the import's layout; it lands under the data root as is.
    const PACKAGE: &str = "disk/Partition1/TDATA/4D530064/$c/4d53006400000101";

    #[test]
    fn seed_copies_the_package_tree_into_the_data_root() {
        let t = temp("seed");
        let (from, to) = (
            t.join("bundle/bootcache").join(SEED_DIR),
            t.join("caches/h2data"),
        );
        write(&from.join(PACKAGE).join("ContentMeta.xbx"), b"meta");
        write(&from.join(PACKAGE).join("patch.lvl"), b"lvl");
        write(&from.join(PACKAGE).join("example.map"), b"a map");
        assert_eq!(seed(&from, &to).unwrap(), 3);
        assert_eq!(
            std::fs::read(to.join(PACKAGE).join("example.map")).unwrap(),
            b"a map"
        );
        assert_eq!(
            std::fs::read(to.join(PACKAGE).join("patch.lvl")).unwrap(),
            b"lvl"
        );
        assert!(!to.join(PACKAGE).join("example.map.seeding").exists());
        assert_eq!(
            seed(&from, &to).unwrap(),
            0,
            "a second launch copies nothing"
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn seed_restores_purged_files_and_keeps_the_games_writes() {
        let t = temp("purge");
        let (from, to) = (t.join("seed"), t.join("data"));
        write(&from.join(PACKAGE).join("example.map"), b"a map");
        write(&from.join(PACKAGE).join("patch.lvl"), b"lvl");
        write(&to.join("disk/Partition1/UDATA/save.bin"), b"a save");
        assert_eq!(seed(&from, &to).unwrap(), 2);
        // tvOS purged one file, and another came out short.
        std::fs::remove_file(to.join(PACKAGE).join("example.map")).unwrap();
        std::fs::write(to.join(PACKAGE).join("patch.lvl"), b"l").unwrap();
        assert_eq!(seed(&from, &to).unwrap(), 2);
        assert_eq!(
            std::fs::read(to.join(PACKAGE).join("example.map")).unwrap(),
            b"a map"
        );
        assert_eq!(
            std::fs::read(to.join(PACKAGE).join("patch.lvl")).unwrap(),
            b"lvl"
        );
        assert_eq!(
            std::fs::read(to.join("disk/Partition1/UDATA/save.bin")).unwrap(),
            b"a save"
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn no_seed_is_nothing_to_do() {
        let t = temp("none");
        assert_eq!(seed(&t.join("missing"), &t.join("data")).unwrap(), 0);
        assert!(!t.join("data").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(unix)]
    #[test]
    fn seed_never_follows_a_symlink() {
        let t = temp("link");
        let (from, to) = (t.join("seed"), t.join("data"));
        write(&t.join("outside/secret"), b"secret");
        std::fs::create_dir_all(&from).unwrap();
        std::os::unix::fs::symlink(t.join("outside"), from.join("dir")).unwrap();
        std::os::unix::fs::symlink(t.join("outside/secret"), from.join("file")).unwrap();
        assert_eq!(seed(&from, &to).unwrap(), 0);
        assert!(!to.join("dir").exists() && !to.join("file").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn prune_drops_what_an_all_pack_left_and_keeps_the_demo_set() {
        let t = temp("prune");
        let disc = t.join("disc");
        for p in [
            "default.xbe",
            "maps/zanzibar.map",
            "maps/fonts/font_table.txt",
            "maps/ascension.map",
            "bink/credits_60.bik",
        ] {
            write(&disc.join(p), b"12345");
        }
        let keep: BTreeSet<PathBuf> = [
            "default.xbe",
            "maps",
            "maps/zanzibar.map",
            "maps/fonts",
            "maps/fonts/font_table.txt",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(prune(&disc, &keep).unwrap(), (2, 10));
        assert!(!disc.join("maps/ascension.map").exists());
        assert!(!disc.join("bink").exists(), "an emptied directory goes too");
        assert!(
            disc.join("maps/zanzibar.map").is_file()
                && disc.join("maps/fonts/font_table.txt").is_file()
        );
        assert!(disc.is_dir());
        assert_eq!(prune(&disc, &keep).unwrap(), (0, 0));
        assert_eq!(prune(&t.join("missing"), &keep).unwrap(), (0, 0));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn prune_keeps_an_empty_directory_the_disc_has() {
        let t = temp("emptydir");
        let disc = t.join("disc");
        std::fs::create_dir_all(disc.join("media/empty")).unwrap();
        write(&disc.join("stale.bin"), b"x");
        let keep: BTreeSet<PathBuf> = ["media", "media/empty"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(prune(&disc, &keep).unwrap(), (1, 1));
        assert!(disc.join("media/empty").is_dir());
        let _ = std::fs::remove_dir_all(&t);
    }
}
