//! `H2_ENTRY_STATS=1` (runtime environment, off by default): count the
//! runner's invocations of generated handlers by why the runner made them
//! and generated code did not (a direct call or an inline return), and
//! print the counts to stderr every 2^24 of them, with the transfers that
//! made the most. Off, the runner reads the cached flag once per run.
//!
//! What led to the invocation:
//! - `start`: the run's first transfer (a run started or resumed).
//! - `interrupt`: an interrupt's call, delivered at the safe point.
//! - `after-rust`: a Rust handler (a kernel export, an override, a
//!   continuation) returned the transfer.
//! - otherwise a generated handler returned it, by kind and route:
//!   `call`/`jump` (entry route), `jump-resume` (a loop header's
//!   preemption poll, a resume), `return`. A call or jump is split by
//!   what the target's direct slot held when the runner got the transfer:
//!   `hit` (this target, this handler, the live generation: a direct call
//!   would have found it, so the site is indirect or not a direct site, or
//!   it was not quiet, too deep, or the target's page not executable from
//!   C), `collision` (another entry's), `empty`, or `stale` (this entry at
//!   another generation or handler).
use crate::{TransferAbi, abi};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Whether `H2_ENTRY_STATS=1` was set when the first runner looked.
#[inline]
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("H2_ENTRY_STATS").as_deref() == Ok("1"))
}

/// What returned the transfer the runner is about to dispatch.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Came {
    Start,
    Interrupt,
    Rust,
    Native,
}

/// What a target's direct slot held (`direct_slot_state`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    Hit,
    Collision,
    Empty,
    Stale,
}

const CAUSES: [&str; 16] = [
    "start",
    "interrupt",
    "after-rust",
    "call-hit",
    "call-collision",
    "call-empty",
    "call-stale",
    "jump-hit",
    "jump-collision",
    "jump-empty",
    "jump-stale",
    "jump-resume",
    "return",
    "other",
    "rust-handler",
    "continuation",
];

/// The cause's index in [`CAUSES`].
pub(crate) fn cause(came: Came, t: &TransferAbi, slot: Option<Slot>) -> usize {
    let split = |base: usize| {
        base + match slot {
            Some(Slot::Hit) => 0,
            Some(Slot::Collision) => 1,
            Some(Slot::Empty) | None => 2,
            Some(Slot::Stale) => 3,
        }
    };
    match came {
        Came::Start => 0,
        Came::Interrupt => 1,
        Came::Rust => 2,
        Came::Native => match (t.kind, t.route) {
            (abi::CALL, abi::ENTRY) => split(3),
            (abi::JUMP, abi::ENTRY) => split(7),
            (abi::JUMP, abi::RESUME) => 11,
            (abi::RETURN, _) => 12,
            _ => 13,
        },
    }
}
pub(crate) const RUST_HANDLER: usize = 14;
pub(crate) const CONTINUATION: usize = 15;

static COUNTS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Per cause, target and resume PC: the transfers behind the counts.
static SITES: Mutex<Option<HashMap<(usize, u32, u32), u64>>> = Mutex::new(None);

/// One invocation of cause `c` for transfer `t` (generated handlers only
/// print; Rust ones are counted alongside).
pub(crate) fn note(c: usize, t: &TransferAbi) {
    COUNTS[c].fetch_add(1, Ordering::Relaxed);
    if c >= RUST_HANDLER {
        return;
    }
    let mut sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    *sites.get_or_insert_with(HashMap::new).entry((c, t.target_pc, t.resume_pc)).or_default() += 1;
    let total: u64 = COUNTS[..RUST_HANDLER].iter().map(|n| n.load(Ordering::Relaxed)).sum();
    if total & ((1 << 24) - 1) != 0 {
        return;
    }
    let mut line = format!("h2 runner entries: {total}");
    for (name, n) in CAUSES.iter().zip(&COUNTS) {
        line += &format!(" {name} {}", n.load(Ordering::Relaxed));
    }
    eprintln!("{line}");
    let mut top: Vec<_> = sites.as_ref().map_or(Vec::new(), |s| s.iter().map(|(k, n)| (*n, *k)).collect());
    top.sort_unstable_by(|a, b| b.cmp(a));
    for (n, (c, target, resume)) in top.into_iter().take(40) {
        eprintln!("h2 runner entries:   {n} {} target {target:#010x} resume {resume:#010x}", CAUSES[c]);
    }
}


/// Match-wide census. These count native handlers handing control back to
/// the runner, plus the runner's HLE dispatches and actual preemptions.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum DispatchCount {
    Call,
    Jump,
    Resume,
    Return,
    Stop,
    Fault,
    Other,
    Hle,
    SlicePreempt,
    RequestPreempt,
}
use DispatchCount::*;

const DISPATCH_COUNTS: [(DispatchCount, &str); 10] = [
    (Call, "call"),
    (Jump, "jump"),
    (Resume, "resume"),
    (Return, "return"),
    (Stop, "stop"),
    (Fault, "fault"),
    (Other, "other"),
    (Hle, "hle"),
    (SlicePreempt, "slice_preempt"),
    (RequestPreempt, "request_preempt"),
];

impl DispatchCount {
    pub(crate) fn native_exit(t: TransferAbi) -> Self {
        match (t.kind, t.route) {
            (abi::CALL, abi::ENTRY) => Call,
            (abi::JUMP, abi::ENTRY) => Jump,
            (abi::JUMP, abi::RESUME) => Resume,
            (abi::RETURN, _) => Return,
            (abi::STOP, abi::ENTRY) => Stop,
            (abi::FAULT, abi::ENTRY) => Fault,
            _ => Other,
        }
    }
}

#[derive(Default)]
pub(crate) struct DispatchCensus([AtomicU64; DISPATCH_COUNTS.len()]);

impl DispatchCensus {
    pub(crate) fn note(&self, count: DispatchCount) {
        self.0[count as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> DispatchCounts {
        DispatchCounts(std::array::from_fn(|i| self.0[i].load(Ordering::Relaxed)))
    }
}

/// A snapshot of the monotonic counters of one runtime.
#[derive(Clone, Copy, Default)]
pub struct DispatchCounts([u64; DISPATCH_COUNTS.len()]);

impl DispatchCounts {
    /// Totals between two snapshots of the same runtime.
    pub fn since(self, start: Self) -> impl Iterator<Item = (&'static str, u64)> {
        DISPATCH_COUNTS.into_iter().map(move |(kind, name)| {
            let index = kind as usize;
            (name, self.0[index].wrapping_sub(start.0[index]))
        })
    }
}
