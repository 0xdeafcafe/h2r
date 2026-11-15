//! Xbox Live public-entry overrides for Halo 2 retail XONLINES 5849.
//! Each Runtime owns one Hle. Locks are guest memory, then Live state, and
//! neither guard escapes an invocation. See docs/live-boundary.md for the ABI.
#![deny(unsafe_code)]

mod client;
mod config;
mod messages;
pub use client::{ServerOptions, ServerStatus};
pub use messages::{ATTACHMENT_CAP, SUMMARY_SIZE};
pub mod entries;
mod host;
mod network;
mod network_config;
mod outbox;
pub use outbox::{
    BuiltMessage, MessageProperty, MessageRecipient, MessageSend, MessageSendKind, MessageUser,
    MessageValue, RecruitProperties,
};
mod qos;
mod qos_memory;
mod sessions;
pub use sessions::{LocalSession, SessionAttribute, SessionAttributeValue, SessionSlots};
mod state;
mod stats;
pub use stats::{Stat, StatSpec, StatValue, StatWrite};
mod storage_api;
mod teams;
pub use teams::{MEMBER_SIZE, TEAM_PROPERTIES_SIZE, TEAM_SIZE};
mod ui;
pub use network_config::NetworkConfig;

pub use config::{ConfigError, LiveConfig};
use entries::{Entry, Operation};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, GuestFault, TransferAbi};
pub use state::USER_SIZE;
use std::{
    net::SocketAddrV4,
    sync::{Arc, Mutex},
};

/// PhyGetLinkState's active / 100 Mbps / full-duplex mask, as exposed by the
/// existing kernel ConsoleConfig (the public link wrapper returns that mask).
pub const PHY_LINK: u32 = 0x0d;

pub mod status {
    pub const OK: u32 = 0;
    pub const INSUFFICIENT_BUFFER: u32 = 0x8007_007a;
    pub const STORAGE_FILE_NOT_FOUND: u32 = 0x8015_c004;
    pub const STORAGE_ACCESS_DENIED: u32 = 0x8015_c002;
    pub const STORAGE_INVALID_PATH: u32 = 0x8015_c008;
    pub const STORAGE_INVALID_FACILITY: u32 = 0x8015_c009;
    pub const NO_ACCEPTED_INVITE: u32 = 0x8000_4005; // E_FAIL; S_FALSE would be an invite to Halo.
    pub const CANNOT_ACCESS_SERVICE: u32 = 0x8015_1001;
    pub const SERVERS_BUSY: u32 = 0x8015_1003;
    pub const CONNECTION_LOST: u32 = 0x8015_1004;
    pub const BUFFER_TOO_SMALL: u32 = 0x8015_0001;
    pub const MATCH_INVALID_SESSION: u32 = 0x8015_5100;
    pub const MATCH_INVALID_DATA_TYPE: u32 = 0x8015_5102;
    pub const MATCH_INVALID_SEARCH: u32 = 0x8015_5105;
    pub const MATCH_INVALID_RESULT_COLUMN: u32 = 0x8015_510a;
    pub const MATCH_INVALID_STRING: u32 = 0x8015_510b;
    pub const MATCH_STRING_TOO_LONG: u32 = 0x8015_510c;
    pub const MATCH_BLOB_TOO_LONG: u32 = 0x8015_510d;
    pub const MESSAGE_MAX_DETAILS: u32 = 0x8015_5a08;
    pub const MESSAGE_INVALID_ID: u32 = 0x8015_5a01;
    pub const MESSAGE_DOWNLOAD_REQUIRED: u32 = 0x8015_5a02;
    pub const MESSAGE_PROPERTY_NOT_FOUND: u32 = 0x8015_5a03;
    pub const CONNECTED: u32 = 0x0015_10f0;
    pub const TASK_SUCCESS: u32 = 0x0015_00f0;
    pub const TASK_RESULTS_AVAILABLE: u32 = 0x0015_00f1;
    pub const SERVICE_TEMPORARILY_UNAVAILABLE: u32 = 0x8015_1102;
    pub const TEAM_NOT_FOUND: u32 = 0x8015_210b;
    pub const STAT_BAD_REQUEST: u32 = 0x8015_9001;
    pub const STAT_TOO_MANY_SPECS: u32 = 0x8015_9004;
    pub const STAT_TOO_MANY_STATS: u32 = 0x8015_9005;
    pub const STAT_PERMISSION_DENIED: u32 = 0x8015_9200;
    pub const RESULTS_UNAVAILABLE: u32 = 0x8015_0007;
    pub const STAT_USER_NOT_FOUND: u32 = 0x8015_9003;
    pub const QUERY_DATASET_NOT_FOUND: u32 = 0x8015_6109;
    pub const RUNNING_IDLE: u32 = 0x0015_00f2;
    pub const NO_NEW_CONTENT: u32 = 0x0015_3102;
    pub const USER_CHANGE_COMMIT: u32 = 0x0015_13f0;
    pub const USER_CHANGE_COMPLETE: u32 = 0x0015_13f1;
    pub const USER_CHANGE_FAILED: u32 = 0x8015_1300;
    pub const INVALID_ARGUMENT: u32 = 0x8007_0057;
    pub const NOT_INITIALIZED: u32 = 0x8015_0005;
    pub const NO_SESSION: u32 = 0x8015_0002;
    pub const NO_USER: u32 = 0x8015_0006;
    pub const OUT_OF_MEMORY: u32 = 0x8015_0008;
    pub const TASK_BUSY: u32 = 0x8015_0009;
    pub const INVALID_USER: u32 = 0x8015_1006;
    pub const SERVICE_NOT_REQUESTED: u32 = 0x8015_1100;
    pub const WSA_NOT_INITIALIZED: u32 = 10093;
    pub const WSA_INVALID_ARGUMENT: u32 = 10022;
    pub const WSA_VERSION: u32 = 10092;
    pub const WSA_NO_BUFFERS: u32 = 10055;
}

