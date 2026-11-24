//! `MapId`: which map exactly, as a hash of what it is made from.
//! `LevelId`: which assembled level exactly, as a hash of the assets and
//! packages it is assembled from (docs/engine/asset-store.md, "Identity on
//! the network and in films").
use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    Manifest, Sha256,
    hash::{BadHex, parse_hex32, write_hex},
    manifest::FORGE_FORMAT,
    package::PackageId,
};

/// The SHA-256 of a manifest's identity in canonical JSON (RFC 8785).
///
/// The identity is the manifest without its display data (package name,
/// author, description, image) and without what does not change the bytes
/// served: layer file names and `[result]`. What remains is
///
/// ```json
/// {"base":{"derived":false,"map":"lockout","sha256":"<hex>"},
///  "format":1,"pack":["<hex>",...],"patch":["<hex>",...]}
/// ```
///
/// with the map name lowercased (map names match case-insensitively), hex
/// lowercase, `sha256` omitted for a derived map without one, and layers in
/// manifest order. Two manifests have the same `MapId` exactly when they
/// start from the same base and apply the same layers in the same order.
///
/// Format 2 hashes `{"base":"<base hash>","forge":["<document hash>"],"format":2}`,
/// with `"game":<engine index>` after `format` for a document whose objects
/// have game types: the map is built for that engine
/// (docs/game/forge-map-variants.md, "Game types"). Every other document
/// keeps the id it has without the member. See docs/game/forge-design.md
/// for the document hash and expectations.
///
/// Displays, parses and serialises as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MapId(pub [u8; 32]);

#[derive(Serialize)]
struct Identity<'a> {
    format: u32,
    base: IdentityBase,
    patch: Vec<&'a Sha256>,
    pack: Vec<&'a Sha256>,
}
#[derive(Serialize)]
struct IdentityBase {
    map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<Sha256>,
    derived: bool,
}

impl MapId {
    /// The identity of `manifest`.
    pub fn of(manifest: &Manifest) -> Self {
        Self(Sha256::of(Self::canonical(manifest).as_bytes()).0)
    }

    /// The identity of the retail map `name` whose disc file hashes to
    /// `sha256`: a manifest with only `[base]`.
    pub fn retail(name: &str, sha256: Sha256) -> Self {
        Self::of(&Manifest::retail(name, sha256))
    }

    /// The format 2 identity of a materialised Forge document with no
    /// object game types.
    pub fn forge(base: Sha256, document: Sha256) -> Self {
        Self::forge_game(base, document, None)
    }

    /// The format 2 identity of a materialised Forge document, built for
    /// the engine `game` when its objects have game types.
    pub fn forge_game(base: Sha256, document: Sha256, game: Option<u8>) -> Self {
        Self(Sha256::of(canonical_forge(base, document, game).as_bytes()).0)
    }

    /// The canonical JSON that [`MapId::of`] hashes.
    pub fn canonical(manifest: &Manifest) -> String {
        if manifest.package.format == FORGE_FORMAT
            && let (Some(base), [layer]) = (manifest.base.sha256, manifest.forge.as_slice())
        {
            return canonical_forge(base, layer.document, layer.game);
        }
        let id = Identity {
            format: manifest.package.format,
            base: IdentityBase {
                map: manifest.base.map.to_ascii_lowercase(),
                sha256: manifest.base.sha256,
                derived: manifest.base.derived,
            },
            patch: manifest.patches.iter().map(|l| &l.sha256).collect(),
            pack: manifest.packs.iter().map(|l| &l.sha256).collect(),
        };
        let value = serde_json::to_value(&id).expect("identity serialises");
        crate::canonical::canonicalize(&value).expect("identity holds no floats")
    }
}

fn canonical_forge(base: Sha256, document: Sha256, game: Option<u8>) -> String {
    match game {
        None => format!(r#"{{"base":"{base}","forge":["{document}"],"format":2}}"#),
        Some(game) => {
            format!(r#"{{"base":"{base}","forge":["{document}"],"format":2,"game":{game}}}"#)
        }
    }
}

impl FromStr for MapId {
    type Err = BadHex;
    fn from_str(s: &str) -> Result<Self, BadHex> {
        parse_hex32(s).map(Self).ok_or_else(|| BadHex(s.to_owned()))
    }
}
impl fmt::Display for MapId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}
impl fmt::Debug for MapId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MapId({self})")
    }
}
impl Serialize for MapId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for MapId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The format of [`LevelId`]'s identity: `"format"` in the JSON it hashes.
pub const LEVEL_FORMAT: u32 = 1;

