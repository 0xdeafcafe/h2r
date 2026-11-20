//! Estimated dedicated-host spend, projected from the verified hosts stream.

use std::collections::BTreeMap;
use std::sync::Mutex;

use live_proto::hosts::{
    self, HostBooting, HostClass, HostEnded, HostLease, HostPlaying, HostReady, HostRequested,
    HostStopped, PackUsage, ProviderKind,
};
use live_proto::sign::{EventBody, parse_time};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

mod warm;

pub const DEFAULT_WORKER_BUDGET_USD: f64 = 25.0;

// Both Wrangler container applications provision 3 GiB per vCPU and 12 GB disk.
const CLOUDFLARE_GIB_PER_VCPU: f64 = 3.0;
const CLOUDFLARE_DISK_GB: f64 = 12.0;

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
    usd_per_gb_disk_second: f64,
    usd_per_start: f64,
}

impl Default for CloudflarePrices {
    fn default() -> Self {
        Self {
            // Cloudflare rates before included allowances, with full CPU usage.
            // https://developers.cloudflare.com/containers/platform/pricing/
            usd_per_vcpu_second: 0.000_02,
            usd_per_gib_second: 0.000_002_5,
            usd_per_egress_gb: 0.025,
            usd_per_gb_disk_second: 0.000_000_07,
            // Planning allowance for each start, separate from Cloudflare's rates.
            usd_per_start: 0.001,
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
    #[error("LIVE_HOST_BUDGET_USD must be a finite non-negative number")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_usage: Option<PackUsage>,
    #[serde(skip)]
    max_players: u8,
    #[serde(skip)]
    deadline_ms: i64,
    #[serde(skip)]
    managed: bool,
    #[serde(skip)]
    pack_linger: bool,
    #[serde(skip)]
    stopped_ms: Option<i64>,
}

impl LeaseUsage {
    fn requested(lease: &HostLease, at_ms: i64, managed: bool) -> Self {
        Self {
            lease_id: lease.id.clone(),
            provider: lease.provider.clone(),
            class: lease.class.clone(),
            region: lease.region.clone(),
            requested_ms: at_ms,
            start_ms: managed.then_some(lease.created_ms),
            end_ms: None,
            ran_ms: None,
            players_peak: 0,
            pack_usage: None,
            max_players: lease.max_players,
            deadline_ms: lease.deadline_ms(),
            managed,
            pack_linger: false,
            stopped_ms: None,
        }
    }

    /// Both live reports and replayed events must fit the interval measured by Live.
    pub(crate) fn accepts_pack_usage(&self, usage: &PackUsage, ended_ms: i64) -> bool {
        let elapsed_ms = ended_ms.saturating_sub(self.requested_ms).max(0) as u64;
        // The supervisor reports whole microseconds. Round the lower bound down.
        let minimum_share = usage.slot_us / live_proto::host_pack::MAX_PACK_MATCHES as u64;
        let maximum_cpu = usage.slot_us.saturating_mul(u64::from(self.class.vcpu()));
        usage.slot_us > 0
            && usage.slot_us <= elapsed_ms.saturating_mul(1000)
            && (minimum_share..=usage.slot_us).contains(&usage.container_share_us)
            && usage.cpu_us.is_none_or(|cpu| cpu <= maximum_cpu)
    }

    fn cloudflare_resources(&self, start_ms: i64, from: u64, to: u64, reserving: bool) -> Resources {
        let seconds = tick_seconds(from, to);
        let vcpus = f64::from(self.class.vcpu());
        let full = Resources { container_seconds: seconds, cpu_seconds: seconds * vcpus, charge_start: true };
        // An open reservation covers the full class. A manually launched
        // host has no provider-managed pack from which to claim a discount.
        let (Some(usage), Some(reported_at)) = (self.pack_usage, self.end_ms) else {
            return full;
        };
        if !self.managed || reserving {
            return full;
        }

        // Split each monthly window into the observed slot interval and
        // the launch/cleanup intervals, which retain full-capacity charges.
        let observed_end = reported_at.saturating_sub(start_ms).max(0) as u64;
        let observed_start = observed_end.saturating_sub(usage.slot_us / 1000);
        let occupied = tick_seconds(from.clamp(observed_start, observed_end), to.clamp(observed_start, observed_end));
        let outside = seconds - occupied;
        let share = usage.container_share_us as f64 / usage.slot_us as f64;
        let cpu_per_second = usage.cpu_us.map_or(vcpus, |cpu| cpu as f64 / usage.slot_us as f64);
        Resources {
            container_seconds: outside + occupied * share,
            cpu_seconds: outside * vcpus + occupied * cpu_per_second,
            charge_start: usage.charge_start,
        }
    }

    fn billing_end(&self) -> Option<i64> {
        if self.managed { self.stopped_ms } else { self.end_ms }
    }
}

struct Resources {
    container_seconds: f64,
    cpu_seconds: f64,
    charge_start: bool,
}

/// Cumulative tick boundaries charge a 10 ms tick crossing midnight once.
fn tick_seconds(from_ms: u64, to_ms: u64) -> f64 {
    (to_ms.div_ceil(10) - from_ms.div_ceil(10)) as f64 / 100.0
}

/// Estimated usage and USD for a lease through a given time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct LeaseCost {
    pub ran_ms: u64,
    pub estimated_egress_gb: f64,
    pub cost_usd: f64,
}

/// Replay as a dispatcher subscriber or fold event bodies directly.
#[derive(Debug, Default)]
pub struct HostCosts {
    prices: PriceTable,
    budget_usd_month: Option<f64>,
    leases: Mutex<BTreeMap<String, LeaseUsage>>,
    warm: Mutex<warm::WarmLedger>,
}

impl HostCosts {
    /// Use the deployment's cap by default. The legacy variable remains an
    /// alias so a deployment with an existing cap keeps it on upgrade.
    pub fn from_env(
        env: &dyn Fn(&str) -> Option<String>,
        default_budget_usd: Option<f64>,
    ) -> Result<Self, HostCostConfigError> {
        let budget = env("LIVE_HOST_BUDGET_USD").or_else(|| env("LIVE_HOST_BUDGET_USD_MONTH"));
        let mut costs = Self::configured(env("LIVE_HOST_PRICES").as_deref(), budget.as_deref())?;
        costs.budget_usd_month = costs.budget_usd_month.or(default_budget_usd);
        Ok(costs)
    }

