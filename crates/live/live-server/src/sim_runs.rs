//! Private Clanker Training matches. Both Live homes use the same game
//! stream, frame archive, blob store and HTTP handlers.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State, rejection::BytesRejection};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use live_proto::game::{self, GameDetail, GameSummary, PlayerScore, SimGame};
use live_proto::hosts::{EndReason, HostClass, HostLease, HostState, ProviderKind, Transport};
use live_proto::sign::Jws;
use live_proto::{Author, ErrorCode, Page, lxm};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found, now, unauthorized};
use crate::blobs::{BlobError, Meta, Seen};
use crate::http::{bearer, json};
use crate::identity::ServerIdentity;
use crate::model::Game;

pub(crate) const RECORDING_LIMITED: &str = "SimRecordingLimited";
pub(crate) const REPORT_STORED: &str = "SimReportStored";
pub(crate) const REPORT_PREFIX: &str = "sim/";
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_TICKS: u64 = 30 * 60 * 60;
pub(crate) const MAX_REPORT: usize = 512 * 1024;
const MAX_METADATA: usize = 16 * 1024;
const PAGE_SIZE: usize = 50;
/// One of the retail session's peer slots belongs to the playerless host.
const MAX_SIM_BOTS: usize = game::MAX_PLAYERS - 1;

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
        || !(1..=MAX_SIM_BOTS).contains(&sim.bots.len())
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

pub(crate) fn validate_hosted_start(sim: &SimGame, game: &game::NewGame) -> Result<(), ApiError> {
    if sim.pace != "unpaced" {
        return Err(invalid("dedicated simulations must be unpaced"));
    }
    validate_start(sim, game)
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
    let report = read_report(&live, &id, game.sim_report.as_ref()).await?;
    Ok(json(
        StatusCode::OK,
        &Detail {
            run: Run::new(&game, sim, live.sim_tick(&id), now()),
            game: live.game(&id)?,
            report,
        },
    ))
}

async fn read_report(
    live: &Live,
    id: &str,
    index: Option<&StoredReport>,
) -> Result<Option<Value>, ApiError> {
    let Some(index) = index else {
        return Ok(None);
    };
    let blobs = live
        .blobs()
        .ok_or_else(|| blob_error("no private blob store"))?;
    let bytes = blobs
        .private
        .get(&report_key(id, &index.sha256))
        .await
        .map_err(blob_error)?
        .ok_or_else(|| blob_error("missing report"))?;
    if live_proto::sign::hex(&Sha256::digest(&bytes)) != index.sha256 {
        return Err(blob_error("report hash did not match"));
    }
    Ok(Some(serde_json::from_slice(&bytes).map_err(blob_error)?))
}

pub(crate) fn host_routes() -> Router<Arc<Live>> {
    Router::new()
        .route("/v1/admin/sim-runs/batches/{batch_id}", post(open_host_batch))
        .route(
            "/v1/admin/sim-runs/{id}/hosts",
            get(poll_host).post(request_host),
        )
        .route("/v1/hosts/{lease}/sim", get(host_match))
        .layer(DefaultBodyLimit::max(MAX_METADATA))
}

const HOST_BATCH_TOKEN: &str = "h2live-sim-host-batch";
const HOST_BATCH_TTL_S: i64 = 24 * 60 * 60;
// Keep the lease limit aligned with sim::batch::max_match_s. The sim
// process's own clock starts after data provisioning.
const DEFAULT_MAX_MATCH_S: u32 = 1800;
const MAX_MATCH_S: u64 = 7200;
const STARTUP_AND_UPLOAD_S: u64 = 60;

/// A single-use admin token opens a batch. Only these sim routes accept
/// the resulting grant, which can start and poll runs for one batch.
#[derive(Deserialize, Serialize)]
struct HostBatchGrant {
    aud: String,
    author: String,
    batch_id: String,
    provider: ProviderKind,
    expires_at: i64,
}

impl HostBatchGrant {
    fn verify(raw: &str, identity: &ServerIdentity, at: i64) -> Result<Self, ApiError> {
        let bad = |_| unauthorized("invalid simulation batch token");
        let jws = Jws::parse(raw).map_err(bad)?;
        let grant: Self = serde_json::from_value(jws.json_payload(HOST_BATCH_TOKEN).map_err(bad)?)
            .map_err(|_| unauthorized("invalid simulation batch token"))?;
        jws.verify(identity.key.public()).map_err(bad)?;
        if grant.aud != identity.id || grant.expires_at <= at {
            return Err(unauthorized("invalid or expired simulation batch token"));
        }
        Ok(grant)
    }

