//! Firefight in the running game (docs/game/firefight.md).
//!
//! `host` drives the director over a [`host::Game`]; `guest` holds the
//! guest bodies of its engine entries and the kit read; `runtime` is the
//! `Game` over those bodies and one match's state; `feature` puts the
//! runtime on the feature plan and answers the agent API's `firefight.*`;
//! `hud` is what the HUD shows, from the mode blob alone; `options` is the
//! Game Setup row and page that choose the match; `lobby` is the main menu's
//! FIREFIGHT lobby that starts one.

pub mod feature;
pub mod guest;
pub mod host;
pub mod hud;
pub mod lobby;
pub mod options;
pub mod runtime;
