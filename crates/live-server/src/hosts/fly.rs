//! The Fly Machines provider: one lease is one Machine, created when
//! matchmaking forms the lobby and destroyed with the match
//! (docs/live/dedicated-host.md, "Fly Machines provider").
//!
//! It speaks the Machines API (`POST`/`DELETE`/`GET
//! /v1/apps/{app}/machines`) through [`FlyHttp`], a one-method seam
//! defined here because the crate had no outbound-HTTP trait that builds
//! in both homes: the native server's adapter is reqwest
//! ([`ReqwestFly`], the way `crate::httpstore` talks to another Live
//! server), and the Worker passes its own adapter over the platform
//! fetch (the way `crates/live-worker/src/r2.rs` adapts the R2 binding
//! for the blob store). `FlyProvider` itself — the request builders,
//! the machine shape, the error mapping — is plain code that compiles
//! for wasm32.
//!
//! UDP on Fly needs a dedicated IPv4 on the app and the game listening
//! on `fly-global-services`, so that address rides in the machine env
//! as `H2_XNET_BIND` and the machine claims a 7-port window
//! (`base..base+6`) picked deterministically from the lease id, aligned
//! so two concurrent hosts either use disjoint windows or the second
//! create is refused by Fly and mapped to an error the lifecycle can
//! fall back from.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use live_proto::hosts::{HostClass, HostLease, ProviderKind, Transport};

use super::{HostBoot, HostError, HostProvider};

// `HostError` is dh-provider's (crates/live-server/src/hosts.rs), with
// the `Config(String)` and `Http(String)` shapes confirmed. Every
// construction goes through these two fns, one per shape.
fn config_error(text: impl Into<String>) -> HostError {
    HostError::Config(text.into())
}

fn call_error(text: impl Into<String>) -> HostError {
    HostError::Http(text.into())
}

/// The Fly API, `https://api.fly.io`.
const API: &str = "https://api.fly.io";
/// The default region when a lease carries none ([`FlyConfig::region`]).
const DEFAULT_REGION: &str = "iad";
/// The default UDP window range a lease's 7 ports are picked from.
const DEFAULT_PORTS: (u16, u16) = (30_000, 44_000);
/// The number of game ports every host claims: the game's channels
/// (+0/+1/+5/+6) and QoS (+2), base through base+6.
pub const GAME_PORTS: u16 = 7;
/// What a UDP host must bind on Fly to receive traffic on the app's
/// dedicated IPv4 (docs.fly.io's "UDP and TCP").
const GLOBAL_BIND: &str = "fly-global-services";

/// One call to the Machines API, as the two homes make it.
#[derive(Debug, Clone)]
pub struct FlyRequest {
    /// `GET`, `POST` or `DELETE`.
    pub method: FlyMethod,
    /// Absolute: `{api}/v1/apps/{app}/machines[...]`.
    pub url: String,
    /// A JSON body, with `content-type: application/json`, when sent.
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyMethod {
    Get,
    Post,
    Delete,
}

impl FlyMethod {
    fn as_str(self) -> &'static str {
        match self {
            FlyMethod::Get => "GET",
            FlyMethod::Post => "POST",
            FlyMethod::Delete => "DELETE",
        }
    }
}

impl std::fmt::Display for FlyMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The answer to a [`FlyRequest`].
#[derive(Debug, Clone)]
pub struct FlyResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Outbound HTTPS to the Fly API: reqwest on a native server, the
/// platform fetch in a Worker. `call` adds the `Authorization: Bearer`
/// header and the JSON content type when a body is sent.
///
/// `Send + Sync` with `Send` futures, like [`HostProvider`] and the
/// store traits: a Worker implements it by wrapping its one-thread
/// platform future in `SendWrapper`, as `r2.rs` does.
#[async_trait]
pub trait FlyHttp: Send + Sync + 'static {
    async fn call(&self, req: &FlyRequest, token: &str) -> Result<FlyResponse, String>;
}

/// The native [`FlyHttp`] over reqwest, TLS on rustls with ring like
/// the rest of the server ([`crate::httpstore`], [`crate::blobs`]).
#[cfg(feature = "native")]
pub struct ReqwestFly {
    client: reqwest::Client,
}

