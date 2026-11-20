//! Ad-hoc dedicated game hosts. The hosts stream owns the lease, its
//! deadlines and its provider reference. Boot credentials never enter the log.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use live_proto::hosts::{
    DEFAULT_IDLE_TIMEOUT_S, DEFAULT_MAX_MATCH_S, EndReason, HOST_BOOTING, HOST_ENDED,
    HOST_PLAYING, HOST_READY, HOST_REQUESTED, HostBooting, HostClass, HostEnded, HostEndpoint,
    HostLease, HostPlaying, HostReady, HostRequested, HostState, ProviderKind, STREAM, Transport,
};
use live_proto::{Author, ErrorCode, lxm, sign};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::dispatch::{Dispatcher, Subscriber};
use crate::host_costs::HostCosts;
use crate::store::{RecordedEvent, StoreError};

mod endpoint;
pub mod fly;
mod lifecycle;
#[cfg(not(target_arch = "wasm32"))]
pub mod local;
#[cfg(test)]
mod tests;

const BOOT_MS: i64 = 60_000;
const PROBE_MS: i64 = 30_000;
const RECENT: usize = 100;
// Give a supervisor time to receive the final HTTP response and exit normally.
const EXIT_GRACE_MS: i64 = 5_000;
const CLEANUP_CURSOR: &str = "hosts.provider-cleanup";

type ApiResult<T> = Result<T, ApiError>;

/// Inputs for a single launch. Avoid Debug: both token and data_url can
/// contain credentials.
#[derive(Clone)]
pub struct HostBoot {
    pub live_url: String,
    pub token: String,
    pub data_url: String,
    pub data_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Provider(String),
}

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }
}

impl From<std::io::Error> for HostError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

/// axum needs Send handler futures on both targets. Worker providers wrap
/// JavaScript handles/futures the same way as the Worker's EventStore.
#[async_trait]
pub trait HostProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn start(&self, lease: &HostLease, boot: &HostBoot) -> Result<String, HostError>;
    /// Stopping an already stopped host succeeds.
    async fn stop(&self, lease_id: &str, provider_ref: &str) -> Result<(), HostError>;
    async fn alive(&self, provider_ref: &str) -> Result<bool, HostError>;

    /// Connect to a local tunnel listener bound by this managed process.
    #[cfg(feature = "native")]
    fn tunnel_addr(&self, _lease_id: &str, _provider_ref: &str) -> Option<std::net::SocketAddr> {
        None
    }
}

/// Worker bindings and native environment variables use the same settings.
#[derive(Clone)]
pub struct HostConfig {
    pub live_url: String,
    pub data_url: String,
    pub data_sha256: String,
    pub max_hosts: usize,
}

impl HostConfig {
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            live_url: env("LIVE_HOST_LIVE_URL").unwrap_or_default(),
            data_url: env("LIVE_HOST_DATA_URL").unwrap_or_default(),
            data_sha256: env("LIVE_HOST_DATA_SHA256").unwrap_or_default(),
            max_hosts: env("LIVE_MAX_HOSTS").map_or(4, |s| s.parse().unwrap_or(0)),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HostView {
    pub lease: HostLease,
    pub state: HostState,
    pub endpoint: Option<HostEndpoint>,
    pub reason: Option<EndReason>,
    pub players: u8,
    pub players_peak: u8,
    pub boot_ms: Option<u64>,
    pub ran_ms: Option<u64>,
    pub updated_ms: i64,
    #[serde(skip)]
    provider_ref: Option<String>,
    #[serde(skip)]
    started_ms: Option<i64>,
    #[serde(skip)]
    idle_since_ms: i64,
}

impl HostView {
    pub fn active(&self) -> bool {
        !matches!(self.state, HostState::Ended | HostState::Failed)
    }

