//! Shared memory and dispatch services. Locks end before handler calls.
use crate::{
    Dispatch, Guest, TransferAbi, abi,
    dispatch::Handler,
    handler::OriginalHandler,
    tsc::{GuestClock, Tsc},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, TryLockError};
use std::time::{Duration, Instant};

pub struct Runtime {
    memory: crate::ffi::Memory,
    dispatch: RwLock<Dispatch>,
    pub(crate) interrupts: crate::interrupt::InterruptQueue,
    tsc: Tsc,
    pub(crate) words: crate::preempt::Words,
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
            interrupts: Default::default(),
            tsc: Tsc::default(),
            words: Default::default(),
        })
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
    pub fn edit_dispatch<R>(&self, f: impl FnOnce(&mut Dispatch) -> R) -> R {
        f(&mut self.dispatch.write().unwrap_or_else(|e| e.into_inner()))
    }
    /// Resolve a transfer to (handler, start selector, function). Entry
    /// routes start at the target; resumes start at the registered selector
    /// of their owner. A RETURN_TARGET checks the exact resume table first,
    /// so an entry override never runs in place of a caller's continuation.
    pub(crate) fn resolve(&self, t: TransferAbi) -> Option<(Handler, u32, u32)> {
        let d = self.dispatch.read().unwrap_or_else(|e| e.into_inner());
        let entry = |pc: u32| d.resolve(pc).map(|h| (h, pc, pc));
        match t.route {
            abi::ENTRY => entry(t.target_pc),
            abi::RESUME => d.resolve_resume(t.target_pc),
            abi::RETURN_TARGET => d.resolve_resume(t.target_pc).or_else(|| entry(t.target_pc)),
            _ => None,
        }
    }
    pub fn resolve_original(&self, pc: u32) -> Option<OriginalHandler> {
        self.dispatch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve_original(pc)
            .map(OriginalHandler)
    }
}
