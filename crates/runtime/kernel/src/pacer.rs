//! Holding logical time to wall time, for a machine that is being watched.
//!
//! The kernel clock never reads the host clock: schedules depend only on
//! guest work and deadlines. A [`Pacer`] only makes the host wait. After
//! any idle advance and before the DPC or thread slice that runs at that
//! time, `Machine::run` hands it the logical time, and it sleeps until
//! that much wall time has passed since it started, so idle advances and
//! fast guest work cannot run the game ahead of real time. It never makes
//! logical time catch up: a host that falls more than `max_lag` behind
//! (a long load, a debugger stop) starts counting again from where it is,
//! rather than hurrying the clock or skipping waits to make up the
//! difference. The schedule, history and guest state are the same paced or
//! not.
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_os = "linux")]
mod wall {
    use rustix::time::{ClockId, Timespec, clock_gettime};
    use std::time::Duration;

    // Absolute time since the monotonic clock's origin. Keep the origin
    // in the same clock domain as TIMER_ABSTIME.
    pub(super) type Instant = Duration;

    pub(super) fn now() -> Instant {
        clock_gettime(ClockId::Monotonic)
            .try_into()
            .expect("nonnegative monotonic clock")
    }

    pub(super) fn clock_offset(now: Instant) -> Duration {
        now
    }

