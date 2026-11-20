//! Each request advances one film by a bounded amount of validation or import.
//! Accepted bytes and the inspection cursor survive process and Worker restarts.
mod cleanup;
mod state;

pub(crate) use cleanup::cleanup_published;

use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
};
use futures_util::StreamExt;
use live_proto::{
    ErrorCode,
    film::{MAX_UPLOAD_BYTES, PART_BYTES, SINGLE_UPLOAD_BYTES},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{ApiResult, Shared, import::Spool, inspect::Inspector, parts};
use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::blobs::{BlobError, BlobStore, Meta};
use crate::host_identity::{FilmReservation, Publisher, TELEMETRY_INGEST, lease_id};
use crate::http::{bearer, game_id};
use state::{Candidate, Progress, Ready, Slot};

// Each TICK can expand to 8 MiB. Stop between actual chunks irrespective of
// compression ratio; a stream of tiny records also consumes inspector units.
const WORK_UNITS: usize = 4;
const IMPORT_PAGES: usize = 4;
const READ_BYTES: u64 = 4 << 20;
const WINDOW: u64 = 1 << 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadQuery {
    part: Option<u64>,
    complete: Option<u64>,
    sha256: Option<String>,
    #[serde(rename = "continue")]
    more: Option<u8>,
}

enum Request {
    Single,
    Part(u64),
    Complete(u64, String),
    Continue,
}
impl UploadQuery {
    fn request(self, method: Method) -> ApiResult<Request> {
        match (method, self.part, self.complete, self.sha256, self.more) {
            (Method::POST, None, None, None, None) => Ok(Request::Single),
            (Method::POST, None, None, None, Some(1)) => Ok(Request::Continue),
            (Method::PUT, Some(n), None, None, None)
                if n < MAX_UPLOAD_BYTES.div_ceil(PART_BYTES) =>
            {
                Ok(Request::Part(n))
            }
            (Method::POST, None, Some(n), Some(hash), None)
                if (1..=MAX_UPLOAD_BYTES).contains(&n)
                    && hash.len() == 64
                    && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                Ok(Request::Complete(n, hash.to_ascii_lowercase()))
            }
            _ => Err(invalid("invalid host film upload parameters")),
        }
    }
}

fn blob_error(e: BlobError) -> ApiError {
    eprintln!("live: film store: {e}");
    ApiError::new(ErrorCode::Unavailable, "the film store is unavailable")
}
fn too_large() -> ApiError {
    ApiError::new(
        ErrorCode::TooLarge,
        "host film upload exceeds the size limit",
    )
}

struct Upload<'a> {
    live: &'a Live,
    store: &'a BlobStore,
    id: &'a str,
    grant: &'a FilmReservation,
    publisher: Publisher,
}

