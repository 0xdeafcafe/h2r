//! One named Durable Object and at most one container for the life of a lease.
//! Only LiveHub can reach this binding. Control requests have no public route.

#[path = "hostroom/lease.rs"]
mod lease;

use async_trait::async_trait;
use axum::response::IntoResponse;
use js_sys::futures::JsFuture;
use live_proto::hosts::{HostClass, HostLease, ProviderKind, Transport};
use live_proto::{Author, ErrorCode, lxm};
use live_server::app::{ApiError, Live};
use live_server::hosts::{HostBoot, HostError, HostProvider};
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use worker::{
    Container, DurableObject, Env, Headers, HttpRequest, Method, Request, RequestInit, Response,
    Result, State, Stub, durable_object,
};

pub(super) use lease::tunnel_lease;
use lease::{HostEnv, TUNNEL_PORT, admit, binding, parse_ref, provider_ref, valid_id};

const ROOM_KEY: &str = "lease";

#[derive(Serialize, Deserialize)]
struct Start {
    env: HostEnv,
    class: HostClass,
    deadline_ms: i64,
}

/// Only the hard deadline survives eviction.
/// A stopped lease retains a tombstone so a delayed start cannot resurrect it.
#[derive(Serialize, Deserialize)]
enum RoomLease {
    Active { deadline_ms: i64 },
    Stopped,
}

fn stub(env: &Env, namespace: &str, lease_id: &str, region: Option<&str>) -> Result<Stub> {
    let rooms = env.durable_object(namespace)?;
    let id = rooms.id_from_name(lease_id)?;
    match region {
        // Cloudflare Durable Object location hints. Other providers also
        // put region names in leases, including three-letter airport codes.
        Some(hint @ ("wnam" | "enam" | "sam" | "weur" | "eeur" | "apac" | "oc" | "afr" | "me")) => {
            id.get_stub_with_location_hint(hint)
        }
        _ => id.get_stub(),
    }
}

pub(super) struct CloudflareProvider {
    env: SendWrapper<Env>,
}

impl CloudflareProvider {
    pub(super) fn new(env: Env) -> Self {
        Self {
            env: SendWrapper::new(env),
        }
    }

    async fn call(
        &self,
        namespace: &str,
        lease_id: &str,
        region: Option<&str>,
        method: Method,
        path: &str,
        body: Option<&Start>,
    ) -> Result<Response> {
        let mut init = RequestInit::new();
        init.with_method(method);
        if let Some(start) = body {
            let headers = Headers::new();
            headers.set("content-type", "application/json")?;
            init.with_headers(headers)
                .with_body(Some(JsValue::from_str(&serde_json::to_string(start)?)));
        }
        let req = Request::new_with_init(&format!("https://host-room/{path}"), &init)?;
        let response = stub(&self.env, namespace, lease_id, region)?
            .fetch_with_request(req)
            .await?;
        if !(200..300).contains(&response.status_code()) {
            return Err(worker::Error::RustError(format!(
                "HostRoom {path} returned HTTP {}",
                response.status_code()
            )));
        }
        Ok(response)
    }
}

#[async_trait]
impl HostProvider for CloudflareProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cloudflare
    }

    async fn start(
        &self,
        lease: &HostLease,
        boot: &HostBoot,
    ) -> std::result::Result<String, HostError> {
        if lease.provider != self.kind()
            || lease.transport != Transport::Tunnel
            || !valid_id(&lease.id)
        {
            return Err(HostError::new(
                "expected a Cloudflare host lease with tunnel transport",
            ));
        }
        let start = Start {
            env: HostEnv::new(lease, boot),
            class: lease.class.clone(),
            deadline_ms: lease
                .created_ms
                .saturating_add(i64::from(lease.max_match_s) * 1000),
        };
        SendWrapper::new(async {
            let namespace = binding(&lease.class);
            let result = self
                .call(
                    namespace,
                    &lease.id,
                    lease.region.as_deref(),
                    Method::Post,
                    "start",
                    Some(&start),
                )
                .await;
            if let Err(error) = result {
                // A lost response can leave the process running even when Live
                // never received a provider reference. Reopen the stub and stop
                // that exact lease; its room alarm covers a second outage.
                if self
                    .call(namespace, &lease.id, None, Method::Post, "stop", None)
                    .await
                    .is_err()
                {
                    worker::console_warn!("host room cleanup could not reach {}", lease.id);
                }
                return Err(HostError::new(error.to_string()));
            }
            Ok(provider_ref(lease))
        })
        .await
    }

    async fn stop(&self, lease_id: &str, provider_ref: &str) -> std::result::Result<(), HostError> {
        let (namespace, id) = parse_ref(provider_ref)
            .filter(|(_, id)| *id == lease_id)
            .ok_or_else(|| HostError::new("HostRoom reference belongs to a different lease"))?;
        SendWrapper::new(async {
            self.call(namespace, id, None, Method::Post, "stop", None)
                .await?;
            Ok::<_, worker::Error>(())
        })
        .await
        .map_err(|e| HostError::new(e.to_string()))
    }

    async fn alive(&self, provider_ref: &str) -> std::result::Result<bool, HostError> {
        let (namespace, id) =
            parse_ref(provider_ref).ok_or_else(|| HostError::new("invalid host room reference"))?;
        SendWrapper::new(async {
            self.call(namespace, id, None, Method::Get, "alive", None)
                .await?
                .json::<bool>()
                .await
        })
        .await
        .map_err(|e| HostError::new(e.to_string()))
    }
}

