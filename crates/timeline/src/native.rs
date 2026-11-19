//! Native replacement counters share the game/worker frame boundary with D3D.
use crate::phase;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Stage { World, Objects, Transparents, Lights, Post, Particles, Decals, Hud, Gels, Fog, Water }

const STAGES: [&str; 11] = ["world", "objects", "transparents", "lights", "post", "particles", "decals", "hud", "gels", "fog", "water"];
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Metric {
    Examined, Suppressed, RefusedOwnership, RefusedResources, RefusedRanges, RefusedMasks, RefusedInputs, RefusedValidation, RefusedReplay,
    PrepareNs, EncodeNs, Buffers, BindGroups, UploadBytes, PipelineMisses,
    WorkerRoundTrips, ReferencePasses, DisplayPasses,
}
const METRICS: [&str; 18] = ["examined", "suppressed", "refused_ownership", "refused_resources", "refused_ranges",
    "refused_masks", "refused_inputs", "refused_validation", "refused_replay", "prepare_ns", "encode_ns", "buffers", "bind_groups", "upload_bytes", "pipeline_misses",
    "worker_round_trips", "reference_passes", "display_passes"];

// Four Xbox player views and a bucket for non-player views.
const VIEWS: usize = 5;
const VIEW_METRICS: [&str; 9] = [
    "reference_ranges", "display_ranges", "invariants", "reference_recorded", "display_recorded",
    "reference_invariants", "display_invariants", "reference_refused", "display_refused",
];
const STRIDE: usize = METRICS.len() + VIEWS * VIEW_METRICS.len();
pub(crate) const COLUMNS: usize = STAGES.len() * STRIDE;
pub(crate) fn labels() -> &'static [String] {
    static LABELS: OnceLock<Vec<String>> = OnceLock::new();
    LABELS.get_or_init(|| {
        let mut out = Vec::with_capacity(COLUMNS);
        for stage in STAGES {
            for name in METRICS { out.push(format!("native_{stage}_{name}")); }
            for view in ["0", "1", "2", "3", "other"] {
                for name in VIEW_METRICS {
                    out.push(format!("native_{stage}_v{view}_{name}"));
                }
            }
        }
        out
    })
}

impl Stage {
    pub fn count(self, metric: Metric, n: u64) {
        phase::count_index(phase::Counter::ALL.len() + self as usize * STRIDE + metric as usize, n);
    }

    fn view_index(self, view: u32) -> usize {
        phase::Counter::ALL.len() + self as usize * STRIDE + METRICS.len() + (view as usize).min(VIEWS - 1) * VIEW_METRICS.len()
    }

    pub fn recorded(self, view: u32, display: bool, ranges: usize) {
        phase::count_index(self.view_index(view) + usize::from(display), ranges as u64);
    }

    pub fn attempted(self, view: u32, display: bool, ranges: usize) {
        phase::count_index(self.view_index(view) + 3 + usize::from(display), ranges as u64);
    }

    pub fn copy_refused(self, view: u32, display: bool) {
        phase::count_index(self.view_index(view) + 7 + usize::from(display), 1);
    }

    pub fn invariant(self, view: u32, display: bool) {
        phase::count_index(self.view_index(view) + 2, 1);
        phase::count_index(self.view_index(view) + 5 + usize::from(display), 1);
    }
}

thread_local! {
    static ACTIVE: std::cell::Cell<Option<Stage>> = const { std::cell::Cell::new(None) };
}

/// Charge actual GPU resource operations to the stage currently executing
/// on this thread. A shared device may serve more than one backend.
#[inline]
pub fn resource(metric: Metric, n: u64) {
    if phase::enabled() && let Some(stage) = ACTIVE.with(std::cell::Cell::get) {
        stage.count(metric, n);
    }
}

pub struct Timer(Option<(Stage, Metric, std::time::Instant, Option<Stage>)>);

impl Timer {
    pub fn start(stage: Stage, metric: Metric) -> Self {
        Self(phase::enabled().then(|| (stage, metric, std::time::Instant::now(), ACTIVE.with(|a| a.replace(Some(stage))))))
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some((stage, metric, start, previous)) = self.0 {
            ACTIVE.with(|a| a.set(previous));
            stage.count(metric, start.elapsed().as_nanos() as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_columns_identify_each_view_and_copy() {
        let columns: Vec<_> = phase::counter_columns().collect();
        assert_eq!(columns.len(), phase::COUNTERS);
        for (i, (slot, _)) in columns.iter().enumerate() { assert_eq!(*slot, i); }
        for (metric, label) in [(Metric::WorkerRoundTrips, "native_lights_worker_round_trips"),
            (Metric::ReferencePasses, "native_lights_reference_passes"), (Metric::DisplayPasses, "native_lights_display_passes")] {
            assert_eq!(columns[phase::Counter::ALL.len() + Stage::Lights as usize * STRIDE + metric as usize].1, label);
        }
        let i = Stage::Objects.view_index(2);
        assert_eq!(columns[i].1, "native_objects_v2_reference_ranges");
        assert_eq!(columns[i + 1].1, "native_objects_v2_display_ranges");
        assert_eq!(columns[i + 2].1, "native_objects_v2_invariants");
    }
}
