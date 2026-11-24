//! `package.toml`: the manifest of a package.
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The manifest format this build reads.
pub const PACKAGE_FORMAT: u32 = 1;
/// The manifest's file name inside a package directory.
pub const MANIFEST_FILE: &str = "package.toml";
/// The longest package id.
pub const MAX_ID: usize = 64;

/// A package manifest. Unknown keys are refused at every level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    /// Who made it: display data, outside the [`PackageId`](super::PackageId).
    #[serde(default)]
    pub credit: Credit,
    /// The `[[item]]` tables, in order. Later items win over earlier ones.
    #[serde(default, rename = "item")]
    pub items: Vec<Item>,
}

/// The `[package]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub format: u32,
    /// Lowercase letters, digits and `-`; unique in a set.
    pub id: String,
    /// Display only.
    pub name: String,
    /// Display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A preview image in the package. Display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Packages that must be in the set and load first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Packages that load first when they are present; order only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// Game features the content needs (`pogo`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// The `[credit]` table: the original authors, as the release credits them.
/// Display data. (`live_proto::maps::Credit` is the same shape; live-proto
/// depends on this crate, so this one cannot name it.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credit {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One `[[item]]`: a source and the builder that turns it into content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub kind: Kind,
    /// A file in the package. Every kind but `kit` names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The tag path the item adds, for the kinds that add one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Where it is added. Omitted is `all`.
    #[serde(default)]
    pub maps: Maps,
}

/// The builder an item names: the closed list of "Contents".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Tag,
    Edits,
    Model,
    Bitmap,
    Animation,
    Sound,
    Screens,
    ScreenEdits,
    Hud,
    Strings,
    Rules,
    Variant,
    Playlist,
    Map,
    MapPatch,
    Convert,
    Data,
    Kit,
}

/// Who needs an item: the builder's choice, not the author's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemScope {
    /// Changes what the simulation reads; every peer needs it.
    Sim,
    /// Changes only what one player sees and hears.
    Client,
    /// Read by the server.
    Live,
    /// Whatever the named feature or kit says.
    PerFeature,
}

impl Kind {
    pub fn scope(self) -> ItemScope {
        use Kind::*;
        match self {
            Tag | Edits | Model | Animation | Rules | Variant | Map | MapPatch | Convert => {
                ItemScope::Sim
            }
            Bitmap | Sound | Screens | ScreenEdits | Hud | Strings => ItemScope::Client,
            Playlist => ItemScope::Live,
            Data | Kit => ItemScope::PerFeature,
        }
    }
}

/// A `maps` value: a scope word or a list of map names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Maps {
    Scope(MapScope),
    Names(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapScope {
    Multiplayer,
    Campaign,
    Mainmenu,
    Shared,
    All,
}

impl MapScope {
    pub fn word(self) -> &'static str {
        match self {
            Self::Multiplayer => "multiplayer",
            Self::Campaign => "campaign",
            Self::Mainmenu => "mainmenu",
            Self::Shared => "shared",
            Self::All => "all",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        [
            Self::Multiplayer,
            Self::Campaign,
            Self::Mainmenu,
            Self::Shared,
            Self::All,
        ]
        .into_iter()
        .find(|m| m.word() == s)
    }
}

impl Default for Maps {
    fn default() -> Self {
        Self::Scope(MapScope::All)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum MapsRaw {
    One(String),
    Many(Vec<String>),
}

impl Serialize for Maps {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let raw = match self {
            Self::Scope(m) => MapsRaw::One(m.word().to_owned()),
            Self::Names(n) => MapsRaw::Many(n.clone()),
        };
        raw.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Maps {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match MapsRaw::deserialize(d)? {
            MapsRaw::One(w) => MapScope::parse(&w).map(Self::Scope).ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "maps {w:?} is not multiplayer, campaign, mainmenu, shared, all or a list of map names"
                ))
            }),
            MapsRaw::Many(n) => Ok(Self::Names(n)),
        }
    }
}

