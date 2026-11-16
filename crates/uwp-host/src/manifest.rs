//! The package's `AppxManifest.xml` and the images it names
//! (docs/runtime/xbox.md).

use std::path::Path;

/// What the manifest says about the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Identity Name: letters, digits, `.` and `-`, 3 to 50 characters.
    pub name: String,
    /// Identity Publisher: must equal the signing certificate's subject.
    pub publisher: String,
    /// `a.b.c.d`, each part at most 65535. An install over an older one
    /// needs a higher version.
    pub version: String,
    pub display_name: String,
    pub publisher_display_name: String,
}

/// The executable the package starts (the `h2uwp` binary).
pub const EXECUTABLE: &str = "h2uwp.exe";
/// The Application Id: the part of the AUMID after `!`.
pub const APP_ID: &str = "App";
/// The oldest Windows (and Xbox OS) build the app claims to run on.
pub const MIN_VERSION: &str = "10.0.17763.0";
/// The newest SDK it was built against.
pub const MAX_VERSION_TESTED: &str = "10.0.26100.0";

/// One image the manifest names, as committed in platform/xbox/Assets
/// (art/app-icon).
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    /// Path in the package.
    pub path: &'static str,
    pub width: u32,
    pub height: u32,
    pub png: &'static [u8],
}

macro_rules! asset {
    ($name:literal, $w:literal x $h:literal) => {
        Asset {
            path: concat!("Assets/", $name),
            width: $w,
            height: $h,
            png: include_bytes!(concat!("../../../platform/xbox/Assets/", $name)),
        }
    };
}

/// The images the manifest names: the scale-200 exports under plain names.
/// makemsix on a Mac makes no `resources.pri`, and without one Windows
/// cannot choose between `.scale-100`/`.scale-200`/`.targetsize-*`
/// variants, so the manifest names each file directly; the shell scales
/// the 2x image down where it needs less.
pub const ASSETS: &[Asset] = &[
    asset!("StoreLogo.png", 100 x 100),
    asset!("Square44x44Logo.png", 88 x 88),
    asset!("Square150x150Logo.png", 300 x 300),
    asset!("Wide310x150Logo.png", 620 x 300),
    asset!("Square310x310Logo.png", 620 x 620),
    asset!("SplashScreen.png", 1240 x 600),
];

/// The tile and splash background: the icon's space colour.
pub const BACKGROUND: &str = "#061323";

/// The package path of the asset named `name`, with Windows separators.
fn asset_path(name: &str) -> String {
    let a = ASSETS.iter().find(|a| a.path.ends_with(name)).expect("a known asset");
    a.path.replace('/', "\\")
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            name: "h2recomp.Halo2".into(),
            publisher: "CN=h2recomp-dev".into(),
            version: "1.0.0.0".into(),
            display_name: "Halo 2 (h2recomp)".into(),
            publisher_display_name: "h2recomp".into(),
        }
    }
}

/// Escapes text for an XML attribute or element.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Why `version` is not a package version, if it is not.
pub fn check_version(version: &str) -> Result<(), String> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 4 || !parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(format!("version {version:?}: expected four dot-separated numbers"));
    }
    if parts.iter().any(|p| p.parse::<u32>().unwrap() > 65535) {
        return Err(format!("version {version:?}: each part must be at most 65535"));
    }
    Ok(())
}

/// Why `name` is not a package name, if it is not.
pub fn check_name(name: &str) -> Result<(), String> {
    let ok = (3..=50).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if ok { Ok(()) } else { Err(format!("name {name:?}: 3 to 50 letters, digits, '.' or '-'")) }
}

impl Manifest {
    /// The package family name (`<name>_<publisher id>`).
    pub fn family_name(&self) -> String {
        crate::identity::family_name(&self.name, &self.publisher)
    }

    /// The AUMID the Device Portal launches.
    pub fn aumid(&self) -> String {
        crate::identity::aumid(&self.name, &self.publisher, APP_ID)
    }

    /// `AppxManifest.xml`. A UWP app (an app container: no `runFullTrust`)
    /// for any Windows 10 or 11 device and the Xbox, x64 only.
    pub fn xml(&self) -> Result<String, String> {
        check_name(&self.name)?;
        check_version(&self.version)?;
        if !self.publisher.starts_with("CN=") {
            return Err(format!("publisher {:?}: a certificate subject such as CN=h2recomp-dev", self.publisher));
        }
        Ok(format!(
            r##"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  IgnorableNamespaces="uap">
  <Identity Name="{name}" Publisher="{publisher}" Version="{version}" ProcessorArchitecture="x64" />
  <Properties>
    <DisplayName>{display}</DisplayName>
    <PublisherDisplayName>{publisher_display}</PublisherDisplayName>
    <Logo>{store_logo}</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Universal" MinVersion="{min}" MaxVersionTested="{max}" />
    <TargetDeviceFamily Name="Windows.Xbox" MinVersion="{min}" MaxVersionTested="{max}" />
  </Dependencies>
  <Resources>
    <Resource Language="en-us" />
  </Resources>
  <Applications>
    <Application Id="{app}" Executable="{exe}" EntryPoint="h2uwp.App">
      <uap:VisualElements DisplayName="{display}" Description="{display}" BackgroundColor="{background}" Square150x150Logo="{square150}" Square44x44Logo="{square44}">
        <uap:DefaultTile Wide310x150Logo="{wide}" Square310x310Logo="{square310}" />
        <uap:SplashScreen Image="{splash}" BackgroundColor="{background}" />
      </uap:VisualElements>
    </Application>
  </Applications>
  <Capabilities>
    <Capability Name="internetClient" />
  </Capabilities>
</Package>
"##,
            name = xml(&self.name),
            publisher = xml(&self.publisher),
            version = xml(&self.version),
            display = xml(&self.display_name),
            publisher_display = xml(&self.publisher_display_name),
            store_logo = asset_path("StoreLogo.png"),
            square44 = asset_path("Square44x44Logo.png"),
            square150 = asset_path("Square150x150Logo.png"),
            wide = asset_path("Wide310x150Logo.png"),
            square310 = asset_path("Square310x310Logo.png"),
            splash = asset_path("SplashScreen.png"),
            background = BACKGROUND,
            min = MIN_VERSION,
            max = MAX_VERSION_TESTED,
            app = APP_ID,
            exe = EXECUTABLE,
        ))
    }
}