/// What a level is assembled from: the identity [`LevelId`] hashes
/// (docs/engine/asset-store.md, "Identity on the network and in films").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelInputs {
    /// The builders' version (`workspace::BUILDERS_VERSION`).
    pub builders: u32,
    /// The level's name (the store's map stem); lowercased in the identity.
    pub level: String,
    /// The SHA-256 of the map file the level's skeleton and extents give
    /// back (the disc's file); `None` for a level built whole.
    pub skeleton: Option<Sha256>,
    /// Assets as (key, asset hash): the key as `<group>:<path>`. Sorted by
    /// key in the identity, whatever their order here.
    pub assets: Vec<(String, Sha256)>,
    /// The packages laid over the level, in load order.
    pub packages: Vec<PackageId>,
}

/// The SHA-256 of a level's [`LevelInputs`] in canonical JSON (RFC 8785):
///
/// ```json
/// {"assets":[["<group>:<path>","<hex>"],...],"builders":7,"format":1,
///  "level":"<name>","packages":["<hex>",...],"skeleton":"<hex>"}
/// ```
///
/// with `skeleton` `null` for a level with none. Two levels with the same
/// id are assembled from the same inputs by the same builders, so they hold
/// the same image bytes. A level with nothing over it has one too; its file
/// is the disc's, named by its retail [`MapId`].
///
/// Displays, parses and serialises as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LevelId(pub [u8; 32]);

impl LevelId {
    /// The identity of `inputs`.
    pub fn of(inputs: &LevelInputs) -> Self {
        Self(Sha256::of(Self::canonical(inputs).as_bytes()).0)
    }

    /// The canonical JSON that [`LevelId::of`] hashes.
    pub fn canonical(inputs: &LevelInputs) -> String {
        let mut assets: Vec<(&str, String)> = inputs
            .assets
            .iter()
            .map(|(k, h)| (k.as_str(), h.to_string()))
            .collect();
        assets.sort();
        let value = serde_json::json!({
            "assets": assets,
            "builders": inputs.builders,
            "format": LEVEL_FORMAT,
            "level": inputs.level.to_ascii_lowercase(),
            "packages": inputs.packages.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            "skeleton": inputs.skeleton.map(|s| s.to_string()),
        });
        crate::canonical::canonicalize(&value).expect("identity holds no floats")
    }
}

