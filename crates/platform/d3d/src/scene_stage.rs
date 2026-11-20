//! Scene stages' accounting (docs/graphics/framescene.md, slice 2): how
//! many each stage queued to the render thread and how many the render
//! thread dropped, with the first of each cause logged once.
//!
//! A drop is a named error: the guest was already told to skip the
//! stage's items, so they are missing from that frame. After its first
//! drop a stage stops queueing ([`stood_down`]) and its glue goes back to
//! reserving on the game thread, where a refusal leaves the items to the
//! guest; so one frame at most is short of them, and the counters say
//! which stage and why.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use timeline::native::{Metric, Stage};

/// Stages there are counters for (`timeline::native::Stage` is smaller).
const STAGES: usize = 16;

static QUEUED: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];
static DROPPED: [AtomicU64; STAGES] = [const { AtomicU64::new(0) }; STAGES];
static DOWN: [AtomicBool; STAGES] = [const { AtomicBool::new(false) }; STAGES];
static CAUSES: Mutex<Option<HashSet<(usize, &'static str)>>> = Mutex::new(None);

fn index(stage: Stage) -> usize {
    (stage as usize).min(STAGES - 1)
}

/// A stage queued to the render thread.
pub fn queued(stage: Stage) {
    QUEUED[index(stage)].fetch_add(1, Ordering::Relaxed);
    stage.count(Metric::SceneStages, 1);
}

/// A stage the render thread could not record, for `why`: counted, logged
/// once per stage and cause, and the stage stands down.
/// `ranges`: the guest ranges the stage promised.
pub fn dropped(stage: Stage, ranges: usize, why: &'static str) {
    let i = index(stage);
    DROPPED[i].fetch_add(1, Ordering::Relaxed);
    stage.count(Metric::SceneDropped, 1);
    DOWN[i].store(true, Ordering::Relaxed);
    let mut causes = CAUSES.lock().unwrap_or_else(|e| e.into_inner());
    if causes.get_or_insert_with(HashSet::new).insert((i, why)) {
        eprintln!(
            "native {}: a scene stage of {ranges} ranges was dropped on the render thread ({why}); \
             the stage reserves on the game thread from here on",
            stage.name()
        );
    }
}

/// Whether `stage` has dropped a scene stage this session: its glue
/// reserves on the game thread instead.
pub fn stood_down(stage: Stage) -> bool {
    DOWN[index(stage)].load(Ordering::Relaxed)
}

/// `scene_stages_<stage>` and `scene_dropped_<stage>` of the stages that
/// queue scene stages, the world and the objects (`render.scene`'s
/// counters).
pub fn counters() -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for stage in [Stage::World, Stage::Objects] {
        let i = index(stage);
        out.push((format!("scene_stages_{}", stage.name()), QUEUED[i].load(Ordering::Relaxed)));
        out.push((format!("scene_dropped_{}", stage.name()), DROPPED[i].load(Ordering::Relaxed)));
    }
    out
}
