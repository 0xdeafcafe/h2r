//! Non-invasive live game-state observation for the retail Halo 2 runtime.
//! Guest hooks always run the original function; transport is asynchronous.

#[cfg(test)]
mod bench;
mod config;
mod event;
pub mod havok;
pub mod havok_agents;
mod hooks;
mod impacts;
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
pub use hooks::{FilmObserver, Telemetry, TelemetryFeature, install, install_film};
/// The optional observer chains the loaded image can install before guest entry.
pub fn image_film_hooks(rt: &runtime::Runtime) -> live_proto::film::TelemetryHooks {
    use guest_layout::points::{DAMAGE_REPORT, OBJECT_DELETE, WEAPON_FIRE};
    live_proto::film::TelemetryHooks {
        weapon_fire: rt.resolve_original(WEAPON_FIRE.base).is_some(),
        damage_report: rt.resolve_original(DAMAGE_REPORT.base).is_some(),
        object_delete: rt.resolve_original(OBJECT_DELETE.base).is_some(),
        ..live_proto::film::TelemetryHooks::UNIFIED
    }
}

pub use layout::LIKELY_FIELDS;
pub use memory::{
    FilmSceneReader, ThingState, active_bsp, actors, film_scene, game_info, game_tick, hit, loaded_map_name, object,
    object_definition, players, rotation_axes, sim_snapshot, snapshot, team_scores, things, world, yaw,
};
pub use pipeline::Pipeline;
pub use session::{Bots, Playlist};
pub use sink::{JsonlSink, Sink};
pub use tags::{
    BotsLocator, HostedSessionLocator, MapFile, MapIdLocator, MapLocator, MapRead, OpenMap,
    PlaylistLocator, TagNames,
};