impl FromStr for LevelId {
    type Err = BadHex;
    fn from_str(s: &str) -> Result<Self, BadHex> {
        parse_hex32(s).map(Self).ok_or_else(|| BadHex(s.to_owned()))
    }
}
impl fmt::Display for LevelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}
impl fmt::Debug for LevelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LevelId({self})")
    }
}
impl Serialize for LevelId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for LevelId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Layer, ResultInfo};

    fn h(n: u8) -> Sha256 {
        Sha256([n; 32])
    }

    fn modded() -> Manifest {
        let mut m = Manifest::retail("Lockout", h(0x11));
        m.package.name = "Lockout Snipers".into();
        m.patches = vec![
            Layer {
                file: "a.ppf".into(),
                sha256: h(0x22),
            },
            Layer {
                file: "b.ips".into(),
                sha256: h(0x33),
            },
        ];
        m.packs = vec![Layer {
            file: "spawns.toml".into(),
            sha256: h(0x44),
        }];
        m
    }

    #[test]
    fn golden_vectors() {
        // Synthetic hashes only. If these change, every published MapId does.
        let retail = Manifest::retail("lockout", h(0x11));
        assert_eq!(
            MapId::canonical(&retail),
            r#"{"base":{"derived":false,"map":"lockout","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"format":1,"pack":[],"patch":[]}"#
        );
        assert_eq!(MapId::retail("lockout", h(0x11)), MapId::of(&retail));
        assert_eq!(
            MapId::retail("lockout", h(0x11)).to_string(),
            "4831fbbba9b1ec300fa08e77f5b484e98d6a9d16dfdad0c6c4de09dcb5c3ee2b"
        );
        let m = modded();
        assert_eq!(
            MapId::canonical(&m),
            r#"{"base":{"derived":false,"map":"lockout","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"format":1,"pack":["4444444444444444444444444444444444444444444444444444444444444444"],"patch":["2222222222222222222222222222222222222222222222222222222222222222","3333333333333333333333333333333333333333333333333333333333333333"]}"#
        );
        assert_eq!(
            m.map_id().to_string(),
            "dc30bd4bf40d942b9ff7d68b18bcf1a59f16fcceab4dc6598e74ddf99176ab08"
        );
        let mut d = Manifest::retail("mymap", h(0));
        d.base.sha256 = None;
        d.base.derived = true;
        d.patches = vec![Layer {
            file: "mymap.map".into(),
            sha256: h(0x55),
        }];
        assert_eq!(
            MapId::canonical(&d),
            r#"{"base":{"derived":true,"map":"mymap"},"format":1,"pack":[],"patch":["5555555555555555555555555555555555555555555555555555555555555555"]}"#
        );
        assert_eq!(
            d.map_id().to_string(),
            "9fc960abddcb17edd8753bb59f86ab3ed4dd821967f7774d8aaf81de9247601d"
        );
    }

    #[test]
    fn forge_format_2_golden_vector() {
        use crate::manifest::ForgeLayer;

        let mut manifest = Manifest::retail("fixture", h(0x11));
        manifest.package.format = FORGE_FORMAT;
        manifest.forge = vec![ForgeLayer {
            document: h(0x22),
            document_file: "document.h2forge".into(),
            file: "local.map".into(),
            sha256: h(0x33),
            game: None,
        }];
        assert_eq!(Manifest::from_toml(&manifest.to_toml()).unwrap(), manifest);
        assert!(!manifest.to_toml().contains("game"), "no game, no member");
        assert_eq!(
            MapId::canonical(&manifest),
            r#"{"base":"1111111111111111111111111111111111111111111111111111111111111111","forge":["2222222222222222222222222222222222222222222222222222222222222222"],"format":2}"#
        );
        let id = manifest.map_id();
        assert_eq!(id, MapId::forge(h(0x11), h(0x22)));
        assert_eq!(
            id.to_string(),
            "ff9e7c653b78c31eba63f40bd105efee54628c026cbcd89135dab606c8029774"
        );

        manifest.package.name = "Renamed".into();
        manifest.forge[0].file = "rebuilt.map".into();
        manifest.forge[0].sha256 = h(0x44);
        assert_eq!(manifest.map_id(), id);
        assert_ne!(MapId::forge(h(0x99), h(0x22)), id);
        assert_ne!(MapId::forge(h(0x11), h(0x99)), id);

        let mut invalid = manifest.clone();
        invalid.package.format = 1;
        assert!(invalid.validate().is_err());
        invalid = manifest.clone();
        invalid.forge.push(manifest.forge[0].clone());
        assert!(invalid.validate().is_err());
        invalid = manifest.clone();
        invalid.base.derived = true;
        assert!(invalid.validate().is_err());
        invalid = manifest.clone();
        invalid.packs.push(Layer {
            file: "x.toml".into(),
            sha256: h(0x33),
        });
        assert!(invalid.validate().is_err());
        invalid = manifest;
        invalid.forge[0].document_file = "../outside.h2forge".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn forge_game_golden_vector() {
        use crate::manifest::ForgeLayer;

        // A document without object game types keeps its id: `None` is the
        // format 2 vector above, byte for byte.
        assert_eq!(
            MapId::forge_game(h(0x11), h(0x22), None).to_string(),
            "ff9e7c653b78c31eba63f40bd105efee54628c026cbcd89135dab606c8029774"
        );

        let mut manifest = Manifest::retail("fixture", h(0x11));
        manifest.package.format = FORGE_FORMAT;
        manifest.forge = vec![ForgeLayer {
            document: h(0x22),
            document_file: "document.h2forge".into(),
            file: "local.map".into(),
            sha256: h(0x33),
            game: Some(2),
        }];
        manifest.validate().unwrap();
        assert_eq!(Manifest::from_toml(&manifest.to_toml()).unwrap(), manifest);
        assert_eq!(
            MapId::canonical(&manifest),
            r#"{"base":"1111111111111111111111111111111111111111111111111111111111111111","forge":["2222222222222222222222222222222222222222222222222222222222222222"],"format":2,"game":2}"#
        );
        let slayer = manifest.map_id();
        assert_eq!(
            slayer.to_string(),
            "59e9ba39b690f918914eaae9d4671d8282307da42ff50d6dfa8658b20d8a8c14"
        );
        assert_eq!(slayer, MapId::forge_game(h(0x11), h(0x22), Some(2)));
        assert_ne!(slayer, MapId::forge(h(0x11), h(0x22)));
        assert_ne!(slayer, MapId::forge_game(h(0x11), h(0x22), Some(1)));
        // The same document built for another engine is another map.
        let mut ctf = manifest.clone();
        ctf.forge[0].game = Some(1);
        assert_ne!(ctf.map_id(), slayer);

        let mut invalid = manifest;
        invalid.forge[0].game = Some(crate::manifest::MAX_FORGE_GAME + 1);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn display_fields_do_not_change_the_id() {
        let base = modded().map_id();
        let mut m = modded();
        m.package.name = "Other".into();
        m.package.author = Some("x".into());
        m.package.description = Some("y".into());
        m.package.image = Some("p.png".into());
        m.patches[0].file = "renamed.ppf".into();
        m.result = Some(ResultInfo {
            geometry: Some(h(9)),
        });
        m.base.map = "LOCKOUT".into();
        assert_eq!(m.map_id(), base);
    }

    #[test]
    fn content_changes_the_id() {
        let base = modded().map_id();
        let mut variants = Vec::new();
        let mut m = modded();
        m.patches.swap(0, 1);
        variants.push(m);
        let mut m = modded();
        m.patches.pop();
        variants.push(m);
        let mut m = modded();
        m.base.sha256 = Some(h(0x12));
        variants.push(m);
        let mut m = modded();
        m.base.map = "midship".into();
        variants.push(m);
        let mut m = modded();
        m.packs.clear();
        variants.push(m);
        let mut m = modded();
        // A pack and a patch with the same hash are different layers.
        let p = m.packs.pop().unwrap();
        m.patches.push(p);
        variants.push(m);
        for v in variants {
            assert_ne!(v.map_id(), base, "{}", MapId::canonical(&v));
        }
    }

    #[test]
    fn hex_and_serde() {
        let id = modded().map_id();
        let s = id.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(s.parse::<MapId>().unwrap(), id);
        assert_eq!(s.to_uppercase().parse::<MapId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{s}\""));
        assert_eq!(serde_json::from_str::<MapId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<MapId>("\"00\"").is_err());
        assert_eq!(format!("{id:?}"), format!("MapId({s})"));
    }

    #[test]
    fn level_id_golden_vectors() {
        // Synthetic hashes only. If these change, every LevelId does.
        let retail = LevelInputs {
            builders: 7,
            level: "Tiny".into(),
            skeleton: Some(h(0x11)),
            ..Default::default()
        };
        assert_eq!(
            LevelId::canonical(&retail),
            r#"{"assets":[],"builders":7,"format":1,"level":"tiny","packages":[],"skeleton":"1111111111111111111111111111111111111111111111111111111111111111"}"#
        );
        assert_eq!(
            LevelId::of(&retail).to_string(),
            "1f910bb4f1a952db3097ab1806ae1423ca46bcb556ea588a2130345cb8b76bd1"
        );
        let layered = LevelInputs {
            builders: 7,
            level: "tiny".into(),
            skeleton: Some(h(0x11)),
            // Out of order: the identity sorts them by key.
            assets: vec![
                ("jpt!:invented\\dmg\\c".into(), h(0x22)),
                ("bitm:invented\\pic".into(), h(0x33)),
            ],
            packages: vec![PackageId([0x44; 32]), PackageId([0x55; 32])],
        };
        assert_eq!(
            LevelId::of(&layered).to_string(),
            "d1beae54142a827611b3f5d0c532ca30899c41ff27daa66c7a2a3c507994442e"
        );
        let whole = LevelInputs {
            builders: 7,
            level: "stage".into(),
            skeleton: None,
            assets: Vec::new(),
            packages: vec![PackageId([0x44; 32])],
        };
        assert_eq!(
            LevelId::canonical(&whole),
            r#"{"assets":[],"builders":7,"format":1,"level":"stage","packages":["4444444444444444444444444444444444444444444444444444444444444444"],"skeleton":null}"#
        );
        assert_eq!(
            LevelId::of(&whole).to_string(),
            "7aa55e8c6d105391e2924db0a313b4e189bd48b75fafdc1a39ac680464fa7884"
        );
    }

    #[test]
    fn every_input_moves_the_level_id() {
        let base = LevelInputs {
            builders: 7,
            level: "tiny".into(),
            skeleton: Some(h(0x11)),
            assets: vec![("jpt!:a".into(), h(0x22))],
            packages: vec![PackageId([0x44; 32]), PackageId([0x55; 32])],
        };
        let id = LevelId::of(&base);
        let mut v = Vec::new();
        let mut m = base.clone();
        m.builders = 8;
        v.push(m);
        let mut m = base.clone();
        m.level = "other".into();
        v.push(m);
        let mut m = base.clone();
        m.skeleton = None;
        v.push(m);
        let mut m = base.clone();
        m.assets[0].1 = h(0x23);
        v.push(m);
        let mut m = base.clone();
        m.packages.swap(0, 1);
        v.push(m);
        let mut m = base.clone();
        m.packages.pop();
        v.push(m);
        for m in v {
            assert_ne!(LevelId::of(&m), id, "{}", LevelId::canonical(&m));
        }
        let s = id.to_string();
        assert_eq!(s.parse::<LevelId>().unwrap(), id);
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{s}\""));
        assert_eq!(format!("{id:?}"), format!("LevelId({s})"));
    }
}
