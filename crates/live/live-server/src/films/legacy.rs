//! Older hosts treat any successful HTTP response as completion. A signed
//! receipt of the sealed submission schedules the same bounded work as a
//! foreground continuation. The journal contains no lease credential.
use std::{
    collections::{BTreeMap, HashMap},
    ops::Bound::{Excluded, Unbounded},
    sync::Mutex,
};

use live_proto::{ErrorCode, game};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{ApiResult, import_stream};
use crate::{
    app::{ApiError, Live, conflict, invalid},
    blobs::{BlobStore, Meta},
    host_identity::FilmReservation,
    store::{RecordedEvent, StoreError},
};

const MAX_PERMIT_BYTES: usize = 128 << 10;
pub(super) const QUEUED: &str = "HostFilmValidationQueued";
pub(super) const FINISHED: &str = "HostFilmValidationFinished";

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    game: String,
    /// Immutable candidate, selected by a verified lease upload.
    blob: String,
    /// Digest of the private reservation constraints used for this upload.
    permit: String,
    #[serde(skip)]
    accepted_at: i64,
}

fn journal(id: &str) -> String {
    format!("{}:validation", import_stream(id))
}

fn permit_key(job: &Job) -> String {
    format!("film-staging/{}/permit-{}", job.game, job.permit)
}

#[derive(Default)]
pub(super) struct Queue {
    pending: BTreeMap<String, Job>,
    versions: HashMap<String, u64>,
    last: Mutex<String>,
}

impl Queue {
    pub(super) fn project(&mut self, event: &RecordedEvent) {
        let Some(id) = event.body.data["game"].as_str() else {
            return;
        };
        if !game::valid_id(id) || event.body.stream != journal(id) {
            return;
        }
        self.versions.insert(id.to_owned(), event.version + 1);
        match event.body.kind.as_str() {
            QUEUED => {
                if let Ok(mut job) = serde_json::from_value::<Job>(event.body.data.clone()) {
                    job.accepted_at = live_proto::sign::parse_time(&event.body.at)
                        .map_or(0, |at| at.unix_timestamp());
                    self.pending.insert(id.to_owned(), job);
                }
            }
            FINISHED
                if self
                    .pending
                    .get(id)
                    .is_some_and(|j| Some(j.blob.as_str()) == event.body.data["blob"].as_str()) =>
            {
                self.pending.remove(id);
            }
            _ => {}
        }
    }

    pub(super) fn published(&mut self, id: &str) {
        self.pending.remove(id);
        self.versions.remove(id);
    }

    // Use a fixed due time. Rescheduling the Worker alarm after a foreground
    // read must not push validation into the future on a busy server.
    pub(super) fn due(&self) -> Option<i64> {
        self.pending
            .first_key_value()
            .map(|(_, job)| job.accepted_at.saturating_add(1))
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.pending.contains_key(id)
    }

    fn next(&self) -> Option<(Job, u64)> {
        let mut cursor = self.last.lock().expect("film validation cursor");
        let (id, job) = self
            .pending
            .range((Excluded(cursor.clone()), Unbounded))
            .next()
            .or_else(|| self.pending.first_key_value())?;
        cursor.clone_from(id);
        Some((job.clone(), self.versions[id]))
    }
}

pub(super) fn selected_blob(live: &Live, id: &str) -> Option<String> {
    live.model
        .read()
        .films
        .legacy
        .pending
        .get(id)
        .map(|j| j.blob.clone())
}

/// Couple receipt selection to this server's observed legacy promise.
pub(super) fn selection_fence(live: &Live, id: &str, candidate: &str) -> ApiResult<(String, u64)> {
    let model = live.model.read();
    let queue = &model.films.legacy;
    if queue
        .pending
        .get(id)
        .is_some_and(|job| job.blob != candidate)
    {
        return Err(conflict("another film is pending"));
    }
    Ok((journal(id), queue.versions.get(id).copied().unwrap_or(0)))
}

fn unavailable() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "staged film authorisation is unavailable",
    )
}

async fn record(
    live: &Live,
    stream: String,
    version: u64,
    kind: &str,
    data: serde_json::Value,
    also: Option<(String, u64)>,
) -> ApiResult<()> {
    let event = live.server_event(stream.clone(), kind, data);
    let mut expected = vec![(stream, version)];
    expected.extend(also);
    let position = match live.store.append(vec![event], &expected).await {
        Ok(rows) => rows[0].position,
        Err(StoreError::Conflict { .. }) => live.store.head().await?,
        Err(e) => return Err(e.into()),
    };
    live.wait_applied(position).await
}

