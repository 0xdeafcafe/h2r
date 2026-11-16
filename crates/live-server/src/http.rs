//! The HTTP and WebSocket API (docs/live/live-api.md) over [`Live`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query as Params, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use live_proto::sign::TOKEN_LEEWAY_S;
use live_proto::{
    ClientMessage, ErrorBody, ErrorCode, Gamertag, Health, StreamMessage, TitleId, Xuid, lxm,
};

use crate::app::{ApiError, Live, MAX_TITLE_FILE, not_found};
use crate::store::Query;

/// Every request body but title files.
pub const MAX_BODY: usize = 64 * 1024;
/// WebSocket messages from clients.
pub const MAX_WS_MESSAGE: usize = 16 * 1024;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;
/// How long a `--private` stream waits for its auth message.
pub(crate) const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// WebSocket close code for a missing, bad or expired token.
pub(crate) const CLOSE_UNAUTHORIZED: u16 = 4401;

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
        .route("/v1/admins", get(crate::admin::list_admins))
        .route(
            "/v1/admins/{gamertag}",
            post(crate::admin::grant_admin).delete(crate::admin::revoke_admin),
        )
        .layer(DefaultBodyLimit::max(MAX_TITLE_FILE));
    // Uploads stream to disk under their own caps (ContentLimits).
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
        .route("/v1/admin/status", get(crate::status::get_admin_status))
        .route("/v1/events", get(list_events).post(post_event))
        .route("/v1/stream", get(stream))
        .route(
            "/v1/sessions",
            get(crate::sessions::list).post(crate::sessions::create),
        )
        .route("/v1/sessions/{id}", get(crate::sessions::detail))
        .route("/v1/sessions/{id}/events", post(crate::sessions::append))
        .route("/v1/sessions/{id}/relay", get(crate::relay::relay))
        .route("/v1/games", get(list_games).post(start_game))
        .route("/v1/games/{id}", get(game))
        .route("/v1/games/{id}/frames", get(game_frames))
        .route("/v1/games/{id}/events", post(game_events))
        .route("/v1/ingest", get(crate::ingest::ingest))
        .route("/v1/local/signin", any(crate::local_signin::redeem))
        .route(
            "/v1/{*rest}",
            any(|| async { not_found("no such endpoint") }),
        )
        .merge(files)
        .merge(uploads)
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
    let mut api = api_router(
        live,
        crate::status::SiteFacts {
            game_art: site.game_art.clone(),
        },
    );
    api = match &site.game_art {
        Some(art) => api.merge(crate::art::routes(art)),
        None => api.route(
            &format!("{}/{{*rest}}", crate::art::PREFIX),
            get(|| async { not_found("no game art on this server") }),
        ),
    };
    let Some(dir) = &site.web else {
        return api.fallback(|| async { not_found("no such endpoint") });
    };
    let index = tower_http::services::ServeFile::new(dir.join("index.html"));
    api.fallback_service(tower_http::services::ServeDir::new(dir).fallback(index))
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
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let created = live.create_account(&jws_body(body)?).await?;
    Ok(json(StatusCode::CREATED, &created))
}

async fn account(
    State(live): Shared,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.account(&gamertag(&tag)?)?))
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
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.friends(&gamertag(&tag)?)?))
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
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.presence()))
}

fn event_query(p: &HashMap<String, String>) -> ApiResult<Query> {
    let paging = paging(p)?;
    Ok(Query {
        after: paging.after,
        limit: paging.limit,
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
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.events(&event_query(&p)?).await?))
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
    Ok(json(StatusCode::OK, &published))
}

/// What a stream reader asked for.
struct StreamFilter {
    from: Option<u64>,
    /// `?stream=`: events of one stream only.
    stream: Option<String>,
    types: Vec<String>,
    exclude: Vec<String>,
    /// `?games=`: live frames only for these games (default: all).
    games: Vec<String>,
    /// `?frames=packed`: live frames as packed binary messages
    /// (docs/live/live-telemetry.md, Packed frames); JSON otherwise.
    packed: bool,
}

