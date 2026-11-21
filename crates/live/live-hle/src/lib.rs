//! Xbox Live public-entry overrides for Halo 2 retail XONLINES 5849.
//! Each Runtime owns one Hle. Locks are guest memory, then Live state, and
//! neither guard escapes an invocation. See docs/live/live-boundary.md for the ABI.
#![deny(unsafe_code)]

pub mod account_setup;
mod auto;
pub mod clankermatch;
mod client;
mod config;
mod feedback;
mod film_service;
pub mod films;
mod invites;
pub mod join_resolver;
pub mod join_link;
pub mod maps;
pub mod mods;
mod messages;
mod mutes;
pub use client::{HostPublisher, Retry, ServerOptions, ServerStatus};
pub use messages::{ATTACHMENT_CAP, SUMMARY_SIZE};
pub mod entries;
#[cfg(unix)]
mod host;
#[cfg(windows)]
#[path = "host_windows.rs"]
mod host;
mod matchmaking;
mod network;
mod network_config;
mod outbox;
pub mod platform;
pub mod signin;
pub use outbox::{
    BuiltMessage, MessageProperty, MessageRecipient, MessageSend, MessageSendKind, MessageUser,
    MessageValue, RecruitProperties,
};
mod qos;
mod qos_memory;
mod socket_wait;
mod sessions;
pub use sessions::{
    LocalSession, MapProbe, SessionAttribute, SessionAttributeValue, SessionSlots, SkullsProbe,
    Visibility, VisibilityProbe, ping_moved,
};
mod state;
pub use state::coop_activity;
mod stats;
pub use stats::{Stat, StatSpec, StatValue, StatWrite};
mod arbitration;
pub use arbitration::{Arbitration, ArbitrationRound};
mod storage_api;
mod teams;
pub mod traffic;
pub mod tunnel_client;
pub use tunnel_client::{TunnelHandle, connect_tunnel};
pub use teams::{MEMBER_SIZE, TEAM_PROPERTIES_SIZE, TEAM_SIZE};
mod ui;
pub use network_config::{COOP_PORT_OFFSET, NetworkConfig};

pub use config::{ConfigError, LiveConfig, NEW_ACCOUNT_TAG, NEW_ACCOUNT_XUID};
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
    pub const FRIEND_NOT_FOUND: u32 = 0x8015_2008;
    pub const NO_ACCEPTED_INVITE: u32 = 0x8000_4005; // E_FAIL; S_FALSE would be an invite to Halo.
    pub const CANNOT_ACCESS_SERVICE: u32 = 0x8015_1001;
    pub const SERVERS_BUSY: u32 = 0x8015_1003;
    pub const CONNECTION_LOST: u32 = 0x8015_1004;
    /// The account needs attention on the dashboard: what a banned account
    /// gets (docs/live/live-boundary.md, game result 2, dialog 0x27).
    pub const ACCOUNT_REQUIRES_MANAGEMENT: u32 = 0x8015_1200;
    pub const BUFFER_TOO_SMALL: u32 = 0x8015_0001;
    /// HLE's customer HRESULT for a terminal Live queue refusal.
    pub const REPLICATION_VERSION_MISMATCH: u32 = 0xa015_5100;
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

/// What a guest knows of its host's skulls ([`Hle::host_skulls`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSkulls {
    /// No session joined: no key registered but this console's own.
    NotJoined,
    /// Joined `session`, whose advertisement was never read (a System Link
    /// game, or no server).
    Unknown { session: String },
    /// Joined `session`, whose host advertised `skulls` (empty for none);
    /// `refreshed` when read just now rather than at the join.
    Known {
        session: String,
        skulls: Vec<u8>,
        refreshed: bool,
    },
}

/// What a guest knows of its host's variant options
/// ([`Hle::host_variant_ext`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostVariantExt {
    /// No session joined.
    NotJoined,
    /// Joined `session`, whose advertisement was never read.
    Unknown { session: String },
    /// Joined `session`, whose host advertised `ext` (None: a host that
    /// sends none, an older build); `refreshed` when read just now.
    Known {
        session: String,
        ext: Option<live_proto::session::VariantExt>,
        refreshed: bool,
    },
}

