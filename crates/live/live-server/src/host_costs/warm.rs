//! Idle Cloudflare packs have no lease on which to book their cost. The hub
//! journals the additional covered interval before granting work to a pool.
//! A lost provider response can therefore overestimate a hold, never erase it.
use super::{CloudflarePrices, HostCosts, LeaseUsage, CLOUDFLARE_DISK_GB, CLOUDFLARE_GIB_PER_VCPU};
use live_proto::host_pack::WarmPlan;
use live_proto::hosts::{
    HostClass, HostLease, HostPackWarmReserved, PackWarmPeriod, ProviderKind, Transport,
    DEFAULT_IDLE_TIMEOUT_S, DEFAULT_MAX_MATCH_S,
};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(super) struct WarmLedger {
    // Provider pack serials are never reused within one class and pool.
    packs: BTreeMap<(HostClass, String, String), Vec<PackWarmPeriod>>,
}

impl WarmLedger {
    fn key(pool: &str, class: &HostClass, pack: &str) -> (HostClass, String, String) {
        (class.clone(), pool.to_owned(), pack.to_owned())
    }

    fn extensions(
        &self, pool: &str, class: &HostClass, plan: &WarmPlan, from_ms: i64, until_ms: i64,
    ) -> HostPackWarmReserved {
        let periods = plan.refresh.iter().chain(plan.start.iter()).filter_map(|pack| {
            let key = Self::key(pool, class, pack);
            let last = self.packs.get(&key).and_then(|spans| spans.last());
            let from_ms = last.map_or(from_ms, |last| from_ms.max(last.until_ms));
            (from_ms < until_ms).then(|| PackWarmPeriod {
                pack: pack.clone(),
                from_ms,
                until_ms,
                start: last.is_none() && plan.start.as_ref() == Some(pack),
            })
        }).collect();
        HostPackWarmReserved { pool: pool.to_owned(), class: class.clone(), periods }
    }

    pub(super) fn apply(&mut self, grant: &HostPackWarmReserved) {
        for period in &grant.periods {
            if period.until_ms <= period.from_ms {
                continue;
            }
            let key = Self::key(&grant.pool, &grant.class, &period.pack);
            let spans = self.packs.entry(key).or_default();
            if let Some(last) = spans.last_mut()
                && period.from_ms <= last.until_ms
            {
                // Includes a replay of an already folded grant.
                last.until_ms = last.until_ms.max(period.until_ms);
            } else {
                spans.push(period.clone());
            }
        }
    }

    pub(super) fn cost(&self, rates: &CloudflarePrices, from: i64, until: i64) -> f64 {
        self.packs.iter().map(|((class, _, _), spans)| {
            spans.iter().rev().take_while(|s| s.until_ms > from)
                .map(|s| period_cost(class, s, rates, from, until)).sum::<f64>()
        }).sum()
    }
}

pub(super) fn grant_cost(grant: &HostPackWarmReserved, rates: &CloudflarePrices, from: i64, until: i64) -> f64 {
    grant.periods.iter().map(|period| period_cost(&grant.class, period, rates, from, until)).sum()
}

pub(super) fn period_cost(class: &HostClass, period: &PackWarmPeriod, rates: &CloudflarePrices, from: i64, until: i64) -> f64 {
    let begin = from.clamp(period.from_ms, period.until_ms).abs_diff(period.from_ms);
    let end = until.clamp(period.from_ms, period.until_ms).abs_diff(period.from_ms);
    let seconds = (end.div_ceil(10) - begin.div_ceil(10)) as f64 / 100.0;
    // Reserve the provider shape, including memory outside the game budget.
    let memory_gib = f64::from(class.vcpu()) * CLOUDFLARE_GIB_PER_VCPU;
    let running = seconds * (f64::from(class.vcpu()) * rates.usd_per_vcpu_second
        + memory_gib * rates.usd_per_gib_second + CLOUDFLARE_DISK_GB * rates.usd_per_gb_disk_second);
    if period.start && from <= period.from_ms && period.from_ms < until {
        running + rates.usd_per_start
    } else {
        running
    }
}

