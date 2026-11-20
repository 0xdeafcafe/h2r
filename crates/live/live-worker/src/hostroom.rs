//! Dedicated host rooms (docs/live/dedicated-host.md#packed-hosts). Only
//! LiveHub can reach these bindings. Control requests have no public route.
//!
//! Each class binding holds three kinds of room, told apart by name:
//!
//! - **Lease** (`hl-…`): the hub's handle for one lease, as before. It keeps
//!   the lease's deadline and tombstone and forwards to the lease's pack.
//! - **Pool** (`pool-<location>`): routes new leases to a pack with room,
//!   using [`Pool`], and stops a pack when its last lease ends.
//! - **Pack** (`pool-<location>/pack-<n>`): owns one container running
//!   `h2 host pack`, which admits matches from measured load.
//!
//! Calls only go lease → pool → pack, so the rooms' locks cannot deadlock.

#[path = "hostroom/lease.rs"]
mod lease;

use std::collections::BTreeMap;

use async_trait::async_trait;
use axum::response::IntoResponse;
use js_sys::futures::JsFuture;
use live_proto::host_pack::{self, AddMatch, Added, Pool};
use live_proto::hosts::{HostClass, HostLease, ProviderKind, Transport};
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
    Stopped,
}

#[derive(Serialize, Deserialize)]
struct Place {
    add: AddMatch,
    deadline_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct Placed {
    /// The pack room's name.
    pack: String,
}

#[derive(Serialize, Deserialize)]
struct Release {
    lease: String,
}

#[derive(Serialize, Deserialize)]
struct PackRemove {
    lease: String,
    /// The pool's last lease on this pack: stop the container.
    last: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct PackLease {
    deadline_ms: i64,
    tunnel_port: u16,
}

/// A pack room's leases. No credential is stored.
#[derive(Default, Serialize, Deserialize)]
struct PackLeases {
    leases: BTreeMap<String, PackLease>,
}

impl PackLeases {
    fn deadline(&self) -> Option<i64> {
        self.leases.values().map(|l| l.deadline_ms).max()
    }
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
    stub(env, binding(&view.lease.class), id, None)?
        .fetch_with_request(tunnel_request(id, req.headers().get(http::header::SEC_WEBSOCKET_PROTOCOL))?)
        .await
}

/// A fresh upgrade request carrying only the WebSocket protocol header.
fn tunnel_request(id: &str, protocol: Option<&http::HeaderValue>) -> Result<Request> {
    let headers = Headers::new();
    headers.set("upgrade", "websocket")?;
    if let Some(protocol) = protocol {
        headers.set(
            "sec-websocket-protocol",
            protocol.to_str().map_err(super::rust_error)?,
        )?;
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
            (Some(Role::Pool), Method::Post, "/pool/place") => self.place(req.json().await?).await,
            (Some(Role::Pool), Method::Post, "/pool/release") => {
                self.release(req.json().await?).await
            }
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
        let pool = match storage.get::<RoomLease>(ROOM_KEY).await? {
            Some(RoomLease::Active { pool, .. }) => pool,
            Some(RoomLease::Releasing { pool }) => Some(pool),
            Some(RoomLease::Stopped) | None => None,
        };
        // A lease container from before packing.
        let container = self.container()?;
        if container.running() {
            container.destroy(None).await?;
        }
        if let Some(pool) = pool {
            // Tombstone first, so a delayed start cannot resurrect the lease
            // while the pool is unreachable; Live's cleanup retries the stop.
            storage
                .put(ROOM_KEY, RoomLease::Releasing { pool: pool.clone() })
                .await?;
            let release = Release { lease: self.name() };
            let response = call_room(
                &self.env,
                self.namespace(),
                &pool,
                pool_hint(&pool),
                Method::Post,
                "/pool/release",
                Some(serde_json::to_string(&release)?),
            )
            .await?;
            if !ok(&response) {
                return Err(http_error("host pool release", &response));
            }
        }
        storage.put(ROOM_KEY, RoomLease::Stopped).await?;
        storage.delete_alarm().await?;
        Response::empty()
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
        if pool.is_empty() {
            storage.delete(POOL_KEY).await?;
            storage.delete_alarm().await?;
        } else {
            storage.put(POOL_KEY, pool).await?;
            if let Some(next) = pool.next_deadline() {
                storage.set_alarm(next.saturating_sub(now_ms()).max(1)).await?;
            }
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

    async fn place(&self, place: Place) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        let mut pool = self.load_pool().await?;
        self.expire(&mut pool).await;
        let lease = place.add.lease.clone();
        let body = serde_json::to_string(&place)?;
        for _ in 0..PLACE_TRIES {
            let placement = pool.place(&lease, self.max_matches);
            let pack = placement.pack().to_owned();
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
                    pool.admitted(&pack, &lease, place.deadline_ms);
                    self.save_pool(&pool).await?;
                    return Response::from_json(&Placed { pack: room });
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
        if let Some(pack) = pool.pack_of(&release.lease).map(str::to_owned) {
            let last = pool.leases(&pack) <= 1;
            let remove = PackRemove {
                lease: release.lease.clone(),
                last,
            };
            let response = self
                .call_pack(&pack, Method::Post, "/pack/remove", Some(&remove))
                .await?;
            // Keep the lease routed until its pack confirms, so a retried
            // release can still find it.
            if !ok(&response) {
                return Err(http_error("host pack remove", &response));
            }
            pool.release(&release.lease);
        }
        self.expire(&mut pool).await;
        self.save_pool(&pool).await?;
        Response::empty()
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

    async fn pack_add(&self, place: Place) -> Result<Response> {
        let _guard = self.lifecycle.lock().await;
        if place.deadline_ms <= now_ms() {
            return refuse(ErrorCode::Gone, "host lease deadline has passed");
        }
        let container = self.container()?;
        let mut pack = self.load_pack().await?;
        if !container.running() {
            // A stopped or crashed container lost every match it ran.
            pack.leases.clear();
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
                pack.leases.insert(
                    place.add.lease.clone(),
                    PackLease {
                        deadline_ms: place.deadline_ms,
                        tunnel_port: added.ports.tunnel_port,
                    },
                );
                self.save_pack(&container, &pack).await?;
                Response::from_json(&added)
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
        pack.leases.remove(&remove.lease);
        let stop = remove.last || pack.leases.is_empty();
        if container.running() {
            if stop {
                container.destroy(None).await?;
            } else if let Err(error) = self
                .control(
                    &container,
                    Method::Delete,
                    &format!("{}/{}", host_pack::MATCHES_PATH, remove.lease),
                    None,
                    false,
                )
                .await
            {
                worker::console_warn!("host pack {} could not stop {}: {error}", self.name(), remove.lease);
                return Err(error);
            }
        }
        if stop {
            pack.leases.clear();
        }
        self.save_pack(&container, &pack).await?;
        Response::empty()
    }

        let mut env = pack_env(&self.class, self.max_matches);
        if let Ok(features) = self.env.var("H2_FEATURES") {
            env.insert("H2_FEATURES", features.to_string());
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
        pack.leases.retain(|_, lease| lease.deadline_ms > now);
        if pack.leases.is_empty() && container.running() {
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
