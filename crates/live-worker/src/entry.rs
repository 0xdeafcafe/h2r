//! The Worker: its fetch handler and the `LiveHub` Durable Object.
//!
//! Every request goes to the one hub, which runs live-server's router
//! over the log in its own SQLite storage and blobs in R2. `/v1/stream` is
//! a hibernatable WebSocket ([`crate::sockets`]); ingest and the relay
//! come in later slices (docs/live/live-worker.md).

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
    Context, DurableObject, Env, HttpRequest, Request, Response, Result, SqlStorage, State,
    WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object, event,
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
    _ctx: Context,
) -> Result<http::Response<axum::body::Body>> {
    let hub = env
        .durable_object(HUB)?
        .id_from_name(HUB_NAME)?
        .get_stub()?;
    let resp = hub.fetch_with_request(Request::try_from(req)?).await?;
    let resp: worker::HttpResponse = resp.try_into()?;
    Ok(resp.map(axum::body::Body::new))
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
    async fn socket(&self, live: Arc<Live>, req: &Request) -> Result<Response> {
        let refuse = |m: &str| {
            let e = live_server::ApiError::new(live_proto::ErrorCode::Invalid, m);
            Response::try_from(axum::response::IntoResponse::into_response(e))
        };
        let stream = req.path() == "/v1/stream";
        let filter = if stream {
            let params: HashMap<String, String> = req.url()?.query_pairs().into_owned().collect();
            match StreamFilter::from_params(&params) {
                Ok(f) => Some(f),
                Err(e) => {
                    return Response::try_from(axum::response::IntoResponse::into_response(e));
                }
            }
        } else {
            None
        };
        let upgrade = req.headers().get("upgrade")?.unwrap_or_default();
        if !upgrade.eq_ignore_ascii_case("websocket") {
            return refuse(&format!("{} is a WebSocket", req.path()));
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
        if req.path() == "/v1/stream" || req.path() == "/v1/ingest" {
            let resp = self.socket(live.clone(), &req).await;
            self.schedule(&live).await?;
            return resp;
        }
        let req: HttpRequest = req.try_into()?;
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
