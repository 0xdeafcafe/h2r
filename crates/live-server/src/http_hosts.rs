//! Dedicated host routes (docs/live/dedicated-host-build.md, contract 5).
//!
//! `Live` methods from dh-provider (`&self`, `bearer: Option<&str>`, `lease_id: &str`):
//! - `async host_register(bearer, lease_id, endpoint: HostEndpoint) -> Result<HostLease, ApiError>`
//! - `async host_report(bearer, lease_id, state: HostState, players: u8, reason: Option<EndReason>) -> Result<HostView, ApiError>`
//! - `host_get(bearer, lease_id) -> Result<HostView, ApiError>` (`{lease, state, endpoint}`)
//! - `admin_hosts(bearer) -> Result<serde_json::Value, ApiError>` (running and recent with cost)
//! - `async admin_stop_host(bearer, lease_id) -> Result<HostView, ApiError>`
//!
//! The `Live` methods authenticate the bearer once and own authorization and
//! write limits. They check the lease-bound `hosts.register` and `hosts.report`
//! scopes, `read` for a roster member or admin, and `admin.read` / `admin.write`
//! with the existing admin check for the inventory and stop routes. The async
//! methods supply `Send` futures to the shared Axum router in both homes.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use live_proto::hosts::{EndReason, HostEndpoint, HostLease, HostState};
use serde::Deserialize;

use crate::app::{ApiError, Live, invalid};
use crate::http::{bearer, body_bytes, json};

type Shared = State<Arc<Live>>;
type ApiResult<T> = Result<T, ApiError>;

const MAX_REGISTER_BODY: usize = 8 * 1024;
const MAX_REPORT_BODY: usize = 1024;
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
        .route("/v1/admin/hosts", get(inventory))
        .route("/v1/admin/hosts/{lease}/stop", post(stop));
    #[cfg(feature = "native")]
    let router = router.route("/v1/hosts/{lease}/tunnel", get(host_tunnel::upgrade));
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
        .host_report(
            bearer(&headers),
            &lease,
            report.state,
            report.players,
            report.reason,
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

async fn stop(
    State(live): Shared,
    headers: HeaderMap,
    Path(lease): Path<String>,
) -> ApiResult<StatusCode> {
    let _ = live.admin_stop_host(bearer(&headers), &lease).await?;
    Ok(StatusCode::NO_CONTENT)
}