    fn authenticate(live: &Live, headers: &HeaderMap) -> Result<Self, ApiError> {
        let raw = bearer(headers).ok_or_else(|| unauthorized("simulation batch token required"))?;
        let grant = Self::verify(raw, &live.identity, now())?;
        let author = Author::parse(&grant.author)
            .ok_or_else(|| unauthorized("invalid simulation batch token author"))?;
        crate::admin::require_admin(live, &author)?;
        Ok(grant)
    }
}

async fn open_host_batch(
    State(live): State<Arc<Live>>,
    Path(batch_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let author = live.authenticate(bearer(&headers), lxm::GAMES_WRITE)?;
    crate::admin::require_admin(&live, &author)?;
    if !game::valid_id(&batch_id) {
        return Err(invalid("invalid simulation batch id"));
    }
    let provider = cloud_provider(&live)?;
    let max_parallel = live.max_hosts();
    if max_parallel == 0 {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "dedicated hosts are disabled",
        ));
    }
    host_match_store(&live)?;
    let grant = HostBatchGrant {
        aud: live.identity.id.clone(),
        author: author.to_string(),
        batch_id,
        provider,
        expires_at: now() + HOST_BATCH_TTL_S,
    };
    let token = live.identity.key.sign_json(HOST_BATCH_TOKEN, &grant);
    Ok(private_json(
        StatusCode::CREATED,
        &serde_json::json!({
            "token": token,
            "provider": grant.provider,
            "max_parallel": max_parallel,
        }),
    ))
}

fn cloud_provider(live: &Live) -> Result<ProviderKind, ApiError> {
    live.host_provider_kind()
        .filter(|kind| matches!(kind, ProviderKind::Cloudflare | ProviderKind::Fly))
        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "no cloud host provider configured"))
}

fn private_json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let mut reply = json(status, body);
    reply
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    reply
}

#[derive(Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HostMatch {
    batch_id: String,
    #[serde(rename = "match")]
    matched: Value,
}

#[derive(Deserialize)]
struct MatchFields {
    map: String,
    game_type: Option<String>,
    pace: Option<String>,
    max_wall_s: Option<u64>,
    #[serde(rename = "bot")]
    bots: Vec<MatchBot>,
}

#[derive(Deserialize)]
struct MatchBot {
    name: Option<String>,
    params: Option<Value>,
    /// Every bot is a phantom peer; `kind = "local"` is refused with an
    /// error rather than silently ignored.
    #[serde(default = "phantom_kind")]
    kind: String,
}

fn phantom_kind() -> String {
    "phantom".into()
}

impl HostMatch {
    fn lease(&self, assignment_id: String, provider: ProviderKind) -> Result<HostLease, ApiError> {
        let m = MatchFields::deserialize(&self.matched)
            .map_err(|err| invalid(format!("simulation match file: {err}")))?;
        if !game::valid_id(&m.map) {
            return Err(invalid("invalid simulation map name"));
        }
        if m.pace.as_deref().is_some_and(|pace| pace != "unpaced") {
            return Err(invalid("dedicated simulations must be unpaced"));
        }
        for bot in &m.bots {
            match bot.kind.as_str() {
                "phantom" => {}
                "local" => return Err(invalid(
                    "kind = \"local\" is unsupported. Controller seats are for people. Use phantom bots",
                )),
                other => return Err(invalid(format!("unknown bot kind {other:?}. Use phantom"))),
            }
        }
        if !(1..=MAX_SIM_BOTS).contains(&m.bots.len()) {
            return Err(invalid(
                "a simulation needs 1 to 15 phantom bots: the host occupies one of 16 peers",
            ));
        }
        if m.bots.iter().any(|bot| bot.params.is_some()) {
            return Err(invalid("simulation matches must inline all bot tuning"));
        }
        let bots: Vec<_> = m
            .bots
            .into_iter()
            .enumerate()
            .map(|(seat, bot)| {
                bot.name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Clanker {seat}"))
            })
            .collect();
        if bots.iter().any(|name| name.chars().count() > 32) {
            return Err(invalid("simulation bot names are at most 32 characters"));
        }
        let max_match_s = m.max_wall_s.map_or(DEFAULT_MAX_MATCH_S, |wall| {
            wall.saturating_add(STARTUP_AND_UPLOAD_S).min(MAX_MATCH_S) as u32
        });
        let transport = match provider {
            ProviderKind::Cloudflare => Transport::Tunnel,
            ProviderKind::Fly => Transport::Udp,
            ProviderKind::Local => return Err(invalid("simulation hosts require a cloud provider")),
        };
        Ok(HostLease {
            id: String::new(),
            assignment_id,
            provider,
            region: None,
            class: if bots.len() <= 4 {
                HostClass::Small
            } else {
                HostClass::Large
            },
            map: m.map.to_ascii_lowercase(),
            map_id: None,
            variant: m.game_type,
            hopper: None,
            max_players: bots.len() as u8,
            roster: vec![],
            bots,
            transport,
            // The host's roster contains only bots. Let the hard deadline
            // bound its lifetime even when the player-idle count is zero.
            idle_timeout_s: max_match_s,
            max_match_s,
            created_ms: 0,
        })
    }
}

