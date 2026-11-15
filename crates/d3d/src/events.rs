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
}