/// Writes `AppxManifest.xml` and [`ASSETS`] into the package directory.
pub fn write(dir: &Path, manifest: &Manifest) -> anyhow::Result<()> {
    let text = manifest.xml().map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("AppxManifest.xml"), text)?;
    for a in ASSETS {
        let to = dir.join(a.path);
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::write(to, a.png)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_targets_the_xbox_and_every_windows_device() {
        let x = Manifest::default().xml().unwrap();
        assert!(x.contains(r#"<TargetDeviceFamily Name="Windows.Xbox" MinVersion="10.0.17763.0""#), "{x}");
        assert!(x.contains(r#"<TargetDeviceFamily Name="Windows.Universal""#));
        assert!(x.contains(
            r#"<Identity Name="h2recomp.Halo2" Publisher="CN=h2recomp-dev" Version="1.0.0.0" ProcessorArchitecture="x64" />"#
        ));
        assert!(x.contains(r#"<Application Id="App" Executable="h2uwp.exe" EntryPoint="h2uwp.App">"#));
        // An app container: no full trust, no restricted capabilities.
        assert!(!x.contains("runFullTrust") && !x.contains("rescap"));
        // Every image it names is one `write` makes, with Windows separators.
        for a in ASSETS {
            assert!(x.contains(&a.path.replace('/', "\\")), "{}", a.path);
        }
        assert_eq!(x.matches("Assets\\").count(), ASSETS.len());
    }

    #[test]
    fn identity_text_is_escaped_and_checked() {
        let m = Manifest { display_name: "A & B <c>".into(), ..Manifest::default() };
        assert!(m.xml().unwrap().contains("<DisplayName>A &amp; B &lt;c&gt;</DisplayName>"));
        for bad in ["1.0.0", "1.0.0.65536", "1.0.0.x", "1..0.0"] {
            let m = Manifest { version: bad.into(), ..Manifest::default() };
            assert!(m.xml().is_err(), "{bad}");
        }
        assert!(check_version("1.0.26320.1439").is_ok());
        assert!(Manifest { name: "h2 recomp".into(), ..Manifest::default() }.xml().is_err());
        assert!(Manifest { name: "ab".into(), ..Manifest::default() }.xml().is_err());
        assert!(Manifest { publisher: "h2recomp".into(), ..Manifest::default() }.xml().is_err());
    }

    #[test]
    fn the_aumid_is_the_family_name_and_the_app_id() {
        let m = Manifest::default();
        assert_eq!(m.aumid(), format!("{}!App", m.family_name()));
        assert!(m.family_name().starts_with("h2recomp.Halo2_"));
    }

    #[test]
    fn write_makes_the_manifest_and_decodable_images() {
        let dir = std::env::var_os("TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("h2-uwp-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, &Manifest::default()).unwrap();
        assert!(std::fs::read_to_string(dir.join("AppxManifest.xml")).unwrap().contains("Windows.Xbox"));
        for a in ASSETS {
            assert_eq!(std::fs::read(dir.join(a.path)).unwrap(), a.png, "{}", a.path);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every image is a PNG of the size the table says, the size each
    /// slot's scale-200 image has.
    #[test]
    fn the_images_are_the_icon_art_at_2x() {
        for a in ASSETS {
            let r = png::Decoder::new(std::io::Cursor::new(a.png)).read_info().unwrap();
            assert_eq!((r.info().width, r.info().height), (a.width, a.height), "{}", a.path);
        }
        let size = |n: &str| ASSETS.iter().find(|a| a.path.ends_with(n)).map(|a| (a.width, a.height));
        assert_eq!(size("/Square150x150Logo.png"), Some((300, 300)));
        assert_eq!(size("/Square44x44Logo.png"), Some((88, 88)));
        assert_eq!(size("/Wide310x150Logo.png"), Some((620, 300)));
        assert_eq!(size("/Square310x310Logo.png"), Some((620, 620)));
        assert_eq!(size("/StoreLogo.png"), Some((100, 100)));
        assert_eq!(size("/SplashScreen.png"), Some((1240, 600)));
    }

    /// platform/xbox/Assets holds exactly the images the manifest names.
    #[test]
    fn the_committed_images_are_the_ones_the_manifest_names() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/xbox/Assets");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        on_disk.sort();
        let mut named: Vec<String> = ASSETS.iter().map(|a| a.path.trim_start_matches("Assets/").to_string()).collect();
        named.sort();
        assert_eq!(on_disk, named);
    }

    #[test]
    fn tiles_and_splash_use_the_icon_background() {
        let x = Manifest::default().xml().unwrap();
        assert_eq!(x.matches(r##"BackgroundColor="#061323""##).count(), 2, "{x}");
        assert!(x.contains(r#"Square310x310Logo="Assets\Square310x310Logo.png""#));
        assert!(x.contains(r#"<Logo>Assets\StoreLogo.png</Logo>"#));
    }
}