#[cfg(feature = "native")]
impl ReqwestFly {
    pub fn new() -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(ReqwestFly { client })
    }
}

#[cfg(feature = "native")]
#[async_trait]
impl FlyHttp for ReqwestFly {
    async fn call(&self, req: &FlyRequest, token: &str) -> Result<FlyResponse, String> {
        let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes())
            .map_err(|e| e.to_string())?;
        let sent = self
            .client
            .request(method, &req.url)
            .bearer_auth(token);
        let sent = match &req.body {
            Some(body) => sent.header("content-type", "application/json").body(body.clone()),
            None => sent,
        };
        let resp = sent.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        Ok(FlyResponse {
            status,
            body: body.to_vec(),
        })
    }
}

/// The provider's settings, from `LIVE_FLY_*` ([`FlyConfig::from_env`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlyConfig {
    /// The Fly API token (`Authorization: Bearer`), $LIVE_FLY_TOKEN.
    pub token: String,
    /// The app the machines live in, $LIVE_FLY_APP.
    pub app: String,
    /// The host image each machine runs, $LIVE_FLY_IMAGE
    /// (deploy/host's `linux/amd64` image).
    pub image: String,
    /// Where a lease without `region` runs; $LIVE_FLY_REGION.
    pub region: String,
    /// The UDP range (`lo..=hi`) lease windows are picked from;
    /// $LIVE_FLY_PORTS as `30000-44000`.
    pub ports: (u16, u16),
    /// The API base, $LIVE_FLY_API — overridable so the provider can be
    /// pointed at a fake in a test.
    pub api: String,
}

impl FlyConfig {
    /// Reads `LIVE_FLY_TOKEN`, `LIVE_FLY_APP` and `LIVE_FLY_IMAGE`
    /// (all required) and the optional `LIVE_FLY_REGION`
    /// (default `iad`), `LIVE_FLY_PORTS` (default `30000-44000`) and
    /// `LIVE_FLY_API` (default `https://api.fly.io`).
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, HostError> {
        let need = |name: &str| {
            env(name).filter(|v| !v.trim().is_empty()).ok_or_else(|| {
                config_error(format!("the Fly provider needs ${name} (LIVE_FLY_TOKEN, LIVE_FLY_APP and LIVE_FLY_IMAGE)"))
            })
        };
        let ports = match env("LIVE_FLY_PORTS") {
            Some(text) => parse_ports(&text)?,
            None => DEFAULT_PORTS,
        };
        let config = FlyConfig {
            token: need("LIVE_FLY_TOKEN")?,
            app: need("LIVE_FLY_APP")?,
            image: need("LIVE_FLY_IMAGE")?,
            region: env("LIVE_FLY_REGION").filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REGION.to_string()),
            ports,
            api: env("LIVE_FLY_API").filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| API.to_string()),
        };
        config.check()?;
        Ok(config)
    }

    /// Every machine call goes through a checked config: the three
    /// required settings present, the app a plausible Fly name and the
    /// port range holding at least one whole window.
    fn check(&self) -> Result<(), HostError> {
        for (name, value) in [("token", &self.token), ("app", &self.app), ("image", &self.image)] {
            if value.trim().is_empty() {
                return Err(config_error(format!("the Fly provider has no {name} set")));
            }
        }
        if !is_fly_name(&self.app) {
            return Err(config_error(format!(
                "{:?} is not a Fly app name (lowercase letters, digits and dashes)",
                self.app
            )));
        }
        windows(self.ports).map_err(|_| {
            config_error(format!(
                "the port range {}-{} holds no whole {}-port window",
                self.ports.0, self.ports.1, GAME_PORTS
            ))
        })?;
        Ok(())
    }

    fn machines_url(&self, machine: Option<&str>) -> Result<String, HostError> {
        self.check()?;
        let id = match machine {
            Some(id) => format!("/{}", machine_id(id)?),
            None => String::new(),
        };
        Ok(format!("{}/v1/apps/{}/machines{}", self.api, self.app, id))
    }
}

