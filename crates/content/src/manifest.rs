//! The map package manifest (docs/maps.md §1), read from a package's
//! `manifest.toml`.
//!
//! ```toml
//! [package]
//! format = 1
//! name = "Lockout Snipers"
//! author = "..."             # optional
//! description = "..."        # optional
//! image = "preview.png"      # optional, PNG/JPEG <= 512 KB
//!
//! [base]
//! map = "lockout"
//! sha256 = "..."             # required unless derived = true
//! derived = false            # optional
//!
//! [[patch]]                  # file layers, applied in order
//! file = "snipers.ppf"
//! sha256 = "..."
//!
//! [[pack]]                   # content packs, applied in memory later
//! file = "spawns.toml"
//! sha256 = "..."
//!
//! [result]                   # optional
//! geometry = "..."
//!
//! [list]                     # optional: 1.0's map list entry of a new map
//! base = "lockout"           # the retail map whose entry gives the rest
//! sort = 300
//! name.ja = "..."            # per-language name; others use [package] name
//! description.ja = "..."     # likewise, from [package] description
//! ```
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{MapId, Sha256};

/// The manifest format this build reads and writes.
pub const FORMAT: u32 = 1;
/// The largest preview image accepted.
pub const MAX_IMAGE_BYTES: u64 = 512 * 1024;
/// The manifest's file name inside a package directory.
pub const MANIFEST_FILE: &str = "manifest.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    pub base: Base,
    /// File layers, applied to the base in order.
    #[serde(default, rename = "patch", skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<Layer>,
    /// Content packs (`map-variant`), applied in memory after the load.
    #[serde(default, rename = "pack", skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultInfo>,
    /// Display data for 1.0's multiplayer map list, for a map whose name the
    /// disc doesn't have (docs/maps.md §4). Not part of the `MapId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListInfo>,
}

/// The languages of the game's map list text, in the order its records hold
/// them (docs/map-pack.md 8.1: nine languages a record).
pub const LANGUAGES: [&str; 9] = ["en", "ja", "de", "fr", "es", "it", "ko", "zh", "pt"];

/// `[list]`: how a new map appears in 1.0's multiplayer map list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListInfo {
    /// The retail map whose own list entry (in `mainmenu.map`) gives the
    /// team counts, the flags and, without an image, the preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The entry's sort order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<i32>,
    /// Per-language names by [`LANGUAGES`] code; a language left out uses
    /// `[package] name`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub name: std::collections::BTreeMap<String, String>,
    /// Per-language descriptions; a language left out uses
    /// `[package] description`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub description: std::collections::BTreeMap<String, String>,
}

/// `[package]`: the format version and display data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A PNG or JPEG in the package directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// `[base]`: the map this package serves as, and the bytes it starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base {
    /// The map name the game loads, `maps\<map>.map`.
    pub map: String,
    /// SHA-256 of the user's disc map. Required unless `derived`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256>,
    /// A user-made map: the first layer is the whole map file, and the disc's
    /// map is not read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub derived: bool,
}

/// One `[[patch]]` or `[[pack]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Path relative to the package directory.
    pub file: String,
    pub sha256: Sha256,
}

/// `[result]`: facts about the patched map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultInfo {
    /// Hash of the structure BSP sections after patching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Sha256>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("manifest format {0} is not supported (this build reads {FORMAT})")]
    Format(u32),
    #[error("package name is empty")]
    EmptyName,
    #[error("base map name {0:?} is not a plain map name")]
    MapName(String),
    #[error("[base] needs sha256 unless derived = true")]
    BaseHash,
    #[error("a derived map needs its map file as the first [[patch]]")]
    DerivedWithoutLayer,
    #[error("file {0:?} is not a relative path inside the package")]
    Escapes(String),
    #[error("image {0:?}: {1}")]
    Image(String, String),
    #[error("[list] {0}")]
    List(String),
}

impl Manifest {
    /// A manifest with only `[base]`: how a retail map is named.
    pub fn retail(map: &str, sha256: Sha256) -> Self {
        Self {
            package: Package {
                format: FORMAT,
                name: map.to_owned(),
                author: None,
                description: None,
                image: None,
            },
            base: Base {
                map: map.to_owned(),
                sha256: Some(sha256),
                derived: false,
            },
            patches: Vec::new(),
            packs: Vec::new(),
            result: None,
            list: None,
        }
    }

