//! The guest side of the native simulation: adapters from hook entries to
//! the sim crates, the installs under `impl.sim.native`, the tick driver
//! and the shadow checks of the ports (docs/engine/sim-rewrite.md).

pub mod shadow;
pub mod sim_frame;
pub mod sim_native;
pub mod sim_effects;
pub mod sim_effects_parts;
pub mod effect_update;
pub mod sim_objects;
pub mod sim_sound;
pub mod sim_ext;
pub mod sim_physics;
pub mod sim_seats;
pub mod sim_lightmap;
pub mod sim_status_poll;
pub mod present;
pub mod tick;
pub mod unit_mouth;