/// `30000-44000` (or `30000`) as a `(lo, hi)` range.
fn parse_ports(text: &str) -> Result<(u16, u16), HostError> {
    let text = text.trim();
    let (lo, hi) = match text.split_once('-') {
        Some((lo, hi)) => (lo, hi),
        None => (text, text),
    };
    let (lo, hi) = (lo.trim(), hi.trim());
    let ports = (
        lo.parse::<u16>().map_err(|_| config_error(format!("LIVE_FLY_PORTS: {lo:?} is not a port")))?,
        hi.parse::<u16>().map_err(|_| config_error(format!("LIVE_FLY_PORTS: {hi:?} is not a port")))?,
    );
    windows(ports).map_err(|_| {
        config_error(format!(
            "LIVE_FLY_PORTS: {}-{} holds no whole {}-port window",
            ports.0, ports.1, GAME_PORTS
        ))
    })?;
    Ok(ports)
}

/// How many aligned `base..base+6` windows fit in `lo..=hi`.
fn windows((lo, hi): (u16, u16)) -> Result<u32, HostError> {
    let (lo, hi) = (lo as u32, hi as u32);
    if hi < lo || hi - lo + 1 < GAME_PORTS as u32 {
        return Err(config_error(format!(
            "the UDP range {lo}-{hi} is empty or shorter than {GAME_PORTS} ports"
        )));
    }
    Ok((hi - lo + 1 - GAME_PORTS as u32) / GAME_PORTS as u32 + 1)
}

/// FNV-1a, so a lease's window is picked by a hash that never moves:
/// the same lease always restarts on the same ports.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The UDP window `base..base+6` this lease claims: aligned, so two
/// leases either take disjoint windows or the same one (and Fly refuses
/// the second create — an error the lifecycle can fall back from), and
/// deterministic from the lease id.
pub fn port_base(lease_id: &str, ports: (u16, u16)) -> Result<u16, HostError> {
    let slots = windows(ports)? as u64;
    let offset = GAME_PORTS as u64 * (fnv1a(lease_id) % slots);
    Ok((ports.0 as u32 + offset as u32) as u16)
}

/// The machine's guest by lease class (dedicated-host.md's sizing):
/// Small is `performance-1x` 2 GiB, Large is `performance-2x` 4 GiB.
pub fn guest(class: &HostClass) -> MachineGuest {
    let (cpus, memory_mb) = match class {
        HostClass::Small => (1, 2048),
        HostClass::Large => (2, 4096),
    };
    MachineGuest {
        cpu_kind: "performance".to_string(),
        cpus,
        memory_mb,
    }
}

/// One machine service per game port: Fly maps one external port to one
/// internal port per service, and the game needs them to match.
pub fn services(base: u16) -> Vec<MachineService> {
    (0..GAME_PORTS)
        .map(|offset| {
            let port = base + offset;
            MachineService {
                protocol: "udp".to_string(),
                internal_port: port,
                ports: vec![MachinePort { port }],
            }
        })
        .collect()
}

/// The env the host process boots with: the lease it serves, where it
/// registers and with what credential, its game data, and the address
/// it must bind for Fly's UDP to reach it.
pub fn machine_env(lease: &HostLease, boot: &HostBoot) -> Result<BTreeMap<String, String>, HostError> {
    let lease_json = serde_json::to_string(lease)
        .map_err(|_| config_error("could not encode Fly host lease"))?;
    Ok(BTreeMap::from([
        ("H2_LEASE".to_string(), lease.id.clone()),
        ("H2_HOST_LEASE_JSON".to_string(), lease_json),
        ("H2_LIVE".to_string(), boot.live_url.clone()),
        ("H2_TOKEN".to_string(), boot.token.clone()),
        ("H2_DATA_URL".to_string(), boot.data_url.clone()),
        ("H2_DATA_SHA".to_string(), boot.data_sha256.clone()),
        ("H2_XNET_BIND".to_string(), GLOBAL_BIND.to_string()),
    ]))
}

