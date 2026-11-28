//! The agent API's wire types (docs/runtime/agent-api.md).
//!
//! JSON-RPC 2.0 over a Unix socket or TCP ([`Endpoint`]): one JSON object
//! per line each way.
//! A request with an `id` gets exactly one response with that `id`; events
//! from `session.subscribe` arrive as notifications with method `event`.
//! Every result carries an [`At`] stamp saying which moment it describes.
//!
//! This crate has no game dependencies, so clients (`h2 agent`, tests)
//! link it without the runtime.

use std::collections::BTreeMap;

pub mod anim;
pub mod audio;
mod client;
pub mod discover;
#[cfg(any(unix, windows))]
pub mod local_socket;
pub mod bot_bounce;
pub mod collision;
pub mod debug;
pub mod editor;
mod endpoint;
pub mod expect;
pub mod firefight;
pub mod forge;
pub mod frame_graph;
pub mod game_load;
pub mod goto;
pub mod havok;
pub mod player_bot;
pub mod pogo;
pub mod superbounce;
pub mod superbounce_view;
pub mod host_lobby;
pub mod host_pause;
pub mod host_quit;
pub mod host_window;
pub mod host_shot;
pub mod host_memory;
pub mod memory_forensics;
pub mod memory_serve;
pub mod hot;
pub mod picture;
pub mod live;
mod load;
mod pad;
pub mod probe;
pub mod route;
pub mod rules;
pub mod raw;
mod render;
pub mod shadow;
pub mod sim_arms;
pub mod sim;
pub mod state;
pub mod takeover;
pub mod test_maps;
pub mod theater;
pub mod trace;
pub use client::{CallError, Client, ConnectionShutdown};
pub use endpoint::Endpoint;
pub use load::{LoadParams, LoadRecord, LoadSegment, Loads};
pub use pad::{AtTick, Now, PAD_BUTTONS, PadParams, PadStep};
pub use render::{
    CaptureCamera, CaptureLighting, CurrentView, CurrentViewParams, CaptureReply, CaptureSource, CompareParams, CompareReply, DepthRecord,
    DisplayRateStats, DrawLayout, DrawRecord, FrameCaptureParams, FrameParams, FrameRecord, FrameReply, FrameTotals,
    LayoutTexture, MarkerRow, MarkersParams, OpRecord, ParityParams, ParityReply, PassRecord, RenderStats, ResidualArm,
    ResidualCaller, ResidualParams, ResidualProducer, ResidualReply, ResidualTotals,
    ResidualUnattributed, ResidualWindow, SceneCarried, UiStats,
    SceneEntries, SceneEntryRecord, SceneLateRecord, SceneOpCounts, SceneParams, ScenePostRecord,
    SceneReply, SceneCarriedCensus, SceneCensusActivation, SceneCensusCheckpoint, SceneReadUse,
    SceneSideTarget, SceneTargetUse, SceneUses, SceneViewRecord, SnapRecord, SpawnViewParams,
    ViewParams, ViewRequest,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped when a method or a field changes meaning. Additions don't bump it.
pub const PROTOCOL: u32 = 1;

/// The environment variable naming where the game listens: a Unix socket
/// path, or `tcp://HOST:PORT` ([`Endpoint`]).
pub const ENV: &str = "H2_AGENT";

/// The root for the game's traces, flights and input logs.
pub const OUT_ENV: &str = "H2_AGENT_OUT";

/// The TCP port the Xbox app listens on (docs/runtime/xbox.md, "Remote
/// control").
pub const XBOX_PORT: u16 = 4720;

/// Where `h2 serve` listens and `h2 agent` connects when neither is told:
/// `$H2_AGENT`, else `agent.sock` in [`cache_dir`]. Not under `$TMPDIR`,
/// which differs between agent sessions.
pub fn default_socket() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os(ENV).filter(|p| !p.is_empty()) {
        return p.into();
    }
    cache_dir().join("agent.sock")
}

/// The user's cache directory for h2recomp ([`host_dirs::cache`],
/// docs/runtime/storage.md), else `$TMPDIR/h2recomp` when the host has
/// none. The agent keeps its input logs (`inputs/`) and captures
/// (`captures/`) here, never in the repository.
pub fn cache_dir() -> std::path::PathBuf {
    host_dirs::cache().unwrap_or_else(|| std::env::temp_dir().join(host_dirs::APP))
}

/// Where the game keeps its `traces/`, `flights/` and `inputs/` directories.
pub fn output_dir() -> std::path::PathBuf {
    output_dir_from(std::env::var_os(OUT_ENV))
}

fn output_dir_from(value: Option<std::ffi::OsString>) -> std::path::PathBuf {
    value.filter(|p| !p.is_empty()).map_or_else(cache_dir, Into::into)
}

/// Method names. `session.methods` lists them at run time.
pub mod method {
    pub const STATUS: &str = "session.status";
    pub const METHODS: &str = "session.methods";
    pub const CLAIM: &str = "session.claim";
    pub const RELEASE: &str = "session.release";
    pub const PAUSE: &str = "session.pause";
    pub const RESUME: &str = "session.resume";
    pub const STEP: &str = "session.step";
    pub const WAIT: &str = "session.wait";
    pub const SUBSCRIBE: &str = "session.subscribe";
    pub const WORLD_COUNT: &str = "world.count";
    pub const WORLD_QUERY: &str = "world.query";
    pub const WORLD_OBJECT: &str = "world.object";
    pub const PROFILE_COUNTERS: &str = "profile.counters";
    pub const PROFILE_CAPTURE: &str = "profile.capture";
    pub const FEATURES: &str = "features.list";
    /// Level loads, BSP switches and stalls: how long, and what the frames
    /// in them read, uploaded and compiled ([`crate::LoadParams`]).
    pub const LOAD_TIMELINE: &str = "load.timeline";
    /// List the names of saved input logs.
    pub const INPUT_LIST: &str = "input.list";
    /// Read a page of a saved input log.
    pub const INPUT_LOG_READ: &str = "input.log.read";

