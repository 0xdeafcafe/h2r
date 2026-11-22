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
    /// Puts a new blob into the running game: in place when the state
    /// shape is the same, else by restarting the game on the same map and
    /// variant ([`super::ReloadParams`], [`super::ReloadResult`]).
    pub const RELOAD: &str = "rules.reload";

    pub const ALL: &[&str] = &[STATE, RELOAD];
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
    /// The last `rules.reload` this run made or refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reload: Option<LastReload>,
}

// ---- rules.reload ----

/// How `rules.reload` may put the new blob in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadMode {
    /// In place when the state shape is the same, else restart.
    #[default]
    Auto,
    /// In place only: refused when the state shape changed.
    InPlace,
    /// Restart the game even when in place would do.
    Restart,
}

/// `rules.reload`'s params: the blob as base64 (`ir_b64`) or the source
/// (`source`, compiled by the game), exactly one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub mode: ReloadMode,
}

/// What `rules.reload` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reloaded {
    /// The program was swapped between ticks; every slot value is kept.
    InPlace,
    /// The game was queued again on the same map and variant, the
    /// variant's script reference naming the new blob.
    Restarted,
}

impl Reloaded {
    pub fn name(self) -> &'static str {
        match self {
            Reloaded::InPlace => "in_place",
            Reloaded::Restarted => "restarted",
        }
    }
}

/// `rules.reload`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResult {
    pub mode: Reloaded,
    /// The new blob's SHA-256, 64 lowercase hex digits.
    pub hash: String,
    /// Why it could not go in place (empty for `in_place`, and for a
    /// restart the caller asked for).
    #[serde(default)]
    pub reasons: Vec<String>,
    /// The game tick the reload was made at.
    pub tick: u32,
}

/// `rules.state`'s record of the last reload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastReload {
    /// `in_place`, `restarted`, or `refused`.
    pub mode: String,
    /// The blob's hash, when it compiled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub tick: u32,
    /// Why not in place, or why refused.
    #[serde(default)]
    pub reasons: Vec<String>,
}

impl LastReload {
    pub fn of(r: &ReloadResult) -> LastReload {
        LastReload {
            mode: r.mode.name().into(),
            hash: Some(r.hash.clone()),
            tick: r.tick,
            reasons: r.reasons.clone(),
        }
    }

    pub fn refused(hash: Option<String>, tick: u32, why: String) -> LastReload {
        LastReload {
            mode: "refused".into(),
            hash,
            tick,
            reasons: vec![why],
        }
    }
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

    #[test]
    fn reload_params_take_a_blob_or_source_and_a_mode() {
        let p: ReloadParams = from_value(json!({"ir_b64": "AAAA"})).unwrap();
        assert_eq!(p.mode, ReloadMode::Auto);
        let p: ReloadParams = from_value(json!({"source": "x", "mode": "in_place"})).unwrap();
        assert_eq!((p.source.as_deref(), p.mode), (Some("x"), ReloadMode::InPlace));
        assert!(from_value::<ReloadParams>(json!({"mode": "sideways"})).is_err());
        assert!(from_value::<ReloadParams>(json!({"ir": "x"})).is_err());
        let r = ReloadResult {
            mode: Reloaded::Restarted,
            hash: "ab".repeat(32),
            reasons: vec!["player variable best changed".into()],
            tick: 9,
        };
        assert_eq!(
            to_value(&r).unwrap(),
            json!({"mode": "restarted", "hash": "ab".repeat(32), "reasons": ["player variable best changed"], "tick": 9})
        );
        assert_eq!(LastReload::of(&r).mode, "restarted");
    }
}