/// `POST /v1/apps/{app}/machines` for this lease: region from the lease
/// (else the configured default), the class's guest, the lease env,
/// `auto_destroy` (a host that ends by itself removes its machine) and
/// `restart: no` (a crashed host is not restarted; the lifecycle reads
/// the failure and falls back). A UDP lease claims its port window as
/// services; a Tunnel lease needs none — its datagrams arrive through
/// the Live tunnel instead.
pub fn create_request(
    lease: &HostLease,
    boot: &HostBoot,
    fly: &FlyConfig,
) -> Result<CreateMachine, HostError> {
    let base = port_base(&lease.id, fly.ports)?;
    let region = lease
        .region
        .clone()
        .unwrap_or_else(|| fly.region.clone());
    Ok(CreateMachine {
        region,
        config: MachineConfig {
            image: fly.image.clone(),
            guest: guest(&lease.class),
            env: machine_env(lease, boot)?,
            auto_destroy: true,
            restart: MachineRestart {
                policy: "no".to_string(),
            },
            services: match lease.transport {
                Transport::Udp => services(base),
                Transport::Tunnel => Vec::new(),
            },
            metadata: BTreeMap::from([("h2-lease".to_string(), lease.id.clone())]),
        },
    })
}

/// The create-machine body. `region` sits at the top level, next to
/// `config`, as the Machines API takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateMachine {
    pub region: String,
    pub config: MachineConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConfig {
    pub image: String,
    pub guest: MachineGuest,
    pub env: BTreeMap<String, String>,
    pub auto_destroy: bool,
    pub restart: MachineRestart,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<MachineService>,
    /// Free-form, on the machine in `fly machines status`: which lease
    /// it serves.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineGuest {
    pub cpu_kind: String,
    pub cpus: u32,
    pub memory_mb: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRestart {
    pub policy: String,
}

/// One UDP port of the game's window: external and internal match, with
/// no proxy handlers — on a dedicated IPv4 the datagrams come straight
/// to the machine, which is why the host binds `fly-global-services`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineService {
    pub protocol: String,
    pub internal_port: u16,
    pub ports: Vec<MachinePort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachinePort {
    pub port: u16,
}

/// The machine the API answers with; only `id` and `state` are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub state: Option<String>,
}

/// A machine state read as alive (it is booting or running) or not.
/// `None` is a state this provider doesn't know: surfaced as an error
/// rather than guessed at, so a future Fly state never silently reads
/// as dead.
pub fn alive_state(state: Option<&str>) -> Option<bool> {
    Some(match state? {
        "created" | "starting" | "started" | "replacing" => true,
        "stopping" | "stopped" | "exited" | "destroyed" | "deleting" | "suspended" => false,
        other => return None,
    })
}

