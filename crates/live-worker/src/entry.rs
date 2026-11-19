//! The Worker: its fetch handler and the `LiveHub` Durable Object.
//!
//! Every request goes to the one hub, which runs live-server's router
//! over the log in its own SQLite storage and blobs in R2. `/v1/stream` is
//! a hibernatable WebSocket ([`crate::sockets`]); the hub also runs ingest
//! and authorizes co-op relay upgrades (docs/live/live-worker.md).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use live_server::Live;
use live_server::app::Timers;
use live_server::blobs::{BlobStore, Blobs};
use live_server::store::EventStore;
use live_server::stream::StreamFilter;
use live_server::titleblobs::BlobsElsewhere;
use tokio::sync::OnceCell;
use tower_service::Service;
use wasm_bindgen::JsValue;
use worker::{
    Context, DurableObject, Env, Headers, HttpRequest, Request, RequestInit, Response, Result,
    SqlStorage, State, WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object, event,
};

use crate::bindings::{BLOBS, HUB, HUB_NAME};
use crate::dosql::DoSql;
use crate::ingest::{Ingests, Message};
use crate::r2::R2;
use crate::settings::Settings;
use crate::sockets::{Sockets, pump};
use crate::sqlstore::SqlStore;

/// Every request to the hub: one log, one writer, one read model.
#[event(fetch)]
async fn fetch(
    req: HttpRequest,
    env: Env,
    ctx: Context,
) -> Result<http::Response<axum::body::Body>> {
    let path = req.uri().path().to_string();
    // Game art is a redirect or a 404 from here, without waking the hub.
    let art_url = env
        .var(crate::settings::GAME_ART_URL)
        .ok()
        .map(|v| v.to_string());
    if crate::site::is_emblem(&path) {
        return Ok(crate::emblem::image(&req, art_url.as_deref(), &ctx).await);
    }
    if let Some(art) = crate::site::game_art(art_url.as_deref(), &path, req.uri().query()) {
        use axum::response::IntoResponse;
        return Ok(match art {
            crate::site::GameArt::Redirect { to, cache_control } => {
                let mut resp = axum::response::Redirect::temporary(&to).into_response();
                resp.headers_mut().insert(
                    http::header::CACHE_CONTROL,
                    http::HeaderValue::from_static(cache_control),
                );
                resp
            }
            crate::site::GameArt::NotFound(why) => live_server::app::not_found(why).into_response(),
        });
    }
    // The stats site's first address: the same page where it is now.
    // (live_server::page_meta::moved; wrangler.toml sends `/stats/*` here.)
    if matches!(*req.method(), http::Method::GET | http::Method::HEAD)
        && let Some(to) = live_server::page_meta::moved(&path, req.uri().query())
    {
        use axum::response::IntoResponse;
        return Ok((
            http::StatusCode::MOVED_PERMANENTLY,
            [(http::header::LOCATION, to)],
        )
            .into_response());
    }
    // The app's pages: its HTML with the page's link preview.
    if live_server::page_meta::is_page(&path)
        && matches!(*req.method(), http::Method::GET | http::Method::HEAD)
        && env.assets(crate::bindings::ASSETS).is_ok()
    {
        return app_page(&req, &env).await;
    }
    let stub = env
        .durable_object(HUB)?
        .id_from_name(HUB_NAME)?
        .get_stub()?;
    let method = req.method().as_str().to_string();
    let resp = fetch_retrying(&stub, Request::try_from(req)?, &method, &path).await?;
    let mut resp: worker::HttpResponse = resp.try_into()?;
    if let Some(encoding) = resp.headers_mut().remove("x-h2-stored-encoding") {
        resp.headers_mut().insert(http::header::CONTENT_ENCODING, encoding);
        resp.extensions_mut().insert(worker::EncodeBody::Manual);
    }
    Ok(resp.map(axum::body::Body::new))
}

