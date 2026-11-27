//! Packages on Live (docs/live/live-api.md, "Packages";
//! docs/editor/studio-redesign.md, "Live: sharing, signing and credit").
//!
//! - **Files** go up as content (`PUT /v1/content/{sha256}`), with that
//!   route's caps, quota and retail check.
//! - **The document** (`PUT /v1/packages`) is a `PackagePublished` event
//!   signed by the publisher's own key: the manifest, its lock and an
//!   optional credit. The server verifies the signature, then runs
//!   [`live_proto::packages::check`] over [`Holdings`] of this home's
//!   catalog: the lock names exactly the manifest's files, its
//!   `PackageId` is the stream's, no `kit` item, a `model` or `animation`
//!   item only from an account with `model_uploads`, and every locked
//!   hash off the retail list, not taken down and stored.
//! - **Credit**: an admin's `PUT /v1/packages/{id}/credit` (a server-signed
//!   `PackageCredited`), then the publisher's signed credit, then the
//!   manifest's `[credit]`.
//! - **The model flag**: `PUT /v1/admin/model-uploads/{gamertag}`, a
//!   server-signed `ModelUploadsSet` on the account's stream.
//!
//! The projection below is rebuilt from the log on both homes; the Worker
//! runs these routes as they are (the parity suite,
//! `tests/support/packages.rs`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, put};
use live_proto::maps::{Credit, Sha256, Standing};
use live_proto::packages::{
    self as wire, Credited, FileState, Holdings, MODEL_UPLOADS_SET, ModelUploadsSet,
    PACKAGE_CREDITED, PACKAGE_PUBLISHED, PackageAnswer, PackageDocument, PackageId, PackageList,
    PackageSummary,
};
use live_proto::sign::{MAX_EVENT_SKEW_S, SignedEvent, parse_time};
use live_proto::{Author, Gamertag, Xuid, streams};
use serde::Deserialize;
use serde_json::json;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found, now, sign_error};
use crate::content::{CatalogState, Retail, gone};
use crate::dispatch::Subscriber;
use crate::http::bearer;
use crate::model::nonce_key;
use crate::store::{NewEvent, RecordedEvent};

type ApiResult<T> = Result<T, ApiError>;

/// The largest document: a manifest and a lock of
/// [`wire::MAX_FILES`] files, with room.
pub const MAX_BODY: usize = 1024 * 1024;

// ---------- the projection ----------

/// A published package.
#[derive(Debug, Clone)]
pub struct Kept {
    pub doc: PackageDocument,
    pub publisher: Xuid,
    /// The event's `author` (the account id whose key signed it).
    pub author: String,
    pub jws: Arc<str>,
    pub published_at: String,
    pub position: u64,
}

#[derive(Debug, Default)]
pub struct PackagesState {
    pub by_id: BTreeMap<PackageId, Kept>,
    /// Admins' credits, kept apart so a package published again keeps
    /// its correction.
    pub credits: HashMap<PackageId, Credit>,
    /// Accounts approved for custom models.
    pub model_uploads: HashSet<Xuid>,
}

impl PackagesState {
    /// Who made it: an admin's credit, the publisher's signed one, the
    /// manifest's.
    pub fn credit_of(&self, id: &PackageId, k: &Kept) -> Option<Credit> {
        self.credits.get(id).cloned().or_else(|| k.doc.credit())
    }

