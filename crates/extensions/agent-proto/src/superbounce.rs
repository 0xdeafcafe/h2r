//! `superbounce.*`: the superbounce game type's state (docs/runtime/agent-api.md,
//! "superbounce"; docs/game/superbounce.md, "The superbounce game type").
//! Wire types only. A game method: `session.methods` lists it where the game
//! registered it (`game::agent_glue`).

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// The local player's primed state and the spot scan
    /// ([`super::State`]; no params).
    pub const STATE: &str = "superbounce.state";

    pub const ALL: &[&str] = &[STATE];
}

// ---- superbounce.state ----

/// The scan of the loaded map's spots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    /// `idle` (nothing asked for yet), `scanning`, `ready` or `failed`.
    pub status: String,
    /// The map the spots are for, once a scan has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Why a scan failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Bounce spots found.
    pub bounce: usize,
    /// Forced-crouch spots found.
    pub crouch: usize,
}

/// The bounce spot nearest the player.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Nearest {
    /// World units, to the spot's edge.
    pub distance: f64,
    /// Estimated launch speed straight up, world units a second.
    pub launch: f64,
    /// How far to fall onto it, world units (0 for a standing jump).
    pub fall: f64,
    pub edge: [[f64; 3]; 2],
}

/// `superbounce.state`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Whether the mode is on: the variant's flag, the built-in
    /// Superbounce variant, or `H2_SUPERBOUNCE=1`.
    pub active: bool,
    /// The level pair's word is -3.
    pub primed: bool,
    /// The level pair's word, eight hex digits, as `havok.agents` prints it;
    /// absent when the player has no level pair (between lives, asleep).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<String>,
    /// The same word as a signed number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_signed: Option<i32>,
    /// The local player's position, world units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearest: Option<Nearest>,
    pub scan: Scan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_omits_what_is_absent() {
        let s = State {
            active: true,
            primed: true,
            word: Some("fffffffd".into()),
            word_signed: Some(-3),
            position: Some([1.0, 2.0, 3.0]),
            nearest: None,
            scan: Scan {
                status: "ready".into(),
                map: Some("ascension".into()),
                error: None,
                bounce: 4,
                crouch: 2,
            },
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["word"], "fffffffd");
        assert!(v.get("nearest").is_none() && v["scan"].get("error").is_none());
        assert_eq!(serde_json::from_value::<State>(v).unwrap(), s);
        assert_eq!(method::ALL, [method::STATE]);
    }
}
