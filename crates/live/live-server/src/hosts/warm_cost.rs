use super::*;
use live_proto::host_pack::{LINGER_MS, WarmPlan};
use live_proto::hosts::HOST_PACK_WARM_RESERVED;

impl Live {
    /// Authorize the precise empty packs advertised by the Worker pool, up
    /// to a fixed deadline enforced by that pool and each container's room.
    pub async fn reserve_warm_packs(
        &self, pool: &str, class: &HostClass, plan: &WarmPlan, from_ms: i64, until_ms: i64,
    ) -> bool {
        if until_ms <= from_ms || until_ms.saturating_sub(from_ms) > LINGER_MS {
            return false;
        }
        let _write = self.hosts.writes.lock().await;
        let occupied = self.hosts.registry.read().unwrap().occupied();
        if occupied >= self.max_hosts() {
            return false;
        }
        let version = self.hosts.version();
        let Some(grant) = self.hosts.costs.warm_grant(pool, class, plan, from_ms, until_ms) else {
            return false;
        };
        grant.periods.is_empty() || self.host_event(version, HOST_PACK_WARM_RESERVED, grant).await.is_ok()
    }
}
