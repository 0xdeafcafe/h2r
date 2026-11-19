//! The mod repository's list (docs/game/maps.md §5, "Mod downloads"): the
//! server's published maps, newest first, as the game's Mods screen shows
//! them, and what counts as new.
//!
//! The list is `GET /v1/maps?sort=new&limit=N[&cursor=C]`, read with the
//! signed-in account's `read` token through the Live worker's retrying GET
//! ([`crate::Hle::mod_list`]). Installing one goes through
//! [`crate::maps::MapGate::install`] on a thread of its own
//! ([`crate::Hle::install_mod`]), so a large download never holds up the
//! worker's heartbeat.
//!
//! The answer is read leniently: `id` or `map_id`; `author` and `base` as
//! a name or as the content API's `{gamertag}` / `{map}` objects; `cursor`
//! or `next`; an entry's `position`, `downloads` and `image` when the
//! server has them. An entry with no `position` is never new. Who made the
//! mod is `credits.authors` (else the one-line `credit`), never the
//! publishing account (`author`). What a mod changes (`features`), the
//! game types it is made for and its version are read when the server
//! lists them (docs/game/maps.md §5, "A mod's page"), unknown ids dropped.
//! An admin's details add screenshots (`images`, each fetched from
//! `/v1/maps/{id}/images/{n}`) and what the mod contains in words
//! (`contents`); `description` is then the admin's longer one. A mod
//! that came out in a map pack names it (`pack`, its id, and `pack_name`);
//! the page says each pack's info once (`packs`), which every entry of it
//! carries as [`ModEntry::pack_info`], so the screen can group a pack's
//! maps and show the pack's own page.

use serde::Deserialize;

use content::MapId;

/// One published map, as the Mods screen lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    pub id: MapId,
    pub name: String,
    pub description: Option<String>,
    /// The publishing account's gamertag (who uploaded it, not who made
    /// it).
    pub author: String,
    /// Who made the mod, on one line (`A & B`), when the server says.
    pub credit: Option<String>,
    /// The disc map it is built on (`lockout`), what the player needs.
    pub base: String,
    /// Whether the server has a preview image for it.
    pub image: bool,
    /// Bytes a player without it downloads.
    pub size: u64,
    /// The event-log position it was published at: newer is larger.
    pub position: Option<u64>,
    pub downloads: Option<u64>,
    pub published_at: String,
    /// What it changes, ids of `live_proto::maps::FEATURES` in that order.
    pub features: Vec<String>,
    /// The game types it is made for (`live_proto::maps::GAMETYPES`);
    /// empty: any.
    pub gametypes: Vec<String>,
    /// Its own version, when the server lists one.
    pub version: Option<String>,
    /// Screenshots, in order: the `n`th is `/v1/maps/{id}/images/{n}`.
    pub images: Vec<ModImage>,
    /// What it contains, one line each (`projectile swap: ...`).
    pub contents: Vec<String>,
    /// The map pack it came out in, by name (the pack's id when the
    /// server has no info for it).
    pub pack: Option<String>,
    /// That pack's info, when the server lists it.
    pub pack_info: Option<ModPack>,
    /// What the pack is: the entry's own `pack.description`, else its
    /// info's description.
    pub pack_description: Option<String>,
}

/// A map pack, as its maps' page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModPack {
    /// `ai-revision`: what [`ModEntry`]s of it are grouped by.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Who made or compiled it, on one line (`A & B`).
    pub authors: Option<String>,
    /// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    pub released: Option<String>,
    pub source_url: Option<String>,
    /// Its images: the `n`th is [`ModPack::image_path`].
    pub images: Vec<ModImage>,
}

impl ModPack {
    /// The API path of image `n`.
    pub fn image_path(&self, n: usize) -> String {
        format!("/v1/mappacks/{}/images/{n}", self.id)
    }
}

/// One screenshot of a mod, as its page captions it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModImage {
    pub caption: Option<String>,
    /// Who made the image.
    pub credit: Option<String>,
}

impl ModEntry {
    /// The API path of screenshot `n`.
    pub fn image_path(&self, n: usize) -> String {
        format!("/v1/maps/{}/images/{n}", self.id)
    }
}

