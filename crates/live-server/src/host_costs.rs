//! Estimated dedicated-host spend, projected from the verified hosts stream.

use std::collections::BTreeMap;
use std::sync::Mutex;

use live_proto::hosts::{
    self, HostBooting, HostClass, HostEnded, HostPlaying, HostReady, HostRequested, ProviderKind,
};
use live_proto::sign::{EventBody, parse_time};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

pub const DEFAULT_BUDGET_USD_MONTH: f64 = 5.0;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PriceTable {
    egress_gb_per_player_minute: f64,
    cloudflare: CloudflarePrices,
    fly: FlyPrices,
}

impl Default for PriceTable {
    fn default() -> Self {
        Self {
            egress_gb_per_player_minute: 0.0005,
            cloudflare: CloudflarePrices::default(),
            fly: FlyPrices::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CloudflarePrices {
    usd_per_vcpu_second: f64,
    usd_per_gib_second: f64,
    usd_per_egress_gb: f64,
}

impl Default for CloudflarePrices {
    fn default() -> Self {
        Self {
            usd_per_vcpu_second: 0.000_02,
            usd_per_gib_second: 0.000_002_5,
            usd_per_egress_gb: 0.025,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FlyPrices {
    small_usd_per_second: f64,
    large_usd_per_second: f64,
}

impl Default for FlyPrices {
    fn default() -> Self {
        Self {
            small_usd_per_second: 0.046 / 3_600.0,
            large_usd_per_second: 0.092 / 3_600.0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostCostConfigError {
    #[error("invalid LIVE_HOST_PRICES JSON: {0}")]
    PricesJson(#[from] serde_json::Error),
    #[error("LIVE_HOST_PRICES rates must be finite and non-negative")]
    PricesRange,
    #[error("LIVE_HOST_BUDGET_USD_MONTH must be a finite non-negative number")]
    Budget,
}

/// Lifecycle data for a lease. Timestamps are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeaseUsage {
    pub lease_id: String,
    pub provider: ProviderKind,
    pub class: HostClass,
    pub region: Option<String>,
    pub requested_ms: i64,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    /// Final runtime from HostEnded, if recorded.
    pub ran_ms: Option<u64>,
    pub players_peak: u8,
}

impl LeaseUsage {
    fn elapsed_ms(&self, at_ms: i64) -> u64 {
        if let (Some(end_ms), Some(ran_ms)) = (self.end_ms, self.ran_ms)
            && at_ms >= end_ms
        {
            return ran_ms;
        }
        self.start_ms
            .map_or(0, |start_ms| at_ms.max(start_ms).abs_diff(start_ms))
    }
}

/// Estimated usage and USD for a lease through a given time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct LeaseCost {
    pub ran_ms: u64,
    pub estimated_egress_gb: f64,
    pub cost_usd: f64,
}

/// Replay as a dispatcher subscriber or fold event bodies directly.
#[derive(Debug)]
pub struct HostCosts {
    prices: PriceTable,
    budget_usd_month: f64,
    leases: Mutex<BTreeMap<String, LeaseUsage>>,
}

impl Default for HostCosts {
    fn default() -> Self {
        Self {
            prices: PriceTable::default(),
            budget_usd_month: DEFAULT_BUDGET_USD_MONTH,
            leases: Mutex::default(),
        }
    }
}

impl HostCosts {
    /// Pass the values of LIVE_HOST_PRICES and LIVE_HOST_BUDGET_USD_MONTH.
    /// Missing values and omitted JSON fields use the defaults.
    pub fn configured(
        prices_json: Option<&str>,
        budget_usd_month: Option<&str>,
    ) -> Result<Self, HostCostConfigError> {
        let mut costs = Self::default();
        if let Some(json) = prices_json {
            let prices: PriceTable = serde_json::from_str(json)?;
            let rates = [
                prices.egress_gb_per_player_minute,
                prices.cloudflare.usd_per_vcpu_second,
                prices.cloudflare.usd_per_gib_second,
                prices.cloudflare.usd_per_egress_gb,
                prices.fly.small_usd_per_second,
                prices.fly.large_usd_per_second,
            ];
            if !rates.into_iter().all(valid_rate) {
                return Err(HostCostConfigError::PricesRange);
            }
            costs.prices = prices;
        }
        if let Some(value) = budget_usd_month {
            costs.budget_usd_month = value
                .trim()
                .parse()
                .ok()
                .filter(|&value| valid_rate(value))
                .ok_or(HostCostConfigError::Budget)?;
        }
        Ok(costs)
    }

    /// Apply verified server events in log order. A request with an unknown
    /// provider or malformed payload cannot establish a billable lease.
    pub fn fold(&self, event: &EventBody) {
        if event.stream != hosts::STREAM || !hosts::KINDS.contains(&event.kind.as_str()) {
            return;
        }
        let Some(at) = parse_time(&event.at) else {
            return;
        };
        let at_ms = at.unix_timestamp_nanos().div_euclid(1_000_000) as i64;
        let mut leases = self.leases.lock().unwrap();
        match event.kind.as_str() {
            hosts::HOST_REQUESTED => {
                if let Ok(HostRequested { lease }) = HostRequested::deserialize(&event.data) {
                    leases.entry(lease.id.clone()).or_insert_with(|| LeaseUsage {
                        lease_id: lease.id,
                        provider: lease.provider,
                        class: lease.class,
                        region: lease.region,
                        requested_ms: at_ms,
                        start_ms: None,
                        end_ms: None,
                        ran_ms: None,
                        players_peak: 0,
                    });
                }
            }
            hosts::HOST_BOOTING => {
                if let Ok(data) = HostBooting::deserialize(&event.data)
                    && let Some(lease) = active_lease(&mut leases, &data.lease_id)
                {
                    lease.start_ms.get_or_insert(at_ms);
                }
            }
            hosts::HOST_READY => {
                if let Ok(data) = HostReady::deserialize(&event.data)
                    && let Some(lease) = active_lease(&mut leases, &data.lease_id)
                {
                    lease.start_ms.get_or_insert(at_ms);
                }
            }
            hosts::HOST_PLAYING => {
                if let Ok(data) = HostPlaying::deserialize(&event.data)
                    && let Some(lease) = active_lease(&mut leases, &data.lease_id)
                {
                    lease.start_ms.get_or_insert(at_ms);
                    lease.players_peak = lease.players_peak.max(data.players);
                }
            }
            hosts::HOST_ENDED => {
                if let Ok(data) = HostEnded::deserialize(&event.data)
                    && let Some(lease) = active_lease(&mut leases, &data.lease_id)
                {
                    if data.ran_ms > 0 || lease.start_ms.is_some() {
                        lease.start_ms = Some(at_ms.saturating_sub_unsigned(data.ran_ms));
                    }
                    lease.end_ms = Some(at_ms);
                    lease.ran_ms = Some(data.ran_ms);
                    lease.players_peak = lease.players_peak.max(data.players_peak);
                }
            }
            _ => {}
        }
    }

    pub fn lease(&self, lease_id: &str) -> Option<LeaseUsage> {
        self.leases.lock().unwrap().get(lease_id).cloned()
    }

    /// Price a lease through now_ms, in Unix milliseconds.
    pub fn lease_cost(&self, lease_id: &str, now_ms: i64) -> Option<LeaseCost> {
        let leases = self.leases.lock().unwrap();
        let lease = leases.get(lease_id)?;
        Some(self.estimate(lease, 0, lease.elapsed_ms(now_ms)))
    }

    /// Estimated USD accrued in the current UTC month, including running
    /// hosts and only the current month's share of a spanning lease.
    pub fn month_to_date(&self, now_ms: i64) -> f64 {
        let Some(month_start_ms) = utc_month_start_ms(now_ms) else {
            return 0.0;
        };
        self.leases
            .lock()
            .unwrap()
            .values()
            .map(|lease| {
                let from_ms = lease.elapsed_ms(month_start_ms);
                let to_ms = lease.elapsed_ms(now_ms);
                self.estimate(lease, from_ms, to_ms).cost_usd
            })
            .sum()
    }

    /// The configured monthly spending limit in USD.
    pub fn budget_usd_month(&self) -> f64 {
        self.budget_usd_month
    }

    /// The guard trips as soon as estimated spending reaches the limit.
    pub fn over_budget(&self, now_ms: i64) -> bool {
        self.month_to_date(now_ms) >= self.budget_usd_month
    }

    fn estimate(&self, lease: &LeaseUsage, from_ms: u64, to_ms: u64) -> LeaseCost {
        let ran_ms = to_ms - from_ms;
        if ran_ms == 0 {
            return LeaseCost::default();
        }
        let player_minutes = ran_ms as f64 / 60_000.0 * f64::from(lease.players_peak);
        let estimated_egress_gb = player_minutes * self.prices.egress_gb_per_player_minute;
        let cost_usd = match lease.provider {
            ProviderKind::Local => 0.0,
            ProviderKind::Cloudflare => {
                // Round cumulative runtime so a tick spanning two months
                // is counted once, in the month when it started.
                let ticks = to_ms.div_ceil(10) - from_ms.div_ceil(10);
                let seconds = ticks as f64 / 100.0;
                let rates = &self.prices.cloudflare;
                let cpu = seconds * f64::from(lease.class.vcpu()) * rates.usd_per_vcpu_second;
                let memory = seconds * f64::from(lease.class.gib()) * rates.usd_per_gib_second;
                let egress = if rates.usd_per_egress_gb > 0.0 {
                    estimated_egress_gb * rates.usd_per_egress_gb
                } else {
                    0.0
                };
                cpu + memory + egress
            }
            ProviderKind::Fly => {
                let rate = match lease.class {
                    HostClass::Small => self.prices.fly.small_usd_per_second,
                    HostClass::Large => self.prices.fly.large_usd_per_second,
                };
                ran_ms as f64 / 1_000.0 * rate
            }
        };
        LeaseCost {
            ran_ms,
            estimated_egress_gb,
            cost_usd,
        }
    }
}

impl Subscriber for HostCosts {
    fn name(&self) -> &str {
        "host-costs"
    }

    fn apply(&self, event: &RecordedEvent) {
        self.fold(&event.body);
    }
}

fn active_lease<'a>(
    leases: &'a mut BTreeMap<String, LeaseUsage>,
    lease_id: &str,
) -> Option<&'a mut LeaseUsage> {
    leases.get_mut(lease_id).filter(|lease| lease.end_ms.is_none())
}

fn valid_rate(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn utc_month_start_ms(now_ms: i64) -> Option<i64> {
    let now = OffsetDateTime::from_unix_timestamp_nanos(i128::from(now_ms) * 1_000_000).ok()?;
    let month_start = Date::from_calendar_date(now.year(), now.month(), 1).ok()?;
    Some(month_start.midnight().assume_utc().unix_timestamp() * 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hosts::{DEFAULT_IDLE_TIMEOUT_S, DEFAULT_MAX_MATCH_S, HostLease, Transport};
    use live_proto::sign::format_time;
    use serde_json::{Value, json};

    // 2026-02-01 00:00 UTC.
    const FEB: i64 = 1_769_904_000_000;
    const MINUTE: i64 = 60_000;

    fn event(kind: &str, data: Value, at_ms: i64) -> EventBody {
        let at =
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(at_ms) * 1_000_000).unwrap();
        EventBody {
            stream: hosts::STREAM.into(),
            kind: kind.into(),
            at: format_time(at),
            data,
            author: "server".into(),
            kid: "test".into(),
            nonce: "test".into(),
        }
    }

    fn request(id: &str, provider: ProviderKind, class: HostClass, at_ms: i64) -> EventBody {
        let lease = HostLease {
            id: id.into(),
            assignment_id: "custom-lockout".into(),
            provider,
            class,
            region: Some("iad".into()),
            map: "lockout".into(),
            map_id: None,
            variant: None,
            hopper: None,
            max_players: 16,
            roster: vec![],
            bots: vec![],
            transport: Transport::Udp,
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
            max_match_s: DEFAULT_MAX_MATCH_S,
            created_ms: at_ms,
        };
        event(hosts::HOST_REQUESTED, json!({ "lease": lease }), at_ms)
    }

    fn boot(id: &str, at_ms: i64) -> EventBody {
        event(
            hosts::HOST_BOOTING,
            json!({ "lease_id": id, "provider_ref": "machine" }),
            at_ms,
        )
    }

    fn ready(id: &str, at_ms: i64) -> EventBody {
        event(
            hosts::HOST_READY,
            json!({
                "lease_id": id,
                "endpoint": { "transport": "udp", "xnaddr": "00" },
                "boot_ms": 500,
            }),
            at_ms,
        )
    }

    fn playing(id: &str, players: u8, at_ms: i64) -> EventBody {
        event(
            hosts::HOST_PLAYING,
            json!({ "lease_id": id, "players": players }),
            at_ms,
        )
    }

    fn end(id: &str, ran_ms: u64, players_peak: u8, at_ms: i64) -> EventBody {
        event(
            hosts::HOST_ENDED,
            json!({
                "lease_id": id,
                "reason": "match_over",
                "ran_ms": ran_ms,
                "players_peak": players_peak,
            }),
            at_ms,
        )
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn settles_lifecycle_with_reported_runtime_and_highest_peak() {
        let costs = HostCosts::default();
        let requested = request("match", ProviderKind::Cloudflare, HostClass::Small, FEB);
        costs.fold(&requested);
        assert_eq!(costs.lease("match").unwrap().start_ms, None);
        assert_eq!(
            costs.lease_cost("match", FEB + 1_000),
            Some(LeaseCost::default())
        );

        costs.fold(&boot("match", FEB + 100));
        costs.fold(&boot("match", FEB + 150));
        costs.fold(&ready("match", FEB + 400));
        costs.fold(&playing("match", 5, FEB + 500));
        costs.fold(&playing("match", 2, FEB + 600));
        assert_eq!(costs.lease("match").unwrap().start_ms, Some(FEB + 100));
        assert_eq!(costs.lease_cost("match", FEB + 700).unwrap().ran_ms, 600);

        costs.fold(&end("match", 640, 4, FEB + 800));
        costs.fold(&end("match", 900, 16, FEB + 1_000));
        costs.fold(&playing("match", 16, FEB + 1_001));
        costs.fold(&requested);

        let usage = costs.lease("match").unwrap();
        assert_eq!(usage.provider, ProviderKind::Cloudflare);
        assert_eq!(usage.class, HostClass::Small);
        assert_eq!(usage.region.as_deref(), Some("iad"));
        assert_eq!(usage.start_ms, Some(FEB + 160));
        assert_eq!(usage.end_ms, Some(FEB + 800));
        assert_eq!(usage.ran_ms, Some(640));
        assert_eq!(usage.players_peak, 5);
        let settled = costs.lease_cost("match", FEB + 2_000).unwrap();
        assert_eq!(settled.ran_ms, 640);
        close(settled.estimated_egress_gb, 640.0 / 60_000.0 * 5.0 * 0.0005);
        close(settled.cost_usd, 0.64 * 0.000_025 + settled.estimated_egress_gb * 0.025);
    }

    #[test]
    fn prices_the_documented_ten_minute_matches() {
        for (provider, class, players, expected) in [
            (ProviderKind::Cloudflare, HostClass::Small, 2, 0.01525),
            (ProviderKind::Cloudflare, HostClass::Large, 16, 0.032),
            (ProviderKind::Fly, HostClass::Small, 2, 0.046 / 6.0),
            (ProviderKind::Fly, HostClass::Large, 16, 0.092 / 6.0),
            (ProviderKind::Local, HostClass::Large, 16, 0.0),
        ] {
            let costs = HostCosts::default();
            costs.fold(&request("match", provider, class, FEB));
            costs.fold(&end("match", 600_000, players, FEB + 10 * MINUTE));
            let cost = costs.lease_cost("match", FEB + 11 * MINUTE).unwrap();
            assert_eq!(cost.ran_ms, 600_000);
            close(cost.estimated_egress_gb, f64::from(players) * 0.005);
            close(cost.cost_usd, expected);
            close(costs.month_to_date(FEB + 11 * MINUTE), expected);
        }
    }

    #[test]
    fn estimates_running_hosts_from_first_start_evidence() {
        let costs = HostCosts::default();
        for id in ["boot", "ready", "playing", "requested"] {
            costs.fold(&request(id, ProviderKind::Cloudflare, HostClass::Small, FEB));
        }
        costs.fold(&boot("boot", FEB + MINUTE));
        costs.fold(&ready("ready", FEB + MINUTE));
        costs.fold(&playing("playing", 2, FEB + MINUTE));
        costs.fold(&playing("playing", 1, FEB + MINUTE + 500));

        let now = FEB + 2 * MINUTE;
        for id in ["boot", "ready", "playing"] {
            assert_eq!(costs.lease(id).unwrap().end_ms, None);
            assert_eq!(costs.lease(id).unwrap().ran_ms, None);
            assert_eq!(costs.lease_cost(id, now).unwrap().ran_ms, 60_000);
        }
        close(costs.lease_cost("playing", now).unwrap().estimated_egress_gb, 0.001);
        assert_eq!(costs.lease_cost("requested", now), Some(LeaseCost::default()));
        close(costs.month_to_date(now), 3.0 * 60.0 * 0.000_025 + 0.001 * 0.025);
    }

    #[test]
    fn ending_without_runtime_is_free() {
        let costs = HostCosts::default();
        costs.fold(&request("failed", ProviderKind::Cloudflare, HostClass::Large, FEB));
        costs.fold(&end("failed", 0, 0, FEB + 1_000));
        let usage = costs.lease("failed").unwrap();
        assert_eq!(usage.start_ms, None);
        assert_eq!(usage.end_ms, Some(FEB + 1_000));
        assert_eq!(usage.ran_ms, Some(0));
        assert_eq!(costs.lease_cost("failed", FEB + MINUTE), Some(LeaseCost::default()));
    }

    #[test]
    fn prorates_completed_and_running_hosts_at_utc_month_rollover() {
        let costs = HostCosts::configured(
            Some(r#"{"fly":{"small_usd_per_second":0.01}}"#),
            None,
        )
        .unwrap();
        for id in ["ended", "running", "last_month", "future"] {
            costs.fold(&request(id, ProviderKind::Fly, HostClass::Small, FEB - 2 * MINUTE));
        }
        costs.fold(&boot("last_month", FEB - 2 * MINUTE));
        costs.fold(&end("last_month", 60_000, 0, FEB - MINUTE));
        costs.fold(&boot("ended", FEB - MINUTE));
        costs.fold(&boot("running", FEB - MINUTE));
        costs.fold(&end("ended", 120_000, 0, FEB + MINUTE));
        costs.fold(&boot("future", FEB + 3 * MINUTE));

        close(costs.month_to_date(FEB - 1_000), 1.78);
        close(costs.month_to_date(FEB), 0.0);
        close(costs.month_to_date(FEB + 2 * MINUTE), 1.8);
        close(costs.lease_cost("ended", FEB + 2 * MINUTE).unwrap().cost_usd, 1.2);
    }

    #[test]
    fn charges_each_cloudflare_tick_once_across_midnight() {
        let costs = HostCosts::configured(
            Some(r#"{"cloudflare":{"usd_per_vcpu_second":1,"usd_per_gib_second":0}}"#),
            None,
        )
        .unwrap();
        costs.fold(&request("match", ProviderKind::Cloudflare, HostClass::Small, FEB - 5));
        costs.fold(&boot("match", FEB - 5));
        close(costs.month_to_date(FEB - 1), 0.01);
        close(costs.month_to_date(FEB + 5), 0.0);
        close(costs.month_to_date(FEB + 6), 0.01);
        close(costs.lease_cost("match", FEB + 6).unwrap().cost_usd, 0.02);
    }

    #[test]
    fn skips_unknown_providers_and_unrelated_or_invalid_events() {
        let costs = HostCosts::default();
        let mut unknown = request("unknown", ProviderKind::Fly, HostClass::Large, FEB);
        unknown.data["lease"]["provider"] = json!("unpriced-provider");
        costs.fold(&unknown);
        let mut unrelated = request("other", ProviderKind::Fly, HostClass::Large, FEB);
        unrelated.stream = "other".into();
        costs.fold(&unrelated);
        let mut bad_time = request("bad-time", ProviderKind::Fly, HostClass::Large, FEB);
        bad_time.at = "yesterday".into();
        costs.fold(&bad_time);
        for id in ["unknown", "other", "bad-time", "missing"] {
            costs.fold(&boot(id, FEB + 1_000));
            costs.fold(&playing(id, 16, FEB + 2_000));
            costs.fold(&end(id, 60_000, 16, FEB + 61_000));
            assert!(costs.lease(id).is_none());
            assert!(costs.lease_cost(id, FEB + 61_000).is_none());
        }
        assert_eq!(costs.month_to_date(FEB + 61_000), 0.0);
    }

    #[test]
    fn budget_trips_at_the_limit_including_running_spend() {
        let costs = HostCosts::configured(
            Some(r#"{"fly":{"small_usd_per_second":1}}"#),
            None,
        )
        .unwrap();
        assert_eq!(costs.budget_usd_month(), 5.0);
        costs.fold(&request("ended", ProviderKind::Fly, HostClass::Small, FEB));
        costs.fold(&end("ended", 2_000, 2, FEB + 2_000));
        costs.fold(&request("running", ProviderKind::Fly, HostClass::Small, FEB + 2_000));
        costs.fold(&boot("running", FEB + 2_000));

        assert!(!costs.over_budget(FEB + 4_999));
        assert_eq!(costs.month_to_date(FEB + 5_000), 5.0);
        assert!(costs.over_budget(FEB + 5_000));
        assert!(costs.over_budget(FEB + 5_001));
        assert!(HostCosts::configured(None, Some("0")).unwrap().over_budget(FEB));
    }

    #[test]
    fn partial_price_overrides_keep_other_defaults() {
        let costs = HostCosts::configured(
            Some(r#"{"egress_gb_per_player_minute":0.001,"cloudflare":{"usd_per_egress_gb":0.1}}"#),
            Some(" 0.0017 "),
        )
        .unwrap();
        assert_eq!(costs.budget_usd_month(), 0.0017);
        costs.fold(&request("match", ProviderKind::Cloudflare, HostClass::Small, FEB));
        costs.fold(&end("match", 60_000, 3, FEB + MINUTE));
        let cost = costs.lease_cost("match", FEB + MINUTE).unwrap();
        close(cost.estimated_egress_gb, 0.003);
        close(cost.cost_usd, 0.0018);
        assert!(costs.over_budget(FEB + MINUTE));
    }

    #[test]
    fn rejects_invalid_price_and_budget_overrides() {
        assert!(matches!(
            HostCosts::configured(Some(r#"{"fly":{"small_usd_per_second":-1}}"#), None),
            Err(HostCostConfigError::PricesRange)
        ));
        for invalid in [
            "{",
            r#"{"new_provider":{}}"#,
            r#"{"fly":{"small_usd_per_second":"1"}}"#,
        ] {
            assert!(matches!(
                HostCosts::configured(Some(invalid), None),
                Err(HostCostConfigError::PricesJson(_))
            ));
        }
        for invalid in ["", "NaN", "inf", "-1"] {
            assert!(matches!(
                HostCosts::configured(None, Some(invalid)),
                Err(HostCostConfigError::Budget)
            ));
        }
    }
}
