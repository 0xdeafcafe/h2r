//! Custom and modded maps over Live (docs/game/maps.md §5): the content store's
//! wire types and events, and which maps a hopper allows.
//!
//! A map is named exactly by its [`MapId`] (a base plus layers); sessions
//! and games carry it beside the map's name. The server keeps the layers
//! (patches, packs, user-made maps, preview images) by SHA-256 and the
//! packages' manifests by `MapId`; never retail bytes.
use serde::{Deserialize, Serialize};

pub use content::{Manifest, MapId, Sha256};

pub use crate::map_details::{
    ClassChange, Contents, Detailed, Details, MAP_DETAILED, MAP_PACK_SET, MAX_DETAIL_IMAGES,
    MAX_DETAILS_DESCRIPTION, MapImage, PackInfo, PackListed, PackSet, PlacementChange,
    StatChange, Swap, pack_id_ok, pack_stream,
};

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
/// `MapDownloaded`: a signed-in account fetched a package's manifest for
/// the first time (server-signed, on [`package_stream`]; private: admins
/// read it, everyone else reads only the count). Data: [`Downloaded`].
pub const MAP_DOWNLOADED: &str = "MapDownloaded";

/// `MapCredited`: an admin set (or cleared) who made a package and where
/// it came from, over what its publisher signed (server-signed, on
/// [`package_stream`]). Display data only: the manifest, its `MapId` and
/// the publisher's signature are untouched. Data: [`Credited`].
pub const MAP_CREDITED: &str = "MapCredited";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 10] = [
    CONTENT_STORED,
    MAP_PUBLISHED,
    CONTENT_MODERATED,
    CONTENT_REPORTED,
    CONTENT_UPLOADS_SET,
    HOPPER_PINS_SET,
    MAP_DOWNLOADED,
    MAP_CREDITED,
    MAP_DETAILED,
    MAP_PACK_SET,
];

/// A package's kind of change, for browsing the repository. Display data:
/// not part of the `MapId`.
pub const CATEGORIES: [&str; 6] = ["gameplay", "weapons", "vehicles", "visuals", "maps", "other"];

/// The longest `source_url` kept.
pub const MAX_SOURCE_URL: usize = 300;
/// At most this many credited authors, each at most [`MAX_CREDIT_NAME`]
/// characters.
pub const MAX_CREDIT_AUTHORS: usize = 12;
pub const MAX_CREDIT_NAME: usize = 64;
/// The longest original release name.
pub const MAX_CREDIT_RELEASE: usize = 120;
/// The longest credit note.
pub const MAX_CREDIT_NOTES: usize = 500;

/// Who made a mod and where it came from: the original author(s), not the
/// account that published the package (that is the [`MapSummary::author`],
/// whose key signs the manifest). Display data, outside the `MapId`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credit {
    /// The original creators, as the release credits them (handles kept as
    /// written). Empty: unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// The original release's name, when the package is named otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// When it was first released: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    /// Where it was released or is kept (an `http`/`https` URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Anything else worth saying (a group, a disputed credit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A short single-line text: trimmed, no control characters.
fn line_ok(s: &str, max: usize) -> bool {
    let t = s.trim();
    !t.is_empty() && t.chars().count() <= max && !t.chars().any(char::is_control)
}

