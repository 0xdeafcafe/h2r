//! Replayable logical time. No host wall clock or CPU-frequency queries.
use std::sync::Arc;
pub const MAX_CLOCK_ADVANCES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    ticks_100ns: u64,
    epoch_100ns: u64,
    advances: Arc<Vec<u64>>,
    dropped_advances: u64,
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
        let advances = Arc::make_mut(&mut self.advances);
        if advances.len() == MAX_CLOCK_ADVANCES {
            advances.drain(..MAX_CLOCK_ADVANCES / 2);
            self.dropped_advances += (MAX_CLOCK_ADVANCES / 2) as u64;
        }
        advances.push(ticks);
        Ok(())
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
