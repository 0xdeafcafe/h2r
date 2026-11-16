//! Cloud saves (docs/live/live-cloud-saves.md, docs/live/live-api.md "Cloud
//! saves"): each account's own versioned, content-addressed items.
//!
//! - **Owner only.** Every request carries the owner's signed token
//!   (`storage.read` or `storage.write`) and names the owner's xuid; any
//!   other account, the server's own key, or no token is refused, on a
//!   public server too. Nothing here is projected from the event log or
//!   appears in the stream: the log is readable by everyone.
//! - **Versions.** An item's version starts at 1 and rises by one with each
//!   write or delete. A write names the version it replaces in `If-Match`
//!   (`0`: it must never have existed); any other is a 409 carrying the
//!   current item, so the client can merge (docs/live/live-cloud-saves.md
//!   "Conflicts"). A delete leaves a tombstone at the next version.
//! - **Bytes** are stored by SHA-256 as the content store keeps blobs
//!   ([`crate::content::blob_path`]), but under `<data>/storage/<xuid>/`,
//!   never the public content directory: a hash someone knows or guesses
//!   must not fetch someone else's save. The writer says the hash too
//!   (`X-H2-SHA256`), and a body that hashes otherwise is refused.
//! - **Caps:** a size cap per item, a quota and an item count per account
//!   ([`StorageLimits`]), and the per-author write rate every write has.
//! - The index of an account's items is `<data>/storage/<xuid>/index.json`,
//!   written whole (a temporary file, then a rename) after each change.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use axum::body::{Body, HttpBody};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use live_proto::storage::{
    self, IF_MATCH, MODIFIED_HEADER, SHA256_HEADER, StorageItem, StorageList, VERSION_HEADER,
    VersionConflict,
};
use live_proto::maps::Sha256;
use live_proto::{Author, ErrorCode, Xuid, lxm};
use serde::{Deserialize, Serialize};

use crate::app::{ApiError, Live, forbidden, invalid, not_found};
use crate::http::bearer;

/// The directory in the data directory.
pub const STORAGE_DIR: &str = "storage";
const INDEX_FILE: &str = "index.json";
const MIB: u64 = 1024 * 1024;

/// Sizes for cloud saves (docs/live/live-cloud-saves.md "Sizes").
#[derive(Debug, Clone, PartialEq)]
pub struct StorageLimits {
    /// One item. Halo 2's largest save file is a campaign save of about
    /// 4 MiB (`0x40b000` reserved, docs/runtime/checkpoints.md); profiles
    /// and variants are a few KiB at most.
    pub item_bytes: u64,
    /// Everything one account keeps (deleted items not counted).
    pub quota_bytes: u64,
    /// Items one account keeps, tombstones included.
    pub max_items: usize,
}

impl Default for StorageLimits {
    fn default() -> Self {
        StorageLimits {
            item_bytes: 8 * MIB,
            quota_bytes: 64 * MIB,
            max_items: 2048,
        }
    }
}

/// One account's index, as stored.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Index {
    items: Vec<StorageItem>,
}

type Key = (String, String);

/// One account's index, in memory.
#[derive(Debug, Default)]
struct Account {
    items: BTreeMap<Key, StorageItem>,
}

impl Account {
    fn used(&self) -> u64 {
        self.items
            .values()
            .filter(|i| !i.deleted)
            .map(|i| i.size)
            .sum()
    }

    fn version(&self, key: &Key) -> u64 {
        self.items.get(key).map_or(0, |i| i.version)
    }

    /// Whether any live item still names `h`.
    fn references(&self, h: &Sha256) -> bool {
        self.items
            .values()
            .any(|i| !i.deleted && i.sha256.as_ref() == Some(h))
    }
}

/// Every account's index that has been read, behind one lock: writes are
/// rare (a save is written when the player saves) and quick.
#[derive(Default)]
pub struct CloudSaves {
    accounts: tokio::sync::Mutex<HashMap<Xuid, Account>>,
}

