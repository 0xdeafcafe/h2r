//! Rules scripts in the running game (docs/game/rules-engine.md, "The host
//! runtime").
//!
//! `guest` holds the reads and engine calls behind the script's API
//! entries, with `roles` (`role_traits`) and `objectives` (the objective
//! set, its carriers, `reset_objective`); `runtime` is the `rules::Game`
//! over them and one game's script and state; `feature` puts
//! the runtime on the feature plan and answers the agent API's
//! `rules.state`; `reload` answers `rules.reload`, in place or by a
//! restart; `header` answers what a script's header asks of the game
//! outside its rules (the superbounce mode, our game type rows); `census` reads the
//! map's labelled things, for the start gate and the script's label reads;
//! `gate` is the lobby's start gate, which holds Start Game on a map that
//! lacks what the variant's script needs; `record` answers the record
//! clean-up, the wire codec and the protocol map for the script index;
//! `base_hud` feeds the HUD kit's base HUD (score and clock) on the script
//! index; `engine_object` puts the script index's engine object in the engine
//! table and answers the spawn gather's jump on it.

pub mod base_hud;
pub mod census;
pub mod engine_object;
pub mod feature;
pub mod gate;
pub mod guest;
pub mod header;
pub mod objectives;
pub mod record;
pub mod reload;
pub mod roles;
pub mod runtime;
pub(crate) mod start_line;
/// The world API (docs/game/rules-world-api.md): bot orders, show cues,
/// zones and loadouts.
pub mod world;
