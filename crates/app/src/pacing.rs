//! Frame pacing: keeping guest time in step with the wall clock.
//!
//! The kernel HLE's clock (`kernel::Clock`) is logical. It counts 100 ns
//! ticks, never reads the host clock, and only moves when the scheduler
//! finds every guest thread waiting and jumps to the next timer deadline (or
//! when the host calls `Kernel::advance_time`). Guest compute is therefore
//! free in guest time, and a game loop that waits for its next frame
//! deadline would run as fast as the host CPU allows.
//!
//! [`Pacer`] closes that gap without touching the kernel. The guest thread
//! reports the guest clock between scheduler slices, and the pacer compares
//! guest time elapsed against wall time elapsed:
//!
//! * guest ahead by more than `max_lead`: sleep the difference (normal case
//!   on a fast host; this is what makes 60 Hz look like 60 Hz);
//! * guest behind by more than `max_lag` (a slow host, a breakpoint, a long
//!   load): rebase, so the game slows down rather than fast-forwarding later
//!   to catch up;
//! * otherwise run.
//!
//! [`VBlank`] is the display-refresh arithmetic the D3D HLE needs for
//! `BlockUntilVerticalBlank`, `GetDisplayFieldStatus` and vertical-blank
//! callbacks, expressed in the same 100 ns guest ticks.

use std::time::Duration;

/// Kernel clock rate: 100 ns ticks (`KeQueryPerformanceFrequency` in the
/// kernel HLE reports this, not the retail 733 MHz TSC rate).
pub const TICKS_PER_SECOND: u64 = 10_000_000;

/// Converts guest ticks to a duration.
pub fn ticks_to_duration(ticks: u64) -> Duration {
    Duration::from_nanos(ticks.saturating_mul(100))
}

/// Converts a duration to guest ticks (truncating).
pub fn duration_to_ticks(d: Duration) -> u64 {
    (d.as_nanos() / 100).min(u64::MAX as u128) as u64
}

/// A display refresh rate as an exact fraction (fields per second).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VBlank {
    /// Numerator of the rate in Hz.
    pub num: u64,
    /// Denominator of the rate in Hz.
    pub den: u64,
}

impl VBlank {
    /// NTSC-M, 60000/1001 (59.94) Hz: the North American retail console.
    pub const NTSC: VBlank = VBlank {
        num: 60_000,
        den: 1001,
    };
    /// PAL-60 / an even 60 Hz.
    pub const HZ60: VBlank = VBlank { num: 60, den: 1 };
    /// PAL-50.
    pub const PAL50: VBlank = VBlank { num: 50, den: 1 };

    /// Vertical blanks completed by guest time `ticks` (the first at time 0).
    pub fn count_at(self, ticks: u64) -> u64 {
        ((ticks as u128 * self.num as u128) / (TICKS_PER_SECOND as u128 * self.den as u128)) as u64
    }

    /// Guest time of vertical blank number `n`, rounded up to a whole tick.
    pub fn time_of(self, n: u64) -> u64 {
        (n as u128 * TICKS_PER_SECOND as u128 * self.den as u128).div_ceil(self.num as u128) as u64
    }

    /// Guest time of the first vertical blank strictly after `ticks`.
    pub fn next_after(self, ticks: u64) -> u64 {
        self.time_of(self.count_at(ticks) + 1)
    }

    /// One refresh period.
    pub fn period(self) -> Duration {
        Duration::from_nanos((1_000_000_000u128 * self.den as u128 / self.num as u128) as u64)
    }
}

/// How guest time is held to the wall clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PacePolicy {
    /// Never sleep (headless runs, traces, benchmarks).
    Unpaced,
    /// Hold guest time to wall time.
    RealTime {
        /// Guest time may run this far ahead before the guest thread sleeps.
        max_lead: Duration,
        /// Guest time may fall this far behind before the pacer rebases.
        max_lag: Duration,
    },
}

impl PacePolicy {
    /// Real time with a 2 ms lead allowance and a 100 ms lag allowance.
    pub const REAL_TIME: PacePolicy = PacePolicy::RealTime {
        max_lead: Duration::from_millis(2),
        max_lag: Duration::from_millis(100),
    };
}

impl Default for PacePolicy {
    fn default() -> Self {
        PacePolicy::REAL_TIME
    }
}

/// What the guest thread should do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaceAction {
    /// Keep running.
    Run,
    /// Guest is ahead: sleep this long, then continue.
    Sleep(Duration),
    /// Guest fell behind by `lag`; the pacer rebased and the lag is forgiven.
    Rebased { lag: Duration },
}

