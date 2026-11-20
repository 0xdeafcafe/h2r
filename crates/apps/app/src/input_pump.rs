//! Live input into XAPI, in the order docs/runtime/xapi.md requires.
//!
//! The window thread only feeds the `KeyboardMouse` handle (and gilrs reads
//! pads on its own); nothing reaches the guest until the guest thread runs
//! one pump per guest frame:
//!
//! 1. with guest memory and the XAPI state locked, `sync_input` (which calls
//!    `InputManager::sync`, publishes device changes and writes feedback
//!    completion statuses), then `take_signals`;
//! 2. drop the memory guard and the XAPI lock;
//! 3. deliver the batch (kernel event signalling relocks guest memory);
//! 4. lock XAPI again and `acknowledge_signals`.
//!
//! Lock order is guest memory, then XAPI, as in docs/runtime/xapi.md.

use std::sync::{Mutex, MutexGuard};

use xapi::{Events, GuestMemory, Storage, Xapi};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one pump did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PumpReport {
    /// Event signals attempted.
    pub signals: usize,
}

/// One input frame. `memory` is the caller's guest-memory adapter, owning
/// its guard; it is dropped before any event is signalled. Errors from
/// `sync_input` are returned after the signals that were owed are still
/// delivered and acknowledged, so one bad feedback buffer cannot stall the
/// rest.
pub fn pump_input<S, M, E>(
    xapi: &Mutex<Xapi<S>>,
    mut memory: M,
    events: &mut E,
) -> xapi::Result<PumpReport>
where
    S: Storage,
    M: GuestMemory,
    E: Events,
{
    let (synced, batch) = {
        let mut x = lock(xapi);
        let synced = x.sync_input(&mut memory);
        (synced, x.take_signals())
    };
    drop(memory);
    let signals = batch.signals().len();
    let report = batch.deliver(events);
    let acked = lock(xapi).acknowledge_signals(report);
    synced?;
    acked?;
    Ok(PumpReport { signals })
}
