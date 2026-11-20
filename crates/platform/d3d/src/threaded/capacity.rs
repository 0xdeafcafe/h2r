//! Capacity for the ordered display queue. A game-view transaction leaves
//! the queue when the worker adopts its sealed scene. A legacy frame still
//! holds a slot until its original present completes. Either way, resource
//! writes, reads and query tickets stay in the same command stream.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{MAX_FRAMES_IN_FLIGHT, Shared, THROTTLE_NS, THROTTLES, ThreadedBackend};
use crate::Backend;

#[derive(Default)]
struct State {
    released: u64,
    stopped: bool,
}

#[derive(Default)]
pub(super) struct DisplayCapacity {
    state: Mutex<State>,
    changed: Condvar,
}

impl DisplayCapacity {
    pub(super) fn release(&self, present: u64) {
        self.state.lock().unwrap().released = present;
        self.changed.notify_one();
    }

    fn wait(&self, present: u64) -> Result<Option<Duration>, String> {
        let mut state = self.state.lock().unwrap();
        let full = |s: &State| present.saturating_sub(s.released) > MAX_FRAMES_IN_FLIGHT;
        let elapsed = if !state.stopped && full(&state) {
            let t = Instant::now();
            let _phase = timeline::phase::enter(timeline::phase::Phase::RenderWait);
            state = self.changed.wait_while(state, |s| !s.stopped && full(s)).unwrap();
            Some(t.elapsed())
        } else {
            None
        };
        if state.stopped { Err("render thread: gone".into()) } else { Ok(elapsed) }
    }
}

impl<B: Backend + 'static> ThreadedBackend<B> {
    pub(super) fn wait_for_display_capacity(&mut self, present: u64) -> Result<(), String> {
        if let Some(elapsed) = self.shared.display_capacity.wait(present)? {
            self.waits.throttles += 1;
            self.waits.throttle_wait += elapsed;
            THROTTLES.fetch_add(1, Ordering::Relaxed);
            THROTTLE_NS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Also wakes the guest if a callback on the display worker unwinds.
pub(super) struct WorkerLifetime(pub(super) Arc<Shared>);

impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        let capacity = &self.0.display_capacity;
        capacity.state.lock().unwrap().stopped = true;
        capacity.changed.notify_all();
    }
}
