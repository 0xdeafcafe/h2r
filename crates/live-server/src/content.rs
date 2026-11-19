//! The content store for custom and modded maps (docs/game/maps.md §5,
//! docs/live/live-api.md "Custom maps", docs/live/live-admin.md "Custom content").
//!
//! - **Blobs** (patches, packs, user-made maps, preview images) are kept by
//!   SHA-256 as `content/<aa>/<sha256>` in the server's private blob store
//!   ([`crate::blobs`]: `<data>/content/...`, or a bucket): checked on
//!   upload, stored once, never changed. They are served through the
//!   server, never by redirect: the game's client follows no redirects,
//!   and moderation must be able to take a blob down at once. `PUT /v1/content/{sha256}` is the
//!   author's, with a per-account rate limit, a size cap (larger for whole
//!   maps) and a per-account quota.
//! - **Packages**: `PUT /v1/maps` takes a manifest signed by its author
//!   (a `MapPublished` event); the server checks it, computes its `MapId`
//!   and refuses it while any layer is missing.
//! - **Retail bytes are never kept**: blobs whose hash is in
//!   `<data>/retail-maps.sha256` are refused, and so is a manifest that is
//!   only a retail base.
//! - **Credit**: who made a mod is display data beside the signed
//!   manifest: the publisher's `credit` (or `[package] author`), which an
//!   admin may correct with `PUT /v1/maps/{id}/credit` (a server-signed
//!   `MapCredited`; the manifest, its `MapId` and the publisher's
//!   signature are untouched).
//! - **Details**: a longer description, screenshots (content blobs, PNG
//!   or JPEG) and a report of what the mod contains are set by an admin
//!   with `PUT /v1/maps/{id}/details` (a server-signed `MapDetailed`, the
//!   manifest untouched like the credit's); the screenshots are served at
//!   `GET /v1/maps/{id}/images/{n}`. Details may name the map pack the
//!   mod came out in; a pack's own info (name, description, authors,
//!   images, source, date) is an admin's `PUT /v1/mappacks/{id}` (a
//!   server-signed `MapPackSet`), listed at `GET /v1/mappacks`, with each
//!   map list page and each map that names it.
//! - **Moderation**: admins hide, delete and block blobs and packages,
//!   switch an account's uploads off, and read reports. Hidden, deleted or
//!   blocked content answers 410, and sessions on it are refused.
//!
//! The metadata is the log's: server-signed `ContentStored`,
//! `ContentModerated`, `ContentReported`, `ContentUploadsSet` and
//! `HopperPinsSet` events, and the author's own `MapPublished`, projected
//! here by [`Catalog`]. So it is kept by whichever store the server runs
//! on (the `events` table in Postgres), and replayed at startup.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use live_proto::maps::{
    self, Action, BaseInfo, CONTENT_MODERATED, CONTENT_REPORTED, CONTENT_STORED,
    CONTENT_UPLOADS_SET, ContentStored, Credit, Credited, Detailed, Details, Downloaded,
    HOPPER_PINS_SET, HopperPin, LayerInfo, MAP_CREDITED, MAP_DETAILED, MAP_DOWNLOADED,
    MAP_PACK_SET, MAP_PUBLISHED, MapAnswer, MapDetail, MapDocument, MapId, MapSummary, Moderated,
    PackInfo, PackListed, PackSet, Reported, Sha256, Standing, Target, UploadsSet, Who,
};
use live_proto::sign::{MAX_EVENT_SKEW_S, SignedEvent, parse_time};
use live_proto::{Author, ErrorCode, Gamertag, Page, Xuid, lxm, streams};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found, now, sign_error};
use crate::dispatch::Subscriber;
use crate::model::nonce_key;
use crate::store::{NewEvent, RecordedEvent};

/// The retail hash list's file name in the data directory: `sha256sum`
/// output (`<hex>  <name>.map`, one per line; the name is optional, `#`
/// starts a comment). Absent or empty by default.
pub const RETAIL_FILE: &str = "retail-maps.sha256";
/// The blob directory's name in the data directory.
pub const CONTENT_DIR: &str = "content";
/// The largest preview image.
pub const MAX_IMAGE_BYTES: u64 = ::content::manifest::MAX_IMAGE_BYTES;
const MIB: u64 = 1024 * 1024;

/// The largest request body Cloudflare passes to a Worker on workers.dev
/// and on Free and Pro zones (100 MB, decimal; larger bodies get the
/// edge's own 413 before the Worker runs). The Worker plan does not raise
/// it.
pub const EDGE_BODY_BYTES: u64 = 100_000_000;
/// The default cap on one upload, whole map or not: the largest whole MiB
/// under [`EDGE_BODY_BYTES`], so both homes take and refuse the same
/// bodies, with the server's own message. 95 MiB holds the delta of a map
/// that doubles its base (Night Ops, 68 MiB), and is under the game's
/// download cap (`live_hle::maps::MAX_DOWNLOAD`, 100 MiB).
pub const DEFAULT_UPLOAD_BYTES: u64 = EDGE_BODY_BYTES / MIB * MIB;

/// Sizes and rates for uploads (docs/live/live-admin.md, "Custom content").
#[derive(Debug, Clone, PartialEq)]
pub struct ContentLimits {
    /// A patch, pack or image.
    pub layer_bytes: u64,
    /// A whole map file (a user-made map's one layer).
    pub map_bytes: u64,
    /// Everything one account has stored, deleted blobs not counted.
    pub quota_bytes: u64,
    /// Uploads and publishes per account: per second, burst.
    pub upload_rate: (f64, u32),
}

impl Default for ContentLimits {
    fn default() -> Self {
        ContentLimits {
            layer_bytes: DEFAULT_UPLOAD_BYTES,
            map_bytes: DEFAULT_UPLOAD_BYTES,
            quota_bytes: 1024 * MIB,
            upload_rate: (0.5, 20),
        }
    }
}

impl ContentLimits {
    /// The defaults with `--content-layer-mib`, `--content-map-mib` and
    /// `--content-quota-mib` applied (whole MiB, at least 1).
    pub fn from_flags(
        layer: Option<String>,
        map: Option<String>,
        quota: Option<String>,
    ) -> Result<Self, String> {
        let mut l = ContentLimits::default();
        for (flag, value, field) in [
            ("--content-layer-mib", layer, &mut l.layer_bytes),
            ("--content-map-mib", map, &mut l.map_bytes),
            ("--content-quota-mib", quota, &mut l.quota_bytes),
        ] {
            if let Some(v) = value {
                let n: u64 = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=1 << 20).contains(n))
                    .ok_or_else(|| format!("{flag} takes a whole number of MiB, got {v:?}"))?;
                *field = n * MIB;
            }
        }
        Ok(l)
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn gone(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Gone, m)
}
fn too_large(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::TooLarge, m)
}
fn unavailable(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Unavailable, m)
}

/// `GET /v1/maps`'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapSort {
    /// Newest first: by the `MapPublished` event's log position.
    #[default]
    New,
    /// Most downloaded first, ties newest first.
    Popular,
}

impl MapSort {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "new" => Some(Self::New),
            "popular" => Some(Self::Popular),
            _ => None,
        }
    }
}

/// Where a page of `GET /v1/maps` ends: the last item's position (`new`),
/// or its downloads and position (`popular`, written `<downloads>:<position>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapCursor {
    New(u64),
    Popular(u64, u64),
}

impl MapCursor {
    pub fn of(sort: MapSort, downloads: u64, position: u64) -> Self {
        match sort {
            MapSort::New => Self::New(position),
            MapSort::Popular => Self::Popular(downloads, position),
        }
    }

    /// The cursor `s` for `sort`; None when it is not that sort's form.
    pub fn parse(sort: MapSort, s: &str) -> Option<Self> {
        match sort {
            MapSort::New => s.parse().ok().map(Self::New),
            MapSort::Popular => {
                let (d, p) = s.split_once(':')?;
                Some(Self::Popular(d.parse().ok()?, p.parse().ok()?))
            }
        }
    }
}

