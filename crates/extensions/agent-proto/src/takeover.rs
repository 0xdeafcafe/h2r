//! Player consent for using the game machine for a lab job.

use serde::{Deserialize, Serialize};

pub const INFO: &str = "session.takeover_info";
pub const REQUEST: &str = "session.request_takeover";
pub const STATUS: &str = "session.takeover_status";

pub const MAX_TIMEOUT_SECONDS: u32 = 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Build the image only.
    Build,
    /// Play a game.
    Run,
    /// Headless sim benchmark.
    Sim,
}

/// Where the job's code comes from, as the player should read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// `--branch NAME`, shown as `lab/NAME`, at `commit`.
    Branch { name: String, commit: String },
    /// `--rev REV` at `commit`.
    Rev { rev: String, commit: String },
    /// `--current`: the image already installed on this Mac, and its
    /// commit when the host knows it.
    Current { commit: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub requester: String,
    /// The job's required `--name`.
    pub name: String,
    pub operation: Operation,
    /// The job builds the image before it starts (a cold build).
    pub builds_image: bool,
    pub source: Source,
    /// The level a run starts on, when it names one.
    pub level: Option<String>,
    /// The run plays on the player's own data, settings and Live accounts.
    pub own_data: bool,
    /// Approximate runtime, or zero if the duration is unknown.
    pub estimate_seconds: u32,
    pub timeout_seconds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub window: bool,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requested {
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Pending,
    Allowed,
    Declined,
    TimedOut,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub decision: Decision,
}
