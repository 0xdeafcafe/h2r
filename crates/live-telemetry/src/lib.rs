//! Non-invasive live game-state observation for the retail Halo 2 runtime.
//! Guest hooks always run the original function; transport is asynchronous.

mod config;
mod event;
mod hooks;
mod layout;
mod memory;
mod motion;
mod server;
mod session;
mod sink;
mod tags;

pub use config::{TelemetryAccount, TelemetryConfig, TelemetrySinkConfig};
pub use event::{
    ActorState, Appearance, GameEvent, GameInfo, HitRecord, MachineState, Motion, ObjectState,
    PlayerState, PlayerStats, Snapshot, VehicleState, Weapon, World,
};
pub use hooks::{Telemetry, install};
pub use layout::LIKELY_FIELDS;
pub use memory::{
    active_bsp, actors, game_info, game_tick, hit, object, players, snapshot, team_scores, world,
    yaw,
};
pub use sink::{JsonlSink, QueuedSink, Sink};
pub use tags::{MapFile, MapLocator, TagNames};