impl std::fmt::Display for MapCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New(p) => write!(f, "{p}"),
            Self::Popular(d, p) => write!(f, "{d}:{p}"),
        }
    }
}

/// `GET /v1/maps`'s query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapQuery {
    pub sort: MapSort,
    /// The base map's name, any case.
    pub base: Option<String>,
    /// The publisher's gamertag; one no account has matches nothing.
    pub author: Option<String>,
    /// One of `live_proto::maps::CATEGORIES`.
    pub category: Option<String>,
    /// A case-insensitive substring of the name, description, credited
    /// authors or release, base map or publisher's gamertag.
    pub q: Option<String>,
    pub cursor: Option<MapCursor>,
    pub limit: usize,
}

impl Default for MapQuery {
    fn default() -> Self {
        Self {
            sort: MapSort::New,
            base: None,
            author: None,
            category: None,
            q: None,
            cursor: None,
            limit: 100,
        }
    }
}

impl MapQuery {
    /// Reads the query string's parameters; empty ones count as absent.
    pub fn parse(q: &HashMap<String, String>) -> ApiResult<Self> {
        let get = |k: &str| {
            q.get(k)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let sort = match get("sort") {
            Some(s) => MapSort::parse(&s).ok_or_else(|| invalid("sort is new or popular"))?,
            None => MapSort::New,
        };
        let limit = get("limit")
            .map(|v| v.parse::<usize>())
            .transpose()
            .map_err(|_| invalid("invalid limit"))?
            .unwrap_or(100)
            .clamp(1, 1000);
        let cursor = get("cursor")
            .map(|c| MapCursor::parse(sort, &c).ok_or_else(|| invalid("bad cursor")))
            .transpose()?;
        let category = get("category");
        if let Some(c) = &category
            && !maps::CATEGORIES.contains(&c.as_str())
        {
            return Err(invalid(format!(
                "category is one of {}",
                maps::CATEGORIES.join(", ")
            )));
        }
        Ok(Self {
            sort,
            base: get("base"),
            author: get("author"),
            category,
            q: get("q"),
            cursor,
            limit,
        })
    }

    /// Whether an item with `downloads` at `position` comes after the
    /// cursor in this query's order (true without one).
    fn after(&self, downloads: u64, position: u64) -> bool {
        match self.cursor {
            None => true,
            Some(MapCursor::New(p)) => position < p,
            Some(MapCursor::Popular(d, p)) => (downloads, position) < (d, p),
        }
    }
}

/// A stored blob.
#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub size: u64,
    pub uploader: Xuid,
    pub stored_at: String,
}

/// A published package.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub doc: MapDocument,
    pub author: Xuid,
    pub published_at: String,
    pub position: u64,
}

/// A report, as admins read them.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub target: Target,
    pub id: String,
    pub by: Xuid,
    pub reason: String,
    pub at: String,
}

/// What the content events say, in memory.
#[derive(Debug, Default)]
pub struct CatalogState {
    pub blobs: HashMap<Sha256, Blob>,
    pub maps: HashMap<MapId, Package>,
    /// Moderated targets (blocks may come before any upload); absent: ok.
    pub standings: HashMap<(Target, String), Standing>,
    pub reports: Vec<Report>,
    pub uploads_off: BTreeSet<Xuid>,
    pub pins: BTreeMap<u32, Vec<MapId>>,
    /// The accounts that fetched each package's manifest (`MapDownloaded`).
    pub downloads: HashMap<MapId, HashSet<Xuid>>,
    /// Admins' credits (`MapCredited`), over the publishers'. Kept apart
    /// from [`Self::maps`] so a package deleted and published again keeps
    /// its correction.
    pub credits: HashMap<MapId, Credit>,
    /// Admins' details (`MapDetailed`), kept apart like the credits.
    pub details: HashMap<MapId, Details>,
    /// Map packs' info (`MapPackSet`), by id.
    pub packs: BTreeMap<String, PackInfo>,
}

impl CatalogState {
    pub fn standing(&self, target: Target, id: &str) -> Standing {
        self.standings
            .get(&(target, id.to_string()))
            .copied()
            .unwrap_or_default()
    }

    pub fn blob_standing(&self, h: &Sha256) -> Standing {
        self.standing(Target::Content, &h.to_string())
    }

    pub fn map_standing(&self, id: &MapId) -> Standing {
        self.standing(Target::Map, &id.to_string())
    }

    /// A blob that is stored and served.
    pub fn servable(&self, h: &Sha256) -> bool {
        self.blobs.contains_key(h) && self.blob_standing(h).is_ok()
    }

    /// Bytes `xuid` has stored (hidden ones count; deleted ones do not).
    pub fn used_by(&self, xuid: Xuid) -> u64 {
        self.blobs
            .iter()
            .filter(|(h, b)| {
                b.uploader == xuid
                    && matches!(self.blob_standing(h), Standing::Ok | Standing::Hidden)
            })
            .map(|(_, b)| b.size)
            .sum()
    }

    /// How many accounts fetched `id`'s manifest.
    pub fn downloads_of(&self, id: &MapId) -> u64 {
        self.downloads.get(id).map_or(0, |d| d.len() as u64)
    }

    /// Whether `xuid` has fetched `id`'s manifest before.
    pub fn downloaded(&self, id: &MapId, xuid: Xuid) -> bool {
        self.downloads.get(id).is_some_and(|d| d.contains(&xuid))
    }

    /// Who made `p` (published as `id`): an admin's credit, else the
    /// publisher's ([`MapDocument::credit`]).
    pub fn credit_of(&self, id: &MapId, p: &Package) -> Option<Credit> {
        self.credits.get(id).cloned().or_else(|| p.doc.credit())
    }

    /// The admin's details of `id`, if any.
    pub fn details_of(&self, id: &MapId) -> Option<&Details> {
        self.details.get(id)
    }

    /// The pack `id` names, with its info, if the server has it.
    pub fn pack_of(&self, id: &MapId) -> Option<PackListed> {
        let pack = self.details_of(id)?.pack.as_ref()?;
        self.packs.get(pack).map(|info| PackListed {
            id: pack.clone(),
            info: info.clone(),
        })
    }

    pub fn reports_of(&self, target: Target, id: &str) -> usize {
        self.reports
            .iter()
            .filter(|r| r.target == target && r.id == id)
            .count()
    }

    /// Why a session may not be on `id`, if an admin took it or one of its
    /// layers down. A `MapId` the server never saw is not refused here
    /// (retail maps and local edits have ids too).
    pub fn refusal(&self, id: &MapId) -> Option<String> {
        match self.map_standing(id) {
            Standing::Ok => {}
            Standing::Hidden => return Some(format!("map {id} is hidden by an admin")),
            Standing::Deleted => return Some(format!("map {id} was deleted by an admin")),
            Standing::Blocked => return Some(format!("map {id} is blocked by an admin")),
        }
        let p = self.maps.get(id)?;
        let m = &p.doc.manifest;
        m.patches
            .iter()
            .chain(&m.packs)
            .find(|l| {
                matches!(
                    self.blob_standing(&l.sha256),
                    Standing::Hidden | Standing::Blocked
                )
            })
            .map(|l| format!("map {id} uses content {} that an admin took down", l.sha256))
    }

