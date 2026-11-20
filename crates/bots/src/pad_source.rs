//! Pad sources: where a bot's controller input comes from
//! (docs/game/bots.md, "Library surface").
//!
//! A [`PadSource`] gives one pad per frame: the brain ([`crate::Brain`]), a
//! script ([`Scripted`]) or a recording ([`Recorded`]). Phantom peers
//! ([`crate::phantom`]) sample their source once per simulation tick and
//! send its pad as the peer's action, as a remote console's controller
//! would. Controller ports belong to people.

use engine::memory::GuestMemory;
use input::host::HostPadState;

/// What a source sees each frame.
pub struct Frame<'a> {
    /// Input sample number, advancing once per simulation sample, including catch-up ticks.
    pub frame: u64,
    /// The game's tick, which restarts with each game.
    pub tick: u32,
    /// The remote machine ID that identifies the phantom's player.
    pub machine: [u8; 6],
    /// The game's memory, to read what the bot sees.
    pub guest: &'a dyn GuestMemory,
}

/// Where a bot's pad comes from: one pad per frame.
pub trait PadSource: Send {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState;

    fn brain_view(&self) -> Option<crate::BrainView> {
        None
    }

    /// The phantom's action stream was interrupted by a new unit or match.
    fn reset_input(&mut self) {}

    /// Phantom uplink timing.
    fn phantom_latency(&self) -> crate::tuning::PhantomLatency {
        crate::tuning::PhantomLatency::default()
    }
}

/// A source from a closure.
impl<F: FnMut(&Frame<'_>) -> HostPadState + Send> PadSource for F {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        self(f)
    }
}

/// Timed pads: each step held for its frames, then the next; idle after
/// the last (or from the start again with `looping`).
pub struct Scripted {
    pub steps: Vec<(u64, HostPadState)>,
    pub looping: bool,
    start: Option<u64>,
}

impl Scripted {
    pub fn new(steps: Vec<(u64, HostPadState)>, looping: bool) -> Self {
        Scripted {
            steps,
            looping,
            start: None,
        }
    }
}

impl PadSource for Scripted {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let start = *self.start.get_or_insert(f.frame);
        let total: u64 = self.steps.iter().map(|s| s.0).sum();
        let mut t = f.frame - start;
        if self.looping && total > 0 {
            t %= total;
        }
        for &(n, pad) in &self.steps {
            if t < n {
                return pad;
            }
            t -= n;
        }
        HostPadState::default()
    }
}

/// A recording: one pad per frame, played once from the first frame the
/// source is asked; idle after the end.
pub struct Recorded {
    pub pads: Vec<HostPadState>,
    start: Option<u64>,
}

impl Recorded {
    pub fn new(pads: Vec<HostPadState>) -> Self {
        Recorded { pads, start: None }
    }
}

impl PadSource for Recorded {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let start = *self.start.get_or_insert(f.frame);
        let i = (f.frame - start) as usize;
        self.pads.get(i).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
