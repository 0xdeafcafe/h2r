//! The guest thread: spawn, outcome capture, and stop with a timeout.
//!
//! The window uses this; it is separate from the winit code so the stop
//! paths (close while sleeping, close while paused, a guest that ignores the
//! request, return, error, panic) are tested on real threads.

use std::panic::AssertUnwindSafe;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::lifecycle::Lifecycle;
use crate::{GuestExit, GuestHost};

/// The guest entry point `app::run` takes.
pub type GuestFn = Box<dyn FnOnce(GuestHost) -> anyhow::Result<()> + Send>;

pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into())
}

/// A running guest thread.
pub struct GuestWorker {
    life: Lifecycle,
    join: Option<JoinHandle<GuestExit>>,
    exit: Option<GuestExit>,
}

impl GuestWorker {
    /// Runs `guest(host)` on a thread named "guest" with a 64 MiB stack.
    /// `on_exit` runs on that thread after the guest returns or panics (the
    /// shell uses it to wake the event loop).
    pub fn spawn(
        host: GuestHost,
        guest: GuestFn,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        let life = host.lifecycle();
        let join = std::thread::Builder::new()
            .name("guest".into())
            // Generated code nests deeply; give it room.
            .stack_size(64 << 20)
            .spawn(move || {
                let exit = match std::panic::catch_unwind(AssertUnwindSafe(|| guest(host))) {
                    Ok(Ok(())) => GuestExit::Returned,
                    Ok(Err(e)) => GuestExit::Failed(format!("{e:#}")),
                    Err(p) => GuestExit::Panicked(panic_message(&*p)),
                };
                on_exit();
                exit
            })?;
        Ok(Self {
            life,
            join: Some(join),
            exit: None,
        })
    }

    /// The guest's stop/pause flags.
    pub fn lifecycle(&self) -> &Lifecycle {
        &self.life
    }

    /// The outcome, if the thread has finished (joins it).
    pub fn try_finish(&mut self) -> Option<&GuestExit> {
        if self.join.as_ref().is_some_and(|h| h.is_finished()) {
            let h = self.join.take().expect("checked");
            self.exit = Some(
                h.join()
                    .unwrap_or_else(|p| GuestExit::Panicked(panic_message(&*p))),
            );
        }
        self.exit.as_ref()
    }

    /// Asks the guest to stop and waits up to `timeout` for the thread.
    /// A guest that does not return in time is reported as `StillRunning`
    /// and left to end with the process.
    pub fn stop(&mut self, timeout: Duration) -> GuestExit {
        self.life.request_stop();
        let deadline = Instant::now() + timeout;
        while self.try_finish().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        match self.try_finish() {
            Some(e) => e.clone(),
            None => GuestExit::StillRunning,
        }
    }
}
