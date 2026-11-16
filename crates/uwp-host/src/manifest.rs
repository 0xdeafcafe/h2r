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

/// The images the manifest names: (path in the package, width, height).
pub const ASSETS: &[(&str, u32, u32)] = &[
    ("Assets/StoreLogo.png", 50, 50),
    ("Assets/Square44x44Logo.png", 44, 44),
    ("Assets/Square150x150Logo.png", 150, 150),
    ("Assets/Wide310x150Logo.png", 310, 150),
    ("Assets/SplashScreen.png", 620, 300),
];

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
        let asset = |p: &str| p.replace('/', "\\");
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
      <uap:VisualElements DisplayName="{display}" Description="{display}" BackgroundColor="#000000" Square150x150Logo="{square150}" Square44x44Logo="{square44}">
        <uap:DefaultTile Wide310x150Logo="{wide}" />
        <uap:SplashScreen Image="{splash}" BackgroundColor="#000000" />
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
            store_logo = asset(ASSETS[0].0),
            square44 = asset(ASSETS[1].0),
            square150 = asset(ASSETS[2].0),
            wide = asset(ASSETS[3].0),
            splash = asset(ASSETS[4].0),
            min = MIN_VERSION,
            max = MAX_VERSION_TESTED,
            app = APP_ID,
            exe = EXECUTABLE,
        ))
    }
}

/// A `width` x `height` PNG: black, with a centred green square (the
/// package images; no Halo or Xbox artwork).
pub fn placeholder_png(width: u32, height: u32) -> Vec<u8> {
    let side = width.min(height) / 2;
    let (x0, y0) = ((width - side) / 2, (height - side) / 2);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let inside = (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y);
            rgba.extend_from_slice(if inside { &[0x4c, 0x8c, 0x2b, 0xff] } else { &[0, 0, 0, 0xff] });
        }
    }
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, width, height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().expect("png header");
        w.write_image_data(&rgba).expect("png data");
    }
    out
}

/// Writes `AppxManifest.xml` and [`ASSETS`] into the package directory.
pub fn write(dir: &Path, manifest: &Manifest) -> anyhow::Result<()> {
    let text = manifest.xml().map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("AppxManifest.xml"), text)?;
    for (path, w, h) in ASSETS {
        let to = dir.join(path);
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::write(to, placeholder_png(*w, *h))?;
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
        for (path, _, _) in ASSETS {
            assert!(x.contains(&path.replace('/', "\\")), "{path}");
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
        for (path, w, h) in ASSETS {
            let d = png::Decoder::new(std::io::Cursor::new(std::fs::read(dir.join(path)).unwrap()));
            let r = d.read_info().unwrap();
            assert_eq!((r.info().width, r.info().height), (*w, *h), "{path}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