/// A page of the web app: the built `index.html` from the assets, with
/// the page's link preview in it ([`live_server::page_meta`]) and, for a
/// game page, its pre-rendered header above the app
/// ([`live_server::page_header`]). Only a page that names a player, game
/// or clan asks the hub; if that fails the page still gets its section's
/// preview (and no header, which the client then renders itself).
async fn app_page(req: &HttpRequest, env: &Env) -> Result<http::Response<axum::body::Body>> {
    use live_server::page_meta;
    let path = req.uri().path();
    let origin = page_meta::origin(
        env.var(crate::settings::PUBLIC_URL)
            .ok()
            .map(|v| v.to_string())
            .as_deref(),
        req.uri().authority().map(|a| a.as_str()).or_else(|| {
            req.headers()
                .get(http::header::HOST)
                .and_then(|h| h.to_str().ok())
        }),
        req.uri().scheme_str(),
    );
    // The assets answer by path; the request's own origin makes the URL.
    let here = format!(
        "{}://{}/",
        req.uri().scheme_str().unwrap_or("https"),
        req.uri().authority().map_or("localhost", |a| a.as_str())
    );
    let assets = env.assets(crate::bindings::ASSETS)?;
    let mut index = Response::try_from(assets.fetch(here, None).await?)?;
    if index.status_code() != 200 {
        let resp: worker::HttpResponse = index.try_into()?;
        return Ok(resp.map(axum::body::Body::new));
    }
    let body = if *req.method() == http::Method::HEAD {
        String::new()
    } else {
        let html = index.text().await?;
        // The hub rate-limits reads per client IP: pass the visitor's on.
        let ip = req
            .headers()
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        crate::site::app_html(&html, &origin, path, |url| async move {
            let hub = env
                .durable_object(HUB)
                .ok()?
                .id_from_name(HUB_NAME)
                .ok()?
                .get_stub()
                .ok()?;
            let headers = Headers::new();
            if let Some(ip) = &ip {
                headers.set("cf-connecting-ip", ip).ok()?;
            }
            let mut init = RequestInit::new();
            init.with_headers(headers);
            let call = Request::new_with_init(&url, &init).ok()?;
            let mut response = hub.fetch_with_request(call).await.ok()?;
            if response.status_code() != 200 {
                return None;
            }
            response.text().await.ok()
        })
        .await
    };
    http::Response::builder()
        .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(
            http::header::CACHE_CONTROL,
            live_server::caching::REVALIDATE,
        )
        .body(axum::body::Body::from(body))
        .map_err(rust_error)
}

/// Calls the object, again if a deploy reset it under a request that is
/// safe to repeat; a 503 with `Retry-After` if it cannot be reached
/// ([`crate::doretry`]).
async fn fetch_retrying(
    stub: &worker::Stub,
    req: Request,
    method: &str,
    path: &str,
) -> Result<Response> {
    use crate::doretry::{self, Outcome, Thrown};
    // Each attempt sends a copy (the body teed), so a retry has its own;
    // a big body is sent once instead, as the tee would hold all of it.
    let length = req
        .headers()
        .get("content-length")?
        .and_then(|v| v.trim().parse::<u64>().ok());
    let kept = doretry::keeps_body(req.inner().body().is_some(), length);
    let mut req = Some(req);
    let mut attempt = 1;
    loop {
        let call = if kept {
            req.as_ref().expect("kept for every attempt").clone()?
        } else {
            req.take().expect("a body that is not kept is sent once")
        };
        let error = match stub.fetch_with_request(call).await {
            Ok(resp) => return Ok(resp),
            Err(e) => e,
        };
        let original = error.to_js_value();
        let flag = |name: &str| {
            js_sys::Reflect::get(&original, &JsValue::from_str(name))
                .ok()
                .and_then(|v| v.as_bool())
        };
        let thrown = Thrown {
            retryable: flag("retryable"),
            overloaded: flag("overloaded"),
            message: error.to_string(),
        };
        match doretry::decide_kept(&thrown, method, path, attempt, kept) {
            Outcome::Retry(pause) => {
                worker::console_log!(
                    "live: {method} {path} attempt {attempt} threw ({}), retrying",
                    thrown.message
                );
                worker::Delay::from(pause).await;
                attempt += 1;
            }
            Outcome::Unavailable => {
                worker::console_log!(
                    "live: {method} {path} unavailable after {attempt} attempt(s): {}",
                    thrown.message
                );
                let mut resp =
                    Response::from_bytes(doretry::unavailable_body(&thrown.message).into_bytes())?
                        .with_status(503);
                resp.headers_mut()
                    .set("retry-after", &doretry::RETRY_AFTER_S.to_string())?;
                resp.headers_mut().set("content-type", "application/json")?;
                return Ok(resp);
            }
            Outcome::Fail => return Err(error),
        }
    }
}

