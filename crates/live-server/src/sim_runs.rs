//! Private Clanker Training matches. Both Live homes use the same game
//! stream, frame archive, blob store and HTTP handlers.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State, rejection::BytesRejection};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::game::{self, GameDetail, GameSummary, PlayerScore, SimGame};
use live_proto::{ErrorCode, Page, lxm};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found, now};
use crate::blobs::Meta;
use crate::http::{bearer, json};
use crate::model::Game;

pub(crate) const RECORDING_LIMITED: &str = "SimRecordingLimited";
pub(crate) const REPORT_STORED: &str = "SimReportStored";
pub(crate) const REPORT_PREFIX: &str = "sim/";
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_TICKS: u64 = 30 * 60 * 60;
pub(crate) const MAX_REPORT: usize = 512 * 1024;
const MAX_METADATA: usize = 16 * 1024;
const PAGE_SIZE: usize = 50;

pub(crate) fn is_sim_id(id: &str) -> bool {
    id.starts_with("sim-")
}

pub(crate) fn is_sim_stream(stream: &str) -> bool {
    stream.starts_with("game-sim-")
}

pub(crate) fn validate_start(sim: &SimGame, game: &game::NewGame) -> Result<(), ApiError> {
    if !game::valid_id(&sim.run_id)
        || sim.run_id.len() > 60
        || sim
            .batch_id
            .as_deref()
            .is_some_and(|id| !game::valid_id(id))
        || sim.seed.parse::<u64>().is_err()
        || !game::valid_id(&sim.gametype)
        || sim.pace.is_empty()
        || sim.pace.len() > 32
        || !(1..=4).contains(&sim.bots.len())
        || sim.bots.iter().any(|b| !b.is_object())
        || game.mode != "multiplayer"
        || game.kind() != "custom"
        || game.session_id.is_some()
        || game.players.iter().any(|p| !p.is_bot() || p.xuid.is_some())
    {
        return Err(invalid("invalid simulation game metadata"));
    }
    if serde_json::to_vec(sim).expect("serializes").len() > MAX_METADATA {
        return Err(too_large("simulation metadata is limited to 16 KiB"));
    }
    Ok(())
}

pub(crate) fn idle_seconds(sim: Option<&SimGame>, default: i64) -> i64 {
    // Deferred recordings may spend the whole match collecting locally.
    if sim.is_some_and(|s| s.deferred) {
        default.max(2 * 60 * 60)
    } else {
        default
    }
}

fn too_large(message: &str) -> ApiError {
    ApiError::new(ErrorCode::TooLarge, message)
}

fn blob_error(error: impl std::fmt::Display) -> ApiError {
    eprintln!("live: sim report: {error}");
    ApiError::new(
        ErrorCode::Unavailable,
        "the sim report store is unavailable",
    )
}

fn report_key(id: &str, digest: &str) -> String {
    format!("{REPORT_PREFIX}{id}/{digest}.json")
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct StoredReport {
    sha256: String,
    ticks: u64,
    speedup: f64,
    result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    determinism: Option<Value>,
}

#[derive(Serialize)]
struct Run<'a> {
    #[serde(flatten)]
    game: GameSummary,
    sim: &'a SimGame,
    status: &'static str,
    ticks: u64,
    speedup: Option<f64>,
    scoreboard: &'a [PlayerScore],
    result: Option<&'a Value>,
    determinism: Option<&'a Value>,
    has_report: bool,
    recording_limited: bool,
}

