//! `PackageId` and `package.lock`.
use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use super::manifest::{Item, Manifest, Maps, PACKAGE_FORMAT};
use crate::{
    Sha256,
    hash::{BadHex, parse_hex32, write_hex},
};

/// The SHA-256 of a manifest's identity in canonical JSON (RFC 8785).
///
/// The identity is the manifest without its display fields (`name`,
/// `description`, `image`) and without `[credit]`, and with each item's
/// `source` replaced by the SHA-256 of that file from the [`Lock`].
/// Additional `inputs` are stored as a mapping from path to hash:
///
/// ```json
/// {"after":[..],"features":[..],"format":1,"id":"pogo",
///  "item":[{"kind":"model","maps":"multiplayer","source":"<hex>","tag":".."}],
///  "requires":[..]}
/// ```
///
/// `requires`, `after` and `features` are sets: sorted and deduplicated.
/// Items keep their order (a later one wins), and map names are lowercased
/// and sorted. Two manifests have the same id exactly when they say the same
/// thing about the same file contents.
///
/// Displays, parses and serialises as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId(pub [u8; 32]);

/// Why an id cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("package.lock has no hash for {0:?}")]
    MissingHash(String),
    #[error("package.lock: {0}")]
    Toml(String),
    #[error("package.lock format {0} is not supported (this build reads {PACKAGE_FORMAT})")]
    Format(u32),
}

/// `package.lock`: the SHA-256 of every file the manifest names. The build
/// writes it beside the manifest; it is what is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub format: u32,
    /// Path inside the package to the file's hash.
    #[serde(default)]
    pub files: BTreeMap<String, Sha256>,
}

impl Lock {
    pub fn new(files: BTreeMap<String, Sha256>) -> Self {
        Self {
            format: PACKAGE_FORMAT,
            files,
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, IdError> {
        let l: Self = toml::from_str(text).map_err(|e| IdError::Toml(e.to_string()))?;
        if l.format != PACKAGE_FORMAT {
            return Err(IdError::Format(l.format));
        }
        Ok(l)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a lock serialises")
    }

    /// The first source of `manifest` that the lock has no hash for.
    pub fn missing<'a>(&self, manifest: &'a Manifest) -> Option<&'a str> {
        manifest
            .items
            .iter()
            .flat_map(Item::files)
            .find(|s| !self.files.contains_key(*s))
    }
}

impl PackageId {
    /// The identity of `manifest` over the file hashes in `lock`.
    pub fn of(manifest: &Manifest, lock: &Lock) -> Result<Self, IdError> {
        Ok(Self(
            Sha256::of(Self::canonical(manifest, lock)?.as_bytes()).0,
        ))
    }

    /// The canonical JSON that [`PackageId::of`] hashes.
    pub fn canonical(manifest: &Manifest, lock: &Lock) -> Result<String, IdError> {
        let p = &manifest.package;
        let set = |v: &[String]| {
            let mut v = v.to_vec();
            v.sort();
            v.dedup();
            v
        };
        let mut items = Vec::new();
        for item in &manifest.items {
            let mut o = serde_json::Map::new();
            o.insert(
                "kind".into(),
                serde_json::to_value(item.kind).expect("kind"),
            );
            if let Some(source) = &item.source {
                let hash = lock
                    .files
                    .get(source)
                    .ok_or_else(|| IdError::MissingHash(source.clone()))?;
                o.insert("source".into(), Value::String(hash.to_string()));
            }
            if !item.inputs.is_empty() {
                let hashes = item
                    .inputs
                    .iter()
                    .map(|path| {
                        lock.files
                            .get(path)
                            .map(|hash| (path, hash.to_string()))
                            .ok_or_else(|| IdError::MissingHash(path.clone()))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;
                o.insert("inputs".into(), json!(hashes));
            }
            if let Some(tag) = &item.tag {
                o.insert("tag".into(), Value::String(tag.clone()));
            }
            let maps = match &item.maps {
                Maps::Scope(s) => Value::String(s.word().to_owned()),
                Maps::Names(n) => {
                    let mut n: Vec<String> = n.iter().map(|n| n.to_ascii_lowercase()).collect();
                    n.sort();
                    n.dedup();
                    json!(n)
                }
            };
            o.insert("maps".into(), maps);
            items.push(Value::Object(o));
        }
        let value = json!({
            "format": p.format,
            "id": p.id,
            "requires": set(&p.requires),
            "after": set(&p.after),
            "features": set(&p.features),
            "item": items,
        });
        Ok(crate::canonical::canonicalize(&value).expect("identity holds no floats"))
    }
}

impl FromStr for PackageId {
    type Err = BadHex;
    fn from_str(s: &str) -> Result<Self, BadHex> {
        parse_hex32(s).map(Self).ok_or_else(|| BadHex(s.to_owned()))
    }
}
impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}
impl fmt::Debug for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PackageId({self})")
    }
}
impl Serialize for PackageId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for PackageId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