    /// Parse and [`validate`](Self::validate) a manifest.
    pub fn from_toml(text: &str) -> Result<Self, ManifestError> {
        let m: Self = toml::from_str(text)?;
        m.validate()?;
        Ok(m)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a manifest always serialises")
    }

    /// Checks that need no files: the format version, the names, the paths.
    /// Hex is checked when parsing.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.package.format != FORMAT {
            return Err(ManifestError::Format(self.package.format));
        }
        if self.package.name.trim().is_empty() {
            return Err(ManifestError::EmptyName);
        }
        if !is_map_name(&self.base.map) {
            return Err(ManifestError::MapName(self.base.map.clone()));
        }
        if self.base.sha256.is_none() && !self.base.derived {
            return Err(ManifestError::BaseHash);
        }
        if self.base.derived && self.patches.is_empty() {
            return Err(ManifestError::DerivedWithoutLayer);
        }
        let files = self.patches.iter().chain(&self.packs).map(|l| &l.file);
        for f in files.chain(&self.package.image) {
            relative(f)?;
        }
        if let Some(list) = &self.list {
            if let Some(base) = list.base.as_deref().filter(|b| !is_map_name(b)) {
                return Err(ManifestError::List(format!(
                    "base {base:?} is not a plain map name"
                )));
            }
            for lang in list.name.keys().chain(list.description.keys()) {
                if !LANGUAGES.contains(&lang.as_str()) {
                    return Err(ManifestError::List(format!(
                        "language {lang:?} is not one of {}",
                        LANGUAGES.join(", ")
                    )));
                }
            }
        }
        if let Some(image) = &self.package.image {
            let lower = image.to_ascii_lowercase();
            if ![".png", ".jpg", ".jpeg"].iter().any(|e| lower.ends_with(e)) {
                return Err(ManifestError::Image(
                    image.clone(),
                    "not .png, .jpg or .jpeg".into(),
                ));
            }
        }
        Ok(())
    }

    /// [`validate`](Self::validate), then the checks that read the package
    /// directory: the image's size and signature.
    pub fn validate_in(&self, dir: &Path) -> Result<(), ManifestError> {
        self.validate()?;
        if let Some(image) = &self.package.image {
            let bad = |why: String| ManifestError::Image(image.clone(), why);
            let path = dir.join(relative(image)?);
            let len = std::fs::metadata(&path)
                .map_err(|e| bad(e.to_string()))?
                .len();
            if len > MAX_IMAGE_BYTES {
                return Err(bad(format!("{len} bytes, over {MAX_IMAGE_BYTES}")));
            }
            let bytes = std::fs::read(&path).map_err(|e| bad(e.to_string()))?;
            if image_signature(&bytes).is_none() {
                return Err(bad("not a PNG or JPEG file".into()));
            }
        }
        Ok(())
    }

    /// This manifest's identity.
    pub fn map_id(&self) -> MapId {
        MapId::of(self)
    }
}

/// `"png"` or `"jpg"` when `bytes` start with that format's signature.
pub fn image_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else {
        None
    }
}

