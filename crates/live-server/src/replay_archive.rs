//! Finished-game replay source inventory. Shared by the native server and
//! LiveHub. Counts and the source revision are projected from the signed log,
//! so listing a page reads neither frame payloads nor the blob store.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
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
}

#[derive(Serialize)]
struct InventoryItem<'a> {
    game_id: &'a str,
    ended: bool,
    state: &'static str,
    source_position: u64,
    chunks: u64,
    legacy_frames: u64,
}

fn inventory_item(game: &Game) -> InventoryItem<'_> {
    let r = &game.recording;
    let ended = game.ended.is_some();
    let state = if !ended {
        "live"
    } else if r.chunks == 0 && r.legacy_frames == 0 {
        "empty"
    } else {
        "raw"
    };
    InventoryItem {
        game_id: &game.started.game_id,
        ended,
        state,
        source_position: r.position,
        chunks: r.chunks,
        legacy_frames: r.legacy_frames,
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