    /// Pass the values of LIVE_HOST_PRICES and LIVE_HOST_BUDGET_USD.
    /// Omitted prices use the defaults and a missing cap is unlimited.
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
                prices.cloudflare.usd_per_gb_disk_second,
                prices.cloudflare.usd_per_start,
                prices.fly.small_usd_per_second,
                prices.fly.large_usd_per_second,
            ];
            if !rates.into_iter().all(valid_rate) {
                return Err(HostCostConfigError::PricesRange);
            }
            costs.prices = prices;
        }
        if let Some(value) = budget_usd_month {
            costs.budget_usd_month = Some(
                value
                    .trim()
                    .parse()
                    .ok()
                    .filter(|&value| valid_rate(value))
                    .ok_or(HostCostConfigError::Budget)?,
            );
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
        if event.kind == hosts::HOST_PACK_WARM_RESERVED {
            if let Ok(grant) = hosts::HostPackWarmReserved::deserialize(&event.data) {
                self.warm.lock().unwrap().apply(&grant);
            }
            return;
        }
        let mut leases = self.leases.lock().unwrap();
        match event.kind.as_str() {
            hosts::HOST_REQUESTED => {
                if let Ok(HostRequested { lease, provider_ref, pack_linger }) =
                    HostRequested::deserialize(&event.data)
                {
                    leases.entry(lease.id.clone()).or_insert_with(|| {
                        let mut usage = LeaseUsage::requested(&lease, at_ms, provider_ref.is_some());
                        usage.pack_linger = pack_linger && usage.managed && lease.provider == ProviderKind::Cloudflare;
                        usage
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
                    if !lease.managed && (data.ran_ms > 0 || lease.start_ms.is_some()) {
                        lease.start_ms = Some(at_ms.saturating_sub_unsigned(data.ran_ms));
                    }
                    lease.end_ms = Some(at_ms);
                    lease.ran_ms = Some(data.ran_ms);
                    lease.pack_usage = data.pack_usage.filter(|usage| lease.accepts_pack_usage(usage, at_ms));
                    lease.players_peak = lease.players_peak.max(data.players_peak);
                }
            }
            hosts::HOST_STOPPED => {
                if let Ok(data) = HostStopped::deserialize(&event.data)
                    && let Some(lease) = leases.get_mut(&data.lease_id)
                    && lease.managed
                {
                    lease.stopped_ms.get_or_insert(at_ms);
                }
            }
            _ => {}
        }
    }

    pub fn lease(&self, lease_id: &str) -> Option<LeaseUsage> {
        self.leases.lock().unwrap().get(lease_id).cloned()
    }

    pub(crate) fn stop_confirmed(&self, lease_id: &str) -> Option<bool> {
        let leases = self.leases.lock().unwrap();
        let lease = leases.get(lease_id)?;
        lease.managed.then_some(lease.stopped_ms.is_some())
    }

    /// Price a lease through now_ms, in Unix milliseconds.
    pub fn lease_cost(&self, lease_id: &str, now_ms: i64) -> Option<LeaseCost> {
        let leases = self.leases.lock().unwrap();
        let lease = leases.get(lease_id)?;
        Some(self.estimate(lease, i64::MIN, now_ms, false))
    }

    /// Accrued USD in the current UTC month, excluding unspent reservations.
    pub fn month_to_date(&self, now_ms: i64) -> f64 {
        let Some((month_start, _)) = utc_month(now_ms) else {
            return 0.0;
        };
        self.leases
            .lock()
            .unwrap()
            .values()
            .map(|lease| self.estimate(lease, month_start, now_ms, false).cost_usd)
            .sum::<f64>()
            + self.warm.lock().unwrap().cost(&self.prices.cloudflare, month_start, now_ms)
    }

    pub fn budget_usd_month(&self) -> Option<f64> {
        self.budget_usd_month
    }

    /// Called under the host writer lock, before appending HostRequested.
    /// In-flight starts and running hosts already claim their full duration.
    pub(crate) fn can_start(&self, lease: &HostLease, pack_linger: bool) -> bool {
        let mut candidate = LeaseUsage::requested(lease, lease.created_ms, false);
        candidate.pack_linger = pack_linger;
        self.fits_budget(&candidate, None)
    }

    fn fits_budget(&self, candidate: &LeaseUsage, grant: Option<&hosts::HostPackWarmReserved>) -> bool {
        let Some(cap) = self.budget_usd_month else {
            return true;
        };
        let now = candidate.requested_ms;
        let Some((mut from, mut until)) = utc_month(now) else {
            return false;
        };
        let leases = self.leases.lock().unwrap();
        // Fail closed while billable compute could be running past its limit.
        if cap == 0.0
            || leases.values().any(|lease| {
                lease.managed && lease.stopped_ms.is_none() && lease.deadline_ms <= now
            })
        {
            return false;
        }

        // Require room for a full match in the admission month, even when
        // some of the runtime will be charged to the next month.
        let finish = candidate.deadline_ms.saturating_add(if candidate.pack_linger {
            live_proto::host_pack::LINGER_MS
        } else { 0 });
        let mut additional = self.estimate(candidate, now, finish, true).cost_usd;
        loop {
            let committed: f64 = leases
                .values()
                .map(|lease| self.estimate(lease, from, until, true).cost_usd)
                .sum();
            let warm = self.warm.lock().unwrap().cost(&self.prices.cloudflare, from, until);
            let extension = grant.map_or(0.0, |g| warm::grant_cost(g, &self.prices.cloudflare, from, until));
            let projected = committed + additional + warm + extension;
            if !projected.is_finite() || projected > cap {
                return false;
            }
            if finish <= until {
                return true;
            }
            let Some(next) = utc_month(until) else {
                return false;
            };
            (from, until) = next;
            additional = self.estimate(candidate, from, until, true).cost_usd;
        }
    }

    fn estimate(&self, lease: &LeaseUsage, from_ms: i64, to_ms: i64, forecast: bool) -> LeaseCost {
        let mut cost = self.estimate_match(lease, from_ms, to_ms, forecast);
        if lease.pack_linger {
            let start = lease.stopped_ms.or(forecast.then_some(lease.deadline_ms));
            if let Some(start) = start {
                let tail = hosts::PackWarmPeriod {
                    pack: String::new(), from_ms: start,
                    until_ms: start.saturating_add(live_proto::host_pack::LINGER_MS), start: false,
                };
                cost.cost_usd += warm::period_cost(&lease.class, &tail, &self.prices.cloudflare, from_ms, to_ms);
            }
        }
        cost
    }

    fn estimate_match(&self, lease: &LeaseUsage, from_ms: i64, to_ms: i64, forecast: bool) -> LeaseCost {
        let reserving = forecast && lease.billing_end().is_none();
        let start = match lease.start_ms {
            Some(start) => start,
            None if reserving => lease.requested_ms,
            None => return LeaseCost::default(),
        };
        let end = lease
            .billing_end()
            .unwrap_or(if reserving { lease.deadline_ms } else { to_ms });
        let elapsed = |at: i64| at.min(end).max(start).abs_diff(start);
        let from = elapsed(from_ms);
        let to = elapsed(to_ms);
        let ran_ms = to - from;
        // The flat fee belongs to the UTC month containing the start,
        // including a start that has yet to accrue its first tick.
        let charge_start = from_ms <= start && (start < to_ms || !forecast && start == to_ms);
        if ran_ms == 0 && !charge_start {
            return LeaseCost::default();
        }
        let players = if reserving {
            lease.max_players
        } else {
            lease.players_peak
        };
        let player_minutes = ran_ms as f64 / 60_000.0 * f64::from(players);
        let estimated_egress_gb = player_minutes * self.prices.egress_gb_per_player_minute;
        let cost_usd = match lease.provider {
            ProviderKind::Local => 0.0,
            ProviderKind::Cloudflare => {
                let resources = lease.cloudflare_resources(start, from, to, reserving);
                let rates = &self.prices.cloudflare;
                let cpu = resources.cpu_seconds * rates.usd_per_vcpu_second;
                let memory = resources.container_seconds * f64::from(lease.class.vcpu())
                    * CLOUDFLARE_GIB_PER_VCPU * rates.usd_per_gib_second;
                let disk = resources.container_seconds * CLOUDFLARE_DISK_GB * rates.usd_per_gb_disk_second;
                let egress = if rates.usd_per_egress_gb > 0.0 {
                    estimated_egress_gb * rates.usd_per_egress_gb
                } else {
                    0.0
                };
                let start_fee = if charge_start && resources.charge_start { rates.usd_per_start } else { 0.0 };
                cpu + memory + disk + egress + start_fee
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

fn utc_month(now_ms: i64) -> Option<(i64, i64)> {
    let now = OffsetDateTime::from_unix_timestamp_nanos(i128::from(now_ms) * 1_000_000).ok()?;
    let start = Date::from_calendar_date(now.year(), now.month(), 1).ok()?;
    let year = now.year() + i32::from(now.month() == time::Month::December);
    let end = Date::from_calendar_date(year, now.month().next(), 1).ok()?;
    let unix_ms = |date: Date| date.midnight().assume_utc().unix_timestamp() * 1_000;
    Some((unix_ms(start), unix_ms(end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hosts::{DEFAULT_IDLE_TIMEOUT_S, DEFAULT_MAX_MATCH_S, Transport};
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
        close(
            settled.cost_usd,
            0.64 * 0.000_028_34 + settled.estimated_egress_gb * 0.025 + 0.001,
        );
    }

    #[test]
    fn prices_the_documented_ten_minute_matches() {
        for (provider, class, players, expected) in [
            (ProviderKind::Cloudflare, HostClass::Small, 2, 0.018254),
            (ProviderKind::Cloudflare, HostClass::Large, 16, 0.036504),
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
        close(
            costs.month_to_date(now),
            3.0 * (60.0 * 0.000_028_34 + 0.001) + 0.001 * 0.025,
        );
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
            Some(r#"{"cloudflare":{"usd_per_vcpu_second":1,"usd_per_gib_second":0,"usd_per_gb_disk_second":0,"usd_per_start":0}}"#),
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

    fn budget_fixture() -> HostCosts {
        HostCosts::configured(
            Some(r#"{"cloudflare":{"usd_per_vcpu_second":1,"usd_per_gib_second":0,"usd_per_gb_disk_second":0,"usd_per_egress_gb":0,"usd_per_start":1}}"#),
            Some("8"),
        )
        .unwrap()
    }

    fn managed_request(id: &str, start_ms: i64, max_match_s: u32) -> EventBody {
        let mut event = request(id, ProviderKind::Cloudflare, HostClass::Small, start_ms);
        event.data["provider_ref"] = json!(format!("room/{id}"));
        event.data["lease"]["max_match_s"] = json!(max_match_s);
        event
    }

    fn candidate(now: i64, max_match_s: u32) -> HostLease {
        HostRequested::deserialize(&managed_request("next", now, max_match_s).data)
            .unwrap()
            .lease
    }

    #[test]
    fn four_packed_matches_settle_to_one_container_and_one_start() {
        let costs = HostCosts::default();
        let full = HostCosts::default();
        let at = FEB + MINUTE;
        full.fold(&managed_request("full", FEB, 120));
        full.fold(&end("full", MINUTE as u64, 0, at));
        full.fold(&event(hosts::HOST_STOPPED, json!({ "lease_id": "full" }), at));
        for i in 0..4 {
            let id = format!("slot-{i}");
            costs.fold(&managed_request(&id, FEB, 120));
            let mut ended = end(&id, MINUTE as u64, 0, at);
            ended.data["pack_usage"] = json!({
                "slot_us": 60_000_000, "container_share_us": 15_000_000,
                "cpu_us": 15_000_000, "charge_start": i == 0, "peak_pss_bytes": 123_000
            });
            costs.fold(&ended);
            costs.fold(&ended);
            costs.fold(&event(hosts::HOST_STOPPED, json!({ "lease_id": id }), at));
        }
        close(costs.month_to_date(at), full.month_to_date(at));
        assert_eq!(costs.lease("slot-2").unwrap().pack_usage.unwrap().peak_pss_bytes, Some(123_000));
    }

    #[test]
    fn replay_prices_the_full_class_when_a_pack_report_is_out_of_bounds() {
        let at = FEB + 1_000;
        let requested = managed_request("match", FEB, 60);
        let stopped = event(hosts::HOST_STOPPED, json!({ "lease_id": "match" }), at);
        let replay = |usage: Option<PackUsage>| {
            let costs = HostCosts::default();
            let mut ended = end("match", 1_000, 0, at);
            if let Some(usage) = usage {
                ended.data["pack_usage"] = json!(usage);
            }
            costs.fold(&requested);
            costs.fold(&ended);
            costs.fold(&stopped);
            assert_eq!(costs.lease("match").unwrap().end_ms, Some(at));
            (costs.lease("match").unwrap().pack_usage, costs.month_to_date(at))
        };
        let (missing, full_cost) = replay(None);
        assert!(missing.is_none());
        assert!(full_cost > 0.0);
        let valid = PackUsage {
            slot_us: 1_000_000,
            container_share_us: 100_000,
            charge_start: false,
            cpu_us: Some(1_000_000),
            peak_pss_bytes: None,
        };
        let (kept, discounted_cost) = replay(Some(valid));
        assert_eq!(kept, Some(valid));
        assert!(discounted_cost < full_cost);
        for usage in [
            PackUsage { slot_us: 0, container_share_us: 0, cpu_us: None, ..valid },
            PackUsage { slot_us: 1_000_001, ..valid },
            PackUsage { container_share_us: 99_999, ..valid },
            PackUsage { container_share_us: 1_000_001, ..valid },
            PackUsage { cpu_us: Some(1_000_001), ..valid },
        ] {
            let (kept, charge) = replay(Some(usage));
            assert_eq!(kept, None, "{usage:?}");
            close(charge, full_cost);
        }
    }

    #[test]
    fn pack_report_bounds_allow_whole_microsecond_rounding_and_both_cores() {
        let mut large = candidate(FEB, 60);
        large.class = HostClass::Large;
        let lease = LeaseUsage::requested(&large, FEB, true);
        let usage = PackUsage {
            slot_us: 999_999,
            container_share_us: 99_999,
            charge_start: true,
            cpu_us: Some(1_999_998),
            peak_pss_bytes: None,
        };
        assert!(lease.accepts_pack_usage(&usage, FEB + 1_000));
        assert!(!lease.accepts_pack_usage(&PackUsage { cpu_us: Some(1_999_999), ..usage }, FEB + 1_000));
        assert!(!lease.accepts_pack_usage(&PackUsage { slot_us: u64::MAX, ..usage }, FEB));
    }

    #[test]
    fn packed_costs_keep_launch_and_cleanup_and_split_at_month_boundary() {
        let costs = HostCosts::configured(
            Some(r#"{"cloudflare":{"usd_per_vcpu_second":1,"usd_per_gib_second":0,"usd_per_gb_disk_second":0,"usd_per_start":0}}"#),
            None,
        ).unwrap();
        costs.fold(&managed_request("a", FEB - 3_000, 120));
        // 1 second of launch, 4 seconds at 25% CPU, then 2 seconds of cleanup.
        let mut ended = end("a", 4_000, 0, FEB + 2_000);
        ended.data["pack_usage"] = json!({
            "slot_us": 4_000_000, "container_share_us": 2_000_000,
            "cpu_us": 1_000_000, "charge_start": true
        });
        costs.fold(&ended);
        costs.fold(&event(hosts::HOST_STOPPED, json!({ "lease_id": "a" }), FEB + 4_000));
        close(costs.lease_cost("a", FEB + MINUTE).unwrap().cost_usd, 4.0);
        close(costs.month_to_date(FEB + MINUTE), 2.5);
    }

    #[test]
    fn only_new_managed_packed_leases_reserve_the_empty_hold() {
        let costs = HostCosts::configured(
            Some(r#"{"egress_gb_per_player_minute":0,"cloudflare":{"usd_per_vcpu_second":1,"usd_per_gib_second":0,"usd_per_gb_disk_second":0,"usd_per_start":0}}"#),
            Some("59"),
        ).unwrap();
        let lease = candidate(FEB, 1);
        assert!(costs.can_start(&lease, false));
        assert!(!costs.can_start(&lease, true));

        for (name, has_hold) in [("legacy", false), ("packed", true)] {
            let mut request = managed_request(name, FEB, 2);
            if has_hold {
                request.data["pack_linger"] = json!(true);
            }
            costs.fold(&request);
            costs.fold(&event(hosts::HOST_STOPPED, json!({ "lease_id": name }), FEB+1_000));
        }
        let legacy = costs.lease_cost("legacy", FEB + 100_000).unwrap();
        let packed = costs.lease_cost("packed", FEB + 100_000).unwrap();
        close(legacy.cost_usd, 1.0);
        close(packed.cost_usd, 61.0);
        assert_eq!(packed.ran_ms, legacy.ran_ms);
        close(costs.month_to_date(FEB + 20_000), 21.0);
    }

    #[test]
    fn reserves_pending_cloudflare_starts_and_settles_once_the_room_stops() {
        let costs = budget_fixture();
        assert!(costs.can_start(&candidate(FEB, 4), false));
        assert!(!costs.can_start(&candidate(FEB, 8), false));
        costs.fold(&managed_request("match", FEB, 4));
        close(costs.month_to_date(FEB), 1.0);
        assert!(!costs.can_start(&candidate(FEB, 4), false));

        // A game can report that it ended while its container is still up.
        costs.fold(&end("match", 100, 2, FEB + 1_000));
        assert_eq!(costs.stop_confirmed("match"), Some(false));
        assert!(!costs.can_start(&candidate(FEB + 1_000, 4), false));
        close(costs.month_to_date(FEB + 2_000), 3.0);
        assert!(!costs.can_start(&candidate(FEB + 4_000, 1), false));

        for at in [FEB + 2_000, FEB + 3_000] {
            costs.fold(&event(hosts::HOST_STOPPED, json!({ "lease_id": "match" }), at));
        }
        assert_eq!(costs.stop_confirmed("match"), Some(true));
        close(costs.month_to_date(FEB + 5_000), 3.0);
        assert!(costs.can_start(&candidate(FEB + 5_000, 4), false));
        assert!(!costs.can_start(&candidate(FEB + 5_000, 5), false));
    }

    #[test]
    fn checks_next_month_reservations_and_charges_the_start_fee_once() {
        let costs = budget_fixture();
        costs.fold(&managed_request("match", FEB - 1_000, 7));
        close(costs.month_to_date(FEB - 1), 2.0);
        // January can cover the reservation (2 + 5). February cannot (6 + 3).
        assert!(!costs.can_start(&candidate(FEB - 1_000, 4), false));
        close(costs.month_to_date(FEB), 0.0);
        close(costs.month_to_date(FEB + 1_000), 1.0);
        costs.fold(&event(
            hosts::HOST_STOPPED,
            json!({ "lease_id": "match" }),
            FEB + 1_000,
        ));
        close(costs.month_to_date(FEB + MINUTE), 1.0);
        assert!(costs.can_start(&candidate(FEB + MINUTE, 4), false));
    }

    #[test]
    fn environment_defaults_and_legacy_cap() {
        let absent = |_: &str| None;
        let worker = Some(DEFAULT_WORKER_BUDGET_USD);
        let budget = |env: &dyn Fn(&str) -> Option<String>, default| {
            HostCosts::from_env(env, default).unwrap().budget_usd_month()
        };
        assert_eq!(budget(&absent, worker), Some(25.0));
        assert_eq!(budget(&absent, None), None);
        let legacy = |key: &str| (key == "LIVE_HOST_BUDGET_USD_MONTH").then(|| "3".into());
        assert_eq!(budget(&legacy, worker), Some(3.0));
        let override_cap = |key: &str| {
            if key == "LIVE_HOST_BUDGET_USD" {
                Some("0".into())
            } else {
                legacy(key)
            }
        };
        let disabled = HostCosts::from_env(&override_cap, worker).unwrap();
        assert!(!disabled.can_start(&candidate(FEB, 4), false));
    }

    #[test]
    fn partial_price_overrides_keep_other_defaults() {
        let costs = HostCosts::configured(
            Some(r#"{"egress_gb_per_player_minute":0.001,"cloudflare":{"usd_per_egress_gb":0.1}}"#),
            Some(" 0.0029 "),
        )
        .unwrap();
        assert_eq!(costs.budget_usd_month(), Some(0.0029));
        costs.fold(&request("match", ProviderKind::Cloudflare, HostClass::Small, FEB));
        costs.fold(&end("match", 60_000, 3, FEB + MINUTE));
        let cost = costs.lease_cost("match", FEB + MINUTE).unwrap();
        close(cost.estimated_egress_gb, 0.003);
        close(cost.cost_usd, 0.0030004);
        assert!(!costs.can_start(&candidate(FEB + MINUTE, 1), false));
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
