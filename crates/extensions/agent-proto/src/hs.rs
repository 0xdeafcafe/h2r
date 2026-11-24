//! HaloScript parity results, sampled at an agent tick boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PARITY: &str = "hs.parity";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parity {
    pub switch: String,
    /// Process-lifetime counters keyed by the VM operation being checked.
    pub checks: BTreeMap<String, Check>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub matched: u64,
    pub mismatched: u64,
    pub declined: u64,
    /// The first difference the check observed.
    pub first: Option<String>,
}