impl HostCosts {
    pub(crate) fn warm_grant(
        &self, pool: &str, class: &HostClass, plan: &WarmPlan, from_ms: i64, until_ms: i64,
    ) -> Option<HostPackWarmReserved> {
        let grant = self.warm.lock().unwrap().extensions(pool, class, plan, from_ms, until_ms);
        // Keep room for the match that the searchers are trying to form.
        let lease = HostLease {
            id: String::new(), assignment_id: String::new(), provider: ProviderKind::Cloudflare,
            class: class.clone(), region: None, map: String::new(), map_id: None, variant: None,
            hopper: None, max_players: 16, roster: Vec::new(), bots: Vec::new(),
            transport: Transport::Tunnel, idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
            max_match_s: DEFAULT_MAX_MATCH_S, created_ms: from_ms,
        };
        let mut candidate = LeaseUsage::requested(&lease, from_ms, false);
        candidate.pack_linger = true;
        self.fits_budget(&candidate, Some(&grant)).then_some(grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_costs::utc_month;
    use live_proto::hosts::{HOST_PACK_WARM_RESERVED, STREAM};
    use live_proto::sign::{EventBody, format_time};
    use serde_json::to_value;
    use time::OffsetDateTime;

    const FEB: i64 = 1_769_904_000_000;

    fn costs(cap: &str) -> HostCosts {
        HostCosts::configured(Some(r#"{"egress_gb_per_player_minute":0,"cloudflare":{
            "usd_per_start":1,"usd_per_vcpu_second":1,"usd_per_gib_second":0,"usd_per_gb_disk_second":0
        }}"#), Some(cap)).unwrap()
    }

    fn fold(costs: &HostCosts, grant: &HostPackWarmReserved, at: i64) {
        costs.fold(&EventBody {
            stream: STREAM.into(), kind: HOST_PACK_WARM_RESERVED.into(),
            at: format_time(OffsetDateTime::from_unix_timestamp_nanos(i128::from(at)*1_000_000).unwrap()),
            author: "server".into(), kid: "synthetic".into(), nonce: "one".into(),
            data: to_value(grant).unwrap(),
        });
    }

    #[test]
    fn replay_reserves_one_start_and_only_the_uncovered_tail() {
        // A full match plus its release hold costs $1861. A cold grant
        // adds $61. The $1932 cap permits ten more idle seconds, then refuses.
        let first = WarmPlan { start: Some("pack-1".into()), refresh: vec![] };
        let refresh = WarmPlan { start: None, refresh: vec!["pack-1".into()] };
        let small = HostClass::Small;
        let costs = costs("1932");
        let a = costs.warm_grant("west", &small, &first, FEB, FEB+60_000).unwrap();
        fold(&costs, &a, FEB);
        fold(&costs, &a, FEB);
        assert!(costs.warm_grant("west", &small, &first, FEB, FEB+60_000).unwrap().periods.is_empty());
        let b = costs.warm_grant("west", &small, &refresh, FEB+10_000, FEB+70_000).unwrap();
        assert_eq!((b.periods[0].from_ms, b.periods[0].start), (FEB+60_000, false));
        fold(&costs, &b, FEB+10_000);
        assert_eq!(costs.month_to_date(FEB+80_000), 71.0);
        assert!(costs.warm_grant("west", &small, &refresh, FEB+20_000, FEB+80_000).is_none());
    }

    #[test]
    fn packs_of_different_pools_and_classes_pay_separately_across_midnight() {
        let costs = costs("100000");
        let plan = WarmPlan { start: Some("pack-0".into()), refresh: vec![] };
        for (pool, class) in [("a", HostClass::Small), ("b", HostClass::Small), ("a", HostClass::Large)] {
            let grant = costs.warm_grant(pool, &class, &plan, FEB-1_000, FEB+1_000).unwrap();
            fold(&costs, &grant, FEB-1_000);
        }
        let ledger = costs.warm.lock().unwrap();
        let rates = &costs.prices.cloudflare;
        assert_eq!(ledger.cost(rates, utc_month(FEB-1).unwrap().0, FEB), 7.0);
        assert_eq!(ledger.cost(rates, FEB, FEB+2_000), 4.0);
    }
}
