//! `state.*`: machine snapshots of the running game (docs/runtime/agent-api.md
//! "`state`", docs/runtime/snapshots.md).
//!
//! `state.save` and `state.load` run between two scheduler slices, at the
//! first slice boundary after a game tick where nothing the snapshot cannot
//! carry is live. They retry at the following ticks, for at most
//! `max_ticks` of them, then answer [`crate::code::BUSY`] with the
//! blockers ([`Busy`]). `state.list` and `state.drop` only touch the store
//! and are answered at once. Every result also carries the `at` stamp,
//! added by the executor. Snapshots live in the user's cache directory,
//! one directory per build, never in the repository.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::At;

/// Method names. Listed by `session.methods` wherever the agent runs; the
/// game answers [`crate::code::UNAVAILABLE`] when it has no snapshot
/// support installed.
pub mod method {
    /// Save the machine under a name ([`super::SaveParams`],
    /// [`super::Saved`]).
    pub const SAVE: &str = "state.save";
    /// Put the machine back where a snapshot was ([`super::LoadParams`],
    /// [`super::Loaded`]).
    pub const LOAD: &str = "state.load";
    /// This build's snapshots ([`super::List`]).
    pub const LIST: &str = "state.list";
    /// Delete one ([`super::NameParams`], [`super::Dropped`]).
    pub const DROP: &str = "state.drop";
    /// Load one and save it again under a new name, for a branch of
    /// experiments ([`super::ForkParams`], [`super::Forked`]).
    pub const FORK: &str = "state.fork";

    pub const ALL: &[&str] = &[SAVE, LOAD, LIST, DROP, FORK];

    /// The ones that wait for a quiescent slice boundary; the rest are
    /// answered at once.
    pub const MACHINE: &[&str] = &[SAVE, LOAD, FORK];
}

/// Ticks a save or load retries for when `max_ticks` is not given: three
/// seconds of game time.
pub const DEFAULT_TICKS: u64 = 90;
/// The most `max_ticks` may ask for: a minute.
pub const MAX_TICKS: u64 = 1800;

/// `state.save`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveParams {
    /// `a-z 0-9 . _ -`, 1 to 64, not starting with a dot
    /// (`oldmombasa.courtyard_fight`).
    pub name: String,
    /// Replace a snapshot of that name; refused otherwise.
    #[serde(default)]
    pub replace: bool,
    /// Also write the raw RAM image `<name>.ram` beside it, for loads that
    /// map guest RAM copy-on-write over the file (shared between processes;
    /// docs/runtime/snapshots.md).
    #[serde(default)]
    pub cow: bool,
    /// Ticks to retry while something is live ([`DEFAULT_TICKS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u64>,
}

/// `state.load`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadParams {
    pub name: String,
    /// Map the saved `<name>.ram` copy-on-write as guest RAM instead of
    /// decoding RAM into it: pages the match never writes stay shared with
    /// every other process on the same image. Refused, naming the file,
    /// when the save wrote no RAM image.
    #[serde(default)]
    pub cow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u64>,
}

/// `state.drop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameParams {
    pub name: String,
}

/// `state.fork`: load `from`, then save the loaded machine as `name`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkParams {
    pub from: String,
    pub name: String,
    #[serde(default)]
    pub replace: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u64>,
}

/// What a save wrote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub name: String,
    /// The file, under the store's directory for this build.
    pub path: String,
    pub bytes: u64,
    /// Section tags in file order (`d3d`, `dsound`, ..., `ram`, `kcore`).
    pub sections: Vec<String>,
    /// Slice boundaries tried before this one was quiescent (0: the first).
    pub retries: u64,
    pub ms: f64,
}

/// The lockstep hash check after a load: guest RAM hashed as
/// `lockstep::hash_guest` does, against the hash taken at save time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HashCheck {
    /// The snapshot holds a hash (written by every save in this build).
    pub checked: bool,
    pub equal: bool,
    /// The MiB chunks that differ, at most 16.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differing_mib: Vec<u32>,
}

/// What a load put back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Loaded {
    pub name: String,
    /// The game tick and presents the snapshot was saved at.
    pub tick: u64,
    pub presents: u64,
    /// The header's notes: map, mode, the agent's tick count at the save.
    pub notes: BTreeMap<String, String>,
    /// Section tags loaded, in the order they were applied.
    pub applied: Vec<String>,
    pub hash: HashCheck,
    pub retries: u64,
    pub ms: f64,
}

/// `state.fork`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Forked {
    pub loaded: Loaded,
    pub saved: Saved,
}

/// One snapshot of this build.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub bytes: u64,
    pub tick: u64,
    pub presents: u64,
    pub notes: BTreeMap<String, String>,
}

/// `state.list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct List {
    /// The store's directory for this build.
    pub dir: String,
    pub snapshots: Vec<Listed>,
}

/// `state.drop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dropped {
    pub name: String,
}

/// The `data` of a [`crate::code::BUSY`] error: what stayed live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Busy {
    pub at: At,
    /// Every blocker at the last try, `owner: why`.
    pub blockers: Vec<String>,
    /// Slice boundaries tried.
    pub tries: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_are_strict() {
        let p: SaveParams = serde_json::from_value(json!({"name": "a"})).unwrap();
        assert_eq!(
            (p.name.as_str(), p.replace, p.max_ticks),
            ("a", false, None)
        );
        assert!(serde_json::from_value::<SaveParams>(json!({"name": "a", "x": 1})).is_err());
        assert!(serde_json::from_value::<LoadParams>(json!({})).is_err());
        let f: ForkParams =
            serde_json::from_value(json!({"from": "a", "name": "b", "max_ticks": 5})).unwrap();
        assert_eq!(f.max_ticks, Some(5));
    }

    #[test]
    fn every_machine_method_is_listed() {
        assert!(method::MACHINE.iter().all(|m| method::ALL.contains(m)));
    }
}
