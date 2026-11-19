//! Finished-game replay source inventory. Shared by the native server and
//! LiveHub. Counts and the source revision are projected from the signed log,
//! so listing a page reads neither frame payloads nor the blob store.

mod job;
mod source;

pub(crate) const REQUESTED: &str = "ReplayArchiveRequested";
const DEFERRED: &str = "ReplayArchiveDeferred";

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::Live;
use crate::app::{ApiError, invalid, not_found};
use crate::http::{bearer, json};
use crate::invites::{LXM_ADMIN_READ, admin};
use crate::model::Game;

/// The most recent source or end event identifies a game's recording.
/// A late chunk updates this revision even if GameEnded is already durable.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Recording {
    pub position: u64,
    pub chunks: u64,
    pub legacy_frames: u64,
    pub next_version: u64,
    pub requested: bool,
    retry_at: i64,
    pub published: bool,
    pub rolled_back: bool,
    pub parts: Arc<Vec<job::Part>>,
}

impl Recording {
    pub(crate) fn due(&self, ended: bool) -> Option<i64> {
        (ended
            && self.requested
            && !self.published
            && !self.rolled_back
            && self.chunks + self.legacy_frames > 0
            && self.retry_at != i64::MAX)
            .then_some(self.retry_at)
    }

    pub(crate) fn observe(&mut self, event: &crate::store::RecordedEvent) {
        use live_proto::{
            chunk::FRAME_CHUNK,
            game::{GAME_ENDED, GAME_FRAME},
        };
        self.next_version = event.version + 1;
        match event.body.kind.as_str() {
            GAME_FRAME | FRAME_CHUNK | GAME_ENDED => {
                self.position = event.position;
                self.retry_at = 0;
                self.parts = Arc::default();
                self.published = false;
                self.rolled_back = false;
                if event.body.kind == FRAME_CHUNK {
                    self.chunks += 1;
                }
                if event.body.kind == GAME_FRAME {
                    self.legacy_frames += 1;
                }
            }
            REQUESTED => self.requested = true,
            DEFERRED => {
                if event.body.data["source_position"].as_u64() == Some(self.position) {
                    self.retry_at = event.body.data["retry_at"].as_i64().unwrap_or(i64::MAX);
                }
            }
            job::PART => {
                let Ok(part) = serde_json::from_value::<job::Part>(event.body.data.clone()) else {
                    return;
                };
                if part.progress.source_position != self.position {
                    return;
                }
                if self.rolled_back && part.progress.totals.segments == 1 {
                    self.parts = Arc::default();
                    self.rolled_back = false;
                }
                if !self.published && part.progress.totals.segments == self.parts.len() as u64 + 1 {
                    Arc::make_mut(&mut self.parts).push(part);
                }
            }
            job::PUBLISHED | job::ROLLED_BACK => {
                if let Ok(p) = serde_json::from_value::<job::Progress>(event.body.data.clone())
                    && self
                        .parts
                        .last()
                        .is_some_and(|part| part.done && part.progress == p)
                {
                    self.published = event.body.kind == job::PUBLISHED;
                    self.rolled_back = !self.published;
                }
            }
            _ => {}
        }
    }
}

#[derive(Serialize)]
struct InventoryItem<'a> {
    game_id: &'a str,
    ended: bool,
    state: &'static str,
    source_position: u64,
    chunks: u64,
    legacy_frames: u64,
    archive_sha256: Option<String>,
    backup_sha256: Option<String>,
    totals: job::Totals,
}

fn inventory_item(game: &Game) -> InventoryItem<'_> {
    let r = &game.recording;
    let ended = game.ended.is_some();
    let state = r.state(ended);
    let p = r.progress();
    InventoryItem {
        game_id: &game.started.game_id,
        ended,
        state,
        source_position: r.position,
        chunks: r.chunks,
        legacy_frames: r.legacy_frames,
        archive_sha256: p.archive_sha256,
        backup_sha256: p.backup_sha256,
        totals: p.totals,
    }
}

#[derive(Deserialize)]
pub(crate) struct InventoryQuery {
    cursor: Option<u64>,
    limit: Option<usize>,
}

pub(crate) async fn inventory(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Query(query): Query<InventoryQuery>,
) -> Result<Response, ApiError> {
    admin(&live, bearer(&headers), LXM_ADMIN_READ)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(invalid("limit must be between 1 and 100"));
    }
    let model = live.model.read();
    let mut games = model.games_before(query.cursor);
    let mut items = Vec::with_capacity(limit);
    let mut last = 0;
    for g in games.by_ref().take(limit) {
        last = g.position;
        items.push(inventory_item(g));
    }
    let next = games.next().map(|_| last.to_string());
    Ok(json(
        StatusCode::OK,
        &live_proto::Page {
            items,
            next,
            position: live.position(),
        },
    ))
}

