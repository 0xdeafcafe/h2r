//! The HTTP and WebSocket API (docs/live/live-api.md) over [`Live`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
#[cfg(feature = "native")]
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query as Params, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use live_proto::{ClientMessage, ErrorBody, ErrorCode, Gamertag, Health, TitleId, Xuid, lxm};

use crate::app::{ApiError, Live, MAX_TITLE_FILE, not_found};
use crate::store::Query;
use crate::stream::{Out, StreamFilter, StreamSession};

/// Every request body but title files.
pub const MAX_BODY: usize = 64 * 1024;
/// WebSocket messages from clients.
pub const MAX_WS_MESSAGE: usize = 16 * 1024;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;
/// How long a `--private` stream waits for its auth message.
pub(crate) const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// WebSocket close code for a missing, bad or expired token.
pub(crate) const CLOSE_UNAUTHORIZED: u16 = crate::stream::CLOSE_UNAUTHORIZED;

type Shared = State<Arc<Live>>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.code {
            ErrorCode::Invalid => StatusCode::BAD_REQUEST,
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::Gone => StatusCode::GONE,
            ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Banned => StatusCode::FORBIDDEN,
            ErrorCode::Unimplemented => StatusCode::NOT_IMPLEMENTED,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        let team_error = self.message.strip_prefix("team:").and_then(|s| {
            serde_json::from_value::<live_proto::team::TeamError>(serde_json::Value::String(
                s.into(),
            ))
            .ok()
        });
        let mut body = serde_json::to_value(ErrorBody::new(self.code, self.message))
            .expect("error serializes");
        if let Some(error) = team_error {
            body["team_error"] = serde_json::to_value(error).expect("team error serializes");
        }
        let mut resp = (status, axum::Json(body)).into_response();
        if status == StatusCode::UNAUTHORIZED {
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        resp
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub(crate) fn json<T: serde::Serialize>(status: StatusCode, body: &T) -> Response {
    (status, axum::Json(body)).into_response()
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, m)
}

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// A JWS request body (`application/jose`): compact text.
fn jws_body(body: Result<Bytes, BytesRejection>) -> ApiResult<String> {
    let bytes = body_bytes(body)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| invalid("the body must be a compact JWS"))?;
    Ok(text.trim().to_string())
}

fn body_bytes(body: Result<Bytes, BytesRejection>) -> ApiResult<Bytes> {
    body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::new(ErrorCode::TooLarge, "request body too large")
        }
        _ => invalid(e.body_text()),
    })
}

fn gamertag(raw: &str) -> ApiResult<Gamertag> {
    Gamertag::new(raw).map_err(|_| not_found("no such account"))
}

/// `?limit=` and `?cursor=` (or `?from=`, its alias on the log).
struct Paging {
    after: u64,
    limit: usize,
}

/// The most events one `GET /v1/events` page holds.
pub const EVENTS_MAX_LIMIT: usize = 200;

fn paging(p: &HashMap<String, String>) -> ApiResult<Paging> {
    let num = |k: &str| -> ApiResult<Option<u64>> {
        p.get(k)
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| invalid(format!("{k} must be a non-negative integer")))
            })
            .transpose()
    };
    let limit = num("limit")?.map_or(DEFAULT_LIMIT, |l| (l as usize).clamp(1, MAX_LIMIT));
    let after = match p.get("cursor").filter(|v| !v.is_empty()) {
        Some(c) => c.parse().map_err(|_| invalid("bad cursor"))?,
        None => num("from")?.unwrap_or(0),
    };
    Ok(Paging { after, limit })
}

