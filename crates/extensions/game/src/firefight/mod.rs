//! Firefight in the running game (docs/game/firefight.md).
//!
//! `host` drives the director over a [`host::Game`]; `guest` holds the
//! guest bodies of its engine entries and the kit read; `runtime` is the
//! `Game` over those bodies and one match's state; `feature` puts the
//! runtime on the feature plan and answers the agent API's `firefight.*`.

pub mod feature;
pub mod guest;
pub mod host;
pub mod runtime;