/// Whether `u` is a short `http`/`https` URL with no spaces or controls.
pub fn source_url_ok(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= MAX_SOURCE_URL
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD` with a plausible month and day.
pub fn release_date_ok(d: &str) -> bool {
    fn num(s: &str, len: usize) -> Option<u32> {
        if s.len() == len && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    }
    let year = |y: &str| num(y, 4).is_some_and(|y| y >= 1990);
    let month = |m: &str| num(m, 2).is_some_and(|m| (1..=12).contains(&m));
    let day = |d: &str| num(d, 2).is_some_and(|d| (1..=31).contains(&d));
    match d.split('-').collect::<Vec<_>>().as_slice() {
        &[y] => year(y),
        &[y, m] => year(y) && month(m),
        &[y, m, dd] => year(y) && month(m) && day(dd),
        _ => false,
    }
}

impl Credit {
    /// A credit naming only `authors` (each trimmed; blanks dropped).
    pub fn authors<I: IntoIterator<Item = S>, S: AsRef<str>>(authors: I) -> Self {
        Self {
            authors: authors
                .into_iter()
                .map(|a| a.as_ref().trim().to_string())
                .filter(|a| !a.is_empty())
                .collect(),
            ..Self::default()
        }
    }

    /// Nothing said: setting it clears an admin's credit.
    pub fn is_empty(&self) -> bool {
        self.authors.is_empty()
            && self.release.is_none()
            && self.released.is_none()
            && self.source_url.is_none()
            && self.notes.is_none()
    }

    /// The authors as one line: `A`, `A & B`, `A, B & C`. None when unknown.
    pub fn names(&self) -> Option<String> {
        match self.authors.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} & {last}", rest.join(", "))),
        }
    }

    /// The same credit with its texts trimmed and blank ones dropped.
    pub fn tidy(self) -> Self {
        let opt = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self {
            authors: Self::authors(self.authors).authors,
            release: opt(self.release),
            released: opt(self.released),
            source_url: opt(self.source_url),
            notes: opt(self.notes),
        }
    }

    /// The checks a publisher's or an admin's credit passes.
    pub fn validate(&self) -> Result<(), String> {
        if self.authors.len() > MAX_CREDIT_AUTHORS {
            return Err(format!("at most {MAX_CREDIT_AUTHORS} authors are credited"));
        }
        if let Some(a) = self.authors.iter().find(|a| !line_ok(a, MAX_CREDIT_NAME)) {
            return Err(format!(
                "author {a:?} must be 1 to {MAX_CREDIT_NAME} characters on one line"
            ));
        }
        if let Some(r) = &self.release
            && !line_ok(r, MAX_CREDIT_RELEASE)
        {
            return Err(format!(
                "release must be 1 to {MAX_CREDIT_RELEASE} characters on one line"
            ));
        }
        if let Some(d) = &self.released
            && !release_date_ok(d)
        {
            return Err(format!("released {d:?} is not YYYY, YYYY-MM or YYYY-MM-DD"));
        }
        if let Some(u) = &self.source_url
            && !source_url_ok(u)
        {
            return Err(format!(
                "source_url must be an http or https URL of at most {MAX_SOURCE_URL} characters"
            ));
        }
        if let Some(n) = &self.notes
            && (n.trim().is_empty()
                || n.chars().count() > MAX_CREDIT_NOTES
                || n.chars().any(|c| c.is_control() && c != '\n'))
        {
            return Err(format!("notes must be 1 to {MAX_CREDIT_NOTES} characters"));
        }
        Ok(())
    }
}

/// `MapCredited` data. `credit` None: the admin's credit is cleared and the
/// publisher's own shows again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credited {
    pub map_id: MapId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
    /// The admin's account (None: the operator's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::Xuid>,
}

/// What a package changes, for the in-game Mods page's checklist
/// (docs/game/maps.md §5, "A mod's page"). `h2 live maps import` derives
/// them from the patched map against its base (`crate`-external:
/// `cli::live_mod_features`); a `mod.toml` may add any. Display data:
/// not part of the `MapId`. In the order the page lists them.
pub const FEATURES: [&str; 8] = [
    "vehicles", "weapons", "objects", "textures", "shaders", "scripts", "geometry", "ai",
];
/// The game types a package says it is made for (`mod.toml`
/// `gametypes`); none said: any.
pub const GAMETYPES: [&str; 7] = [
    "slayer",
    "ctf",
    "assault",
    "oddball",
    "koth",
    "juggernaut",
    "territories",
];
/// The longest `version` kept.
pub const MAX_VERSION: usize = 24;

/// Whether `list` is ids of `known`, each once.
pub(crate) fn known_ids(list: &[String], known: &[&str]) -> Result<(), String> {
    for (i, f) in list.iter().enumerate() {
        if !known.contains(&f.as_str()) {
            return Err(format!("{f:?} is not one of {}", known.join(", ")));
        }
        if list[..i].contains(f) {
            return Err(format!("{f:?} is listed twice"));
        }
    }
    Ok(())
}

/// `list` in `known`'s order, unknown and repeated ids dropped: how a
/// derived list and a hand-written one are merged.
pub fn in_order<S: AsRef<str>>(list: &[S], known: &[&str]) -> Vec<String> {
    known
        .iter()
        .filter(|k| list.iter().any(|f| f.as_ref().trim() == **k))
        .map(|k| (*k).to_string())
        .collect()
}

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
    /// What the mod changes ([`FEATURES`], in that order), for the in-game
    /// checklist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// The game types it is made for ([`GAMETYPES`]); empty: any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gametypes: Vec<String>,
    /// The mod's own version, as its author numbers it (`1.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// One of [`CATEGORIES`], for the repository's filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Where the mod came from (an `http`/`https` URL), for credit when the
    /// publisher is not its author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Who made the mod, when the publisher is not its author. Newer than
    /// the manifest's `[package] author` (one free-text name), which still
    /// counts when this is absent ([`MapDocument::credit`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
}