/// Why a write was refused: an API error, or a version race with the item
/// as it is now.
#[derive(Debug)]
pub enum Refusal {
    Api(ApiError),
    Conflict(Option<Box<StorageItem>>),
}

impl From<ApiError> for Refusal {
    fn from(e: ApiError) -> Self {
        Refusal::Api(e)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Refusal::Api(e) => e.into_response(),
            Refusal::Conflict(current) => {
                let message = match &current {
                    Some(i) => format!("the item is at version {}", i.version),
                    None => "the item does not exist (If-Match: 0 creates it)".into(),
                };
                crate::http::json(
                    StatusCode::CONFLICT,
                    &VersionConflict {
                        error: ErrorCode::Conflict,
                        message,
                        current: current.map(|b| *b),
                    },
                )
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn unavailable(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Unavailable, m)
}

fn too_large(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::TooLarge, m)
}

fn cannot_write(what: &Path, e: impl std::fmt::Display) -> ApiError {
    eprintln!("live: cloud saves: {}: {e}", what.display());
    unavailable("cloud saves cannot be written right now")
}

fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

fn now_rfc3339() -> String {
    live_proto::sign::format_time(time::OffsetDateTime::now_utc())
}

fn check_item(kind: &str, name: &str) -> ApiResult<Key> {
    if !storage::valid_kind(kind) {
        return Err(not_found(format!(
            "no such kind {kind:?} (kinds: {})",
            storage::KINDS.join(", ")
        )));
    }
    if !storage::valid_name(name) {
        return Err(invalid(
            "names are up to 4 /-separated segments of printable ASCII, \
             without \" * + , : ; < = > ? \\ | %, . or ..",
        ));
    }
    Ok((kind.to_string(), name.to_string()))
}

/// `If-Match`: a version, quoted (as an ETag) or not. Required on writes.
fn if_match(headers: &HeaderMap) -> ApiResult<u64> {
    let raw = headers
        .get(IF_MATCH)
        .ok_or_else(|| invalid("a write needs If-Match: the version it replaces (0: new)"))?;
    raw.to_str()
        .ok()
        .map(|v| v.trim().trim_matches('"'))
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("If-Match is a version number"))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn read_index(path: &Path) -> ApiResult<Account> {
    let text = match std::fs::read(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Account::default()),
        Err(e) => {
            eprintln!("live: cloud saves: {}: {e}", path.display());
            return Err(unavailable("cloud saves cannot be read right now"));
        }
    };
    let index: Index = serde_json::from_slice(&text).map_err(|e| {
        eprintln!("live: cloud saves: {}: {e}", path.display());
        unavailable("this account's cloud saves index is damaged; ask the server's operator")
    })?;
    Ok(Account {
        items: index
            .items
            .into_iter()
            .map(|i| ((i.kind.clone(), i.name.clone()), i))
            .collect(),
    })
}

fn write_index(path: &Path, account: &Account) -> ApiResult<()> {
    let index = Index {
        items: account.items.values().cloned().collect(),
    };
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&index).expect("an index serializes");
    std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(&bytes)?;
            f.sync_data()
        })
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| cannot_write(path, e))
}

/// Reads a request body into `tmp`, hashing as it goes; stops past `cap`.
async fn receive(body: Body, tmp: &Path, cap: u64) -> ApiResult<(Sha256, u64)> {
    let mut body = body;
    let mut file = std::fs::File::create(tmp).map_err(|e| cannot_write(tmp, e))?;
    let mut hasher = ::content::Hasher::default();
    let mut size = 0u64;
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|_| invalid("the upload was cut off"))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        size += data.len() as u64;
        if size > cap {
            return Err(too_large(format!(
                "a cloud save item is at most {} MiB",
                cap / MIB
            )));
        }
        hasher.update(&data);
        file.write_all(&data).map_err(|e| cannot_write(tmp, e))?;
    }
    file.sync_data().map_err(|e| cannot_write(tmp, e))?;
    Ok((hasher.finish(), size))
}