fn list_param(p: &HashMap<String, String>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter_map(|k| p.get(*k))
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// The API, with JSON 404s for every other path.
pub fn router(live: Arc<Live>) -> Router {
    router_with_web(live, None)
}

fn api_router(live: Arc<Live>, facts: crate::status::SiteFacts) -> Router {
    let files = Router::new()
        .route(
            "/v1/titles/{title}/files/{name}",
            get(get_file).put(put_file),
        )
        .route("/v1/maps/{map}/outline", get(outline).put(put_outline))
        .route(
            "/v1/maps",
            get(crate::content::list_maps).put(crate::content::put_map),
        )
        .route("/v1/maps/{map}", get(crate::content::get_map))
        .route("/v1/maps/{map}/image", get(crate::content::get_map_image))
        .route("/v1/maps/{map}/report", post(crate::content::report_map))
        .route(
            "/v1/content/{sha256}/report",
            post(crate::content::report_content),
        )
        .route("/v1/admin/content", get(crate::content::admin_content))
        .route("/v1/admin/reports", get(crate::reports::get_admin_reports))
        .route(
            "/v1/admin/reports/{id}/{action}",
            post(crate::reports::post_review),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/report",
            post(crate::reports::post_report),
        )
        .route("/v1/admin/bans", get(crate::bans::get_admin_bans))
        .route("/v1/admin/bots", post(crate::bots::post_admin_bots))
        .route("/v1/admin/bans/{xuid}", post(crate::bans::post_ban))
        .route("/v1/admin/bans/{xuid}/lift", post(crate::bans::post_lift))
        .route("/v1/accounts/by-xuid/{xuid}/ban", get(crate::bans::get_ban))
        .route(
            "/v1/accounts/by-xuid/{xuid}/mutes",
            get(crate::mutes::get_mutes),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/mutes/{target}",
            axum::routing::put(crate::mutes::put_mute).delete(crate::mutes::delete_mute),
        )
        .route(
            "/v1/accounts/{gamertag}/commendations",
            get(crate::bans::get_commendations),
        )
        .route(
            "/v1/admin/maps/{map}/{action}",
            post(crate::content::moderate_map),
        )
        .route(
            "/v1/admin/content/{sha256}/{action}",
            post(crate::content::moderate_content),
        )
        .route(
            "/v1/admin/uploads/{gamertag}",
            axum::routing::put(crate::content::put_uploads),
        )
        .route("/v1/hoppers/pins", get(crate::content::get_pins))
        .route(
            "/v1/hoppers/{hopper}/pins",
            axum::routing::put(crate::content::put_pins),
        )
        .route(
            "/v1/titles/{title}/motd",
            get(crate::admin::get_motd).put(crate::admin::put_motd),
        )
        .route(
            "/v1/titles/{title}/hopper",
            get(crate::admin::get_hopper).put(crate::admin::put_hopper),
        )
        .route("/v1/sessions/{id}/relay/ticket", post(relay_ticket))
        .route("/v1/admins", get(crate::admin::list_admins))
        .route(
            "/v1/admins/{gamertag}",
            post(crate::admin::grant_admin).delete(crate::admin::revoke_admin),
        )
        .layer(DefaultBodyLimit::max(MAX_TITLE_FILE));
    // Uploads stream into the blob store under their own caps
    // (ContentLimits, StorageLimits).
    let uploads = Router::new()
        .route(
            "/v1/content/{sha256}",
            get(crate::content::get_content).put(crate::content::put_content),
        )
        // Cloud saves (docs/live/live-cloud-saves.md). The segment is
        // `{gamertag}` as on the other /v1/accounts routes (the router
        // wants one name per position); these take a xuid there.
        .route(
            "/v1/accounts/{gamertag}/storage",
            get(crate::cloud_saves::list),
        )
        .route(
            "/v1/accounts/{gamertag}/storage/{kind}/{*name}",
            get(crate::cloud_saves::get_item)
                .put(crate::cloud_saves::put_item)
                .delete(crate::cloud_saves::delete_item),
        )
        .layer(DefaultBodyLimit::disable());
    Router::new()
        .route("/healthz", get(healthz))
        .route("/.well-known/h2live.json", get(well_known))
        .route("/v1/accounts", get(list_accounts).post(create_account))
        .route(
            "/v1/accounts/{gamertag}/recover",
            post(crate::recovery::post_recover),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/recovery",
            get(crate::recovery::get_recovery).post(crate::recovery::post_recovery),
        )
        .route("/v1/accounts/by-xuid/{xuid}/keys", get(keys_by_xuid))
        .route(
            "/v1/accounts/by-xuid/{xuid}/signins",
            get(crate::signins::get_signins),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/signins/sign-out-others",
            post(crate::signins::post_sign_out_others),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/signins/{kid}",
            delete(crate::signins::delete_signin),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}",
            get(crate::teams::account_identity),
        )
        .route(
            "/v1/accounts/by-xuid/{xuid}/teams",
            get(crate::teams::account_teams_by_id),
        )
        .route(
            "/v1/accounts/{gamertag}/teams",
            get(crate::teams::account_teams),
        )
        .route("/v1/teams", get(crate::teams::list))
        .route("/v1/teams/{id}", get(crate::teams::detail))
        .route("/v1/accounts/{gamertag}", get(account))
        .route("/v1/accounts/{gamertag}/keys", get(keys_by_tag))
        .route("/v1/accounts/{gamertag}/friends", get(account_friends))
        .route(
            "/v1/accounts/by-xuid/{xuid}/mailbox",
            get(crate::mailbox::read),
        )
        .route("/v1/presence", get(presence))
        .route("/v1/presence/ping", post(ping))
        .route(
            "/v1/invites",
            post(create_invite).get(crate::invites::get_invites),
        )
        .route(
            "/v1/invites/{jti}/revoke",
            post(crate::invites::revoke_invite),
        )
        .route("/v1/server", get(crate::status::get_server))
        .route("/v1/stats/summary", get(crate::stats::get_summary))
        .route("/v1/page-meta", get(page_meta))
        .route("/v1/ranks/{playlist}", get(crate::ranks::leaderboard))
        .route("/v1/rank-stats", get(crate::ranks::stats))
        .route(
            "/v1/arbitration/{session}/{round}/{action}",
            axum::routing::post(crate::arbitration::post_round),
        )
        .route(
            "/v1/accounts/{gamertag}/ranks",
            get(crate::ranks::player_ranks),
        )
        .route(
            "/v1/accounts/{gamertag}/ranks/{playlist}",
            get(crate::ranks::player_history),
        )
        .route("/v1/admin/status", get(crate::status::get_admin_status))
        .route("/v1/admin/usage", get(crate::usage::get_overview))
        .route("/v1/admin/usage/accounts", get(crate::usage::get_accounts))
        .route("/v1/events", get(list_events).post(post_event))
        .route(
            "/v1/sessions",
            get(crate::sessions::list).post(crate::sessions::create),
        )
        .route("/v1/coop/games", get(crate::sessions::coop_games))
        .route("/v1/sessions/{id}", get(crate::sessions::detail))
        .route("/v1/sessions/{id}/events", post(crate::sessions::append))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/games", get(list_games).post(start_game))
        .route("/v1/games/bots", post(crate::bots::post_roster))
        .route("/v1/games/{id}", get(game))
        .route("/v1/games/{id}/frames", get(game_frames))
        .route("/v1/games/{id}/events", post(game_events))
        .route("/v1/local/signin", any(crate::local_signin::redeem))
        .route(
            "/v1/{*rest}",
            any(|| async { not_found("no such endpoint") }),
        )
        .merge(files)
        .merge(uploads)
        .merge(crate::logio::routes())
        .merge(sockets())
        .method_not_allowed_fallback(|| async {
            ApiError::new(ErrorCode::Invalid, "method not allowed here")
                .into_response()
                .with_status(StatusCode::METHOD_NOT_ALLOWED)
        })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(axum::Extension(facts))
        .layer(middleware::from_fn(cors))
        .with_state(live)
}