    fn apply(&mut self, e: &RecordedEvent) {
        let b = &e.body;
        match b.kind.as_str() {
            CONTENT_STORED => {
                if let Ok(d) = serde_json::from_value::<ContentStored>(b.data.clone()) {
                    self.standings
                        .remove(&(Target::Content, d.sha256.to_string()));
                    self.blobs.entry(d.sha256).or_insert(Blob {
                        size: d.size,
                        uploader: d.uploader,
                        stored_at: b.at.clone(),
                    });
                }
            }
            MAP_PUBLISHED => {
                let Some(Author::Account { xuid, .. }) = Author::parse(&b.author) else {
                    return;
                };
                if let Ok(doc) = serde_json::from_value::<MapDocument>(b.data.clone()) {
                    let id = MapId::of(&doc.manifest);
                    self.standings.remove(&(Target::Map, id.to_string()));
                    self.maps.insert(
                        id,
                        Package {
                            doc,
                            author: xuid,
                            published_at: b.at.clone(),
                            position: e.position,
                        },
                    );
                }
            }
            CONTENT_MODERATED => {
                if let Ok(d) = serde_json::from_value::<Moderated>(b.data.clone()) {
                    let key = (d.target, d.id.clone());
                    let next = self
                        .standings
                        .get(&key)
                        .copied()
                        .unwrap_or_default()
                        .after(d.action);
                    if next == Standing::Ok {
                        self.standings.remove(&key);
                    } else {
                        self.standings.insert(key, next);
                    }
                    if d.target == Target::Map
                        && matches!(next, Standing::Deleted | Standing::Blocked)
                        && let Ok(id) = d.id.parse::<MapId>()
                    {
                        self.maps.remove(&id);
                    }
                }
            }
            CONTENT_REPORTED => {
                if let Ok(d) = serde_json::from_value::<Reported>(b.data.clone()) {
                    self.reports.push(Report {
                        target: d.target,
                        id: d.id,
                        by: d.by,
                        reason: d.reason,
                        at: b.at.clone(),
                    });
                }
            }
            CONTENT_UPLOADS_SET => {
                if let Ok(d) = serde_json::from_value::<UploadsSet>(b.data.clone()) {
                    if d.allowed {
                        self.uploads_off.remove(&d.xuid);
                    } else {
                        self.uploads_off.insert(d.xuid);
                    }
                }
            }
            MAP_DOWNLOADED => {
                if let Ok(d) = serde_json::from_value::<Downloaded>(b.data.clone()) {
                    self.downloads.entry(d.map_id).or_default().insert(d.by);
                }
            }
            MAP_CREDITED => {
                if let Ok(d) = serde_json::from_value::<Credited>(b.data.clone()) {
                    match d.credit.filter(|c| !c.is_empty()) {
                        Some(c) => {
                            self.credits.insert(d.map_id, c);
                        }
                        None => {
                            self.credits.remove(&d.map_id);
                        }
                    }
                }
            }
            MAP_DETAILED => {
                if let Ok(d) = serde_json::from_value::<Detailed>(b.data.clone()) {
                    match d.details.filter(|x| !x.is_empty()) {
                        Some(x) => {
                            self.details.insert(d.map_id, x);
                        }
                        None => {
                            self.details.remove(&d.map_id);
                        }
                    }
                }
            }
            MAP_PACK_SET => {
                if let Ok(d) = serde_json::from_value::<PackSet>(b.data.clone()) {
                    match d.pack {
                        Some(p) => {
                            self.packs.insert(d.id, p);
                        }
                        None => {
                            self.packs.remove(&d.id);
                        }
                    }
                }
            }
            HOPPER_PINS_SET => {
                if let Ok(d) = serde_json::from_value::<HopperPin>(b.data.clone()) {
                    if d.map_ids.is_empty() {
                        self.pins.remove(&d.hopper_id);
                    } else {
                        self.pins.insert(d.hopper_id, d.map_ids);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The content events' projection.
#[derive(Default)]
pub struct Catalog {
    state: RwLock<CatalogState>,
}

impl Catalog {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, CatalogState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Catalog {
    fn name(&self) -> &str {
        "content"
    }

    fn apply(&self, event: &RecordedEvent) {
        if maps::KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

/// The operator's list of retail map hashes ([`RETAIL_FILE`]).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Retail {
    pub hashes: HashSet<Sha256>,
    /// `MapId::retail(name, hash)` of each line that names its map.
    pub ids: HashSet<MapId>,
}

impl Retail {
    /// Parses `sha256sum` output. Lines that do not start with 64 hex
    /// digits are skipped.
    pub fn parse(text: &str) -> Self {
        let mut r = Retail::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (hex, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let Ok(h) = hex.parse::<Sha256>() else {
                continue;
            };
            r.hashes.insert(h);
            let name = rest.trim().trim_start_matches('*');
            let stem = Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if ::content::manifest::is_map_name(stem) {
                r.ids.insert(MapId::retail(stem, h));
            }
        }
        r
    }

    /// Reads `<data>/retail-maps.sha256`; none when it is absent.
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(data_dir.join(RETAIL_FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }
}

/// `<data>/content/<aa>/<sha256>`.
pub fn blob_path(root: &Path, h: &Sha256) -> PathBuf {
    let hex = h.to_string();
    root.join(&hex[..2]).join(hex)
}

/// `content/<aa>/<sha256>`, the blob's key in the blob store (in the data
/// directory, [`blob_path`] under it).
pub fn blob_key(h: &Sha256) -> String {
    let hex = h.to_string();
    format!("{CONTENT_DIR}/{}/{hex}", &hex[..2])
}

/// What an upload turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub sha256: Sha256,
    pub size: u64,
    /// A whole map file (the larger cap applied).
    pub whole_map: bool,
    /// False when the server already had it.
    pub stored: bool,
}

/// Streams a request body into `out` (nothing is stored until the caller
/// finishes it), hashing as it goes, and stops at the cap: `layer` bytes,
/// or `map` bytes when it starts as a whole map does.
async fn receive(
    body: Body,
    out: &mut crate::blobs::BlobWriter,
    layer: u64,
    map: u64,
) -> ApiResult<(Sha256, u64, bool)> {
    let mut body = body;
    let mut hasher = ::content::Hasher::default();
    let mut head = Vec::new();
    let mut size = 0u64;
    let over = |whole: bool| {
        too_large(if whole {
            format!("a whole map is at most {} MiB", map / MIB)
        } else {
            format!("a patch, pack or image is at most {} MiB", layer / MIB)
        })
    };
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|_| invalid("the upload was cut off"))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if head.len() < 4 {
            head.extend_from_slice(&data[..data.len().min(4 - head.len())]);
        }
        size += data.len() as u64;
        let whole = maps::is_whole_map(&head);
        if size > map || (size > layer && head.len() >= 4 && !whole) {
            return Err(over(whole));
        }
        hasher.update(&data);
        out.write(&data).await.map_err(|e| {
            eprintln!("live: content: {e}");
            unavailable("the content store cannot write")
        })?;
    }
    let whole = maps::is_whole_map(&head);
    if !whole && size > layer {
        return Err(over(false));
    }
    Ok((hasher.finish(), size, whole))
}

impl Live {
    pub(crate) fn content_store(&self) -> ApiResult<crate::blobs::BlobStore> {
        self.blobs()
            .map(|b| b.private.clone())
            .ok_or_else(|| unavailable("this server keeps no custom content (no data directory)"))
    }

    pub fn retail(&self) -> Retail {
        self.data_dir().map(Retail::load).unwrap_or_default()
    }

    /// An account allowed to upload, within its rate.
    fn uploader(&self, bearer: Option<&str>) -> ApiResult<(Author, Xuid)> {
        let author = self.authenticate(bearer, lxm::CONTENT_WRITE)?;
        let Author::Account { xuid, .. } = author else {
            return Err(forbidden("content is uploaded by an account"));
        };
        if self.catalog.read().uploads_off.contains(&xuid) {
            return Err(forbidden(
                "an admin has switched off uploads for this account",
            ));
        }
        if !self.uploads.allow(&author.to_string()) {
            return Err(ApiError::new(ErrorCode::RateLimited, "slow down"));
        }
        Ok((author, xuid))
    }

    /// `PUT /v1/content/{sha256}`.
    pub async fn put_content(
        &self,
        bearer: Option<&str>,
        sha256: &str,
        length: Option<u64>,
        body: Body,
    ) -> ApiResult<(StatusCode, Stored)> {
        let (_, xuid) = self.uploader(bearer)?;
        let h: Sha256 = sha256
            .parse()
            .map_err(|_| invalid("the path is a SHA-256 in hex"))?;
        let store = self.content_store()?;
        let limits = self.policy().content.clone();
        let known = |s: &CatalogState| -> ApiResult<Option<Stored>> {
            match s.blob_standing(&h) {
                Standing::Hidden | Standing::Blocked => {
                    return Err(gone("an admin took this content down"));
                }
                Standing::Ok | Standing::Deleted => {}
            }
            Ok(s.blobs.get(&h).filter(|_| s.servable(&h)).map(|b| Stored {
                sha256: h,
                size: b.size,
                whole_map: false,
                stored: false,
            }))
        };
        if let Some(have) = known(&self.catalog.read())? {
            return Ok((StatusCode::OK, have));
        }
        if self.retail().hashes.contains(&h) {
            return Err(forbidden(
                "that is a retail map: players use their own disc for it, it is never shared",
            ));
        }
        let quota = |extra: u64| -> ApiResult<()> {
            let used = self.catalog.read().used_by(xuid);
            if used + extra > limits.quota_bytes {
                return Err(too_large(format!(
                    "over this account's quota ({} of {} MiB used)",
                    used / MIB,
                    limits.quota_bytes / MIB
                )));
            }
            Ok(())
        };
        if let Some(n) = length {
            if n > limits.map_bytes {
                return Err(too_large(format!(
                    "a whole map is at most {} MiB",
                    limits.map_bytes / MIB
                )));
            }
            quota(n)?;
        }
        // Content is stored under its hash, which the path names, so the
        // body streams straight to its key; it shows there only once it
        // has checked out.
        let key = blob_key(&h);
        let meta = crate::blobs::Meta {
            content_type: Some("application/octet-stream".into()),
            ..Default::default()
        };
        let mut out = store.writer(&key, &meta).map_err(|e| {
            eprintln!("live: content: {key}: {e}");
            unavailable("the content store cannot write")
        })?;
        let got = receive(body, &mut out, limits.layer_bytes, limits.map_bytes).await;
        let mut out = Some(out);
        let result = async {
            let (actual, size, whole) = got?;
            self.meter(Some(xuid), crate::usage::metric::UP_CONTENT, size)
                .await;
            if actual != h {
                return Err(invalid(format!(
                    "the body's SHA-256 is {actual}, not the path's"
                )));
            }
            if size == 0 {
                return Err(invalid("empty content"));
            }
            quota(size)?;
            let _guard = self.content_writes.lock().await;
            if let Some(have) = known(&self.catalog.read())? {
                return Ok((StatusCode::OK, have));
            }
            let out = out.take().expect("not finished yet");
            out.finish().await.map_err(|e| {
                eprintln!("live: content: {key}: {e}");
                unavailable("the content store cannot write")
            })?;
            let data = ContentStored {
                sha256: h,
                size,
                uploader: xuid,
            };
            let event = self.server_event(
                maps::content_stream(&h),
                CONTENT_STORED,
                serde_json::to_value(&data).expect("serializes"),
            );
            let recorded = self.store.append(vec![event], &[]).await?;
            self.wait_applied(recorded[0].position).await?;
            Ok((
                StatusCode::CREATED,
                Stored {
                    sha256: h,
                    size,
                    whole_map: whole,
                    stored: true,
                },
            ))
        }
        .await;
        if let Some(out) = out {
            out.abort().await;
        }
        result
    }

    fn readable_content(&self, sha256: &str) -> ApiResult<Sha256> {
        let h: Sha256 = sha256.parse().map_err(|_| not_found("no such content"))?;
        {
            let s = self.catalog.read();
            match s.blob_standing(&h) {
                Standing::Ok => {}
                Standing::Hidden | Standing::Blocked => {
                    return Err(gone("an admin took this content down"));
                }
                Standing::Deleted => return Err(gone("an admin deleted this content")),
            }
            if !s.blobs.contains_key(&h) {
                return Err(not_found("no such content"));
            }
        }
        Ok(h)
    }

    /// Bounded image reads use the whole object.
    pub async fn content(&self, sha256: &str) -> ApiResult<(Sha256, Vec<u8>)> {
        let h = self.readable_content(sha256)?;
        let key = blob_key(&h);
        match self.content_store()?.get(&key).await {
            Ok(Some(bytes)) => Ok((h, bytes.to_vec())),
            Ok(None) => Err(not_found("no such content")),
            Err(e) => {
                eprintln!("live: content: {key}: {e}");
                Err(unavailable("the content store cannot read"))
            }
        }
    }

    fn who(&self, xuid: Xuid) -> Who {
        Who {
            xuid,
            gamertag: self
                .model
                .read()
                .accounts
                .get(&xuid)
                .map(|a| a.gamertag.to_string())
                .unwrap_or_default(),
        }
    }

    fn map_summary(&self, s: &CatalogState, id: &MapId, p: &Package) -> MapSummary {
        let m = &p.doc.manifest;
        let layers: Vec<LayerInfo> = m
            .patches
            .iter()
            .map(|l| (l, "patch"))
            .chain(m.packs.iter().map(|l| (l, "pack")))
            .map(|(l, kind)| LayerInfo {
                sha256: l.sha256,
                size: s.blobs.get(&l.sha256).map_or(0, |b| b.size),
                kind: kind.into(),
            })
            .collect();
        let credit = s.credit_of(id, p);
        let details = s.details_of(id);
        let images = details.map(|d| d.images.clone()).unwrap_or_default();
        MapSummary {
            map_id: *id,
            name: m.package.name.clone(),
            description: details
                .and_then(|d| d.description.clone())
                .or_else(|| m.package.description.clone()),
            author: self.who(p.author),
            base: BaseInfo {
                map: m.base.map.clone(),
                sha256: m.base.sha256,
                derived: m.base.derived,
            },
            size: layers.iter().map(|l| l.size).sum(),
            layers,
            image: p.doc.image.or_else(|| images.first().map(|i| i.sha256)),
            features: details
                .and_then(|d| d.features.clone())
                .unwrap_or_else(|| p.doc.features.clone()),
            gametypes: p.doc.gametypes.clone(),
            version: p.doc.version.clone(),
            geometry: m.result.as_ref().and_then(|r| r.geometry),
            published_at: p.published_at.clone(),
            position: p.position,
            downloads: s.downloads_of(id),
            credit: credit.as_ref().and_then(Credit::names),
            category: p.doc.category.clone(),
            source_url: credit
                .as_ref()
                .and_then(|c| c.source_url.clone())
                .or_else(|| p.doc.source_url.clone()),
            credits: credit,
            contents: details
                .and_then(|d| d.contents.as_ref())
                .map(|c| c.lines.clone())
                .unwrap_or_default(),
            images,
            pack: details.and_then(|d| d.pack.clone()),
            pack_name: details
                .and_then(|d| d.pack.as_ref())
                .and_then(|p| s.packs.get(p))
                .map(|p| p.name.clone()),
        }
    }

    /// `PUT /v1/maps`: a `MapPublished` event signed by the package's
    /// author.
    pub async fn publish_map(
        &self,
        bearer: Option<&str>,
        jws: &str,
    ) -> ApiResult<(StatusCode, MapAnswer)> {
        let (author, xuid) = self.uploader(bearer)?;
        let event = SignedEvent::open(jws).map_err(sign_error)?;
        let body = &event.body;
        if body.author != author.to_string() {
            return Err(forbidden("the manifest's author is not the token's issuer"));
        }
        let key = self
            .key_for(&author, &body.kid)
            .ok_or_else(|| crate::app::unauthorized("the manifest's key is unknown or revoked"))?;
        event.verify(&key).map_err(sign_error)?;
        if body.kind != MAP_PUBLISHED {
            return Err(invalid(format!("a package is a {MAP_PUBLISHED} event")));
        }
        let clock = now();
        let at = parse_time(&body.at)
            .ok_or_else(|| invalid("at is not an RFC 3339 time"))?
            .unix_timestamp();
        if (at - clock).abs() > MAX_EVENT_SKEW_S {
            return Err(invalid("at is more than 5 minutes from the server's clock"));
        }
        let doc: MapDocument = serde_json::from_value(body.data.clone())
            .map_err(|e| invalid(format!("manifest: {e}")))?;
        let m = &doc.manifest;
        m.validate().map_err(|e| invalid(e.to_string()))?;
        doc.validate_extras().map_err(invalid)?;
        if m.package.image.is_some() != doc.image.is_some() {
            return Err(invalid(
                "image: give the image's SHA-256 exactly when the manifest names one",
            ));
        }
        let id = MapId::of(m);
        if body.stream != maps::package_stream(&id) {
            return Err(invalid(format!(
                "the stream is {}, the manifest's MapId says {}",
                body.stream,
                maps::package_stream(&id)
            )));
        }
        let retail = self.retail();
        if !m.base.derived && m.patches.is_empty() && m.packs.is_empty() {
            return Err(forbidden(
                "a manifest with only a retail base is the retail map: there is nothing to share",
            ));
        }
        if let Some(l) = m
            .patches
            .iter()
            .chain(&m.packs)
            .find(|l| retail.hashes.contains(&l.sha256))
        {
            return Err(forbidden(format!(
                "layer {} is a retail map: it is never shared",
                l.file
            )));
        }
        let _guard = self.content_writes.lock().await;
        {
            let s = self.catalog.read();
            match s.map_standing(&id) {
                Standing::Hidden | Standing::Blocked => {
                    return Err(gone("an admin took this map down"));
                }
                Standing::Ok | Standing::Deleted => {}
            }
            if let Some(p) = s.maps.get(&id) {
                if p.author != xuid {
                    return Err(conflict(format!(
                        "this map is already published by {}",
                        self.who(p.author).gamertag
                    )));
                }
                let answer = self.answer(&s, &id, p);
                return Ok((StatusCode::OK, answer));
            }
            let missing: Vec<String> = m
                .patches
                .iter()
                .chain(&m.packs)
                .filter(|l| !s.servable(&l.sha256))
                .map(|l| format!("{} ({})", l.file, l.sha256))
                .collect();
            if !missing.is_empty() {
                return Err(conflict(format!(
                    "upload these layers first: {}",
                    missing.join(", ")
                )));
            }
            if let Some(img) = &doc.image {
                if !s.servable(img) {
                    return Err(conflict(format!("upload the image first ({img})")));
                }
                if s.blobs[img].size > MAX_IMAGE_BYTES {
                    return Err(too_large(format!(
                        "the image is over {} KiB",
                        MAX_IMAGE_BYTES / 1024
                    )));
                }
            }
        }
        if let Some(img) = &doc.image {
            let head = self
                .content_store()?
                .get_prefix(&blob_key(img), 8)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| unavailable("the content store cannot read"))?;
            if !maps::is_image(&head) {
                return Err(invalid("the image is not a PNG or JPEG"));
            }
        }
        if !self.model.nonces.insert_new(
            nonce_key(&body.author, &body.nonce),
            at + MAX_EVENT_SKEW_S,
            clock,
        ) {
            return Err(conflict("nonce already used"));
        }
        let new = NewEvent {
            jws: jws.into(),
            jws_index: None,
            body: event.body.clone(),
        };
        let recorded = self.store.append(vec![new], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        let s = self.catalog.read();
        let p = s.maps.get(&id).ok_or_else(|| unavailable("not applied"))?;
        Ok((StatusCode::CREATED, self.answer(&s, &id, p)))
    }

    fn answer(&self, s: &CatalogState, id: &MapId, p: &Package) -> MapAnswer {
        let m = &p.doc.manifest;
        MapAnswer {
            map: MapDetail {
                summary: self.map_summary(s, id, p),
                manifest: m.clone(),
                report: s.details_of(id).and_then(|d| d.contents.clone()),
                pack_info: s.pack_of(id),
            },
            missing: m
                .patches
                .iter()
                .chain(&m.packs)
                .map(|l| l.sha256)
                .filter(|h| !s.servable(h))
                .collect(),
            position: self.position(),
        }
    }

    /// `GET /v1/maps/{map_id}`.
    pub fn map(&self, id: &str) -> ApiResult<MapAnswer> {
        let id: MapId = id.parse().map_err(|_| not_found("no such map"))?;
        let s = self.catalog.read();
        if let Some(why) = s.refusal(&id) {
            return Err(gone(why));
        }
        let p = s.maps.get(&id).ok_or_else(|| not_found("no such map"))?;
        Ok(self.answer(&s, &id, p))
    }

    /// `GET /v1/maps/{map_id}/image`.
    pub async fn map_image(&self, id: &str) -> ApiResult<(&'static str, Vec<u8>)> {
        let img = self
            .map(id)?
            .map
            .summary
            .image
            .ok_or_else(|| not_found("this map has no image"))?;
        let (_, bytes) = self.content(&img.to_string()).await?;
        let kind = maps::image_type(&bytes).ok_or_else(|| not_found("this map has no image"))?;
        Ok((kind, bytes))
    }

    /// `GET /v1/maps/{map_id}/images/{n}`: the admin's `n`th screenshot
    /// (from 0).
    pub async fn map_detail_image(&self, id: &str, n: &str) -> ApiResult<(&'static str, Vec<u8>)> {
        let n: usize = n.parse().map_err(|_| not_found("no such image"))?;
        let img = self
            .map(id)?
            .map
            .summary
            .images
            .get(n)
            .map(|i| i.sha256)
            .ok_or_else(|| not_found("no such image"))?;
        let (_, bytes) = self.content(&img.to_string()).await?;
        let kind = maps::image_type(&bytes).ok_or_else(|| not_found("no such image"))?;
        Ok((kind, bytes))
    }

    /// `GET /v1/maps/{map_id}` by a signed-in console: counts `xuid` as a
    /// download of `id` the first time (a server-signed `MapDownloaded`).
    /// A failure to record is logged, never the reader's error.
    pub async fn count_download(&self, id: &MapId, xuid: Xuid) {
        {
            let s = self.catalog.read();
            if !s.maps.contains_key(id) || s.downloaded(id, xuid) {
                return;
            }
        }
        let data = Downloaded {
            map_id: *id,
            by: xuid,
        };
        let event = self.server_event(
            maps::package_stream(id),
            MAP_DOWNLOADED,
            serde_json::to_value(&data).expect("serializes"),
        );
        match self.store.append(vec![event], &[]).await {
            Ok(recorded) => {
                let _ = self.wait_applied(recorded[0].position).await;
            }
            Err(e) => eprintln!("live: map {id}: download not counted: {e}"),
        }
    }

    /// `GET /v1/maps`: published maps by `query` (docs/live/live-api.md
    /// "Custom maps"), newest first or most downloaded first.
    pub fn maps(&self, query: &MapQuery) -> Page<MapSummary> {
        let position = self.position();
        let s = self.catalog.read();
        let author = query.author.as_deref().map(|a| {
            Gamertag::new(a)
                .ok()
                .and_then(|t| self.model.read().account_by_tag(&t).map(|a| a.xuid))
        });
        let needle = query.q.as_deref().map(str::to_lowercase);
        let mut all: Vec<(&MapId, &Package, u64)> = s
            .maps
            .iter()
            .filter(|(id, p)| {
                let m = &p.doc.manifest;
                s.refusal(id).is_none()
                    && query
                        .base
                        .as_deref()
                        .is_none_or(|b| m.base.map.eq_ignore_ascii_case(b))
                    && author.is_none_or(|a| a == Some(p.author))
                    && query
                        .category
                        .as_deref()
                        .is_none_or(|c| p.doc.category.as_deref() == Some(c))
                    && needle.as_deref().is_none_or(|n| {
                        let has = |t: &str| t.to_lowercase().contains(n);
                        let credit = s.credit_of(id, p).unwrap_or_default();
                        has(m.package.name.as_str())
                            || m.package.description.as_deref().is_some_and(has)
                            || s.details_of(id)
                                .and_then(|d| d.description.as_deref())
                                .is_some_and(has)
                            || credit.authors.iter().any(|a| has(a.as_str()))
                            || credit.release.as_deref().is_some_and(has)
                            || has(m.base.map.as_str())
                            || has(self.who(p.author).gamertag.as_str())
                    })
            })
            .map(|(id, p)| (id, p, s.downloads_of(id)))
            .filter(|(_, p, d)| query.after(*d, p.position))
            .collect();
        match query.sort {
            MapSort::New => all.sort_by_key(|(_, p, _)| std::cmp::Reverse(p.position)),
            MapSort::Popular => {
                all.sort_by_key(|(_, p, d)| std::cmp::Reverse((*d, p.position)));
            }
        }
        all.truncate(query.limit);
        let next = (all.len() == query.limit)
            .then(|| {
                all.last()
                    .map(|(_, p, d)| MapCursor::of(query.sort, *d, p.position).to_string())
            })
            .flatten();
        Page {
            items: all
                .iter()
                .map(|(id, p, _)| self.map_summary(&s, id, p))
                .collect(),
            next,
            position,
        }
    }

    /// `POST /v1/maps/{map_id}/report`, `POST /v1/content/{sha256}/report`.
    pub async fn report(
        &self,
        bearer: Option<&str>,
        target: Target,
        id: &str,
        reason: &str,
    ) -> ApiResult<u64> {
        let author = self.authenticate(bearer, lxm::CONTENT_WRITE)?;
        let Author::Account { xuid, .. } = author else {
            return Err(forbidden("reports come from accounts"));
        };
        self.rate_limit(&author, lxm::CONTENT_WRITE)?;
        let reason = reason.trim();
        if reason.chars().count() > maps::MAX_REASON || reason.contains('\0') {
            return Err(invalid(format!(
                "a reason is at most {} characters",
                maps::MAX_REASON
            )));
        }
        let (stream, id) = {
            let s = self.catalog.read();
            match target {
                Target::Map => {
                    let id: MapId = id.parse().map_err(|_| not_found("no such map"))?;
                    if !s.maps.contains_key(&id) {
                        return Err(not_found("no such map"));
                    }
                    (maps::package_stream(&id), id.to_string())
                }
                Target::Content => {
                    let h: Sha256 = id.parse().map_err(|_| not_found("no such content"))?;
                    if !s.blobs.contains_key(&h) {
                        return Err(not_found("no such content"));
                    }
                    (maps::content_stream(&h), h.to_string())
                }
            }
        };
        let data = Reported {
            target,
            id,
            by: xuid,
            reason: reason.into(),
        };
        let event = self.server_event(
            stream,
            CONTENT_REPORTED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// An admin (or the operator) with a `content.moderate` token.
    fn moderator(&self, bearer: Option<&str>) -> ApiResult<Option<Xuid>> {
        let author = self.authenticate(bearer, lxm::CONTENT_MODERATE)?;
        crate::admin::require_admin(self, &author)
            .map_err(|_| forbidden("moderating content needs the operator or an admin"))?;
        self.rate_limit(&author, lxm::CONTENT_MODERATE)?;
        Ok(match author {
            Author::Account { xuid, .. } => Some(xuid),
            Author::Server(_) => None,
        })
    }

    /// `POST /v1/admin/{maps|content}/{id}/{action}`.
    pub async fn moderate(
        &self,
        bearer: Option<&str>,
        target: Target,
        id: &str,
        action: &str,
    ) -> ApiResult<u64> {
        let by = self.moderator(bearer)?;
        let action = Action::parse(action)
            .ok_or_else(|| invalid("the action is hide, unhide, delete, block or unblock"))?;
        let _guard = self.content_writes.lock().await;
        let (stream, id, known) = {
            let s = self.catalog.read();
            match target {
                Target::Map => {
                    let id: MapId = id
                        .parse()
                        .map_err(|_| invalid("a MapId is 64 hex digits"))?;
                    let known = s.maps.contains_key(&id)
                        || s.standings.contains_key(&(Target::Map, id.to_string()));
                    (maps::package_stream(&id), id.to_string(), known)
                }
                Target::Content => {
                    let h: Sha256 = id
                        .parse()
                        .map_err(|_| invalid("a SHA-256 is 64 hex digits"))?;
                    let known = s.blobs.contains_key(&h)
                        || s.standings.contains_key(&(Target::Content, h.to_string()));
                    (maps::content_stream(&h), h.to_string(), known)
                }
            }
        };
        // A block may come first (a hash an admin knows is bad); the rest
        // need something to act on.
        if !known && action != Action::Block {
            return Err(not_found("no such content"));
        }
        if target == Target::Content && matches!(action, Action::Delete | Action::Block) {
            let h: Sha256 = id.parse().expect("checked");
            if let Ok(store) = self.content_store() {
                let _ = store.delete(&blob_key(&h)).await;
            }
        }
        let data = Moderated {
            target,
            id,
            action,
            by,
        };
        let event = self.server_event(
            stream,
            CONTENT_MODERATED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// `PUT /v1/maps/{map_id}/credit`: an admin sets who made a published
    /// package (a server-signed `MapCredited`); an empty credit clears the
    /// admin's and the publisher's shows again. The manifest and its
    /// signature are left as published.
    pub async fn set_credit(
        &self,
        bearer: Option<&str>,
        id: &str,
        credit: Credit,
    ) -> ApiResult<(u64, MapSummary)> {
        let by = self.moderator(bearer)?;
        let id: MapId = id
            .parse()
            .map_err(|_| invalid("a MapId is 64 hex digits"))?;
        let credit = credit.tidy();
        credit.validate().map_err(invalid)?;
        let _guard = self.content_writes.lock().await;
        if !self.catalog.read().maps.contains_key(&id) {
            return Err(not_found("no such map"));
        }
        let data = Credited {
            map_id: id,
            credit: (!credit.is_empty()).then_some(credit),
            by,
        };
        let event = self.server_event(
            maps::package_stream(&id),
            MAP_CREDITED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        let s = self.catalog.read();
        let p = s.maps.get(&id).ok_or_else(|| not_found("no such map"))?;
        Ok((recorded[0].position, self.map_summary(&s, &id, p)))
    }

    /// `PUT /v1/maps/{map_id}/details`: an admin sets a published
    /// package's details (a server-signed `MapDetailed`): a description,
    /// screenshots already uploaded as content (each must be a stored,
    /// served PNG or JPEG), a contents report, features. Empty details
    /// clear the admin's. The manifest and its signature are left as
    /// published.
    pub async fn set_details(
        &self,
        bearer: Option<&str>,
        id: &str,
        details: Details,
    ) -> ApiResult<(u64, MapSummary)> {
        let by = self.moderator(bearer)?;
        let id: MapId = id
            .parse()
            .map_err(|_| invalid("a MapId is 64 hex digits"))?;
        let mut details = details.tidy();
        if let Some(f) = details.features.take() {
            let ordered = maps::in_order(&f, &maps::FEATURES);
            if ordered.len() != f.len() {
                return Err(invalid(
                    "features: each must be one of the checklist's, once",
                ));
            }
            details.features = Some(ordered);
        }
        details.validate().map_err(invalid)?;
        if !self.catalog.read().maps.contains_key(&id) {
            return Err(not_found("no such map"));
        }
        self.check_images(&details.images).await?;
        let _guard = self.content_writes.lock().await;
        if !self.catalog.read().maps.contains_key(&id) {
            return Err(not_found("no such map"));
        }
        let data = Detailed {
            map_id: id,
            details: (!details.is_empty()).then_some(details),
            by,
        };
        let event = self.server_event(
            maps::package_stream(&id),
            MAP_DETAILED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        let s = self.catalog.read();
        let p = s.maps.get(&id).ok_or_else(|| not_found("no such map"))?;
        Ok((recorded[0].position, self.map_summary(&s, &id, p)))
    }

    /// The images `images` name are stored, served PNGs or JPEGs.
    async fn check_images(&self, images: &[maps::MapImage]) -> ApiResult<()> {
        for i in images {
            let (_, bytes) = self
                .content(&i.sha256.to_string())
                .await
                .map_err(|_| invalid(format!("image {} is not stored content", i.sha256)))?;
            if maps::image_type(&bytes).is_none() {
                return Err(invalid(format!("image {} is not a PNG or JPEG", i.sha256)));
            }
        }
        Ok(())
    }

    /// `PUT /v1/mappacks/{id}`: an admin sets a map pack's info (a
    /// server-signed `MapPackSet`); None clears it.
    pub async fn set_pack(
        &self,
        bearer: Option<&str>,
        id: &str,
        pack: Option<PackInfo>,
    ) -> ApiResult<u64> {
        let by = self.moderator(bearer)?;
        if !maps::pack_id_ok(id) {
            return Err(invalid("a pack id is 1 to 64 of a-z, 0-9 and -"));
        }
        let pack = pack.map(PackInfo::tidy);
        if let Some(p) = &pack {
            p.validate().map_err(invalid)?;
            self.check_images(&p.images).await?;
        }
        let _guard = self.content_writes.lock().await;
        let data = PackSet {
            id: id.to_string(),
            pack,
            by,
        };
        let event = self.server_event(
            maps::pack_stream(id),
            MAP_PACK_SET,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// `GET /v1/mappacks`.
    pub fn packs(&self) -> Vec<PackListed> {
        self.catalog
            .read()
            .packs
            .iter()
            .map(|(id, info)| PackListed {
                id: id.clone(),
                info: info.clone(),
            })
            .collect()
    }

    /// The packs `items` name, for a list page.
    pub fn packs_of(&self, items: &[MapSummary]) -> Vec<PackListed> {
        let s = self.catalog.read();
        let ids: BTreeSet<&String> = items.iter().filter_map(|m| m.pack.as_ref()).collect();
        ids.into_iter()
            .filter_map(|id| {
                s.packs.get(id).map(|info| PackListed {
                    id: id.clone(),
                    info: info.clone(),
                })
            })
            .collect()
    }

    /// `GET /v1/mappacks/{id}/images/{n}`.
    pub async fn pack_image(&self, id: &str, n: &str) -> ApiResult<(&'static str, Vec<u8>)> {
        let n: usize = n.parse().map_err(|_| not_found("no such image"))?;
        let img = self
            .catalog
            .read()
            .packs
            .get(id)
            .and_then(|p| p.images.get(n))
            .map(|i| i.sha256)
            .ok_or_else(|| not_found("no such image"))?;
        let (_, bytes) = self.content(&img.to_string()).await?;
        let kind = maps::image_type(&bytes).ok_or_else(|| not_found("no such image"))?;
        Ok((kind, bytes))
    }

    /// `PUT /v1/admin/uploads/{gamertag}`.
    pub async fn set_uploads(
        &self,
        bearer: Option<&str>,
        tag: &str,
        allowed: bool,
    ) -> ApiResult<u64> {
        self.moderator(bearer)?;
        let tag = Gamertag::new(tag).map_err(|_| not_found("no such account"))?;
        let xuid = self
            .model
            .read()
            .account_by_tag(&tag)
            .map(|a| a.xuid)
            .ok_or_else(|| not_found("no such account"))?;
        let event = self.server_event(
            streams::account(xuid),
            CONTENT_UPLOADS_SET,
            serde_json::to_value(UploadsSet { xuid, allowed }).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// `PUT /v1/hoppers/{hopper_id}/pins`.
    pub async fn set_pins(
        &self,
        bearer: Option<&str>,
        hopper_id: u32,
        map_ids: Vec<MapId>,
    ) -> ApiResult<u64> {
        self.moderator(bearer)?;
        let pin = HopperPin { hopper_id, map_ids };
        pin.validate().map_err(invalid)?;
        let event = self.server_event(
            maps::hopper_stream(hopper_id),
            HOPPER_PINS_SET,
            serde_json::to_value(&pin).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// Whether a session may advertise `map_id` (docs/game/maps.md §5): not on
    /// content an admin took down, and with ranked flags only a retail map
    /// (from [`RETAIL_FILE`]) or one pinned on the hopper.
    pub(crate) fn check_session_map(
        &self,
        flags: &str,
        hopper_id: Option<u32>,
        map_id: Option<&MapId>,
    ) -> ApiResult<()> {
        let Some(id) = map_id else {
            return Ok(());
        };
        let s = self.catalog.read();
        if let Some(why) = s.refusal(id) {
            return Err(gone(why));
        }
        if maps::ranked_flags(flags) {
            let pinned = hopper_id
                .and_then(|h| s.pins.get(&h))
                .is_some_and(|p| p.contains(id));
            if !pinned && !self.retail().ids.contains(id) {
                return Err(forbidden(
                    "a matchmaking session is on a retail map or one pinned on its hopper",
                ));
            }
        }
        Ok(())
    }
}

/// The admin's view (`GET /v1/admin/content`).
#[derive(Debug, Serialize)]
struct AdminView {
    maps: Vec<AdminMap>,
    blobs: Vec<AdminBlob>,
    reports: Vec<AdminReport>,
    uploads_off: Vec<String>,
    position: u64,
}

#[derive(Debug, Serialize)]
struct AdminMap {
    #[serde(flatten)]
    summary: MapSummary,
    state: Standing,
    reports: usize,
}

#[derive(Debug, Serialize)]
struct AdminBlob {
    sha256: Sha256,
    size: u64,
    uploader: Who,
    state: Standing,
    reports: usize,
    stored_at: String,
}

#[derive(Debug, Serialize)]
struct AdminReport {
    target: Target,
    id: String,
    by: String,
    reason: String,
    at: String,
}

impl Live {
    fn admin_view(&self) -> AdminView {
        let position = self.position();
        let s = self.catalog.read();
        let mut maps: Vec<(&MapId, &Package)> = s.maps.iter().collect();
        maps.sort_by_key(|(_, p)| std::cmp::Reverse(p.position));
        let maps = maps
            .into_iter()
            .map(|(id, p)| AdminMap {
                summary: self.map_summary(&s, id, p),
                state: s.map_standing(id),
                reports: s.reports_of(Target::Map, &id.to_string()),
            })
            .collect();
        let mut blobs: Vec<AdminBlob> = s
            .blobs
            .iter()
            .map(|(h, b)| AdminBlob {
                sha256: *h,
                size: b.size,
                uploader: self.who(b.uploader),
                state: s.blob_standing(h),
                reports: s.reports_of(Target::Content, &h.to_string()),
                stored_at: b.stored_at.clone(),
            })
            .collect();
        blobs.sort_by(|a, b| b.stored_at.cmp(&a.stored_at).then(a.sha256.cmp(&b.sha256)));
        let reports = s
            .reports
            .iter()
            .rev()
            .map(|r| AdminReport {
                target: r.target,
                id: r.id.clone(),
                by: self.who(r.by).gamertag,
                reason: r.reason.clone(),
                at: r.at.clone(),
            })
            .collect();
        let uploads_off = s
            .uploads_off
            .iter()
            .map(|x| self.who(*x).gamertag)
            .collect();
        AdminView {
            maps,
            blobs,
            reports,
            uploads_off,
            position,
        }
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    crate::http::bearer(headers)
}

fn ok(position: u64) -> Response {
    crate::http::json(StatusCode::OK, &json!({"ok": true, "position": position}))
}

fn body_text(body: Result<Bytes, BytesRejection>) -> ApiResult<Bytes> {
    body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => too_large("request body too large"),
        _ => invalid(e.body_text()),
    })
}

pub(crate) async fn put_content(
    State(live): Shared,
    UrlPath(sha256): UrlPath<String>,
    req: Request,
) -> ApiResult<Response> {
    let headers = req.headers().clone();
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let (status, stored) = live
        .put_content(bearer(&headers), &sha256, length, req.into_body())
        .await?;
    Ok(crate::http::json(status, &stored))
}

pub(crate) async fn get_content(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(sha256): UrlPath<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let h = live.readable_content(&sha256)?;
    let key = blob_key(&h);
    let (size, stream) = live
        .content_store()?
        .get_stream(&key)
        .await
        .map_err(|e| {
            eprintln!("live: content: {key}: {e}");
            unavailable("the content store cannot read")
        })?
        .ok_or_else(|| not_found("no such content"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::ETAG, format!("\"{h}\"")),
            (header::CONTENT_LENGTH, size.to_string()),
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_string(),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub(crate) async fn put_map(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let jws = std::str::from_utf8(&bytes)
        .map_err(|_| invalid("the body must be a compact JWS"))?
        .trim();
    let (status, answer) = live.publish_map(bearer(&headers), jws).await?;
    Ok(crate::http::json(status, &answer))
}

pub(crate) async fn get_map(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Response> {
    // A signed-in reader (the console's download) counts once per account.
    // Tokens are single use, so it is checked once: one that does not
    // check out is no download, and on a public server no error either
    // (the manifest is public); a private server refuses the read.
    let reader = bearer(&headers).and_then(|b| live.authenticate(Some(b), lxm::READ).ok());
    if reader.is_none() {
        live.authorize_read(None)?;
    }
    let mut answer = live.map(&id)?;
    if let Some(Author::Account { xuid, .. }) = reader {
        let id = answer.map.summary.map_id;
        live.count_download(&id, xuid).await;
        answer.map.summary.downloads = live.catalog.read().downloads_of(&id);
    }
    Ok(crate::http::json(StatusCode::OK, &answer))
}

pub(crate) async fn get_map_image(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let (kind, bytes) = live.map_image(&id).await?;
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

pub(crate) async fn list_maps(
    State(live): Shared,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let page = live.maps(&MapQuery::parse(&q)?);
    // The packs the page's maps name, said once for the page.
    let packs = live.packs_of(&page.items);
    let mut v = serde_json::to_value(&page).expect("serializes");
    if !packs.is_empty() {
        v["packs"] = serde_json::to_value(&packs).expect("serializes");
    }
    Ok(crate::http::json(StatusCode::OK, &v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    #[serde(default)]
    reason: String,
}

async fn report(
    live: &Live,
    headers: &HeaderMap,
    target: Target,
    id: &str,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let r: ReportBody = if bytes.iter().all(u8::is_ascii_whitespace) {
        ReportBody {
            reason: String::new(),
        }
    } else {
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("report: {e}")))?
    };
    let pos = live.report(bearer(headers), target, id, &r.reason).await?;
    Ok(crate::http::json(
        StatusCode::CREATED,
        &json!({"ok": true, "position": pos}),
    ))
}

pub(crate) async fn report_map(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    report(&live, &headers, Target::Map, &id, body).await
}

pub(crate) async fn report_content(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    report(&live, &headers, Target::Content, &id, body).await
}

pub(crate) async fn admin_content(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.moderator(bearer(&headers))?;
    Ok(crate::http::json(StatusCode::OK, &live.admin_view()))
}

pub(crate) async fn moderate_map(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, action)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    Ok(ok(live
        .moderate(bearer(&headers), Target::Map, &id, &action)
        .await?))
}

pub(crate) async fn moderate_content(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, action)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    Ok(ok(live
        .moderate(bearer(&headers), Target::Content, &id, &action)
        .await?))
}

pub(crate) async fn put_credit(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let credit: Credit =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("credit: {e}")))?;
    let (position, map) = live.set_credit(bearer(&headers), &id, credit).await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position, "map": map}),
    ))
}

pub(crate) async fn put_details(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let details: Details =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("details: {e}")))?;
    let (position, map) = live.set_details(bearer(&headers), &id, details).await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position, "map": map}),
    ))
}

pub(crate) async fn put_pack(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let pack: Option<PackInfo> =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("pack: {e}")))?;
    Ok(ok(live.set_pack(bearer(&headers), &id, pack).await?))
}

pub(crate) async fn list_packs(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"items": live.packs(), "position": live.position()}),
    ))
}

pub(crate) async fn get_pack_image(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, n)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let (kind, bytes) = live.pack_image(&id, &n).await?;
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

pub(crate) async fn get_map_detail_image(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, n)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let (kind, bytes) = live.map_detail_image(&id, &n).await?;
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadsBody {
    allowed: bool,
}

pub(crate) async fn put_uploads(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(tag): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let b: UploadsBody =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("uploads: {e}")))?;
    Ok(ok(live
        .set_uploads(bearer(&headers), &tag, b.allowed)
        .await?))
}