/// Counters for the status line and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaceStats {
    /// Sleeps requested.
    pub sleeps: u64,
    /// Total sleep requested.
    pub slept: Duration,
    /// Rebases after falling behind.
    pub rebases: u64,
    /// Largest lag forgiven by a rebase.
    pub worst_lag: Duration,
}

/// Maps guest time onto wall time. Pure: the caller supplies both clocks.
#[derive(Clone, Debug)]
pub struct Pacer {
    policy: PacePolicy,
    /// Guest ticks and wall time at the last (re)base.
    origin: Option<(u64, Duration)>,
    stats: PaceStats,
}

impl Pacer {
    pub fn new(policy: PacePolicy) -> Self {
        Self {
            policy,
            origin: None,
            stats: PaceStats::default(),
        }
    }

    pub fn policy(&self) -> PacePolicy {
        self.policy
    }

    pub fn stats(&self) -> PaceStats {
        self.stats
    }

    /// Forget the origin (after a pause, a reset or a load). The next call
    /// to [`decide`](Self::decide) rebases without counting a lag.
    pub fn reset(&mut self) {
        self.origin = None;
    }

    /// Decide given the guest clock (`guest_ticks`, 100 ns) and wall time
    /// since any fixed point (`wall`).
    pub fn decide(&mut self, guest_ticks: u64, wall: Duration) -> PaceAction {
        let PacePolicy::RealTime { max_lead, max_lag } = self.policy else {
            return PaceAction::Run;
        };
        let Some((g0, w0)) = self.origin else {
            self.origin = Some((guest_ticks, wall));
            return PaceAction::Run;
        };
        if guest_ticks < g0 || wall < w0 {
            // A clock went backwards (guest reset, new machine): start over.
            self.origin = Some((guest_ticks, wall));
            return PaceAction::Run;
        }
        let guest = ticks_to_duration(guest_ticks - g0);
        let real = wall - w0;
        if guest > real + max_lead {
            let d = guest - real;
            self.stats.sleeps += 1;
            self.stats.slept += d;
            return PaceAction::Sleep(d);
        }
        if real > guest + max_lag {
            let lag = real - guest;
            self.origin = Some((guest_ticks, wall));
            self.stats.rebases += 1;
            self.stats.worst_lag = self.stats.worst_lag.max(lag);
            return PaceAction::Rebased { lag };
        }
        PaceAction::Run
    }
}

/// Smoothed rate of some event (frames, presents), for the title bar.
#[derive(Clone, Debug, Default)]
pub struct RateMeter {
    window_start: Option<Duration>,
    count: u64,
    rate: f64,
}

impl RateMeter {
    /// Records one event at `now`; returns the rate when a window of at
    /// least `every` closes.
    pub fn tick(&mut self, now: Duration, every: Duration) -> Option<f64> {
        self.count += 1;
        self.sample(now, every)
    }

    /// Closes the window at `now` if it is `every` long, without counting.
    pub fn sample(&mut self, now: Duration, every: Duration) -> Option<f64> {
        let start = *self.window_start.get_or_insert(now);
        let span = now.saturating_sub(start);
        if span >= every && !span.is_zero() {
            self.rate = self.count as f64 / span.as_secs_f64();
            self.count = 0;
            self.window_start = Some(now);
            return Some(self.rate);
        }
        None
    }

    /// The last closed window's rate.
    pub fn rate(&self) -> f64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 10_000; // ticks per millisecond

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn tick_conversions() {
        assert_eq!(ticks_to_duration(TICKS_PER_SECOND), Duration::from_secs(1));
        assert_eq!(duration_to_ticks(ms(16)), 16 * MS);
        assert_eq!(ticks_to_duration(u64::MAX), Duration::from_nanos(u64::MAX));
    }

    #[test]
    fn vblank_ntsc() {
        let v = VBlank::NTSC;
        assert_eq!(v.count_at(0), 0);
        // One NTSC field is 166833.33 ticks.
        assert_eq!(v.count_at(166_833), 0);
        assert_eq!(v.count_at(166_834), 1);
        assert_eq!(v.time_of(1), 166_834);
        assert_eq!(v.next_after(0), 166_834);
        assert_eq!(v.next_after(166_834), v.time_of(2));
        // 60000/1001 fields per second: 59 complete fields after one second.
        assert_eq!(v.count_at(TICKS_PER_SECOND), 59);
        assert_eq!(v.count_at(1001 * TICKS_PER_SECOND), 60_000);
        assert_eq!(v.period(), Duration::from_nanos(16_683_333));
    }