    /// Core methods, in the order `session.methods` lists them.
    pub const ALL: &[&str] = &[
        STATUS,
        METHODS,
        CLAIM,
        RELEASE,
        PAUSE,
        RESUME,
        STEP,
        WAIT,
        SUBSCRIBE,
        WORLD_COUNT,
        WORLD_QUERY,
        WORLD_OBJECT,
        PROFILE_COUNTERS,
        PROFILE_CAPTURE,
        FEATURES,
        LOAD_TIMELINE,
        INPUT_LIST,
        INPUT_LOG_READ,
    ];

    /// Methods that change the run; they need `session.claim` first.
    pub const CONTROL: &[&str] = &[PAUSE, RESUME, STEP];

    // Optional methods: `session.methods` lists only those registered by
    // the input tap, game-thread features or host glue.

    /// A structure BSP's breakable surfaces: which pieces of glass are
    /// broken ([`crate::BreakableParams`], [`crate::Breakable`]).
    pub const WORLD_BREAKABLE: &str = "world.breakable";
    /// Submit a website join through the same inbox as the OS and CLI.
    pub const LIVE_JOIN_OPEN: &str = "live.join.open";
    /// The latest web join and its outcome, if any.
    pub const LIVE_JOIN_STATUS: &str = "live.join.status";
    /// Guest matchmaking and session transitions, retained through teardown.
    pub const LIVE_MATCHMAKING_TRACE: &str = "live.matchmaking.trace";
    /// Queue pad steps for a player ([`crate::PadParams`]).
    pub const INPUT_PAD: &str = "input.pad";
    /// Drive one controller port through a sequence of movement legs.
    pub const INPUT_ROUTE: &str = "input.route";
    /// Read a port's active or most recent route.
    pub const INPUT_ROUTE_STATUS: &str = "input.route.status";
    /// Stop the active route and release the port.
    pub const INPUT_ROUTE_STOP: &str = "input.route.stop";
    /// Drop the queued steps and let go of everything.
    pub const INPUT_RELEASE: &str = "input.release";
    /// What each pad holds now, what is queued, and recording or replay.
    pub const INPUT_STATE: &str = "input.state";
    /// Log every controller read to a named input log.
    pub const INPUT_RECORD: &str = "input.record";
    /// Answer controller reads from a named input log.
    pub const INPUT_REPLAY: &str = "input.replay";
    /// End the recording (writing it) or the replay.
    pub const INPUT_STOP: &str = "input.stop";
    /// The host's controllers as their backends and the debug menu's gate
    /// see them: profile class, touchpad, buttons, and whether the menu is
    /// open (`h2 agent hostpads`).
    pub const INPUT_HOST: &str = "input.host";
    /// Desired PlayStation lightbar colours by Xbox controller port.
    pub const INPUT_LIGHTBAR: &str = "input.lightbar";
    /// Set or clear a temporary per-port RGB override.
    pub const INPUT_LIGHTBAR_SET: &str = "input.lightbar.set";
    /// Start the in-process sampling profiler (`{"hz": 1000}`).
    pub const SAMPLE_START: &str = "profile.sample.start";
    /// Stop it and write the samples (`{"inline": true}` returns them too).
    pub const SAMPLE_STOP: &str = "profile.sample.stop";
    /// Whether it runs, for how long, and what the last dump was.
    pub const SAMPLE_STATUS: &str = "profile.sample.status";
    /// The newest host perf line (`boot::host_perf`).
    pub const HOST_PERF: &str = "host.perf";
    /// Recent tick cost and process resource use.
    pub const HOST_METRICS: &str = "host.metrics";
    /// Process footprint, recent samples, registered counters and saved reports.
    pub const HOST_MEMORY: &str = "host.memory";
    pub const HOST_PEER_RTTS: &str = "host.peer_rtts";
    pub const HOST_AUDIT: &str = "host.audit";
    pub const HOST_LOBBY_OPEN: &str = "host.lobby.open";
    pub const HOST_LOBBY_STATE: &str = "host.lobby.state";
    pub const HOST_LOBBY_START: &str = "host.lobby.start";
    pub const HOST_LOBBY_CLOSE: &str = "host.lobby.close";
    /// Leave the running campaign or multiplayer game for the main menu
    /// as the title's own departure does (`game_menu.rs` in `game`).
    pub const GAME_MENU: &str = "game.menu";
    /// Queue an installed multiplayer package by its complete MapId.
    pub const MAPS_LOAD: &str = "maps.load";
    /// The game's content packages in load order, on or off and why, and
    /// what each map read got from them (docs/editor/studio-redesign.md,
    /// "How the game consumes packages"; `boot::packages`).
    pub const PACKAGES_LIST: &str = "packages.list";
    /// The tag store at `<data>/tags`: the maps it can serve and whether
    /// this boot serves them (docs/engine/asset-store.md, `store-maps`;
    /// `boot::store_status`).
    pub const STORE_STATUS: &str = "store.status";
    /// Test maps (docs/game/test-maps.md, [`crate::test_maps`]): the
    /// built-in recipes and the test maps built.
    pub const MAPS_TEST_LIST: &str = "maps.test.list";
    /// Build a test map from a recipe, in the background.
    pub const MAPS_TEST_CREATE: &str = "maps.test.create";
    /// Queue a built test map as the next game.
    pub const MAPS_TEST_LOAD: &str = "maps.test.load";
    /// Delete a test map's package.
    pub const MAPS_TEST_REMOVE: &str = "maps.test.remove";
    /// The fallback interpreter's entry addresses, by instructions
    /// interpreted (`{"top": 20}`): where generated code still stops.
    pub const PROFILE_FALLBACKS: &str = "profile.fallbacks";
    /// The flat RAM window's counters (`runtime::window`): fixups taken,
    /// mapped pages outside the window, bytes the congruent allocator
    /// skipped; where a slow page shows.
    pub const PROFILE_MEMORY: &str = "profile.memory";
    /// The pages outside the flat RAM window by reason and owner, and the
    /// slow-access census (`{"top": 20, "census": true}`): the guest pages
    /// generated code reached the checked helpers for, and why
    /// (`runtime::slow_pages`).
    pub const PROFILE_MEMORY_PAGES: &str = "profile.memory.pages";
    /// The fault census (`{"top": 20}`): every guest memory fault, who
    /// raised it, and whether guest execution went on from its state
    /// (`runtime::fault_census`).
    pub const PROFILE_FAULTS: &str = "profile.faults";
    /// The menu screen player one's input goes to (`H2_UI_WATCH=1`,
    /// `boot::ui_watch`): `{"screen", "window", "slot", "live_menu_row",
    /// "mode", "dialog", "age_ms"}`; `dialog` is the first screen on windows
    /// 0 to 4 (`{"id", "name", "window", "slot"}`) or `null`.
    pub const UI_CAMPAIGN: &str = "ui.campaign";
    /// The pregame session's player slots, peer indices, names and connection bars.
    pub const UI_LOBBY_PLAYERS: &str = "ui.lobby.players";
    /// The pregame lobby's modes: `{place, leader, selected, header,
    /// clanker, tabs: [{tab, label, state, reason}], notice}`
    /// (docs/game/party-lobby.md).
    pub const UI_LOBBY_TABS: &str = "ui.lobby.tabs";
    pub const UI_SCREEN: &str = "ui.screen";
    /// Active filters of the Live Custom Game Browser.
    pub const UI_BROWSER_FILTERS: &str = "ui.browser.filters";
    pub const UI_INPUT: &str = "ui.input";
    /// Main-menu attract movie, idle timer and last attempt.
    pub const UI_ATTRACT: &str = "ui.attract";
    /// The visible option lines in the active pregame custom lobby.
    pub const UI_LOBBY_OPTIONS: &str = "ui.lobby_options";
    /// Active widget panes, resolved text and audit findings at this boundary.
    pub const UI_AUDIT: &str = "ui.audit";
    /// Deliver one menu action guarded by the input owner and focus from `ui.audit`.
    pub const UI_PRESS: &str = "ui.press";
    /// Live screen channels and the generic Back target, read at a game boundary.
    pub const UI_STACK: &str = "ui.stack";
    /// Record the next whole frame's passes, draws and targets, or read
    /// the last one recorded ([`crate::FrameParams`]).
    pub const RENDER_FRAME: &str = "render.frame";
    /// Record the next frame's display layout and sampled surfaces, or read
    /// the same last frame as `render.frame` ([`crate::FrameParams`]).
    pub const RENDER_FRAME_LAYOUT: &str = "render.frame_layout";
    /// The render pipeline's counters: in-between frames and their hold
    /// reasons, the frame-stats counters ([`crate::RenderStats`]).
    pub const RENDER_STATS: &str = "render.stats";
    /// Where the window drew the world marks on its last presents: the
    /// frame kind, a made frame's blend fraction and camera shift, and the
    /// first anchors' window pixels (no params; the first call arms it).
    pub const RENDER_MARKS: &str = "render.marks";
    /// The game type markers the last present drew, and what the 256
    /// budget left out ([`crate::MarkersParams`]).
    pub const RENDER_MARKERS: &str = "render.markers";
    /// Two PNG files compared pixel by pixel ([`crate::CompareParams`]).
    pub const RENDER_COMPARE: &str = "render.compare";
    /// The frame of a sim tick as a PNG ([`crate::FrameCaptureParams`]).
    pub const RENDER_CAPTURE: &str = "render.capture";
    /// Position the world camera at a loaded player starting location.
    pub const RENDER_SPAWN_VIEW: &str = "render.spawn_view";
    /// Position the world camera at an explicit eye and heading, or release
    /// it ([`crate::ViewParams`]).
    pub const RENDER_VIEW: &str = "render.view";
    /// The pose of the view the game draws for a local player, in
    /// `render.view`'s conventions ([`crate::CurrentViewParams`]).
    pub const RENDER_VIEW_CURRENT: &str = "render.view.current";
    /// Arm a sequence of headless display presents, including in-between frames.
    pub const RENDER_CAPTURE_CLIP: &str = "render.capture_clip";
    /// Read the number of display frames written by a clip capture.
    pub const RENDER_CAPTURE_CLIP_STATUS: &str = "render.capture_clip.status";
    /// The last sealed `FrameScene`'s counts ([`crate::SceneParams`]).
    pub const RENDER_SCENE: &str = "render.scene";
    /// What the guest front-end still asks `d3d` to do, by caller and by
    /// producer, over a window of presents ([`crate::ResidualParams`]).
    pub const RENDER_RESIDUAL: &str = "render.residual";
    /// The native particle path's geometry refusals by named check, each
    /// with the first refused draw's shape (no params).
    pub const RENDER_PARTICLES_REFUSALS: &str = "render.particles.refusals";
    /// Arm the in-process two-path comparison for one frame
    /// ([`crate::ParityParams`]); a diagnostic, deleted with the D3D
    /// backend.
    pub const RENDER_PARITY: &str = "render.parity";
    pub const COOP_LOOK: &str = "render.coop_look";
    /// Active mixer voices, bin gains and distance-dependent path parameters.
    pub const AUDIO_VOICES: &str = "audio.voices";
    /// Active guest listeners and their local-user mappings.
    pub const AUDIO_LISTENERS: &str = "audio.listeners";
    /// Active guest looping sounds, their transitions and host output health.
    pub const AUDIO_LOOPS: &str = "audio.loops";
    /// The host mixer's output level over a window, or of a host clip it
    /// starts ([`crate::audio::MeterParams`]).
    pub const AUDIO_METER: &str = "audio.meter";
    /// Sim mode's match (`H2_SIM`, `boot::sim_glue`): the match file, the
    /// phase (`waiting`, `playing`, `over`), ticks, game and wall seconds,
    /// the speed-up so far, frames skipped, and the end once over.
    pub const MATCH_STATUS: &str = "match.status";
    /// Sim mode: the match report as it would be written now
    /// (`h2-sim-report/1`, docs/runtime/sim.md §6).
    pub const MATCH_REPORT: &str = "match.report";
    /// Sim mode: end the match at the next slice boundary, as `stopped`.
    pub const MATCH_STOP: &str = "match.stop";
    /// Sim mode: the guest census (local users, the local visibility set,
    /// the effect, particle, decal and light arrays' live counts) read now,
    /// and the report's `guest_census` block so far (docs/runtime/sim.md §6).
    pub const SIM_GUEST_CENSUS: &str = "sim.guest_census";
}

