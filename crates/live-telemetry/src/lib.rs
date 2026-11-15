//! Non-invasive live game-state observation for the retail Halo 2 runtime.
//! Guest hooks always run the original function; transport is asynchronous.

mod config;
mod event;
mod hooks;
mod layout;
mod memory;
mod server;
mod session;
mod sink;

pub use config::{TelemetryAccount, TelemetryConfig, TelemetrySinkConfig};
pub use event::{GameEvent, GameInfo, ObjectState, PlayerState, PlayerStats, Snapshot};
pub use hooks::{Telemetry, install};
pub use layout::LIKELY_FIELDS;
pub use memory::{game_info, game_tick, object, players, snapshot, team_scores, yaw};
pub use sink::{JsonlSink, QueuedSink, Sink};
