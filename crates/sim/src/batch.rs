//! Admission and estimated compute costs for batches on dedicated hosts.

use crate::clock::GAME_HZ;
use crate::config::{MatchConfig, TIME_LIMIT_SETTING};
use serde::Deserialize;

pub const DEFAULT_MAX_PARALLEL: usize = 4;
pub const DEFAULT_MAX_MATCH_S: u32 = 1800;
pub const MAX_MATCH_S: u32 = 7200;
const STARTUP_AND_UPLOAD_S: u32 = 60;
const MICROS_PER_USD: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    Cloudflare,
    Fly,
}

impl CloudProvider {
    fn hourly_microusd(self, large: bool) -> u64 {
        // Full active CPU and memory for Small (1 vCPU / 2 GiB), or
        // Fly's performance-1x/2 GB. Large doubles both resources.
        let small = match self {
            Self::Cloudflare => 90_000,
            Self::Fly => 46_000,
        };
        if large { 2 * small } else { small }
    }
}

pub fn budget_microusd(usd: f64) -> Result<u64, &'static str> {
    let micros = usd * MICROS_PER_USD;
    if !micros.is_finite() || !(0.0..u64::MAX as f64).contains(&micros) {
        return Err("budget must be a finite nonnegative USD amount");
    }
    Ok(micros.floor() as u64)
}

pub fn dollars(microusd: u64) -> f64 {
    microusd as f64 / MICROS_PER_USD
}

/// The lease must allow time for provisioning and the report beyond the
/// match's own wall clock, which starts when sim mode boots.
pub fn max_match_s(config: &MatchConfig) -> u32 {
    config.max_wall_s.map_or(DEFAULT_MAX_MATCH_S, |wall| {
        wall.saturating_add(u64::from(STARTUP_AND_UPLOAD_S))
            .min(u64::from(MAX_MATCH_S)) as u32
    })
}

pub fn estimate_microusd(config: &MatchConfig, provider: CloudProvider) -> u64 {
    let time_limit = config
        .time_limit_s
        .map(u64::from)
        .or_else(|| {
            config
                .settings
                .iter()
                .find(|(key, _)| key.trim().parse::<u32>().ok() == Some(TIME_LIMIT_SETTING))
                .and_then(|(_, value)| u64::try_from(*value).ok())
        })
        .filter(|seconds| *seconds > 0);
    let game_s = config
        .max_ticks
        .map(|ticks| ticks.div_ceil(GAME_HZ))
        .into_iter()
        .chain(time_limit)
        .min();
    let max_secs = u64::from(max_match_s(config));
    // The two-bot estimate in dedicated-host.md was 2.8x unpaced.
    let billable_secs = game_s
        .map(|seconds| {
            u64::from(STARTUP_AND_UPLOAD_S).saturating_add(seconds.saturating_mul(10).div_ceil(28))
        })
        .unwrap_or(max_secs)
        .min(max_secs);
    let hourly = provider.hourly_microusd(config.bots.len() > 4);
    (billable_secs * hourly).div_ceil(3600)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Ready,
    AtCapacity,
    OverBudget,
}

pub struct Schedule {
    max_parallel: usize,
    budget: Option<u64>,
    reserved: u64,
}

impl Schedule {
    pub fn new(max_parallel: usize, budget: Option<u64>) -> Self {
        Self {
            max_parallel,
            budget,
            reserved: 0,
        }
    }

    /// Reserve cost before the POST. A failed or early finish still counts
    /// toward the estimate because the provider may have allocated a host.
    pub fn admit(&mut self, active: usize, cost: u64) -> Admission {
        if active >= self.max_parallel {
            return Admission::AtCapacity;
        }
        let reserved = self.reserved.saturating_add(cost);
        if self.budget.is_some_and(|budget| reserved > budget) {
            return Admission::OverBudget;
        }
        self.reserved = reserved;
        Admission::Ready
    }

    pub fn reserved(&self) -> u64 {
        self.reserved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BotSpec;

    #[test]
    fn schedule_reuses_a_slot_but_keeps_its_budget_reservation() {
        let mut schedule = Schedule::new(2, Some(15));
        assert_eq!(schedule.admit(0, 10), Admission::Ready);
        assert_eq!(schedule.admit(1, 4), Admission::Ready);
        assert_eq!(schedule.admit(2, 1), Admission::AtCapacity);
        assert_eq!(schedule.reserved(), 14);
        assert_eq!(schedule.admit(1, 1), Admission::Ready);
        assert_eq!(schedule.admit(0, 1), Admission::OverBudget);
        assert_eq!(schedule.reserved(), 15);
    }

    #[test]
    fn schedule_with_zero_budget_can_admit_only_zero_cost() {
        let mut schedule = Schedule::new(1, Some(0));
        assert_eq!(schedule.admit(0, 1), Admission::OverBudget);
        assert_eq!(schedule.admit(0, 0), Admission::Ready);
        assert_eq!(budget_microusd(0.01), Ok(10_000));
        assert!(budget_microusd(f64::NAN).is_err());
        assert!(budget_microusd(f64::INFINITY).is_err());
        assert!(budget_microusd(-0.01).is_err());
    }

    #[test]
    fn estimate_uses_provider_class_and_the_shortest_limit() {
        let mut config = MatchConfig {
            bots: vec![BotSpec::default(); 4],
            time_limit_s: Some(560),
            max_ticks: Some(GAME_HZ * 280),
            max_wall_s: Some(1200),
            ..Default::default()
        };
        // 60 s setup and upload, 100 s for 280 game seconds.
        assert_eq!(estimate_microusd(&config, CloudProvider::Cloudflare), 4000);
        assert_eq!(estimate_microusd(&config, CloudProvider::Fly), 2045);
        config.bots.push(BotSpec::default());
        assert_eq!(estimate_microusd(&config, CloudProvider::Cloudflare), 8000);
        config.max_wall_s = Some(10);
        assert_eq!(max_match_s(&config), 70);
        assert_eq!(estimate_microusd(&config, CloudProvider::Cloudflare), 3500);
        config.max_wall_s = None;
        config.max_ticks = None;
        config.time_limit_s = Some(0);
        assert_eq!(estimate_microusd(&config, CloudProvider::Cloudflare), 90_000);
    }

    #[test]
    fn estimate_reads_the_variant_timer_and_caps_long_leases() {
        let config = MatchConfig {
            settings: [(TIME_LIMIT_SETTING.to_string(), 28)].into(),
            max_wall_s: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(max_match_s(&config), MAX_MATCH_S);
        assert_eq!(estimate_microusd(&config, CloudProvider::Cloudflare), 1750);
    }
}
