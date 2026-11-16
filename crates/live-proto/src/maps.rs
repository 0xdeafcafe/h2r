//! Custom and modded maps over Live (docs/game/maps.md §5): the content store's
//! wire types and events, and which maps a hopper allows.
//!
//! A map is named exactly by its [`MapId`] (a base plus layers); sessions
//! and games carry it beside the map's name. The server keeps the layers
//! (patches, packs, user-made maps, preview images) by SHA-256 and the
//! packages' manifests by `MapId`; never retail bytes.
use serde::{Deserialize, Serialize};

pub use content::{Manifest, MapId, Sha256};

/// `ContentStored`: a blob's bytes are kept (server-signed, on
/// [`content_stream`]). Data: [`ContentStored`].
pub const CONTENT_STORED: &str = "ContentStored";
/// `MapPublished`: a package's manifest, signed by its author's own key on
/// [`package_stream`]. Data: [`MapDocument`].
pub const MAP_PUBLISHED: &str = "MapPublished";
/// `ContentModerated`: an admin hid, deleted or blocked a blob or a
/// package, or undid it (server-signed, on the target's stream). Data:
/// [`Moderated`].
pub const CONTENT_MODERATED: &str = "ContentModerated";
/// `ContentReported`: an account flagged a blob or package for the admins
/// (server-signed, on the target's stream). Data: [`Reported`].
pub const CONTENT_REPORTED: &str = "ContentReported";
/// `ContentUploadsSet`: an admin switched an account's uploads on or off
/// (server-signed, on the account's stream). Data: [`UploadsSet`].
pub const CONTENT_UPLOADS_SET: &str = "ContentUploadsSet";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 6] = [
    CONTENT_STORED,
    MAP_PUBLISHED,
    CONTENT_MODERATED,
    CONTENT_REPORTED,
    CONTENT_UPLOADS_SET,
    HOPPER_PINS_SET,
];

/// The stream a blob's events are on.
pub fn content_stream(sha256: &Sha256) -> String {
    format!("content-{sha256}")
}

/// The stream a package's events are on. Not `map-`: that prefix is the
/// map outlines', keyed by map name.
pub fn package_stream(id: &MapId) -> String {
    format!("package-{id}")
}

/// Whether `bytes` is a whole map file (it starts with the cache file
/// header's `head` tag, stored little-endian), which may be larger than a
/// patch.
pub fn is_whole_map(bytes: &[u8]) -> bool {
    bytes.starts_with(b"daeh")
}

/// Whether `bytes` is a PNG or JPEG by its signature.
pub fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xff, 0xd8, 0xff])
}

/// The image's content type, by signature.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// `ContentStored` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStored {
    pub sha256: Sha256,
    pub size: u64,
    /// The account that uploaded it first; its quota pays for it.
    pub uploader: crate::Xuid,
}

/// The document a package's author signs (`MapPublished` data): the
/// manifest as JSON (the TOML's tables and keys, `[[patch]]` as `patch`)
/// and the SHA-256 of its preview image, uploaded like any layer, when the
/// manifest names one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDocument {
    pub manifest: Manifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Sha256>,
}

/// What an admin did to a blob or package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Served as 410 Gone, kept; undone by `unhide`.
    Hide,
    Unhide,
    /// The bytes (or manifest) are removed; a fresh upload may bring them
    /// back.
    Delete,
    /// Deleted and refused from now on, by hash (or `MapId`).
    Block,
    Unblock,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.into())).ok()
    }
}

/// Whether a moderation target is a blob or a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Content,
    Map,
}

/// A blob's or package's standing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    #[default]
    Ok,
    Hidden,
    Deleted,
    Blocked,
}

impl Standing {
    /// The standing after `action`. Unhide only lifts a hide and unblock
    /// only a block; a block is lifted to deleted (the bytes are gone).
    pub fn after(self, action: Action) -> Standing {
        match (self, action) {
            (_, Action::Block) => Standing::Blocked,
            (Standing::Blocked, Action::Unblock) => Standing::Deleted,
            (Standing::Blocked, _) => Standing::Blocked,
            (_, Action::Delete) => Standing::Deleted,
            (Standing::Deleted, _) => Standing::Deleted,
            (_, Action::Hide) => Standing::Hidden,
            (Standing::Hidden, Action::Unhide) => Standing::Ok,
            (s, _) => s,
        }
    }

    /// Served (200) rather than 410 Gone.
    pub fn is_ok(self) -> bool {
        self == Standing::Ok
    }
}

/// `ContentModerated` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moderated {
    pub target: Target,
    /// The SHA-256 or `MapId`, hex.
    pub id: String,
    pub action: Action,
    /// The admin's account (None: the operator's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::Xuid>,
}

/// `ContentReported` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    pub target: Target,
    pub id: String,
    pub by: crate::Xuid,
    pub reason: String,
}

/// The longest report reason kept.
pub const MAX_REASON: usize = 500;

/// `ContentUploadsSet` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadsSet {
    pub xuid: crate::Xuid,
    pub allowed: bool,
}

/// An account as the content API names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    pub xuid: crate::Xuid,
    pub gamertag: String,
}

/// A package's base, as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseInfo {
    pub map: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256>,
    pub derived: bool,
}

/// One layer of a listed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerInfo {
    pub sha256: Sha256,
    pub size: u64,
    /// `patch` or `pack`.
    pub kind: String,
}

/// An entry of `GET /v1/maps`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapSummary {
    pub map_id: MapId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub author: Who,
    pub base: BaseInfo,
    pub layers: Vec<LayerInfo>,
    /// The layers' bytes: what a player without them downloads.
    pub size: u64,
    /// The preview image's SHA-256 (served at `/v1/maps/{id}/image`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Sha256>,
    /// `[result] geometry`: set when the mod says its geometry changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Sha256>,
    pub published_at: String,
}