fn selected_receipt(live: &Live, id: &str) -> Option<String> {
    let model = live.model.read();
    model
        .films
        .imports
        .get(id)
        .and_then(|p| p.receipt["blob"].as_str().map(str::to_owned))
        .or_else(|| {
            model.games.get(id).filter(|g| g.has_host_film).map(|g| {
                g.film_blob
                    .clone()
                    .unwrap_or_else(|| super::host_film_key(id))
            })
        })
        .or_else(|| {
            model
                .films
                .standalone_host_blob(id)
                .map(|(_, blob)| blob.to_owned())
        })
}

/// The caller holds this game's ingestion lock and has already persisted the
/// exact candidate cursor. Return a success response only after the signed
/// work item is observable to both the native sweep and the Worker alarm.
pub(super) async fn enrol(
    live: &Live,
    id: &str,
    grant: &FilmReservation,
    blob: &str,
) -> ApiResult<()> {
    if let Some(selected) = selected_receipt(live, id) {
        return if selected == blob {
            Ok(())
        } else {
            Err(conflict("another film was published"))
        };
    }
    let version = {
        let model = live.model.read();
        let queue = &model.films.legacy;
        if let Some(job) = queue.pending.get(id) {
            return if job.blob == blob {
                Ok(())
            } else {
                Err(conflict("another film is pending"))
            };
        }
        queue.versions.get(id).copied().unwrap_or(0)
    };
    let bytes = serde_json::to_vec(grant).map_err(|_| invalid("encoding film reservation"))?;
    if bytes.len() > MAX_PERMIT_BYTES {
        return Err(invalid("film reservation too large"));
    }
    let job = Job {
        game: id.to_owned(),
        blob: blob.to_owned(),
        permit: format!("{:x}", Sha256::digest(&bytes)),
        accepted_at: 0,
    };
    let stores = live.blobs().ok_or_else(unavailable)?;
    stores
        .private
        .put(&permit_key(&job), bytes.into(), &Meta::default())
        .await
        .map_err(|_| unavailable())?;
    record(
        live,
        journal(id),
        version,
        QUEUED,
        json!(job),
        Some((import_stream(id), 0)),
    )
    .await?;
    if selected_blob(live, id).as_deref() == Some(blob)
        || selected_receipt(live, id).as_deref() == Some(blob)
    {
        Ok(())
    } else {
        Err(conflict("film validation changed; retry"))
    }
}

async fn reservation(store: &BlobStore, job: &Job) -> ApiResult<FilmReservation> {
    let bytes = store
        .get_prefix(&permit_key(job), MAX_PERMIT_BYTES as u64 + 1)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if format!("{:x}", Sha256::digest(&bytes)) != job.permit {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("invalid staged reservation"))
}

/// One candidate and at most one validation/import budget per sweep.
pub(crate) async fn advance(live: &Live) {
    let Some((job, version)) = live.model.read().films.legacy.next() else {
        return;
    };
    let result: ApiResult<bool> = async {
        if selected_receipt(live, &job.game).is_some() {
            return Ok(true);
        }
        let stores = live.blobs().ok_or_else(unavailable)?;
        let grant = reservation(&stores.private, &job).await?;
        super::upload::continue_legacy(live, &job.game, &grant, &job.blob).await
    }
    .await;
    let reason = match result {
        Ok(false) => return,
        Ok(true) if selected_blob(live, &job.game).as_deref() != Some(job.blob.as_str()) => return,
        Ok(true) => "superseded",
        Err(e)
            if matches!(
                e.code,
                ErrorCode::Invalid
                    | ErrorCode::Forbidden
                    | ErrorCode::TooLarge
                    | ErrorCode::NotFound
            ) =>
        {
            eprintln!(
                "live: rejected queued host film for {}: {}",
                job.game, e.message
            );
            "rejected"
        }
        Err(e) => {
            eprintln!(
                "live: continuing queued host film for {}: {}",
                job.game, e.message
            );
            return;
        }
    };
    // CAS protects a newer candidate when the failed one was superseded.
    if let Err(e) = record(
        live,
        journal(&job.game),
        version,
        FINISHED,
        json!({"game":job.game,"blob":job.blob,"reason":reason}),
        None,
    )
    .await
    {
        eprintln!(
            "live: retiring queued host film for {}: {}",
            job.game, e.message
        );
    }
}
