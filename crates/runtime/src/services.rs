//! Shared memory and dispatch services. Locks end before handler calls.
use crate::{
    Dispatch, Guest, TransferAbi, abi,
    dispatch::Handler,
    handler::OriginalHandler,
    tsc::{GuestClock, Tsc},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, TryLockError};
use std::time::{Duration, Instant};

pub struct Runtime {
    memory: crate::ffi::Memory,
    dispatch: RwLock<Dispatch>,
    /// Bumped by every [`edit_dispatch`](Self::edit_dispatch): execution
    /// contexts drop their resolve caches when it moves.
    dispatch_gen: AtomicU64,
    pub(crate) interrupts: crate::interrupt::InterruptQueue,
    tsc: Tsc,
    pub(crate) words: crate::preempt::Words,
    pub(crate) coverage: OnceLock<crate::coverage::Coverage>,
    pub(crate) dispatch_census: Option<crate::entry_stats::DispatchCensus>,
}

/// Waits on the guest memory lock that found it held by another thread.
#[derive(Debug, Default)]
pub(crate) struct LockContention {
    waits: AtomicU64,
    nanos: AtomicU64,
}

/// What [`Runtime::memory_contention`] reports: how often taking the guest
/// memory lock had to wait for another thread, and for how long in all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contention {
    pub waits: u64,
    pub waited: Duration,
}

impl LockContention {
    #[inline]
    pub(crate) fn lock<'a>(&self, memory: &'a Mutex<Guest>) -> MutexGuard<'a, Guest> {
        match memory.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => self.contended(memory),
        }
    }
    #[cold]
    #[inline(never)]
    fn contended<'a>(&self, memory: &'a Mutex<Guest>) -> MutexGuard<'a, Guest> {
        let _t = timeline::span(timeline::Cat::Lock, timeline::Track::Host, "memory lock wait");
        let start = Instant::now();
        let g = memory.lock().unwrap_or_else(|e| e.into_inner());
        let waited = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.waits.fetch_add(1, Ordering::Relaxed);
        self.nanos.fetch_add(waited, Ordering::Relaxed);
        g
    }
    pub(crate) fn snapshot(&self) -> Contention {
        Contention {
            waits: self.waits.load(Ordering::Relaxed),
            waited: Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        }
    }
}

impl Runtime {
    pub fn new(memory: Guest, dispatch: Dispatch) -> Arc<Self> {
        Arc::new(Self {
            memory: crate::ffi::Memory::new(memory),
            dispatch: RwLock::new(dispatch),
            dispatch_gen: AtomicU64::new(1),
            interrupts: Default::default(),
            tsc: Tsc::default(),
            words: Default::default(),
            coverage: OnceLock::new(),
            dispatch_census: (std::env::var("H2_DISPATCH_STATS").as_deref() == Ok("1"))
                .then(crate::entry_stats::DispatchCensus::default),
        })
    }
    /// Runtime dispatch counters, enabled when constructed with
    /// `H2_DISPATCH_STATS=1`.
    pub fn dispatch_counts(&self) -> Option<crate::DispatchCounts> {
        self.dispatch_census.as_ref().map(|census| census.snapshot())
    }

    /// Install the generated registry before any execution contexts are built.
    pub fn enable_guest_coverage(&self, entries: &[crate::H2GenEntry], resumes: &[crate::H2GenResume]) {
        let _ = self.coverage.set(crate::coverage::Coverage::new(entries, resumes));
    }

    pub fn guest_coverage(&self) -> Option<crate::GuestCoverage> {
        self.coverage.get().map(crate::coverage::Coverage::snapshot)
    }