/// A plain map name: ASCII letters, digits, `_` and `-` (no dots, no
/// separators), 1 to 64 bytes.
pub fn is_map_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `file` as a path that stays inside its package directory: relative, with
/// no `..`, root, drive prefix or backslash. Symlinks are checked when the
/// file is opened.
pub fn relative(file: &str) -> Result<PathBuf, ManifestError> {
    let escapes = || ManifestError::Escapes(file.to_owned());
    if file.is_empty() || file.contains('\\') || file.contains('\0') {
        return Err(escapes());
    }
    let path = Path::new(file);
    let mut normal = 0;
    for c in path.components() {
        match c {
            Component::Normal(_) => normal += 1,
            Component::CurDir => {}
            _ => return Err(escapes()),
        }
    }
    if normal == 0 {
        return Err(escapes());
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn sample() -> String {
        format!(
            r#"
[package]
format = 1
name = "Lockout Snipers"
author = "someone"
description = "snipers only"
image = "preview.png"

[base]
map = "lockout"
sha256 = "{H1}"

[[patch]]
file = "snipers.ppf"
sha256 = "{H2}"

[[patch]]
file = "more/fix.ips"
sha256 = "{}"

[[pack]]
file = "spawns.toml"
sha256 = "{H1}"

[result]
geometry = "{H2}"
"#,
            H1.to_uppercase()
        )
    }

    #[test]
    fn round_trips_through_toml() {
        let m = Manifest::from_toml(&sample()).unwrap();
        assert_eq!(m.patches.len(), 2);
        assert_eq!(m.patches[1].file, "more/fix.ips");
        assert_eq!(m.patches[1].sha256.to_string(), H1, "hex is normalised");
        assert_eq!(m.packs[0].file, "spawns.toml");
        assert!(!m.base.derived);
        let text = m.to_toml();
        assert_eq!(Manifest::from_toml(&text).unwrap(), m);
        assert!(!text.contains("derived"), "{text}");
        let bare = Manifest::retail("lockout", H1.parse().unwrap());
        assert_eq!(Manifest::from_toml(&bare.to_toml()).unwrap(), bare);
    }

    fn refused(text: &str) -> String {
        Manifest::from_toml(text).unwrap_err().to_string()
    }

    #[test]
    fn validation() {
        let s = sample();
        assert!(refused(&s.replace("format = 1", "format = 2")).contains("format 2"));
        assert!(refused(&s.replace("Lockout Snipers", " ")).contains("empty"));
        assert!(refused(&s.replace("\"lockout\"", "\"../lockout\"")).contains("map name"));
        assert!(refused(&s.replace("\"lockout\"", "\"lock.out\"")).contains("map name"));
        assert!(refused(&s.replace(H2, "abc")).contains("SHA-256"));
        assert!(refused(&s.replace(H2, &H2.replace('2', "z"))).contains("SHA-256"));
        assert!(refused(&s.replace("snipers.ppf", "../x.ppf")).contains("inside"));
        assert!(refused(&s.replace("snipers.ppf", "/abs.ppf")).contains("inside"));
        assert!(refused(&s.replace("snipers.ppf", "a\\\\b.ppf")).contains("inside"));
        assert!(refused(&s.replace("snipers.ppf", "a/../../b")).contains("inside"));
        assert!(refused(&s.replace("preview.png", "../p.png")).contains("inside"));
        assert!(refused(&s.replace("preview.png", "p.gif")).contains("image"));
        assert!(refused(&s.replace("author", "writer")).contains("unknown field"));
        // No base hash: only for derived maps, which need a layer.
        let nohash = s.replace(
            &format!("map = \"lockout\"\nsha256 = \"{}\"", H1.to_uppercase()),
            "map = \"lockout\"",
        );
        assert!(refused(&nohash).contains("sha256"), "{nohash}");
        let derived = nohash.replace("map = \"lockout\"", "map = \"lockout\"\nderived = true");
        assert!(Manifest::from_toml(&derived).unwrap().base.derived);
        let mut m = Manifest::from_toml(&derived).unwrap();
        m.patches.clear();
        assert!(matches!(
            m.validate(),
            Err(ManifestError::DerivedWithoutLayer)
        ));
        assert!(relative("./a/b").is_ok());
        assert!(relative(".").is_err());
    }

    #[test]
    fn image_size_and_signature() {
        let dir = std::env::temp_dir().join(format!("h2-content-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = Manifest::from_toml(&sample()).unwrap();
        let img = dir.join("preview.png");
        assert!(m.validate_in(&dir).is_err(), "missing image");
        std::fs::write(&img, b"\x89PNG\r\n\x1a\nrest").unwrap();
        m.validate_in(&dir).unwrap();
        std::fs::write(&img, [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        m.validate_in(&dir).unwrap();
        std::fs::write(&img, b"GIF89a").unwrap();
        assert!(
            m.validate_in(&dir)
                .unwrap_err()
                .to_string()
                .contains("PNG or JPEG")
        );
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&img, big).unwrap();
        assert!(
            m.validate_in(&dir)
                .unwrap_err()
                .to_string()
                .contains("over")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
