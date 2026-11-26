//! Firefight definitions and admission rules for a Slayer match of one
//! player team, or two that also fight each other, against the AI.
//!
//! The definitions carry authored tuning and kit class names. [`script`]
//! loads the definition as data for the handwritten director, which turns
//! what the host observes into host commands
//! ([`ScriptDirector`]). See [the design](../../../../docs/game/firefight.md).
#![forbid(unsafe_code)]

pub mod builtin;
mod def;
pub mod director;
pub mod scoring;
pub mod script;
mod validate;
mod wire;

pub use content::Sha256;
pub use def::*;
pub use director::{Command, Observed, Outcome, PlayerKill, Settings};
pub use scoring::ScoreReason;
pub use script::{ScriptDirector, ScriptRefusal};
pub use validate::{HOST_ACTOR_CAPS, Limits, validate};

/// The Firefight map kit's version (`firefight_kit` writes it into the map
/// it builds, so it is part of the package's id). Session data names it
/// (`live_proto::session::Firefight::kit`) so every machine plays one kit.
pub const KIT_VERSION: u32 = 5;

/// The roles a kit gives a drop zone, in the zone's flags
/// (`scnr` zone `+0x20`, above the engine's own low bits). The kit writes
/// them, the guest reads them back (`firefight-kit`'s `Roles`,
/// `game::firefight::guest::read_kit`).
pub mod roles {
    /// First flag bit of the roles.
    pub const SHIFT: u32 = 8;
    /// A line squad's zone.
    pub const LINE: u8 = 1;
    /// A flanking squad's zone.
    pub const FLANK: u8 = 2;
    /// A heavy (boss wave) squad's zone.
    pub const HEAVY: u8 = 4;
    /// Every role: what a kit with no authored layout gives each zone.
    pub const ALL: u8 = LINE | FLANK | HEAVY;

    /// The roles as zone flag bits.
    pub fn pack(roles: u8) -> u32 {
        u32::from(roles & ALL) << SHIFT
    }

    /// The roles a zone's flags carry.
    pub fn unpack(flags: u32) -> u8 {
        ((flags >> SHIFT) as u8) & ALL
    }
}

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
    #[error("the kit cut class {0} to fit the map's tag headroom")]
    CutClass(String),
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
