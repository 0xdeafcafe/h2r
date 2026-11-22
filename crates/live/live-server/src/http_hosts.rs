//! Dedicated host routes (docs/live/dedicated-host-build.md, contract 5).
//!
//! `Live` methods from dh-provider (`&self`, `bearer: Option<&str>`, `lease_id: &str`):
//! - `async host_register(bearer, lease_id, endpoint: HostEndpoint) -> Result<HostLease, ApiError>`
//! - `async host_report(bearer, lease_id, state: HostState, players: u8, reason: Option<EndReason>) -> Result<HostView, ApiError>`
//! - `host_get(bearer, lease_id) -> Result<HostView, ApiError>` (`{lease, state, endpoint}`)
//! - `admin_hosts(bearer) -> Result<serde_json::Value, ApiError>` (running and recent with cost)
//! - `async admin_stop_host(bearer, lease_id) -> Result<HostView, ApiError>`
//!
//! `POST /v1/admin/hosts` (`async admin_start_host(bearer, AdminHostStart)`)
//! is the admin's manual start: it launches on the configured provider or,
//! with `launch: false`, records a `Requested` lease and answers its boot
//! token — the only route that ever returns one.
//!
//! The `Live` methods authenticate the bearer once and own authorization and
//! write limits. They check the lease-bound `hosts.register` and `hosts.report`
//! scopes, `read` for a roster member or admin, and `admin.read` / `admin.write`
//! with the existing admin check for the inventory, start and stop routes. The
//! async methods supply `Send` futures to the shared Axum router in both homes.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use live_proto::hosts::{EndReason, HostEndpoint, HostLease, HostState};
use serde::Deserialize;
use serde_json::json;

use crate::app::{ApiError, Live, invalid};
use crate::http::{bearer, body_bytes, json};

type Shared = State<Arc<Live>>;
type ApiResult<T> = Result<T, ApiError>;

const MAX_REGISTER_BODY: usize = 8 * 1024;
const MAX_REPORT_BODY: usize = 1024;
const MAX_START_BODY: usize = 1024;
#[cfg(feature = "native")]
#[path = "host_tunnel.rs"]
mod host_tunnel;

pub(crate) fn routes() -> Router<Arc<Live>> {
    let router = Router::new()
        .route(
            "/v1/hosts/{lease}/register",
            post(register).layer(DefaultBodyLimit::max(MAX_REGISTER_BODY)),
        )
        .route(
            "/v1/hosts/{lease}/report",
            post(report).layer(DefaultBodyLimit::max(MAX_REPORT_BODY)),
        )
        .route("/v1/hosts/{lease}", get(detail))
        .route(
            "/v1/admin/hosts",
            get(inventory).merge(post(create).layer(DefaultBodyLimit::max(MAX_START_BODY))),
        )
        .route("/v1/admin/hosts/{lease}/stop", post(stop));
    #[cfg(feature = "native")]
    let router = router
        .route("/v1/hosts/{lease}/tunnel", get(host_tunnel::upgrade))
        .route("/v1/hosts/regions/{region}/echo", get(host_tunnel::region_echo));
    router
}

async fn register(
    State(live): Shared,
    headers: HeaderMap,
    Path(lease): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let endpoint: HostEndpoint = serde_json::from_slice(&body_bytes(body)?)
        .map_err(|e| invalid(format!("host endpoint: {e}")))?;
    let registered: HostLease = live.host_register(bearer(&headers), &lease, endpoint).await?;
    Ok(json(StatusCode::OK, &registered))
}

#[derive(Deserialize)]
struct ReportBody {
    state: HostState,
    players: u8,
    reason: Option<EndReason>,
    pack_usage: Option<live_proto::hosts::PackUsage>,
}

async fn report(
    State(live): Shared,
    headers: HeaderMap,
    Path(lease): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<StatusCode> {
    let report: ReportBody = serde_json::from_slice(&body_bytes(body)?)
        .map_err(|e| invalid(format!("host report: {e}")))?;
    if !matches!(report.state, HostState::Playing | HostState::Ended) {
        return Err(invalid("host report state must be playing or ended"));
    }
    let _ = live
        .host_report_with_usage(
            bearer(&headers),
            &lease,
            report.state,
            report.players,
            report.reason,
            report.pack_usage,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn detail(
    State(live): Shared,
    headers: HeaderMap,
    Path(lease): Path<String>,
) -> ApiResult<Response> {
    let view = live.host_get(bearer(&headers), &lease)?;
    Ok(json(StatusCode::OK, &view))
}

async fn inventory(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    let hosts = live.admin_hosts(bearer(&headers))?;
    Ok(json(StatusCode::OK, &hosts))
}

/// `POST /v1/admin/hosts`: `201` with `{lease}` when the provider launches
/// it, or `{lease, token}` when the admin runs the container by hand.
async fn create(State(live): Shared, request: Request) -> ApiResult<Response> {
    let who = live.authenticate_host_admin(bearer(request.headers()))?;
    let body = body_bytes(Bytes::from_request(request, &live).await)?;
    let start = serde_json::from_slice(&body)
        .map_err(|e| invalid(format!("host start: {e}")))?;
    let (lease, token) = live.start_authorized_host(&who, start).await?;
    let reply = match token {
        Some(token) => json!({ "lease": lease, "token": token }),
        None => json!({ "lease": lease }),
    };
    Ok(json(StatusCode::CREATED, &reply))
}

async fn stop(
    State(live): Shared,
    headers: HeaderMap,
    Path(lease): Path<String>,
) -> ApiResult<StatusCode> {
    let _ = live.admin_stop_host(bearer(&headers), &lease).await?;
    Ok(StatusCode::NO_CONTENT)
}