/// The WebSocket routes: the event stream, the co-op relay and the game's
/// ingest. On a Worker these are Durable Objects instead
/// (docs/live/live-worker.md), so without `native` there are none here.
#[cfg(feature = "native")]
fn sockets() -> Router<Arc<Live>> {
    Router::new()
        .route("/v1/stream", get(stream))
        .route("/v1/sessions/{id}/relay", get(crate::relay::relay))
        .route("/v1/ingest", get(crate::ingest::ingest))
}

#[cfg(not(feature = "native"))]
fn sockets() -> Router<Arc<Live>> {
    Router::new()
}

/// What is served beside the API.
#[derive(Debug, Clone, Default)]
pub struct Site {
    /// The built web app, at `/`.
    pub web: Option<std::path::PathBuf>,
    /// Art from the owner's disc, at `/art/game/` ([`crate::art`]).
    pub game_art: Option<crate::art::GameArt>,
}

/// [`router`] plus the built web app at `/`: files from `dir`, and
/// `index.html` for any other path (the app routes in the browser). API
/// paths keep their JSON 404s.
pub fn router_with_web(live: Arc<Live>, dir: Option<&std::path::Path>) -> Router {
    router_with_site(
        live,
        &Site {
            web: dir.map(std::path::Path::to_path_buf),
            game_art: None,
        },
    )
}