pub(crate) async fn detail(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    admin(&live, bearer(&headers), LXM_ADMIN_READ)?;
    let model = live.model.read();
    let game = model
        .games
        .get(&id)
        .ok_or_else(|| not_found("no such game"))?;
    Ok(json(StatusCode::OK, &inventory_item(game)))
}

pub(crate) async fn run_step(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(request): axum::Json<job::Request>,
) -> Result<Response, ApiError> {
    let author = admin(&live, bearer(&headers), crate::admin::LXM_ADMIN_WRITE)?;
    live.rate_limit(&author, crate::admin::LXM_ADMIN_WRITE)?;
    let _slot = live.archive_work.lock().await;
    let result = job::step(&live, &id, request).await?;
    Ok(json(StatusCode::OK, &result))
}

pub(crate) async fn rollback(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(request): axum::Json<job::Rollback>,
) -> Result<Response, ApiError> {
    let author = admin(&live, bearer(&headers), crate::admin::LXM_ADMIN_WRITE)?;
    live.rate_limit(&author, crate::admin::LXM_ADMIN_WRITE)?;
    job::rollback(&live, &id, request).await?;
    let model = live.model.read();
    Ok(json(StatusCode::OK, &inventory_item(&model.games[&id])))
}

/// Opted-in readers can discover an archive only after the final part and
/// its publication event commit. Old readers retain their original index.
pub(crate) fn index(live: &Live, id: &str) -> Option<Response> {
    let model = live.model.read();
    let game = model.games.get(id)?;
    let r = &game.recording;
    if !r.published {
        return None;
    }
    let parts = r.parts.iter().map(|p| &p.segment).collect::<Vec<_>>();
    Some(json(
        StatusCode::OK,
        &serde_json::json!({
            "game_id":id,"ended":true,"archive_sha256":r.progress().archive_sha256,
            "chunk_ticks":live_proto::archive::SPAN_TICKS,"items":parts
        }),
    ))
}

pub(crate) async fn segment(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path((id, index, number)): Path<(String, String, usize)>,
) -> Result<Response, ApiError> {
    use axum::http::header;
    let token = bearer(&headers);
    let viewer = live.read_viewer(token)?;
    live.check_game_shows(&id, viewer)?;
    let hash = {
        let model = live.model.read();
        let game = model
            .games
            .get(&id)
            .ok_or_else(|| not_found("no such game"))?;
        let r = &game.recording;
        if !r.published || r.progress().archive_sha256.as_deref() != Some(&index) {
            return Err(not_found("archive is not published"));
        }
        r.parts
            .get(number)
            .ok_or_else(|| not_found("no such archive segment"))?
            .segment
            .sha256
            .clone()
    };
    // The storage digest is for these Brotli bytes. HTTP handles inflation.
    let bytes = job::read_blob(&live, job::ARCHIVE, &hash).await?;
    let cache = if headers.contains_key(header::AUTHORIZATION) {
        crate::caching::PRIVATE
    } else {
        crate::caching::IMMUTABLE
    };
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-h2-replay"),
            (header::CONTENT_ENCODING, "br"),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response())
}

/// The native timer and the Durable Object alarm each claim at most one
/// segment. A restart resumes from the last signed part. Historic games
/// without a request remain available for an operator's migration dry run.
pub(crate) async fn advance(live: &Live, now: i64) {
    let Ok(_slot) = live.archive_work.try_lock() else {
        return;
    };
    let candidate = {
        let model = live.model.read();
        model
            .games
            .values()
            .filter_map(|g| {
                let r = &g.recording;
                let due = r.due(g.ended.is_some())?;
                (due <= now).then_some((r.position, g.started.game_id.clone(), r.next_version))
            })
            .min_by_key(|(position, _, _)| *position)
    };
    let Some((position, id, version)) = candidate else {
        return;
    };
    if let Err(err) = job::step(
        live,
        &id,
        job::Request {
            mode: job::Mode::Apply,
            source_position: Some(position),
            cursor: None,
        },
    )
    .await
    {
        if err.code == live_proto::ErrorCode::Conflict {
            return;
        }
        // Retry transient storage failures. An invalid recording is seen
        // again on source revision or on an explicit admin step.
        let retry_at = if err.code == live_proto::ErrorCode::Unavailable {
            now + 60
        } else {
            i64::MAX
        };
        let entry =
            serde_json::json!({"source_position":position,"retry_at":retry_at,"error":err.message});
        if let Err(e) = job::append(live, &id, version, vec![(DEFERRED, entry)]).await {
            eprintln!("live: could not defer replay archive {id}: {e}");
        }
    }
}