/// What a host knows of its players' builds ([`Hle::players_lacking`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lacking {
    /// Nothing known: not signed in to a server, or no answer in time; why.
    Unknown(&'static str),
    /// The gamertags of the players whose presence lacks the capability,
    /// in the order asked; empty when every player with a presence has it.
    Known(Vec<String>),
}

/// A friend, as the co-op screen's friends list shows them
/// ([`Hle::friends`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Friend {
    pub xuid: live_proto::Xuid,
    pub gamertag: String,
    pub online: bool,
    pub playing: bool,
    pub activity: Option<String>,
}

/// The session this console joined, as the server last described it
/// ([`Hle::joined`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Joined {
    /// No session joined: no key registered but this console's own.
    NotJoined,
    /// Joined `session`, whose advertisement was never read (a System Link
    /// game, or no server).
    Unknown { session: String },
    /// Joined it, and this is what its host advertised; `refreshed` when
    /// read just now rather than at the join.
    Known {
        session: Box<live_proto::session::Session>,
        refreshed: bool,
    },
}

impl Joined {
    /// The co-op campaign of the joined session, when it is one.
    pub fn campaign(&self) -> Option<&live_proto::session::Campaign> {
        match self {
            Joined::Known { session, .. } if session.advertisement.is_coop() => {
                session.advertisement.campaign.as_ref()
            }
            _ => None,
        }
    }
}

pub struct Hle {
    offline_film: bool,
    service_taped: std::sync::OnceLock<bool>,
    film_side: Mutex<film_service::Side>,
    config: LiveConfig,
    state: Mutex<state::State>,
    network: network::Network,
}
impl Hle {
    fn film_view<T>(&self, mut read: impl FnMut(&film_service::View) -> T) -> Option<T> {
        (self.offline_film && self.film_service_taped()).then(|| {
            read(
                &self
                    .film_side
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .view,
            )
        })
    }
    /// Installed once by boot, before any guest thread runs.
    pub fn set_film_tap(&self, tap: Arc<dyn film::tape::Tap>) {
        let service_taped = tap.service_calls();
        assert!(self.network.tap.set(tap).is_ok());
        assert!(self.service_taped.set(service_taped).is_ok());
    }
    /// Game-thread observations made by the co-op lobby use the same cursor
    /// as the guest's XOnline calls. V1 films did not carry those observations.
    pub fn film_observation_tap(&self) -> Option<Arc<dyn film::tape::Tap>> {
        self.network
            .tap
            .get()
            .filter(|_| self.film_service_taped())
            .cloned()
    }

    /// Fixes the version used by Live QoS admission. Boot calls this once
    /// before starting the guest, with the version of its entity definitions.
    pub fn set_replication_version(&self, version: u16) {
        self.network.version.set(version);
    }