/// The API plus everything in `site`. `/art/game/*` answers a JSON 404
/// when there is no game art.
pub fn router_with_site(live: Arc<Live>, site: &Site) -> Router {
    let published = live.published_art().cloned();
    let pages = live.clone();
    let mut api = api_router(
        live,
        crate::status::SiteFacts {
            game_art: site.game_art.clone(),
        },
    );
    api = match &site.game_art {
        #[cfg(feature = "native")]
        Some(art) => api.merge(crate::art::routes(art, published)),
        _ => api.route(
            &format!("{}/{{*rest}}", crate::art::PREFIX),
            get(|| async { not_found("no game art on this server") }),
        ),
    };
    #[cfg(not(feature = "native"))]
    let _ = published;
    with_web(api, pages, site.web.as_deref()).layer(middleware::from_fn(crate::caching::layer))
}

/// The built web app's files beside the API, `index.html` for any other
/// path; or, with none, a JSON 404.
#[cfg(feature = "native")]
fn with_web(api: Router, live: Arc<Live>, dir: Option<&std::path::Path>) -> Router {
    let Some(dir) = dir else {
        return api.fallback(|| async { not_found("no such endpoint") });
    };
    let index = dir.join("index.html");
    let page = get(move |State(live): Shared, req: Request| {
        let index = index.clone();
        async move { app_page(&live, &index, req).await }
    })
    .with_state(live);
    // `/` too: the app's HTML is always the page's, never the bare file.
    let files = tower_http::services::ServeDir::new(dir)
        .append_index_html_on_directories(false)
        .fallback(page);
    api.fallback_service(files)
}

/// The app's HTML for a page: `index.html` with the page's link preview
/// ([`crate::page_meta`]), read afresh so a new build shows at once.
#[cfg(feature = "native")]
async fn app_page(live: &Live, index: &std::path::Path, req: Request) -> Response {
    // The stats site's first address: the same page where it is now.
    if let Some(to) = crate::page_meta::moved(req.uri().path(), req.uri().query())
        && let Ok(to) = HeaderValue::from_str(&to)
    {
        return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, to)]).into_response();
    }
    let Ok(html) = tokio::fs::read_to_string(index).await else {
        return not_found("no web app").into_response();
    };
    let path = req.uri().path();
    let sent = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok());
    let origin = crate::page_meta::origin(
        live.policy().public.get().as_deref(),
        sent("host"),
        sent("x-forwarded-proto"),
    );
    let meta = crate::page_meta::describe(live, path);
    let body = crate::page_meta::page_html(&html, &meta, &origin, path);
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

/// `GET /v1/page-meta?path=`: what a page of the web app is, for its link
/// preview ([`crate::page_meta`]). The Worker asks this for pages that
/// name a player, game or clan; the machine's server reads it directly.
async fn page_meta(
    State(live): Shared,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    let path = p.get("path").map_or("/", String::as_str);
    Ok(json(
        StatusCode::OK,
        &crate::page_meta::describe(&live, path),
    ))
}

/// A Worker serves the web app as static assets, ahead of this router.
#[cfg(not(feature = "native"))]
fn with_web(api: Router, _live: Arc<Live>, _dir: Option<&std::path::Path>) -> Router {
    api.fallback(|| async { not_found("no such endpoint") })
}

trait WithStatus {
    fn with_status(self, status: StatusCode) -> Self;
}

impl WithStatus for Response {
    fn with_status(mut self, status: StatusCode) -> Self {
        *self.status_mut() = status;
        self
    }
}

/// CORS (security §6): reads are open to any origin, writes to none. A
/// preflight for anything but GET/HEAD gets no CORS headers, so browsers
/// refuse to send it.
async fn cors(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    if method == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        let wanted = req.headers()[header::ACCESS_CONTROL_REQUEST_METHOD].as_bytes();
        let mut resp = StatusCode::NO_CONTENT.into_response();
        if wanted == b"GET" || wanted == b"HEAD" {
            let h = resp.headers_mut();
            h.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, HEAD"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("authorization, if-none-match"),
            );
            h.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("600"),
            );
        }
        return resp;
    }
    let mut resp = next.run(req).await;
    if method == Method::GET || method == Method::HEAD {
        let h = resp.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        h.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("etag, x-next, x-position"),
        );
    }
    resp
}

