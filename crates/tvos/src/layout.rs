//! Where the tvOS app finds and keeps things.
//!
//! The bundle is read-only: the trimmed disc (`disc/`, a directory of the
//! disc's files, served by `vfs::DirDiscBackend`), the generated module (an
//! embedded framework, with its build stamp beside the binary) and the
//! boot cache's analysis, so the first launch need not analyse the XBE.
//! Everything written goes under the app's Caches directory, which tvOS
//! may purge when it needs space: saves and settings included (docs/runtime/tvos.md).

use std::path::{Path, PathBuf};

/// The generated module's framework inside the bundle (its `-install_name`
/// is `@rpath/h2game.framework/h2game`, emit::compile::Arch::install_name).
pub const FRAMEWORK: &str = "Frameworks/h2game.framework/h2game";

/// The disc files the app bundles (`xiso::subset_matches` patterns): the
/// XBE, the menu, the shared maps, The Heretic (`00a_introduction`),
/// Zanzibar, the fonts, the dashboard media and the intro movie. About
/// 1 GB of the disc's 7 GB; the other levels and multiplayer maps are left
/// out, and choosing one stops the game (docs/runtime/tvos.md, "Known gaps").
pub const DISC_FILES: &[&str] = &[
    "default.xbe",
    "maps/mainmenu.map",
    "maps/shared.map",
    "maps/single_player_shared.map",
    "maps/00a_introduction.map",
    "maps/zanzibar.map",
    "maps/fonts/",
    "media/",
    "bink/intro_60.bik",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The trimmed disc, mounted as D:.
    pub disc: PathBuf,
    /// The generated module (`<module>.stamp` beside it).
    pub module: PathBuf,
    /// The boot cache shipped in the bundle (read-only).
    pub bundled_cache: PathBuf,
    /// Hard-disk storage, settings and saves (`RunOpts::data_root`).
    pub data: PathBuf,
    /// The boot cache used at run time (`H2_BOOT_CACHE`), writable.
    pub boot_cache: PathBuf,
}

impl Layout {
    /// `bundle` is the app bundle (`Bundle.main.bundlePath`), `caches` the
    /// app's Caches directory.
    pub fn new(bundle: &Path, caches: &Path) -> Self {
        Self {
            disc: bundle.join("disc"),
            module: bundle.join(FRAMEWORK),
            bundled_cache: bundle.join("bootcache"),
            data: caches.join("h2data"),
            boot_cache: caches.join("h2boot-cache"),
        }
    }

    /// What is missing from the bundle, in words; empty when it is whole.
    pub fn missing(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.disc.is_dir() {
            v.push(format!("no disc directory at {}", self.disc.display()));
        }
        if !self.module.is_file() {
            v.push(format!("no generated module at {}", self.module.display()));
        }
        let mut stamp = self.module.as_os_str().to_owned();
        stamp.push(".stamp");
        if !Path::new(&stamp).is_file() {
            v.push(format!(
                "no module stamp at {}",
                Path::new(&stamp).display()
            ));
        }
        v
    }

    /// Creates the writable directories and copies the bundled boot cache
    /// into the writable one, skipping files already there. Returns how
    /// many files were copied.
    pub fn prepare(&self) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.data)?;
        std::fs::create_dir_all(&self.boot_cache)?;
        let mut copied = 0;
        let Ok(entries) = std::fs::read_dir(&self.bundled_cache) else {
            return Ok(0);
        };
        for e in entries.flatten() {
            let to = self.boot_cache.join(e.file_name());
            if e.path().is_file() && !to.exists() {
                std::fs::copy(e.path(), &to)?;
                copied += 1;
            }
        }
        Ok(copied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("h2-tvos-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_bundled_disc_holds_the_menu_the_shared_maps_and_two_levels() {
        let picked = |p: &str| xiso::subset_matches(p, false, DISC_FILES);
        for p in [
            "/default.xbe",
            "/maps/mainmenu.map",
            "/maps/shared.map",
            "/maps/single_player_shared.map",
            "/maps/00a_introduction.map",
            "/maps/zanzibar.map",
            "/maps/fonts/font_table.txt",
            "/media/Sound.xsb",
        ] {
            assert!(picked(p), "{p}");
        }
        for p in [
            "/maps/01a_tutorial.map",
            "/maps/ascension.map",
            "/bink/credits_60.bik",
            "/bink/attract_60.bik",
        ] {
            assert!(!picked(p), "{p}");
        }
        assert!(xiso::subset_matches("/maps", true, DISC_FILES));
    }

    #[test]
    fn paths_are_bundle_relative_and_writes_go_to_caches() {
        let l = Layout::new(Path::new("/b/H2.app"), Path::new("/c/Caches"));
        assert_eq!(l.disc, Path::new("/b/H2.app/disc"));
        assert_eq!(
            l.module,
            Path::new("/b/H2.app/Frameworks/h2game.framework/h2game")
        );
        assert_eq!(l.bundled_cache, Path::new("/b/H2.app/bootcache"));
        assert_eq!(l.data, Path::new("/c/Caches/h2data"));
        assert_eq!(l.boot_cache, Path::new("/c/Caches/h2boot-cache"));
    }

    #[test]
    fn a_whole_bundle_has_nothing_missing() {
        let b = temp("bundle");
        let l = Layout::new(&b, &b.join("caches"));
        assert_eq!(l.missing().len(), 3);
        std::fs::create_dir_all(b.join("disc")).unwrap();
        std::fs::create_dir_all(b.join("Frameworks/h2game.framework")).unwrap();
        std::fs::write(&l.module, b"").unwrap();
        assert_eq!(
            l.missing(),
            [format!("no module stamp at {}.stamp", l.module.display())]
        );
        std::fs::write(b.join("Frameworks/h2game.framework/h2game.stamp"), b"x").unwrap();
        assert!(l.missing().is_empty());
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn prepare_seeds_the_boot_cache_once_and_keeps_newer_files() {
        let b = temp("seed");
        let l = Layout::new(&b, &b.join("caches"));
        assert_eq!(l.prepare().unwrap(), 0, "no bundled cache is fine");
        assert!(l.data.is_dir() && l.boot_cache.is_dir());
        std::fs::create_dir_all(&l.bundled_cache).unwrap();
        std::fs::write(l.bundled_cache.join("analysis-x.h2a"), b"bundled").unwrap();
        assert_eq!(l.prepare().unwrap(), 1);
        assert_eq!(
            std::fs::read(l.boot_cache.join("analysis-x.h2a")).unwrap(),
            b"bundled"
        );
        std::fs::write(l.boot_cache.join("analysis-x.h2a"), b"written at run time").unwrap();
        assert_eq!(l.prepare().unwrap(), 0);
        assert_eq!(
            std::fs::read(l.boot_cache.join("analysis-x.h2a")).unwrap(),
            b"written at run time"
        );
        let _ = std::fs::remove_dir_all(&b);
    }
}