    fn deadline(&self) -> Option<(i64, EndReason)> {
        if !self.active() {
            return None;
        }
        let hard = self
            .lease
            .created_ms
            .saturating_add(i64::from(self.lease.max_match_s) * 1000);
        // The game reports the transition to Playing, then Empty/MatchOver;
        // an occupied match does not need periodic heartbeats to keep its lease.
        if self.state == HostState::Playing && self.players > 0 {
            return Some((hard, EndReason::MaxDuration));
        }
        let (other, reason) = match self.state {
            HostState::Requested | HostState::Booting => (
                self.lease.created_ms.saturating_add(BOOT_MS),
                EndReason::BootTimeout,
            ),
            _ => (
                self.idle_since_ms
                    .saturating_add(i64::from(self.lease.idle_timeout_s) * 1000),
                EndReason::Idle,
            ),
        };
        Some(if other <= hard {
            (other, reason)
        } else {
            (hard, EndReason::MaxDuration)
        })
    }
}

#[derive(Default)]
struct Registry {
    leases: BTreeMap<String, HostView>,
    next_version: u64,
    // Terminal leases that had a provider reference, in log position order.
    cleanup: BTreeMap<u64, String>,
    // An expired request can still have an outstanding provider.start.
    // Keep its slot until the reference arrives and cleanup succeeds.
    launching: BTreeMap<String, Option<String>>,
    cleaned_through: u64,
}

impl Registry {
    fn occupied(&self) -> usize {
        let mut occupied: std::collections::BTreeSet<&str> = self
            .leases
            .values()
            .filter(|r| r.active())
            .map(|r| r.lease.id.as_str())
            .collect();
        occupied.extend(self.cleanup.values().map(String::as_str));
        occupied.extend(self.launching.keys().map(String::as_str));
        occupied.len()
    }
}

pub(crate) struct Hosts {
    live: OnceLock<Weak<Live>>,
    registry: RwLock<Registry>,
    writes: Mutex<()>,
    cleanup_work: Mutex<()>,
    last_probe_ms: AtomicI64,
    reports: crate::guard::RateLimiter,
    provider: RwLock<Option<Arc<dyn HostProvider>>>,
    config: RwLock<HostConfig>,
    costs: Arc<HostCosts>,
}

fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

impl Hosts {
    pub(crate) async fn subscribe(
        dispatcher: &mut Dispatcher,
        write_rate: (f64, u32),
        store: &dyn crate::store::EventStore,
        costs: HostCosts,
    ) -> Result<Arc<Self>, StoreError> {
        let costs = Arc::new(costs);
        dispatcher.subscribe(costs.clone());
        let hosts = Arc::new(Self {
            live: Default::default(),
            registry: RwLock::new(Registry {
                cleaned_through: store.load_checkpoint(CLEANUP_CURSOR).await?.unwrap_or(0),
                ..Registry::default()
            }),
            cleanup_work: Default::default(),
            last_probe_ms: Default::default(),
            writes: Default::default(),
            reports: crate::guard::RateLimiter::new(write_rate.0, write_rate.1),
            provider: Default::default(),
            config: RwLock::new(HostConfig::from_env(&|k| std::env::var(k).ok())),
            costs,
        });
        dispatcher.subscribe(hosts.clone());
        Ok(hosts)
    }

    pub(crate) fn attach(&self, live: &Arc<Live>) {
        let _ = self.live.set(Arc::downgrade(live));
    }

    fn version(&self) -> u64 {
        self.registry.read().unwrap().next_version
    }

    fn provider_for(&self, lease: &HostLease) -> ApiResult<Arc<dyn HostProvider>> {
        self.provider
            .read()
            .unwrap()
            .as_ref()
            .filter(|p| p.kind() == lease.provider)
            .cloned()
            .ok_or_else(|| unavailable("host provider unavailable"))
    }

