//! The adapter's side of `render.residual` (docs/graphics/retire-d3d.md,
//! "New method: `render.residual`"): what each guest D3D entry call did,
//! by the return address it was called from.
//!
//! Unarmed, the adapter in [`crate::runtime_glue`] pays one relaxed load
//! per entry ([`armed`]). Armed, it reads the return address at `[esp]`,
//! takes the device's [`Tally`] before the entry's drain, after it and after
//! the entry, times the call, and hands the [`EntryCall`] to the sink the
//! host registered ([`set_sink`]; `render_bridge::residual_census` through
//! boot's scene glue). The sink is called with the device locked and must
//! not take the device.
//!
//! The device's tally is plain integers in its [`Stats`](crate::Stats),
//! bumped under the device lock it already holds: no atomics, no
//! allocation. The process-wide present count ([`presents`]) is one relaxed
//! add per `Swap`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// What one device did, as running totals ([`crate::Device::residual_tally`]).
/// Two of them subtract to what happened between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Guest draws the device resolved (an entry's own, or decoded from the
    /// push buffer).
    pub draws: u64,
    /// Of those, draws a native replacement committed in their place.
    pub claimed: u64,
    /// Draws handed to the backend: the growth of
    /// [`crate::device::backend_draws`] this device caused.
    pub residual: u64,
    /// Push-buffer words decoded.
    pub push_words: u64,
    /// Render-state and texture-state words the deferred-state commit
    /// (`0x400fa0`) applied from the guest's arrays.
    pub state_words: u64,
    /// Frame-buffer copies made for draws that sample one.
    pub copies: u64,
    /// Texture uploads.
    pub uploads: u64,
    pub native_guest_binds: u64,
    pub guest_bind_refused_destroy: u64,
    pub native_rect_clears: u64,
    pub clear_refused_rect_count: u64,
}

impl Tally {
    /// What happened between `before` and `self`.
    pub fn since(self, before: Tally) -> Tally {
        Tally {
            draws: self.draws.saturating_sub(before.draws),
            claimed: self.claimed.saturating_sub(before.claimed),
            residual: self.residual.saturating_sub(before.residual),
            push_words: self.push_words.saturating_sub(before.push_words),
            state_words: self.state_words.saturating_sub(before.state_words),
            copies: self.copies.saturating_sub(before.copies),
            uploads: self.uploads.saturating_sub(before.uploads),
            native_guest_binds: self.native_guest_binds.saturating_sub(before.native_guest_binds),
            guest_bind_refused_destroy: self.guest_bind_refused_destroy.saturating_sub(before.guest_bind_refused_destroy),
            native_rect_clears: self.native_rect_clears.saturating_sub(before.native_rect_clears),
            clear_refused_rect_count: self.clear_refused_rect_count.saturating_sub(before.clear_refused_rect_count),
        }
    }
}

/// One guest call of a D3D entry, armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryCall {
    /// The entry's address.
    pub addr: u32,
    /// The entry's name ([`crate::abi::entries`]).
    pub name: &'static str,
    /// The return address at `[esp]`: the caller's site.
    pub ret: u32,
    /// What the drain before the entry did: words game code wrote into the
    /// push buffer before the call, inline draws among them.
    pub inline: Tally,
    /// What the entry itself did.
    pub own: Tally,
    /// Nanoseconds in the adapter, drain and entry together.
    pub ns: u64,
    /// False when the drain stopped at a push-buffer callback: the entry
    /// runs again after the callback and is filed then.
    pub ran: bool,
    /// [`presents`] after the call.
    pub present: u64,
}

/// Where armed entry calls go.
pub type Sink = fn(&EntryCall);

static ARMED: AtomicBool = AtomicBool::new(false);
static SINK: OnceLock<Sink> = OnceLock::new();
static PRESENTS: AtomicU64 = AtomicU64::new(0);

/// Registers the sink (once per process; later calls are ignored).
pub fn set_sink(sink: Sink) {
    let _ = SINK.set(sink);
}

/// Turns the adapter's watch on or off. Without a sink it stays off.
pub fn arm(on: bool) {
    ARMED.store(on && SINK.get().is_some(), Ordering::Relaxed);
}

/// Whether entry calls are watched: one relaxed load.
#[inline(always)]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// `Swap` calls, every device together, since the process started.
pub fn presents() -> u64 {
    PRESENTS.load(Ordering::Relaxed)
}

pub(crate) fn present() {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record(call: &EntryCall) {
    if let Some(sink) = SINK.get() {
        sink(call);
    }
}

/// One entry call being watched: the tallies around its drain and its body.
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    ret: u32,
    start: Tally,
    drained: Tally,
    at: Instant,
}

impl Watch {
    /// Before the drain.
    pub fn start(ret: u32, tally: Tally) -> Watch {
        Watch {
            ret,
            start: tally,
            drained: tally,
            at: Instant::now(),
        }
    }

    /// After the drain, before the entry.
    pub fn drained(&mut self, tally: Tally) {
        self.drained = tally;
    }

    /// After the entry (or after a drain that stopped at a callback, with
    /// `ran` false).
    pub fn finish(self, addr: u32, name: &'static str, tally: Tally, ran: bool) -> EntryCall {
        let ns = u64::try_from(self.at.elapsed().as_nanos()).unwrap_or(u64::MAX);
        EntryCall {
            addr,
            name,
            ret: self.ret,
            inline: self.drained.since(self.start),
            own: if ran {
                tally.since(self.drained)
            } else {
                Tally::default()
            },
            ns,
            ran,
            present: presents(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watch_splits_the_drain_from_the_entry() {
        let t = |draws, residual, push_words| Tally {
            draws,
            residual,
            push_words,
            ..Tally::default()
        };
        let mut w = Watch::start(0x1234, t(10, 8, 100));
        w.drained(t(12, 10, 140));
        let call = w.finish(0x3f00, "D3DDevice_DrawVertices", t(13, 11, 150), true);
        assert_eq!(call.ret, 0x1234);
        assert_eq!(call.inline, t(2, 2, 40));
        assert_eq!(call.own, t(1, 1, 10));
        assert!(call.ran);
        let mut w = Watch::start(0x1234, t(0, 0, 0));
        w.drained(t(1, 1, 9));
        let call = w.finish(0x3f00, "D3DDevice_DrawVertices", t(5, 5, 50), false);
        assert_eq!((call.inline, call.own), (t(1, 1, 9), Tally::default()));
    }

    #[test]
    fn arming_without_a_sink_stays_off() {
        // No test in this binary registers a sink.
        arm(true);
        assert!(!armed());
    }
}