    pub(super) fn sleep_until(due: Instant, _sample: Instant) {
        let deadline = Timespec::try_from(due).expect("pacer deadline fits time_t");
        loop {
            match rustix::thread::clock_nanosleep_absolute(ClockId::Monotonic, &deadline) {
                Ok(()) => return,
                Err(rustix::io::Errno::INTR) => continue,
                Err(_) => {
                    std::thread::sleep(due.saturating_sub(now()));
                    return;
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod wall {
    use std::time::Duration;
    pub(super) use std::time::Instant;

    pub(super) fn now() -> Instant {
        Instant::now()
    }

    // Instant exposes no shared epoch. Sample the common clock once at
    // host-grid setup so that each pack slot gets its assigned phase.
    #[cfg(unix)]
    pub(super) fn clock_offset(_now: Instant) -> Duration {
        rustix::time::clock_gettime(rustix::time::ClockId::Monotonic)
            .try_into()
            .expect("nonnegative monotonic clock")
    }

    #[cfg(not(unix))]
    pub(super) fn clock_offset(_now: Instant) -> Duration {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    }

    pub(super) fn sleep_until(due: Instant, sample: Instant) {
        std::thread::sleep(due - sample);
    }
}

/// Shared across processes on one pack. Rounding is wall time only: all
/// intervening audio, vblank and scheduler events still run in guest order.
const HOST_TICK_NS: u64 = 1_000_000_000 / 30;

#[derive(Clone)]
struct HostTicks {
    first: wall::Instant,
    packet_tick: Option<wall::Instant>,
    wait_for_packet: Arc<dyn Fn(Duration) -> bool + Send + Sync>,
}

impl std::fmt::Debug for HostTicks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostTicks")
            .field("first", &self.first)
            .finish_non_exhaustive()
    }
}

impl HostTicks {
    fn until_phase(elapsed: Duration, phase: u64) -> Duration {
        let position = (elapsed.as_nanos() % u128::from(HOST_TICK_NS)) as u64;
        Duration::from_nanos((phase + HOST_TICK_NS - position) % HOST_TICK_NS)
    }

    fn sleep_deadline(&mut self, due: wall::Instant, now: wall::Instant) -> wall::Instant {
        let tick = if due <= self.first {
            self.first
        } else {
            due + Self::until_phase(due - self.first, 0)
        };
        // Keep the ordinary pace while inbound data is pending. Even an
        // always-readable socket cannot run the logical clock ahead.
        if self.packet_tick == Some(tick) {
            return due;
        }
        if tick > due && (self.wait_for_packet)(tick - now) {
            // Keep ordinary waits through this tick instead of polling the
            // same unread socket at every intermediate logical deadline.
            self.packet_tick = Some(tick);
            return due;
        }
        tick
    }
}

#[derive(Debug, Clone)]
pub struct Pacer {
    /// Wall time and logical ticks at which counting started.
    origin: Option<(wall::Instant, u64)>,
    /// How far behind wall time the host may fall before the pacer starts
    /// counting again from the present.
    pub max_lag: Duration,
    /// Logical ticks per wall second (`Clock::FREQUENCY` is real time).
    pub ticks_per_second: u64,
    reanchors: u64,
    host_ticks: Option<HostTicks>,
}

impl Pacer {
    /// Real time, re-anchoring after falling 100 ms behind.
    pub fn real_time() -> Self {
        Self {
            origin: None,
            max_lag: Duration::from_millis(100),
            ticks_per_second: crate::clock::Clock::FREQUENCY,
            reanchors: 0,
            host_ticks: None,
        }
    }
    /// Align real-time sleeps to a slot's 30 Hz wall-clock phase. The
    /// optional network wait in the caller reports readiness without
    /// consuming the packet. It may return false immediately with no sockets.
    pub fn coalesce_host_ticks(
        &mut self,
        phase_ns: u64,
        wait_for_packet: impl Fn(Duration) -> bool + Send + Sync + 'static,
    ) {
        let now = wall::now();
        self.host_ticks = Some(HostTicks {
            first: now + HostTicks::until_phase(wall::clock_offset(now), phase_ns % HOST_TICK_NS),
            packet_tick: None,
            wait_for_packet: Arc::new(wait_for_packet),
        });
    }

    /// How many times the host fell behind and counting started again.
    pub fn reanchors(&self) -> u64 {
        self.reanchors
    }
    /// Sleep until wall time catches up with logical time `ticks`.
    pub fn wait(&mut self, ticks: u64) {
        let now = wall::now();
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
            if let Some(grid) = &mut self.host_ticks {
                let wake = grid.sleep_deadline(due, now);
                let after_poll = wall::now();
                if wake > after_poll {
                    wall::sleep_until(wake, after_poll);
                }
            } else {
                wall::sleep_until(due, now);
            }
        } else if now - due > self.max_lag {
            self.origin = Some((now, ticks));
            self.reanchors += 1;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn paced_ticks_wait_until_the_monotonic_deadline() {
        let mut pacer = Pacer::real_time();
        pacer.ticks_per_second = 1_000;
        pacer.wait(500);
        let origin = pacer.origin.unwrap().0;
        pacer.wait(503);
        assert!(wall::now() >= origin + Duration::from_millis(3));
        pacer.wait(0);
        assert_eq!(pacer.origin.unwrap().1, 0);
    }
}

#[cfg(test)]
mod host_tick_tests {
    use super::*;

    #[test]
    fn intermediate_deadlines_share_one_slot_wake() {
        let now = wall::now();
        let period = Duration::from_nanos(HOST_TICK_NS);
        let mut grid = HostTicks {
            first: now,
            packet_tick: None,
            wait_for_packet: Arc::new(move |wait| {
                assert_eq!(wait, period);
                false
            }),
        };
        for ms in [1, 5, 17, 31] {
            assert_eq!(
                grid.sleep_deadline(now + Duration::from_millis(ms), now),
                now + period
            );
        }
        assert_eq!(grid.sleep_deadline(now + period, now), now + period);
    }

    #[test]
    fn packet_readiness_keeps_the_original_logical_deadline() {
        let now = wall::now();
        let due = now + Duration::from_millis(2);
        let mut grid = HostTicks {
            first: now,
            packet_tick: None,
            wait_for_packet: Arc::new(|_| true),
        };
        assert_eq!(grid.sleep_deadline(due, now), due);
        grid.wait_for_packet = Arc::new(|_| panic!("re-polled a ready socket in the same tick"));
        let next = due + Duration::from_millis(1);
        assert_eq!(grid.sleep_deadline(next, due), next);
    }

    #[test]
    fn slot_phase_is_shared_even_when_matches_start_at_different_times() {
        let phase = HOST_TICK_NS / 4;
        let boundary = Duration::from_nanos(HOST_TICK_NS * 9 + phase);
        for before in [Duration::from_millis(7), Duration::from_millis(29)] {
            let start = boundary - before;
            assert_eq!(start + HostTicks::until_phase(start, phase), boundary);
        }
        assert_eq!(HostTicks::until_phase(boundary, phase), Duration::ZERO);
    }
}
