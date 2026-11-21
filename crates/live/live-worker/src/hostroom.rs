//! Dedicated host rooms (docs/live/dedicated-host.md#packed-hosts). Only
//! LiveHub can reach these bindings. Control requests have no public route.
//!
//! Each class binding holds three kinds of room, told apart by name:
//!
//! - **Lease** (`hl-…`): the hub's handle for one lease, as before. It keeps
//!   the lease's deadline and tombstone and forwards to the lease's pack.
//! - **Pool** (`pool-<location>`): routes new leases to a pack with room,
//!   using [`Pool`], and keeps warm slots while the region has demand.
//! - **Pack** (`pool-<location>/pack-<n>`): owns one container running
//!   `h2 host pack`, which admits matches from measured load.
//!
//! Calls only go lease → pool → pack, so the rooms' locks cannot deadlock.

#[path = "hostroom/region_echo.rs"]
pub(super) mod region_echo;
#[path = "hostroom/lease.rs"]
mod lease;

#[path = "hostroom/pack_state.rs"]
mod pack_state;
use pack_state::{PackLease, PackLeases};

use async_trait::async_trait;
use axum::response::IntoResponse;
use js_sys::futures::JsFuture;
use live_proto::host_pack::{self, AddMatch, Added, Pool, WarmDemand};
use live_proto::hosts::{HostClass, HostLease, PackRelease, ProviderKind, Transport};
use live_proto::{Author, ErrorCode, lxm};
use live_server::app::{ApiError, Live};
use live_server::hosts::{HostBoot, HostError, HostProvider};
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use worker::{
    Container, DurableObject, Env, Headers, HttpRequest, Method, Request, RequestInit, Response,
    Result, State, Stub, durable_object,
};

pub(super) use lease::tunnel_lease;
use lease::{
    CONTROL_PORT, Role, add_match, admit, binding, location_hint, pack_env, pack_matches,
    pack_room, parse_pack_room, parse_ref, pool_hint, pool_name, provider_ref, role, valid_id,
};

const ROOM_KEY: &str = "lease";
const POOL_KEY: &str = "pool";
const PACK_KEY: &str = "pack";
/// How long a new container has to open its control port.
const READY_MS: i64 = 30_000;
/// New containers one placement may try before giving up.
const PLACE_TRIES: usize = 3;

#[derive(Serialize, Deserialize)]
struct Start {
    add: AddMatch,
    class: HostClass,
    deadline_ms: i64,
    #[serde(default)]
    region: Option<String>,
}

/// Only the hard deadline and the lease's pack survive eviction.
/// A stopped lease retains a tombstone so a delayed start cannot resurrect it.
#[derive(Serialize, Deserialize)]
enum RoomLease {
    Active {
        deadline_ms: i64,
        #[serde(default)]
        pool: Option<String>,
        /// The pack room, once placed.
        #[serde(default)]
        pack: Option<String>,
    },
    /// Stopped, but the pool has not yet confirmed the release.
    Releasing { pool: String },
    /// Recorded before acknowledging the pool receipt. Retried stops
    /// return the original result even after a different lease refills it.
    Settled { receipt: Option<PackRelease>, ack_pool: Option<String> },
    Stopped,
}

