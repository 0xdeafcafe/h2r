//! Shadow checks of native ports against the lifted original
//! (docs/engine/shadow-exec.md, docs/runtime/agent-api.md "Shadow checks").
//! Host methods: answered where boot's agent glue registers them, so
//! `session.methods` lists them there.
//!
//! Ports are named by their label (`sim.port.unit_pose`,
//! `sim.port.first_person_pose`), by group (`sim.native`, `sim.physics`)
//! or as `all`. A port is checked only when it is installed: no
//! subsystem body owns its point in this run.

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// Turn checks on ([`super::EnableParams`]) →
    /// `{"enabled", "not_installed", "every", "step_limit"}`.
    pub const ENABLE: &str = "shadow.enable";
    /// Turn checks off ([`super::DisableParams`]; no ports: all) →
    /// `{"disabled"}`. Counts and mismatches stay for `shadow.status`.
    pub const DISABLE: &str = "shadow.disable";
    /// Counts per port and the mismatches so far: `{"on", "ready",
    /// "every", "step_limit", "calls", "checked", "matched", "mismatched",
    /// "skipped", "ports": [...], "mismatches": [...],
    /// "mismatches_dropped"}`.
    pub const STATUS: &str = "shadow.status";
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableParams {
    /// Ports, groups or `all`.
    pub ports: Vec<String>,
    /// Check one call in this many (default 1, or the last value set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u64>,
    /// Instructions the original may run per check before the check is
    /// skipped (default 5,000,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_limit: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisableParams {
    /// Ports, groups or `all`; none turns every port off.
    #[serde(default)]
    pub ports: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_params_read_from_the_cli_form() {
        let p: EnableParams =
            serde_json::from_str(r#"{"ports":["sim.physics"],"every":4}"#).unwrap();
        assert_eq!(p.ports, ["sim.physics"]);
        assert_eq!((p.every, p.step_limit), (Some(4), None));
        assert!(serde_json::from_str::<EnableParams>(r#"{"ports":[],"often":1}"#).is_err());
        let d: DisableParams = serde_json::from_str("{}").unwrap();
        assert!(d.ports.is_empty());
    }
}
