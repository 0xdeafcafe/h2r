//! Firefight in the running game (docs/game/firefight.md).
//!
//! `host` drives the director over a [`host::Game`]; `guest` holds the
//! guest bodies of its engine entries and the kit read; `runtime` is the
//! `Game` over those bodies and one match's state; `feature` puts the
//! runtime on the feature plan and answers the agent API's `firefight.*`;
//! `hud` is what the HUD shows, from the mode blob alone; `options` is the
//! Game Setup row and page that choose the match; `film` carries the host
//! log's entries in and out of films. Firefight is a mode of
//! the lobby (`crate::lobby_mode`).

pub mod feature;
pub mod film;
pub mod guest;
pub mod host;
pub mod hud;
pub mod options;
pub mod runtime;
