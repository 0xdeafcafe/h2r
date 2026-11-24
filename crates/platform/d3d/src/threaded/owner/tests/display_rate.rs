//! The display-rate hook ([`DisplayRate`]) runs on the render thread
//! while nothing is queued, presents host-only frames through the inner
//! backend, and hears about a Swap that went out without the graph.

use super::*;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct Log {
    idle: usize,
    declined: usize,
}

/// Shows the inner backend's only surface as a host-only frame on each
/// of `frames` turns, one millisecond apart, then waits for the next
/// command.
struct Frames {
    log: Arc<Mutex<Log>>,
    surface: u32,
    frames: usize,
}

impl DisplayRate<Pixels> for Frames {
    fn idle(&mut self, backend: &mut Pixels, now: Instant) -> Option<Instant> {
        let mut log = self.log.lock().unwrap();
        // Frames between presents follow a present.
        if log.idle == self.frames || backend.shown.is_empty() {
            return None;
        }
        log.idle += 1;
        backend.present_replay(self.surface, None).unwrap();
        Some(now + Duration::from_millis(1))
    }
    fn declined(&mut self) {
        self.log.lock().unwrap().declined += 1;
    }
}

fn wait_for(log: &Mutex<Log>, done: impl Fn(&Log) -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !done(&log.lock().unwrap()) {
        assert!(Instant::now() < until, "the display-rate hook did not run");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn display_frames_run_while_nothing_is_queued_and_after_the_owned_present() {
    let (gpu, owned, target) = device(true);
    let log = Arc::new(Mutex::new(Log::default()));
    let mut gpu = gpu.with_display_rate(Box::new(Frames { log: Arc::clone(&log), surface: 0, frames: 3 }));
    gpu.clear(target, None, clear(9)).unwrap();
    gpu.present(target, 0).unwrap();
    gpu.finish().unwrap();
    wait_for(&log, |l| l.idle == 3);
    assert_eq!(owned.load(Ordering::Relaxed), 1);
    let shown = gpu.with_inner(|p| p.shown.clone()).unwrap();
    // The owned present, then the three host-only frames of its surface.
    assert_eq!(shown, [1, 1, 1, 1]);
    assert_eq!(log.lock().unwrap().declined, 0);
}

#[test]
fn a_swap_without_the_graph_tells_the_display_rate_hook() {
    let (gpu, owned, target) = device(false);
    let log = Arc::new(Mutex::new(Log::default()));
    let mut gpu = gpu.with_display_rate(Box::new(Frames { log: Arc::clone(&log), surface: 0, frames: 0 }));
    gpu.clear(target, None, clear(4)).unwrap();
    gpu.present(target, 0).unwrap();
    gpu.finish().unwrap();
    wait_for(&log, |l| l.declined == 1);
    assert_eq!(owned.load(Ordering::Relaxed), 0);
}
