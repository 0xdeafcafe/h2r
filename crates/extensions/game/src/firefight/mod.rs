//! Firefight in the running game (docs/game/firefight.md).
//!
//! `host` drives the director over a [`host::Game`]; `guest` holds what is
//! already read and written on the guest directly.

pub mod guest;
pub mod host;
