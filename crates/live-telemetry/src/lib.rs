//! Non-invasive live game-state observation for the retail Halo 2 runtime.
//! Guest hooks always run the original function; transport is asynchronous.

#[cfg(test)]
mod bench;
mod config;
mod event;
mod hooks;
pub mod layout;
mod memory;
mod motion;
mod pipeline;
mod server;
mod session;
mod sink;
mod tags;

pub use config::{TelemetryAccount, TelemetryConfig, TelemetrySinkConfig};
pub use event::{
    ActorState, Appearance, GameEvent, GameInfo, HitRecord, MachineState, Motion, ObjectState,
    PlayerState, PlayerStats, Snapshot, VehicleState, Weapon, World,
};
pub use hooks::{Telemetry, TelemetryFeature, install};
pub use layout::LIKELY_FIELDS;
pub use memory::{
    active_bsp, actors, game_info, game_tick, hit, loaded_map_name, object, players, snapshot,
    team_scores, world, yaw,
};
pub use pipeline::Pipeline;
pub use sink::{JsonlSink, Sink};
pub use tags::{MapFile, MapIdLocator, MapLocator, TagNames};
