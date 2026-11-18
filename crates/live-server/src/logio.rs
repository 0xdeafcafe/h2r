//! The log over HTTP, for the operator: export and import as stored,
//! positions, versions and hashes kept (docs/live/live-worker.md,
//! "Migrating the current data"). This is how a log gets into a server
//! whose storage only it can reach (the Worker's hub), and back out:
//! `h2 live store copy --to https://…` and `--from https://…`
//! ([`crate::httpstore`]).
//!
//! | Route | Token | |
//! |---|---|---|
//! | `GET /v1/admin/log?after=N&limit=M` | operator, `log.read` | a [`LogPage`]: events with position > N, at most M (≤ [`PAGE_MAX`]) |
//! | `POST /v1/admin/log` | operator, `log.write` | imports a [`LogPage`] ([`EventStore::import`]: each event must follow the head and continue its stream's version and hash chain); 409 with the reason when one does not. Answers `{head}` once the read model has it. |
//! | `POST /v1/admin/log/release` | operator, `log.write` | the copy is done: the server's own writes, held since the first import ([`crate::hold`]), start again |
//! | `GET /v1/admin/log/meta` | operator, `log.read` | [`LogMeta`]: the head, whether writes are held, the title-file blobs, the checkpoints |
//! | `GET`, `PUT /v1/admin/log/blobs/{sha256}` | `log.read`, `log.write` | a title-file blob's bytes (a PUT's must hash to the name) |
//! | `PUT /v1/admin/log/checkpoints/{name}` | `log.write` | `{position}` |
//!
//! Operator tokens are signed with the server's own key (`iss` the server),
//! so only whoever holds `server.key` can read the log whole or write into
//! it. A batch JWS is sent once a page ([`LogPage::batches`]), not once an
//! event.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query as Params, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use live_proto::{ErrorCode, lxm};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::{ApiError, Live, invalid, not_found};
use crate::http::bearer;
use crate::store::{BatchDecoder, Query, RecordedEvent, StoreError, batch_key};

/// The most events a page holds.
pub const PAGE_MAX: usize = 1000;
/// The largest import body.
pub const MAX_IMPORT_BYTES: usize = 32 << 20;

/// One stored event: its JWS inline, or its batch's key in
/// [`LogPage::batches`] and its index there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRow {
    pub position: u64,
    pub version: u64,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jws: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jws_index: Option<usize>,
}

/// A page of the log, and the head of the log it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    pub head: u64,
    pub events: Vec<LogRow>,
    /// Batch JWS by key ([`batch_key`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub batches: BTreeMap<String, String>,
}

impl LogPage {
    pub fn new(head: u64, events: &[RecordedEvent]) -> Self {
        let mut page = LogPage {
            head,
            ..LogPage::default()
        };
        let mut last: Option<(&str, String)> = None;
        for e in events {
            let (jws, batch) = match e.jws_index {
                None => (Some(e.jws.to_string()), None),
                Some(_) => {
                    let key = match &last {
                        Some((jws, key)) if *jws == &*e.jws => key.clone(),
                        _ => {
                            let key = batch_key(&e.jws);
                            page.batches.insert(key.clone(), e.jws.to_string());
                            last = Some((&e.jws, key.clone()));
                            key
                        }
                    };
                    (None, Some(key))
                }
            };
            page.events.push(LogRow {
                position: e.position,
                version: e.version,
                hash: e.hash.clone(),
                jws,
                batch,
                jws_index: e.jws_index,
            });
        }
        page
    }

    /// The events as stored, each body decoded from its JWS. A batch's key
    /// must be its JWS's hash, and an event names its JWS one way only.
    pub fn into_events(self) -> Result<Vec<RecordedEvent>, String> {
        for (key, jws) in &self.batches {
            if batch_key(jws) != *key {
                return Err(format!("batch {key} is not the hash of its JWS"));
            }
        }
        let mut decoder = BatchDecoder::default();
        self.events
            .into_iter()
            .map(|row| {
                let jws = match (&row.jws, &row.batch, row.jws_index) {
                    (Some(jws), None, None) => jws.as_str(),
                    (None, Some(key), Some(_)) => self
                        .batches
                        .get(key)
                        .ok_or_else(|| format!("position {}: no batch {key}", row.position))?,
                    _ => {
                        return Err(format!(
                            "position {}: an event is a jws, or a batch and an index",
                            row.position
                        ));
                    }
                };
                let new = decoder
                    .decode(jws, row.jws_index)
                    .map_err(|e| format!("position {}: {e}", row.position))?;
                Ok(RecordedEvent {
                    position: row.position,
                    version: row.version,
                    hash: row.hash,
                    jws: new.jws,
                    jws_index: new.jws_index,
                    body: new.body,
                })
            })
            .collect()
    }
}

/// `GET /v1/admin/log/meta`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogMeta {
    pub head: u64,
    /// Whether the server's own writes are held ([`crate::hold`]).
    #[serde(default)]
    pub held: bool,
    pub blobs: Vec<String>,
    pub checkpoints: Vec<(String, u64)>,
}

/// `PUT /v1/admin/log/checkpoints/{name}`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Checkpoint {
    pub position: u64,
}

/// `POST /v1/admin/log`'s answer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Imported {
    pub head: u64,
}

type Shared = State<Arc<Live>>;
type ApiResult<T> = Result<T, ApiError>;

fn json<T: Serialize>(status: StatusCode, v: &T) -> Response {
    (status, axum::Json(v)).into_response()
}

fn import_error(e: StoreError) -> ApiError {
    match e {
        // The operator needs the reason: which position, why.
        StoreError::Import { .. } | StoreError::Conflict { .. } => {
            ApiError::new(ErrorCode::Conflict, e.to_string())
        }
        e => e.into(),
    }
}

