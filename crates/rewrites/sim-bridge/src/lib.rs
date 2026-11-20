//! The guest side of the native simulation: adapters from hook entries to
//! the sim crates, the installs under `impl.sim.native`, the tick driver
//! and the shadow checks of the ports (docs/engine/sim-rewrite.md).

pub mod shadow;
pub mod sim_frame;
pub mod sim_native;
pub mod sim_objects;
pub mod sim_physics;
pub mod sim_observer;
pub mod sim_static_light;
pub mod sim_submix;