    /// Whether a provider start is still expected for this lease: a
    /// `Requested` lease without one is a manual host an admin started
    /// without a launch, which registers directly.
    pub(crate) fn launching(&self, lease_id: &str) -> bool {
        self.registry
            .read()
            .unwrap()
            .launching
            .contains_key(lease_id)
    }
}

/// The `POST /v1/admin/hosts` body (docs/live/live-api.md): a custom
/// dedicated host an admin starts by hand, either launched on the
/// configured provider (`launch`, the default) or reserved for a container
/// the operator runs.
#[derive(Debug, Clone, Deserialize)]
pub struct AdminHostStart {
    /// The scenario name.
    pub map: String,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default = "default_max_players")]
    pub max_players: u8,
    #[serde(default = "default_class")]
    pub class: HostClass,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub transport: Option<Transport>,
    /// Start the configured provider (the default). `false` records the
    /// lease as `Requested` and answers its boot token instead.
    #[serde(default = "default_launch")]
    pub launch: bool,
}

fn default_max_players() -> u8 {
    8
}

fn default_class() -> HostClass {
    HostClass::Small
}

fn default_launch() -> bool {
    true
}

impl AdminHostStart {
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if self.map.is_empty() || !self.map.is_ascii() || self.map.len() > 64 {
            return Err(invalid("map must be 1 to 64 ASCII characters"));
        }
        if !(1..=16).contains(&self.max_players) {
            return Err(invalid("max_players must be between 1 and 16"));
        }
        Ok(())
    }

    /// The lease this request describes, with a fresh assignment id. Live
    /// mints the lease id and creation time.
    pub(crate) fn lease(&self, provider: ProviderKind) -> HostLease {
        let transport = self.transport.clone().unwrap_or(match provider {
            ProviderKind::Cloudflare => Transport::Tunnel,
            ProviderKind::Fly | ProviderKind::Local => Transport::Udp,
        });
        HostLease {
            id: String::new(),
            assignment_id: format!("custom-{}", sign::random_id()),
            provider,
            region: self.region.clone().filter(|r| !r.is_empty()),
            class: self.class.clone(),
            map: self.map.clone(),
            map_id: None,
            variant: self.variant.clone().filter(|v| !v.is_empty()),
            hopper: None,
            max_players: self.max_players,
            roster: Vec::new(),
            bots: Vec::new(),
            transport,
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
            max_match_s: DEFAULT_MAX_MATCH_S,
            created_ms: 0,
        }
    }
}

fn unavailable(message: &'static str) -> ApiError {
    ApiError::new(ErrorCode::Unavailable, message)
}

impl Subscriber for Hosts {
    fn name(&self) -> &str {
        STREAM
    }