impl MapDocument {
    /// A document with no repository extras.
    pub fn new(manifest: Manifest, image: Option<Sha256>) -> Self {
        Self {
            manifest,
            image,
            features: Vec::new(),
            gametypes: Vec::new(),
            version: None,
            category: None,
            source_url: None,
            credit: None,
        }
    }

    /// The publisher's credit: `credit`, else the manifest's `[package]
    /// author` and the document's `source_url`. None when neither says
    /// anything.
    pub fn credit(&self) -> Option<Credit> {
        let mut c = match &self.credit {
            Some(c) => c.clone(),
            None => Credit::authors(self.manifest.package.author.iter()),
        };
        if c.source_url.is_none() {
            c.source_url = self.source_url.clone();
        }
        (!c.is_empty()).then_some(c)
    }

    /// The extras' checks (the manifest has its own): a known category, a
    /// short `http`/`https` source URL with no spaces or controls.
    pub fn validate_extras(&self) -> Result<(), String> {
        known_ids(&self.features, &FEATURES).map_err(|e| format!("features: {e}"))?;
        known_ids(&self.gametypes, &GAMETYPES).map_err(|e| format!("gametypes: {e}"))?;
        if let Some(v) = &self.version
            && (v.trim().is_empty()
                || v.chars().count() > MAX_VERSION
                || v.chars().any(char::is_control))
        {
            return Err(format!(
                "version must be 1 to {MAX_VERSION} characters on one line"
            ));
        }
        if let Some(c) = &self.category
            && !CATEGORIES.contains(&c.as_str())
        {
            return Err(format!(
                "category {c:?} is not one of {}",
                CATEGORIES.join(", ")
            ));
        }
        if let Some(u) = &self.source_url
            && !source_url_ok(u)
        {
            return Err(format!(
                "source_url must be an http or https URL of at most {MAX_SOURCE_URL} characters"
            ));
        }
        if let Some(c) = &self.credit {
            c.validate().map_err(|e| format!("credit: {e}"))?;
        }
        Ok(())
    }
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

/// `MapDownloaded` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Downloaded {
    pub map_id: MapId,
    pub by: crate::Xuid,
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
    /// [`MapDocument::features`], [`MapDocument::gametypes`] and
    /// [`MapDocument::version`] as published.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gametypes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// `[result] geometry`: set when the mod says its geometry changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Sha256>,
    pub published_at: String,
    /// The `MapPublished` event's log position: a client that remembers
    /// the highest it has seen counts the packages above it as new.
    #[serde(default)]
    pub position: u64,
    /// Distinct signed-in accounts that fetched the manifest.
    #[serde(default)]
    pub downloads: u64,
    /// Who made the mod, on one line ([`Credit::names`] of `credits`): the
    /// original author(s), not the publisher (`author`). Kept for readers
    /// that know only this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Where the mod came from: `credits.source_url` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// The whole credit: an admin's `MapCredited`, else the publisher's
    /// ([`MapDocument::credit`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credit>,
    /// An admin's screenshots (`MapDetailed`), each served at
    /// `/v1/maps/{id}/images/{n}`. `description` is the admin's when one
    /// is set, `features` likewise, and `image` falls back to the first
    /// of these when the package has no preview of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MapImage>,
    /// What the mod contains, in words ([`Contents::lines`]); the whole
    /// report is on [`MapDetail::report`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<String>,
    /// The map pack it came out in, by id ([`Details::pack`]), and the
    /// pack's name when the server has its info. A list's page carries
    /// the info of the packs its items name (`packs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_name: Option<String>,
}

