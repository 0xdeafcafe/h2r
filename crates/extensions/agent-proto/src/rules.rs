//! `rules.*`: the rules script of the game running now
//! (docs/runtime/agent-api.md, "Rules scripts"; docs/game/rules-engine.md,
//! "The host runtime"). Wire types only. Game methods: `session.methods`
//! lists them where the game registered them (`game::agent_glue`).

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// The runtime: the script that runs and its hash, or why none does,
    /// and the tick's budget use ([`super::State`]; no params).
    pub const STATE: &str = "rules.state";

    pub const ALL: &[&str] = &[STATE];
}

// ---- rules.state ----

/// `rules.state`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// A script runs on this machine.
    pub running: bool,
    /// Why none does: the feature is off, no multiplayer game, the record
    /// names no script, this machine is not the authority, or the
    /// script's refusal with its reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The script's name, from its header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// The script's own version number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u16>,
    /// The blob's SHA-256, 64 lowercase hex digits; the prefix the record
    /// carries (16 digits) when the blob was not found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Where the blob came from: `builtin` or `live`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The retail engine index the game rides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<u8>,
    /// The packed option bits from the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<u32>,
    /// What `GAME_ENGINE_AUTHORITATIVE` answered at the first tick; none
    /// before it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<bool>,
    /// The script error that stopped the VM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// Engine calls waiting for the next tick boundary.
    #[serde(default)]
    pub pending: usize,
    /// The latest calls the engine refused, oldest first.
    #[serde(default)]
    pub errors: Vec<Refusal>,
}

/// The VM's op budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// The tick the VM last began.
    pub tick: u32,
    /// Ops used in that tick so far.
    pub ops: u32,
    /// Ops a tick may use.
    pub limit: u32,
    /// The verifier's worst-tick estimate.
    pub worst_tick: u64,
}

/// One engine call the engine refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The tick boundary the call was made at.
    pub tick: u32,
    /// The API entry's name (`score`, `end_round`, ...).
    pub call: String,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn an_idle_state_names_only_its_reason() {
        let s = State {
            refused: Some("off".into()),
            ..Default::default()
        };
        assert_eq!(
            to_value(&s).unwrap(),
            json!({"running": false, "refused": "off", "pending": 0, "errors": []})
        );
        let back: State = from_value(json!({"running": false})).unwrap();
        assert_eq!(back, State::default());
    }
}
