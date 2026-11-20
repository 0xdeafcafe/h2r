//! Durable pack state. Only ports and absolute cleanup deadlines survive
//! eviction. A search extends an empty supervisor's deadline for one linger.
use std::collections::BTreeMap;

use live_proto::host_pack::{LINGER_MS, WarmPlan};
use serde::{Deserialize, Serialize};

/// Return only operations the hub has paid for, including the difference
/// between keeping a running supervisor and starting a container.
pub(super) fn funded(mut current: WarmPlan, grant: &WarmPlan) -> WarmPlan {
    current.refresh.retain(|name| grant.refresh.contains(name));
    if current.start != grant.start {
        current.start = None;
    }
    current
}

pub(super) fn grant_deadline(now: i64, until: i64) -> Option<i64> {
    (until > now).then(|| until.min(now.saturating_add(LINGER_MS)))
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct PackLease {
    pub deadline_ms: i64,
    pub tunnel_port: u16,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct PackLeases {
    pub leases: BTreeMap<String, PackLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    idle_until_ms: Option<i64>,
}

impl PackLeases {
    pub fn deadline(&self) -> Option<i64> {
        self.leases.values().map(|l| l.deadline_ms).max().or(self.idle_until_ms)
    }

    pub fn hold_until(&mut self, until_ms: i64) {
        if self.leases.is_empty() {
            self.idle_until_ms = Some(until_ms);
        }
    }

    pub fn admitted(&mut self, id: String, lease: PackLease) {
        self.idle_until_ms = None;
        self.leases.insert(id, lease);
    }

    pub fn release(&mut self, id: &str, now_ms: i64) {
        if self.leases.remove(id).is_some() {
            self.hold_until(now_ms.saturating_add(LINGER_MS));
        }
    }

    pub fn expire(&mut self, now_ms: i64) {
        self.leases.retain(|_, l| l.deadline_ms > now_ms);
        if self.idle_until_ms.is_some_and(|until| until <= now_ms) {
            self.idle_until_ms = None;
        }
    }
}
