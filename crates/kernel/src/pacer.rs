//! Holding logical time to wall time, for a machine that is being watched.
//!
//! The kernel clock never reads the host clock: schedules depend only on
//! guest work and deadlines. A [`Pacer`] only makes the host wait. Before
//! each slice `Machine::run` hands it the logical time, and it sleeps until
//! that much wall time has passed since it started, so idle advances and
//! fast guest work cannot run the game ahead of real time. It never makes
//! logical time catch up: a host that falls more than `max_lag` behind
//! (a long load, a debugger stop) starts counting again from where it is,
//! rather than hurrying the clock or skipping waits to make up the
//! difference. The schedule, history and guest state are the same paced or
//! not.
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Pacer {
    /// Wall time and logical ticks at which counting started.
    origin: Option<(Instant, u64)>,
    /// How far behind wall time the host may fall before the pacer starts
    /// counting again from the present.
    pub max_lag: Duration,
    /// Logical ticks per wall second (`Clock::FREQUENCY` is real time).
    pub ticks_per_second: u64,
    reanchors: u64,
}

impl Pacer {
    /// Real time, re-anchoring after falling 100 ms behind.
    pub fn real_time() -> Self {
        Self {
            origin: None,
            max_lag: Duration::from_millis(100),
            ticks_per_second: crate::clock::Clock::FREQUENCY,
            reanchors: 0,
        }
    }
    /// How many times the host fell behind and counting started again.
    pub fn reanchors(&self) -> u64 {
        self.reanchors
    }
    /// Sleep until wall time catches up with logical time `ticks`.
    pub fn wait(&mut self, ticks: u64) {
        let now = Instant::now();
        let (wall, start) = *self.origin.get_or_insert((now, ticks));
        let Some(elapsed) = ticks.checked_sub(start) else {
            self.origin = Some((now, ticks));
            return;
        };
        let rate = self.ticks_per_second.max(1) as u128;
        let nanos = elapsed as u128 * 1_000_000_000 / rate;
        let Some(due) = u64::try_from(nanos)
            .ok()
            .and_then(|n| wall.checked_add(Duration::from_nanos(n)))
        else {
            self.origin = Some((now, ticks));
            return;
        };
        if due > now {
            std::thread::sleep(due - now);
        } else if now - due > self.max_lag {
            self.origin = Some((now, ticks));
            self.reanchors += 1;
        }
    }
}