#[derive(Serialize, Deserialize)]
struct Place {
    add: AddMatch,
    deadline_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct FundedDemand {
    demand: WarmDemand,
    plan: host_pack::WarmPlan,
    until_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct Placed {
    /// The pack room's name.
    pack: String,
    /// The supervisor was already running when it accepted this match.
    #[serde(default)]
    warm: bool,
}

#[derive(Serialize, Deserialize)]
struct Release {
    lease: String,
}

#[derive(Serialize, Deserialize)]
struct WarmHold {
    until_ms: i64,
    allow_start: bool,
}

#[derive(Serialize, Deserialize)]
struct PackRemove {
    lease: String,
    /// The pool has retired this pack and will never route to it again.
    last: bool,
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn stub(env: &Env, namespace: &str, name: &str, hint: Option<&str>) -> Result<Stub> {
    let rooms = env.durable_object(namespace)?;
    let id = rooms.id_from_name(name)?;
    match location_hint(hint) {
        Some(hint) => id.get_stub_with_location_hint(hint),
        None => id.get_stub(),
    }
}

/// One private room-to-room request. The caller reads the status.
async fn call_room(
    env: &Env,
    namespace: &str,
    name: &str,
    hint: Option<&str>,
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<Response> {
    let mut init = RequestInit::new();
    init.with_method(method);
    if let Some(body) = body {
        let headers = Headers::new();
        headers.set("content-type", "application/json")?;
        init.with_headers(headers)
            .with_body(Some(JsValue::from_str(&body)));
    }
    let req = Request::new_with_init(&format!("https://host-room{path}"), &init)?;
    stub(env, namespace, name, hint)?
        .fetch_with_request(req)
        .await
}

/// A room from before release receipts returns an empty success response.
async fn read_release(mut response: Response) -> Result<Option<PackRelease>> {
    let body = response.text().await?;
    if body.is_empty() {
        Ok(None)
    } else {
        Ok(serde_json::from_str(&body)?)
    }
}

fn ok(response: &Response) -> bool {
    (200..300).contains(&response.status_code())
}

fn http_error(what: &str, response: &Response) -> worker::Error {
    worker::Error::RustError(format!("{what} returned HTTP {}", response.status_code()))
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
        let body = body.map(serde_json::to_string).transpose()?;
        let response = call_room(
            &self.env,
            namespace,
            lease_id,
            region,
            method,
            &format!("/{path}"),
            body,
        )
        .await?;
        if !ok(&response) {
            return Err(http_error(&format!("HostRoom {path}"), &response));
        }
        Ok(response)
    }
}

#[async_trait]
impl HostProvider for CloudflareProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cloudflare
    }

    fn reserved_ref(&self, lease: &HostLease) -> Option<String> {
        Some(provider_ref(lease))
    }

    fn log_budget_refusal(&self, message: &str) {
        worker::console_warn!("{message}");
    }

    async fn warm(
        &self,
        live: &Live,
        region: Option<&str>,
        class: &HostClass,
        demand: WarmDemand,
    ) -> std::result::Result<usize, HostError> {
        let pool = pool_name(region);
        SendWrapper::new(async {
            let mut preview = call_room(
                &self.env, binding(class), &pool, pool_hint(&pool), Method::Post,
                "/pool/preview", Some(serde_json::to_string(&demand)?),
            ).await?;
            if !ok(&preview) {
                return Err(http_error("host pool preview", &preview));
            }
            let plan = preview.json::<host_pack::WarmPlan>().await?;
            let from_ms = now_ms();
            let until_ms = from_ms.saturating_add(host_pack::LINGER_MS);
            if !live.reserve_warm_packs(&pool, class, &plan, from_ms, until_ms).await {
                return Ok(0);
            }
            let funded = FundedDemand { demand, plan, until_ms };
            let mut response = call_room(
                &self.env, binding(class), &pool, pool_hint(&pool), Method::Post,
                "/pool/demand", Some(serde_json::to_string(&funded)?),
            ).await?;
            if !ok(&response) {
                return Err(http_error("host pool demand", &response));
            }
            response.json::<usize>().await
        })
        .await
        .map_err(|e| HostError::new(e.to_string()))
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
            add: add_match(lease, boot)
                .map_err(|e| HostError::new(format!("could not encode host lease: {e}")))?,
            class: lease.class.clone(),
            deadline_ms: lease.deadline_ms(),
            region: lease.region.clone(),
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
                // A lost response can leave the match running even when Live
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
        self.stop_and_settle(lease_id, provider_ref).await.map(|_| ())
    }

    async fn stop_and_settle(
        &self, lease_id: &str, provider_ref: &str,
    ) -> std::result::Result<Option<PackRelease>, HostError> {
        let (namespace, id) = parse_ref(provider_ref)
            .filter(|(_, id)| *id == lease_id)
            .ok_or_else(|| HostError::new("HostRoom reference belongs to a different lease"))?;
        SendWrapper::new(async {
            let response = self.call(namespace, id, None, Method::Post, "stop", None).await?;
            read_release(response).await
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
    stub(env, binding(&view.lease.class), id, None)?
        .fetch_with_request(tunnel_request(id, req.headers())?)
        .await
}

/// A fresh upgrade request carrying the WebSocket protocol and actor offer.
fn tunnel_request(id: &str, incoming: &http::HeaderMap) -> Result<Request> {
    use live_proto::tunnel::{ACTOR_CHANNEL_HEADER, ACTOR_CHANNEL_OFFER};

    let headers = Headers::new();
    headers.set("upgrade", "websocket")?;
    if let Some(protocol) = incoming.get(http::header::SEC_WEBSOCKET_PROTOCOL) {
        headers.set(
            "sec-websocket-protocol",
            protocol.to_str().map_err(super::rust_error)?,
        )?;
    }
    if incoming
        .get(ACTOR_CHANNEL_HEADER)
        .is_some_and(|offer| offer == ACTOR_CHANNEL_OFFER)
    {
        headers.set(ACTOR_CHANNEL_HEADER, ACTOR_CHANNEL_OFFER)?;
    }
    let mut init = RequestInit::new();
    init.with_headers(headers);
    Request::new_with_init(&format!("http://container/v1/hosts/{id}/tunnel"), &init)
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
    let remaining = deadline_ms.saturating_sub(now_ms()).clamp(1, 21_600_000);
    let raw: &ContainerControl = container.unchecked_ref();
    JsFuture::from(raw.inactivity_timeout(remaining as f64)?).await?;
    Ok(())
}

struct Room {
    state: State,
    env: Env,
    class: HostClass,
    max_matches: usize,
    lifecycle: tokio::sync::Mutex<()>,
}

impl Room {
    fn new(state: State, env: Env, class: HostClass) -> Self {
        // The runtime drops the inactivity timeout when the DO is evicted.
        // Only pack rooms run containers; a lease room may still own one
        // started before packing.
        if let Some(container) = state.container().filter(|c| c.running()) {
            let storage = state.storage();
            let _restore = state.block_concurrency_while(async move {
                let deadline = match storage.get::<PackLeases>(PACK_KEY).await? {
                    Some(pack) => pack.deadline(),
                    None => match storage.get::<RoomLease>(ROOM_KEY).await? {
                        Some(RoomLease::Active { deadline_ms, .. }) => Some(deadline_ms),
                        _ => None,
                    },
                };
                match deadline {
                    Some(deadline_ms) if deadline_ms > now_ms() => {
                        keep_until(&container, deadline_ms).await
                    }
                    _ => container.destroy(None).await,
                }
            });
        }
        let max_matches = pack_matches(
            &class,
            env.var("LIVE_HOST_PACK_MATCHES")
                .ok()
                .map(|v| v.to_string())
                .as_deref(),
        );
        Self {
            state,
            env,
            class,
            max_matches,
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    fn name(&self) -> String {
        self.state.id().name().unwrap_or_default()
    }

    fn namespace(&self) -> &'static str {
        binding(&self.class)
    }

    fn container(&self) -> Result<Container> {
        self.state
            .container()
            .ok_or_else(|| worker::Error::RustError("HostRoom has no container binding".into()))
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let name = self.name();
        let path = req.path();
        match (role(&name), req.method(), path.as_str()) {
            (Some(Role::Lease), Method::Post, "/start") => self.start(req.json().await?).await,
            (Some(Role::Lease), Method::Post, "/stop") => self.stop().await,
            (Some(Role::Lease), Method::Get, "/alive") => self.alive().await,
            (Some(Role::Pool), Method::Post, "/pool/preview") => self.preview(req.json().await?).await,
            (Some(Role::Pool), Method::Post, "/pool/demand") => self.demand(req.json().await?).await,
            (Some(Role::Pool), Method::Get, "/pool/echo") if is_upgrade(&req)? => {
                region_echo::pool(self).await
            }
            (Some(Role::Pack), Method::Get, "/pack/echo") if is_upgrade(&req)? => {
                region_echo::pack(self).await
            }
            (Some(Role::Pool), Method::Post, "/pool/place") => self.place(req.json().await?).await,
            (Some(Role::Pool), Method::Post, "/pool/release") => {
                self.release(req.json().await?).await
            }
            (Some(Role::Pool), Method::Post, "/pool/release/ack") => {
                let release: Release = req.json().await?;
                let _guard = self.lifecycle.lock().await;
                self.state.storage().delete(&format!("released:{}", release.lease)).await?;
                Response::empty()
            }
            (Some(Role::Pack), Method::Post, "/pack/warm") => self.pack_warm(req.json().await?).await,
            (Some(Role::Pack), Method::Post, "/pack/add") => self.pack_add(req.json().await?).await,
            (Some(Role::Pack), Method::Post, "/pack/remove") => {
                self.pack_remove(req.json().await?).await
            }
            (Some(Role::Pack), Method::Get, path) if path.starts_with("/pack/alive/") => {
                self.pack_alive(&path["/pack/alive/".len()..]).await
            }
            (Some(role), Method::Get, path) => match tunnel_lease(path) {
                Some(id) if role == Role::Lease => self.tunnel(req, id).await,
                Some(id) if role == Role::Pack => self.pack_tunnel(req, id).await,
                _ => Response::error("not found", 404),
            },
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        match role(&self.name()) {
            // Backstop for a lost start response or an unavailable hub at match end.
            Some(Role::Lease) => self.stop().await,
            Some(Role::Pool) => self.pool_alarm().await,
            Some(Role::Pack) => self.pack_alarm().await,
            None => Response::empty(),
        }
    }

    // -- Lease rooms ------------------------------------------------------------

    async fn start(&self, start: Start) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        if self.class != start.class || self.name() != start.add.lease {
            return refuse(ErrorCode::Invalid, "lease does not name this HostRoom");
        }
        if start.deadline_ms <= now_ms() {
            return refuse(ErrorCode::Gone, "host lease deadline has passed");
        }
        let storage = self.state.storage();
        match storage.get::<RoomLease>(ROOM_KEY).await? {
            None => {}
            Some(RoomLease::Active { pack: Some(_), .. }) => return Response::empty(),
            // An earlier start failed before its pack answered. Placement is
            // idempotent per lease, so try again.
            Some(RoomLease::Active { pack: None, .. }) => {}
            Some(_) => return refuse(ErrorCode::Gone, "this host lease has already ended"),
        }
        let pool = pool_name(start.region.as_deref());
        // Arm cleanup before asking for billable compute. No token or bundle
        // URL is kept in storage. DO storage operations hold the input gate.
        storage
            .put(
                ROOM_KEY,
                RoomLease::Active {
                    deadline_ms: start.deadline_ms,
                    pool: Some(pool.clone()),
                    pack: None,
                },
            )
            .await?;
        storage
            .set_alarm(start.deadline_ms.saturating_sub(now_ms()).max(1))
            .await?;
        let place = Place {
            add: start.add,
            deadline_ms: start.deadline_ms,
        };
        let mut response = call_room(
            &self.env,
            self.namespace(),
            &pool,
            pool_hint(&pool),
            Method::Post,
            "/pool/place",
            Some(serde_json::to_string(&place)?),
        )
        .await?;
        if !ok(&response) {
            return Err(http_error("host pool place", &response));
        }
        let placed: Placed = response.json().await?;
        worker::console_log!("host pool placed {} in {} (warm={})", self.name(), placed.pack, placed.warm);
        storage
            .put(
                ROOM_KEY,
                RoomLease::Active {
                    deadline_ms: start.deadline_ms,
                    pool: Some(pool),
                    pack: Some(placed.pack),
                },
            )
            .await?;
        Response::empty()
    }

    async fn stop(&self) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let storage = self.state.storage();
        let (pool, mut receipt) = match storage.get::<RoomLease>(ROOM_KEY).await? {
            Some(RoomLease::Active { pool, .. }) => (pool, Some(PackRelease::NoLinger)),
            Some(RoomLease::Releasing { pool }) => (Some(pool), None),
            Some(RoomLease::Settled { receipt, ack_pool }) => {
                return self.finish_stop(receipt, ack_pool).await;
            }
            // A historical tombstone cannot recover the original release.
            Some(RoomLease::Stopped) => (None, None),
            // Stopping before placement allocates no slot.
            None => (None, Some(PackRelease::NoLinger)),
        };
        // A lease container from before packing.
        let container = self.container()?;
        if container.running() {
            container.destroy(None).await?;
        }
        if let Some(pool) = pool.as_deref() {
            storage.put(ROOM_KEY, RoomLease::Releasing { pool: pool.to_owned() }).await?;
            let release = Release { lease: self.name() };
            let response = call_room(
                &self.env, self.namespace(), pool, pool_hint(pool), Method::Post,
                "/pool/release", Some(serde_json::to_string(&release)?),
            ).await?;
            if !ok(&response) {
                return Err(http_error("host pool release", &response));
            }
            receipt = read_release(response).await?;
        }
        let ack_pool = pool.filter(|_| matches!(receipt, Some(PackRelease::Linger { .. })));
        storage.put(ROOM_KEY, RoomLease::Settled { receipt, ack_pool: ack_pool.clone() }).await?;
        self.finish_stop(receipt, ack_pool).await
    }

    async fn finish_stop(&self, receipt: Option<PackRelease>, ack_pool: Option<String>) -> Result<Response> {
        let storage = self.state.storage();
        if let Some(pool) = ack_pool {
            let release = Release { lease: self.name() };
            let response = call_room(
                &self.env, self.namespace(), &pool, pool_hint(&pool), Method::Post,
                "/pool/release/ack", Some(serde_json::to_string(&release)?),
            ).await?;
            if !ok(&response) {
                return Err(http_error("host pool release ack", &response));
            }
            storage.put(ROOM_KEY, RoomLease::Settled { receipt, ack_pool: None }).await?;
        }
        storage.delete_alarm().await?;
        Response::from_json(&receipt)
    }

    async fn alive(&self) -> Result<Response> {
        let alive = match self.state.storage().get::<RoomLease>(ROOM_KEY).await? {
            Some(RoomLease::Active {
                pack: Some(pack), ..
            }) => {
                let hint = parse_pack_room(&pack).and_then(|(pool, _)| pool_hint(pool));
                let mut response = call_room(
                    &self.env,
                    self.namespace(),
                    &pack,
                    hint,
                    Method::Get,
                    &format!("/pack/alive/{}", self.name()),
                    None,
                )
                .await?;
                if !ok(&response) {
                    return Err(http_error("host pack alive", &response));
                }
                response.json::<bool>().await?
            }
            Some(RoomLease::Active { pack: None, .. }) => self.container()?.running(),
            _ => false,
        };
        Response::from_json(&alive)
    }

    async fn tunnel(&self, req: Request, id: &str) -> Result<Response> {
        if self.name() != id || !is_upgrade(&req)? {
            return refuse(ErrorCode::Invalid, "invalid host tunnel upgrade");
        }
        let Some(RoomLease::Active {
            deadline_ms, pack, ..
        }) = self.state.storage().get(ROOM_KEY).await?
        else {
            return refuse(ErrorCode::NotFound, "no active host lease");
        };
        if deadline_ms <= now_ms() {
            return refuse(ErrorCode::Gone, "the host is stopped");
        }
        let Some(pack) = pack else {
            // A lease container started before packing.
            let container = self.container()?;
            if !container.running() {
                return refuse(ErrorCode::Unavailable, "the host is still starting");
            }
            return forward_tunnel(&container, lease::TUNNEL_PORT, req).await;
        };
        // The pack returns the container's upgrade with its socket intact,
        // so frames bypass this room and LiveHub.
        let hint = parse_pack_room(&pack).and_then(|(pool, _)| pool_hint(pool));
        stub(&self.env, self.namespace(), &pack, hint)?
            .fetch_with_request(req)
            .await
    }

    // -- Pool rooms ---------------------------------------------------------------

    async fn load_pool(&self) -> Result<Pool> {
        Ok(self
            .state
            .storage()
            .get::<Pool>(POOL_KEY)
            .await?
            .unwrap_or_default())
    }

    async fn save_pool(&self, pool: &Pool) -> Result<()> {
        let storage = self.state.storage();
        // Keep the serial and spent warm grants when the last pack exits.
        // A delayed request cannot reuse a name or replenish warm capacity.
        storage.put(POOL_KEY, pool).await?;
        self.schedule_pool_alarm(pool).await
    }

    async fn schedule_pool_alarm(&self, pool: &Pool) -> Result<()> {
        let storage = self.state.storage();
        if let Some(next) = pool.next_deadline() {
            storage.set_alarm(next.saturating_sub(now_ms()).max(1)).await?;
        } else {
            storage.delete_alarm().await?;
        }
        Ok(())
    }

    async fn call_pack<T: Serialize>(
        &self,
        pack: &str,
        method: Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<Response> {
        let room = pack_room(&self.name(), pack);
        let body = body.map(serde_json::to_string).transpose()?;
        call_room(
            &self.env,
            self.namespace(),
            &room,
            pool_hint(&self.name()),
            method,
            path,
            body,
        )
        .await
    }

    /// Stop a pack the pool no longer routes to. Its own alarm is the backstop.
    async fn stop_pack(&self, pack: &str, lease: &str) {
        let remove = PackRemove {
            lease: lease.to_owned(),
            last: true,
        };
        match self
            .call_pack(pack, Method::Post, "/pack/remove", Some(&remove))
            .await
        {
            Ok(response) if ok(&response) => {}
            _ => worker::console_warn!("host pool {} could not stop {pack}", self.name()),
        }
    }

    async fn expire(&self, pool: &mut Pool) {
        for pack in pool.expire(now_ms()) {
            self.stop_pack(&pack, "").await;
        }
    }

    /// Quote the current plan without allocating a name or starting compute.
    async fn preview(&self, demand: WarmDemand) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        let now = now_ms();
        pool.expire(now);
        Response::from_json(&pool.warm_plan(demand, self.max_matches, now))
    }

    async fn demand(&self, grant: FundedDemand) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        let now = now_ms();
        let Some(until) = pack_state::grant_deadline(now, grant.until_ms) else {
            return refuse(ErrorCode::Gone, "warm capacity grant expired");
        };
        let expired = pool.next_deadline().is_some_and(|at| at <= now);
        if expired {
            self.expire(&mut pool).await;
        }
        let plan = pool.warm_plan(grant.demand, self.max_matches, now);
        // Other searches or placements can advance the pool between the quote
        // and the grant. Only apply the intersection, including startup intent.
        let plan = pack_state::funded(plan, &grant.plan);
        let reserved = pool.reserve_warm(&plan, until);
        if expired || reserved {
            // Persist the attempt before an eviction or a lost container reply.
            self.save_pool(&pool).await?;
        }
        if !reserved {
            return Response::from_json(&pool.free_slots(self.max_matches));
        }
        let refresh = plan.refresh.into_iter().map(|name| (name, false));
        for (name, allow_start) in refresh.chain(plan.start.map(|name| (name, true))) {
            let hold = WarmHold { until_ms: until, allow_start };
            let mut response = self
                .call_pack(&name, Method::Post, "/pack/warm", Some(&hold))
                .await?;
            if !allow_start && response.status_code() == 404 {
                // Renewals do not buy a second startup. The next quote names
                // a fresh pack, whose first hold must be funded separately.
                pool.forget(&name);
                self.save_pool(&pool).await?;
                continue;
            }
            if !ok(&response) {
                return Err(http_error("host pack warm", &response));
            }
            let cold_ms = response.json::<Option<u64>>().await?;
            pool.warmed(&name, until.saturating_sub(host_pack::LINGER_MS), cold_ms);
            self.save_pool(&pool).await?;
        }
        Response::from_json(&pool.free_slots(self.max_matches))
    }

    async fn place(&self, place: Place) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        self.expire(&mut pool).await;
        let lease = place.add.lease.clone();
        let body = serde_json::to_string(&place)?;
        for _ in 0..PLACE_TRIES {
            let placement = pool.place(&lease, self.max_matches);
            let pack = placement.pack().to_owned();
            if matches!(&placement, host_pack::Placement::New(_)) {
                self.save_pool(&pool).await?;
            }
            let room = pack_room(&self.name(), &pack);
            let response = call_room(
                &self.env,
                self.namespace(),
                &room,
                pool_hint(&self.name()),
                Method::Post,
                "/pack/add",
                Some(body.clone()),
            )
            .await;
            match response {
                Ok(response) if ok(&response) => {
                    let cold_ms = response.headers().get("x-h2-pack-cold-ms")?
                        .and_then(|value| value.parse::<u64>().ok());
                    if let Some(ms) = cold_ms {
                        pool.warmed(&pack, now_ms(), Some(ms));
                    }
                    pool.admitted_at(&pack, &lease, place.deadline_ms, now_ms());
                    self.save_pool(&pool).await?;
                    return Response::from_json(&Placed { pack: room, warm: cold_ms.is_none() });
                }
                // The pack's measured load has no room for another match.
                Ok(response) if response.status_code() == 409 => {
                    if pool.refused(&pack) {
                        self.stop_pack(&pack, &lease).await;
                    }
                }
                failed => {
                    if pool.refused(&pack) {
                        self.stop_pack(&pack, &lease).await;
                    }
                    self.save_pool(&pool).await?;
                    return match failed {
                        Ok(response) => Err(http_error("host pack add", &response)),
                        Err(error) => Err(error),
                    };
                }
            }
        }
        self.save_pool(&pool).await?;
        refuse(ErrorCode::Unavailable, "no host pack has room for the match")
    }

    async fn release(&self, release: Release) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        let storage = self.state.storage();
        let key = format!("released:{}", release.lease);
        let Some(pack) = pool.pack_of(&release.lease).map(str::to_owned) else {
            // A lost reply or lease-room eviction can replay the release.
            // Only the lease that emptied its pack has a pending receipt.
            let receipt = storage.get::<PackRelease>(&key).await?.unwrap_or(PackRelease::NoLinger);
            self.schedule_pool_alarm(&pool).await?;
            return Response::from_json(&receipt);
        };
        let remove = PackRemove { lease: release.lease.clone(), last: false };
        let response = self.call_pack(&pack, Method::Post, "/pack/remove", Some(&remove)).await?;
        if !ok(&response) {
            return Err(http_error("host pack remove", &response));
        }
        let receipt = pool.release_at(&release.lease, now_ms());
        if matches!(receipt, PackRelease::Linger { .. }) {
            // Publish the last-slot decision and removal in one transaction.
            // The receipt is forgotten only after its lease has stored it.
            let snapshot = pool.clone();
            storage.transaction(move |tx| async move {
                tx.put(&key, receipt).await?;
                tx.put(POOL_KEY, &snapshot).await
            }).await?;
            self.schedule_pool_alarm(&pool).await?;
        } else {
            self.save_pool(&pool).await?;
        }
        Response::from_json(&receipt)
    }

    async fn pool_alarm(&self) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        self.expire(&mut pool).await;
        self.save_pool(&pool).await?;
        Response::empty()
    }

