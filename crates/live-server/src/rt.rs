//! Tasks and timers that work both on tokio (a native server) and in a
//! Cloudflare Worker, which has no tokio runtime: there a task is a
//! JavaScript promise (`spawn_local`) and a timer is `setTimeout`
//! (docs/live/live-worker.md).
//!
//! A Worker runs one thread, so its tasks need not be `Send`; the native
//! ones still must be, as with `tokio::spawn`.

use std::future::Future;
use std::time::Duration;

/// A point in time to measure intervals from: `std`'s on a native server,
/// `performance.now()` in a Worker (where `std::time::Instant` panics).
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

/// A spawned task; [`Task::abort`] stops it at its next await.
pub struct Task {
    #[cfg(not(target_arch = "wasm32"))]
    handle: tokio::task::AbortHandle,
    #[cfg(target_arch = "wasm32")]
    handle: futures_util::future::AbortHandle,
}

impl Task {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

/// Runs `task` in the background.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(task: F) -> Task
where
    F: Future<Output = ()> + Send + 'static,
{
    Task {
        handle: tokio::spawn(task).abort_handle(),
    }
}

/// Runs `task` in the background.
#[cfg(target_arch = "wasm32")]
pub fn spawn<F>(task: F) -> Task
where
    F: Future<Output = ()> + 'static,
{
    let (handle, registration) = futures_util::future::AbortHandle::new_pair();
    let task = futures_util::future::Abortable::new(task, registration);
    wasm_bindgen_futures::spawn_local(async move {
        let _ = task.await;
    });
    Task { handle }
}

pub async fn sleep(d: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(d).await;
    // A Worker has one thread, so the JavaScript timer never moves; axum
    // wants `Send` futures all the same.
    #[cfg(target_arch = "wasm32")]
    send_wrapper::SendWrapper::new(gloo_timers::future::sleep(d)).await;
}

/// The time ran out before the future finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

/// `future`'s output, unless `d` passes first.
pub async fn timeout<F: Future>(d: Duration, future: F) -> Result<F::Output, Elapsed> {
    #[cfg(not(target_arch = "wasm32"))]
    return tokio::time::timeout(d, future).await.map_err(|_| Elapsed);
    #[cfg(target_arch = "wasm32")]
    {
        use futures_util::future::{Either, select};
        let future = std::pin::pin!(future);
        let timer = std::pin::pin!(sleep(d));
        match select(future, timer).await {
            Either::Left((out, _)) => Ok(out),
            Either::Right(_) => Err(Elapsed),
        }
    }
}

/// Ticks every `period`, the first at once. A tick that comes late moves
/// the ones after it (tokio's `MissedTickBehavior::Delay`).
pub struct Interval {
    #[cfg(not(target_arch = "wasm32"))]
    inner: tokio::time::Interval,
    #[cfg(target_arch = "wasm32")]
    period: Duration,
    #[cfg(target_arch = "wasm32")]
    next: Option<Instant>,
}

pub fn interval(period: Duration) -> Interval {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut inner = tokio::time::interval(period);
        inner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Interval { inner }
    }
    #[cfg(target_arch = "wasm32")]
    Interval { period, next: None }
}

impl Interval {
    pub async fn tick(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.inner.tick().await;
        #[cfg(target_arch = "wasm32")]
        {
            let now = Instant::now();
            if let Some(next) = self.next
                && next > now
            {
                sleep(next - now).await;
            }
            self.next = Some(Instant::now() + self.period);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn timeout_and_interval_keep_tokio_s_meaning() {
        let slow = timeout(Duration::from_secs(1), sleep(Duration::from_secs(2))).await;
        assert_eq!(slow, Err(Elapsed));
        let quick = timeout(Duration::from_secs(2), async { 7 }).await;
        assert_eq!(quick, Ok(7));

        let start = tokio::time::Instant::now();
        let mut every = interval(Duration::from_millis(100));
        every.tick().await;
        assert_eq!(start.elapsed(), Duration::ZERO, "the first tick is at once");
        every.tick().await;
        every.tick().await;
        assert_eq!(start.elapsed(), Duration::from_millis(200));
    }

    #[tokio::test]
    async fn an_aborted_task_stops() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = spawn(async move {
            sleep(Duration::from_secs(60)).await;
            let _ = tx.send(());
        });
        task.abort();
        assert!(rx.await.is_err(), "the task was dropped without sending");
    }
}
