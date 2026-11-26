//! The kits that ship with the game: authored layouts of ours that the
//! builder applies to the player's own disc, and [`ensure`], which builds
//! the packages on the player's machine when they are missing or stale
//! (docs/game/firefight.md, "Built-in kits").
mod ensure;

pub use ensure::{
    DiscMaps, MAPS, Outcome, Report, Target, ensure, ensure_with, pending, pending_with,
    refusal_line, targets,
};