/// Authenticate once on the hub and check the current lease before opening
/// its private room. Neither credentials nor arbitrary client headers need to
/// cross into the container.
pub(super) async fn upgrade(
    live: &Live,
    env: &Env,
    req: HttpRequest,
    id: &str,
) -> Result<Response> {
    if req.method() != http::Method::GET || !crate::relay::websocket_upgrade(req.headers()) {
        return refuse(ErrorCode::Invalid, "the host tunnel is a WebSocket");
    }
    let bearer = live_server::http::bearer(req.headers());
    let xuid = match live.authenticate(bearer, lxm::SESSIONS_RELAY) {
        Ok(Author::Account { xuid, .. }) => xuid,
        Ok(_) => return refuse(ErrorCode::Forbidden, "the host tunnel is for accounts"),
        Err(e) => return Response::try_from(e.into_response()),
    };
    let Some(view) = live.host_view(id) else {
        return refuse(ErrorCode::NotFound, "unknown host lease");
    };
    if let Err(e) = admit(&view.lease, &view.state, xuid) {
        return Response::try_from(e.into_response());
    }
    let headers = Headers::new();
    headers.set("upgrade", "websocket")?;
    if let Some(protocol) = req.headers().get(http::header::SEC_WEBSOCKET_PROTOCOL) {
        headers.set(
            "sec-websocket-protocol",
            protocol.to_str().map_err(super::rust_error)?,
        )?;
    }
    let mut init = RequestInit::new();
    init.with_headers(headers);
    let request = Request::new_with_init(&format!("http://container/v1/hosts/{id}/tunnel"), &init)?;
    stub(env, binding(&view.lease.class), id, None)?
        .fetch_with_request(request)
        .await
}

fn refuse(code: ErrorCode, message: &str) -> Result<Response> {
    Response::try_from(ApiError::new(code, message).into_response())
}

// workers-rs 0.9 exposes running, destroy and get_tcp_port. Its startup
// options use JS Map for env; workerd expects a Record<string, string>.
// setInactivityTimeout is also missing from the Rust wrapper.
#[wasm_bindgen]
extern "C" {
    type ContainerControl;

    #[wasm_bindgen(method, structural, catch, js_name = start)]
    fn start_with_options(
        this: &ContainerControl,
        options: &JsValue,
    ) -> std::result::Result<(), JsValue>;

    #[wasm_bindgen(method, structural, catch, js_name = setInactivityTimeout)]
    fn inactivity_timeout(
        this: &ContainerControl,
        ms: f64,
    ) -> std::result::Result<js_sys::Promise, JsValue>;
}

async fn keep_until(container: &Container, deadline_ms: i64) -> Result<()> {
    // Cloudflare measures this from DO inactivity, not from the last game
    // datagram. Silence from the hub must not interrupt an active match.
    let remaining = deadline_ms
        .saturating_sub(js_sys::Date::now() as i64)
        .clamp(1, 21_600_000);
    let raw: &ContainerControl = container.unchecked_ref();
    JsFuture::from(raw.inactivity_timeout(remaining as f64)?).await?;
    Ok(())
}

struct Room {
    state: State,
    class: HostClass,
    lifecycle: tokio::sync::Mutex<()>,
}

impl Room {
    fn container(&self) -> Result<Container> {
        self.state
            .container()
            .ok_or_else(|| worker::Error::RustError("HostRoom has no container binding".into()))
    }

    async fn start(&self, container: &Container, start: Start) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        if self.class != start.class
            || self.state.id().name().as_deref() != Some(start.env.h2_lease.as_str())
        {
            return refuse(ErrorCode::Invalid, "lease does not name this HostRoom");
        }
        let remaining = start.deadline_ms.saturating_sub(js_sys::Date::now() as i64);
        if remaining <= 0 {
            return refuse(ErrorCode::Gone, "host lease deadline has passed");
        }
        let storage = self.state.storage();
        if let Some(existing) = storage.get::<RoomLease>(ROOM_KEY).await? {
            return if matches!(existing, RoomLease::Active { .. }) && container.running() {
                Response::empty()
            } else {
                refuse(ErrorCode::Gone, "this host lease has already ended")
            };
        }