    #[test]
    fn vblank_round_trip() {
        for v in [VBlank::NTSC, VBlank::HZ60, VBlank::PAL50] {
            for n in [0, 1, 2, 59, 60, 61, 12_345, 1_000_000] {
                assert_eq!(v.count_at(v.time_of(n)), n, "{v:?} n={n}");
                assert_eq!(
                    v.count_at(v.time_of(n).saturating_sub(1)),
                    n.saturating_sub(1),
                    "{v:?} n={n}"
                );
            }
        }
    }

    #[test]
    fn unpaced_never_sleeps() {
        let mut p = Pacer::new(PacePolicy::Unpaced);
        assert_eq!(p.decide(0, ms(0)), PaceAction::Run);
        assert_eq!(p.decide(1000 * MS, ms(0)), PaceAction::Run);
    }

    #[test]
    fn guest_ahead_sleeps_the_lead() {
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        assert_eq!(p.decide(5 * MS, ms(100)), PaceAction::Run); // origin
        // Guest jumped one 60 Hz frame (16.67 ms) while 1 ms of wall passed.
        assert_eq!(
            p.decide(5 * MS + 166_667, ms(101)),
            PaceAction::Sleep(Duration::from_nanos(15_666_700))
        );
        // After sleeping, wall caught up: run.
        assert_eq!(p.decide(5 * MS + 166_667, ms(117)), PaceAction::Run);
        assert_eq!(p.stats().sleeps, 1);
    }

    #[test]
    fn small_leads_are_allowed() {
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        p.decide(0, ms(0));
        assert_eq!(p.decide(2 * MS, ms(0)), PaceAction::Run);
        assert!(matches!(p.decide(3 * MS, ms(0)), PaceAction::Sleep(_)));
    }

    #[test]
    fn falling_behind_rebases_instead_of_bursting() {
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        p.decide(0, ms(0));
        // Slow host: 300 ms of wall for 16 ms of guest.
        assert_eq!(
            p.decide(16 * MS, ms(300)),
            PaceAction::Rebased { lag: ms(284) }
        );
        // Guest continues at real speed from the new origin: one frame ahead
        // of wall means a sleep, not a free run to "catch up" 284 ms.
        assert_eq!(p.decide(33 * MS, ms(301)), PaceAction::Sleep(ms(16)));
        assert_eq!(p.stats().rebases, 1);
        assert_eq!(p.stats().worst_lag, ms(284));
    }

    #[test]
    fn moderate_lag_is_absorbed() {
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        p.decide(0, ms(0));
        assert_eq!(p.decide(0, ms(50)), PaceAction::Run);
        // The guest then catches up within the allowance without a sleep.
        assert_eq!(p.decide(51 * MS, ms(50)), PaceAction::Run);
    }

    #[test]
    fn reset_and_backwards_clocks_rebase_silently() {
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        p.decide(100 * MS, ms(0));
        p.reset();
        assert_eq!(p.decide(100 * MS, ms(10_000)), PaceAction::Run);
        assert_eq!(p.decide(0, ms(10_001)), PaceAction::Run);
        assert_eq!(p.stats().rebases, 0);
    }

    #[test]
    fn simulated_60hz_loop_tracks_wall_time() {
        // A guest that renders instantly and waits for the next NTSC field,
        // under a host that sleeps exactly what it is told.
        let mut p = Pacer::new(PacePolicy::REAL_TIME);
        let v = VBlank::NTSC;
        let mut guest = 0u64;
        let mut wall = Duration::ZERO;
        let mut origin = None;
        for _ in 0..600 {
            guest = v.next_after(guest);
            wall += Duration::from_micros(300); // host work per frame
            origin.get_or_insert((guest, wall));
            if let PaceAction::Sleep(d) = p.decide(guest, wall) {
                wall += d;
            }
        }
        // 599 fields of 16.683 ms after the first; wall stays within the lead allowance.
        let (g0, w0) = origin.unwrap();
        let target = ticks_to_duration(guest - g0) + w0;
        assert!(target > Duration::from_secs(9), "{target:?}");
        assert!(
            wall + ms(2) >= target && wall <= target + ms(1),
            "wall {wall:?} target {target:?}"
        );
        assert_eq!(p.stats().rebases, 0);
    }

    #[test]
    fn rate_meter() {
        let mut m = RateMeter::default();
        assert_eq!(m.tick(ms(0), ms(1000)), None);
        for i in 1..60 {
            assert_eq!(m.tick(ms(i * 1000 / 60), ms(1000)), None);
        }
        let r = m.tick(ms(1000), ms(1000)).unwrap();
        assert!((r - 61.0).abs() < 1e-9, "{r}");
        assert_eq!(m.sample(ms(1500), ms(1000)), None);
        assert_eq!(m.sample(ms(2000), ms(1000)), Some(0.0));
    }
}
