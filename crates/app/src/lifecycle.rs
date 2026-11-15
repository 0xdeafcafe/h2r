//! Stop and pause, shared between the window thread and the guest thread.
//!
//! The guest is never killed mid-instruction: the window asks, and the guest
//! thread checks between scheduler slices ([`crate::GuestHost::tick`] or
//! [`Lifecycle::should_stop`]) and returns. Every wait here wakes promptly on
//! a stop request.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What the guest loop should do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Continue,
    Stop,
}

#[derive(Default)]
struct Flags {
    stop: bool,
    paused: bool,
}

#[derive(Default)]
struct Inner {
    flags: Mutex<Flags>,
    cv: Condvar,
}

/// Cloneable, `Send + Sync`.
#[derive(Clone, Default)]
pub struct Lifecycle {
    inner: Arc<Inner>,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    fn flags(&self) -> MutexGuard<'_, Flags> {
        self.inner.flags.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ask the guest to stop. Wakes any pause or pacing wait.
    pub fn request_stop(&self) {
        self.flags().stop = true;
        self.inner.cv.notify_all();
    }

    pub fn should_stop(&self) -> bool {
        self.flags().stop
    }

    pub fn set_paused(&self, paused: bool) {
        self.flags().paused = paused;
        self.inner.cv.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.flags().paused
    }

    /// Blocks while paused. Returns whether it waited at all, and what to do.
    pub fn wait_while_paused(&self) -> (bool, Control) {
        let mut f = self.flags();
        let mut waited = false;
        while f.paused && !f.stop {
            waited = true;
            f = self.inner.cv.wait(f).unwrap_or_else(|e| e.into_inner());
        }
        (
            waited,
            if f.stop {
                Control::Stop
            } else {
                Control::Continue
            },
        )
    }

    /// Sleeps for `d`. Returns early with `Stop` on a stop request, and
    /// early with `Continue` when a pause begins (the caller then checks
    /// [`is_paused`](Self::is_paused) and waits).
    pub fn sleep(&self, d: Duration) -> Control {
        let deadline = Instant::now() + d;
        let mut f = self.flags();
        let paused_at_start = f.paused;
        loop {
            if f.stop {
                return Control::Stop;
            }
            if f.paused && !paused_at_start {
                return Control::Continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Control::Continue;
            }
            f = self
                .inner
                .cv
                .wait_timeout(f, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_interrupts_a_long_sleep() {
        let l = Lifecycle::new();
        let l2 = l.clone();
        let t = Instant::now();
        let h = std::thread::spawn(move || l2.sleep(Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(20));
        l.request_stop();
        assert_eq!(h.join().unwrap(), Control::Stop);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn sleep_runs_to_its_deadline() {
        let l = Lifecycle::new();
        let t = Instant::now();
        assert_eq!(l.sleep(Duration::from_millis(15)), Control::Continue);
        assert!(t.elapsed() >= Duration::from_millis(15));
    }

    #[test]
    fn pause_blocks_until_resumed_or_stopped() {
        let l = Lifecycle::new();
        assert_eq!(l.wait_while_paused(), (false, Control::Continue));
        l.set_paused(true);
        let l2 = l.clone();
        let h = std::thread::spawn(move || l2.wait_while_paused());
        std::thread::sleep(Duration::from_millis(20));
        assert!(!h.is_finished());
        l.set_paused(false);
        assert_eq!(h.join().unwrap(), (true, Control::Continue));

        l.set_paused(true);
        let l2 = l.clone();
        let h = std::thread::spawn(move || l2.wait_while_paused());
        std::thread::sleep(Duration::from_millis(20));
        l.request_stop();
        assert_eq!(h.join().unwrap(), (true, Control::Stop));
    }
    #[test]
    fn a_pause_ends_a_sleep_early() {
        let l = Lifecycle::new();
        let l2 = l.clone();
        let t = Instant::now();
        let h = std::thread::spawn(move || l2.sleep(Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(20));
        l.set_paused(true);
        assert_eq!(h.join().unwrap(), Control::Continue);
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
