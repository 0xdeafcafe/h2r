//! Every way a monitor look is turned down, each with a code and a
//! sentence for the player (docs/game/forge-monitor-appearance.md,
//! "Other players").
use std::fmt;

use serde_json::{Value, json};

/// The refusal codes, in the order the design lists them.
pub const CODES: [&str; 6] = [
    "monitor_look_invalid",
    "monitor_look_source_missing",
    "monitor_look_renderer",
    "monitor_look_locked",
    "monitor_look_version",
    "monitor_look_no_session",
];

/// Why a look, or a change to one, was refused. The seat keeps its
/// previous look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A field out of bounds, an unknown key, a variant the model lacks.
    Invalid { field: String, reason: String },
    /// A machine in the session does not hold the model's source map at
    /// that hash.
    SourceMissing { machine: String, map: String },
    /// A machine in the session runs without the native renderer.
    Renderer { machine: String },
    /// The field cannot be changed now: the host set team colours, or the
    /// model has no such thing.
    Locked { field: String, reason: String },
    /// The look's version is not this build's; at join, before the map
    /// loads.
    Version { theirs: u32, ours: u32 },
    /// A set outside a Forge session through the agent API (the menu
    /// writes the profile instead).
    NoSession,
}

impl Refusal {
    pub fn invalid(field: &str, reason: impl Into<String>) -> Refusal {
        Refusal::Invalid {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub fn locked(field: &str, reason: impl Into<String>) -> Refusal {
        Refusal::Locked {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Refusal::Invalid { .. } => CODES[0],
            Refusal::SourceMissing { .. } => CODES[1],
            Refusal::Renderer { .. } => CODES[2],
            Refusal::Locked { .. } => CODES[3],
            Refusal::Version { .. } => CODES[4],
            Refusal::NoSession => CODES[5],
        }
    }

    /// The sentence the affected player reads.
    pub fn sentence(&self) -> String {
        match self {
            Refusal::Invalid { field, reason } => format!("Monitor look: {field} {reason}."),
            Refusal::SourceMissing { machine, map } => {
                format!("Monitor look: {machine} does not have the map {map}.")
            }
            Refusal::Renderer { machine } => {
                format!("Monitor look: {machine} is not drawing monitor bodies.")
            }
            Refusal::Locked { field, reason } => {
                format!("Monitor look: {field} is locked. {reason}")
            }
            Refusal::Version { theirs, ours } => {
                format!("Monitor look: version {theirs} does not match this game's version {ours}.")
            }
            Refusal::NoSession => "Monitor look: there is no Forge session to change.".into(),
        }
    }

    /// `{code, message, ...fields}`: what the agent API returns.
    pub fn to_value(&self) -> Value {
        let mut v = json!({"code": self.code(), "message": self.sentence()});
        let fields = match self {
            Refusal::Invalid { field, reason } => json!({"field": field, "reason": reason}),
            Refusal::SourceMissing { machine, map } => json!({"machine": machine, "map": map}),
            Refusal::Renderer { machine } => json!({"machine": machine}),
            Refusal::Locked { field, reason } => json!({"field": field, "reason": reason}),
            Refusal::Version { theirs, ours } => json!({"theirs": theirs, "ours": ours}),
            Refusal::NoSession => json!({}),
        };
        if let (Some(v), Some(f)) = (v.as_object_mut(), fields.as_object()) {
            v.extend(f.clone());
        }
        v
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.sentence())
    }
}

impl std::error::Error for Refusal {}
