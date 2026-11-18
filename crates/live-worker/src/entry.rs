//! The Worker: its fetch handler and the `LiveHub` Durable Object.
//!
//! Slice 1 (docs/live/live-worker.md): every request goes to the one hub,
//! which runs live-server's router over the D1 log and R2 blobs. The
//! WebSocket routes (stream, ingest, relay) are not here yet; they come
//! with the hibernation API in later slices.

use std::sync::Arc;

use live_server::blobs::{BlobStore, Blobs};
use live_server::{Live, Policy, ServerIdentity};
use tokio::sync::OnceCell;
use tower_service::Service;
use worker::{
    Context, DurableObject, Env, HttpRequest, Request, Response, Result, State, durable_object,
    event,
};

use crate::bindings::{BLOBS, DB, HUB, HUB_NAME, SERVER_ID, SERVER_KEY};
use crate::d1::D1;
use crate::d1store::D1Store;
use crate::r2::R2;

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
    live: OnceCell<Arc<Live>>,
}

impl LiveHub {
    /// The server, started on the first request after the object wakes:
    /// the read model replays the log from D1 (docs/live/live-worker.md,
    /// "Cold starts").
    async fn live(&self) -> Result<Arc<Live>> {
        self.live
            .get_or_try_init(|| async {
                let id = self.env.var(SERVER_ID)?.to_string();
                let key = self.env.secret(SERVER_KEY)?.to_string();
                let identity = ServerIdentity::from_secret(&id, &key)
                    .map_err(|e| worker::Error::RustError(e.to_string()))?;
                let store = D1Store::open(D1::new(self.env.d1(DB)?))
                    .await
                    .map_err(|e| worker::Error::RustError(e.to_string()))?;
                let live = Live::start(Arc::new(store), identity, Policy::default())
                    .await
                    .map_err(|e| worker::Error::RustError(format!("{e:#}")))?;
                let r2 = R2::new(self.env.bucket(BLOBS)?, BLOBS);
                live.set_blobs(Blobs {
                    private: BlobStore::from_store(Arc::new(r2), "r2://blobs"),
                    public: None,
                });
                Ok(live)
            })
            .await
            .cloned()
    }
}

impl DurableObject for LiveHub {
    fn new(_state: State, env: Env) -> Self {
        LiveHub {
            env,
            live: OnceCell::new(),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let live = self.live().await?;
        let req: HttpRequest = req.try_into()?;
        let mut router = live_server::http::router(live);
        let resp = match router.call(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        };
        Response::try_from(resp)
    }
}
