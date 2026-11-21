//! Map preparation can span many presents. Account its elapsed worker time
//! at the game frame boundary, independently of the render worker's FIFO.

use crate::phase;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Stage {
    World,
    Lights,
    Fog,
    Water,
    Decorators,
    Objects,
    Layers,
    Pipelines,
    Nav,
    Visibility,
}

const STAGES: usize = Stage::Visibility as usize + 1;
const DISPATCH_NS: usize = STAGES + 1;
const DISPATCH_STALLS: usize = STAGES + 2;
pub(crate) const LABELS: [&str; STAGES + 3] = [
    "setup_world_worker_ns", "setup_lights_worker_ns", "setup_fog_worker_ns",
    "setup_water_worker_ns", "setup_decorators_worker_ns", "setup_objects_worker_ns",
    "setup_layers_worker_ns", "setup_pipelines_worker_ns", "setup_nav_worker_ns",
    "setup_visibility_worker_ns", "setup_over_33ms_frames",
    "setup_dispatch_ns", "setup_dispatch_stalled_frames",
];

impl Stage {
    /// Time this portion of a background job while frame stats are enabled.
    pub fn worker(self) -> Worker {
        Worker { stage: self, generation: phase::begin_setup(self) }
    }
}

#[must_use = "keep the guard until this worker finishes the stage"]
pub struct Worker {
    stage: Stage,
    generation: Option<u64>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(generation) = self.generation {
            phase::end_setup(self.stage, generation);
        }
    }
}

/// Time the foreground handoff that captures guest state and starts a loader.
pub fn dispatch() -> Dispatch {
    Dispatch(phase::enabled().then(crate::now_ns))
}

#[must_use = "keep the guard through the foreground handoff"]
pub struct Dispatch(Option<u64>);

impl Drop for Dispatch {
    fn drop(&mut self) {
        if let Some(start) = self.0 {
            phase::count_index(phase::SETUP_BASE + DISPATCH_NS, crate::now_ns().saturating_sub(start));
        }
    }
}

pub(crate) fn finish_dispatch(counts: &mut [u64], frame_ns: u64) {
    counts[DISPATCH_STALLS] = u64::from(frame_ns > 33_333_333 && counts[DISPATCH_NS] > 33_333_333);
}

#[derive(Default)]
struct Clock {
    last: u64,
    workers: u32,
    elapsed: u64,
}

impl Clock {
    fn advance(&mut self, now: u64) {
        self.elapsed += now.saturating_sub(self.last) * u64::from(self.workers);
        self.last = self.last.max(now);
    }
}

#[derive(Default)]
pub(crate) struct Accounting {
    stages: [Clock; STAGES],
}

impl Accounting {
    pub(crate) fn begin(&mut self, stage: Stage, now: u64) {
        let clock = &mut self.stages[stage as usize];
        clock.advance(now);
        clock.workers += 1;
    }

    pub(crate) fn end(&mut self, stage: Stage, now: u64) {
        let clock = &mut self.stages[stage as usize];
        clock.advance(now);
        clock.workers -= 1;
    }

    pub(crate) fn frame(&mut self, now: u64, frame_ns: u64, counts: &mut [u64]) {
        let mut active = false;
        for (clock, dest) in self.stages.iter_mut().zip(counts.iter_mut()) {
            clock.advance(now);
            *dest = std::mem::take(&mut clock.elapsed);
            active |= *dest != 0;
        }
        counts[STAGES] = u64::from(active && frame_ns > 33_333_333);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_overlap_and_a_foreground_stall_count_separately() {
        let mut counts = [0; LABELS.len()];
        counts[Stage::Objects as usize] = 90_000_000;
        finish_dispatch(&mut counts, 90_000_000);
        assert_eq!(counts[DISPATCH_STALLS], 0);
        counts[DISPATCH_NS] = 40_000_000;
        finish_dispatch(&mut counts, 90_000_000);
        assert_eq!(counts[DISPATCH_STALLS], 1);
    }

    #[test]
    fn workers_span_frames_and_a_slow_present_counts_once() {
        let mut work = Accounting::default();
        let mut counts = [0; LABELS.len()];
        work.begin(Stage::Fog, 4);
        work.begin(Stage::Objects, 6);
        work.begin(Stage::Fog, 9);
        work.frame(10, 40_000_000, &mut counts);
        assert_eq!((counts[Stage::Fog as usize], counts[Stage::Objects as usize], counts[STAGES]), (7, 4, 1));
        work.end(Stage::Fog, 12);
        work.end(Stage::Objects, 15);
        work.end(Stage::Fog, 18);
        work.frame(20, 12_000_000, &mut counts);
        assert_eq!((counts[Stage::Fog as usize], counts[Stage::Objects as usize], counts[STAGES]), (10, 5, 0));
        work.frame(30, 90_000_000, &mut counts);
        assert_eq!(counts, [0; LABELS.len()]);
    }
}
