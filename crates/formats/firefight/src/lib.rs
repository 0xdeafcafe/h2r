//! Firefight definitions and admission rules for a one-team Slayer match.
//!
//! The definitions carry authored tuning and kit class names.
//! See [the design](../../../../docs/game/firefight.md).
#![forbid(unsafe_code)]

pub mod builtin;
mod def;
mod validate;
mod wire;

pub use content::Sha256;
pub use def::*;
pub use validate::{HOST_ACTOR_CAPS, Limits, validate};

pub type Result<T> = std::result::Result<T, Refusal>;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("Firefight format {0} is unsupported (this build reads {FORMAT})")]
    Format(u32),
    #[error("invalid Firefight JSON: {0}")]
    Json(String),
    #[error("{field} exceeds its limit of {max}")]
    Limit { field: &'static str, max: usize },
    #[error("{field}: {reason}")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[error("the kit does not provide class {0}")]
    UnknownClass(String),
    #[error("round {round}, wave {wave} needs {actors} actors for {players} players (cap {cap})")]
    WaveOverCap {
        /// One-based round number.
        round: usize,
        /// One-based wave number within the round.
        wave: usize,
        players: u8,
        actors: u32,
        cap: u16,
    },
}