impl StreamFilter {
    fn query(&self, after: u64, limit: usize) -> Query {
        Query {
            after,
            limit,
            stream: self.stream.clone(),
            types: self.types.clone(),
            exclude: self.exclude.clone(),
        }
    }
}

async fn stream(
    State(live): Shared,
    Params(p): Params<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> ApiResult<Response> {
    let from = p
        .get("from")
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| invalid("from must be a position"))
        })
        .transpose()?;
    let filter = StreamFilter {
        from,
        stream: p.get("stream").filter(|s| !s.is_empty()).cloned(),
        types: list_param(&p, &["types", "type"]),
        exclude: list_param(&p, &["exclude"]),
        games: list_param(&p, &["games"]),
        packed: match p.get("frames").map(String::as_str) {
            None | Some("") | Some("json") => false,
            Some("packed") => true,
            Some(_) => return Err(invalid("frames must be json or packed")),
        },
    };
    let ws = ws.map_err(|_| invalid("/v1/stream is a WebSocket"))?;
    Ok(ws
        .max_message_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| async move {
            if let Err(reason) = run_stream(live, socket, filter).await {
                eprintln!("live: stream closed: {reason}");
            }
        }))
}

pub(crate) async fn send_json<T: serde::Serialize>(
    socket: &mut WebSocket,
    msg: &T,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).expect("serializes");
    socket.send(Message::Text(text.into())).await
}

async fn send(socket: &mut WebSocket, msg: &StreamMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).expect("serializes");
    socket.send(Message::Text(text.into())).await
}

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