    /// Install (or with `None` remove) the guest-time source the
    /// time-stamp counter follows; see [`crate::tsc`].
    pub fn set_guest_clock(&self, clock: Option<GuestClock>) {
        self.tsc.set_clock(clock);
    }
    /// Guest time in [`crate::tsc::GUEST_CLOCK_HZ`] ticks from the clock
    /// [`Runtime::set_guest_clock`] installed (the kernel's logical clock),
    /// or `None` without one. Reading it changes nothing. Call it with no
    /// runtime lock held: the kernel's clock takes the kernel lock, which
    /// comes before guest memory.
    pub fn guest_time(&self) -> Option<u64> {
        self.tsc.guest_time()
    }
    /// What `rdtsc` reads next; see [`crate::tsc`].
    pub fn read_tsc(&self) -> u64 {
        self.tsc.read()
    }
    /// The time-stamp counter's last value, for a machine snapshot.
    pub fn tsc_last(&self) -> u64 {
        self.tsc.last()
    }
    /// Continue the time-stamp counter from a snapshot's
    /// [`Runtime::tsc_last`], while no guest code runs.
    pub fn restore_tsc_last(&self, last: u64) {
        self.tsc.restore_last(last);
    }
    /// Compound HLE operations use this guard, including Guest::call/ret.
    /// Release it before dispatching or invoking callbacks.
    /// Mutex contention is counted separately from the inline-memory gate.
    #[inline]
    pub fn memory(&self) -> crate::ffi::MemoryGuard<'_> {
        self.memory.lock()
    }
    /// Contended waits on the guest mutex since the runtime was made.
    pub fn memory_contention(&self) -> Contention {
        self.memory.contention()
    }
    /// The guest layout as the last memory guard left it ([`Guest`]'s
    /// layout identity): an execute check that passed under this value
    /// still passes while it stays the same.
    #[inline]
    pub(crate) fn layout_epoch(&self) -> u64 {
        self.memory.layout()
    }
    pub(crate) fn check_execute(&self, addr: u32) -> Result<(), crate::GuestFault> {
        if self.memory.allows_execute(addr) {
            Ok(())
        } else {
            self.memory().check(addr, 1, crate::guest::Access::Execute, 1)
        }
    }
    pub(crate) fn fast_memory(&self) -> *const crate::ffi::MemoryShared {
        self.memory.fast_shared()
    }
    /// Reads the dispatch tables (override names, say) without a new
    /// generation.
    pub fn read_dispatch<R>(&self, f: impl FnOnce(&Dispatch) -> R) -> R {
        f(&self.dispatch.read().unwrap_or_else(|e| e.into_inner()))
    }
    pub fn edit_dispatch<R>(&self, f: impl FnOnce(&mut Dispatch) -> R) -> R {
        let mut d = self.dispatch.write().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut d);
        // Under the write lock: a context that resolves after this sees the
        // new tables and the new generation together.
        self.dispatch_gen.fetch_add(1, Ordering::AcqRel);
        r
    }
    /// The hook for a generated handler's STOP
    /// ([`Dispatch::set_stop_recovery`]); read only when one happens.
    pub(crate) fn stop_recovery(&self) -> Option<std::sync::Arc<crate::dispatch::StopRecovery>> {
        self.dispatch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .stop_recovery()
    }
    /// The generation counter itself, for generated code's direct calls
    /// (`direct.rs`); it lives as long as the runtime.
    pub(crate) fn dispatch_generation_ptr(&self) -> *const AtomicU64 {
        &self.dispatch_gen
    }
    /// The dispatch tables' generation (see [`edit_dispatch`](Self::edit_dispatch)).
    pub fn dispatch_generation(&self) -> u64 {
        self.dispatch_gen.load(Ordering::Acquire)
    }
    /// Resolve a transfer to (handler, start selector, function), plus the
    /// dispatch generation the answer belongs to (read under the same lock,
    /// so the two agree). Entry routes start at the target; resumes start at
    /// the registered selector of their owner. A RETURN_TARGET checks the
    /// exact resume table first, so an entry override never runs in place of
    /// a caller's continuation.
    pub(crate) fn resolve_with_generation(
        &self,
        t: TransferAbi,
    ) -> (u64, Option<(Handler, u32, u32)>) {
        loop {
            let d = self.dispatch.read().unwrap_or_else(|e| e.into_inner());
            let generation = self.dispatch_gen.load(Ordering::Acquire);
            let found = Self::resolve_in(&d, t);
            if found.is_some() || !matches!(t.route, abi::ENTRY | abi::RETURN_TARGET) {
                return (generation, found);
            }
            let Some(fallback) = d.entry_fallback() else {
                return (generation, None);
            };
            drop(d);
            let pc = t.target_pc;
            let found = fallback(pc).map(|h| (h, pc, pc));
            // A slow entry fallback can race a new explicit registration.
            if self.dispatch_generation() == generation {
                return (generation, found);
            }
        }
    }
    fn resolve_in(d: &Dispatch, t: TransferAbi) -> Option<(Handler, u32, u32)> {
        let entry = |pc: u32| d.resolve(pc).map(|h| (h, pc, pc));
        match t.route {
            abi::ENTRY => entry(t.target_pc),
            abi::RESUME => d.resolve_resume(t.target_pc),
            abi::RETURN_TARGET => d.resolve_resume(t.target_pc).or_else(|| entry(t.target_pc)),
            _ => None,
        }
    }
    /// [`Dispatch::fork`] of this runtime's tables, under their read lock.
    pub(crate) fn fork_dispatch(
        &self,
        keep: impl Fn(u32, &str) -> bool,
        refuse: impl Fn(u32, &str) -> Handler,
    ) -> Dispatch {
        self.dispatch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .fork(keep, refuse)
    }
    /// Original owner and selector for a guest return or loop checkpoint.
    pub fn resolve_original_resume(&self, pc: u32) -> Option<(OriginalHandler,u32)> {
        self.dispatch.read().unwrap_or_else(|e| e.into_inner()).original_resume(pc)
            .map(|(handler,selector)| (OriginalHandler(handler),selector))
    }
    pub fn resolve_original(&self, pc: u32) -> Option<OriginalHandler> {
        self.dispatch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve_original(pc)
            .map(OriginalHandler)
    }
}