fn host_match_key(game_id: &str) -> String {
    format!("{REPORT_PREFIX}{game_id}/host-match.json")
}

fn checked_sim_game_id(run_id: &str) -> Result<String, ApiError> {
    if run_id.len() > 60 || !game::valid_id(run_id) {
        return Err(invalid("invalid simulation run id"));
    }
    Ok(game::sim_game_id(run_id))
}

fn host_match_store(live: &Live) -> Result<&crate::blobs::BlobStore, ApiError> {
    live.blobs()
        .map(|blobs| &blobs.private)
        .ok_or_else(|| blob_error("no private blob store"))
}

async fn read_host_match(live: &Live, game_id: &str) -> Result<Option<HostMatch>, ApiError> {
    host_match_store(live)?
        .get(&host_match_key(game_id))
        .await
        .map_err(blob_error)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(blob_error))
        .transpose()
}

async fn reserve_host_match(live: &Live, game_id: &str, spec: &HostMatch) -> Result<(), ApiError> {
    {
        // Also serializes creates on blob stores without conditional puts.
        // Release before calling the provider so the host can start writing.
        let _guard = live.sim_writes.lock().await;
        if let Some(old) = read_host_match(live, game_id).await? {
            return same_host_match(spec, old);
        }
        if live.model.read().games.contains_key(game_id) {
            return Err(conflict("that simulation run id is already in use"));
        }
        let private = host_match_store(live)?;
        let meta = Meta {
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let bytes = Bytes::from(serde_json::to_vec(spec).expect("serializes"));
        match private
            .put_if(&host_match_key(game_id), bytes, &Seen::default(), &meta)
            .await
        {
            Ok(_) => return Ok(()),
            Err(BlobError::Changed(_)) => {}
            Err(err) => return Err(blob_error(err)),
        }
    }
    let old = read_host_match(live, game_id)
        .await?
        .ok_or_else(|| conflict("simulation match reservation changed"))?;
    same_host_match(spec, old)
}

fn same_host_match(requested: &HostMatch, old: HostMatch) -> Result<(), ApiError> {
    if *requested != old {
        return Err(conflict(
            "that simulation run id already reserves a different match",
        ));
    }
    Ok(())
}

async fn request_host(
    State(live): State<Arc<Live>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let grant = HostBatchGrant::authenticate(&live, &headers)?;
    let game_id = checked_sim_game_id(&run_id)?;
    let bytes = body.map_err(|err| match err.status() {
        StatusCode::PAYLOAD_TOO_LARGE => too_large("simulation match files are limited to 16 KiB"),
        _ => invalid(err.body_text()),
    })?;
    let spec: HostMatch =
        serde_json::from_slice(&bytes).map_err(|err| invalid(format!("simulation match: {err}")))?;
    if spec.batch_id != grant.batch_id {
        return Err(forbidden("simulation batch token does not cover this match"));
    }
    if cloud_provider(&live)? != grant.provider {
        return Err(conflict("the cloud host provider changed during this batch"));
    }
    let lease = spec.lease(game_id.clone(), grant.provider)?;
    reserve_host_match(&live, &game_id, &spec).await?;
    if let Some(host) = live.host_for_assignment(&game_id) {
        // A repeat submission gets the original lease even after it ended.
        return Ok(private_json(StatusCode::OK, &host.lease));
    }
    let lease = live.request_host(lease).await?;
    Ok(private_json(StatusCode::CREATED, &lease))
}

#[derive(Serialize)]
struct HostRunStatus {
    state: HostState,
    reason: Option<EndReason>,
    report: Option<Value>,
}

async fn poll_host(
    State(live): State<Arc<Live>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let grant = HostBatchGrant::authenticate(&live, &headers)?;
    let game_id = checked_sim_game_id(&run_id)?;
    let spec = read_host_match(&live, &game_id)
        .await?
        .filter(|spec| spec.batch_id == grant.batch_id)
        .ok_or_else(|| not_found("no such simulation host run"))?;
    let host = live
        .host_for_assignment(&game_id)
        .ok_or_else(|| not_found("no host lease for this simulation run"))?;
    let index = {
        let model = live.model.read();
        model.games.get(&game_id).and_then(|game| {
            let sim = game.started.game.sim.as_ref()?;
            if sim.batch_id.as_ref() == Some(&spec.batch_id) {
                game.sim_report.clone()
            } else {
                None
            }
        })
    };
    let report = read_report(&live, &game_id, index.as_ref()).await?;
    Ok(private_json(
        StatusCode::OK,
        &HostRunStatus {
            state: host.state,
            reason: host.reason,
            report,
        },
    ))
}

async fn host_match(
    State(live): State<Arc<Live>>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let verified = live.verify_host_token(bearer(&headers), crate::host_identity::HOST_REGISTER)?;
    if verified != lease_id {
        return Err(forbidden("host token does not cover this lease"));
    }
    let host = live
        .host_view(&lease_id)
        .ok_or_else(|| not_found("no such host lease"))?;
    let game_id = &host.lease.assignment_id;
    let run_id = game_id
        .strip_prefix("sim-")
        .filter(|run_id| !host.lease.bots.is_empty() && game::valid_id(run_id) && run_id.len() <= 60)
        .ok_or_else(|| not_found("no simulation assigned to this lease"))?;
    let spec = read_host_match(&live, game_id)
        .await?
        .ok_or_else(|| not_found("no match file for this lease"))?;
    Ok(private_json(
        StatusCode::OK,
        &serde_json::json!({ "run_id": run_id, "batch_id": spec.batch_id, "match": spec.matched }),
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
    let (publisher, _, _) =
        live.publisher(bearer(&headers), crate::host_identity::TELEMETRY_INGEST)?;
    if let Some(peer) = publisher.peer() {
        crate::admin::require_admin(&live, peer)?;
    }
    let bytes = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => too_large("simulation report is too large"),
        _ => invalid(e.body_text()),
    })?;
    let (report, index) = parse_report(&bytes)?;
    let _guard = live.sim_writes.lock().await;
    {
        let state = live.model.read();
        let (game, sim) = lookup(&state.games, &id)?;
        if game.started.host != publisher.to_string() {
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
                return Ok(StatusCode::NO_CONTENT.into_response());
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
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn sim_metadata_allows_fifteen_phantoms_and_hosted_runs_are_unpaced() {
        let game = game::NewGame {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            ..Default::default()
        };
        let mut sim = SimGame {
            run_id: "run-42".into(),
            batch_id: Some("batch-7".into()),
            seed: "42".into(),
            gametype: "slayer".into(),
            pace: "unpaced".into(),
            bots: vec![serde_json::json!({"kind": "phantom"}); MAX_SIM_BOTS],
            deferred: false,
        };
        assert!(validate_start(&sim, &game).is_ok());
        assert!(validate_hosted_start(&sim, &game).is_ok());
        sim.bots.push(serde_json::json!({}));
        assert!(validate_start(&sim, &game).is_err());
        assert!(validate_hosted_start(&sim, &game).is_err());
        sim.bots.pop();
        sim.pace = "realtime".into();
        assert!(validate_start(&sim, &game).is_ok());
        assert!(validate_hosted_start(&sim, &game).is_err());
    }

    #[test]
    fn simulation_batch_grant_requires_the_live_signature_and_audience() {
        let at = 1_800_000_000;
        let mut identity = ServerIdentity {
            id: "this-live".into(),
            key: live_proto::sign::KeyPair::generate(),
        };
        let grant = HostBatchGrant {
            author: identity.author(),
            aud: identity.id.clone(),
            provider: ProviderKind::Cloudflare,
            batch_id: "batch-7".into(),
            expires_at: at + 60,
        };
        let raw = identity.key.sign_json(HOST_BATCH_TOKEN, &grant);
        let checked = HostBatchGrant::verify(&raw, &identity, at).unwrap();
        assert_eq!(checked.batch_id, grant.batch_id);
        assert_eq!(checked.provider, ProviderKind::Cloudflare);
        assert!(HostBatchGrant::verify(&raw, &identity, grant.expires_at).is_err());

        let unrelated = live_proto::sign::KeyPair::generate().sign_json(HOST_BATCH_TOKEN, &grant);
        assert!(HostBatchGrant::verify(&unrelated, &identity, at).is_err());
        identity.id = "other-live".into();
        assert!(HostBatchGrant::verify(&raw, &identity, at).is_err());
    }

    fn spec(matched: Value) -> HostMatch {
        HostMatch {
            batch_id: "batch-7".into(),
            matched,
        }
    }

    #[test]
    fn simulation_host_lease_uses_the_bot_only_roster_and_exact_run_id() {
        let spec = spec(serde_json::json!({
            "map": "Lockout",
            "game_type": "slayer",
            "max_wall_s": 300,
            "bot": [
                { "name": "A", "tuning": { "reaction": 4 } },
                {},
                { "name": "C" },
                { "name": "D" },
                { "kind": "phantom", "name": "E" }
            ]
        }));
        let id = checked_sim_game_id("run-42").unwrap();
        let lease = spec.lease(id, ProviderKind::Cloudflare).unwrap();
        assert_eq!(lease.assignment_id, "sim-run-42");
        assert_eq!(lease.map, "lockout");
        assert_eq!(lease.variant.as_deref(), Some("slayer"));
        assert_eq!(lease.bots, ["A", "Clanker 1", "C", "D", "E"]);
        assert!(lease.roster.is_empty());
        assert_eq!(lease.max_players, 5);
        assert_eq!(lease.class, HostClass::Large);
        assert_eq!(lease.transport, Transport::Tunnel);
        assert_eq!(lease.max_match_s, 360);
        assert_eq!(lease.idle_timeout_s, lease.max_match_s);
        assert!(checked_sim_game_id("../run-42").is_err());
        assert!(checked_sim_game_id(&"x".repeat(61)).is_err());
    }

    #[test]
    fn simulation_host_refuses_pacing_and_remote_tuning_files() {
        for matched in [
            serde_json::json!({"map": "lockout", "pace": "realtime", "bot": [{}]}),
            serde_json::json!({"map": "lockout", "bot": [{"params": "local.toml"}]}),
            serde_json::json!({"map": "lockout", "bot": []}),
            serde_json::json!({"map": "lockout", "bot": vec![serde_json::json!({"kind":"phantom"}); 16]}),
        ] {
            assert!(spec(matched).lease("sim-run-42".into(), ProviderKind::Fly).is_err());
        }
        let spec = spec(serde_json::json!({"map": "lockout", "bot": [{"kind": "phantom"}]}));
        let lease = spec.lease("sim-run-42".into(), ProviderKind::Fly).unwrap();
        assert_eq!(lease.transport, Transport::Udp);
        assert_eq!(lease.class, HostClass::Small);
        assert_eq!(lease.max_match_s, DEFAULT_MAX_MATCH_S);
        assert!(spec.lease("sim-run-42".into(), ProviderKind::Local).is_err());
    }

    #[test]
    fn simulation_host_rejects_local_seats() {
        let spec = spec(serde_json::json!({"map": "lockout", "bot": [{"kind": "local"}]}));
        let Err(err) = spec.lease("sim-run-42".into(), ProviderKind::Fly) else {
            panic!("lease admitted a controller seat");
        };
        assert!(err.message.contains("Use phantom bots"));
    }

    #[test]
    fn simulation_run_id_retries_require_the_same_batch_and_match() {
        let matched = serde_json::json!({"map": "lockout", "bot": [{}]});
        let submitted = spec(matched.clone());
        assert!(same_host_match(&submitted, spec(matched.clone())).is_ok());
        assert!(same_host_match(
            &submitted,
            HostMatch {
                batch_id: "different-batch".into(),
                matched,
            },
        ).is_err());
        assert!(same_host_match(
            &submitted,
            spec(serde_json::json!({"map": "turf", "bot": [{}]})),
        ).is_err());
    }
}
