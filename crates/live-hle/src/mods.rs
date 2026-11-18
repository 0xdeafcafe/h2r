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
//! server has them. An entry with no `position` is never new.

use serde::Deserialize;

use content::MapId;

/// One published map, as the Mods screen lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    pub id: MapId,
    pub name: String,
    pub description: Option<String>,
    /// The author's gamertag.
    pub author: String,
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
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(10))
                .timeout_read(std::time::Duration::from_secs(30))
                .redirects(0)
                .build();
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
/// file name as a player reads it (first letter up, `_` as a space).
pub fn base_title(base: &str) -> String {
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
    image: Option<serde_json::Value>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    position: Option<u64>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    published_at: String,
}

#[derive(Deserialize)]
struct RawPage {
    items: Vec<RawEntry>,
    #[serde(default, alias = "next")]
    cursor: Option<serde_json::Value>,
    #[serde(default)]
    position: u64,
}

impl From<RawPage> for ModPage {
    fn from(r: RawPage) -> Self {
        let cursor = match r.cursor {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        ModPage {
            items: r
                .items
                .into_iter()
                .map(|e| ModEntry {
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

    fn entry(position: Option<u64>) -> ModEntry {
        ModEntry {
            id: ID.parse().unwrap(),
            name: "M".into(),
            description: None,
            author: "A".into(),
            base: "lockout".into(),
            image: false,
            size: 0,
            position,
            downloads: None,
            published_at: String::new(),
        }
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
        assert_eq!(size_text(0), "1 KB");
        assert_eq!(size_text(1536), "2 KB");
        assert_eq!(size_text(3_565_158), "3.4 MB");
    }
}
