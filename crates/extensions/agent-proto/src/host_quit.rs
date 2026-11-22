//! `host.quit` (docs/runtime/agent-api.md, "Close the window"): close the
//! game window as its close button does, asked for over the agent API. The
//! lab uses it when the player allows a job to take the Mac
//! (docs/runtime/lab.md, "Player consent before a job").
//!
//! The window shell (`app::shell`) [`attach`](Hub::attach)es to the
//! process-wide [`Hub`] ([`hub`]) with a waker that asks its event loop to
//! close; the method ([`Hub::quit`]) wakes it and answers at once. The
//! close itself stops the guest and exits the window's event loop, the
//! same path as the close button and `h2 play`'s SIGTERM. This module has
//! no game dependencies: the shell links it without the agent.

use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::{RpcError, code};

pub mod method {
    /// Close the game window ([`super::QuitParams`], [`super::QuitReply`]).
    pub const QUIT: &str = "host.quit";
    pub const ALL: &[&str] = &[QUIT];
}

/// `host.quit`'s params: none (`{}` or null).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuitParams {}

/// `host.quit`'s result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuitReply {
    /// The window is closing. The process exits once the guest has
    /// stopped; the caller watches for that.
    pub closing: bool,
}

type Waker = Box<dyn Fn() + Send>;

struct State {
    /// A window shell is attached, and how to ask it to close.
    shell: Option<Waker>,
    /// A close was asked for since the shell attached.
    requested: bool,
}

/// The meeting point. [`hub`] is the process's; tests make their own.
pub struct Hub {
    state: Mutex<State>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

static HUB: Hub = Hub::new();

/// The process's hub.
pub fn hub() -> &'static Hub {
    &HUB
}

const NO_WINDOW: &str = "no window here: host.quit closes a game window; a headless game \
                         stops with its process (SIGTERM)";

impl Hub {
    pub const fn new() -> Hub {
        Hub {
            state: Mutex::new(State {
                shell: None,
                requested: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A window shell is here; `close` asks its event loop to close the
    /// window. It must not block: it is called with the hub locked.
    pub fn attach(&self, close: Waker) {
        let mut s = self.lock();
        s.shell = Some(close);
        s.requested = false;
    }

    /// The shell is gone: later calls are refused.
    pub fn detach(&self) {
        let mut s = self.lock();
        s.shell = None;
        s.requested = false;
    }

    /// Whether a close was asked for since the shell attached.
    pub fn requested(&self) -> bool {
        self.lock().requested
    }

    /// `host.quit`: asks the window to close and answers at once. Repeats
    /// are harmless; refused with no window.
    pub fn quit(&self) -> Result<QuitReply, RpcError> {
        let mut s = self.lock();
        let Some(close) = &s.shell else {
            return Err(RpcError::new(code::UNAVAILABLE, NO_WINDOW));
        };
        close();
        s.requested = true;
        Ok(QuitReply { closing: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn leak() -> &'static Hub {
        Box::leak(Box::new(Hub::new()))
    }

    #[test]
    fn without_a_window_it_refuses() {
        let h = leak();
        let e = h.quit().unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(e.message.contains("headless"), "{e}");
        assert!(!h.requested());
    }

    #[test]
    fn a_quit_wakes_the_shell_and_repeats_are_harmless() {
        let h = leak();
        let closes = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&closes);
        h.attach(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(!h.requested());
        assert_eq!(h.quit().unwrap(), QuitReply { closing: true });
        assert!(h.requested());
        assert!(h.quit().unwrap().closing);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
        // The window closed: refused again, and nothing is pending.
        h.detach();
        assert!(!h.requested());
        assert_eq!(h.quit().unwrap_err().code, code::UNAVAILABLE);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_wire_shapes() {
        let p: QuitParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p, QuitParams {});
        assert!(serde_json::from_value::<QuitParams>(serde_json::json!({"save": true})).is_err());
        let v = serde_json::to_value(QuitReply { closing: true }).unwrap();
        assert_eq!(v, serde_json::json!({"closing": true}));
    }
}