impl Live {
    /// `<data>/storage/<xuid>`.
    fn storage_dir(&self, xuid: Xuid) -> ApiResult<PathBuf> {
        self.data_dir()
            .map(|d| d.join(STORAGE_DIR).join(xuid.to_string()))
            .ok_or_else(|| unavailable("this server keeps no cloud saves (no data directory)"))
    }

    /// The token's account, which must be the one the path names.
    fn storage_owner(&self, bearer: Option<&str>, xuid: &str, method: &str) -> ApiResult<(Author, Xuid)> {
        let xuid = Xuid::parse(xuid).map_err(|_| not_found("no such account"))?;
        let author = self.authenticate(bearer, method)?;
        match &author {
            Author::Account { xuid: who, .. } if *who == xuid => Ok((author, xuid)),
            _ => Err(forbidden("cloud saves are read and written by their owner only")),
        }
    }

    /// `GET /v1/accounts/{xuid}/storage`.
    pub async fn list_storage(&self, bearer: Option<&str>, xuid: &str) -> ApiResult<StorageList> {
        let (_, xuid) = self.storage_owner(bearer, xuid, lxm::STORAGE_READ)?;
        let dir = self.storage_dir(xuid)?;
        let limits = self.policy().storage.clone();
        let mut accounts = self.cloud.accounts.lock().await;
        let account = loaded(&mut accounts, xuid, &dir)?;
        Ok(StorageList {
            items: account.items.values().cloned().collect(),
            used_bytes: account.used(),
            quota_bytes: limits.quota_bytes,
            item_bytes: limits.item_bytes,
        })
    }