/// A page of the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "RawPage")]
pub struct ModPage {
    pub items: Vec<ModEntry>,
    /// The next page's cursor, when there is one.
    pub cursor: Option<String>,
    /// The server's event-log position when it answered.
    pub position: u64,
}

/// Why there is no list, or no install.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModsError {
    #[error("Live is off or its server is not set up")]
    Offline,
    #[error("not signed in to Live")]
    NotSignedIn,
    #[error("Live is busy; try again")]
    Busy,
    #[error("the server could not be read: {0}")]
    Server(String),
    #[error("{0}")]
    Refused(crate::maps::Refusal),
}

/// Told the list, or why there is none (on the Live worker).
pub type ListDone = Box<dyn FnOnce(Result<ModPage, ModsError>) + Send>;
/// Told the installed package's directory, or why not (on the download's
/// thread).
pub type InstallDone = Box<dyn FnOnce(Result<std::path::PathBuf, ModsError>) + Send>;
/// Told the bytes written so far and the bytes to fetch in all.
pub type Progress = Box<dyn FnMut(u64, u64) + Send>;

/// A failed worker request as the screens say it.
pub fn server_error(code: u32) -> ModsError {
    use crate::status::{
        CONNECTION_LOST, INVALID_USER, NO_SESSION, NO_USER, SERVERS_BUSY, TASK_BUSY,
    };
    match code {
        NO_SESSION | NO_USER | INVALID_USER | CONNECTION_LOST => ModsError::NotSignedIn,
        TASK_BUSY | SERVERS_BUSY => ModsError::Busy,
        _ => ModsError::Server(format!("error {code:#010x}")),
    }
}

/// Installs `id` through `gate` from the API at `api`, on a thread of its
/// own (a big map must not hold up the Live worker), counting the download
/// with `reader`'s tokens. `done` is called once either way.
pub(crate) fn spawn_install(
    api: String,
    gate: std::sync::Arc<crate::maps::MapGate>,
    reader: Option<crate::maps::Reader>,
    id: MapId,
    mut progress: Progress,
    done: InstallDone,
) {
    use std::sync::{Arc, Mutex};
    let slot = Arc::new(Mutex::new(Some(done)));
    let theirs = Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .name("h2-live-mod".into())
        .spawn(move || {
            let agent = crate::maps::download_agent();
            let fetch = crate::maps::HttpFetch {
                agent: &agent,
                api: &api,
                reader,
            };
            let result = gate
                .install(&fetch, id, &mut |d, t| progress(d, t))
                .map_err(ModsError::Refused);
            if let Some(d) = theirs.lock().unwrap_or_else(|e| e.into_inner()).take() {
                d(result);
            }
        });
    if let Err(e) = spawned
        && let Some(d) = slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    {
        d(Err(ModsError::Server(format!("no download thread: {e}"))));
    }
}

/// The most entries one list asks for.
pub const MAX_LIMIT: u32 = 100;

/// The list's path under the API (`/maps?sort=new&limit=N[&cursor=C]`).
pub fn list_path(limit: u32, cursor: Option<&str>) -> String {
    let mut p = format!("/maps?sort=new&limit={}", limit.clamp(1, MAX_LIMIT));
    if let Some(c) = cursor.filter(|c| !c.is_empty()) {
        p.push_str("&cursor=");
        p.extend(url::form_urlencoded::byte_serialize(c.as_bytes()));
    }
    p
}

/// How many of `items` are new to a player who last saw position `seen`:
/// published after it.
pub fn new_count(items: &[ModEntry], seen: u64) -> usize {
    items.iter().filter(|e| is_new(e, seen)).count()
}

/// Whether `e` was published after position `seen`.
pub fn is_new(e: &ModEntry, seen: u64) -> bool {
    e.position.is_some_and(|p| p > seen)
}

/// The position to remember once the player has seen `page`: the newest
/// entry's, else the server's, never less than `seen`.
pub fn seen_after(page: &ModPage, seen: u64) -> u64 {
    page.items
        .iter()
        .filter_map(|e| e.position)
        .max()
        .unwrap_or(0)
        .max(seen)
}