    pub fn film_address(&self) -> SocketAddrV4 {
        self.network.config.address
    }

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
        if options.publisher.is_some()
            && (!config.enabled
                || config.server_url.is_none()
                || config.accounts_path.is_some()
                || config.offer_new_account)
        {
            return Err(ConfigError::HostCredential);
        }
        let address = network.address;
        let accounts = config.accounts()?;
        let version = qos::Version::default();
        let service = if config.enabled {
            config
                .server_url
                .as_ref()
                .map(|url| client::Service::start(url.clone(), options, version.clone()))
                .transpose()
                .map_err(|_| ConfigError::Worker)?
        } else {
            None
        };
        Ok(Arc::new(Self {
            offline_film: false,
            film_side: Mutex::default(),
            service_taped: Default::default(),
            state: Mutex::new(state::State::new(
                accounts,
                address,
                service,
                config.clone(),
            )),
            config,
            network: network::Network::new(network, version),
        }))
    }
    /// Preserve the dedicated lobby's configured session class, with no HTTP
    /// worker, accounts or usable remote URL. The guest network uses its tape.
    pub fn for_film(network: NetworkConfig, live_service: bool) -> Result<Arc<Self>, ConfigError> {
        network.validate().map_err(ConfigError::Network)?;
        let config = LiveConfig {
            enabled: true,
            server_url: live_service.then(|| "film:offline".to_owned()),
            ..Default::default()
        };
        Ok(Arc::new(Self {
            offline_film: true,
            film_side: Mutex::default(),
            service_taped: Default::default(),
            state: Mutex::new(state::State::new(
                Vec::new(),
                network.address,
                None,
                config.clone(),
            )),
            config,
            network: network::Network::new(network, qos::Version::default()),
        }))
    }
    /// Reads the accounts file again (after [`account_setup::save`]), so a
    /// new or recovered account is listed by `XOnlineGetUsers` and can sign
    /// in without a restart. Accounts signed in now stay signed in; the
    /// file is checked as at startup, and on an error nothing changes.
    /// Returns how many accounts there are. No guest memory access.
    pub fn reload_accounts(&self) -> Result<usize, ConfigError> {
        let accounts = self.config.accounts()?;
        let n = accounts.len();
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_accounts(accounts);
        Ok(n)
    }
    /// The account signed into Live on this controller, when it has a key.
    pub fn signed_in_account(&self, controller: u32) -> Option<(live_proto::Xuid, String)> {
        self.lock().signed_in_account(controller)
    }

    /// The in-game sign-in prompt, while a logon waits for a code
    /// ([`signin`]). Takes in a finished server call; no guest memory.
    pub fn signin(&self) -> Option<signin::Prompt> {
        self.lock().signin()
    }
    /// The controller whose prompt should open now: once per sign-in.
    pub fn take_signin_open(&self) -> Option<u32> {
        self.lock().take_signin_open()
    }
    /// The code the player typed; the check runs on its own thread.
    /// False when no code is being asked for.
    pub fn signin_submit(&self, code: &str) -> bool {
        self.lock().signin_submit(code)
    }
    /// Make the gamertag on the server, when the prompt offers that.
    pub fn signin_create(&self) -> bool {
        self.lock().signin_create()
    }
    /// Text typed on the game's keyboard for a new account's gamertag or
    /// invite; `None` when the keyboard was backed out of. False when no
    /// text is being asked for.
    pub fn signin_typed(&self, text: Option<&str>) -> bool {
        self.lock().signin_typed(text)
    }
    /// The player backed out: the waiting logon fails.
    pub fn signin_cancel(&self) {
        self.lock().signin_cancel()
    }
    /// The prompt closed; a finished sign-in is forgotten.
    pub fn signin_dismiss(&self) {
        self.lock().signin_dismiss()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, state::State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Telemetry can call this after the server assigns a game ID; no I/O or
    /// guest memory access occurs on the caller's thread. None returns to online.
    pub fn set_game_id(&self, id: Option<&str>) -> Result<(), u32> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_game_id(id)
    }
    /// Who may find this console's hosted sessions in the server's lists and
    /// the custom games browser (docs/live/live-custom-games.md). Boot sets it from
    /// the `live.visibility` setting; open advertisements are updated at once.
    /// No guest memory access.
    pub fn set_session_visibility(&self, v: Visibility) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_session_visibility(v)
    }
    /// Where the title keeps its own party privacy: read (with guest memory
    /// already held) each time a lobby is advertised or updated, winning over
    /// the setting. Boot installs one that reads the squad session.
    pub fn set_visibility_probe(&self, probe: Option<VisibilityProbe>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_visibility_probe(probe)
    }
    /// Lets advertisements name the host's selected map
    /// (`NewSession.map_id`, docs/game/maps.md §5). Without it they name none.
    pub fn set_map_probe(&self, probe: Option<MapProbe>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_map_probe(probe)
    }
    /// The title has changed the map selected in its multiplayer lobby.
    pub fn lobby_map_changed(&self, guest: &runtime::Guest) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lobby_map_changed(guest);
    }
    /// The host's multiplayer skulls (docs/game/skulls.md "In multiplayer"):
    /// advertisements carry them from now, and open ones are updated at
    /// once. No guest memory access.
    pub fn set_session_skulls(&self, skulls: Option<Vec<u8>>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_session_skulls(skulls)
    }
    /// Lets each advertisement and update the game makes read the host's
    /// skulls afresh (docs/game/skulls.md "In multiplayer"); called with guest
    /// memory held, so it must take no lock guest memory may come after.
    pub fn set_skulls_probe(&self, probe: Option<SkullsProbe>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_skulls_probe(probe)
    }
    /// The session this console joined, asked of the server again (waiting
    /// at most `wait`), else as last read (the search, find or invite that
    /// led to the join). Call with no guest memory held; it never takes
    /// guest memory.
    pub fn joined(&self, wait: std::time::Duration) -> Joined {
        let keys = self.network.registered_keys();
        let (id, pending, seen) = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(id) = s.joined_session(&keys) else {
                return Joined::NotJoined;
            };
            let (pending, seen) = s.refresh_session(&id);
            (id, pending, seen)
        };
        let refreshed = pending.and_then(|p| p.wait(wait)).is_some_and(|r| r.is_ok());
        let known = seen.and_then(|m| {
            m.lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&id)
                .cloned()
        });
        match known {
            Some(session) => Joined::Known {
                session: Box::new(session),
                refreshed,
            },
            None => Joined::Unknown { session: id },
        }
    }
    /// The playlist (hopper descriptor index, as game records name it) and
    /// Live session id (16 hex digits) of the game about to start, for its
    /// record (docs/live/live-matchmaking.md §8.3, §8.4): the dedicated
    /// match, this console's advertisement when it hosts, or a joined session.
    /// `None` for a custom game or when nothing says. Never waits and never
    /// takes guest memory, so the game-start hook may call it.
    pub fn matchmade(&self) -> Option<(u32, String)> {
        if let Some(value) = self.film_view(|v| v.matchmade.clone()) {
            return value;
        }
        let keys = self.network.registered_keys();
        self.lock().matchmade(&keys)
    }
    /// Whether Live assigned frame uploads to the host for this match alone.
    /// The uploader reads it when it latches the published session ID.
    pub fn frames_from_host(&self, session_id: &str) -> bool {
        if let Some(value) = self.film_view(|v| {
            v.frames_from_host && v.matchmade.as_ref().is_some_and(|(_, id)| id == session_id)
        }) {
            return value;
        }
        self.lock().frames_from_host(session_id)
    }
    /// Map Pack availability, host progress and replication refusals for matchmaking.
    pub fn matchmaking_status(&self) -> Option<String> {
        if let Some(value) = self.film_view(|v| v.status.clone()) {
            return value;
        }
        self.lock().matchmaking_status().map(str::to_owned)
    }
    /// The mounted console has the original Multiplayer Map Pack installed.
    pub fn has_map_pack(&self) -> bool {
        self.lock().has_map_pack()
    }
    /// Only a queue assignment for this console's hosted lobby may use an
    /// add-on entry in the guest's weighted map picker.
    pub fn allow_hopper_addons(&self, hopper: u32) -> bool {
        self.lock().allow_hopper_addons(hopper)
    }
    /// The clanker hoppers the server served last (docs/game/bots.md §8):
    /// the playlists the Clankermatch row lists and OptiMatch does not.
    /// Never waits.
    pub fn clanker_hoppers(&self) -> clankermatch::ClankerHoppers {
        if let Some(value) = self.film_view(|v| v.hoppers.clone()) {
            return value;
        }
        self.lock().clanker_hoppers()
    }
    /// Asks the server for its clanker hoppers again (also asked at each
    /// logon). Returns at once.
    pub fn refresh_clanker_hoppers(&self) -> Result<(), String> {
        self.lock().refresh_clanker_hoppers()
    }
    /// The hosted session (hex) and its hopper's mark while this console
    /// hosts a clanker hopper's lobby: a Clankermatch, whose bots the host
    /// adds (§8.4). Never waits.
    pub fn clanker_hosted(&self) -> Option<(String, clankermatch::ClankerHopper)> {
        if let Some(value) = self.film_view(|v| v.clanker_hosted.clone()) {
            return value;
        }
        self.lock().clanker_hosted()
    }
    /// The Live session this console hosts (16 lowercase hex digits),
    /// while its advertisement stands. Never waits on the server.
    pub fn hosted_session(&self) -> Option<String> {
        self.film_view(|v| v.hosted.clone())
            .unwrap_or_else(|| self.lock().hosted_session())
    }
    /// The v1 recorder included bot roster polls in its ordered network
    /// calls. V2 replays bot membership and actions from the film, so even
    /// an idle roster poll must leave the guest/lobby call cursor alone.
    pub fn bot_roster_hosted_session(&self) -> Option<String> {
        if self.network.tap.get().is_some() && !self.film_service_taped() {
            return self.lobby_hosted_session();
        }
        self.hosted_session()
    }
    /// Ordered observation for a pending dedicated lobby request. Legacy bot
    /// roster polls also use it to retain the v1 network call sequence.
    pub fn lobby_hosted_session(&self) -> Option<String> {
        let Some(tap) = self.network.tap.get() else {
            return self.lock().hosted_session();
        };
        const NAME: &str = "HostedSession";
        if tap.replaying() {
            let call = tap.next(NAME, &[], 0)?;
            match call.outputs.as_slice() {
                [] => None,
                [id] if id.len() == 16 && id.iter().all(u8::is_ascii_hexdigit) => {
                    String::from_utf8(id.clone()).ok()
                }
                _ => {
                    tap.fault("invalid recorded session ID".into());
                    None
                }
            }
        } else {
            let id = self.lock().hosted_session();
            tap.record(
                film::NETR,
                film::tape::Call {
                    index: 0,
                    op: NAME.into(),
                    args: vec![],
                    thread: 0,
                    result: Ok(0),
                    outputs: id
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                        .into_iter()
                        .collect(),
                },
            );
            id
        }
    }
    /// Whether the dedicated host's specific advertisement still exists. A
    /// different lobby can already be current when retail finishes closing.
    pub fn hosts_session(&self, id: &str) -> bool {
        let present = || {
            self.local_sessions()
                .iter()
                .any(|session| live_proto::sign::hex(&session.id) == id)
        };
        let Some(tap) = self.network.tap.get().filter(|t| t.service_calls()) else {
            return present();
        };
        let Ok(id) = u64::from_str_radix(id, 16) else {
            return false;
        };
        let args = [id as u32, (id >> 32) as u32];
        let op = "HostsSession";
        if tap.replaying() {
            return match tap.next(op, &args, 0) {
                Some(call) if call.outputs.is_empty() && matches!(call.result, Ok(0 | 1)) => {
                    call.result == Ok(1)
                }
                _ => {
                    tap.fault("invalid hosted session observation".into());
                    false
                }
            };
        }
        let exists = present();
        tap.record(
            film::NETR,
            film::tape::Call {
                op: op.into(),
                args: args.into(),
                thread: 0,
                index: 0,
                result: Ok(u32::from(exists)),
                outputs: Vec::new(),
            },
        );
        exists
    }
    /// Asks the server for a bot roster for this console's session
    /// (`POST /v1/games/bots` with a `games.write` token, docs/game/bots.md
    /// §9.1). Returns at once; `done` is called on the Live worker with the
    /// roster, already [`live_proto::bots::Roster::check`]ed, or why there
    /// is none. A later roster for the same session replaces this one.
    pub fn roster_bots(
        &self,
        request: live_proto::bots::RosterRequest,
        done: Box<dyn FnOnce(Result<live_proto::bots::Roster, String>) + Send>,
    ) -> Result<(), String> {
        self.lock().roster_bots(request, done)
    }
    /// Queues a remote or recent Clanker lookup. A queued lookup calls
    /// `done` with its result on the Live worker. A lookup that cannot
    /// be queued returns its status immediately.
    pub fn bot_card(
        &self,
        xuid: live_proto::Xuid,
        done: Box<dyn FnOnce(Result<live_proto::bots::PlayerCard, u32>) + Send>,
    ) -> Result<(), u32> {
        self.lock().bot_card(xuid, done)
    }
    /// Whether the guest has a signed-in player who can read the mod list.
    /// Makes no request and never touches guest memory.
    pub fn check_mod_list(&self) -> Result<(), mods::ModsError> {
        self.lock().check_mod_list()
    }
    /// A page of the mod repository, newest pack first (`GET /v1/mod-packs`
    /// with a `read` token, docs/game/maps.md §5 "Mod downloads"). Returns
    /// at once; `done` is called on the Live worker with the page, or why
    /// there is none. Never touches guest memory.
    pub fn mod_list(
        &self,
        limit: u32,
        cursor: Option<String>,
        done: mods::ListDone,
    ) -> Result<(), mods::ModsError> {
        self.lock().mod_list(limit, cursor, done)
    }
    /// Downloads and installs published map `id` into `<data>/maps`
    /// through the map gate (the join's checks: the disc's base, every
    /// layer's SHA-256, `active.toml`), on a thread of its own. Returns at
    /// once; `progress` is told the bytes written and the bytes in all,
    /// `done` the package directory or why not.
    pub fn install_mod(
        &self,
        id: content::MapId,
        progress: mods::Progress,
        done: mods::InstallDone,
    ) -> Result<(), mods::ModsError> {
        self.lock().install_mod(id, progress, done)
    }
    /// The joined host's skulls from the last cached advertisement.
    /// Reads the Live cache without requesting a refresh.
    pub fn cached_host_skulls(&self) -> Option<Vec<u8>> {
        let keys = self.network.registered_keys();
        self.lock().cached_host_skulls(&keys)
    }
    /// What the host of the session this console joined advertised as its
    /// skulls: [`Hle::joined`]'s answer, for the skulls.
    pub fn host_skulls(&self, wait: std::time::Duration) -> HostSkulls {
        match self.joined(wait) {
            Joined::NotJoined => HostSkulls::NotJoined,
            Joined::Unknown { session } => HostSkulls::Unknown { session },
            Joined::Known { session, refreshed } => HostSkulls::Known {
                session: session.session_id.clone(),
                skulls: session.advertisement.skulls.clone().unwrap_or_default(),
                refreshed,
            },
        }
    }
    /// What the host of the session this console joined advertised as its
    /// variant options for the next game: [`Hle::joined`]'s answer.
    pub fn host_variant_ext(&self, wait: std::time::Duration) -> HostVariantExt {
        match self.joined(wait) {
            Joined::NotJoined => HostVariantExt::NotJoined,
            Joined::Unknown { session } => HostVariantExt::Unknown { session },
            Joined::Known { session, refreshed } => HostVariantExt::Known {
                session: session.session_id.clone(),
                ext: session.advertisement.variant_ext.clone(),
                refreshed,
            },
        }
    }
    /// Which of `xuids` (the players of this console's game: the session's
    /// roster, or the game's own player list) are on a build without
    /// `capability`, from their presence ([`live_proto::lacking`]). Waits at
    /// most `wait` for the server; call with no guest memory held, it never
    /// takes guest memory. A player with no presence (a bot) is not named.
    pub fn players_lacking(
        &self,
        capability: &str,
        xuids: &[live_proto::Xuid],
        wait: std::time::Duration,
    ) -> Lacking {
        if xuids.is_empty() {
            return Lacking::Known(Vec::new());
        }
        let Some(pending) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read_presence()
        else {
            return Lacking::Unknown("not signed in to a Live server");
        };
        match pending.wait(wait) {
            Some(Ok(client::Reply::Presence(all))) => Lacking::Known(
                live_proto::lacking(capability, xuids, &all)
                    .iter()
                    .map(|p| p.gamertag.as_str().to_string())
                    .collect(),
            ),
            Some(_) => Lacking::Unknown("the server's presence read failed"),
            None => Lacking::Unknown("the server did not answer in time"),
        }
    }
    /// The final setting returned to the game after a Live request (including
    /// any retries). Only this synchronous observation has a film cursor;
    /// the background transport and elapsed real time do not.
    pub fn game_setting<const N: usize>(
        &self,
        name: &'static str,
        args: &[u32],
        get: impl FnOnce() -> Option<[u8; N]>,
    ) -> Option<[u8; N]> {
        let Some(tap) = self.film_observation_tap() else {
            return get();
        };
        if tap.replaying() {
            let call = tap.next(name, args, 0)?;
            match (call.result, call.outputs.as_slice()) {
                (Ok(0), []) => None,
                (Ok(0), [bytes]) if bytes.len() == N => Some(bytes.as_slice().try_into().unwrap()),
                _ => {
                    tap.fault(format!("invalid {name} setting"));
                    None
                }
            }
        } else {
            let bytes = get();
            tap.record(
                film::NETR,
                film::tape::Call {
                    index: 0,
                    op: name.into(),
                    args: args.to_vec(),
                    thread: 0,
                    result: Ok(0),
                    outputs: bytes.iter().map(|b| b.to_vec()).collect(),
                },
            );
            bytes
        }
    }
    /// The host's variant options for the game it starts (docs/game/
    /// game-options.md section 3): advertisements carry them from now, and
    /// open ones are updated at once. No guest memory access.
    pub fn set_session_variant_ext(&self, ext: Option<live_proto::session::VariantExt>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_session_variant_ext(ext)
    }
    /// The bots (server accounts) in this console's hosted game, beside
    /// its signed-in users: the Live session their roster is for
    /// (`session`, hex) is told of them as players
    /// (`PlayerJoined`/`PlayerLeft`), so it counts every player in the
    /// game; the other sessions it hosts (a Clankermatch's party lobby)
    /// only of its users. Host-made bots are left out. No guest memory
    /// access.
    pub fn set_session_players(&self, session: Option<&str>, bots: Vec<live_proto::Xuid>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_session_players(session, bots)
    }
    /// Arms (Some) or leaves (None) a co-op campaign on this console's lobby
    /// (docs/live/live-coop.md, "Co-op over Live"): its advertisements become
    /// `coop` sessions naming the campaign, friends-only unless the host
    /// made them invite-only, carrying the host's campaign `skulls`
    /// (engine indices) in place of the multiplayer ones, and presence says
    /// "Co-op: <level>". Open advertisements are updated at once. No guest
    /// memory access.
    pub fn set_coop(
        &self,
        campaign: Option<live_proto::session::Campaign>,
        skulls: Option<Vec<u8>>,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_coop(campaign, skulls)
    }
    /// Co-op ping, the guest's side (docs/live/live-coop.md, "Ping"): one
    /// ping to the host's QoS responder (its XNet endpoint's port + 2),
    /// carrying `report`, our last round trip, for the host to show. The
    /// round trip in ms, None when no echo came back within `timeout`.
    /// Blocks up to `timeout` with no lock held.
    pub fn ping_host(
        &self,
        host: std::net::SocketAddrV4,
        report: Option<u16>,
        timeout: std::time::Duration,
    ) -> Option<u16> {
        if self.network.tap.get().is_some_and(|t| t.replaying()) {
            return None;
        }
        let qos = std::net::SocketAddrV4::new(*host.ip(), host.port().wrapping_add(2));
        qos::ping(qos, report.unwrap_or(0), timeout).map(qos::round_trip_ms)
    }
    /// Co-op ping, the host's side: keeps the QoS responder up (it answers
    /// the guest's pings), and returns the slowest round trip a guest
    /// reported in the last `fresh`.
    pub fn guest_ping(&self, fresh: std::time::Duration) -> Option<u16> {
        if let Err(e) = self.network.ensure_responder() {
            eprintln!("live: co-op ping: the QoS responder did not start: {e}");
        }
        self.network
            .reported_pings(fresh)
            .into_iter()
            .map(|(_, ms)| ms)
            .max()
    }
    /// A co-op host's round trip to its guest, for its sessions (None: no
    /// guest). No guest memory access.
    pub fn set_coop_ping(&self, ms: Option<u16>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_coop_ping(ms)
    }
    /// The co-op campaign armed with [`Hle::set_coop`], if any.
    pub fn coop(&self) -> Option<live_proto::session::Campaign> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).coop()
    }
    /// Presence's activity in words; None clears it. No guest memory access.
    pub fn set_activity(&self, activity: Option<&str>) -> Result<(), u32> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_activity(activity)
    }
    pub fn activity(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .activity()
            .map(str::to_owned)
    }
    /// Controller `controller`'s friends as last read, online first; asks
    /// the server again in the background. No guest memory access.
    pub fn friends(&self, controller: usize) -> Vec<Friend> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .friend_list(controller)
    }
    /// Sends `to` a game invitation to this console's advertised lobby from
    /// controller `controller`'s player; the session id on success. No
    /// guest memory access.
    pub fn invite_to_lobby(&self, controller: usize, to: live_proto::Xuid) -> Result<String, u32> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invite_to_lobby(controller, to)
    }
    /// Co-op test automation in place of `H2_COOP_AUTO_INVITE` and
    /// `H2_COOP_AUTO_ACCEPT` (read when the Hle is made): `invite` is a
    /// comma-separated list of gamertags this console invites to its
    /// advertised lobby, `accept_from` the host whose invitation it accepts.
    pub fn set_coop_auto(&self, invite: Option<&str>, accept_from: Option<&str>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_auto(crate::auto::Auto::from_values(invite, accept_from));
    }
    /// Publish the active host squad's actual capacity for lobbies whose
    /// title advertisement omits the slot counts.
    pub fn sync_squad_slots(&self, slots: SessionSlots) {
        self.lock().sync_squad_slots(slots);
    }

    /// The signed-in player on controller `controller`, 0 for none.
    pub fn user(&self, controller: usize) -> u64 {
        if self.offline_film && self.film_service_taped() {
            return self
                .film_side
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .view
                .users
                .get(controller)
                .copied()
                .unwrap_or(0);
        }
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .user(controller)
    }
    pub fn session_visibility(&self) -> Visibility {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session_visibility()
    }
    pub fn server_status(&self) -> ServerStatus {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .server_status()
    }
    /// The arbitration registrations and reports the game made, oldest
    /// first (docs/live/live-matchmaking.md §8.3). Kept here only.
    pub fn arbitration(&self) -> Vec<Arbitration> {
        self.lock().arbitration.snapshot()
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
    /// Interrupt a host tick wait on inbound XNet traffic. Leaves all
    /// reads, guest timeouts and readiness reporting to the game.
    pub fn wait_for_network_input(&self, timeout: std::time::Duration) -> bool {
        self.network.wait_for_input(timeout)
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
        // dh-client-join: unregistering this exact key leaves the dedicated
        // match. Unregistering the older party key can happen during its join.
        let leaving = if op == network::Op::UnregisterKey {
            let mut id = [0; 8];
            ctx.memory().read_bytes(args[0], &mut id, 1).ok().map(|()| id)
        } else {
            None
        };
        let value = hle.network.invoke(op, &args, ctx.thread_id, ctx.runtime());
        if value == 0 && let Some(id) = leaving {
            let mut state = hle.lock();
            if state.end_match_join(id) {
                hle.clear_tunnel();
            }
        }
        cpu.eax = value;
        return ctx.memory().ret(cpu, e.stack_words * 4);
    }
    if matches!(e.operation, Operation::Sessions(crate::sessions::Op::Search))
        && crate::matchmaking::config().is_some()
        && let Err(e) = hle.network.ensure_responder()
    {
        eprintln!("live: matchmaking: could not start QoS responder: {e}");
    }
    let full_tape = hle.network.tap.get().filter(|_| hle.film_service_taped());
    let released = e.operation == Operation::QosRelease && hle.network.release_qos(args[0]);
    if released && full_tape.is_none() {
        cpu.eax = hle.network.tap.get().map_or(0, |t| {
            network::film::service_qos(t.as_ref(), e.name, &args, ctx.thread_id, ctx.runtime(), 0)
        });
        return ctx.memory().ret(cpu, e.stack_words * 4);
    }
    // The accepted-invitation read stamps the record in the guest's clock;
    // the kernel's clock lock comes before guest memory, so read it first.
    // (The kernel's epoch is 0: its system time is this counter.)
    // A stat read names the session this console is in; the joined one
    // comes from the network's keys, read before the state lock as
    // `Hle::matchmade` does.
    let read_keys = matches!(e.operation, Operation::Stats(crate::stats::Op::Read))
        .then(|| hle.network.registered_keys());
    let guest_now = (e.operation == Operation::AcceptedInvite)
        .then(|| ctx.runtime().guest_time())
        .flatten();
    let thread_id = ctx.thread_id;
    let (result, cleanup) = {
        let mut m = ctx.memory();
        let mut state = hle.state.lock().unwrap_or_else(|e| e.into_inner());
        state.set_guest_now(guest_now);
        state.set_read_keys(read_keys);
        let had_match_tunnel = state.has_match_tunnel();
        let result = if let Some(tap) = full_tape {
            let mut side = hle.film_side.lock().unwrap_or_else(|e| e.into_inner());
            film_service::invoke(
                tap.as_ref(),
                e,
                &args,
                thread_id,
                &mut m,
                &mut side,
                |mem| {
                    let value = if released {
                        0
                    } else {
                        state.invoke(e.operation, &args, mem, hle)?
                    };
                    Ok((
                        value,
                        film_service::View::read(&state, &hle.network.registered_keys()),
                    ))
                },
            )
        } else {
            let mut invoke = || state.invoke(e.operation, &args, &mut m, hle);
            match hle.network.tap.get() {
                Some(tap) => network::film::with_random(tap, e.name, thread_id, invoke),
                None => invoke(),
            }
        };
        if had_match_tunnel && !state.has_match_tunnel() {
            hle.clear_tunnel();
        }
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
            let value = if full_tape.is_none()
                && matches!(e.operation, Operation::QosLookup | Operation::QosRelease)
                && let Some(tap) = hle.network.tap.get()
            {
                network::film::service_qos(
                    tap.as_ref(),
                    e.name,
                    &args,
                    ctx.thread_id,
                    ctx.runtime(),
                    value,
                )
            } else {
                value
            };
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
