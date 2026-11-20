//! A pad driven from outside the game: the agent API's `input.pad`
//! (docs/runtime/agent-api.md, "Input"), for a console nobody is holding a
//! pad at (docs/runtime/xbox.md, "Remote control") and for `h2 serve`.
//!
//! [`Remote`] is a queue of timed steps for one controller port. The
//! agent's input tap (`crates/extensions/agent`, at `xapi`'s `XInputGetState`) merges
//! the current step over what the port's read returns
//! ([`merge`](crate::backend::fallback::merge)), so the guest sees one pad
//! whoever presses it, and counts each read with [`Remote::state_read`]. A
//! step ends once its time is up *and* the guest has read the pad as often
//! as it asks ([`Step::reads`]): at 6 fps a 50 ms press would otherwise
//! fall between two reads and never happen. Time is wall-clock
//! ([`Step::hold`]), game ticks ([`Step::ticks`]), or both.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::gamepad::Gamepad;

/// One pad state and how long to hold it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Step {
    pub pad: Gamepad,
    /// Wall-clock time to hold it. With `ticks` also `None`: hold until
    /// the queue is replaced or cleared.
    pub hold: Option<Duration>,
    /// Game ticks to hold it (as [`Remote::state_at_tick`] is told them).
    pub ticks: Option<u64>,
    /// Don't start before the game tick reaches this.
    pub at_tick: Option<u64>,
    /// Guest reads (`XInputGetState`) the step must see before it ends.
    pub reads: u32,
}

#[derive(Debug, Default)]
struct Inner {
    queue: VecDeque<Step>,
    current: Option<Current>,
    /// Steps finished since the start, for `input.state`.
    done: u64,
}

#[derive(Debug, Clone, Copy)]
struct Current {
    step: Step,
    since: Instant,
    since_tick: Option<u64>,
    reads: u32,
}

/// What [`Remote::status`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The state being held now (neutral when idle).
    pub pad: Gamepad,
    /// Whether a step is being held.
    pub active: bool,
    /// Steps waiting after it.
    pub queued: usize,
    /// Steps finished so far.
    pub done: u64,
}

/// A shared handle; clones drive the same pad.
#[derive(Debug, Clone, Default)]
pub struct Remote(Arc<Mutex<Inner>>);