/// `GET /v1/maps/{map_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapDetail {
    #[serde(flatten)]
    pub summary: MapSummary,
    pub manifest: Manifest,
    /// The admin's contents report, whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Contents>,
    /// The pack's info, when it names one the server has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_info: Option<PackListed>,
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
        let doc = MapDocument::new(Manifest::retail("lockout", h), None);
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
        assert_eq!(KINDS.len(), 10);
    }

    #[test]
    fn document_extras_are_optional_and_checked() {
        let h = Sha256([3; 32]);
        let mut doc = MapDocument::new(Manifest::retail("lockout", h), None);
        let v = serde_json::to_value(&doc).unwrap();
        assert!(v.get("category").is_none() && v.get("source_url").is_none());
        assert!(doc.validate_extras().is_ok());
        doc.category = Some("weapons".into());
        doc.source_url = Some("https://example.invalid/mods/1".into());
        assert!(doc.validate_extras().is_ok());
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["category"], "weapons");
        assert_eq!(serde_json::from_value::<MapDocument>(v).unwrap(), doc);
        doc.category = Some("cheats".into());
        assert!(doc.validate_extras().is_err());
        doc.category = None;
        for bad in [
            "javascript:alert(1)",
            "ftp://example.invalid/x",
            "https://example.invalid/a b",
        ] {
            doc.source_url = Some(bad.into());
            assert!(doc.validate_extras().is_err(), "{bad}");
        }
        doc.source_url = Some(format!("https://{}", "a".repeat(MAX_SOURCE_URL)));
        assert!(doc.validate_extras().is_err());
    }

    #[test]
    fn a_documents_features_gametypes_and_version_are_checked() {
        let h = Sha256([3; 32]);
        let mut doc = MapDocument::new(Manifest::retail("lockout", h), None);
        let v = serde_json::to_value(&doc).unwrap();
        assert!(v.get("features").is_none() && v.get("gametypes").is_none());
        assert!(v.get("version").is_none(), "an older server reads it");
        doc.features = vec!["vehicles".into(), "scripts".into()];
        doc.gametypes = vec!["ctf".into()];
        doc.version = Some("1.2".into());
        assert!(doc.validate_extras().is_ok());
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["features"], json!(["vehicles", "scripts"]));
        assert_eq!(serde_json::from_value::<MapDocument>(v).unwrap(), doc);
        for (features, gametypes, version, why) in [
            (vec!["cheats"], vec![], None, "an unknown feature"),
            (vec!["ai", "ai"], vec![], None, "a repeated feature"),
            (vec![], vec!["infection"], None, "an unknown game type"),
            (vec![], vec![], Some(" "), "a blank version"),
            (vec![], vec![], Some("1\n2"), "a two-line version"),
        ] {
            let mut d = doc.clone();
            d.features = features.into_iter().map(Into::into).collect();
            d.gametypes = gametypes.into_iter().map(Into::into).collect();
            d.version = version.map(Into::into);
            assert!(d.validate_extras().is_err(), "{why}");
        }
        let mut d = doc.clone();
        d.version = Some("v".repeat(MAX_VERSION + 1));
        assert!(d.validate_extras().is_err(), "a long version");
    }

    #[test]
    fn lists_merge_in_the_known_order() {
        assert_eq!(
            in_order(&["scripts", " vehicles", "cheats", "vehicles"], &FEATURES),
            ["vehicles", "scripts"]
        );
        assert!(in_order::<&str>(&[], &FEATURES).is_empty());
    }

    #[test]
    fn summaries_from_older_servers_read_with_no_counts() {
        let v = json!({
            "map_id": "05".repeat(32), "name": "x",
            "author": {"xuid": "0009000000000001", "gamertag": "Modder"},
            "base": {"map": "lockout", "derived": false},
            "layers": [], "size": 0, "published_at": "2026-11-18T00:00:00Z"
        });
        let s: MapSummary = serde_json::from_value(v).unwrap();
        assert_eq!((s.position, s.downloads), (0, 0));
        assert!(s.credit.is_none() && s.category.is_none() && s.credits.is_none());
    }

    #[test]
    fn a_credit_names_its_authors_and_is_checked() {
        let c = Credit::authors([" Synch ", "", "Badsmiley"]);
        assert_eq!(c.authors, ["Synch", "Badsmiley"]);
        assert_eq!(c.names().as_deref(), Some("Synch & Badsmiley"));
        assert_eq!(
            Credit::authors(["A", "B", "C"]).names().as_deref(),
            Some("A, B & C")
        );
        assert_eq!(Credit::default().names(), None);
        assert!(Credit::default().is_empty() && !c.is_empty());
        assert!(c.validate().is_ok());
        let full = Credit {
            authors: vec!["Group Bocked".into()],
            release: Some("Beasthog v1".into()),
            released: Some("2005-03".into()),
            source_url: Some("https://web.archive.org/web/2005/http://x.invalid/a.ppf".into()),
            notes: Some("halomods.com group\nsecond line".into()),
        };
        assert!(full.validate().is_ok());
        let v = serde_json::to_value(&full).unwrap();
        assert_eq!(serde_json::from_value::<Credit>(v).unwrap(), full);
        for (bad, why) in [
            (
                Credit {
                    released: Some("05".into()),
                    ..full.clone()
                },
                "date",
            ),
            (
                Credit {
                    released: Some("2005-13".into()),
                    ..full.clone()
                },
                "month",
            ),
            (
                Credit {
                    released: Some("2005-01-32".into()),
                    ..full.clone()
                },
                "day",
            ),
            (
                Credit {
                    source_url: Some("ftp://x".into()),
                    ..full.clone()
                },
                "url",
            ),
            (
                Credit {
                    authors: vec!["a\nb".into()],
                    ..full.clone()
                },
                "newline",
            ),
            (
                Credit {
                    authors: vec!["x".repeat(MAX_CREDIT_NAME + 1)],
                    ..full.clone()
                },
                "long",
            ),
            (
                Credit {
                    authors: vec!["a".into(); MAX_CREDIT_AUTHORS + 1],
                    ..full.clone()
                },
                "many",
            ),
            (
                Credit {
                    release: Some(" ".into()),
                    ..full.clone()
                },
                "blank",
            ),
            (
                Credit {
                    notes: Some("n".repeat(MAX_CREDIT_NOTES + 1)),
                    ..full.clone()
                },
                "notes",
            ),
        ] {
            assert!(bad.validate().is_err(), "{why}");
        }
        for ok in ["2004", "2005-11", "2006-02-28"] {
            assert!(release_date_ok(ok), "{ok}");
        }
        assert!(serde_json::from_value::<Credit>(json!({"authors": [], "x": 1})).is_err());
        let tidy = Credit {
            authors: vec![" A ".into(), " ".into()],
            release: Some("  ".into()),
            notes: Some(" n ".into()),
            ..Credit::default()
        }
        .tidy();
        assert_eq!(
            tidy,
            Credit {
                authors: vec!["A".into()],
                notes: Some("n".into()),
                ..Credit::default()
            }
        );
    }

    #[test]
    fn a_documents_credit_falls_back_to_the_manifests_author() {
        let h = Sha256([3; 32]);
        let mut doc = MapDocument::new(Manifest::retail("lockout", h), None);
        assert_eq!(doc.credit(), None);
        doc.source_url = Some("https://example.invalid/m".into());
        assert_eq!(
            doc.credit(),
            Some(Credit {
                source_url: doc.source_url.clone(),
                ..Credit::default()
            })
        );
        doc.manifest.package.author = Some("Synch".into());
        let legacy = doc.credit().unwrap();
        assert_eq!(legacy.authors, ["Synch"]);
        assert_eq!(legacy.source_url, doc.source_url);
        doc.credit = Some(Credit::authors(["Badsmiley"]));
        let c = doc.credit().unwrap();
        assert_eq!(c.authors, ["Badsmiley"], "the document's credit wins");
        assert_eq!(c.source_url, doc.source_url, "the source URL is kept");
        assert!(doc.validate_extras().is_ok());
        doc.credit = Some(Credit {
            released: Some("soon".into()),
            ..Credit::default()
        });
        assert!(doc.validate_extras().unwrap_err().starts_with("credit:"));
        // An older document (no credit) reads; a credited one round-trips.
        doc.credit = Some(Credit::authors(["Badsmiley"]));
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["credit"]["authors"][0], "Badsmiley");
        assert_eq!(serde_json::from_value::<MapDocument>(v).unwrap(), doc);
        let d = Credited {
            map_id: MapId([1; 32]),
            credit: None,
            by: None,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("credit").is_none());
        assert_eq!(serde_json::from_value::<Credited>(v).unwrap(), d);
    }

    #[test]
    fn ranked_is_everything_but_custom_and_coop() {
        assert!(!ranked_flags("custom"));
        assert!(!ranked_flags("coop"));
        assert!(ranked_flags("hopper"));
        assert!(ranked_flags("ranked"));
    }
}
