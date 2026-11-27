//! Sound simulation data and kernels independent of guest memory.
//! The engine adapters read the live inputs at each guest callback boundary.

#![forbid(unsafe_code)]

pub mod ambience;
pub mod channel;
pub mod chooser;
pub mod environment;
pub mod events;
pub mod feed;
pub mod impulse;
pub mod instances;
pub mod listener;
pub mod looping;
pub mod loops;
pub mod mix;
pub mod objects;
pub mod owners;
pub mod schedule;
pub mod scheduling;
pub mod segments;
pub mod sources;
pub mod spatial;
pub mod submix;
pub mod tick;
pub mod voices;