/// `"Lockout"` from `lockout`, `"Ascension"` from `ascension`: the base map's
/// file name as a player reads it (first letter up, `_` as a space). A
/// Multiplayer Map Pack map has its game name: `"Relic"` from `dune`.
pub fn base_title(base: &str) -> String {
    if let Some(title) = content::map_pack::title(base) {
        return title.to_owned();
    }
    let mut out = String::with_capacity(base.len());
    let mut up = true;
    for c in base.chars() {
        if c == '_' {
            out.push(' ');
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A size as the screens show it: `512 KB`, `3.4 MB`.
pub fn size_text(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        format!("{} KB", bytes.div_ceil(KB).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Named {
    Name(String),
    Account { gamertag: String },
    Base { map: String },
}

impl Named {
    fn into_name(self) -> String {
        match self {
            Named::Name(n) | Named::Account { gamertag: n } | Named::Base { map: n } => n,
        }
    }
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(alias = "map_id")]
    id: MapId,
    name: String,
    #[serde(default)]
    description: Option<String>,
    author: Named,
    base: Named,
    #[serde(default)]
    credit: Option<String>,
    #[serde(default)]
    credits: Option<RawCredits>,
    #[serde(default)]
    image: Option<serde_json::Value>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    position: Option<u64>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    gametypes: Vec<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    images: Vec<RawImage>,
    #[serde(default)]
    contents: Vec<String>,
    #[serde(default)]
    pack: Option<RawPackRef>,
    #[serde(default)]
    pack_name: Option<String>,
}

#[derive(Deserialize)]
struct RawPack {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    images: Vec<RawImage>,
}

impl RawPack {
    fn into_pack(self) -> ModPack {
        let authors: Vec<String> = self
            .authors
            .into_iter()
            .filter_map(|a| text(Some(a)))
            .collect();
        let authors = match authors.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} & {last}", rest.join(", "))),
        };
        ModPack {
            name: text(Some(self.name)).unwrap_or_else(|| self.id.clone()),
            id: self.id,
            description: text(self.description),
            authors,
            released: text(self.released),
            source_url: text(self.source_url),
            images: self.images.into_iter().map(RawImage::into_image).collect(),
        }
    }
}

impl RawImage {
    fn into_image(self) -> ModImage {
        ModImage {
            caption: text(self.caption),
            credit: text(self.credit),
        }
    }
}

#[derive(Deserialize)]
struct RawImage {
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    credit: Option<String>,
}

fn text(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Deserialize)]
struct RawCredits {
    #[serde(default)]
    authors: Vec<String>,
    /// The release a credit names (a pack's name), the pack when the
    /// entry names none.
    #[serde(default)]
    release: Option<String>,
}

/// A mod's pack: its name, or an object with a name and a description.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawPackRef {
    Name(String),
    Pack {
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
}

/// `s` trimmed, or None when blank.
fn some_text(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl RawEntry {
    /// The pack's name and description: `pack`, else the credit's release.
    fn pack(&self) -> (Option<String>, Option<String>) {
        match &self.pack {
            Some(RawPackRef::Name(n)) if !n.trim().is_empty() => (some_text(Some(n)), None),
            Some(RawPackRef::Pack { name, description }) if !name.trim().is_empty() => {
                (some_text(Some(name)), some_text(description.as_deref()))
            }
            _ => (
                some_text(self.credits.as_ref().and_then(|c| c.release.as_deref())),
                None,
            ),
        }
    }
    /// `credits.authors` as one line, else `credit`; blanks are none.
    fn credit_line(&self) -> Option<String> {
        let authors: Vec<&str> = self
            .credits
            .iter()
            .flat_map(|c| c.authors.iter())
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .collect();
        let line = match authors.as_slice() {
            [] => self
                .credit
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string(),
            [one] => (*one).to_string(),
            [rest @ .., last] => format!("{} & {last}", rest.join(", ")),
        };
        (!line.is_empty()).then_some(line)
    }
}

#[derive(Deserialize)]
struct RawPage {
    items: Vec<RawEntry>,
    #[serde(default, alias = "next")]
    cursor: Option<serde_json::Value>,
    #[serde(default)]
    position: u64,
    #[serde(default)]
    packs: Vec<RawPack>,
}

impl From<RawPage> for ModPage {
    fn from(r: RawPage) -> Self {
        let cursor = match r.cursor {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let packs: Vec<ModPack> = r.packs.into_iter().map(RawPack::into_pack).collect();
        ModPage {
            items: r
                .items
                .into_iter()
                .map(|e| {
                    let (named, described) = e.pack();
                    let pack_info = match &e.pack {
                        Some(RawPackRef::Name(id)) => {
                            packs.iter().find(|p| p.id == id.trim()).cloned()
                        }
                        _ => None,
                    };
                    let pack = text(e.pack_name.clone())
                        .or_else(|| pack_info.as_ref().map(|p| p.name.clone()))
                        .or(named);
                    let pack_description = described
                        .or_else(|| pack_info.as_ref().and_then(|p| p.description.clone()));
                    (e, pack, pack_info, pack_description)
                })
                .map(|(e, pack, pack_info, pack_description)| ModEntry {
                    pack_info,
                    pack,
                    pack_description,
                    credit: e.credit_line(),
                    id: e.id,
                    name: e.name,
                    description: e.description.filter(|d| !d.trim().is_empty()),
                    author: e.author.into_name(),
                    base: e.base.into_name(),
                    image: e.image.is_some_and(|v| !v.is_null()),
                    size: e.size,
                    position: e.position,
                    downloads: e.downloads,
                    published_at: e.published_at,
                    features: live_proto::maps::in_order(&e.features, &live_proto::maps::FEATURES),
                    gametypes: live_proto::maps::in_order(
                        &e.gametypes,
                        &live_proto::maps::GAMETYPES,
                    ),
                    version: e
                        .version
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty()),
                    images: e.images.into_iter().map(RawImage::into_image).collect(),
                    contents: e
                        .contents
                        .into_iter()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect(),
                })
                .collect(),
            cursor,
            position: r.position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn the_repository_shape_reads() {
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [{
                "id": ID, "name": "Lockout Snipers", "author": "Modder",
                "base": "lockout", "image": "/v1/maps/x/image", "size": 2048,
                "position": 41, "downloads": 7, "published_at": "2026-11-18T00:00:00Z"
            }],
            "cursor": "41",
            "position": 50
        }))
        .unwrap();
        let e = &page.items[0];
        assert_eq!(e.id.to_string(), ID);
        assert_eq!((e.author.as_str(), e.base.as_str()), ("Modder", "lockout"));
        assert_eq!(e.credit, None);
        assert!(e.image);
        assert_eq!((e.size, e.position, e.downloads), (2048, Some(41), Some(7)));
        assert_eq!(page.cursor.as_deref(), Some("41"));
        assert_eq!(page.position, 50);
    }

    #[test]
    fn the_content_api_shape_reads_too() {
        // `GET /v1/maps` as the content API answered before the repository:
        // `map_id`, objects for the author and base, `next`, no positions.
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [{
                "map_id": ID, "name": "Lockout Snipers", "description": "  ",
                "author": {"xuid": "0009000000000001", "gamertag": "Modder"},
                "base": {"map": "lockout", "derived": false},
                "layers": [], "size": 10, "published_at": "2026-11-16T00:00:00Z"
            }],
            "next": null,
            "position": 9
        }))
        .unwrap();
        let e = &page.items[0];
        assert_eq!((e.author.as_str(), e.base.as_str()), ("Modder", "lockout"));
        assert_eq!(
            (e.position, e.image, e.description.clone()),
            (None, false, None)
        );
        assert_eq!(page.cursor, None);
        assert_eq!(new_count(&page.items, 0), 0, "no position: never new");
    }

    #[test]
    fn the_credit_is_the_makers_not_the_publishers() {
        let item = |extra: serde_json::Value| {
            let mut v = serde_json::json!({
                "map_id": ID, "name": "Beasthog",
                "author": {"xuid": "0009000000000001", "gamertag": "afr"},
                "base": {"map": "zanzibar", "derived": false}
            });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let page: ModPage =
                serde_json::from_value(serde_json::json!({ "items": [v] })).unwrap();
            page.items[0].clone()
        };
        let e = item(serde_json::json!({
            "credit": "Synch & NeoN",
            "credits": {"authors": ["Synch", " NeoN ", ""], "released": "2006"}
        }));
        assert_eq!(e.author, "afr", "the publisher");
        assert_eq!(e.credit.as_deref(), Some("Synch & NeoN"));
        let e = item(serde_json::json!({"credits": {"authors": ["A", "B", "C"]}}));
        assert_eq!(e.credit.as_deref(), Some("A, B & C"));
        // An older server: the one-line credit only.
        let e = item(serde_json::json!({"credit": " Group Bocked "}));
        assert_eq!(e.credit.as_deref(), Some("Group Bocked"));
        let e = item(serde_json::json!({"credit": " ", "credits": {"authors": []}}));
        assert_eq!(e.credit, None);
    }

    #[test]
    fn a_pack_reads_as_a_name_an_object_or_the_credits_release() {
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [
                {"id": ID, "name": "A", "author": "M", "base": "lockout", "pack": " Phantom Mappack "},
                {"id": ID, "name": "B", "author": "M", "base": "lockout",
                 "pack": {"name": "AI Revision", "description": "Bots that think. "}},
                {"id": ID, "name": "C", "author": "M", "base": "lockout",
                 "credits": {"authors": ["X"], "release": "Hockey Fans"}},
                {"id": ID, "name": "D", "author": "M", "base": "lockout", "pack": " ",
                 "credits": {"release": "DoorM4n Legacy"}},
                {"id": ID, "name": "E", "author": "M", "base": "lockout"}
            ]
        }))
        .unwrap();
        let packs: Vec<(Option<&str>, Option<&str>)> = page
            .items
            .iter()
            .map(|e| (e.pack.as_deref(), e.pack_description.as_deref()))
            .collect();
        assert_eq!(
            packs,
            [
                (Some("Phantom Mappack"), None),
                (Some("AI Revision"), Some("Bots that think.")),
                (Some("Hockey Fans"), None),
                (Some("DoorM4n Legacy"), None),
                (None, None),
            ]
        );
    }

    fn entry(position: Option<u64>) -> ModEntry {
        ModEntry {
            id: ID.parse().unwrap(),
            name: "M".into(),
            description: None,
            author: "A".into(),
            credit: None,
            base: "lockout".into(),
            image: false,
            size: 0,
            position,
            downloads: None,
            published_at: String::new(),
            features: Vec::new(),
            gametypes: Vec::new(),
            version: None,
            images: Vec::new(),
            contents: Vec::new(),
            pack: None,
            pack_info: None,
            pack_description: None,
        }
    }

    #[test]
    fn a_packs_maps_name_it_and_carry_its_info() {
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [
                {"id": ID, "name": "Zombiebar", "author": "afr", "base": "zanzibar",
                 "pack": "ai-revision", "pack_name": "Halo 2 Remapped AI Revision"},
                {"id": ID, "name": "Lone", "author": "afr", "base": "lockout",
                 "pack": "no-info"},
                {"id": ID, "name": "Plain", "author": "afr", "base": "lockout"}
            ],
            "packs": [{
                "id": "ai-revision", "name": "Halo 2 Remapped AI Revision",
                "description": "Custom maps with AI.", "authors": ["Zone 117", " ", "Detox"],
                "released": "2007-06", "images": [{"sha256": "01".repeat(32), "caption": "menu"}]
            }]
        }))
        .unwrap();
        let [bar, lone, plain] = &page.items[..] else {
            panic!("three items")
        };
        assert_eq!(bar.pack.as_deref(), Some("Halo 2 Remapped AI Revision"));
        let info = bar.pack_info.as_ref().unwrap();
        assert_eq!(info.id, "ai-revision");
        assert_eq!(info.authors.as_deref(), Some("Zone 117 & Detox"));
        assert_eq!(info.released.as_deref(), Some("2007-06"));
        assert_eq!(info.images[0].caption.as_deref(), Some("menu"));
        assert_eq!(info.image_path(0), "/v1/mappacks/ai-revision/images/0");
        assert_eq!(
            lone.pack.as_deref(),
            Some("no-info"),
            "the id when there is no info"
        );
        assert_eq!(lone.pack_info, None);
        assert_eq!(
            (plain.pack.as_ref(), plain.pack_info.as_ref()),
            (None, None)
        );
    }

    #[test]
    fn an_admins_images_and_contents_read() {
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [{
                "id": ID, "name": "Beasthog", "author": "afr", "base": "zanzibar",
                "description": "A longer one.",
                "images": [
                    {"sha256": "01".repeat(32), "caption": " The hog ", "credit": "Bocked"},
                    {"sha256": "02".repeat(32)}
                ],
                "contents": ["model swap: warthog uses the scorpion model", " "]
            }]
        }))
        .unwrap();
        let e = &page.items[0];
        assert_eq!(e.description.as_deref(), Some("A longer one."));
        assert_eq!(
            e.images,
            [
                ModImage {
                    caption: Some("The hog".into()),
                    credit: Some("Bocked".into())
                },
                ModImage::default()
            ]
        );
        assert_eq!(e.image_path(1), format!("/v1/maps/{ID}/images/1"));
        assert_eq!(e.contents, ["model swap: warthog uses the scorpion model"]);
        assert!(entry(None).images.is_empty() && entry(None).contents.is_empty());
    }

    #[test]
    fn features_game_types_and_version_read_when_listed() {
        let page: ModPage = serde_json::from_value(serde_json::json!({
            "items": [{
                "id": ID, "name": "Lockout Hogs", "author": "Modder", "base": "lockout",
                "features": ["scripts", "vehicles", "teleporters"],
                "gametypes": ["ctf"], "version": " 1.2 "
            }, {
                "id": ID, "name": "Plain", "author": "Modder", "base": "lockout",
                "version": " "
            }]
        }))
        .unwrap();
        let e = &page.items[0];
        assert_eq!(e.features, ["vehicles", "scripts"], "known ids, in order");
        assert_eq!(e.gametypes, ["ctf"]);
        assert_eq!(e.version.as_deref(), Some("1.2"));
        let plain = &page.items[1];
        assert!(plain.features.is_empty() && plain.gametypes.is_empty());
        assert_eq!(plain.version, None);
    }

    #[test]
    fn new_is_published_after_the_last_seen_position() {
        let items = vec![
            entry(Some(30)),
            entry(Some(20)),
            entry(Some(10)),
            entry(None),
        ];
        assert_eq!(new_count(&items, 0), 3, "never seen: every positioned one");
        assert_eq!(new_count(&items, 10), 2);
        assert_eq!(new_count(&items, 20), 1);
        assert_eq!(new_count(&items, 30), 0);
        let page = ModPage {
            items,
            cursor: None,
            position: 99,
        };
        assert_eq!(
            seen_after(&page, 0),
            30,
            "the newest entry, not the log's end"
        );
        assert_eq!(seen_after(&page, 40), 40, "never goes back");
        assert_eq!(seen_after(&ModPage::default(), 5), 5);
    }

    #[test]
    fn the_list_path_asks_newest_first_within_bounds() {
        assert_eq!(list_path(25, None), "/maps?sort=new&limit=25");
        assert_eq!(list_path(0, Some("")), "/maps?sort=new&limit=1");
        assert_eq!(
            list_path(1000, Some("41")),
            "/maps?sort=new&limit=100&cursor=41"
        );
        assert_eq!(
            list_path(5, Some("a b&c")),
            "/maps?sort=new&limit=5&cursor=a+b%26c"
        );
    }

    #[test]
    fn names_and_sizes_read_as_the_screens_show_them() {
        assert_eq!(base_title("lockout"), "Lockout");
        assert_eq!(base_title("beaver_creek"), "Beaver Creek");
        assert_eq!(base_title("dune"), "Relic");
        assert_eq!(base_title("deltatap"), "Sanctuary");
        assert_eq!(base_title("turf"), "Turf");
        assert_eq!(size_text(0), "1 KB");
        assert_eq!(size_text(1536), "2 KB");
        assert_eq!(size_text(3_565_158), "3.4 MB");
    }
}
