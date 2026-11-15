//! Periodic host interrupts on logical time: a display's vertical blank.
//!
//! A source fires every `1 / rate` seconds of the logical clock. Each
//! period that comes due is delivered like a DPC: the scheduler's wake
//! pass queues the source behind any queued KDPCs, and `Machine::run`
//! drains it before the next thread slice, on the kernel DPC stack at
//! DISPATCH_LEVEL. There the source's Rust handler builds one guest call
//! (writing any record it needs into a scratch area on that stack), and
//! the call runs to its return before anything else. On the Xbox the GPU
//! interrupt's DPC calls the D3D vertical-blank callback in the same way.
//!
//! Deadlines are exact multiples of the period from the time the source
//! was added, so the count of periods always matches elapsed logical time.
//! Deadlines take part in the idle advance and in NtYieldExecution's poll
//! like timers do, except that a source alone never keeps a machine with
//! no ready thread alive (see [`crate::threads::Machine::run`]).
//!
//! Catch-up is bounded. When the clock passes several deadlines between
//! two wake passes (one long slice at DISPATCH_LEVEL, say), up to
//! `max_catch_up` of them are delivered one after the other; the rest are
//! dropped and reported to the next delivery as `missed`, so a stall never
//! turns into a burst of callbacks.
use crate::clock::Clock;
use runtime::{Guest, GuestFault, GuestInterrupt};

/// Scratch bytes a handler may write on the DPC stack for its call.
pub const SCRATCH_SIZE: u32 = 64;

/// Builds one delivery: `(guest, scratch, missed)`. `scratch` is the guest
/// address of [`SCRATCH_SIZE`] bytes that stay valid until the call returns;
/// `missed` counts the periods dropped since the previous delivery. `None`
/// delivers nothing for this period. Of the returned interrupt only
/// `target`, `args`, `convention` and `name` are used: the call always runs
/// on the DPC context, and the source does its own coalescing.
///
/// The handler runs with guest memory locked and kernel state unlocked: it
/// must not call back into the kernel.
pub type Handler =
    Box<dyn FnMut(&mut Guest, u32, u64) -> Result<Option<GuestInterrupt>, GuestFault> + Send>;

/// A rate in hertz, `num / den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub num: u64,
    pub den: u64,
}

impl Rate {
    /// 60 Hz: the refresh Halo assumes when it turns vblanks into frame time
    /// (`0x12b0e0` divides by the word at `0x485AC0`, 60 when unset).
    pub const HZ_60: Rate = Rate { num: 60, den: 1 };
    /// NTSC's 59.94 Hz (60000/1001).
    pub const NTSC: Rate = Rate {
        num: 60_000,
        den: 1001,
    };
    /// PAL's 50 Hz.
    pub const PAL: Rate = Rate { num: 50, den: 1 };

    fn valid(&self) -> bool {
        self.num != 0
            && self.den != 0
            && self.num as u128 <= Clock::FREQUENCY as u128 * self.den as u128
    }
    /// Clock ticks from the origin to the end of period `n`, rounded up.
    fn ticks(&self, n: u64) -> u64 {
        let t =
            (n as u128 * Clock::FREQUENCY as u128 * self.den as u128).div_ceil(self.num as u128);
        u64::try_from(t).unwrap_or(u64::MAX)
    }
    /// Whole periods in `elapsed` ticks.
    fn periods(&self, elapsed: u64) -> u64 {
        let n = elapsed as u128 * self.num as u128 / (Clock::FREQUENCY as u128 * self.den as u128);
        u64::try_from(n).unwrap_or(u64::MAX)
    }
}

/// What to add with [`crate::Kernel::add_periodic`].
pub struct Source {
    /// Diagnostic name.
    pub name: &'static str,
    pub rate: Rate,
    /// Most periods delivered for one wake pass (at least 1).
    pub max_catch_up: u32,
    pub handler: Handler,
}

/// Counters for one source ([`crate::Kernel::periodic_stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Periods whose handler ran.
    pub delivered: u64,
    /// Periods dropped by the catch-up bound.
    pub dropped: u64,
    /// Clock ticks of the next deadline not yet due.
    pub next_deadline: u64,
}

pub(crate) struct Periodic {
    pub name: &'static str,
    rate: Rate,
    origin: u64,
    /// Index of the next period not yet due (the first is 1).
    next: u64,
    /// Due periods waiting for delivery.
    pub owed: u64,
    /// Dropped periods not yet reported to a delivery.
    pub missed: u64,
    /// Queued on the DPC queue.
    pub queued: bool,
    max_catch_up: u64,
    /// None while the handler runs.
    pub handler: Option<Handler>,
    pub stats: Stats,
}

impl Periodic {
    pub(crate) fn new(source: Source, now: u64) -> Option<Self> {
        if !source.rate.valid() {
            return None;
        }
        let mut p = Self {
            name: source.name,
            rate: source.rate,
            origin: now,
            next: 1,
            owed: 0,
            missed: 0,
            queued: false,
            max_catch_up: source.max_catch_up.max(1) as u64,
            handler: Some(source.handler),
            stats: Stats::default(),
        };
        p.stats.next_deadline = p.deadline();
        Some(p)
    }
    /// Clock ticks of the next deadline not yet due.
    pub(crate) fn deadline(&self) -> u64 {
        self.origin.saturating_add(self.rate.ticks(self.next))
    }
    /// Account for every period due by `now`. Returns true if any came due.
    pub(crate) fn fire(&mut self, now: u64) -> bool {
        if self.deadline() > now {
            return false;
        }
        let due = self.rate.periods(now - self.origin);
        let new = due + 1 - self.next;
        self.next = due + 1;
        self.owed += new;
        if self.owed > self.max_catch_up {
            let dropped = self.owed - self.max_catch_up;
            self.missed += dropped;
            self.stats.dropped += dropped;
            self.owed = self.max_catch_up;
        }
        self.stats.next_deadline = self.deadline();
        true
    }
}