pub(crate) async fn get_pins(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let pins: Vec<HopperPin> = live
        .catalog
        .read()
        .pins
        .iter()
        .map(|(h, ids)| HopperPin {
            hopper_id: *h,
            map_ids: ids.clone(),
        })
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"pins": pins, "position": live.position()}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinsBody {
    map_ids: Vec<MapId>,
}

pub(crate) async fn put_pins(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(hopper): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let hopper: u32 = hopper
        .parse()
        .map_err(|_| invalid("a hopper id is a number"))?;
    let bytes = body_text(body)?;
    let b: PinsBody = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("pins: {e}")))?;
    Ok(ok(live
        .set_pins(bearer(&headers), hopper, b.map_ids)
        .await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_lists_read_sha256sum_output() {
        let a = "aa".repeat(32);
        let b = "BB".repeat(32);
        let text = format!(
            "# retail maps\n{a}  lockout.map\n{b} *maps/Midship.map\n\nnot a hash line\n{}\n",
            "cc".repeat(32)
        );
        let r = Retail::parse(&text);
        assert_eq!(r.hashes.len(), 3);
        let ha: Sha256 = a.parse().unwrap();
        let hb: Sha256 = b.parse().unwrap();
        assert!(r.ids.contains(&MapId::retail("lockout", ha)));
        assert!(
            r.ids.contains(&MapId::retail("midship", hb)),
            "names match any case"
        );
        assert_eq!(r.ids.len(), 2, "a bare hash names no map");
        assert_eq!(Retail::parse(""), Retail::default());
    }

    #[test]
    fn limits_from_flags() {
        let d = ContentLimits::default();
        assert_eq!(d.layer_bytes, 95 * MIB);
        assert_eq!(d.map_bytes, 95 * MIB);
        let l = ContentLimits::from_flags(Some("8".into()), None, Some("2048".into())).unwrap();
        assert_eq!(
            (l.layer_bytes, l.map_bytes, l.quota_bytes),
            (8 * MIB, 95 * MIB, 2048 * MIB)
        );
        assert!(ContentLimits::from_flags(Some("0".into()), None, None).is_err());
        assert!(ContentLimits::from_flags(None, Some("lots".into()), None).is_err());
    }

    #[test]
    fn the_default_caps_fit_the_edge_and_the_games_download() {
        // The Worker takes ContentLimits::default() (it has no flags), so
        // a body the native server takes by default reaches the Worker
        // too, and is the largest whole MiB that does.
        let d = ContentLimits::default();
        for cap in [d.layer_bytes, d.map_bytes] {
            assert!(cap <= EDGE_BODY_BYTES);
            assert!(cap + MIB > EDGE_BODY_BYTES);
            assert_eq!(cap % MIB, 0);
        }
        // A 68.2 MiB layer (Night Ops' delta over Containment) fits.
        assert!(68 * MIB + MIB / 4 <= d.layer_bytes);
    }

    #[test]
    fn blobs_fan_out_by_their_first_byte() {
        let h = Sha256([0xab; 32]);
        assert_eq!(
            blob_path(Path::new("/d/content"), &h),
            Path::new("/d/content/ab").join("ab".repeat(32))
        );
    }
}
