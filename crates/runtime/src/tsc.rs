//! The guest's time-stamp counter, which `rdtsc` in generated code reads
//! through `h2shim_v1_rdtsc`.
//!
//! The counter follows guest time, the clock an embedder installs with
//! [`Runtime::set_guest_clock`](crate::Runtime::set_guest_clock) (the
//! kernel's logical clock, in 100 ns ticks), at the Xbox CPU's frequency.
//! Logical time can stand still while a thread spins on the counter, so
//! every read also moves it on by at least [`TSC_MIN_STEP`]: a spin that
//! waits for N cycles ends after at most N / `TSC_MIN_STEP` reads. With no
//! clock installed the counter moves by the step alone. Either way it is
//! deterministic for a given order of reads and never repeats, across all
//! threads (the Xbox has one CPU, so one counter).
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

/// The Xbox CPU's clock, which its time-stamp counter counts: a 733 MHz
/// Pentium III, 133⅓ MHz front-side bus times 5.5. This is the one place
/// the frequency is defined.
pub const TSC_HZ: u64 = 733_333_333;

/// Guest-time ticks per second, as [`GuestClock`] reports them.
pub const GUEST_CLOCK_HZ: u64 = 10_000_000;

/// The least a read advances the counter, in cycles: about what an `rdtsc`
/// and a compare cost on a Pentium III.
pub const TSC_MIN_STEP: u64 = 64;

/// Guest time in [`GUEST_CLOCK_HZ`] ticks, or `None` if it can't be read
/// right now (the reading falls back to the step). Called with no runtime
/// lock held, from whichever thread executes `rdtsc`.
pub type GuestClock = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

#[derive(Default)]
pub struct Tsc {
    clock: RwLock<Option<GuestClock>>,
    last: AtomicU64,
}

impl Tsc {
    pub fn set_clock(&self, clock: Option<GuestClock>) {
        *self.clock.write().unwrap_or_else(|e| e.into_inner()) = clock;
    }

    /// Guest time from the installed clock, without moving the counter.
    pub fn guest_time(&self) -> Option<u64> {
        let clock = self.clock.read().unwrap_or_else(|e| e.into_inner()).clone();
        clock.and_then(|c| c())
    }

    /// The next counter value: guest time in cycles, but always at least
    /// [`TSC_MIN_STEP`] past the previous value.
    pub fn read(&self) -> u64 {
        let clock = self.clock.read().unwrap_or_else(|e| e.into_inner()).clone();
        let now = clock.and_then(|c| c()).map_or(0, cycles);
        let mut out = 0;
        // try_update retries on contention, so concurrent reads still get
        // distinct, increasing values.
        let _ = self
            .last
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
                out = now.max(last.wrapping_add(TSC_MIN_STEP));
                Some(out)
            });
        out
    }
}

/// Guest-clock ticks to TSC cycles, exactly (rounded down).
pub fn cycles(ticks: u64) -> u64 {
    (u128::from(ticks) * u128::from(TSC_HZ) / u128::from(GUEST_CLOCK_HZ)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_of_guest_time_is_the_cpu_clock() {
        assert_eq!(cycles(GUEST_CLOCK_HZ), TSC_HZ);
        assert_eq!(cycles(1), 73);
    }

    #[test]
    fn reads_follow_the_clock_and_never_repeat() {
        let tsc = Tsc::default();
        assert_eq!(tsc.read(), TSC_MIN_STEP, "no clock: the step alone");
        assert_eq!(tsc.read(), 2 * TSC_MIN_STEP);
        let ticks = Arc::new(AtomicU64::new(0));
        let t = ticks.clone();
        tsc.set_clock(Some(Arc::new(move || Some(t.load(Ordering::SeqCst)))));
        // The clock is behind the counter: it still moves by the step.
        assert_eq!(tsc.read(), 3 * TSC_MIN_STEP);
        ticks.store(GUEST_CLOCK_HZ, Ordering::SeqCst);
        assert_eq!(tsc.read(), TSC_HZ);
        assert_eq!(tsc.read(), TSC_HZ + TSC_MIN_STEP, "time standing still");
        tsc.set_clock(Some(Arc::new(|| None)));
        assert_eq!(tsc.read(), TSC_HZ + 2 * TSC_MIN_STEP, "an unreadable clock");
    }

    #[test]
    fn concurrent_reads_are_distinct_and_increasing() {
        // With no clock every read is one step on from the last, so the
        // values from all threads together are exactly the first n steps.
        let tsc = Arc::new(Tsc::default());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let tsc = tsc.clone();
                std::thread::spawn(move || {
                    let reads: Vec<u64> = (0..10_000).map(|_| tsc.read()).collect();
                    assert!(
                        reads.windows(2).all(|w| w[1] > w[0]),
                        "a thread saw time go back"
                    );
                    reads
                })
            })
            .collect();
        let mut all: Vec<u64> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        all.sort_unstable();
        let steps: Vec<u64> = (1..=all.len() as u64).map(|i| i * TSC_MIN_STEP).collect();
        assert_eq!(all, steps);
    }
}