    fn apply(&mut self, e: &RecordedEvent) {
        let b = &e.body;
        match b.kind.as_str() {
            PACKAGE_PUBLISHED => {
                let Some(Author::Account { xuid, .. }) = Author::parse(&b.author) else {
                    return;
                };
                let Ok(doc) = serde_json::from_value::<PackageDocument>(b.data.clone()) else {
                    return;
                };
                let Ok(id) = doc.id() else { return };
                self.by_id.entry(id).or_insert(Kept {
                    doc,
                    publisher: xuid,
                    author: b.author.clone(),
                    jws: e.jws.clone(),
                    published_at: b.at.clone(),
                    position: e.position,
                });
            }
            PACKAGE_CREDITED => {
                if let Ok(d) = serde_json::from_value::<Credited>(b.data.clone()) {
                    match d.credit.filter(|c| !c.is_empty()) {
                        Some(c) => {
                            self.credits.insert(d.id, c);
                        }
                        None => {
                            self.credits.remove(&d.id);
                        }
                    }
                }
            }
            MODEL_UPLOADS_SET => {
                if let Ok(d) = serde_json::from_value::<ModelUploadsSet>(b.data.clone()) {
                    if d.approved {
                        self.model_uploads.insert(d.xuid);
                    } else {
                        self.model_uploads.remove(&d.xuid);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The packages' projection.
#[derive(Default)]
pub struct Packages {
    state: RwLock<PackagesState>,
}

impl Packages {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, PackagesState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Packages {
    fn name(&self) -> &str {
        "packages"
    }

    fn apply(&self, event: &RecordedEvent) {
        if wire::KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

/// This home's catalog as the upload check reads it.
struct Here<'a> {
    catalog: &'a CatalogState,
    packages: &'a PackagesState,
    retail: &'a Retail,
    uploads_on: bool,
}

impl Holdings for Here<'_> {
    fn is_retail(&self, h: &Sha256) -> bool {
        self.retail.hashes.contains(h)
    }

    fn file(&self, h: &Sha256) -> FileState {
        match self.catalog.blob_standing(h) {
            Standing::Hidden | Standing::Blocked => FileState::TakenDown,
            // A deleted blob may be uploaded again.
            Standing::Ok | Standing::Deleted if self.catalog.servable(h) => FileState::Stored,
            Standing::Ok | Standing::Deleted => FileState::Missing,
        }
    }

    fn model_uploads(&self, xuid: Xuid) -> bool {
        self.uploads_on && self.packages.model_uploads.contains(&xuid)
    }
}

/// Why a published package may not be served: a file an admin took down.
fn withheld(catalog: &CatalogState, k: &Kept) -> Option<String> {
    k.doc.lock.files.iter().find_map(|(file, h)| {
        (!catalog.blob_standing(h).is_ok())
            .then(|| format!("an admin took down {file} ({h}), which this package uses"))
    })
}

// ---------- the service ----------

impl Live {
    fn package_summary(
        &self,
        catalog: &CatalogState,
        s: &PackagesState,
        id: &PackageId,
        k: &Kept,
    ) -> PackageSummary {
        let m = &k.doc.manifest;
        let mut kinds: Vec<String> = Vec::new();
        for item in &m.items {
            let word = serde_json::to_value(item.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            if !kinds.contains(&word) {
                kinds.push(word);
            }
        }
        PackageSummary {
            id: *id,
            slug: m.package.id.clone(),
            name: m.package.name.clone(),
            description: m.package.description.clone(),
            publisher: self.who(k.publisher),
            credit: s.credit_of(id, k),
            kinds,
            items: m.items.len(),
            sim: m.has_sim_item(),
            requires: m.package.requires.clone(),
            features: m.package.features.clone(),
            size: k
                .doc
                .lock
                .files
                .values()
                .map(|h| catalog.blobs.get(h).map_or(0, |b| b.size))
                .sum(),
            published_at: k.published_at.clone(),
            position: k.position,
        }
    }

    fn package_answer(
        &self,
        catalog: &CatalogState,
        s: &PackagesState,
        id: &PackageId,
        k: &Kept,
    ) -> PackageAnswer {
        PackageAnswer {
            package: self.package_summary(catalog, s, id, k),
            manifest: k.doc.manifest.clone(),
            lock: k.doc.lock.clone(),
            jws: k.jws.to_string(),
            author: k.author.clone(),
            position: self.position(),
        }
    }

    /// `PUT /v1/packages`: a `PackagePublished` event signed by the
    /// package's publisher.
    pub async fn publish_package(
        &self,
        bearer: Option<&str>,
        jws: &str,
    ) -> ApiResult<(StatusCode, PackageAnswer)> {
        let (author, xuid) = self.uploader(bearer)?;
        let event = SignedEvent::open(jws).map_err(sign_error)?;
        let body = &event.body;
        if body.author != author.to_string() {
            return Err(forbidden("the document's author is not the token's issuer"));
        }
        let key = self
            .key_for(&author, &body.kid)
            .ok_or_else(|| crate::app::unauthorized("the document's key is unknown or revoked"))?;
        event.verify(&key).map_err(sign_error)?;
        if body.kind != PACKAGE_PUBLISHED {
            return Err(invalid(format!("a package is a {PACKAGE_PUBLISHED} event")));
        }
        let clock = now();
        let at = parse_time(&body.at)
            .ok_or_else(|| invalid("at is not an RFC 3339 time"))?
            .unix_timestamp();
        if (at - clock).abs() > MAX_EVENT_SKEW_S {
            return Err(invalid("at is more than 5 minutes from the server's clock"));
        }
        let doc: PackageDocument = serde_json::from_value(body.data.clone())
            .map_err(|e| invalid(format!("the document: {e}")))?;
        let retail = self.retail();
        let _guard = self.content_writes.lock().await;
        let id = {
            let catalog = self.catalog.read();
            let s = self.packages.read();
            let here = Here {
                catalog: &catalog,
                packages: &s,
                retail: &retail,
                uploads_on: !catalog.uploads_off.contains(&xuid),
            };
            let id = wire::check(&doc, &body.stream, xuid, &here)
                .map_err(|r| ApiError::new(r.code(), r.to_string()))?;
            if let Some(k) = s.by_id.get(&id) {
                if k.publisher != xuid {
                    return Err(conflict(format!(
                        "this package is already published by {}",
                        self.who(k.publisher).gamertag
                    )));
                }
                return Ok((StatusCode::OK, self.package_answer(&catalog, &s, &id, k)));
            }
            id
        };
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
        let catalog = self.catalog.read();
        let s = self.packages.read();
        let k = s
            .by_id
            .get(&id)
            .ok_or_else(|| ApiError::new(live_proto::ErrorCode::Unavailable, "not applied"))?;
        Ok((StatusCode::CREATED, self.package_answer(&catalog, &s, &id, k)))
    }

    /// `GET /v1/packages/{id}`.
    pub fn package(&self, id: &str) -> ApiResult<PackageAnswer> {
        let id: PackageId = id.parse().map_err(|_| not_found("no such package"))?;
        let catalog = self.catalog.read();
        let s = self.packages.read();
        let k = s.by_id.get(&id).ok_or_else(|| not_found("no such package"))?;
        if let Some(why) = withheld(&catalog, k) {
            return Err(gone(why));
        }
        Ok(self.package_answer(&catalog, &s, &id, k))
    }

    /// `GET /v1/packages`: every package whose files are served, newest
    /// first.
    pub fn package_list(&self) -> PackageList {
        let catalog = self.catalog.read();
        let s = self.packages.read();
        let mut items: Vec<PackageSummary> = s
            .by_id
            .iter()
            .filter(|(_, k)| withheld(&catalog, k).is_none())
            .map(|(id, k)| self.package_summary(&catalog, &s, id, k))
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.position));
        PackageList {
            items,
            position: self.position(),
        }
    }

    /// `PUT /v1/packages/{id}/credit`: an admin sets who made a package
    /// (a server-signed `PackageCredited`); an empty credit clears it.
    pub async fn set_package_credit(
        &self,
        bearer: Option<&str>,
        id: &str,
        credit: Credit,
    ) -> ApiResult<(u64, PackageSummary)> {
        let by = self.moderator(bearer)?;
        let id: PackageId = id
            .parse()
            .map_err(|_| invalid("a PackageId is 64 hex digits"))?;
        let credit = credit.tidy();
        credit.validate().map_err(invalid)?;
        let _guard = self.content_writes.lock().await;
        if !self.packages.read().by_id.contains_key(&id) {
            return Err(not_found("no such package"));
        }
        let data = Credited {
            id,
            credit: (!credit.is_empty()).then_some(credit),
            by,
        };
        let event = self.server_event(
            wire::stream(&id),
            PACKAGE_CREDITED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        let catalog = self.catalog.read();
        let s = self.packages.read();
        let k = s.by_id.get(&id).ok_or_else(|| not_found("no such package"))?;
        Ok((recorded[0].position, self.package_summary(&catalog, &s, &id, k)))
    }

    /// `PUT /v1/admin/model-uploads/{gamertag}`.
    pub async fn set_model_uploads(
        &self,
        bearer: Option<&str>,
        tag: &str,
        approved: bool,
        note: Option<String>,
    ) -> ApiResult<u64> {
        let by = self.moderator(bearer)?;
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        if note
            .as_ref()
            .is_some_and(|n| n.chars().count() > wire::MAX_NOTE)
        {
            return Err(invalid(format!("the note is at most {} characters", wire::MAX_NOTE)));
        }
        let tag = Gamertag::new(tag).map_err(|_| not_found("no such account"))?;
        let xuid = self
            .model
            .read()
            .account_by_tag(&tag)
            .map(|a| a.xuid)
            .ok_or_else(|| not_found("no such account"))?;
        let data = ModelUploadsSet {
            xuid,
            approved,
            note,
            by,
        };
        let event = self.server_event(
            streams::account(xuid),
            MODEL_UPLOADS_SET,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

fn body_text(body: Result<Bytes, BytesRejection>) -> ApiResult<Bytes> {
    body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::new(live_proto::ErrorCode::TooLarge, "request body too large")
        }
        _ => invalid(e.body_text()),
    })
}

async fn put_package(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let jws = std::str::from_utf8(&bytes)
        .map_err(|_| invalid("the body must be a compact JWS"))?
        .trim();
    let (status, answer) = live.publish_package(bearer(&headers), jws).await?;
    Ok(crate::http::json(status, &answer))
}

async fn list(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    Ok(crate::http::json(StatusCode::OK, &live.package_list()))
}

async fn get_package(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    Ok(crate::http::json(StatusCode::OK, &live.package(&id)?))
}

async fn put_credit(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let credit: Credit =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("credit: {e}")))?;
    let (position, package) = live.set_package_credit(bearer(&headers), &id, credit).await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position, "package": package}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelUploadsBody {
    approved: bool,
    #[serde(default)]
    note: Option<String>,
}

async fn put_model_uploads(
    State(live): Shared,
    headers: HeaderMap,
    Path(tag): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_text(body)?;
    let b: ModelUploadsBody =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("model uploads: {e}")))?;
    let position = live
        .set_model_uploads(bearer(&headers), &tag, b.approved, b.note)
        .await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position}),
    ))
}

/// The package routes, with their own body cap.
pub(crate) fn routes() -> Router<Arc<Live>> {
    Router::new()
        .route("/v1/packages", get(list).put(put_package))
        .route("/v1/packages/{id}", get(get_package))
        .route("/v1/packages/{id}/credit", put(put_credit))
        .route("/v1/admin/model-uploads/{gamertag}", put(put_model_uploads))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}