async fn healthz(State(live): Shared) -> ApiResult<axum::Json<Health>> {
    let position = live.store().head().await?;
    Ok(axum::Json(Health { ok: true, position }))
}

async fn well_known(State(live): Shared) -> Response {
    json(StatusCode::OK, &live.discovery())
}

async fn list_accounts(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let paging = paging(&p)?;
    Ok(json(
        StatusCode::OK,
        &live.accounts(paging.after, paging.limit),
    ))
}

async fn create_account(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let device = crate::signins::device_of(&headers);
    let created = live.create_account_on(&jws_body(body)?, device).await?;
    Ok(json(StatusCode::CREATED, &created))
}

async fn account(
    State(live): Shared,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    Ok(json(
        StatusCode::OK,
        &live.account(&gamertag(&tag)?, viewer)?,
    ))
}

async fn keys_by_tag(
    State(live): Shared,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.keys_by_tag(&gamertag(&tag)?)?))
}

/// `GET /v1/accounts/{gamertag}/friends` (docs/live/live-api.md, Friends).
async fn account_friends(
    State(live): Shared,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    Ok(json(
        StatusCode::OK,
        &live.friends(&gamertag(&tag)?, viewer)?,
    ))
}

async fn keys_by_xuid(
    State(live): Shared,
    headers: HeaderMap,
    Path(xuid): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let xuid = Xuid::parse(&xuid).map_err(|_| not_found("no such account"))?;
    Ok(json(StatusCode::OK, &live.keys_by_xuid(xuid)?))
}

async fn presence(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.presence(viewer)))
}

fn event_query(p: &HashMap<String, String>) -> ApiResult<Query> {
    let paging = paging(p)?;
    Ok(Query {
        after: paging.after,
        // A page of the log is at most EVENTS_MAX_LIMIT events: GameFrames
        // make 1000 of them megabytes.
        limit: paging.limit.min(EVENTS_MAX_LIMIT),
        stream: p.get("stream").filter(|s| !s.is_empty()).cloned(),
        types: list_param(p, &["type", "types"]),
        exclude: list_param(p, &["exclude"]),
    })
}

async fn list_events(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    Ok(json(
        StatusCode::OK,
        &live.events_for(&event_query(&p)?, viewer).await?,
    ))
}

async fn post_event(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let event = live
        .append_signed(bearer(&headers), &jws_body(body)?)
        .await?;
    Ok(json(StatusCode::CREATED, &event))
}

fn title_path(title: &str, name: &str) -> ApiResult<(TitleId, String)> {
    let title = TitleId::parse(title).map_err(|_| invalid("title ids are 8 hex digits"))?;
    if !live_proto::valid_file_name(name) {
        return Err(invalid("file names are [A-Za-z0-9_.-]{1,64}"));
    }
    Ok((title, name.to_string()))
}

