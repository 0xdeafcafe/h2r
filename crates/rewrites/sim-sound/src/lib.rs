//! Sound simulation data and kernels independent of guest memory.
//! `sim-bridge` reads the live inputs from guest memory where a guest owner
//! remains.

#![forbid(unsafe_code)]

pub mod ambience;
pub mod chooser;
pub mod environment;
pub mod events;
pub mod feed;
pub mod impulse;
pub mod instances;
pub mod levels;
pub mod looping;
pub mod loops;
pub mod owners;
pub mod schedule;
pub mod scheduling;
pub mod segments;
pub mod sources;
pub mod spatial;
pub mod starts;
