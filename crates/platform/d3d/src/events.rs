//! Errors (the call cannot be completed: stop) and events (something worth
//! recording happened, execution continues).

use crate::guest::GuestError;
use crate::pushbuf::PushError;
use crate::state::BadState;

/// A D3D call that cannot be completed. Adapters turn this into a runtime
/// stop with the message; nothing is silently skipped.
#[derive(Debug, thiserror::Error)]
pub enum D3dError {
    /// Guest memory fault while reading arguments or resources.
    #[error(transparent)]
    Guest(#[from] GuestError),
    /// An entry point, or a mode of one, that is not implemented.
    #[error("D3D {entry}: unimplemented: {detail}")]
    Unimplemented {
        /// Entry point name (XDK name where known).
        entry: &'static str,
        /// What exactly is missing.
        detail: String,
    },
    /// Invalid arguments that real D3D would assert on.
    #[error("D3D {entry}: invalid call: {detail}")]
    Invalid {
        /// Entry point name.
        entry: &'static str,
        /// What is wrong.
        detail: String,
    },
    /// The guest push buffer held something the device cannot execute.
    #[error("push buffer: {0}")]
    Push(#[from] PushError),
    /// The device was used before `Direct3D_CreateDevice`.
    #[error("D3D {0}: no device")]
    NoDevice(&'static str),
    /// An earlier push-buffer failure latched the device: the failing
    /// command was not executed, and nothing after it will be.
    #[error("D3D device faulted earlier: {0}")]
    Faulted(String),
    /// The host backend failed (GPU or translation error).
    #[error("D3D backend: {0}")]
    Backend(String),
}

impl D3dError {
    pub(crate) fn unimplemented(entry: &'static str, detail: impl Into<String>) -> Self {
        Self::Unimplemented {
            entry,
            detail: detail.into(),
        }
    }
    pub(crate) fn invalid(entry: &'static str, detail: impl Into<String>) -> Self {
        Self::Invalid {
            entry,
            detail: detail.into(),
        }
    }
}

/// The last call that stopped the guest, recorded by the runtime glue (see
/// [`crate::runtime_glue`]) so the harness can report the whole message
/// whatever event its runtime version is able to carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HleStop {
    /// Guest address of the entry point.
    pub entry: u32,
    /// Its name.
    pub name: &'static str,
    /// The error, rendered.
    pub error: String,
}

/// Something the device noticed. Drained with `Device::take_events`.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A draw the backend could not render (no shader for it yet, a vertex
    /// format it cannot read). The draw was skipped; the call succeeded.
    DrawSkipped {
        /// Entry point that issued it.
        entry: &'static str,
        /// Why.
        reason: String,
    },
    /// A state value with no translation; the default was used.
    BadState(BadState),
    /// A cached copy changed without a reported write (`UploadPolicy::Verify`):
    /// the runtime's write barrier missed `[addr, addr + len)`.
    MissedInvalidation {
        /// Resource header.
        header: u32,
        /// Data address.
        addr: u32,
        /// Data length.
        len: u32,
    },
    /// Guest code stored into the NV2A register window
    /// ([`crate::regs`]); the store was undone. No game site does this.
    RegisterWrite {
        /// Register offset from the window base.
        offset: u32,
        /// The word stored.
        value: u32,
    },
}

/// The events a device keeps until a caller drains them
/// ([`crate::Device::take_events`]). Draws record events every frame and the
/// product drains them only for the run report at exit, so the log keeps
/// the first [`EventLog::LIMIT`] and counts the rest: a long session or a
/// heavy effect load cannot grow it past a fixed size.
#[derive(Debug, Default)]
pub(crate) struct EventLog {
    kept: Vec<Event>,
    dropped: u64,
}

impl EventLog {
    /// Events kept between drains.
    pub(crate) const LIMIT: usize = 4096;

    pub(crate) fn push(&mut self, event: Event) {
        if self.kept.len() < Self::LIMIT {
            self.kept.push(event);
        } else {
            self.dropped += 1;
        }
    }

    /// The kept events and how many were counted past the limit since the
    /// last drain.
    pub(crate) fn take(&mut self) -> (Vec<Event>, u64) {
        (
            std::mem::take(&mut self.kept),
            std::mem::take(&mut self.dropped),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skipped(n: usize) -> Event {
        Event::DrawSkipped {
            entry: "draw",
            reason: format!("draw {n}: a reason long enough to cost a heap allocation"),
        }
    }

    #[test]
    fn a_long_session_keeps_a_fixed_number_of_events() {
        let mut log = EventLog::default();
        // A frame's worth of skipped draws, for many frames, never drained.
        for frame in 0..500 {
            for draw in 0..2000 {
                log.push(skipped(frame * 2000 + draw));
            }
            assert!(log.kept.len() <= EventLog::LIMIT);
            assert!(log.kept.capacity() <= 2 * EventLog::LIMIT);
        }
        let (kept, dropped) = log.take();
        assert_eq!(kept.len(), EventLog::LIMIT);
        assert_eq!(kept[0], skipped(0));
        assert_eq!(dropped, 500 * 2000 - EventLog::LIMIT as u64);
        // A drain starts the window again.
        log.push(skipped(1));
        assert_eq!(log.take(), (vec![skipped(1)], 0));
    }
}