/// The error text in a Fly answer: its `error` (or `message`) field,
/// else the body itself, kept short — it rides inside [`HostError`].
fn fly_message(body: &[u8]) -> String {
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let text = parsed
        .as_ref()
        .and_then(|v| {
            ["error", "message"]
                .iter()
                .find_map(|key| v[key].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string());
    text.chars().take(240).collect()
}

/// A machine id is `[A-Za-z0-9_-]{1,64}`; anything else never reaches a
/// URL of ours.
fn machine_id(id: &str) -> Result<&str, HostError> {
    let ok = (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(id)
    } else {
        Err(config_error(format!("{id:?} is not a machine id")))
    }
}

/// A Fly app name is `[a-z0-9-]{1,60}` starting with a letter or digit.
fn is_fly_name(name: &str) -> bool {
    (1..=60).contains(&name.len())
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The Fly Machines provider ([`HostProvider`]). `http` is how this
/// home talks HTTPS: [`ReqwestFly`] on a native server, the Worker's
/// own adapter over the platform fetch in `crates/live-worker`.
pub struct FlyProvider {
    config: FlyConfig,
    http: Arc<dyn FlyHttp>,
}

impl FlyProvider {
    pub fn new(config: FlyConfig, http: Arc<dyn FlyHttp>) -> Self {
        FlyProvider { config, http }
    }

    /// From `LIVE_FLY_*`, with the reqwest adapter (native only; the
    /// Worker builds the provider with its own [`FlyHttp`]).
    #[cfg(feature = "native")]
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, HostError> {
        Ok(FlyProvider::new(
            FlyConfig::from_env(env)?,
            Arc::new(ReqwestFly::new().map_err(call_error)?),
        ))
    }

    async fn call(
        &self,
        method: FlyMethod,
        url: &str,
        body: Option<Vec<u8>>,
    ) -> Result<FlyResponse, HostError> {
        let req = FlyRequest {
            method,
            url: url.to_string(),
            body,
        };
        self.http
            .call(&req, &self.config.token)
            .await
            .map_err(|e| call_error(format!("{method} {url}: {e}")))
    }
}

#[async_trait]
impl HostProvider for FlyProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Fly
    }

    /// Creates the machine and returns its id as the provider ref. The
    /// host registers itself when it is up (`HostReady`); nothing here
    /// waits for it.
    async fn start(&self, lease: &HostLease, boot: &HostBoot) -> Result<String, HostError> {
        let machine = create_request(lease, boot, &self.config)?;
        let body = serde_json::to_vec(&machine)
            .map_err(|e| call_error(format!("encoding the Fly machine request: {e}")))?;
        let url = self.config.machines_url(None)?;
        let resp = self.call(FlyMethod::Post, &url, Some(body)).await?;
        if !(200..300).contains(&resp.status) {
            return Err(call_error(format!(
                "POST {url} answered {}: {}",
                resp.status,
                fly_message(&resp.body)
            )));
        }
        let machine: Machine = serde_json::from_slice(&resp.body)
            .map_err(|e| call_error(format!("POST {url}: not a machine ({e})")))?;
        machine_id(&machine.id)?;
        Ok(machine.id)
    }

    /// Deletes the machine, forcing it off if it is still running. A
    /// machine already gone (404) or on its way down (409) is stopped.
    async fn stop(&self, _lease_id: &str, provider_ref: &str) -> Result<(), HostError> {
        let url = format!("{}?force=true", self.config.machines_url(Some(provider_ref))?);
        let resp = self.call(FlyMethod::Delete, &url, None).await?;
        match resp.status {
            200..=299 | 404 | 409 => Ok(()),
            status => Err(call_error(format!(
                "DELETE {url} answered {}: {}",
                status,
                fly_message(&resp.body)
            ))),
        }
    }

    /// Whether the machine is up: booting or running states are alive;
    /// a machine that's gone (404) is not, rather than an error, so the
    /// reaper can treat it as ended.
    async fn alive(&self, provider_ref: &str) -> Result<bool, HostError> {
        let url = self.config.machines_url(Some(provider_ref))?;
        let resp = self.call(FlyMethod::Get, &url, None).await?;
        match resp.status {
            200..=299 => {
                let machine: Machine = serde_json::from_slice(&resp.body).map_err(|e| {
                    call_error(format!("GET {url}: not a machine ({e})"))
                })?;
                alive_state(machine.state.as_deref())
                    .ok_or_else(|| call_error(format!("GET {url}: machine state {:?}", machine.state)))
            }
            404 => Ok(false),
            status => Err(call_error(format!(
                "GET {url} answered {}: {}",
                status,
                fly_message(&resp.body)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::Xuid;

    fn config() -> FlyConfig {
        FlyConfig {
            token: "flytoken".to_string(),
            app: "h2-hosts".to_string(),
            image: "registry.forbes.red/h2-host:97".to_string(),
            region: "iad".to_string(),
            ports: DEFAULT_PORTS,
            api: "https://api.fly.io".to_string(),
        }
    }

    fn lease() -> HostLease {
        HostLease {
            id: "hl-3fRNhbDZ7EpNRRF0".to_string(),
            assignment_id: "assignment-7".to_string(),
            provider: ProviderKind::Fly,
            region: Some("lhr".to_string()),
            class: HostClass::Small,
            map: "lockout".to_string(),
            map_id: None,
            variant: Some("slayer".to_string()),
            hopper: Some(3),
            max_players: 8,
            roster: vec![Xuid(1), Xuid(2)],
            bots: Vec::new(),
            transport: Transport::Udp,
            idle_timeout_s: 120,
            max_match_s: 1800,
            created_ms: 1_763_000_000_000,
        }
    }

    fn boot() -> HostBoot {
        HostBoot {
            live_url: "https://live.forbes.red".to_string(),
            token: "host-jwt".to_string(),
            data_url: "https://blobs.forbes.red/host-data.tar.zst?sig=1".to_string(),
            data_sha256: "deadbeefcafe".to_string(),
        }
    }

    /// A scripted [`FlyHttp`]: answers in order, records every call.
    struct FakeFly {
        replies: std::sync::Mutex<std::collections::VecDeque<FlyResponse>>,
        calls: std::sync::Mutex<Vec<(FlyMethod, String, Option<Vec<u8>>, String)>>,
    }

    impl FakeFly {
        fn new(replies: Vec<FlyResponse>) -> Arc<Self> {
            Arc::new(FakeFly {
                replies: std::sync::Mutex::new(replies.into()),
                calls: std::sync::Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl FlyHttp for FakeFly {
        async fn call(&self, req: &FlyRequest, token: &str) -> Result<FlyResponse, String> {
            self.calls.lock().unwrap().push((
                req.method,
                req.url.clone(),
                req.body.clone(),
                token.to_string(),
            ));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "no reply scripted".to_string())
        }
    }

    fn provider(replies: Vec<FlyResponse>) -> FlyProvider {
        FlyProvider::new(config(), FakeFly::new(replies))
    }

    fn reply(status: u16, body: serde_json::Value) -> FlyResponse {
        FlyResponse {
            status,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    #[test]
    fn guest_sizes_follow_the_lease_class() {
        assert_eq!(
            guest(&HostClass::Small),
            MachineGuest {
                cpu_kind: "performance".to_string(),
                cpus: 1,
                memory_mb: 2048,
            }
        );
        assert_eq!(
            guest(&HostClass::Large),
            MachineGuest {
                cpu_kind: "performance".to_string(),
                cpus: 2,
                memory_mb: 4096,
            }
        );
    }

    #[test]
    fn udp_services_cover_the_window_with_matching_ports() {
        let services = services(31_000);
        assert_eq!(services.len(), GAME_PORTS as usize);
        for (i, service) in services.iter().enumerate() {
            let port = 31_000 + i as u16;
            assert_eq!(service.protocol, "udp");
            assert_eq!(service.internal_port, port, "external and internal match");
            assert_eq!(service.ports, vec![MachinePort { port }]);
        }
    }

    #[test]
    fn port_bases_are_deterministic_aligned_and_inside_the_range() {
        let (lo, hi) = DEFAULT_PORTS;
        for id in ["hl-3fRNhbDZ7EpNRRF0", "hl-aaaaaaaaaaaaaaaa", "sim-42"] {
            let base = port_base(id, DEFAULT_PORTS).unwrap();
            assert_eq!(base, port_base(id, DEFAULT_PORTS).unwrap(), "stable");
            assert!(base >= lo && base + GAME_PORTS - 1 <= hi, "{id}: {base}");
            assert_eq!((base - lo) % GAME_PORTS, 0, "windows are aligned");
        }
    }

    #[test]
    fn a_range_without_a_whole_window_is_refused() {
        for ports in [(31_000, 31_002), (44_000, 30_000)] {
            assert!(port_base("hl-a", ports).is_err(), "{ports:?}");
        }
        // Exactly one whole window still fits.
        assert!(port_base("hl-a", (31_000, 31_006)).is_ok());
        assert_eq!(windows((31_000, 31_006)).unwrap(), 1);
    }

    #[test]
    fn machine_env_carries_the_lease_the_boot_and_the_bind() {
        let lease = lease();
        let env = machine_env(&lease, &boot()).unwrap();
        assert_eq!(
            env,
            BTreeMap::from([
                ("H2_LEASE".to_string(), "hl-3fRNhbDZ7EpNRRF0".to_string()),
                (
                    "H2_HOST_LEASE_JSON".to_string(),
                    serde_json::to_string(&lease).unwrap(),
                ),
                ("H2_LIVE".to_string(), "https://live.forbes.red".to_string()),
                ("H2_TOKEN".to_string(), "host-jwt".to_string()),
                (
                    "H2_DATA_URL".to_string(),
                    "https://blobs.forbes.red/host-data.tar.zst?sig=1".to_string()
                ),
                ("H2_DATA_SHA".to_string(), "deadbeefcafe".to_string()),
                ("H2_XNET_BIND".to_string(), GLOBAL_BIND.to_string()),
            ])
        );
    }

    #[test]
    fn create_request_builds_the_machine_the_contract_spells() {
        let fly = config();
        let lease = lease();
        let request = create_request(&lease, &boot(), &fly).unwrap();
        let base = port_base(&lease.id, fly.ports).unwrap();
        let expected_services: Vec<_> = (0..GAME_PORTS)
            .map(|i| {
                serde_json::json!({
                    "protocol": "udp",
                    "internal_port": base + i,
                    "ports": [{"port": base + i}],
                })
            })
            .collect();
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "region": "lhr",
                "config": {
                    "image": "registry.forbes.red/h2-host:97",
                    "guest": {"cpu_kind": "performance", "cpus": 1, "memory_mb": 2048},
                    "env": {
                        "H2_LEASE": "hl-3fRNhbDZ7EpNRRF0",
                        "H2_HOST_LEASE_JSON": serde_json::to_string(&lease).unwrap(),
                        "H2_LIVE": "https://live.forbes.red",
                        "H2_TOKEN": "host-jwt",
                        "H2_DATA_URL": "https://blobs.forbes.red/host-data.tar.zst?sig=1",
                        "H2_DATA_SHA": "deadbeefcafe",
                        "H2_XNET_BIND": "fly-global-services",
                    },
                    "auto_destroy": true,
                    "restart": {"policy": "no"},
                    "services": expected_services,
                    "metadata": {"h2-lease": "hl-3fRNhbDZ7EpNRRF0"},
                },
            })
        );
    }

    #[test]
    fn a_tunnel_lease_gets_no_services_and_the_default_region() {
        let mut lease = lease();
        lease.transport = Transport::Tunnel;
        lease.region = None;
        let request = create_request(&lease, &boot(), &config()).unwrap();
        assert_eq!(request.region, "iad");
        assert!(request.config.services.is_empty());
    }

    #[test]
    fn from_env_reads_the_settings_and_names_what_is_missing() {
        let env = |k: &str| {
            match k {
                "LIVE_FLY_TOKEN" => Some("flytoken".to_string()),
                "LIVE_FLY_APP" => Some("h2-hosts".to_string()),
                "LIVE_FLY_IMAGE" => Some("registry.forbes.red/h2-host:97".to_string()),
                "LIVE_FLY_REGION" => Some("lhr".to_string()),
                "LIVE_FLY_PORTS" => Some("31000-32000".to_string()),
                _ => None,
            }
        };
        let fly = FlyConfig::from_env(&env).unwrap();
        assert_eq!(fly.region, "lhr");
        assert_eq!(fly.ports, (31_000, 32_000));
        assert_eq!(fly.api, API);

        let without_app = |k: &str| match k {
            "LIVE_FLY_TOKEN" => Some("flytoken".to_string()),
            "LIVE_FLY_IMAGE" => Some("registry.forbes.red/h2-host:97".to_string()),
            _ => None,
        };
        let err = FlyConfig::from_env(&without_app).unwrap_err().to_string();
        assert!(err.contains("LIVE_FLY_APP"), "{err}");

        let bad_ports = |k: &str| (k == "LIVE_FLY_PORTS").then(|| "1-2".to_string());
        let err = FlyConfig::from_env(&bad_ports).unwrap_err().to_string();
        assert!(err.contains("LIVE_FLY_PORTS"), "{err}");
    }

    #[tokio::test]
    async fn start_posts_the_built_machine_and_returns_its_id() {
        let http = FakeFly::new(vec![reply(
            201,
            serde_json::json!({"id": "0ad9d4f6c1e1b", "state": "starting"}),
        )]);
        let fly = FlyProvider::new(config(), http.clone());
        let id = fly.start(&lease(), &boot()).await.unwrap();
        assert_eq!(id, "0ad9d4f6c1e1b");
        let calls = http.calls.lock().unwrap();
        let (method, url, body, token) = &calls[0];
        assert_eq!(*method, FlyMethod::Post);
        assert_eq!(url, "https://api.fly.io/v1/apps/h2-hosts/machines");
        assert_eq!(token, "flytoken");
        let expected = serde_json::to_vec(&create_request(&lease(), &boot(), &config()).unwrap());
        assert_eq!(body.as_deref(), expected.ok().as_deref());
    }

    #[tokio::test]
    async fn start_maps_an_api_refusal_to_an_error_that_says_why() {
        let fly = provider(vec![reply(
            400,
            serde_json::json!({"error": "port 31000 already allocated"}),
        )]);
        let err = fly.start(&lease(), &boot()).await.unwrap_err().to_string();
        assert!(err.contains("400"), "{err}");
        assert!(err.contains("port 31000 already allocated"), "{err}");
    }

    #[tokio::test]
    async fn stop_deletes_with_force_and_accepts_a_machine_already_gone() {
        let destroyed = reply(200, serde_json::json!({"id": "0ad9d4f6c1e1b", "state": "destroyed"}));
        let http = FakeFly::new(vec![destroyed, reply(404, serde_json::json!({}))]);
        let fly = FlyProvider::new(config(), http.clone());
        fly.stop("hl-3fRNhbDZ7EpNRRF0", "0ad9d4f6c1e1b")
            .await
            .unwrap();
        fly.stop("hl-3fRNhbDZ7EpNRRF0", "0ad9d4f6c1e1b")
            .await
            .unwrap();
        let calls = http.calls.lock().unwrap();
        assert_eq!(
            calls[0].1,
            "https://api.fly.io/v1/apps/h2-hosts/machines/0ad9d4f6c1e1b?force=true"
        );
        assert_eq!(calls[0].0, FlyMethod::Delete);
        assert!(calls[0].2.is_none(), "DELETE has no body");
    }

    #[tokio::test]
    async fn alive_reads_the_state_and_a_gone_machine_is_not_alive() {
        for (state, alive) in [
            ("starting", true),
            ("started", true),
            ("created", true),
            ("stopped", false),
            ("exited", false),
            ("destroyed", false),
        ] {
            let fly = provider(vec![reply(
                200,
                serde_json::json!({"id": "m", "state": state}),
            )]);
            assert_eq!(
                fly.alive("m").await.unwrap(),
                alive,
                "state {state} reads {alive}"
            );
        }
        let gone = provider(vec![reply(404, serde_json::json!({}))]);
        assert!(!gone.alive("m").await.unwrap(), "404 is not alive");
        let unknown = provider(vec![reply(
            200,
            serde_json::json!({"id": "m", "state": "reconfiguring"}),
        )]);
        let err = unknown.alive("m").await.unwrap_err().to_string();
        assert!(err.contains("reconfiguring"), "{err}");
    }

    #[test]
    fn machine_states_are_classified_without_a_call() {
        assert_eq!(alive_state(Some("started")), Some(true));
        assert_eq!(alive_state(Some("destroyed")), Some(false));
        assert_eq!(alive_state(None), None);
        assert_eq!(alive_state(Some("warping")), None);
    }

    #[test]
    fn ids_that_are_not_ids_never_reach_a_url() {
        let long = "x".repeat(65);
        for bad in ["", "../../v1/tokens", "m nj", long.as_str()] {
            assert!(machine_id(bad).is_err(), "{bad:?}");
        }
        assert_eq!(machine_id("0ad9d4f6c1e1b").unwrap(), "0ad9d4f6c1e1b");
        assert!(is_fly_name("h2-hosts"));
        assert!(!is_fly_name("H2_Hosts"));
        assert!(!is_fly_name(""));
        assert!(!is_fly_name("-nope"));
    }

    #[test]
    fn fly_errors_come_out_as_their_text() {
        let said = fly_message(br#"{"error":"not enough credits"}"#);
        assert_eq!(said, "not enough credits");
        assert_eq!(fly_message(br#"{"message":"slow down"}"#), "slow down");
        assert_eq!(fly_message(b"plain text"), "plain text");
        let long: String = std::iter::repeat('x').take(400).collect();
        assert_eq!(fly_message(long.as_bytes()).chars().count(), 240);
    }
}
