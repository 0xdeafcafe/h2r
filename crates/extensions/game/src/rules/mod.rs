//! Rules scripts in the running game (docs/game/rules-engine.md, "The host
//! runtime").
//!
//! `guest` holds the reads and engine calls behind the script's API
//! entries; `runtime` is the `rules::Game` over them and one game's script
//! and state; `feature` puts the runtime on the feature plan and answers
//! the agent API's `rules.state`; `header` answers what a script's header
//! asks of the game outside its rules (the superbounce mode, Tweakers).

pub mod feature;
pub mod guest;
pub mod header;
pub mod runtime;