    fn apply(&self, e: &RecordedEvent) {
        if e.body.stream != STREAM {
            return;
        }
        let mut registry = self.registry.write().unwrap();
        registry.next_version = e.version + 1;
        let Some(at) = sign::parse_time(&e.body.at) else {
            return;
        };
        let at = (at.unix_timestamp_nanos() / 1_000_000) as i64;
        let data = &e.body.data;
        if e.body.kind == HOST_REQUESTED {
            if let Ok(HostRequested { lease }) = HostRequested::deserialize(data) {
                registry.leases.entry(lease.id.clone()).or_insert(HostView {
                    lease,
                    state: HostState::Requested,
                    endpoint: None,
                    provider_ref: None,
                    started_ms: None,
                    players: 0,
                    players_peak: 0,
                    boot_ms: None,
                    ran_ms: None,
                    updated_ms: at,
                    idle_since_ms: at,
                    reason: None,
                });
            }
            return;
        }
        let Some(row) = data["lease_id"]
            .as_str()
            .and_then(|id| registry.leases.get_mut(id))
        else {
            return;
        };
        if !row.active() {
            return;
        }
        match e.body.kind.as_str() {
            HOST_BOOTING if row.state == HostState::Requested => {
                let Ok(boot) = HostBooting::deserialize(data) else {
                    return;
                };
                row.provider_ref = Some(boot.provider_ref);
                row.started_ms = Some(at);
                row.state = HostState::Booting;
            }
            // A manual lease (admin-started without a launch) registers
            // straight out of Requested; it never passes through Booting.
            HOST_READY if matches!(row.state, HostState::Booting | HostState::Requested) => {
                let Ok(ready) = HostReady::deserialize(data) else {
                    return;
                };
                row.endpoint = Some(ready.endpoint);
                row.boot_ms = Some(ready.boot_ms);
                row.idle_since_ms = at;
                row.state = HostState::Joinable;
            }
            HOST_PLAYING if matches!(row.state, HostState::Joinable | HostState::Playing) => {
                let Ok(HostPlaying { players, .. }) = HostPlaying::deserialize(data) else {
                    return;
                };
                // Empty heartbeats cannot keep an empty host alive.
                if players > 0 || row.players > 0 {
                    row.idle_since_ms = at;
                }
                row.players = players;
                row.players_peak = row.players_peak.max(players);
                row.state = HostState::Playing;
            }
            HOST_ENDED => {
                let Ok(ended) = HostEnded::deserialize(data) else {
                    return;
                };
                row.state = match ended.reason {
                    EndReason::BootTimeout | EndReason::Crashed | EndReason::Budget => {
                        HostState::Failed
                    }
                    _ => HostState::Ended,
                };
                row.reason = Some(ended.reason);
                row.ran_ms = Some(ended.ran_ms);
                row.players_peak = row.players_peak.max(ended.players_peak);
                row.players = 0;
            }
            _ => return,
        }
        row.updated_ms = at;
        if !row.active() && row.provider_ref.is_some() {
            let id = row.lease.id.clone();
            if e.position > registry.cleaned_through {
                registry.cleanup.insert(e.position, id);
            }
        }
    }
}

impl Live {
    pub fn set_host_provider(&self, provider: Arc<dyn HostProvider>) {
        *self.hosts.provider.write().unwrap() = Some(provider);
    }

    pub fn set_host_config(&self, config: HostConfig) {
        *self.hosts.config.write().unwrap() = config;
    }

    pub fn host_provider_kind(&self) -> Option<ProviderKind> {
        self.hosts
            .provider
            .read()
            .unwrap()
            .as_ref()
            .map(|p| p.kind())
    }

    /// Trusted lookup for matchmaking, arbitration and the host service identity.
    pub fn host_view(&self, lease_id: &str) -> Option<HostView> {
        self.hosts
            .registry
            .read()
            .unwrap()
            .leases
            .get(lease_id)
            .cloned()
    }

    pub(crate) fn dedicated_host_fields(
        &self,
        subject: &str,
    ) -> live_proto::hosts::DedicatedHostFields {
        let Some(id) = crate::host_identity::lease_id(subject) else {
            return Default::default();
        };
        let dedicated_region = self
            .hosts
            .registry
            .read()
            .unwrap()
            .leases
            .get(id)
            .and_then(|row| row.lease.region.clone());
        live_proto::hosts::DedicatedHostFields {
            host_lease: Some(id.to_owned()),
            dedicated_region,
        }
    }

    fn require_host(&self, lease_id: &str) -> ApiResult<HostView> {
        self.host_view(lease_id)
            .ok_or_else(|| not_found("no such host lease"))
    }

    // Check the hosts stream's expected version as well as the in-process
    // write lock: a second native server can share this database.
    async fn host_event(
        &self,
        version: u64,
        kind: &str,
        data: impl Serialize + Send,
    ) -> ApiResult<()> {
        let event = self.server_event(
            STREAM.into(),
            kind,
            serde_json::to_value(data).expect("host event serializes"),
        );
        let rows = self
            .store
            .append(vec![event], &[(STREAM.into(), version)])
            .await
            .map_err(|e| match e {
                StoreError::Conflict { .. } => conflict("host state changed; retry"),
                other => other.into(),
            })?;
        self.wait_applied(rows[0].position).await
    }
}