    // -- Pack rooms ---------------------------------------------------------------

    async fn load_pack(&self) -> Result<PackLeases> {
        Ok(self
            .state
            .storage()
            .get::<PackLeases>(PACK_KEY)
            .await?
            .unwrap_or_default())
    }

    async fn save_pack(&self, container: &Container, pack: &PackLeases) -> Result<()> {
        let storage = self.state.storage();
        match pack.deadline() {
            Some(deadline) => {
                storage.put(PACK_KEY, pack).await?;
                storage
                    .set_alarm(deadline.saturating_sub(now_ms()).max(1))
                    .await?;
                if container.running() {
                    keep_until(container, deadline).await?;
                }
            }
            None => {
                storage.delete(PACK_KEY).await?;
                storage.delete_alarm().await?;
            }
        }
        Ok(())
    }

    /// One request to the container's `h2 host pack` control port. Waits
    /// for a container that is still opening the port.
    async fn control(
        &self,
        container: &Container,
        method: Method,
        path: &str,
        body: Option<&str>,
        wait: bool,
    ) -> Result<(u16, String)> {
        let deadline = now_ms() + if wait { READY_MS } else { 0 };
        loop {
            let attempt = async {
                let mut init = RequestInit::new();
                init.with_method(method.clone());
                if let Some(body) = body {
                    let headers = Headers::new();
                    headers.set("content-type", "application/json")?;
                    init.with_headers(headers)
                        .with_body(Some(JsValue::from_str(body)));
                }
                let response = container
                    .get_tcp_port(CONTROL_PORT)?
                    .fetch(format!("http://pack{path}"), Some(init))
                    .await?;
                let mut response = Response::try_from(response)?;
                Ok::<_, worker::Error>((response.status_code(), response.text().await?))
            }
            .await;
            match attempt {
                Ok(answer) => return Ok(answer),
                Err(_) if now_ms() < deadline && container.running() => {
                    worker::Delay::from(std::time::Duration::from_millis(500)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn start_container(&self, container: &Container) -> Result<()> {
        // The image entrypoint execs `h2 host pack`, which starts empty.
        let mut env = pack_env(&self.class, self.max_matches);
        if let Ok(features) = self.env.var("H2_FEATURES") {
            env.insert("H2_FEATURES", features.to_string());
        }
        let options = serde_json::json!({
            "entrypoint": ["/opt/h2/entrypoint.sh"],
            "enableInternet": true,
            "env": env,
        });
        let options = js_sys::JSON::parse(&serde_json::to_string(&options)?)?;
        let raw: &ContainerControl = container.unchecked_ref();
        raw.start_with_options(&options)
            .map_err(|_| worker::Error::RustError("container start failed".into()))
    }

    async fn pack_warm(&self, grant: WarmHold) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let container = self.container()?;
        let now = now_ms();
        let Some(until) = pack_state::grant_deadline(now, grant.until_ms) else {
            return refuse(ErrorCode::Gone, "warm capacity grant expired");
        };
        let cold = !container.running();
        if cold && !grant.allow_start {
            return Response::error("warm pack has exited", 404);
        }
        let mut pack = if cold {
            PackLeases::default()
        } else {
            self.load_pack().await?
        };
        pack.expire(now);
        // A lost response, a failed start, or eviction during startup cannot
        // leave a billable container without its own cleanup deadline.
        if cold {
            pack.hold_until(until);
            self.save_pack(&container, &pack).await?;
            self.start_container(&container)?;
            keep_until(&container, until).await?;
        }
        let (status, _) = self
            .control(&container, Method::Post, host_pack::WARM_PATH, None, true)
            .await?;
        if status != 204 {
            return Err(worker::Error::RustError(format!("host pack warm returned HTTP {status}")));
        }
        let ready = now_ms();
        if ready >= until {
            if pack.leases.is_empty() {
                container.destroy(None).await?;
                self.save_pack(&container, &PackLeases::default()).await?;
            }
            return refuse(ErrorCode::Gone, "warm capacity grant expired before the pack was ready");
        }
        pack.hold_until(until);
        self.save_pack(&container, &pack).await?;
        Response::from_json(&cold.then(|| ready.saturating_sub(now).max(0) as u64))
    }

    async fn pack_add(&self, place: Place) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        if place.deadline_ms <= now_ms() {
            return refuse(ErrorCode::Gone, "host lease deadline has passed");
        }
        let container = self.container()?;
        let mut pack = self.load_pack().await?;
        let cold_since = (!container.running()).then(now_ms);
        if cold_since.is_some() {
            // A stopped or crashed container lost every match it ran.
            pack = PackLeases::default();
            // Arm cleanup before starting billable compute.
            self.state
                .storage()
                .set_alarm(place.deadline_ms.saturating_sub(now_ms()).max(1))
                .await?;
            self.start_container(&container)?;
        }
        let body = serde_json::to_string(&place.add)?;
        let (status, answer) = match self
            .control(
                &container,
                Method::Post,
                host_pack::MATCHES_PATH,
                Some(&body),
                true,
            )
            .await
        {
            Ok(answer) => answer,
            Err(error) => {
                if pack.leases.is_empty() {
                    container.destroy(None).await?;
                }
                self.save_pack(&container, &pack).await?;
                return Err(error);
            }
        };
        match status {
            200 => {
                let added: Added = serde_json::from_str(&answer)?;
                pack.admitted(
                    place.add.lease.clone(),
                    PackLease {
                        deadline_ms: place.deadline_ms,
                        tunnel_port: added.ports.tunnel_port,
                    },
                );
                self.save_pack(&container, &pack).await?;
                let mut response = Response::from_json(&added)?;
                if let Some(started) = cold_since {
                    let elapsed = now_ms().saturating_sub(started).max(0);
                    response.headers_mut().set("x-h2-pack-cold-ms", &elapsed.to_string())?;
                }
                Ok(response)
            }
            409 => {
                worker::console_log!("host pack {} refused {}: {answer}", self.name(), place.add.lease);
                self.save_pack(&container, &pack).await?;
                Ok(Response::from_bytes(answer.into_bytes())?.with_status(409))
            }
            _ => {
                self.save_pack(&container, &pack).await?;
                Err(worker::Error::RustError(format!(
                    "host pack control returned HTTP {status}"
                )))
            }
        }
    }

    async fn pack_remove(&self, remove: PackRemove) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let container = self.container()?;
        let mut pack = self.load_pack().await?;
        let stop = remove.last;
        if container.running() {
            if stop {
                container.destroy(None).await?;
            } else {
                let (status, _) = self.control(
                    &container,
                    Method::Delete,
                    &format!("{}/{}", host_pack::MATCHES_PATH, remove.lease),
                    None,
                    false,
                ).await?;
                if !matches!(status, 200 | 204 | 404) {
                    return Err(worker::Error::RustError(format!("host pack remove returned HTTP {status}")));
                }
            }
        }
        if stop || !container.running() {
            pack = PackLeases::default();
        } else {
            pack.release(&remove.lease, now_ms());
        }
        self.save_pack(&container, &pack).await?;
        Response::empty()
    }

    async fn pack_alive(&self, lease: &str) -> Result<Response> {
        let container = self.container()?;
        let pack = self.load_pack().await?;
        if !valid_id(lease) || !pack.leases.contains_key(lease) || !container.running() {
            return Response::from_json(&false);
        }
        let path = format!("{}/{lease}", host_pack::MATCHES_PATH);
        let alive = match self.control(&container, Method::Get, &path, None, false).await {
            Ok((200, body)) => json_field::<host_pack::MatchStatus>(&body).is_some_and(|m| m.alive),
            Ok(_) => false,
            Err(error) => return Err(error),
        };
        Response::from_json(&alive)
    }

    async fn pack_tunnel(&self, req: Request, id: &str) -> Result<Response> {
        if !is_upgrade(&req)? {
            return refuse(ErrorCode::Invalid, "invalid host tunnel upgrade");
        }
        let container = self.container()?;
        let pack = self.load_pack().await?;
        let Some(lease) = pack.leases.get(id) else {
            return refuse(ErrorCode::NotFound, "no active host lease");
        };
        if lease.deadline_ms <= now_ms() || !container.running() {
            return refuse(ErrorCode::Gone, "the host is stopped");
        }
        forward_tunnel(&container, lease.tunnel_port, req).await
    }

    async fn pack_alarm(&self) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let container = self.container()?;
        let mut pack = self.load_pack().await?;
        let now = now_ms();
        pack.expire(now);
        if pack.deadline().is_none() && container.running() {
            container.destroy(None).await?;
        }
        self.save_pack(&container, &pack).await?;
        Response::empty()
    }
}

/// The fetcher terminates HTTP on the match's private tunnel port. Return
/// the upgrade with its socket intact so frames bypass the rooms and LiveHub.
async fn forward_tunnel(container: &Container, port: u16, req: Request) -> Result<Response> {
    match container.get_tcp_port(port)?.fetch_request(req).await {
        Ok(response) => Response::try_from(response),
        Err(_) => refuse(
            ErrorCode::Unavailable,
            "the host tunnel port is unavailable",
        ),
    }
}

fn is_upgrade(req: &Request) -> Result<bool> {
    Ok(req
        .headers()
        .get("upgrade")?
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket")))
}

fn json_field<T: DeserializeOwned>(body: &str) -> Option<T> {
    serde_json::from_str(body).ok()
}

// Wrangler's capped applications take one instance size per exported class.
// Both namespaces use the same rooms and tunnel.
macro_rules! host_room {
    ($name:ident, $class:ident) => {
        #[durable_object(alarm)]
        pub struct $name {
            room: Room,
        }

        impl DurableObject for $name {
            fn new(state: State, env: Env) -> Self {
                Self {
                    room: Room::new(state, env, HostClass::$class),
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