impl Upload<'_> {
    fn published(&self) -> ApiResult<bool> {
        let model = self.live.model.read();
        if let Some((host, _)) = model.films.standalone_host_blob(self.id) {
            return if lease_id(host) == self.publisher.host_lease_id() {
                Ok(true)
            } else {
                Err(forbidden("the game belongs to another host"))
            };
        }
        match model.games.get(self.id) {
            Some(g) if lease_id(&g.started.host) != self.publisher.host_lease_id() => {
                Err(forbidden("the game belongs to another host"))
            }
            Some(g) => Ok(g.has_host_film),
            None if self.id == self.grant.canonical_film_id() => Ok(false),
            None => Err(not_found("no such assigned game")),
        }
    }

    async fn acknowledge(&self) -> ApiResult<StatusCode> {
        if !super::resume_import(self.live, self.id, IMPORT_PAGES).await? {
            return Ok(StatusCode::ACCEPTED);
        }
        self.live.rank_game(self.id).await?;
        cleanup_published(self.live, self.id).await;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn single(&self, body: Body, declared: Option<u64>) -> ApiResult<Progress> {
        let mut candidate = Candidate::new(self.id, false, self.grant);
        let mut inspector = Inspector::default();
        let mut writer = self
            .store
            .writer(&candidate.blob, &Meta::default())
            .map_err(blob_error)?;
        let mut stream = body.into_data_stream();
        let result = async {
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|_| invalid("the film upload was cut off"))?;
                if writer.size().saturating_add(bytes.len() as u64) > SINGLE_UPLOAD_BYTES {
                    return Err(too_large());
                }
                self.inspect_window(
                    &mut inspector,
                    &mut candidate,
                    &bytes,
                    READ_BYTES,
                    WORK_UNITS,
                )
                .await?;
                writer.write(&bytes).await.map_err(blob_error)?;
            }
            if writer.size() == 0 || declared.is_some_and(|n| n != writer.size()) {
                return Err(invalid("empty or incomplete host film"));
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            writer.abort().await;
            cleanup::reconcile(self.live, self.id, &candidate.blob, false, true).await?;
            return Err(e);
        }
        candidate.size = writer.size();
        writer.finish().await.map_err(blob_error)?;
        Ok(Progress::Reading(inspector, candidate))
    }

    async fn part(
        &self,
        previous: Option<Progress>,
        n: u64,
        bytes: Bytes,
        client_continues: bool,
    ) -> ApiResult<(Progress, bool)> {
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let (mut inspector, mut candidate) = match previous {
            Some(Progress::Reading(i, c))
                if c.multipart() && (n != 0 || c.hashes.first() == Some(&digest)) =>
            {
                (i, c)
            }
            Some(Progress::Ready(r)) if r.candidate.hashes.get(n as usize) == Some(&digest) => {
                return Ok((Progress::Ready(r), false));
            }
            _ if n == 0 => (
                Inspector::default(),
                Candidate::new(self.id, true, self.grant),
            ),
            _ => return Err(conflict("send film part zero first")),
        };
        if let Some(stored) = candidate.hashes.get(n as usize) {
            if *stored != digest {
                return Err(conflict("film part differs from the accepted bytes"));
            }
            return Ok((Progress::Reading(inspector, candidate), false));
        }
        if n as usize != candidate.hashes.len()
            || candidate.size != n * PART_BYTES
            || (client_continues && inspector.position() != candidate.size)
            || bytes.is_empty()
        {
            return Err(conflict(
                "continue inspection and send consecutive film parts",
            ));
        }
        let key = parts::part_key(&candidate.blob, n, &digest);
        let size = bytes.len() as u64;
        let first_pass_done = inspector.position() == candidate.size;
        if first_pass_done {
            let stop = candidate.size + READ_BYTES.min(size);
            let work = inspector.completed_work().saturating_add(WORK_UNITS);
            if let Err(err) = self
                .inspect_window(&mut inspector, &mut candidate, &bytes, stop, work)
                .await
            {
                cleanup::reconcile(self.live, self.id, &candidate.blob, false, true).await?;
                return Err(err);
            }
        }
        self.store
            .put(&key, bytes, &Meta::default())
            .await
            .map_err(blob_error)?;
        candidate.size += size;
        candidate.hashes.push(digest);
        Ok((Progress::Reading(inspector, candidate), first_pass_done))
    }

    async fn inspect_window(
        &self,
        i: &mut Inspector,
        c: &mut Candidate,
        bytes: &[u8],
        stop: u64,
        budget: usize,
    ) -> ApiResult<()> {
        let len = bytes.len().min(stop.saturating_sub(i.position()) as usize);
        let mut tail = &bytes[..len];
        while !tail.is_empty() && i.completed_work() < budget {
            let n = i.consume_one(tail)?;
            c.spool
                .drain(
                    &mut i.events,
                    self.store,
                    &c.blob,
                    self.live,
                    self.id,
                    &self.publisher,
                    self.grant,
                )
                .await?;
            tail = &tail[n..];
        }
        Ok(())
    }

    /// Read at most one bounded prefix of the received bytes, even when an old
    /// client sends a whole match in a single POST.
    async fn inspect(&self, i: &mut Inspector, c: &mut Candidate) -> ApiResult<()> {
        let stop = c.size.min(i.position().saturating_add(READ_BYTES));
        let budget = i.completed_work().saturating_add(WORK_UNITS);
        while i.position() < stop && i.completed_work() < budget {
            let bytes = c
                .read(self.store, i.position(), WINDOW.min(stop - i.position()))
                .await?;
            self.inspect_window(i, c, &bytes, stop, budget).await?;
        }
        Ok(())
    }

    async fn advance(
        &self,
        slot: &mut Slot,
        progress: Progress,
        first_pass_done: bool,
    ) -> ApiResult<StatusCode> {
        if &progress.candidate().reservation != self.grant {
            return Err(forbidden(
                "film ingestion belongs to another host reservation",
            ));
        }
        let mut ready = match progress {
            Progress::Reading(mut i, mut c) => {
                if !first_pass_done {
                    self.inspect(&mut i, &mut c).await?;
                }
                let caught_up = i.position() == c.size;
                if !caught_up || (c.multipart() && c.seal.is_none()) {
                    slot.save(self.store, &Progress::Reading(i, c)).await?;
                    return Ok(if caught_up {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::ACCEPTED
                    });
                }
                let sha256 = i.sha256();
                if let Some(expected) = &c.seal
                    && *expected != sha256
                {
                    return Err(invalid("host film digest mismatch"));
                }
                let film = i.finish()?;
                if c.multipart() {
                    parts::publish(self.store, &c.blob, c.size, &c.hashes)
                        .await
                        .map_err(blob_error)?;
                }
                let mut receipt = json!({"game":self.id, "blob":c.blob});
                if let Some(ranges) = film.web_ranges {
                    receipt["web_ranges"] = json!(ranges);
                }
                if let Some(summary) = film.summary {
                    receipt["summary"] = json!(summary);
                }
                Ready {
                    candidate: c,
                    receipt,
                    unified: film.unified,
                    started_ms: film.events.started_ms,
                    hz: film.events.tick_rate.unwrap_or(30),
                    sha256,
                }
            }
            Progress::Ready(mut r) => {
                r.unified |= r.receipt.get("web_ranges").is_some();
                // A continuation may have been checkpointed by an older
                // service before public summaries omitted player IDs.
                if let Some(value) = r.receipt.get_mut("summary") {
                    let mut summary = serde_json::from_value(value.take())
                        .map_err(|_| invalid("invalid saved film summary"))?;
                    live_proto::film::privacy::public_summary(&mut summary);
                    *value = json!(summary);
                }
                r
            }
        };
        let _guard = self.live.host_game_writes.lock(self.id).await;
        if !self.published()? && !self.live.model.read().films.imports.contains_key(self.id) {
            let result = ready
                .candidate
                .spool
                .prepare(
                    self.live,
                    self.id,
                    &self.publisher,
                    self.grant,
                    self.store,
                    ready.receipt.clone(),
                    ready.started_ms,
                    ready.hz,
                    ready.unified,
                    IMPORT_PAGES,
                )
                .await?;
            let Some((events, game_version, _pending)) = result else {
                slot.save(self.store, &Progress::Ready(ready)).await?;
                return Ok(StatusCode::ACCEPTED);
            };
            let expected = [
                (live_proto::game::stream(self.id), game_version),
                (super::import_stream(self.id), 0),
                super::legacy::selection_fence(self.live, self.id, &ready.candidate.blob)?,
            ];
            match self.live.store.append(events, &expected).await {
                Ok(rows) => {
                    self.live
                        .wait_applied(rows.last().expect("receipt").position)
                        .await?
                }
                Err(crate::store::StoreError::Conflict { .. }) => {
                    self.live
                        .wait_applied(self.live.store.head().await?)
                        .await?;
                    if !self.published()?
                        && !self.live.model.read().films.imports.contains_key(self.id)
                    {
                        slot.save(self.store, &Progress::Ready(ready)).await?;
                        return Ok(StatusCode::ACCEPTED);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        drop(_guard);
        self.acknowledge().await
    }
}

/// The durable legacy queue retains only the verified reservation and the
/// chosen candidate key. Consume one slice in the same path as a caller's
/// continuation. A stale job can never select another recording for the game.
pub(super) async fn continue_legacy(
    live: &Live,
    id: &str,
    grant: &FilmReservation,
    candidate_blob: &str,
) -> ApiResult<bool> {
    let store = &live
        .blobs()
        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the film store is unavailable"))?
        .private;
    let upload = Upload {
        live,
        store,
        id,
        grant,
        publisher: grant.publisher(),
    };
    let _ingest = live.film_uploads.lock(id).await;
    if upload.published()? || live.model.read().films.imports.contains_key(id) {
        return Ok(true);
    }
    let (mut slot, progress) = Slot::legacy(store, id, candidate_blob).await?;
    let Some(progress) = progress else {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "the queued film cursor is unavailable",
        ));
    };
    if progress.blob() != candidate_blob {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "the queued film cursor is inconsistent",
        ));
    }
    let status = upload.advance(&mut slot, progress, false).await?;
    Ok(status == StatusCode::NO_CONTENT || live.model.read().films.imports.contains_key(id))
}