    /// `GET /v1/accounts/{xuid}/storage/{kind}/{name}`.
    pub async fn get_storage(
        &self,
        bearer: Option<&str>,
        xuid: &str,
        kind: &str,
        name: &str,
    ) -> ApiResult<(StorageItem, Vec<u8>)> {
        let (_, xuid) = self.storage_owner(bearer, xuid, lxm::STORAGE_READ)?;
        let key = check_item(kind, name)?;
        let dir = self.storage_dir(xuid)?;
        let item = {
            let mut accounts = self.cloud.accounts.lock().await;
            loaded(&mut accounts, xuid, &dir)?
                .items
                .get(&key)
                .filter(|i| !i.deleted)
                .cloned()
                .ok_or_else(|| not_found("no such item"))?
        };
        let h = item.sha256.expect("a live item has a hash");
        let path = crate::content::blob_path(&dir.join("blobs"), &h);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok((item, bytes)),
            Err(e) => {
                eprintln!("live: cloud saves: {}: {e}", path.display());
                Err(unavailable("cloud saves cannot be read right now"))
            }
        }
    }

    /// `PUT /v1/accounts/{xuid}/storage/{kind}/{name}`.
    pub async fn put_storage(
        &self,
        xuid: &str,
        kind: &str,
        name: &str,
        headers: &HeaderMap,
        body: Body,
    ) -> Result<(StatusCode, StorageItem), Refusal> {
        let (author, xuid) = self.storage_owner(bearer(headers), xuid, lxm::STORAGE_WRITE)?;
        self.rate_limit(&author, lxm::STORAGE_WRITE)?;
        let key = check_item(kind, name)?;
        let expected = if_match(headers)?;
        let said: Sha256 = header_str(headers, SHA256_HEADER)
            .ok_or_else(|| invalid("a write needs X-H2-SHA256: the body's SHA-256"))?
            .parse()
            .map_err(|_| invalid("X-H2-SHA256 is 64 hex digits"))?;
        let modified = match header_str(headers, MODIFIED_HEADER) {
            Some(v) => v
                .parse::<i64>()
                .ok()
                .filter(|m| *m > 0)
                .ok_or_else(|| invalid("X-H2-Modified is Unix milliseconds"))?,
            None => now_ms(),
        };
        let limits = self.policy().storage.clone();
        if let Some(n) = header_str(headers, header::CONTENT_LENGTH.as_str())
            .and_then(|v| v.parse::<u64>().ok())
            && n > limits.item_bytes
        {
            return Err(too_large(format!(
                "a cloud save item is at most {} MiB",
                limits.item_bytes / MIB
            ))
            .into());
        }
        let dir = self.storage_dir(xuid)?;
        // Refuse a stale write before reading its body.
        {
            let mut accounts = self.cloud.accounts.lock().await;
            let account = loaded(&mut accounts, xuid, &dir)?;
            if account.version(&key) != expected {
                return Err(Refusal::Conflict(account.items.get(&key).cloned().map(Box::new)));
            }
        }
        let tmp_dir = dir.join("tmp");
        std::fs::create_dir_all(&tmp_dir).map_err(|e| cannot_write(&tmp_dir, e))?;
        let tmp = tmp_dir.join(live_proto::sign::random_id());
        let result = async {
            let (h, size) = receive(body, &tmp, limits.item_bytes).await?;
            if h != said {
                return Err(invalid(format!("the body's SHA-256 is {h}, not X-H2-SHA256's")).into());
            }
            let mut accounts = self.cloud.accounts.lock().await;
            let account = loaded(&mut accounts, xuid, &dir)?;
            let old = account.items.get(&key).cloned();
            if old.as_ref().map_or(0, |i| i.version) != expected {
                return Err(Refusal::Conflict(old.map(Box::new)));
            }
            let replaced = old.as_ref().filter(|i| !i.deleted).map_or(0, |i| i.size);
            let used = account.used() - replaced;
            if used + size > limits.quota_bytes {
                return Err(too_large(format!(
                    "over this account's cloud save quota ({} of {} MiB used)",
                    used.div_ceil(MIB),
                    limits.quota_bytes / MIB
                ))
                .into());
            }
            if old.is_none() && account.items.len() >= limits.max_items {
                return Err(too_large(format!(
                    "an account keeps at most {} cloud save items",
                    limits.max_items
                ))
                .into());
            }
            let path = crate::content::blob_path(&dir.join("blobs"), &h);
            if !path.exists() {
                std::fs::create_dir_all(path.parent().expect("has a parent"))
                    .and_then(|_| std::fs::rename(&tmp, &path))
                    .map_err(|e| cannot_write(&path, e))?;
            }
            let item = StorageItem {
                kind: key.0.clone(),
                name: key.1.clone(),
                version: expected + 1,
                sha256: Some(h),
                size,
                modified,
                stored_at: now_rfc3339(),
                deleted: false,
            };
            account.items.insert(key.clone(), item.clone());
            if let Err(e) = write_index(&dir.join(INDEX_FILE), account) {
                // Keep memory as the disk has it.
                match &old {
                    Some(o) => account.items.insert(key.clone(), o.clone()),
                    None => account.items.remove(&key),
                };
                return Err(e.into());
            }
            if let Some(old_hash) = old.and_then(|o| o.sha256)
                && old_hash != h
                && !account.references(&old_hash)
            {
                let _ = std::fs::remove_file(crate::content::blob_path(&dir.join("blobs"), &old_hash));
            }
            let status = if expected == 0 {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            Ok((status, item))
        }
        .await;
        let _ = std::fs::remove_file(&tmp);
        result
    }

    /// `DELETE /v1/accounts/{xuid}/storage/{kind}/{name}`: a tombstone at
    /// the next version.
    pub async fn delete_storage(
        &self,
        xuid: &str,
        kind: &str,
        name: &str,
        headers: &HeaderMap,
    ) -> Result<StorageItem, Refusal> {
        let (author, xuid) = self.storage_owner(bearer(headers), xuid, lxm::STORAGE_WRITE)?;
        self.rate_limit(&author, lxm::STORAGE_WRITE)?;
        let key = check_item(kind, name)?;
        let expected = if_match(headers)?;
        let dir = self.storage_dir(xuid)?;
        let mut accounts = self.cloud.accounts.lock().await;
        let account = loaded(&mut accounts, xuid, &dir)?;
        let old = account.items.get(&key).cloned();
        let Some(live) = old.clone().filter(|i| !i.deleted) else {
            return Err(Refusal::Api(not_found("no such item")));
        };
        if live.version != expected {
            return Err(Refusal::Conflict(old.map(Box::new)));
        }
        let modified = header_str(headers, MODIFIED_HEADER)
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|m| *m > 0)
            .unwrap_or_else(now_ms);
        let tomb = StorageItem {
            version: live.version + 1,
            sha256: None,
            size: 0,
            modified,
            stored_at: now_rfc3339(),
            deleted: true,
            ..live.clone()
        };
        account.items.insert(key.clone(), tomb.clone());
        if let Err(e) = write_index(&dir.join(INDEX_FILE), account) {
            account.items.insert(key, live);
            return Err(e.into());
        }
        if let Some(h) = live.sha256
            && !account.references(&h)
        {
            let _ = std::fs::remove_file(crate::content::blob_path(&dir.join("blobs"), &h));
        }
        Ok(tomb)
    }
}

