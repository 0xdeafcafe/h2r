//! Film page presentation of the library agent's preparation job.

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(super) struct Preparation {
    pub id: String,
    pub tick: u64,
    pub length_ticks: u64,
    #[serde(flatten)]
    pub status: Status,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Status {
    Idle,
    Starting,
    Running,
    Complete,
    Failed { error: String },
}

impl Preparation {
    pub(super) fn starting(id: &str, length_ticks: u64) -> Self {
        Self {
            id: id.into(),
            tick: 0,
            length_ticks,
            status: Status::Starting,
        }
    }

    pub(super) fn active(&self) -> bool {
        matches!(self.status, Status::Starting | Status::Running)
    }

    pub(super) fn copy(&self, tick_rate: u64) -> Option<String> {
        use super::selector::clock;
        Some(match &self.status {
            Status::Idle => return None,
            Status::Starting => "Starting preparation…".into(),
            Status::Running => format!(
                "Preparing {} / {}",
                clock(self.tick, tick_rate),
                clock(self.length_ticks, tick_rate),
            ),
            Status::Complete => "Preparation complete".into(),
            Status::Failed { error } => format!("Prepare failed: {error}"),
        })
    }
}