pub(crate) async fn upload_host_film(
    State(live): Shared,
    headers: HeaderMap,
    method: Method,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> ApiResult<StatusCode> {
    let grant = live.host_claims(bearer(&headers), TELEMETRY_INGEST)?;
    game_id(&id)?;
    let request = query.request(method)?;
    let client_continues = headers.get("x-h2-film-continue").is_some_and(|v| v == "1");
    let legacy_seal =
        !client_continues && matches!(request, Request::Single | Request::Complete(..));
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    let limit = match request {
        Request::Part(_) => PART_BYTES,
        Request::Single => SINGLE_UPLOAD_BYTES,
        Request::Continue | Request::Complete(..) => 0,
    };
    if declared.is_some_and(|n| n > limit) {
        return Err(too_large());
    }
    let store = &live
        .blobs()
        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the film store is unavailable"))?
        .private;
    let upload = Upload {
        live: &live,
        store,
        id: &id,
        grant: &grant,
        publisher: grant.publisher(),
    };
    // Serialise only this game's cursor, including on the local directory
    // backend (which has a single owner). R2/S3 additionally compare ETags.
    let _ingest = live.film_uploads.lock(&id).await;
    if upload.published()? || live.model.read().films.imports.contains_key(&id) {
        return upload.acknowledge().await;
    }
    if let Some(chosen) = super::legacy::selected_blob(&live, &id) {
        let (mut slot, queued) = Slot::legacy(store, &id, &chosen).await?;
        let chosen = queued.filter(|p| p.blob() == chosen).ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unavailable,
                "the queued film cursor is unavailable",
            )
        })?;
        return upload.advance(&mut slot, chosen, false).await;
    }
    let (mut slot, previous) = Slot::load(store, &id).await?;
    let previous = match previous {
        Some(p) if &p.candidate().reservation != &*grant => {
            if matches!(request, Request::Part(0) | Request::Single) {
                None
            } else {
                return Err(forbidden(
                    "film ingestion belongs to another host reservation",
                ));
            }
        }
        p => p,
    };
    let single_post = matches!(request, Request::Single);
    let (progress, first_pass_done) = match request {
        Request::Single => (upload.single(body, declared).await?, true),
        other => {
            let bytes = to_bytes(body, limit as usize)
                .await
                .map_err(|_| too_large())?;
            if declared.is_some_and(|n| n != bytes.len() as u64) {
                return Err(invalid("the film upload was cut off"));
            }
            match other {
                Request::Part(n) => upload.part(previous, n, bytes, client_continues).await?,
                Request::Continue => (
                    previous.ok_or_else(|| invalid("no staged host film"))?,
                    false,
                ),
                Request::Complete(size, hash) => match previous {
                    Some(Progress::Reading(i, mut c)) if c.multipart() && c.size == size => {
                        c.seal = Some(hash);
                        (Progress::Reading(i, c), false)
                    }
                    Some(Progress::Ready(r)) if r.candidate.size == size && r.sha256 == hash => {
                        (Progress::Ready(r), false)
                    }
                    _ => return Err(invalid("missing host film parts or wrong digest")),
                },
                Request::Single => unreachable!(),
            }
        }
    };
    // This object was created exclusively by this request. A rejected direct
    // submission with no durable continuation will never be selected.
    let fresh_key = match &progress {
        Progress::Reading(_, c) if single_post => Some(c.blob.clone()),
        _ => None,
    };
    let legacy_candidate = legacy_seal.then(|| progress.blob().to_owned());
    if let Some(candidate) = &legacy_candidate {
        slot = Slot::legacy(store, &id, candidate).await?.0;
    }
    let written_candidate = progress.blob().to_owned();
    let open_part =
        matches!(&progress, Progress::Reading(_, c) if c.multipart() && c.seal.is_none());
    let mut result = upload.advance(&mut slot, progress, first_pass_done).await;
    if let Some(candidate) = legacy_candidate
        && matches!(result, Ok(StatusCode::ACCEPTED))
        && let Err(err) = super::legacy::enrol(&live, &id, &grant, &candidate).await
    {
        result = Err(err);
    }
    // A late PUT/POST may have left its own object, cursor or stats pages
    // after the other replica's first GC pass. Settled retries wrote nothing.
    let cursor_race = result
        .as_ref()
        .is_err_and(|err| err.code == ErrorCode::Conflict);
    // A failed validator may have flushed temporary stats pages before
    // rejecting the candidate, even though no cursor was acknowledged.
    let wrote_cursor = result.is_err() || matches!(result, Ok(StatusCode::ACCEPTED)) || open_part;
    cleanup::reconcile(&live, &id, &written_candidate, cursor_race, wrote_cursor).await?;
    if let Some(key) = fresh_key
        && let Err(error) = &result
        && matches!(
            error.code,
            ErrorCode::Invalid | ErrorCode::Forbidden | ErrorCode::TooLarge
        )
    {
        if !upload.published().unwrap_or(true) && !live.model.read().films.imports.contains_key(&id)
        {
            let _ = store.delete(&key).await;
        }
    }
    result
}
