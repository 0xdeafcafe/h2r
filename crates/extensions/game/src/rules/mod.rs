//! Rules scripts in the running game (docs/game/rules-engine.md, "The host
//! runtime").
//!
//! `guest` holds the reads and engine calls behind the script's API
//! entries, with `roles` (`role_traits`) and `objectives` (the objective
//! set, its carriers, `reset_objective`); `runtime` is the `rules::Game`
//! over them and one game's script and state; `scoring` keeps the carrier
//! engine's own scoring from running beside a script's; `feature` puts
//! the runtime on the feature plan and answers the agent API's
//! `rules.state`; `reload` answers `rules.reload`, in place or by a
//! restart; `header` answers what a script's header asks of the game
//! outside its rules (the superbounce mode, Tweakers).

pub mod feature;
pub mod guest;
pub mod header;
pub mod objectives;
pub mod reload;
pub mod roles;
pub mod runtime;
pub mod scoring;
