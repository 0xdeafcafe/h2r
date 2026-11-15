//! Owned references and completion signalling for producers in other HLE crates.
//!
//! Lock order for every kernel service is kernel state -> guest memory -> VFS.
//! `signal_event` takes kernel state and then guest memory itself, so it must
//! never be called while the caller holds `runtime.memory()` (the same thread
//! would block on the memory mutex it already owns) or the shared VFS guard.
//! A producer that completes work while it holds the Guest guard uses
//! `DispatcherReference::defer_signal` instead: that takes only the kernel's
//! completion-queue mutex, a leaf lock under which nothing else is acquired.
//! The queue is drained, under kernel state then guest memory, at the start of
//! every `Machine::run` slice or by `Kernel::drain_deferred_signals`.
use crate::{Action, Error, Kernel, abi::Value, status, sync::Object};
use std::sync::{Arc, MutexGuard};

/// Pins an HLE dispatcher object independently of its guest handles. Dropping
/// the reference releases the pin. The guest address is never a host pointer.
///
/// Dropping or releasing takes the kernel-state lock (not guest memory). Do so
/// after releasing any Guest guard, or hand the pin to `defer_signal`, which
/// keeps it until the deferred signal has been applied.
pub struct DispatcherReference {
    kernel: Arc<Kernel>,
    address: u32,
    /// False once ownership of the pin has moved into the deferred queue.
    armed: bool,
}

/// Outcome of one deferred completion. `result` is the event's previous signal
/// state, or why it could not be signalled (for example a changed guest header).
#[derive(Debug)]
pub struct DeferredSignal {
    pub address: u32,
    pub result: Result<i32, Error>,
}

impl DispatcherReference {
    pub fn address(&self) -> u32 {
        self.address
    }
    /// Signal the pinned event even after its original handle has been closed.
    /// Takes kernel state then guest memory: never call this while holding
    /// `runtime.memory()` or the VFS guard; use `defer_signal` there.
    pub fn signal_event(&self) -> Result<i32, Error> {
        self.kernel.signal_address(self.address)
    }
    /// Queue this pinned event to be signalled once no Guest guard is held.
    /// Safe to call while holding `runtime.memory()` or the VFS guard: it takes
    /// only the leaf completion-queue lock and does not release the pin here.
    /// The queue owns the pin until the signal is applied, after which the pin
    /// is released exactly as `release` would. Each queued pin is signalled
    /// once; queue the same object twice by deferring two references.
    pub fn defer_signal(mut self) {
        self.armed = false;
        self.kernel.deferred_queue().push(self.address);
    }
    pub fn release(self) {
        drop(self);
    }
}
impl Drop for DispatcherReference {
    fn drop(&mut self) {
        if self.armed {
            self.kernel.lock().release_reference(self.address);
        }
    }
}
impl crate::Core {
    pub(crate) fn release_reference(&mut self, address: u32) {
        if let Some(count) = self.references.get_mut(&address) {
            *count -= 1;
            if *count == 0 {
                self.references.remove(&address);
            }
        }
        self.collect_objects();
    }
}
impl Kernel {
    pub fn reference_dispatcher(
        self: &Arc<Self>,
        handle: u32,
    ) -> Result<DispatcherReference, Error> {
        let runtime = self.runtime()?;
        let mut core = self.lock();
        let address = core.handles.get(handle).map_err(Error::Resource)?;
        if core.ensure_object(&runtime.memory(), address)?.is_none() {
            return Err(Error::Invalid("invalid/unsupported dispatcher object"));
        }
        let count = core.references.entry(address).or_default();
        *count = count
            .checked_add(1)
            .ok_or(Error::Resource(status::INSUFFICIENT_RESOURCES))?;
        Ok(DispatcherReference {
            kernel: Arc::clone(self),
            address,
            armed: true,
        })
    }
    /// Handle lookup and signal are atomic with respect to guest NtClose.
    /// This changes dispatcher state and readies waiters; it runs no guest code.
    /// Takes kernel state then guest memory: never call while holding either
    /// `runtime.memory()` or the VFS guard.
    pub fn signal_event(&self, handle: u32) -> Result<i32, Error> {
        let runtime = self.runtime()?;
        let mut core = self.lock();
        let address = core.handles.get(handle).map_err(Error::Resource)?;
        Self::signal_locked(&mut core, &mut runtime.memory(), address)
    }
    /// Apply every queued `defer_signal` now, in queue order, and release
    /// their pins. Takes kernel state then guest memory, so call it only with
    /// no Guest or VFS guard held. `Machine::run` also drains the queue at the
    /// start of each slice; failures there are kept for
    /// `take_deferred_signal_failures`.
    pub fn drain_deferred_signals(&self) -> Result<Vec<DeferredSignal>, Error> {
        let runtime = self.runtime()?;
        let mut core = self.lock();
        let mut memory = runtime.memory();
        Ok(self.drain_locked(&mut core, &mut memory))
    }
    /// Number of signals queued and not yet applied.
    pub fn pending_deferred_signals(&self) -> usize {
        self.deferred_queue().len()
    }
    /// Deferred signals that `Machine::run` could not apply, oldest first.
    /// At most `limits::MAX_DEFERRED_FAILURES` are kept; see
    /// `dropped_deferred_signal_failures`.
    pub fn take_deferred_signal_failures(&self) -> Vec<DeferredSignal> {
        std::mem::take(&mut self.lock().deferred_failures)
    }
    pub fn dropped_deferred_signal_failures(&self) -> u64 {
        self.lock().dropped_deferred_failures
    }
    pub(crate) fn drain_deferred_into_failures(
        &self,
        core: &mut crate::Core,
        memory: &mut runtime::Guest,
    ) {
        for signal in self.drain_locked(core, memory) {
            if signal.result.is_err() {
                crate::push_bounded(
                    &mut core.deferred_failures,
                    crate::limits::MAX_DEFERRED_FAILURES,
                    &mut core.dropped_deferred_failures,
                    signal,
                );
            }
        }
    }
    fn drain_locked(
        &self,
        core: &mut crate::Core,
        memory: &mut runtime::Guest,
    ) -> Vec<DeferredSignal> {
        // Leaf lock: taken and released without acquiring anything else.
        let queued = std::mem::take(&mut *self.deferred_queue());
        queued
            .into_iter()
            .map(|address| {
                let result = Self::signal_locked(core, memory, address);
                core.release_reference(address);
                DeferredSignal { address, result }
            })
            .collect()
    }
    fn deferred_queue(&self) -> MutexGuard<'_, Vec<u32>> {
        self.deferred.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn signal_address(&self, address: u32) -> Result<i32, Error> {
        let runtime = self.runtime()?;
        Self::signal_locked(&mut self.lock(), &mut runtime.memory(), address)
    }
    fn signal_locked(
        core: &mut crate::Core,
        memory: &mut runtime::Guest,
        address: u32,
    ) -> Result<i32, Error> {
        match core.ensure_object(memory, address)? {
            Some(Object::Event { .. }) => {}
            Some(_) => return Err(Error::Resource(status::OBJECT_TYPE_MISMATCH)),
            None => return Err(Error::Invalid("invalid/unsupported event header")),
        }
        match core.event_set(memory, address, true)? {
            Action::Complete(Value::U32(old)) => Ok(old as i32),
            _ => Err(Error::Invalid("unsupported event signal")),
        }
    }
}