pub struct Hle {
    config: LiveConfig,
    state: Mutex<state::State>,
    network: network::Network,
}
impl Hle {
    /// Loads account identities once; does not connect to the server. Disabled
    /// configuration never reads a file or queries host networking.
    pub fn new(config: LiveConfig) -> Result<Arc<Self>, ConfigError> {
        let address = if config.enabled {
            config::lan_address()
        } else {
            SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 3074)
        };
        Self::with_address(config, address)
    }
    /// An explicit host endpoint, for embedding and deterministic tests.
    pub fn with_address(
        config: LiveConfig,
        address: SocketAddrV4,
    ) -> Result<Arc<Self>, ConfigError> {
        Self::with_network(config, NetworkConfig::new(address))
    }
    /// Explicit channel base and loopback discovery peers. No sockets bind until
    /// the guest calls bind or QosListen. The server URL remains service-only.
    pub fn with_network(
        config: LiveConfig,
        network: NetworkConfig,
    ) -> Result<Arc<Self>, ConfigError> {
        Self::with_server_options(config, network, ServerOptions::default())
    }
    /// Host HTTP timing, including the 30-second production heartbeat. The
    /// worker is started without making a request; logon initiates service I/O.
    pub fn with_server_options(
        config: LiveConfig,
        network: NetworkConfig,
        options: ServerOptions,
    ) -> Result<Arc<Self>, ConfigError> {
        network.validate().map_err(ConfigError::Network)?;
        if options.request_timeout.is_zero() || options.heartbeat_interval.is_zero() {
            return Err(ConfigError::Worker);
        }
        let address = network.address;
        let accounts = config.accounts()?;
        let service = if config.enabled {
            config
                .server_url
                .as_ref()
                .map(|url| client::Service::start(url.clone(), options))
                .transpose()
                .map_err(|_| ConfigError::Worker)?
        } else {
            None
        };
        Ok(Arc::new(Self {
            config,
            state: Mutex::new(state::State::new(accounts, address, service)),
            network: network::Network::new(network),
        }))
    }
    /// Telemetry can call this after the server assigns a game ID; no I/O or
    /// guest memory access occurs on the caller's thread. None returns to online.
    pub fn set_game_id(&self, id: Option<&str>) -> Result<(), u32> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_game_id(id)
    }
    pub fn server_status(&self) -> ServerStatus {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .server_status()
    }
    /// Accepted local stat writes, with no guest pointers or account keys.
    pub fn stat_writes(&self) -> Vec<StatWrite> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stat_writes()
    }
    /// Transfer captured writes to a future server-event forwarder.
    pub fn drain_stat_writes(&self) -> Vec<StatWrite> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain_stat_writes()
    }
    /// Locally accepted outgoing messages, with owned properties/recipients.
    pub fn message_sends(&self) -> Vec<MessageSend> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .message_sends()
    }
    /// Transfer captures to a future server-event forwarder without reusing IDs.
    pub fn drain_message_sends(&self) -> Vec<MessageSend> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain_message_sends()
    }
    /// Local advertisements, with owned attributes and no exchange keys.
    pub fn local_sessions(&self) -> Vec<LocalSession> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .local_sessions()
    }
    pub fn local_teams(&self) -> live_proto::team::Store {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .local_teams()
    }
    /// Trusted host-side seam for properties whose SDK thunk is not linked.
    pub fn local_team_action(
        &self,
        actor: u64,
        action: &live_proto::team::Action,
    ) -> Result<(), live_proto::team::TeamError> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .local_team_action(actor, action)
    }
    pub fn network_config(&self) -> &NetworkConfig {
        &self.network.config
    }
    pub fn config(&self) -> &LiveConfig {
        &self.config
    }
}