/// `xuid`'s index, read from `dir` the first time.
fn loaded<'a>(
    accounts: &'a mut HashMap<Xuid, Account>,
    xuid: Xuid,
    dir: &Path,
) -> ApiResult<&'a mut Account> {
    std::fs::create_dir_all(dir).map_err(|e| cannot_write(dir, e))?;
    Ok(match accounts.entry(xuid) {
        std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => v.insert(read_index(&dir.join(INDEX_FILE))?),
    })
}

fn item_headers(item: &StorageItem) -> [(header::HeaderName, String); 3] {
    [
        (header::ETAG, format!("\"{}\"", item.version)),
        (
            header::HeaderName::from_static(VERSION_HEADER),
            item.version.to_string(),
        ),
        (
            header::HeaderName::from_static(MODIFIED_HEADER),
            item.modified.to_string(),
        ),
    ]
}

type Shared = State<Arc<Live>>;

pub(crate) async fn list(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
) -> ApiResult<Response> {
    let list = live.list_storage(bearer(&headers), &xuid).await?;
    let mut resp = crate::http::json(StatusCode::OK, &list);
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().expect("a header"));
    Ok(resp)
}

pub(crate) async fn get_item(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((xuid, kind, name)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (item, bytes) = live.get_storage(bearer(&headers), &xuid, &kind, &name).await?;
    let mut resp = (item_headers(&item), bytes).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, "application/octet-stream".parse().expect("a header"));
    h.insert(header::CACHE_CONTROL, "private, no-store".parse().expect("a header"));
    if let Some(sha) = item.sha256 {
        h.insert(SHA256_HEADER, sha.to_string().parse().expect("a header"));
    }
    Ok(resp)
}

pub(crate) async fn put_item(
    State(live): Shared,
    UrlPath((xuid, kind, name)): UrlPath<(String, String, String)>,
    req: Request,
) -> Result<Response, Refusal> {
    let headers = req.headers().clone();
    let (status, item) = live
        .put_storage(&xuid, &kind, &name, &headers, req.into_body())
        .await?;
    let mut resp = crate::http::json(status, &item);
    for (k, v) in item_headers(&item) {
        resp.headers_mut().insert(k, v.parse().expect("a header"));
    }
    Ok(resp)
}

pub(crate) async fn delete_item(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((xuid, kind, name)): UrlPath<(String, String, String)>,
) -> Result<Response, Refusal> {
    let item = live.delete_storage(&xuid, &kind, &name, &headers).await?;
    Ok(crate::http::json(StatusCode::OK, &item))
}