#[durable_object]
pub struct LiveHub {
    env: Env,
    state: State,
    sql: SqlStorage,
    /// The object's `ctx` as JavaScript, for `storage.transactionSync`.
    ctx: JsValue,
    live: OnceCell<Arc<Live>>,
    sockets: Rc<Sockets<WebSocket>>,
    ingests: Ingests,
    /// The alarm as last set (ms since the epoch; `None` inside: no
    /// alarm), or `None` before this instance has set one.
    alarm_at: Cell<Option<Option<i64>>>,
    /// Whether the packed tick is running.
    ticking: Rc<Cell<bool>>,
}

fn rust_error(e: impl std::fmt::Display) -> worker::Error {
    worker::Error::RustError(e.to_string())
}

impl LiveHub {
    /// A variable or a secret.
    fn setting(&self, name: &str) -> Option<String> {
        self.env
            .var(name)
            .map(|v| v.to_string())
            .or_else(|_| self.env.secret(name).map(|s| s.to_string()))
            .ok()
    }

    /// Starts the server: the read model replays the log from the object's
    /// storage. Returns how long that took (ms) and the log's head.
    async fn start(&self) -> Result<(Arc<Live>, f64, u64)> {
        let settings = Settings::from_env(&|name| self.setting(name)).map_err(rust_error)?;
        let started = js_sys::Date::now();
        let store = SqlStore::open(DoSql::new(self.sql.clone(), &self.ctx)?)
            .await
            .map_err(rust_error)?;
        let head = store.head().await.map_err(rust_error)?;
        let private = BlobStore::from_store(
            Arc::new(R2::new(self.env.bucket(BLOBS)?, BLOBS)),
            "r2://blobs",
        );
        // Title files in R2: a Durable Object's row holds at most 2 MB.
        let store = BlobsElsewhere::new(Arc::new(store), private.clone());
        let live = Live::start_with(
            Arc::new(store),
            settings.identity,
            settings.policy,
            Timers::Driven,
        )
        .await
        .map_err(|e| rust_error(format!("{e:#}")))?;
        live.set_blobs(Blobs {
            private,
            public: None,
        });
        Ok((live, js_sys::Date::now() - started, head))
    }
}

impl LiveHub {
    /// The server, started on the first request or socket message after
    /// the object wakes; with the cold start's timing when this call did
    /// it. Starting also starts the socket pump and takes back the sockets
    /// that stayed open while the object slept.
    async fn live(&self) -> Result<(Arc<Live>, Option<(f64, u64)>)> {
        let mut cold = None;
        let live = self
            .live
            .get_or_try_init(|| async {
                let (live, ms, head) = self.start().await?;
                worker::console_log!("live: started in {ms} ms, the log at {head}");
                cold = Some((ms, head));
                self.sockets.adopt_all(&live, self.state.get_websockets());
                live_server::rt::spawn(pump(live.clone(), self.sockets.clone()));
                Ok::<_, worker::Error>(live)
            })
            .await?
            .clone();
        Ok((live, cold))
    }

    /// After anything that may have changed what is due: sets the alarm
    /// for the next sweep or socket check (or removes it), and runs the
    /// packed tick while a game is live. Nothing else in the hub runs on
    /// a timer, so with no alarm due and no game live it can hibernate.
    async fn schedule(&self, live: &Arc<Live>) -> Result<()> {
        if live.packed_active() && !self.ticking.replace(true) {
            let (live, ticking) = (live.clone(), self.ticking.clone());
            live_server::rt::spawn(async move {
                let mut tick = live_server::rt::interval(live_server::packed::TICK);
                loop {
                    tick.tick().await;
                    if !live.packed_active() {
                        ticking.set(false);
                        return;
                    }
                    live.publish_packed();
                }
            });
        }
        let sweep = live.sweep_due().map(|s| s as f64 * 1000.0);
        let due = [
            sweep,
            self.sockets.next_due_ms(),
            self.ingests.next_due_ms(),
        ]
        .into_iter()
        .flatten()
        .min_by(f64::total_cmp)
        .map(|ms| ms.ceil() as i64);
        if self.alarm_at.get() == Some(due) {
            return Ok(());
        }
        let storage = self.state.storage();
        match due {
            Some(at) => {
                // Not sooner than 250 ms: a sweep that could not write must not
                // spin the alarm.
                let wait = (at - js_sys::Date::now() as i64).max(250);
                storage.set_alarm(wait).await?;
            }
            None => storage.delete_alarm().await?,
        }
        self.alarm_at.set(Some(due));
        Ok(())
    }

    fn closed(&self, ws: &WebSocket) {
        match crate::ingest::attachment(ws) {
            Some(a) => self.ingests.closed(a.ingest),
            None => self.sockets.closed(ws),
        }
    }

