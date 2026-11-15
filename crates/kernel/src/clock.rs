//! Replayable logical time. No host wall clock or CPU-frequency queries.
use std::sync::Arc;
pub const MAX_CLOCK_ADVANCES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    ticks_100ns: u64,
    epoch_100ns: u64,
    advances: Arc<Vec<u64>>,
    dropped_advances: u64,
    /// The last history entry came from [`Clock::charge`]: the next charge
    /// moves it rather than adding one.
    charging: bool,
}
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("logical clock overflow or non-monotonic replay")]
pub struct ClockError;
impl Default for Clock {
    fn default() -> Self {
        Self::new(0)
    }
}
impl Clock {
    pub const FREQUENCY: u64 = 10_000_000;
    pub fn new(epoch_100ns: u64) -> Self {
        Self {
            ticks_100ns: 0,
            epoch_100ns,
            advances: Arc::new(Vec::new()),
            dropped_advances: 0,
            charging: false,
        }
    }
    pub fn counter(&self) -> u64 {
        self.ticks_100ns
    }
    pub fn system_time(&self) -> u64 {
        self.epoch_100ns + self.ticks_100ns
    }
    pub fn tick_count(&self) -> u32 {
        (self.ticks_100ns / 10_000) as u32
    }
    pub fn advances(&self) -> &[u64] {
        &self.advances
    }
    pub fn dropped_advances(&self) -> u64 {
        self.dropped_advances
    }
    pub fn advance_to(&mut self, ticks: u64) -> Result<(), ClockError> {
        if ticks < self.ticks_100ns || self.epoch_100ns.checked_add(ticks).is_none() {
            return Err(ClockError);
        }
        self.ticks_100ns = ticks;
        self.charging = false;
        self.record(ticks);
        Ok(())
    }
    /// Move time on by executed work (`threads::ACCESSES_PER_TICK`).
    /// Consecutive charges share one history entry, which each moves to the
    /// new time: the steps between them follow from replaying the
    /// execution, and a long run of work does not flush the history. Any
    /// other advance starts a new entry. Saturates at the epoch's limit.
    pub fn charge(&mut self, ticks: u64) {
        let target = self
            .ticks_100ns
            .saturating_add(ticks)
            .min(u64::MAX - self.epoch_100ns);
        if target == self.ticks_100ns {
            return;
        }
        self.ticks_100ns = target;
        if self.charging
            && let Some(last) = Arc::make_mut(&mut self.advances).last_mut()
        {
            *last = target;
        } else {
            self.charging = true;
            self.record(target);
        }
    }
    fn record(&mut self, ticks: u64) {
        let advances = Arc::make_mut(&mut self.advances);
        if advances.len() == MAX_CLOCK_ADVANCES {
            advances.drain(..MAX_CLOCK_ADVANCES / 2);
            self.dropped_advances += (MAX_CLOCK_ADVANCES / 2) as u64;
        }
        advances.push(ticks);
    }
    pub fn advance(&mut self, ticks: u64) -> Result<(), ClockError> {
        self.advance_to(self.ticks_100ns.checked_add(ticks).ok_or(ClockError)?)
    }
    pub fn deadline(&self, interval: i64) -> Result<u64, ClockError> {
        let deadline = if interval < 0 {
            self.ticks_100ns
                .checked_add(interval.unsigned_abs())
                .ok_or(ClockError)?
        } else {
            (interval as u64)
                .saturating_sub(self.epoch_100ns)
                .max(self.ticks_100ns)
        };
        self.epoch_100ns.checked_add(deadline).ok_or(ClockError)?;
        Ok(deadline)
    }
    pub fn replay(epoch: u64, advances: &[u64]) -> Result<Self, ClockError> {
        let mut clock = Self::new(epoch);
        for &ticks in advances {
            clock.advance_to(ticks)?;
        }
        Ok(clock)
    }
}