/// Why a manifest is refused. Each names what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Bad TOML, an unknown key (named in the text) or a wrong type.
    #[error("package.toml: {0}")]
    Toml(String),
    #[error("package format {0} is not supported (this build reads {PACKAGE_FORMAT})")]
    Format(u32),
    #[error("package id {0:?} is not lowercase letters, digits and '-' (1 to {MAX_ID})")]
    Id(String),
    #[error("package name is empty")]
    EmptyName,
    #[error("package {package:?}: {field} names {name:?}, which is not a package id")]
    BadReference {
        package: String,
        field: &'static str,
        name: String,
    },
    #[error("package {package:?}: {field} names {name:?} twice")]
    DuplicateReference {
        package: String,
        field: &'static str,
        name: String,
    },
    #[error("package {0:?} names itself in requires or after")]
    SelfReference(String),
    #[error("package {package:?}: {name:?} is in both requires and after")]
    BothRequiresAndAfter { package: String, name: String },
    #[error("package {package:?}: feature {name:?} is not a feature name")]
    Feature { package: String, name: String },
    #[error("{0:?} is not a relative path inside the package")]
    Escapes(String),
    #[error("item {index} ({kind:?}) needs a source")]
    NoSource { index: usize, kind: Kind },
    #[error("item {index}: tag path {tag:?} is empty or escapes")]
    BadTag { index: usize, tag: String },
    #[error("item {index}: maps lists no map, or {name:?} is not a map name")]
    BadMaps { index: usize, name: String },
    #[error(
        "{path:?}: a package never holds {what} (cache maps, tag bundles and disc bytes stay out)"
    )]
    Forbidden { path: String, what: &'static str },
}

/// Lowercase letters, digits and `-`; no `-` at either end; 1 to 64 long.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn feature_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn map_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// What a file name says it is, when a package may not hold it.
fn forbidden(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    match ext.as_str() {
        "map" => Some("a cache map"),
        "h2tag" => Some("a .h2tag bundle"),
        "xbe" | "iso" | "xiso" => Some("disc files"),
        _ => None,
    }
}

impl Manifest {
    /// Parse and [`validate`](Self::validate) a `package.toml`.
    pub fn from_toml(text: &str) -> Result<Self, ManifestError> {
        let m: Self = toml::from_str(text).map_err(|e| ManifestError::Toml(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    /// The manifest as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a manifest serialises")
    }

    /// Whether any item changes what the simulation reads.
    pub fn has_sim_item(&self) -> bool {
        self.items.iter().any(|i| i.kind.scope() == ItemScope::Sim)
    }

    /// Every file the manifest names: item sources, then the image.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.items
            .iter()
            .filter_map(|i| i.source.as_deref())
            .chain(self.package.image.as_deref())
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        let p = &self.package;
        if p.format != PACKAGE_FORMAT {
            return Err(ManifestError::Format(p.format));
        }
        if !valid_slug(&p.id) {
            return Err(ManifestError::Id(p.id.clone()));
        }
        if p.name.trim().is_empty() {
            return Err(ManifestError::EmptyName);
        }
        for (field, list) in [("requires", &p.requires), ("after", &p.after)] {
            for (i, name) in list.iter().enumerate() {
                let bad = |f| ManifestError::BadReference {
                    package: p.id.clone(),
                    field: f,
                    name: name.clone(),
                };
                if !valid_slug(name) {
                    return Err(bad(field));
                }
                if *name == p.id {
                    return Err(ManifestError::SelfReference(p.id.clone()));
                }
                if list[..i].contains(name) {
                    return Err(ManifestError::DuplicateReference {
                        package: p.id.clone(),
                        field,
                        name: name.clone(),
                    });
                }
            }
        }
        if let Some(name) = p.after.iter().find(|a| p.requires.contains(a)) {
            return Err(ManifestError::BothRequiresAndAfter {
                package: p.id.clone(),
                name: name.clone(),
            });
        }
        if let Some(name) = p.features.iter().find(|f| !feature_ok(f)) {
            return Err(ManifestError::Feature {
                package: p.id.clone(),
                name: name.clone(),
            });
        }
        for file in self.files() {
            crate::manifest::relative(file).map_err(|_| ManifestError::Escapes(file.to_owned()))?;
            if let Some(what) = forbidden(file) {
                return Err(ManifestError::Forbidden {
                    path: file.to_owned(),
                    what,
                });
            }
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.source.is_none() && item.kind != Kind::Kit {
                return Err(ManifestError::NoSource {
                    index,
                    kind: item.kind,
                });
            }
            if let Some(tag) = &item.tag
                && (tag.trim().is_empty()
                    || tag.contains("..")
                    || tag.starts_with(['\\', '/'])
                    || tag.contains('\0'))
            {
                return Err(ManifestError::BadTag {
                    index,
                    tag: tag.clone(),
                });
            }
            if let Maps::Names(names) = &item.maps {
                if names.is_empty() {
                    return Err(ManifestError::BadMaps {
                        index,
                        name: String::new(),
                    });
                }
                if let Some(name) = names.iter().find(|n| !map_name_ok(n)) {
                    return Err(ManifestError::BadMaps {
                        index,
                        name: name.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}
