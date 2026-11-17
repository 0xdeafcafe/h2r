//! Player one's pad driven from outside the game: the agent API's
//! `input.pad` (docs/runtime/agent-api.md, "Input"), for a console nobody
//! is holding a pad at (docs/runtime/xbox.md, "Remote control").
//!
//! [`Remote`] is a queue of timed steps. [`KeyboardFallback`] merges the
//! current step into the player-one pad it already builds from the
//! keyboard and the first physical pad, so the guest sees one pad whoever
//! presses it. A step ends once its time is up *and* the guest has read
//! the pad as often as it asks ([`Step::reads`]): at 6 fps a 50 ms press
//! would otherwise fall between two reads and never happen.
//!
//! [`KeyboardFallback`]: crate::backend::fallback::KeyboardFallback

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::gamepad::Gamepad;

/// One pad state and how long to hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub pad: Gamepad,
    /// `None`: hold until the queue is replaced or cleared.
    pub hold: Option<Duration>,
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

    /// The process's remote pad: the one the app's input merges into
    /// player one, and the one the agent glue drives.
    pub fn global() -> &'static Remote {
        static G: OnceLock<Remote> = OnceLock::new();
        G.get_or_init(Remote::new)
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
    pub fn state_at(&self, now: Instant) -> Gamepad {
        let mut g = self.lock();
        loop {
            match g.current {
                Some(c) => {
                    let timed_out = c
                        .step
                        .hold
                        .is_some_and(|h| now.saturating_duration_since(c.since) >= h);
                    if !(timed_out && c.reads >= c.step.reads) {
                        return c.step.pad;
                    }
                    g.current = None;
                    g.done += 1;
                }
                None => match g.queue.pop_front() {
                    Some(step) => {
                        g.current = Some(Current {
                            step,
                            since: now,
                            reads: 0,
                        })
                    }
                    None => return Gamepad::default(),
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
            },
            Step {
                pad: pad(Buttons::DPAD_DOWN),
                hold: Some(ms(50)),
                reads: 2,
            },
            Step {
                pad: pad(Buttons::BACK),
                hold: None,
                reads: 1,
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
    fn append_queues_and_replace_cuts_in() {
        let r = Remote::new();
        let t = Instant::now();
        let hold = |b| Step {
            pad: pad(b),
            hold: None,
            reads: 0,
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