pub struct Registration {
    pub installed: Vec<Entry>,
}
#[derive(Debug, thiserror::Error)]
#[error("Live override already registered at {0:#010x}")]
pub struct RegisterError(pub u32);

/// Install the entire named boundary, including explicit stops. Like XAPI and
/// DSound, reject conflicts before editing dispatch. Disabled installs nothing.
pub fn register(dispatch: &mut Dispatch, hle: &Arc<Hle>) -> Result<Registration, RegisterError> {
    if !hle.config.enabled {
        return Ok(Registration {
            installed: Vec::new(),
        });
    }
    for e in entries::ENTRIES {
        if dispatch.override_name(e.va).is_some() {
            return Err(RegisterError(e.va));
        }
    }
    for &e in entries::ENTRIES {
        let hle = Arc::clone(hle);
        dispatch.register_override(e.va, format!("live_hle::{}", e.name), move |cpu, ctx, _| {
            invoke(&hle, e, cpu, ctx)
        });
    }
    Ok(Registration {
        installed: entries::ENTRIES.to_vec(),
    })
}
/// Disable only this registration's entries; generated handlers are preserved.
pub fn unregister(dispatch: &mut Dispatch, registration: &Registration) {
    for e in &registration.installed {
        dispatch.set_enabled(e.va, false);
    }
}
fn stop(e: Entry, cpu: &CpuAbi, ctx: &mut ExecCtx, detail: String) -> TransferAbi {
    let backtrace = ctx.backtrace(cpu.eip);
    TransferAbi::stop(ctx.emit(Event::Unsupported {
        pc: cpu.eip,
        mnemonic: format!("live_hle::{}", e.name),
        detail,
        backtrace,
    }))
}
fn invoke(
    hle: &Hle,
    e: Entry,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> Result<TransferAbi, GuestFault> {
    if e.operation == Operation::Stop {
        return Ok(stop(
            e,
            cpu,
            ctx,
            format!("unimplemented Live API {}", e.name),
        ));
    }
    let args = {
        let m = ctx.memory();
        m.check(
            cpu.esp,
            (e.stack_words as usize + 1) * 4,
            runtime::guest::Access::Read,
            1,
        )?;
        (0..e.stack_words)
            .map(|i| m.u32(cpu.esp.wrapping_add(4 + i * 4)))
            .collect::<Result<Vec<_>, _>>()?
    };
    if let Operation::Net(op) = e.operation {
        let value = hle.network.invoke(op, &args, ctx.thread_id, ctx.runtime());
        cpu.eax = value;
        return ctx.memory().ret(cpu, e.stack_words * 4);
    }
    if e.operation == Operation::QosRelease && hle.network.release_qos(args[0]) {
        cpu.eax = 0;
        return ctx.memory().ret(cpu, e.stack_words * 4);
    }
    let (result, cleanup) = {
        let mut m = ctx.memory();
        let mut state = hle.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = state.invoke(e.operation, &args, &mut m);
        let cleanup = if matches!(result, Ok(0)) {
            hle.network.lifecycle(e.operation)
        } else {
            None
        };
        (result, cleanup)
    };
    drop(cleanup); // Socket wakeups and QoS responder join run without locks.
    let result = match result {
        Ok(value) => {
            cpu.eax = if e.operation == Operation::WsaCleanup && value != 0 {
                hle.network.set_error(ctx.thread_id, value);
                u32::MAX
            } else {
                value
            };
            return ctx.memory().ret(cpu, e.stack_words * 4);
        }
        Err(error) => error,
    };
    match result {
        state::Error::Fault(f) => Err(f),
        state::Error::Unverified(why) => Ok(stop(
            e,
            cpu,
            ctx,
            format!("unverified Live API {}: {why}", e.name),
        )),
    }
}