/// `GET /v1/admin/log`.
pub async fn export(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<std::collections::HashMap<String, String>>,
) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_READ)?;
    let num = |k: &str, default: u64| -> ApiResult<u64> {
        p.get(k).map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| invalid(format!("{k} must be a non-negative integer")))
        })
    };
    let after = num("after", 0)?;
    let limit = (num("limit", PAGE_MAX as u64)? as usize).clamp(1, PAGE_MAX);
    let store = live.store();
    let head = store.head().await?;
    let events = store.read(&Query::after(after, limit)).await?;
    Ok(json(StatusCode::OK, &LogPage::new(head, &events)))
}

/// `POST /v1/admin/log`.
pub async fn import(State(live): Shared, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_WRITE)?;
    let page: LogPage = serde_json::from_slice(&body).map_err(|e| invalid(format!("body: {e}")))?;
    let events = page.into_events().map_err(invalid)?;
    let store = live.store();
    if let Some(last) = events.last().map(|e| e.position) {
        // Nothing of the server's own may land between two pages.
        if !live.held.held() {
            live.held.set_held(true).await?;
        }
        store.import(&events).await.map_err(import_error)?;
        live.wait_applied(last).await?;
    }
    let head = store.head().await?;
    Ok(json(StatusCode::OK, &Imported { head }))
}

/// `POST /v1/admin/log/release`: the copy is done; the server writes
/// again.
pub async fn release(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_WRITE)?;
    live.held.set_held(false).await?;
    let head = live.store().head().await?;
    Ok(json(StatusCode::OK, &Imported { head }))
}

/// `GET /v1/admin/log/meta`.
pub async fn meta(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_READ)?;
    let store = live.store();
    let meta = LogMeta {
        head: store.head().await?,
        held: live.held.held(),
        blobs: store.blob_names().await?,
        checkpoints: store.checkpoints().await?,
    };
    Ok(json(StatusCode::OK, &meta))
}

fn sha_name(sha: &str) -> ApiResult<()> {
    let ok = sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    ok.then_some(())
        .ok_or_else(|| invalid("a blob is named by its SHA-256, 64 lowercase hex digits"))
}

/// `GET /v1/admin/log/blobs/{sha256}`.
pub async fn get_blob(
    State(live): Shared,
    headers: HeaderMap,
    Path(sha): Path<String>,
) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_READ)?;
    sha_name(&sha)?;
    match live.store().blob(&sha).await? {
        Some(bytes) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response()),
        None => Err(not_found("no such blob")),
    }
}

/// `PUT /v1/admin/log/blobs/{sha256}`.
pub async fn put_blob(
    State(live): Shared,
    headers: HeaderMap,
    Path(sha): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_WRITE)?;
    sha_name(&sha)?;
    if format!("{:x}", Sha256::digest(&body)) != sha {
        return Err(invalid("the bytes do not hash to the blob's name"));
    }
    live.store().put_blob(&sha, &body).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PUT /v1/admin/log/checkpoints/{name}`.
pub async fn put_checkpoint(
    State(live): Shared,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    live.authorize_operator(bearer(&headers), lxm::LOG_WRITE)?;
    let c: Checkpoint = serde_json::from_slice(&body).map_err(|e| invalid(format!("body: {e}")))?;
    live.store().save_checkpoint(&name, c.position).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The routes, with their own body limit (an import page is bigger than
/// other requests).
pub fn routes() -> axum::Router<Arc<Live>> {
    use axum::routing::{get, put};
    axum::Router::new()
        .route("/v1/admin/log", get(export).post(import))
        .route("/v1/admin/log/meta", get(meta))
        .route("/v1/admin/log/release", axum::routing::post(release))
        .route("/v1/admin/log/blobs/{sha256}", get(get_blob).put(put_blob))
        .route("/v1/admin/log/checkpoints/{name}", put(put_checkpoint))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_IMPORT_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::conformance::{batch, event};
    use crate::store::{EventStore, MemoryStore, NewEvent};
    use live_proto::sign::KeyPair;

    /// A page carries a batch's JWS once and gives back exactly what was
    /// stored; a page that lies about a batch is refused.
    #[tokio::test]
    async fn a_page_round_trips_with_batches_once() {
        let key = KeyPair::generate();
        let store = MemoryStore::new();
        store.append(vec![event(&key, "a", 0)], &[]).await.unwrap();
        let (jws, bodies) = batch(&key, "b", 3);
        let events: Vec<NewEvent> = bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| NewEvent {
                jws: jws.clone(),
                jws_index: Some(i),
                body,
            })
            .collect();
        store.append(events, &[]).await.unwrap();
        store.append(vec![event(&key, "a", 1)], &[]).await.unwrap();
        let stored = store.read(&Query::after(0, 100)).await.unwrap();
        assert_eq!(stored.len(), 5);

        let page = LogPage::new(5, &stored);
        assert_eq!(page.batches.len(), 1, "the batch once");
        let text = serde_json::to_string(&page).unwrap();
        assert_eq!(text.matches(&*jws).count(), 1);
        let back: LogPage = serde_json::from_str(&text).unwrap();
        assert_eq!(back.clone().into_events().unwrap(), stored);

        let mut bad = back.clone();
        let key0 = bad.batches.keys().next().unwrap().clone();
        bad.batches.insert(key0, stored[0].jws.to_string());
        assert!(bad.into_events().is_err(), "a batch that is not its hash");
        let mut bad = back.clone();
        bad.events[0].batch = Some("x".into());
        assert!(bad.into_events().is_err(), "both a jws and a batch");
        let mut bad = back;
        bad.events[1].jws_index = Some(9);
        assert!(bad.into_events().is_err(), "an index past the batch");
    }
}
