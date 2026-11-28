//! `sim.arms`: one read of every native-sim shadow and authority arm
//! (docs/runtime/agent-api.md, "sim.arms"; the gate read of
//! docs/engine/sim-rewrite.md section 3). Wire types only.

use serde::{Deserialize, Serialize};

/// Method names. Game method: `session.methods` lists it where the game
/// registered it (`agent::sim_arms::install`).
pub mod method {
    /// Every subsystem's arms (no params → [`super::SimArms`]).
    pub const ARMS: &str = "sim.arms";

    pub const ALL: &[&str] = &[ARMS];
}

/// `sim.arms`: one entry per native-sim subsystem, in a fixed order
/// (`physics`, `objects`, `anim`, `collision`, `seats`, `effects`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimArms {
    pub subsystems: Vec<Subsystem>,
}

/// One subsystem's arms and its gate verdict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Subsystem {
    /// The subsystem's name: `physics`, `objects`, `anim`, `collision`,
    /// `seats` or `effects`.
    pub name: String,
    /// The switch the subsystem runs under (`impl.sim.native.<name>`);
    /// empty for `effects-parts`, which always runs.
    pub switch: String,
    /// Whether the switch is on in this process.
    pub on: bool,
    pub arms: Vec<Arm>,
    /// `checked > 0 && mismatched == 0`, summed over the arms.
    pub clean: bool,
}

/// One arm's counters since start. A source with no such counter reports 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub name: String,
    /// Calls the arm compared against the guest.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    /// Calls the Rust side answered in the guest's place.
    pub answered: u64,
    /// Calls left to the guest unchecked, summed over reasons.
    pub declined: u64,
}
