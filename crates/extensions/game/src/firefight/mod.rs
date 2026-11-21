//! Firefight in the running game (docs/game/firefight.md).
//!
//! `host` drives the director over a [`host::Game`]; `guest` holds the
//! guest bodies of its engine entries and the kit read.

pub mod guest;
pub mod host;