impl Remote {
    pub fn new() -> Remote {
        Remote::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces everything (the step being held too) with `steps`.
    pub fn replace(&self, steps: impl IntoIterator<Item = Step>) {
        let mut g = self.lock();
        g.current = None;
        g.queue = steps.into_iter().collect();
    }

    /// Queues `steps` after what is queued.
    pub fn append(&self, steps: impl IntoIterator<Item = Step>) {
        self.lock().queue.extend(steps);
    }

    /// Lets go of everything.
    pub fn clear(&self) {
        self.replace([]);
    }

    /// The pad to report at `now`: advances past finished steps first.
    /// Steps timed in ticks, or waiting for one, don't move.
    pub fn state_at(&self, now: Instant) -> Gamepad {
        self.state_at_tick(now, None)
    }

    /// The pad to report at `now` with the game at `tick`: advances past
    /// finished steps first.
    pub fn state_at_tick(&self, now: Instant, tick: Option<u64>) -> Gamepad {
        let mut g = self.lock();
        loop {
            match g.current {
                Some(c) => {
                    let s = c.step;
                    let wall = s
                        .hold
                        .is_none_or(|h| now.saturating_duration_since(c.since) >= h);
                    let ticks = s.ticks.is_none_or(|n| {
                        matches!((tick, c.since_tick), (Some(t), Some(from)) if t.saturating_sub(from) >= n)
                    });
                    let timed = s.hold.is_some() || s.ticks.is_some();
                    if !(timed && wall && ticks && c.reads >= s.reads) {
                        return s.pad;
                    }
                    g.current = None;
                    g.done += 1;
                }
                None => match g.queue.front() {
                    Some(step) if step.at_tick.is_none_or(|at| tick.is_some_and(|t| t >= at)) => {
                        let step = g.queue.pop_front().expect("front");
                        g.current = Some(Current {
                            step,
                            since: now,
                            since_tick: tick,
                            reads: 0,
                        })
                    }
                    // Waiting for its tick: nothing held meanwhile.
                    _ => return Gamepad::default(),
                },
            }
        }
    }

    /// The guest read the pad (`XInputGetState`) since the last poll.
    pub fn state_read(&self) {
        if let Some(c) = self.lock().current.as_mut() {
            c.reads = c.reads.saturating_add(1);
        }
    }

    /// Whether anything is held or queued: when not, merging is skipped.
    pub fn busy(&self) -> bool {
        let g = self.lock();
        g.current.is_some() || !g.queue.is_empty()
    }

    pub fn status(&self) -> Status {
        let g = self.lock();
        Status {
            pad: g.current.map(|c| c.step.pad).unwrap_or_default(),
            active: g.current.is_some(),
            queued: g.queue.len(),
            done: g.done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::Buttons;

    fn pad(b: Buttons) -> Gamepad {
        Gamepad {
            buttons: b,
            ..Gamepad::default()
        }
    }

    #[test]
    fn steps_hold_for_their_time_and_their_reads() {
        let r = Remote::new();
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        r.replace([
            Step {
                pad: pad(Buttons::START),
                hold: Some(ms(50)),
                reads: 1,
                ..Step::default()
            },
            Step {
                pad: pad(Buttons::DPAD_DOWN),
                hold: Some(ms(50)),
                reads: 2,
                ..Step::default()
            },
            Step {
                pad: pad(Buttons::BACK),
                hold: None,
                reads: 1,
                ..Step::default()
            },
        ]);
        assert!(r.busy());
        assert_eq!(r.state_at(t0).buttons, Buttons::START);
        // Time is up but the guest never read it: still held.
        assert_eq!(r.state_at(t0 + ms(80)).buttons, Buttons::START);
        r.state_read();
        let t1 = t0 + ms(90);
        assert_eq!(
            r.state_at(t1).buttons,
            Buttons::DPAD_DOWN,
            "the next step starts"
        );
        r.state_read();
        assert_eq!(
            r.state_at(t1 + ms(60)).buttons,
            Buttons::DPAD_DOWN,
            "one read of two"
        );
        r.state_read();
        assert_eq!(r.state_at(t1 + ms(60)).buttons, Buttons::BACK);
        // No time limit: held until replaced.
        r.state_read();
        assert_eq!(r.state_at(t1 + ms(10_000)).buttons, Buttons::BACK);
        let s = r.status();
        assert!(s.active && s.queued == 0 && s.done == 2);
        r.clear();
        assert!(!r.busy());
        assert_eq!(r.state_at(t1), Gamepad::default());
    }

    #[test]
    fn steps_can_wait_for_a_tick_and_hold_for_ticks() {
        let r = Remote::new();
        let t = Instant::now();
        r.replace([
            Step {
                pad: pad(Buttons::START),
                ticks: Some(2),
                at_tick: Some(10),
                reads: 1,
                ..Step::default()
            },
            Step {
                pad: pad(Buttons::BACK),
                ticks: Some(1),
                ..Step::default()
            },
        ]);
        assert_eq!(r.state_at_tick(t, Some(9)), Gamepad::default(), "not yet");
        assert!(r.busy());
        assert_eq!(r.state_at_tick(t, Some(10)).buttons, Buttons::START);
        r.state_read();
        assert_eq!(r.state_at_tick(t, Some(11)).buttons, Buttons::START);
        // Wall time alone never ends a step timed in ticks.
        assert_eq!(
            r.state_at(t + Duration::from_secs(60)).buttons,
            Buttons::START
        );
        assert_eq!(r.state_at_tick(t, Some(12)).buttons, Buttons::BACK);
        assert_eq!(r.state_at_tick(t, Some(13)), Gamepad::default());
        assert_eq!(r.status().done, 2);
    }

    #[test]
    fn append_queues_and_replace_cuts_in() {
        let r = Remote::new();
        let t = Instant::now();
        let hold = |b| Step {
            pad: pad(b),
            hold: None,
            reads: 0,
            ..Step::default()
        };
        r.replace([hold(Buttons::START)]);
        r.append([hold(Buttons::BACK)]);
        assert_eq!(r.state_at(t).buttons, Buttons::START);
        assert_eq!(r.status().queued, 1);
        r.replace([hold(Buttons::DPAD_UP)]);
        assert_eq!(r.state_at(t).buttons, Buttons::DPAD_UP);
        assert_eq!(r.status().queued, 0);
    }
}
