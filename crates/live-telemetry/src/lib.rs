//! Non-invasive live game-state observation for the retail Halo 2 runtime.
//! Guest hooks always run the original function; transport is asynchronous.

#[cfg(test)]
mod bench;
mod config;
mod event;
pub mod havok;
pub mod havok_agents;
mod hooks;
pub mod layout;
mod memory;
mod motion;
mod pipeline;
pub mod raw;
mod server;
mod session;
mod sink;
mod tags;

pub use config::{HostToken, TelemetryAccount, TelemetryConfig, TelemetrySinkConfig};
pub use event::{
    ActorState, Appearance, CharacterContact, GameEvent, GameInfo, HitRecord, MachineState, Motion,
    ObjectState, PlayerState, PlayerStats, Shots, Snapshot, Support, VehicleState, Weapon, World,
};
pub use hooks::{Telemetry, TelemetryFeature, install};
pub use layout::LIKELY_FIELDS;
pub use memory::{
    ThingState, active_bsp, actors, game_info, game_tick, hit, loaded_map_name, object,
    object_definition, players, sim_snapshot, snapshot, team_scores, things, world, yaw,
};
pub use pipeline::Pipeline;
pub use session::{Bots, Playlist};
pub use sink::{JsonlSink, Sink};
pub use tags::{BotsLocator, MapFile, MapIdLocator, MapLocator, PlaylistLocator, TagNames};