        // The image entrypoint sets up the module and bundle cache, then
        // execs /opt/h2/h2 host run with flags from this lease's environment.
        let options = serde_json::json!({
            "entrypoint": ["/opt/h2/entrypoint.sh"],
            "enableInternet": true,
            "env": start.env,
        });
        let options = js_sys::JSON::parse(&serde_json::to_string(&options)?)?;

        // Arm cleanup before starting billable compute. No token or bundle
        // URL is kept in storage. DO storage operations hold the input gate.
        let active = RoomLease::Active {
            deadline_ms: start.deadline_ms,
        };
        storage.put(ROOM_KEY, active).await?;
        storage.set_alarm(remaining).await?;
        let raw: &ContainerControl = container.unchecked_ref();
        raw.start_with_options(&options)
            .map_err(|_| worker::Error::RustError("container start failed".into()))?;
        if let Err(error) = keep_until(container, start.deadline_ms).await {
            container.destroy(None).await?;
            return Err(error);
        }
        Response::empty()
    }

    async fn stop(&self, container: &Container) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let storage = self.state.storage();
        storage.put(ROOM_KEY, RoomLease::Stopped).await?;
        if container.running() {
            container.destroy(None).await?;
        }
        storage.delete_alarm().await?;
        Response::empty()
    }

    async fn tunnel(&self, container: &Container, req: Request, id: &str) -> Result<Response> {
        if self.state.id().name().as_deref() != Some(id)
            || !req
                .headers()
                .get("upgrade")?
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        {
            return refuse(ErrorCode::Invalid, "invalid host tunnel upgrade");
        }
        let Some(RoomLease::Active { deadline_ms }) = self.state.storage().get(ROOM_KEY).await?
        else {
            return refuse(ErrorCode::NotFound, "no active host lease");
        };
        if deadline_ms <= js_sys::Date::now() as i64 || !container.running() {
            return refuse(ErrorCode::Gone, "the host is stopped");
        }
        // The fetcher terminates HTTP on the private container port. Return
        // the upgrade with its socket intact so frames bypass LiveHub.
        match container
            .get_tcp_port(TUNNEL_PORT)?
            .fetch_request(req)
            .await
        {
            Ok(response) => Response::try_from(response),
            Err(_) => refuse(
                ErrorCode::Unavailable,
                "the host tunnel port is unavailable",
            ),
        }
    }
}

impl Room {
    fn new(state: State, class: HostClass) -> Self {
        // The runtime drops the inactivity timeout when the DO is evicted.
        if let Some(container) = state.container().filter(|c| c.running()) {
            let storage = state.storage();
            let _restore = state.block_concurrency_while(async move {
                match storage.get::<RoomLease>(ROOM_KEY).await? {
                    Some(RoomLease::Active { deadline_ms, .. })
                        if deadline_ms > js_sys::Date::now() as i64 =>
                    {
                        keep_until(&container, deadline_ms).await
                    }
                    _ => container.destroy(None).await,
                }
            });
        }
        Self {
            state,
            class,
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let container = self.container()?;
        let path = req.path();
        match (req.method(), path.as_str()) {
            (Method::Post, "/start") => self.start(&container, req.json::<Start>().await?).await,
            (Method::Post, "/stop") => self.stop(&container).await,
            (Method::Get, "/alive") => Response::from_json(&container.running()),
            (Method::Get, path) => match tunnel_lease(path) {
                Some(id) => self.tunnel(&container, req, id).await,
                None => Response::error("not found", 404),
            },
            _ => Response::error("not found", 404),
        }
    }

    // Backstop for a lost start response or an unavailable hub at match end.
    async fn alarm(&self) -> Result<Response> {
        self.stop(&self.container()?).await
    }
}

// Wrangler's capped applications take one instance size per exported class.
// Both namespaces use the same lease lifecycle and tunnel.
macro_rules! host_room {
    ($name:ident, $class:ident) => {
        #[durable_object(alarm)]
        pub struct $name {
            room: Room,
        }

        impl DurableObject for $name {
            fn new(state: State, _env: Env) -> Self {
                Self {
                    room: Room::new(state, HostClass::$class),
                }
            }
            async fn fetch(&self, request: Request) -> Result<Response> {
                self.room.fetch(request).await
            }
            async fn alarm(&self) -> Result<Response> {
                self.room.alarm().await
            }
        }
    };
}

host_room!(HostRoom, Small);
host_room!(LargeHostRoom, Large);