impl<'a> Run<'a> {
    fn new(game: &'a Game, sim: &'a SimGame, tick: Option<u64>, now: i64) -> Self {
        let report = game.sim_report.as_ref();
        let ticks = tick
            .or(game.last_tick)
            .unwrap_or(0)
            .saturating_sub(game.started.game.tick.unwrap_or(0));
        let status = match &game.ended {
            None => "running",
            Some((end, _)) if end.reason == "lost" && report.is_none() => "interrupted",
            Some(_) => "finished",
        };
        let wall = live_proto::sign::parse_time(&game.started_at)
            .map_or(0, |at| now.saturating_sub(at.unix_timestamp()));
        let speedup = if status == "running" && !sim.deferred && wall > 0 && ticks > 0 {
            Some(ticks as f64 / 30.0 / wall as f64)
        } else {
            None
        };
        Self {
            game: game.summary(),
            sim,
            status,
            ticks: report.map_or(ticks, |r| r.ticks),
            speedup: report.map(|r| r.speedup).or(speedup),
            scoreboard: &game.scoreboard,
            result: report.map(|r| &r.result),
            determinism: report.and_then(|r| r.determinism.as_ref()),
            has_report: report.is_some(),
            recording_limited: game.sim_recording_limited,
        }
    }
}

fn read_admin(live: &Live, headers: &HeaderMap) -> Result<(), ApiError> {
    let author = live.authenticate(bearer(headers), lxm::READ)?;
    crate::admin::require_admin(live, &author)
}

fn lookup<'a>(
    games: &'a HashMap<String, Game>,
    id: &str,
) -> Result<(&'a Game, &'a SimGame), ApiError> {
    games
        .get(id)
        .and_then(|game| Some((game, game.started.game.sim.as_ref()?)))
        .ok_or_else(|| not_found("no such sim run"))
}

#[derive(Default, Deserialize)]
pub(crate) struct Filter {
    map: Option<String>,
    tuning: Option<String>,
    batch: Option<String>,
    before: Option<u64>,
}

/// A dedicated start path makes an older server refuse an upload rather
/// than silently ignoring the metadata and creating a public game.
pub(crate) async fn start(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let new: game::NewGame =
        serde_json::from_slice(&body).map_err(|e| invalid(format!("sim game: {e}")))?;
    if new.sim.is_none() {
        return Err(invalid("simulation metadata is required"));
    }
    Ok(json(
        StatusCode::CREATED,
        &live.start_game(bearer(&headers), new).await?,
    ))
}

pub(crate) async fn list(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Query(filter): Query<Filter>,
) -> Result<Response, ApiError> {
    read_admin(&live, &headers)?;
    let state = live.model.read();
    let now = now();
    let tuning = filter
        .tuning
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase);
    let items: Vec<_> = state
        .games_newest()
        .filter(|g| filter.before.is_none_or(|b| g.position < b))
        .filter_map(|g| Some((g, g.started.game.sim.as_ref()?)))
        .filter(|(g, sim)| {
            filter
                .map
                .as_ref()
                .is_none_or(|map| g.started.game.map.eq_ignore_ascii_case(map))
                && filter
                    .batch
                    .as_ref()
                    .is_none_or(|id| sim.batch_id.as_ref() == Some(id))
                && tuning.as_ref().is_none_or(|term| {
                    sim.bots.iter().any(|bot| {
                        bot["arm"]
                            .as_str()
                            .is_some_and(|name| name.to_lowercase().contains(term.as_str()))
                            || bot["tuning"].as_object().is_some_and(|tuning| {
                                serde_json::to_string(tuning)
                                    .expect("serializes")
                                    .to_lowercase()
                                    .contains(term.as_str())
                            })
                    })
                })
        })
        .take(PAGE_SIZE)
        .collect();
    let next = items
        .last()
        .filter(|_| items.len() == PAGE_SIZE)
        .map(|(g, _)| g.position.to_string());
    let items = items
        .into_iter()
        .map(|(g, sim)| Run::new(g, sim, live.sim_tick(&g.started.game_id), now))
        .collect();
    Ok(json(
        StatusCode::OK,
        &Page {
            items,
            next,
            position: live.position(),
        },
    ))
}

#[derive(Serialize)]
struct Detail<'a> {
    run: Run<'a>,
    game: GameDetail,
    report: Option<Value>,
}