async fn get_file(
    State(live): Shared,
    headers: HeaderMap,
    Path((title, name)): Path<(String, String)>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let (title, name) = title_path(&title, &name)?;
    let (meta, bytes) = live.file(title, &name).await?;
    let etag = format!("\"{}\"", meta.sha256);
    let etag_value = HeaderValue::from_str(&etag).expect("hex is a valid header");
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim() == etag || t.trim() == meta.sha256 || t.trim() == "*")
        });
    if matches {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag_value)]).into_response());
    }
    Ok((
        [
            (header::ETAG, etag_value),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `POST /v1/sessions/{id}/relay/ticket`: with a `sessions.relay` token, a
/// signed ticket for the session's relay; with `{"ticket"}` and no token,
/// a renewal of a ticket that has (nearly) run out (docs/live/live-worker.md,
/// The co-op relay).
async fn relay_ticket(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let reply = match bearer(&headers) {
        Some(token) => live.relay_ticket(Some(token), &id)?,
        None => {
            let bytes = body_bytes(body)?;
            #[derive(serde::Deserialize)]
            struct Renew {
                ticket: String,
            }
            let r: Renew = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("send a sessions.relay token, or {\"ticket\": ...}"))?;
            live.renew_relay_ticket(&r.ticket, &id)?
        }
    };
    Ok(json(StatusCode::OK, &reply))
}

async fn put_file(
    State(live): Shared,
    headers: HeaderMap,
    Path((title, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let (title, name) = title_path(&title, &name)?;
    let bytes = body_bytes(body)?;
    let published = live
        .publish_file(bearer(&headers), title, &name, &bytes)
        .await?;
    live.meter(
        crate::usage::bearer_xuid(bearer(&headers)),
        crate::usage::metric::UP_CONTENT,
        bytes.len() as u64,
    )
    .await;
    Ok(json(StatusCode::OK, &published))
}

#[cfg(feature = "native")]
async fn stream(
    State(live): Shared,
    Params(p): Params<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> ApiResult<Response> {
    let filter = StreamFilter::from_params(&p)?;
    let ws = ws.map_err(|_| invalid("/v1/stream is a WebSocket"))?;
    Ok(ws
        .max_message_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| async move {
            if let Err(reason) = run_stream(live, socket, filter).await {
                eprintln!("live: stream closed: {reason}");
            }
        }))
}

#[cfg(feature = "native")]
pub(crate) async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let frame = CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

/// The token in an auth message, checked; its `exp`.
pub(crate) fn auth_message(live: &Live, text: &str, method: &str) -> Result<i64, ApiError> {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Auth { token }) => live.authenticate_until(&token, method),
        _ => Err(ApiError::new(
            ErrorCode::Unauthorized,
            "expected {\"kind\":\"auth\",\"token\":...}",
        )),
    }
}

/// Sends a session's messages. `Ok(false)`: it closed the socket.
#[cfg(feature = "native")]
pub(crate) async fn send_all(socket: &mut WebSocket, out: Vec<Out>) -> Result<bool, String> {
    for o in out {
        if !send_out(socket, o).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(feature = "native")]
async fn send_out(socket: &mut WebSocket, o: Out) -> Result<bool, String> {
    let msg = match o {
        Out::Text(t) => Message::Text(t),
        Out::Binary(b) => Message::Binary(b),
        Out::Close(code, reason) => {
            close(socket, code, &reason).await;
            return Ok(false);
        }
    };
    socket.send(msg).await.map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(feature = "native")]
/// Drives a [`StreamSession`] over axum's WebSocket: waits for the auth
/// message when one is due, subscribes, lets the reader in, then follows
/// the dispatcher's feed and the frame feeds. On a `--private` server the
/// socket closes when the token expires unless the client sends a fresh
/// one.
async fn run_stream(
    live: Arc<Live>,
    mut socket: WebSocket,
    filter: StreamFilter,
) -> Result<(), String> {
    let mut session = StreamSession::new(live.clone(), filter);
    if session.awaits_auth() {
        let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
        let result = match first {
            Ok(Some(Ok(Message::Text(text)))) => session.on_text(&text),
            _ => Err(StreamSession::auth_timed_out()),
        };
        if let Err(close) = result {
            send_out(&mut socket, close).await?;
            return Ok(());
        }
    }
    // Subscribe before replaying, so nothing falls between the two. A
    // packed reader of one game hears only that game's batches; of more,
    // every tick. JSON readers take every frame as it comes.
    let filter = session.filter().clone();
    let mut feed = live.feed();
    let mut packed = filter.packed.then(|| match filter.games.as_slice() {
        [one] => PackedFeed::Game(live.packed().subscribe_game(one)),
        _ => PackedFeed::All(live.packed().subscribe_all()),
    });
    let mut frames = (!filter.packed).then(|| live.frames());
    if !send_all(&mut socket, session.open().await?).await? {
        return Ok(());
    }
    // The token's end: one timer, moved when a fresh token comes (not
    // made anew for every message).
    let at = |deadline: i64| {
        let wait = (deadline - crate::app::now()).max(0) as u64;
        tokio::time::Instant::now() + Duration::from_secs(wait)
    };
    let mut deadline_for = session.deadline();
    let deadline =
        tokio::time::sleep_until(deadline_for.map_or_else(tokio::time::Instant::now, at));
    tokio::pin!(deadline);
    loop {
        if deadline_for != session.deadline() {
            deadline_for = session.deadline();
            if let Some(d) = deadline_for {
                deadline.as_mut().reset(at(d));
            }
        }
        let out: Vec<Out> = tokio::select! {
            event = feed.recv() => match event {
                Ok(e) => session.on_event(&e).into_iter().collect(),
                // Fell behind the feed: re-read from the store.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => session.catch_up().await?,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            frame = recv_frame(&mut frames) => match frame {
                Ok(f) => session.on_frame(&f).into_iter().collect(),
                // Frames are best effort: a slow reader skips some.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Vec::new(),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            batch = recv_packed(&mut packed) => match batch {
                Packed::Tick(tick) => session.on_tick(&tick).into_iter().collect(),
                Packed::Game(b) => session.on_game_batch(&b).into_iter().collect(),
                Packed::Lagged => session.rejoin().into_iter().collect(),
                Packed::Closed => return Ok(()),
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => match session.on_text(&text) {
                    Ok(_) => Vec::new(),
                    Err(close) => vec![close],
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                Some(Ok(_)) => Vec::new(),
            },
            _ = &mut deadline, if deadline_for.is_some() => {
                session.expired(crate::app::now()).into_iter().collect()
            }
        };
        if !send_all(&mut socket, out).await? {
            return Ok(());
        }
    }
}

#[cfg(feature = "native")]
/// A JSON reader's next frame (never, for a packed reader).
async fn recv_frame(
    frames: &mut Option<tokio::sync::broadcast::Receiver<Arc<crate::games::LiveFrame>>>,
) -> Result<Arc<crate::games::LiveFrame>, tokio::sync::broadcast::error::RecvError> {
    match frames {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(feature = "native")]
/// What a packed reader hears: every game's ticks, or one game's batches.
enum PackedFeed {
    All(tokio::sync::broadcast::Receiver<Arc<crate::packed::Tick>>),
    Game(tokio::sync::broadcast::Receiver<Arc<crate::packed::GameBatch>>),
}

#[cfg(feature = "native")]
enum Packed {
    Tick(Arc<crate::packed::Tick>),
    Game(Arc<crate::packed::GameBatch>),
    Lagged,
    Closed,
}

#[cfg(feature = "native")]
/// A packed reader's next tick or batch (never, for a JSON reader).
async fn recv_packed(feed: &mut Option<PackedFeed>) -> Packed {
    use tokio::sync::broadcast::error::RecvError;
    let lost = |e: RecvError| match e {
        RecvError::Lagged(_) => Packed::Lagged,
        RecvError::Closed => Packed::Closed,
    };
    match feed {
        Some(PackedFeed::All(rx)) => rx.recv().await.map_or_else(lost, Packed::Tick),
        Some(PackedFeed::Game(rx)) => rx.recv().await.map_or_else(lost, Packed::Game),
        None => std::future::pending().await,
    }
}

async fn ping(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    let ttl = live.ping(bearer(&headers))?;
    Ok(json(
        StatusCode::OK,
        &serde_json::json!({"ok": true, "expires_in": ttl}),
    ))
}

/// `POST /v1/invites`: with a signed `InviteCreated` event as the body, an
/// admin account's invite ([`crate::invites::create`]); with no body, the
/// operator's (`?days=`).
async fn create_invite(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<HashMap<String, String>>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let body = body_bytes(body)?;
    if !body.iter().all(u8::is_ascii_whitespace) {
        let text = std::str::from_utf8(&body)
            .map_err(|_| invalid("the body must be a compact JWS"))?
            .trim();
        // A bare JWS, or `{"event": <jws>, "code": <custom code>}`.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WithCode {
            event: String,
            code: Option<String>,
        }
        let (jws, code) = if text.starts_with('{') {
            let w: WithCode =
                serde_json::from_str(text).map_err(|e| invalid(format!("body: {e}")))?;
            (w.event, w.code)
        } else {
            (text.to_string(), None)
        };
        let v = crate::invites::create(&live, bearer(&headers), &jws, code.as_deref()).await?;
        return Ok(json(StatusCode::CREATED, &v));
    }
    let days = p
        .get("days")
        .map(|d| d.parse().map_err(|_| invalid("days must be a number")))
        .transpose()?
        .unwrap_or(7);
    live.authorize_operator(bearer(&headers), lxm::INVITES_WRITE)?;
    let v = crate::invites::create_operator(&live, days, p.get("code").map(String::as_str)).await?;
    Ok(json(StatusCode::CREATED, &v))
}

async fn snapshot(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.snapshot(viewer).await?))
}

async fn list_games(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    let paging = paging(&p)?;
    let state = match p.get("state").map(String::as_str) {
        None | Some("") => None,
        Some(s @ ("live" | "ended")) => Some(s),
        Some(_) => return Err(invalid("state is live or ended")),
    };
    let before = (paging.after > 0).then_some(paging.after);
    let filter = crate::games::GameFilter {
        map: p.get("map").filter(|m| !m.is_empty()).cloned(),
        map_id: match p.get("map_id").map(String::as_str) {
            None | Some("") => None,
            Some("none") => Some(None),
            Some(id) => Some(Some(
                id.parse()
                    .map_err(|_| invalid("map_id is 64 hex digits or none"))?,
            )),
        },
        skulls: match p.get("skulls") {
            None => Vec::new(),
            Some(v) => crate::games::parse_skulls(v).map_err(invalid)?,
        },
    };
    Ok(json(
        StatusCode::OK,
        &live.games_where(state, &filter, viewer, before, paging.limit),
    ))
}

async fn start_game(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_bytes(body)?;
    let new = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("game: {e}")))?;
    let created = live.start_game(bearer(&headers), new).await?;
    Ok(json(StatusCode::CREATED, &created))
}

