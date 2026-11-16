//! Where the tvOS, iOS and Xbox (UWP) apps find and keep things.
//!
//! The bundle is read-only: the trimmed disc (`disc/`, a directory of the
//! disc's files, served by `vfs::DirDiscBackend`), the generated module (an
//! embedded framework, with its build stamp beside the binary) and the
//! boot cache's analysis, so the first launch need not analyse the XBE.
//! Caches (the boot cache, built pipelines) go under the app's Caches
//! directory. The hard disk, saves and settings go under Documents where
//! the platform has one that lasts (iOS, docs/runtime/ios.md); tvOS has none, so
//! there they go in Caches too, which tvOS may purge (docs/runtime/tvos.md).

use std::path::{Path, PathBuf};

/// The generated module's framework inside the bundle (its `-install_name`
/// is `@rpath/h2game.framework/h2game`, emit::compile::Arch::install_name).
pub const FRAMEWORK: &str = "Frameworks/h2game.framework/h2game";

/// The generated module in the Xbox app's package (crates/uwp-host,
/// docs/runtime/xbox.md): a DLL at the package root, its stamp beside it as
/// `h2game.dll.stamp`.
pub const DLL: &str = "h2game.dll";

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

/// The device an app bundle is packed for (`examples/pack.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Tvos,
    Ios,
    /// The UWP app on an Xbox in Dev Mode (crates/uwp-host): the module is
    /// [`DLL`], cross-built with `H2_EMIT_TARGET=windows`.
    Xbox,
}

impl Platform {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tvos" => Some(Platform::Tvos),
            "ios" => Some(Platform::Ios),
            "xbox" => Some(Platform::Xbox),
            _ => None,
        }
    }

    /// `H2_EMIT_TARGET` for the module (emit::compile::Arch::target_from).
    pub fn emit_target(self) -> &'static str {
        match self {
            Platform::Tvos => "tvos",
            Platform::Ios => "ios",
            Platform::Xbox => "windows",
        }
    }

    /// Whether the module goes in an Apple framework (with
    /// [`Platform::framework_plist`]) or, for Xbox, is [`DLL`] at the
    /// package root.
    pub fn uses_framework(self) -> bool {
        self != Platform::Xbox
    }

    /// The module framework's `Info.plist`. Its minimum OS matches
    /// emit's `TVOS_DEPLOYMENT_TARGET` / `IOS_DEPLOYMENT_TARGET`. `None`
    /// for Xbox, which has no framework.
    pub fn framework_plist(self) -> Option<String> {
        let (platform, min) = match self {
            Platform::Tvos => ("AppleTVOS", "17.0"),
            Platform::Ios => ("iPhoneOS", "17.0"),
            Platform::Xbox => return None,
        };
        Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>h2game</string>
	<key>CFBundleIdentifier</key>
	<string>red.forbes.h2recomp.h2game</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>h2game</string>
	<key>CFBundlePackageType</key>
	<string>FMWK</string>
	<key>CFBundleShortVersionString</key>
	<string>0.1</string>
	<key>CFBundleVersion</key>
	<string>1</string>
	<key>CFBundleSupportedPlatforms</key>
	<array>
		<string>{platform}</string>
	</array>
	<key>MinimumOSVersion</key>
	<string>{min}</string>
</dict>
</plist>
"#
        ))
    }
}

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
    /// app's Caches directory and `documents` its Documents directory, when
    /// that persists (iOS; `None` on tvOS, where saves live in Caches).
    pub fn new(bundle: &Path, caches: &Path, documents: Option<&Path>) -> Self {
        Self {
            disc: bundle.join("disc"),
            module: bundle.join(FRAMEWORK),
            bundled_cache: bundle.join("bootcache"),
            data: documents.unwrap_or(caches).join("h2data"),
            boot_cache: caches.join("h2boot-cache"),
        }
    }

    /// The Xbox app's places: `package` is the installed package (read-only:
    /// `Package.Current.InstalledLocation`), `local` the app's LocalFolder
    /// (`ApplicationData.Current.LocalFolder`), which lasts across launches
    /// and updates, so the hard disk, saves and both caches go there.
    pub fn for_package(package: &Path, local: &Path) -> Self {
        Self {
            disc: package.join("disc"),
            module: package.join(DLL),
            bundled_cache: package.join("bootcache"),
            data: local.join("h2data"),
            boot_cache: local.join("h2boot-cache"),
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
        let p = base.join(format!("h2-host-layout-{name}-{}", std::process::id()));
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
    fn each_platform_packs_its_own_target_and_framework_plist() {
        assert_eq!(Platform::parse("tvos"), Some(Platform::Tvos));
        assert_eq!(Platform::parse("ios"), Some(Platform::Ios));
        assert_eq!(Platform::parse("xbox"), Some(Platform::Xbox));
        assert_eq!(Platform::parse("macos"), None);
        assert_eq!(Platform::Xbox.emit_target(), "windows");
        assert!(!Platform::Xbox.uses_framework());
        assert!(Platform::Tvos.uses_framework() && Platform::Ios.uses_framework());
        assert_eq!(Platform::Tvos.emit_target(), "tvos");
        assert_eq!(Platform::Ios.emit_target(), "ios");
        let tv = Platform::Tvos.framework_plist().unwrap();
        let ios = Platform::Ios.framework_plist().unwrap();
        assert_eq!(Platform::Xbox.framework_plist(), None);
        assert!(tv.contains("<string>AppleTVOS</string>") && !tv.contains("iPhoneOS"));
        assert!(ios.contains("<string>iPhoneOS</string>") && !ios.contains("AppleTVOS"));
        for p in [&tv, &ios] {
            assert!(p.contains("<string>FMWK</string>") && p.contains("<string>h2game</string>"));
            assert!(p.contains("<key>MinimumOSVersion</key>\n\t<string>17.0</string>"));
        }
    }

    #[test]
    fn paths_are_bundle_relative_and_writes_go_to_caches() {
        let l = Layout::new(Path::new("/b/H2.app"), Path::new("/c/Caches"), None);
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
    fn the_xbox_package_holds_the_dll_and_local_state_holds_the_writes() {
        let l = Layout::for_package(Path::new("/pkg"), Path::new("/local"));
        assert_eq!(l.disc, Path::new("/pkg/disc"));
        assert_eq!(l.module, Path::new("/pkg/h2game.dll"));
        assert_eq!(l.bundled_cache, Path::new("/pkg/bootcache"));
        assert_eq!(l.data, Path::new("/local/h2data"));
        assert_eq!(l.boot_cache, Path::new("/local/h2boot-cache"));
        let b = temp("xbox");
        let l = Layout::for_package(&b, &b.join("local"));
        std::fs::create_dir_all(b.join("disc")).unwrap();
        std::fs::write(b.join("h2game.dll"), b"").unwrap();
        std::fs::write(b.join("h2game.dll.stamp"), b"x").unwrap();
        assert!(l.missing().is_empty());
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn with_documents_the_hard_disk_persists_and_caches_stay_in_caches() {
        let l = Layout::new(
            Path::new("/b/H2.app"),
            Path::new("/c/Caches"),
            Some(Path::new("/d/Documents")),
        );
        assert_eq!(l.data, Path::new("/d/Documents/h2data"));
        assert_eq!(l.boot_cache, Path::new("/c/Caches/h2boot-cache"));
        assert_eq!(l.disc, Path::new("/b/H2.app/disc"));
    }

    #[test]
    fn a_whole_bundle_has_nothing_missing() {
        let b = temp("bundle");
        let l = Layout::new(&b, &b.join("caches"), None);
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
        let l = Layout::new(&b, &b.join("caches"), None);
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
