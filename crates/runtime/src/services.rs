//! Shared memory and dispatch services. Locks end before handler calls.
use crate::{
    Dispatch, Guest, TransferAbi, abi,
    dispatch::Handler,
    handler::OriginalHandler,
    tsc::{GuestClock, Tsc},
};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

pub struct Runtime {
    memory: Mutex<Guest>,
    dispatch: RwLock<Dispatch>,
    pub(crate) interrupts: crate::interrupt::InterruptQueue,
    tsc: Tsc,
    pub(crate) words: crate::preempt::Words,
}
impl Runtime {
    pub fn new(memory: Guest, dispatch: Dispatch) -> Arc<Self> {
        Arc::new(Self {
            memory: Mutex::new(memory),
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
    pub fn memory(&self) -> MutexGuard<'_, Guest> {
        self.memory.lock().unwrap_or_else(|e| e.into_inner())
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
