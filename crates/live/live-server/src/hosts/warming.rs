//! Reconcile one class and region at a time from the live queue and registry.
//! Losing the hub or a pulse drops the hold naturally at the supervisor.
use super::*;
use live_proto::host_pack::WarmDemand;

impl Live {
    pub async fn refresh_host_pools(&self) {
        if self.held.held() || self.hosts.costs.budget_usd_month() == Some(0.0) {
            return;
        }
        let Some(provider) = self.hosts.provider.read().unwrap().clone() else { return };
        let kind = provider.kind();
        if kind == ProviderKind::Fly {
            return;
        }
        let available_limit = {
            let config = self.hosts.config.read().unwrap();
            if config.data_url.is_empty() || config.data_sha256.len() != 64
                || !config.data_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return;
            }
            config.max_hosts
        };
        let at = now_ms();
        let mut groups: BTreeMap<_, WarmDemand> = self.matchmaking.lock().unwrap()
            .search_demand(at.div_euclid(1000)).into_iter().map(|(region, searchers)| {
                ((region, HostClass::Small), WarmDemand { searchers, ..WarmDemand::default() })
            }).collect();
        let available = {
            let registry = self.hosts.registry.read().unwrap();
            for row in registry.active.iter().filter_map(|id| registry.leases.get(id))
                .filter(|r| r.lease.provider == kind) {
                let key = (row.lease.region.clone(), row.lease.class.clone());
                groups.entry(key).or_default().leases += 1;
            }
            available_limit.saturating_sub(registry.occupied())
        };
        // Only one reconciliation of each group per five seconds, regardless
        // of how many tickets were submitted. Run cold starts outside locks.
        if available == 0 {
            return;
        }
        {
            let mut pulses = self.hosts.warm_pulses.lock().await;
            pulses.retain(|_, t| at.saturating_sub(*t) < 5_000);
            groups.retain(|key, _| {
                if pulses.contains_key(key) {
                    return false;
                }
                pulses.insert(key.clone(), at);
                true
            });
        }
        for ((region, class), mut demand) in groups {
            demand.available = available;
            let result = provider.warm(self, region.as_deref(), &class, demand).await;
            if let Err(error) = &result {
                eprintln!("live: host pool warm request failed: {error}");
            }
        }
    }
}
