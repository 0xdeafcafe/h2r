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
//! Catch-up is spread out, not dropped. When the clock passes several
//! deadlines between two wake passes (one long slice at DISPATCH_LEVEL, a
//! jump to a timer while threads were busy), every period stays owed: a
//! guest callback that counts calls (Halo's `0x14280` adds one to
//! `0x485AB0` per call and reads nothing else of the time) must see each
//! one, or game time falls behind the clock. Each drain delivers at most
//! `max_catch_up` of them and puts the source back on the queue, so the
//! rest come in later drains, between thread slices, rather than as one
//! burst. Only a debt older than [`MAX_DEBT_SECONDS`] is dropped, counted,
//! and reported to the next delivery as `missed`.
use crate::clock::Clock;
use runtime::{Guest, GuestFault, GuestInterrupt};

/// Owed periods are kept up to this much logical time's worth; older ones
/// are dropped (a debugger stop or `Kernel::advance_time` by hours should
/// not replay every vblank).
pub const MAX_DEBT_SECONDS: u64 = 10;

/// Scratch bytes a handler may write on the DPC stack for its call.
pub const SCRATCH_SIZE: u32 = 64;

/// Builds one delivery: `(guest, scratch, missed)`. `scratch` is the guest
/// address of [`SCRATCH_SIZE`] bytes that stay valid until the call returns;
/// `missed` counts the periods dropped since the previous delivery (only a
/// debt beyond [`MAX_DEBT_SECONDS`] is dropped). `None`
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
    /// Most owed periods delivered in one drain, before the next thread
    /// slice (at least 1). The rest wait for later drains.
    pub max_catch_up: u32,
    pub handler: Handler,
}

/// Counters for one source ([`crate::Kernel::periodic_stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Periods whose handler ran.
    pub delivered: u64,
    /// Periods dropped because the debt passed [`MAX_DEBT_SECONDS`].
    pub dropped: u64,
    /// Due periods not yet delivered.
    pub owed: u64,
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
    pub max_catch_up: u64,
    /// Most periods owed at once ([`MAX_DEBT_SECONDS`]).
    max_owed: u64,
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
            max_owed: source
                .rate
                .periods(MAX_DEBT_SECONDS * Clock::FREQUENCY)
                .max(1),
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
        self.owed = self.owed.saturating_add(new);
        if self.owed > self.max_owed {
            let dropped = self.owed - self.max_owed;
            self.missed += dropped;
            self.stats.dropped += dropped;
            self.owed = self.max_owed;
        }
        self.stats.owed = self.owed;
        self.stats.next_deadline = self.deadline();
        true
    }
}

/// A source's schedule without its handler, for a snapshot: the handler
/// is a Rust closure the boot registers, so a load puts the saved schedule
/// onto the booted source of the same ID and name.
pub(crate) struct Schedule {
    pub name: String,
    rate: Rate,
    origin: u64,
    next: u64,
    owed: u64,
    missed: u64,
    queued: bool,
    max_catch_up: u64,
    max_owed: u64,
    stats: Stats,
}
crate::snapshot::persist_struct!(Schedule {
    name,
    rate,
    origin,
    next,
    owed,
    missed,
    queued,
    max_catch_up,
    max_owed,
    stats,
});
crate::snapshot::persist_struct!(Rate { num, den });
crate::snapshot::persist_struct!(Stats {
    delivered,
    dropped,
    owed,
    next_deadline,
});

impl Periodic {
    pub(crate) fn save(&self, w: &mut snapshot::codec::Writer) {
        use crate::snapshot::Persist;
        let Self {
            name,
            rate,
            origin,
            next,
            owed,
            missed,
            queued,
            max_catch_up,
            max_owed,
            handler: _,
            stats,
        } = self;
        Schedule {
            name: (*name).to_owned(),
            rate: *rate,
            origin: *origin,
            next: *next,
            owed: *owed,
            missed: *missed,
            queued: *queued,
            max_catch_up: *max_catch_up,
            max_owed: *max_owed,
            stats: *stats,
        }
        .put(w);
    }
    pub(crate) fn restore(&mut self, s: Schedule) {
        self.rate = s.rate;
        self.origin = s.origin;
        self.next = s.next;
        self.owed = s.owed;
        self.missed = s.missed;
        self.queued = s.queued;
        self.max_catch_up = s.max_catch_up;
        self.max_owed = s.max_owed;
        self.stats = s.stats;
    }
}