pub(crate) async fn detail(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    read_admin(&live, &headers)?;
    let game = {
        let state = live.model.read();
        lookup(&state.games, &id)?.0.clone()
    };
    let sim = game.started.game.sim.as_ref().expect("looked up a sim");
    let report = if let Some(index) = &game.sim_report {
        let blobs = live
            .blobs()
            .ok_or_else(|| blob_error("no private blob store"))?;
        let bytes = blobs
            .private
            .get(&report_key(&id, &index.sha256))
            .await
            .map_err(blob_error)?
            .ok_or_else(|| blob_error("missing report"))?;
        if live_proto::sign::hex(&Sha256::digest(&bytes)) != index.sha256 {
            return Err(blob_error("report hash did not match"));
        }
        Some(serde_json::from_slice(&bytes).map_err(blob_error)?)
    } else {
        None
    };
    Ok(json(
        StatusCode::OK,
        &Detail {
            run: Run::new(&game, sim, live.sim_tick(&id), now()),
            game: live.game(&id)?,
            report,
        },
    ))
}

fn parse_report(bytes: &[u8]) -> Result<(Value, StoredReport), ApiError> {
    if bytes.len() > MAX_REPORT {
        return Err(too_large("simulation reports are limited to 512 KiB"));
    }
    let report: Value =
        serde_json::from_slice(bytes).map_err(|e| invalid(format!("sim report: {e}")))?;
    if report["format"].as_str() != Some("h2-sim-report/1")
        || !report["match"].is_object()
        || !report["players"]
            .as_array()
            .is_some_and(|p| p.len() <= game::MAX_PLAYERS)
        || report["result"]["end"].as_str().is_none()
    {
        return Err(invalid("expected an h2-sim-report/1 report"));
    }
    let ticks = report["ticks"]
        .as_u64()
        .ok_or_else(|| invalid("report ticks must be an integer"))?;
    let speedup = report["wall"]["speedup"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| invalid("report wall.speedup must be a nonnegative number"))?;
    if ticks > MAX_TICKS {
        return Err(too_large(
            "simulation recordings are limited to one in-game hour",
        ));
    }
    let index = StoredReport {
        sha256: live_proto::sign::hex(&Sha256::digest(bytes)),
        ticks,
        speedup,
        result: report["result"].clone(),
        determinism: report.get("determinism").cloned(),
    };
    if serde_json::to_vec(&index).expect("serializes").len() > MAX_METADATA {
        return Err(too_large("simulation result metadata is limited to 16 KiB"));
    }
    Ok((report, index))
}

pub(crate) async fn upload(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let author = live.authenticate(bearer(&headers), lxm::GAMES_WRITE)?;
    crate::admin::require_admin(&live, &author)?;
    let bytes = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => too_large("simulation report is too large"),
        _ => invalid(e.body_text()),
    })?;
    let (report, index) = parse_report(&bytes)?;
    let _guard = live.sim_writes.lock().await;
    {
        let state = live.model.read();
        let (game, sim) = lookup(&state.games, &id)?;
        if game.started.host != author.to_string() {
            return Err(forbidden("only the sim host may upload its report"));
        }
        if game.ended.is_none() {
            return Err(conflict("the sim recording is still running"));
        }
        if report["match"]["seed"]
            .as_u64()
            .is_none_or(|seed| seed.to_string() != sim.seed)
            || report["match"]["game_type"].as_str() != Some(sim.gametype.as_str())
            || report["match"]["bot"].as_array() != Some(&sim.bots)
        {
            return Err(invalid("the report does not match the simulation"));
        }
        if let Some(old) = &game.sim_report {
            if old.sha256 == index.sha256 {
                return Ok(StatusCode::NO_CONTENT.into());
            }
            return Err(conflict("this run already has a report"));
        }
    }
    let blobs = live
        .blobs()
        .ok_or_else(|| blob_error("no private blob store"))?;
    let meta = Meta {
        content_type: Some("application/json".into()),
        ..Default::default()
    };
    blobs
        .private
        .put(&report_key(&id, &index.sha256), bytes, &meta)
        .await
        .map_err(blob_error)?;
    let event = live.server_event(
        game::stream(&id),
        REPORT_STORED,
        serde_json::to_value(index).expect("serializes"),
    );
    let stored = live.store().append(vec![event], &[]).await?;
    live.wait_applied(stored[0].position).await?;
    Ok(StatusCode::NO_CONTENT.into())
}
