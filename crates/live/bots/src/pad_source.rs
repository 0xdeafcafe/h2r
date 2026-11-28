//! Pad sources: where a bot's controller input comes from
//! (docs/game/bots.md, "Library surface").
//!
//! A [`PadSource`] gives one pad per frame: the brain ([`crate::Brain`]), a
//! script ([`Scripted`]) or a recording ([`Recorded`]). Phantom peers
//! ([`crate::phantom`]) sample their source once per simulation tick and
//! send its pad as the peer's action, as a remote console's controller
//! would. Controller ports belong to people.

use guest_layout::memory::GuestMemory;
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

/// What the read phase of a split sample gave ([`PadSource::observe`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampled {
    /// The pad, with nothing left to think.
    Pad(HostPadState),
    /// A think is staged: call [`PadSource::think`], then
    /// [`PadSource::conclude`].
    Think,
}

/// Where a bot's pad comes from: one pad per frame.
///
/// A sample can also be taken in three phases (`crate::think`,
/// docs/game/bots.md §5.5): [`PadSource::observe`] on the game thread,
/// in seat order, reads everything from the guest and the host;
/// [`PadSource::think`] on any thread touches only the source's own state
/// and read-only shared data; [`PadSource::conclude`] on the game thread,
/// in seat order, publishes what the think said. `pad` is the three in a
/// row and gives the same pad.
pub trait PadSource: Send {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState;

    /// The read phase. A source with nothing worth moving off the game
    /// thread gives its pad here.
    fn observe(&mut self, f: &Frame<'_>) -> Sampled {
        Sampled::Pad(self.pad(f))
    }

    /// The think staged by [`PadSource::observe`]. No guest memory and no
    /// host-wide mutable state.
    fn think(&mut self) -> HostPadState {
        HostPadState::default()
    }

    /// Publishes what the last think said to the other bots.
    fn conclude(&mut self) {}

    fn brain_view(&self) -> Option<crate::BrainView> {
        None
    }

    fn combat_stats(&self) -> Option<&crate::stats::CombatStats> {
        None
    }

    /// Asks the source for a bounce route to `to` (docs/game/bots.md,
    /// "Superbounce routes"); `false` when it takes no orders.
    fn order_bounce(&mut self, to: crate::brain::Vec3, mode: crate::bounce_route::Mode) -> bool {
        let _ = (to, mode);
        false
    }

    /// Gives the source an outside order (docs/game/rules-world-api.md,
    /// "Bots"), or frees it (`None`); `false` when it takes no orders.
    fn order(&mut self, order: Option<crate::brain::Order>) -> bool {
        let _ = order;
        false
    }

    /// Where its bounce route stands; `None` when it takes no orders.
    fn order_view(&self) -> Option<crate::mind::OrderView> {
        None
    }

    /// Sends `pad` every sample in place of its own until called with
    /// `None` (`superbounce.verify`'s trials on a bot seat); `false` for a
    /// source that cannot be held.
    fn hold_pad(&mut self, pad: Option<HostPadState>) -> bool {
        let _ = pad;
        false
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
