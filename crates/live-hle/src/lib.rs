//! Xbox Live public-entry overrides for Halo 2 retail XONLINES 5849.
//! Each Runtime owns one Hle. Locks are guest memory, then Live state, and
//! neither guard escapes an invocation. See docs/live-boundary.md for the ABI.
#![deny(unsafe_code)]

mod config;
pub mod entries;
mod host;
mod network;
mod network_config;
mod qos;
mod qos_memory;
mod state;
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
    pub const CONNECTED: u32 = 0x0015_10f0;
    pub const TASK_SUCCESS: u32 = 0x0015_00f0;
    pub const RUNNING_IDLE: u32 = 0x0015_00f2;
    pub const NO_NEW_CONTENT: u32 = 0x0015_3102;
    pub const USER_CHANGE_COMMIT: u32 = 0x0015_13f0;
    pub const USER_CHANGE_COMPLETE: u32 = 0x0015_13f1;
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
        network.validate().map_err(ConfigError::Network)?;
        let address = network.address;
        let accounts = config.accounts()?;
        Ok(Arc::new(Self {
            config,
            state: Mutex::new(state::State::new(accounts, address)),
            network: network::Network::new(network),
        }))
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