/// `GET /v1/maps/{map_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapDetail {
    #[serde(flatten)]
    pub summary: MapSummary,
    pub manifest: Manifest,
}

/// `GET /v1/maps/{map_id}`'s whole answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapAnswer {
    pub map: MapDetail,
    /// Layers the server does not hold (deleted or never uploaded).
    pub missing: Vec<Sha256>,
    pub position: u64,
}

/// `HopperPinsSet`: the maps a matchmaking hopper (playlist) allows beside
/// retail ones, server-signed on [`hopper_stream`].
pub const HOPPER_PINS_SET: &str = "HopperPinsSet";

/// The stream a hopper's pins are kept on.
pub fn hopper_stream(hopper_id: u32) -> String {
    format!("hopper-{hopper_id}")
}

/// A hopper entry's pinned maps. The game's hopper file
/// (`multiplayer_hopper.dat`) has no room for a `MapId`, so the pin is kept
/// by the server beside it, keyed by the `hopper_id` sessions advertise.
/// A ranked (non-custom) session may only be on a retail map or on one
/// pinned here for its hopper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HopperPin {
    pub hopper_id: u32,
    /// Empty: retail maps only.
    #[serde(default)]
    pub map_ids: Vec<MapId>,
}

/// At most this many maps are pinned on one hopper.
pub const MAX_PINS: usize = 64;

impl HopperPin {
    pub fn validate(&self) -> Result<(), String> {
        if self.map_ids.len() > MAX_PINS {
            return Err(format!("at most {MAX_PINS} maps are pinned on a hopper"));
        }
        let mut seen = std::collections::BTreeSet::new();
        if !self.map_ids.iter().all(|m| seen.insert(*m)) {
            return Err("a map is pinned twice".into());
        }
        Ok(())
    }

    pub fn allows(&self, id: &MapId) -> bool {
        self.map_ids.contains(id)
    }
}

/// Whether a session with these `flags` (`custom`, `hopper`, `ranked`,
/// `coop`) is held to the ranked rule: retail or pinned maps only.
pub fn ranked_flags(flags: &str) -> bool {
    flags != "custom" && flags != "coop"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pins_parse_validate_and_allow() {
        let a = MapId([1; 32]);
        let p: HopperPin =
            serde_json::from_value(json!({"hopper_id": 7, "map_ids": [a.to_string()]})).unwrap();
        assert!(p.validate().is_ok());
        assert!(p.allows(&a));
        assert!(!p.allows(&MapId([2; 32])));
        let empty: HopperPin = serde_json::from_value(json!({"hopper_id": 7})).unwrap();
        assert!(empty.map_ids.is_empty());
        let twice = HopperPin {
            hopper_id: 1,
            map_ids: vec![a, a],
        };
        assert!(twice.validate().is_err());
        let many = HopperPin {
            hopper_id: 1,
            map_ids: (0..=MAX_PINS as u8).map(|i| MapId([i; 32])).collect(),
        };
        assert!(many.validate().is_err());
        assert!(serde_json::from_value::<HopperPin>(json!({"hopper_id": 1, "x": 1})).is_err());
        assert_eq!(hopper_stream(3), "hopper-3");
    }

    #[test]
    fn standing_follows_actions() {
        use Action::*;
        use Standing::*;
        assert_eq!(Ok.after(Hide), Hidden);
        assert_eq!(Hidden.after(Unhide), Ok);
        assert_eq!(Ok.after(Unhide), Ok);
        assert_eq!(Hidden.after(Delete), Deleted);
        assert_eq!(Deleted.after(Unhide), Deleted);
        assert_eq!(Deleted.after(Hide), Deleted);
        assert_eq!(Ok.after(Block), Blocked);
        assert_eq!(Blocked.after(Unhide), Blocked);
        assert_eq!(Blocked.after(Delete), Blocked);
        assert_eq!(Blocked.after(Unblock), Deleted);
        assert_eq!(Ok.after(Unblock), Ok);
        assert_eq!(Action::parse("block"), Some(Block));
        assert_eq!(Action::parse("purge"), None);
        assert!(Ok.is_ok() && !Hidden.is_ok());
    }

    #[test]
    fn sniffing_and_documents() {
        assert!(is_whole_map(b"daeh\x08\0\0\0"));
        assert!(!is_whole_map(b"PPF30"));
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\nIHDR"), Some("image/png"));
        assert_eq!(image_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(image_type(b"GIF89a"), None);
        assert!(!is_image(b""));
        let h = Sha256([3; 32]);
        let doc = MapDocument {
            manifest: Manifest::retail("lockout", h),
            image: None,
        };
        let v = serde_json::to_value(&doc).unwrap();
        assert!(v.get("image").is_none());
        assert_eq!(v["manifest"]["base"]["map"], "lockout");
        assert_eq!(serde_json::from_value::<MapDocument>(v).unwrap(), doc);
        assert!(
            serde_json::from_value::<MapDocument>(json!({"manifest": {}, "extra": 1})).is_err()
        );
        assert_eq!(content_stream(&h), format!("content-{}", "03".repeat(32)));
        assert_eq!(
            package_stream(&MapId([4; 32])),
            format!("package-{}", "04".repeat(32))
        );
        assert_eq!(KINDS.len(), 6);
    }

    #[test]
    fn ranked_is_everything_but_custom_and_coop() {
        assert!(!ranked_flags("custom"));
        assert!(!ranked_flags("coop"));
        assert!(ranked_flags("hopper"));
        assert!(ranked_flags("ranked"));
    }
}