/// Sends every stored event after `last` that the filter keeps, then
/// moves `last` to at least the position the read model had reached.
async fn catch_up(
    live: &Live,
    socket: &mut WebSocket,
    filter: &StreamFilter,
    last: &mut u64,
) -> Result<(), String> {
    const PAGE: usize = 500;
    let target = live.position();
    loop {
        let batch = live
            .store()
            .read(&filter.query(*last, PAGE))
            .await
            .map_err(|e| e.to_string())?;
        for e in &batch {
            send(
                socket,
                &StreamMessage::Event {
                    event: Box::new(e.to_wire()),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            *last = e.position;
        }
        if batch.len() < PAGE {
            *last = (*last).max(target);
            return Ok(());
        }
    }
}

/// Replays from `from` (default: the head), says `caught_up`, then
/// follows the dispatcher's feed. On a `--private` server the first
/// message must be a token, and the socket closes when it expires unless
/// the client sends a fresh one.
async fn run_stream(
    live: Arc<Live>,
    mut socket: WebSocket,
    filter: StreamFilter,
) -> Result<(), String> {
    let mut expires: Option<i64> = None;
    if live.policy().private {
        let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
        let result = match first {
            Ok(Some(Ok(Message::Text(text)))) => auth_message(&live, &text, lxm::READ),
            _ => Err(ApiError::new(
                ErrorCode::Unauthorized,
                "send an auth message first",
            )),
        };
        match result {
            Ok(exp) => expires = Some(exp),
            Err(e) => {
                close(&mut socket, CLOSE_UNAUTHORIZED, &e.message).await;
                return Ok(());
            }
        }
    }
    // Subscribe before replaying, so nothing falls between the two.
    let mut feed = live.feed();
    let mut frames = live.frames();
    let mut last = filter.from.unwrap_or_else(|| live.position());
    catch_up(&live, &mut socket, &filter, &mut last).await?;
    send(&mut socket, &StreamMessage::CaughtUp { position: last })
        .await
        .map_err(|e| e.to_string())?;
    let probe = filter.query(0, 1);
    let mut batch = PackedBatch::default();
    if filter.packed {
        // Start each live game's stream at its last keyframe.
        for f in live.frames_since_keyframe(&filter.games) {
            batch.add(&f);
        }
        batch.flush(&mut socket).await?;
    }
    loop {
        let deadline = async {
            match expires {
                Some(exp) => {
                    let wait = (exp + TOKEN_LEEWAY_S - crate::app::now()).max(0) as u64;
                    tokio::time::sleep(Duration::from_secs(wait)).await
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = feed.recv() => match event {
                Ok(e) => {
                    if e.position > last {
                        last = e.position;
                        if probe.matches(&e) {
                            send(&mut socket, &StreamMessage::Event { event: Box::new(e.to_wire()) }).await.map_err(|e| e.to_string())?;
                        }
                    }
                }
                // Fell behind the feed: re-read from the store.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    catch_up(&live, &mut socket, &filter, &mut last).await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            frame = frames.recv() => match frame {
                Ok(f) if filter.games.is_empty() || filter.games.contains(&f.frame.game_id) => {
                    if filter.packed {
                        batch.add(&f);
                    } else {
                        send(&mut socket, &StreamMessage::Frame { frame: Box::new(f.frame.clone()) }).await.map_err(|e| e.to_string())?;
                    }
                }
                Ok(_) => {}
                // Frames are best effort: a slow reader skips some. A packed
                // one picks its streams up again from their last keyframes.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if filter.packed {
                        for f in live.frames_since_keyframe(&filter.games) {
                            batch.add(&f);
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            _ = batch.due() => batch.flush(&mut socket).await?,
            msg = socket.recv() => match msg {
                // Fresh tokens on a private stream; public streams ignore
                // what clients send.
                Some(Ok(Message::Text(text))) if expires.is_some() => match auth_message(&live, &text, lxm::READ) {
                    Ok(exp) => expires = Some(exp),
                    Err(e) => {
                        close(&mut socket, CLOSE_UNAUTHORIZED, &e.message).await;
                        return Ok(());
                    }
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                Some(Ok(_)) => {}
            },
            _ = deadline => {
                close(&mut socket, CLOSE_UNAUTHORIZED, "token expired").await;
                return Ok(());
            }
        }
    }
}

/// How long a packed reader's frames wait to share a message.
const PACKED_BATCH: Duration = Duration::from_millis(100);

/// A packed reader's next binary message: frames of every stream it
/// follows, each only once and only after its base (a frame that does not
/// follow the last one sent of its stream waits for a keyframe).
#[derive(Default)]
struct PackedBatch {
    bytes: Vec<u8>,
    due: Option<tokio::time::Instant>,
    /// Each stream's last sequence number sent.
    sent: HashMap<u64, u64>,
}

impl PackedBatch {
    fn add(&mut self, f: &crate::games::LiveFrame) {
        let Some(packed) = &f.packed else {
            return;
        };
        let follows = match self.sent.get(&f.sid) {
            Some(last) if f.seq <= *last => return,
            Some(last) => f.seq == last + 1 || f.keyframe,
            None => f.keyframe,
        };
        if !follows {
            return;
        }
        self.sent.insert(f.sid, f.seq);
        live_proto::pack::push_record(&mut self.bytes, packed);
        self.due
            .get_or_insert_with(|| tokio::time::Instant::now() + PACKED_BATCH);
    }

    /// Resolves when the batch should go out (never while it is empty).
    async fn due(&self) {
        match self.due {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    }

    async fn flush(&mut self, socket: &mut WebSocket) -> Result<(), String> {
        self.due = None;
        if self.bytes.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.bytes);
        socket
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|e| e.to_string())
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

async fn list_games(
    State(live): Shared,
    headers: HeaderMap,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
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
        &live.games_where(state, &filter, before, paging.limit),
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
    live.authorize_read(bearer(&headers))?;
    Ok(json(StatusCode::OK, &live.game(game_id(&id)?)?))
}

async fn game_frames(
    State(live): Shared,
    headers: HeaderMap,
    Path(id): Path<String>,
    Params(p): Params<HashMap<String, String>>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
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
    Ok(json(
        StatusCode::OK,
        &live.put_outline(bearer(&headers), &map, &bytes).await?,
    ))
}