    /// `GET /v1/stream` and `GET /v1/ingest`: WebSockets the object
    /// accepts for hibernation.
    async fn socket(&self, live: Arc<Live>, req: &HttpRequest) -> Result<Response> {
        let refuse = |m: &str| {
            let e = live_server::ApiError::new(live_proto::ErrorCode::Invalid, m);
            Response::try_from(axum::response::IntoResponse::into_response(e))
        };
        let path = req.uri().path();
        let filter = if path == "/v1/stream" {
            let Ok(axum::extract::Query(params)) =
                axum::extract::Query::<HashMap<String, String>>::try_from_uri(req.uri())
            else {
                return refuse("invalid stream query");
            };
            match StreamFilter::from_params(&params) {
                Ok(f) => Some(f),
                Err(e) => {
                    return Response::try_from(axum::response::IntoResponse::into_response(e));
                }
            }
        } else {
            None
        };
        if req.method() != http::Method::GET || !crate::relay::websocket_upgrade(req.headers()) {
            return refuse(&format!("{path} is a WebSocket"));
        }
        let pair = WebSocketPair::new()?;
        match filter {
            Some(filter) => {
                self.state.accept_web_socket(&pair.server);
                self.sockets.accept(live, pair.server, filter).await
            }
            None => self.ingests.accept(&self.state, live, pair.server),
        }
        Response::from_websocket(pair.client)
    }
}

impl DurableObject for LiveHub {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        let inner = state._inner();
        let ctx: JsValue = inner.clone().into();
        LiveHub {
            env,
            state: State::from(inner),
            sql,
            ctx,
            live: OnceCell::new(),
            sockets: Rc::new(Sockets::default()),
            ingests: Ingests::default(),
            alarm_at: Cell::new(None),
            ticking: Rc::new(Cell::new(false)),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let (live, cold) = self.live().await?;
        let req: HttpRequest = req.try_into()?;
        let path = req.uri().path();
        let relay = crate::relayroom::session_of(path);
        if path == "/v1/stream" || path == "/v1/ingest" || relay.is_some() {
            if matches!(*req.method(), http::Method::GET | http::Method::HEAD) {
                let ip = live_server::http::client_ip_key(&live, None, req.headers());
                if let Err(e) = live_server::http::limit_read(&live, &ip) {
                    return Response::try_from(axum::response::IntoResponse::into_response(e));
                }
            }
            if let Some(session) = relay {
                let session = session.to_owned();
                return crate::relay::upgrade(&live, &self.env, req, &session).await;
            }
            let resp = self.socket(live.clone(), &req).await;
            self.schedule(&live).await?;
            return resp;
        }
        let mut router = live_server::http::router(live.clone());
        let mut resp = match router.call(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        };
        if let Some((ms, head)) = cold
            && let Ok(v) = format!("start;dur={ms}, events;desc={head}").parse()
        {
            resp.headers_mut().append(crate::COLD_START_HEADER, v);
        }
        // Carry the stored representation through the internal subrequest.
        // Only the outer HTTP response declares its content encoding.
        if resp.headers().get(http::header::CONTENT_TYPE).is_some_and(|v| v == "application/x-h2-replay")
            && let Some(encoding) = resp.headers_mut().remove(http::header::CONTENT_ENCODING) {
            resp.headers_mut().insert("x-h2-stored-encoding", encoding);
        }
        self.schedule(&live).await?;
        Response::try_from(resp)
    }

    /// The sweep and the sockets' timed checks, when [`LiveHub::schedule`]
    /// said they were due.
    async fn alarm(&self) -> Result<Response> {
        let (live, _) = self.live().await?;
        // Set again below if anything is still due.
        self.alarm_at.set(Some(None));
        live.sweep().await;
        self.sockets.check();
        self.ingests.check();
        self.schedule(&live).await?;
        Response::ok("")
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let (live, _) = self.live().await?;
        if let Some(a) = crate::ingest::attachment(&ws) {
            let msg = match &message {
                WebSocketIncomingMessage::String(t) => Message::Text(t),
                WebSocketIncomingMessage::Binary(b) => Message::Binary(b),
            };
            self.ingests.message(&live, ws, a, msg).await;
        } else if let WebSocketIncomingMessage::String(text) = message {
            self.sockets.message(&live, ws, &text).await;
        }
        self.schedule(&live).await
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.closed(&ws);
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.closed(&ws);
        Ok(())
    }
}
