//! The Worker: its fetch handler and the `LiveHub` Durable Object.
//!
//! Every request goes to the one hub, which runs live-server's router
//! over the log in its own SQLite storage and blobs in R2. `/v1/stream` is
//! a hibernatable WebSocket ([`crate::sockets`]); ingest and the relay
//! come in later slices (docs/live/live-worker.md).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use live_server::Live;
use live_server::blobs::{BlobStore, Blobs};
use live_server::store::EventStore;
use live_server::stream::StreamFilter;
use tokio::sync::OnceCell;
use tower_service::Service;
use wasm_bindgen::JsValue;
use worker::{
    Context, DurableObject, Env, HttpRequest, Request, Response, Result, SqlStorage, State,
    WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object, event,
};

use crate::bindings::{BLOBS, HUB, HUB_NAME};
use crate::dosql::DoSql;
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
    sockets: Rc<Sockets>,
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
        let live = Live::start(Arc::new(store), settings.identity, settings.policy)
            .await
            .map_err(|e| rust_error(format!("{e:#}")))?;
        let r2 = R2::new(self.env.bucket(BLOBS)?, BLOBS);
        live.set_blobs(Blobs {
            private: BlobStore::from_store(Arc::new(r2), "r2://blobs"),
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
                self.sockets.adopt_all(&live, &self.state);
                live_server::rt::spawn(pump(live.clone(), self.sockets.clone()));
                Ok::<_, worker::Error>(live)
            })
            .await?
            .clone();
        Ok((live, cold))
    }

    /// `GET /v1/stream`: a WebSocket the object accepts for hibernation.
    async fn stream(&self, live: Arc<Live>, req: &Request) -> Result<Response> {
        let params: HashMap<String, String> = req.url()?.query_pairs().into_owned().collect();
        let filter = match StreamFilter::from_params(&params) {
            Ok(f) => f,
            Err(e) => return Response::try_from(axum::response::IntoResponse::into_response(e)),
        };
        let upgrade = req.headers().get("upgrade")?.unwrap_or_default();
        if !upgrade.eq_ignore_ascii_case("websocket") {
            let e = live_server::ApiError::new(
                live_proto::ErrorCode::Invalid,
                "/v1/stream is a WebSocket",
            );
            return Response::try_from(axum::response::IntoResponse::into_response(e));
        }
        let pair = WebSocketPair::new()?;
        self.sockets
            .accept(&self.state, live, pair.server, filter)
            .await?;
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
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let (live, cold) = self.live().await?;
        if req.path() == "/v1/stream" {
            return self.stream(live, &req).await;
        }
        let req: HttpRequest = req.try_into()?;
        let mut router = live_server::http::router(live);
        let mut resp = match router.call(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        };
        if let Some((ms, head)) = cold
            && let Ok(v) = format!("start;dur={ms}, events;desc={head}").parse()
        {
            resp.headers_mut().append(crate::COLD_START_HEADER, v);
        }
        Response::try_from(resp)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let (live, _) = self.live().await?;
        if let WebSocketIncomingMessage::String(text) = message {
            self.sockets.message(&live, ws, &text).await;
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.sockets.closed(&ws);
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.sockets.closed(&ws);
        Ok(())
    }
}