fn game_id(id: &str) -> ApiResult<&str> {
    if live_proto::game::valid_id(id) {
        Ok(id)
    } else {
        Err(not_found("no such game"))
    }
}

async fn game(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    live.check_game_shows(game_id(&id)?, viewer)?;
    Ok(json(StatusCode::OK, &live.game(game_id(&id)?)?))
}

async fn game_frames(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    let viewer = live.read_viewer(bearer(&headers))?;
    live.check_game_shows(game_id(&id)?, viewer)?;
    let tick = |k: &str| {
        p.get(k)
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| invalid(format!("{k} must be a tick")))
            })
            .transpose()
    };
    let (from_tick, to_tick) = (tick("from_tick")?, tick("to_tick")?);
    let paging = paging(&p)?;
    let limit = if p.contains_key("limit") {
        paging.limit
    } else {
        crate::games::FRAMES_PAGE
    };
    let packed = match p.get("format").map(String::as_str) {
        None | Some("") | Some("json") => false,
        Some("packed") => true,
        Some(_) => return Err(invalid("format must be json or packed")),
    };
    if !packed {
        return Ok(json(
            StatusCode::OK,
            &live
                .recorded_frames(game_id(&id)?, from_tick, to_tick, paging.after, limit)
                .await?,
        ));
    }
    let page = live
        .recorded_frame_data(game_id(&id)?, from_tick, to_tick, paging.after, limit)
        .await?;
    // A container of keyframes; the page's cursor and head in headers.
    let mut body = Vec::new();
    for item in &page.items {
        if let Some(bytes) = crate::games::recorded_packed(item) {
            live_proto::pack::push_record(&mut body, &bytes);
        }
    }
    let mut resp = (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-h2-frames"),
        )],
        body,
    )
        .into_response();
    let h = resp.headers_mut();
    if let Some(next) = page
        .next
        .as_deref()
        .and_then(|n| HeaderValue::from_str(n).ok())
    {
        h.insert("x-next", next);
    }
    h.insert("x-position", HeaderValue::from(page.position));
    Ok(resp)
}

async fn game_events(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let jws = jws_body(body)?;
    let position = live
        .append_game_events(bearer(&headers), game_id(&id)?, &jws)
        .await?;
    Ok(json(
        StatusCode::CREATED,
        &serde_json::json!({"position": position}),
    ))
}

async fn outline(
    State(live): Shared,
    headers: HeaderMap,
    Path(map): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let bytes = live.outline(&map).await?;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        bytes,
    )
        .into_response())
}

async fn put_outline(
    State(live): Shared,
    headers: HeaderMap,
    Path(map): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body_bytes(body)?;
    let stored = live.put_outline(bearer(&headers), &map, &bytes).await?;
    live.meter(
        crate::usage::bearer_xuid(bearer(&headers)),
        crate::usage::metric::UP_CONTENT,
        bytes.len() as u64,
    )
    .await;
    Ok(json(StatusCode::OK, &stored))
}