/// Error codes. The negative ones are JSON-RPC's own.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// A control method from a connection that doesn't hold control.
    pub const NOT_CONTROLLER: i64 = 1;
    /// `session.wait` (or `session.step`) ran out of time first.
    pub const TIMEOUT: i64 = 2;
    /// The object an id named is no longer in the world (or its slot now
    /// holds a different object).
    pub const GONE: i64 = 3;
    /// The game can't answer this now: no game loaded, a read faulted,
    /// frame stats off.
    pub const UNAVAILABLE: i64 = 4;
    /// The server is shutting down.
    pub const SHUTDOWN: i64 = 5;
    /// A `session.step` ended early: a resume, a new pause, or control
    /// released before its ticks ran.
    pub const INTERRUPTED: i64 = 6;
    /// `state.save` or `state.load` never found a quiescent slice boundary
    /// in the ticks it had; `data` is a [`crate::state::Busy`] naming what
    /// stayed live.
    pub const BUSY: i64 = 7;
    /// A conditional tag batch disagreed with the loaded bytes.
    pub const MISMATCH: i64 = 8;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "two")]
    pub jsonrpc: String,
    /// Absent: a notification, which gets no response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

fn two() -> String {
    "2.0".into()
}

impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Request {
        Request {
            jsonrpc: two(),
            id: Some(Value::from(id)),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Response {
        Response {
            jsonrpc: two(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Value, error: RpcError) -> Response {
        Response {
            jsonrpc: two(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A server-to-client notification (`method: "event"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Event,
}

impl Notification {
    pub fn event(event: Event) -> Notification {
        Notification {
            jsonrpc: two(),
            method: "event".into(),
            params: event,
        }
    }
}

/// Any line the server sends: a response or an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerLine {
    Response(Response),
    Notification(Notification),
}

/// What the game is doing, as far as the server can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No game tick for a while: a map loading, or the game not started.
    Loading,
    /// The main menu (its scenario is running).
    Menu,
    /// A campaign level or a multiplayer game, outside cutscenes.
    Gameplay,
    /// Between HaloScript's `cinematic_start` and `cinematic_stop`.
    Cinematic,
}

/// Where in the run an answer was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    /// Right after a game tick returned: one consistent simulation state.
    Tick,
    /// Between scheduler slices, with no tick for a while (loading, or a
    /// game that doesn't tick). Guest memory may be mid-update.
    Slice,
}

/// The stamp on every result: which moment it describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct At {
    /// The game's own tick count (`None` when no game state is readable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// Ticks the server has seen since the game started; never resets.
    pub ticks: u64,
    /// Identity of the last observed full game start. Zero until one is observed.
    #[serde(default)]
    pub activation: u64,
    /// Ticks since the current map loaded. A level's scripts start its
    /// opening cutscene in its first ticks, so "in control" is better
    /// asked as `{"phase":"gameplay","map_ticks":30}` than as the phase
    /// alone.
    pub map_ticks: u64,
    /// Frames presented so far.
    pub present: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// `campaign`, `multiplayer`, `main_menu`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp: Option<u32>,
    pub phase: Phase,
    pub paused: bool,
    pub boundary: Boundary,
}

// ---- session ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub at: At,
    pub protocol: u32,
    /// Frames per second over the frame-stats window, when collected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Whether this connection holds control.
    pub controller: bool,
    /// Whether any connection holds control.
    pub claimed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseParams {
    /// Resume by itself after this long (default 60 s, at most 600 s), so
    /// a forgotten pause can't hang the game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepParams {
    /// Ticks to run before pausing again (1 to 1800).
    pub ticks: u64,
}

/// What `session.wait` waits for. Every field given must hold at once;
/// at least one must be given.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Until {
    /// The exact full game start to observe, as returned by map.reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<u64>,
    /// The game's own tick is at least this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// This many more ticks have passed since the wait began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    /// At least this many ticks since the current map loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_ticks: Option<u64>,
    /// The present count is at least this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// The loaded map's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
}

impl Until {
    pub fn is_empty(&self) -> bool {
        *self == Until::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitParams {
    pub until: Until,
    /// Give up after this many ticks (default: no tick limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ticks: Option<u64>,
    /// Give up after this long (default 30 s, at most 600 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waited {
    pub at: At,
    /// Ticks that passed while waiting.
    pub ticks: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Phase,
    Map,
    Bsp,
    Pause,
    /// `render.picture`'s per-frame events. They go to the connection that
    /// opened the picture; subscribing adds nothing.
    Picture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscribeParams {
    pub events: Vec<EventKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Phase {
        from: Phase,
        to: Phase,
        at: At,
    },
    Map {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        at: At,
    },
    Bsp {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<u32>,
        at: At,
    },
    Paused {
        at: At,
    },
    Resumed {
        at: At,
        /// `request`, `timeout` (the pause's `max_ms`) or `released`
        /// (the controller let go or disconnected).
        why: String,
    },
    /// A ring picture published a frame (`render.picture.open`).
    Picture(picture::PictureEvent),
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Phase { .. } => EventKind::Phase,
            Event::Map { .. } => EventKind::Map,
            Event::Bsp { .. } => EventKind::Bsp,
            Event::Paused { .. } | Event::Resumed { .. } => EventKind::Pause,
            Event::Picture(_) => EventKind::Picture,
        }
    }
}

// ---- world ----

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A player (their unit's state; the id is the player's index).
    #[default]
    Player,
    /// An AI-controlled biped.
    Actor,
    Vehicle,
    /// A door, elevator, lift or other device machine.
    Machine,
    /// A dead biped not cleared away yet.
    Body,
    /// Scenery (object type 6): rocks, trees, teleporter bases.
    Scenery,
    /// A weapon lying in the world (type 2); held weapons are their
    /// unit's `weapon`, not listed.
    Weapon,
    /// Equipment lying in the world (type 3): grenades, powerups.
    Equipment,
    /// A device control (type 8): a switch or panel.
    Control,
    /// A crate (type 11).
    Crate,
    /// A creature (type 12).
    Creature,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Player,
        Kind::Actor,
        Kind::Vehicle,
        Kind::Machine,
        Kind::Body,
        Kind::Scenery,
        Kind::Weapon,
        Kind::Equipment,
        Kind::Control,
        Kind::Crate,
        Kind::Creature,
    ];

    /// The kind objects of engine object type `t` (the object header's
    /// `+3`) are listed as, for the types the reader lists without a scan
    /// of its own (units, vehicles and machines have theirs).
    pub fn of_object_type(t: u8) -> Option<Kind> {
        Some(match t {
            2 => Kind::Weapon,
            3 => Kind::Equipment,
            6 => Kind::Scenery,
            8 => Kind::Control,
            11 => Kind::Crate,
            12 => Kind::Creature,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Player => "player",
            Kind::Actor => "actor",
            Kind::Vehicle => "vehicle",
            Kind::Machine => "machine",
            Kind::Body => "body",
            Kind::Scenery => "scenery",
            Kind::Weapon => "weapon",
            Kind::Equipment => "equipment",
            Kind::Control => "control",
            Kind::Crate => "crate",
            Kind::Creature => "creature",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// An object id: `player.<index>`, or `<kind>.<datum>` with the datum
/// handle as 8 hex digits (its salt in the top half), so an id never names
/// a different object that later reuses the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub kind: Kind,
    /// The player index for players; the object datum handle otherwise.
    pub key: u32,
}

impl ObjectId {
    pub fn parse(s: &str) -> Option<ObjectId> {
        let (kind, key) = s.split_once('.')?;
        let kind = Kind::parse(kind)?;
        let key = if kind == Kind::Player {
            key.parse().ok()?
        } else {
            if key.len() != 8 {
                return None;
            }
            u32::from_str_radix(key, 16).ok()?
        };
        Some(ObjectId { kind, key })
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            Kind::Player => write!(f, "player.{}", self.key),
            k => write!(f, "{}.{:08x}", k.as_str(), self.key),
        }
    }
}

/// One object in the world. Fields the read didn't produce are left out.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 3]>,
    /// Where it faces. For a unit (a player's, an actor, a body) this is
    /// where it aims: the unit's aiming vector (unit `+0x168`, the vector
    /// the held weapon's targeting ray follows, `0x000cf3d0`), so it turns
    /// with the camera; a biped's body turns only as it moves, so its own
    /// forward is `body_forward`. Others: the object's forward (`+0x70`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<[f64; 3]>,
    /// Units only: the object's own forward (`+0x70`), the body's facing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_forward: Option<[f64; 3]>,
    /// `forward` as degrees about +z from +x, 0 to 360.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    /// `forward` as degrees up from level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// The definition tag's datum handle, as 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The definition tag's name (`scenery\teleporter_base\teleporter_base`),
    /// from the loaded map's file; absent when the map's names can't be
    /// read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_name: Option<String>,
    /// The vehicle it rides, as an object id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
    /// The held weapon's definition tag datum, as 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// Players only: the gamertag or profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Players only: the local controller slot, absent for remote players.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_user: Option<u8>,
    /// Players only: the unit they control, as an object datum (8 hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Vehicles only: the vehicle it is mounted on (a turret), as an
    /// object id. Its `pos` and `forward` are then relative to that
    /// vehicle, not the world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounted_on: Option<String>,
    /// Vehicles only: someone is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied: Option<bool>,
    /// Machines, scenery, items, controls and crates: the scenario
    /// placement index, when a placement made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<u16>,
    /// Machines only: the device position, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    /// Distance from the `near` point, when the query had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    /// Units only: crouch and character-physics state, for tracing
    /// movement bugs tick by tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<Physics>,
}

/// A unit's crouch and character-physics state as the engine holds it.
/// Raw flag words are kept whole so a trace can read bits not named here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Physics {
    /// Crouch amount, 0 standing to 1 crouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crouch: Option<f64>,
    /// The crouch request (unit flags bit 23): set while crouch is held,
    /// and kept set after release until there is room to stand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crouch_request: Option<bool>,
    /// Biped physics mode: 1 ground, 2 flying, 3 dead, 4 posture,
    /// 5 climbing, 6 melee (names likely).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u8>,
    /// Bipeds: biped flags bit 0 (likely airborne).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airborne: Option<bool>,
    /// Bipeds: ticks `airborne` has been set (capped at 127).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airborne_ticks: Option<i8>,
    /// Raw unit flags, 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_flags: Option<String>,
    /// Bipeds: raw biped flags, 4 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biped_flags: Option<String>,
    /// Bipeds: the hitbox offset counter, ticks (a melee lunge sets it;
    /// it steps toward 0 each tick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hitbox_offset_ticks: Option<i8>,
    /// Bipeds: how far the Havok body sits from the origin in z because
    /// of that counter, world units (the body is at origin z minus this).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hitbox_offset: Option<f64>,
    /// The object's first Havok rigid body: where the physics has it,
    /// which can differ from `pos` (where it is drawn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub havok_pos: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub havok_velocity: Option<[f64; 3]>,
    /// Bipeds: the ground support the physics update keeps and the
    /// character contacts its ground test walks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<Support>,
    /// Bipeds: whether a standing capsule fits at the origin, the test
    /// that decides whether releasing crouch clears the crouch request.
    /// The superbounce mode's capsule test against the loaded map's
    /// collision, while that mode is on and its scan is ready; absent
    /// otherwise (docs/game/superbounce.md, "Charge").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_fits: Option<bool>,
}

/// A biped's ground support as the engine keeps it (biped `+0x349`,
/// `+0x354..+0x388`, `+0x3ec`) and its physics component's character
/// contacts (component `+0x88`, `0x48` each). Raw words stay whole: what
/// most of them mean is not traced yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Support {
    /// Biped `+0x349` (the high byte of the biped flags at `+0x348`), 2 hex
    /// digits; bit 4 (`10`, biped flags bit 12) is set when this tick's
    /// update copied a support contact.
    pub flags: String,
    /// Bit 4 of `flags`.
    pub copied: bool,
    /// Biped `+0x354..+0x364`, 8 hex digits each: what supports the biped
    /// (all `ffffffff` when nothing does).
    pub words: Vec<String>,
    /// Biped `+0x368`.
    pub point: [f64; 3],
    /// Biped `+0x374`: the support contact's normal.
    pub normal: [f64; 3],
    /// Biped `+0x380`: the support contact's point.
    pub contact_point: [f64; 3],
    /// Biped `+0x3ec`: the ground mode's state vector.
    pub ground_state: [f64; 3],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contacts: Vec<CharacterContact>,
}

/// One character contact of a biped's physics component.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterContact {
    /// `+0x04`, 8 hex digits: the shape key (`key >> 29` its type; 1 a
    /// structure surface, index in the low 16 bits).
    pub key: String,
    /// `+0x14`, 8 hex digits.
    pub other: String,
    /// `+0x1c`.
    pub point: [f64; 3],
    /// `+0x28`.
    pub normal: [f64; 3],
    /// `+0x38`: the first pass of the ground test takes the contact only
    /// when its magnitude is at least 1e-4.
    pub value: f64,
    /// `+0x44`, 8 hex digits.
    pub flags: String,
}

/// Field names a query's `fields` may keep (`id` and `kind` are always
/// kept).
pub const OBJECT_FIELDS: &[&str] = &[
    "pos",
    "forward",
    "body_forward",
    "yaw",
    "pitch",
    "velocity",
    "team",
    "tag",
    "tag_name",
    "vehicle",
    "weapon",
    "name",
    "local_user",
    "unit",
    "mounted_on",
    "occupied",
    "placement",
    "position",
    "distance",
    "physics",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Near {
    /// Measure from this point…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f64; 3]>,
    /// …or from this player's unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<u32>,
    /// World units.
    pub radius: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryParams {
    /// Kinds to list (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<Kind>>,
    /// Only objects within a radius; results are then sorted nearest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near: Option<Near>,
    /// Only objects whose definition tag is this: a datum (8 hex digits,
    /// `0x` optional), or text the tag's name contains (any case, `/` for
    /// `\`). A name matches nothing when the map's names can't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Keep only these fields of each object (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    /// Page size (default 50, at most 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

impl QueryParams {
    pub const DEFAULT_LIMIT: usize = 50;
    pub const MAX_LIMIT: usize = 500;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub at: At,
    /// Matches before paging.
    pub total: usize,
    pub offset: usize,
    /// Objects, trimmed to `fields` when given.
    pub objects: Vec<Value>,
    /// More matches after this page.
    pub more: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    #[default]
    Kind,
    Team,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountParams {
    #[serde(default)]
    pub group_by: GroupBy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub at: At,
    pub total: usize,
    pub groups: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectResult {
    pub at: At,
    pub object: Object,
}

// ---- profile ----

/// The frame-stats rolling window (docs/perf/notes/gameplay-budget.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counters {
    pub at: At,
    pub frames: usize,
    pub fps: f64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Mean ms per frame in each phase (`cpu`, `render`, `gpu-wait`, …).
    pub phase_ms: BTreeMap<String, f64>,
    /// Mean per frame of each counter (`draws`, `uploads`, …).
    pub counts: BTreeMap<String, f64>,
}

/// `profile.capture`: every frame over a window, by game ticks or wall
/// time (one of them).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureParams {
    /// Game ticks to capture (1 to 36000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    /// Milliseconds to capture (1 to 600000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// A frame longer than this is a spike (default 33.3, 30 fps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<f64>,
    /// List at most this many spikes, worst first (default 20, at most
    /// 500). `over_budget` counts them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spikes: Option<usize>,
    /// Also run the sampling profiler over the window (an h2 built with
    /// `--features perf` only).
    #[serde(default)]
    pub sample: bool,
}

impl CaptureParams {
    pub const MAX_TICKS: u64 = 36_000;
    pub const MAX_MS: u64 = 600_000;
    pub const DEFAULT_BUDGET_MS: f64 = 33.3;
    pub const DEFAULT_SPIKES: usize = 20;
    pub const MAX_SPIKES: usize = 500;
}

/// Milliseconds per frame over a capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseStats {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    /// Summed over every frame.
    pub total: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CountStats {
    /// Per frame.
    pub mean: f64,
    pub max: u64,
    pub total: u64,
}

/// A frame over the budget, with its own breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spike {
    /// The present that ended it (frame stats count frames by present).
    pub present: u64,
    /// The game tick it fell in, and the map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Wall time, and wall time less pacing.
    pub ms: f64,
    pub busy_ms: f64,
    /// Each phase it spent time in, ms.
    pub phases: BTreeMap<String, f64>,
    /// Each counter it counted.
    pub counts: BTreeMap<String, u64>,
    /// Its biggest phases, biggest first (pacing left out).
    pub top: Vec<String>,
    /// Why it was slow, as far as its counters and phases say:
    /// `shader_compile`, `pipeline_compile`, `texture_uploads`, `file_io`,
    /// `gpu_readback`, `gpu`, `render_thread`, `pushbuf_decode`,
    /// `guest_cpu` (see docs/runtime/agent-api.md, phase 5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<String>,
}

/// What a capture's frames say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureStats {
    pub frames: usize,
    pub wall_ms: f64,
    pub fps: f64,
    /// Whole-frame wall time.
    pub frame_ms: PhaseStats,
    /// Wall time less pacing: what frames cost.
    pub busy_ms: PhaseStats,
    /// Per phase (`cpu`, `decode`, `encode`, `submit`, `gpu-wait`, `pace`,
    /// `render-wait`, `d3d`, and `render` when measured).
    pub phases: BTreeMap<String, PhaseStats>,
    /// Per counter (`draws`, `passes`, `uploads`, …).
    pub counts: BTreeMap<String, CountStats>,
    pub budget_ms: f64,
    /// Frames over the budget; `spikes` lists the worst of them.
    pub over_budget: usize,
    pub spikes: Vec<Spike>,
    /// Every frame over the budget (listed or not) counted by each of its
    /// causes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub causes: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub at: At,
    /// Game ticks the window spanned.
    pub ticks: u64,
    /// False when a capture by ticks ran out of time (600 s) first.
    pub complete: bool,
    #[serde(flatten)]
    pub stats: CaptureStats,
    /// Every frame as a CSV row (present, end, tick, map, then each phase
    /// in us and each counter).
    pub csv: String,
    /// Present when `sample` was asked for: what the sampler answered when
    /// the window ended (`profile.sample.stop`'s answer: the dump's `file`,
    /// `samples`, ...), or `{"unavailable": why}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Value>,
}

// ---- input ----

/// Controller ports (players) a pad step can name.
pub const PORTS: u32 = 4;

/// `input.record`: start logging every controller read the game makes
/// to the agent output directory's `inputs/<name>.input` (an `xapi::tap` input
/// log, the format `H2_INPUT_RECORD` writes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordParams {
    pub name: String,
    /// Overwrite a log of that name (default: refuse).
    #[serde(default)]
    pub replace: bool,
}

/// `input.replay`: answer the game's controller reads from a log, read
/// for read, from now on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayParams {
    pub name: String,
}

/// Whether a log name is safe as a file name: letters, digits, `.`, `_`
/// and `-`, at most 64, not starting with `.`.
pub fn valid_log_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// What a replay has done so far (`xapi::tap::ReplayStats`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCounts {
    /// Reads in the log.
    pub reads: u64,
    /// Reads answered from the log.
    pub replayed: u64,
    /// Of those, reads of the same controller handle as in the recording.
    pub matched: u64,
    /// Reads of a different handle: the runs opened their controllers
    /// differently, so they have already parted.
    pub handle_mismatches: u64,
    /// Reads after the log ran out, left live.
    pub past_end: u64,
}

/// What the input tap is doing besides pad packets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum InputMode {
    Live,
    Recording {
        name: String,
        path: String,
        /// Reads logged so far.
        reads: u64,
        /// The game tick it started at.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_tick: Option<u64>,
    },
    Replaying {
        name: String,
        path: String,
        #[serde(flatten)]
        counts: ReplayCounts,
    },
}

// ---- features ----

/// One feature of the plan, as `features.list`'s `features` lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureState {
    pub id: String,
    pub on: bool,
    /// Why it is off, when the plan turned it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl FeatureState {
    /// The plan text's line for it (`hooks::Plan::text`'s form):
    /// `feature <id>: on` or `feature <id>: off (<reason>)`.
    pub fn plan_line(&self) -> String {
        match (&self.reason, self.on) {
            (_, true) => format!("feature {}: on", self.id),
            (Some(r), false) => format!("feature {}: off ({r})", self.id),
            (None, false) => format!("feature {}: off", self.id),
        }
    }

    /// [`plan_line`](Self::plan_line) read back; `None` for any other line
    /// of the plan (a chain, an entry, a note).
    pub fn from_plan_line(line: &str) -> Option<FeatureState> {
        let (id, state) = line.strip_prefix("feature ")?.split_once(": ")?;
        if id.is_empty() || id.contains(char::is_whitespace) {
            return None;
        }
        let (on, reason) = match state {
            "on" => (true, None),
            "off" => (false, None),
            s => {
                let r = s.strip_prefix("off (")?.strip_suffix(')')?;
                (false, Some(r.to_owned()))
            }
        };
        Some(FeatureState {
            id: id.to_owned(),
            on,
            reason,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Features {
    pub at: At,
    /// The feature plan as the game printed it at start.
    pub plan: Vec<String>,
    /// Each feature, on or off and why: the plan's feature lines, read.
    #[serde(default)]
    pub features: Vec<FeatureState>,
}

impl Features {
    /// The answer for `plan`, its features read from its lines.
    pub fn new(at: At, plan: Vec<String>) -> Features {
        let features = features_of(&plan);
        Features { at, plan, features }
    }
}

/// The features a plan's lines name ([`FeatureState::from_plan_line`]).
pub fn features_of(plan: &[String]) -> Vec<FeatureState> {
    plan.iter()
        .filter_map(|l| FeatureState::from_plan_line(l))
        .collect()
}

/// `world.breakable`'s params: the structure BSP (default 0, the one the
/// bots' map data is).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakableParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp: Option<u32>,
}

/// `world.breakable`'s result: the BSP's breakable-surface state
/// (`bots::guest::breakable`, docs/game/bots.md, "Breaking the glass").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Breakable {
    pub bsp: u32,
    /// The enable byte `breakable_surfaces_enable` writes.
    pub enabled: bool,
    /// The breakable indices whose surfaces are broken, ascending; the
    /// other 256 minus these are whole.
    pub broken: Vec<u8>,
    /// The mask as the game holds it: 32 bytes, lowest index first, as
    /// hexadecimal (a set bit is a whole surface).
    pub mask: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn features_list_reads_the_plans_feature_lines_and_nothing_else() {
        let states = [
            FeatureState { id: "cheats".into(), on: true, reason: None },
            FeatureState {
                id: "lockstep".into(),
                on: false,
                reason: Some("switched off (H2_FEATURES)".into()),
            },
            FeatureState { id: "x".into(), on: false, reason: None },
        ];
        let mut plan: Vec<String> = states.iter().map(FeatureState::plan_line).collect();
        assert_eq!(plan[0], "feature cheats: on");
        assert_eq!(plan[1], "feature lockstep: off (switched off (H2_FEATURES))");
        plan.push("0x00137fe0 game_tick: before [cheats:our cheats] -> original".into());
        plan.push("0x00400000 entry agent:probe".into());
        plan.push("featured note: not a feature line".into());
        plan.push("feature two words: on".into());
        let features = features_of(&plan);
        assert_eq!(features, states);
        let v = serde_json::to_value(&features).unwrap();
        assert_eq!(v[1], json!({"id": "lockstep", "on": false, "reason": "switched off (H2_FEATURES)"}));
        assert_eq!(v[0], json!({"id": "cheats", "on": true}));
        for bad in ["feature : on", "feature a: maybe", "feature a: off (unclosed"] {
            assert_eq!(FeatureState::from_plan_line(bad), None, "{bad}");
        }
    }

    #[test]
    fn breakable_params_default_to_bsp_0_and_refuse_strays() {
        let p: BreakableParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p.bsp, None);
        let p: BreakableParams = serde_json::from_value(json!({"bsp": 1})).unwrap();
        assert_eq!(p.bsp, Some(1));
        assert!(serde_json::from_value::<BreakableParams>(json!({"bps": 1})).is_err());
        let r = Breakable {
            bsp: 0,
            enabled: true,
            broken: vec![3],
            mask: "f7".into(),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"bsp": 0, "enabled": true, "broken": [3], "mask": "f7"})
        );
    }

    #[test]
    fn agent_output_directory_uses_the_override_or_the_cache() {
        let run_dir = std::path::PathBuf::from("runs/0007");
        assert_eq!(output_dir_from(Some(run_dir.clone().into_os_string())), run_dir);
        assert_eq!(output_dir_from(None), cache_dir());
        assert_eq!(output_dir_from(Some("".into())), cache_dir());
    }

    #[test]
    fn ids_round_trip_and_keep_the_salt() {
        let id = ObjectId {
            kind: Kind::Actor,
            key: 0xe123_0045,
        };
        assert_eq!(id.to_string(), "actor.e1230045");
        assert_eq!(ObjectId::parse("actor.e1230045"), Some(id));
        let p = ObjectId::parse("player.2").unwrap();
        assert_eq!((p.kind, p.key), (Kind::Player, 2));
        assert_eq!(p.to_string(), "player.2");
    }

    #[test]
    fn bad_ids_are_refused() {
        for s in [
            "actor",
            "actor.45",
            "actor.zzzzzzzz",
            "ghost.e1230045",
            "player.x",
            "",
        ] {
            assert_eq!(ObjectId::parse(s), None, "{s}");
        }
    }

    #[test]
    fn a_request_without_jsonrpc_or_params_parses() {
        let r: Request = serde_json::from_str(r#"{"id":1,"method":"session.status"}"#).unwrap();
        assert_eq!(r.jsonrpc, "2.0");
        assert_eq!(r.params, Value::Null);
        assert_eq!(r.id, Some(json!(1)));
    }

    #[test]
    fn events_are_tagged_by_kind() {
        let at = At {
            tick: Some(5),
            ticks: 5,
            activation: 1,
            map_ticks: 5,
            present: 9,
            map: None,
            mode: None,
            bsp: None,
            phase: Phase::Menu,
            paused: false,
            boundary: Boundary::Tick,
        };
        let e = Event::Phase {
            from: Phase::Loading,
            to: Phase::Menu,
            at,
        };
        let v = serde_json::to_value(Notification::event(e.clone())).unwrap();
        assert_eq!(v["method"], "event");
        assert_eq!(v["params"]["kind"], "phase");
        assert_eq!(v["params"]["to"], "menu");
        let line: ServerLine = serde_json::from_value(v).unwrap();
        assert_eq!(line, ServerLine::Notification(Notification::event(e)));
    }

    #[test]
    fn responses_and_events_are_told_apart() {
        let r = Response::ok(json!(3), json!({"x": 1}));
        let line: ServerLine = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(line, ServerLine::Response(r));
    }

    #[test]
    fn empty_until_is_detected() {
        assert!(Until::default().is_empty());
        assert!(
            !Until {
                ticks: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn log_names_stay_file_names() {
        for ok in ["courtyard", "03a.fight-2", "A_b"] {
            assert!(valid_log_name(ok), "{ok}");
        }
        for bad in ["", "../x", "a/b", ".hidden", "sp ace", &"x".repeat(65)] {
            assert!(!valid_log_name(bad), "{bad}");
        }
    }

    #[test]
    fn input_modes_are_tagged_and_flat() {
        let v = serde_json::to_value(InputMode::Replaying {
            name: "a".into(),
            path: "/c/a.input".into(),
            counts: ReplayCounts {
                reads: 3,
                replayed: 2,
                matched: 2,
                ..Default::default()
            },
        })
        .unwrap();
        assert_eq!(v["mode"], "replaying");
        assert_eq!(v["matched"], 2);
    }

    #[test]
    fn every_control_method_is_listed() {
        for m in method::CONTROL {
            assert!(method::ALL.contains(m));
        }
    }
}
