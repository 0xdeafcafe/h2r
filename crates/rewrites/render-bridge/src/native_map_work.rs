//! Map-wide empty results from the native loaders. Draw hooks only read the
//! atomic mask. The lock coordinates map activation with worker reports.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use engine::points;

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "native-map-work";

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Stage {
    World = 1 << 0,
    Lights = 1 << 1,
    Fog = 1 << 2,
    Water = 1 << 3,
    Objects = 1 << 4,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::World => "native-world",
            Self::Lights => "native-lights",
            Self::Fog => "native-fog",
            Self::Water => "native-water layer-6 edge blend",
            Self::Objects => "native-objects",
        }
    }

    fn log_stand_down(self, map: &str) {
        eprintln!("{}: nothing native on {map}; standing down until the next map", self.name());
    }

    #[inline]
    pub fn standing_down(self) -> bool {
        MAP.standing_down(self)
    }
}

const BSP_STAGES: [Stage; 4] = [Stage::World, Stage::Lights, Stage::Fog, Stage::Water];

struct MapWork {
    generation: AtomicU64,
    standing_down: AtomicU8,
    // One result per BSP. Unvisited BSPs supply no evidence of map-wide emptiness.
    bsps: Mutex<Vec<Option<u8>>>,
}

impl MapWork {
    const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            standing_down: AtomicU8::new(0),
            bsps: Mutex::new(Vec::new()),
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[inline]
    fn standing_down(&self, stage: Stage) -> bool {
        self.standing_down.load(Ordering::Relaxed) & stage as u8 != 0
    }

    fn reset(&self) {
        let mut bsps = self.bsps.lock().unwrap_or_else(|e| e.into_inner());
        bsps.clear();
        self.standing_down.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn bsp_loaded(
        &self,
        generation: u64,
        map: &str,
        index: usize,
        count: usize,
        empty: [Option<Stage>; 4],
    ) {
        let mut bsps = self.bsps.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation() != generation || index >= count {
            return;
        }
        if bsps.is_empty() {
            bsps.resize(count, None);
        }
        if bsps.len() != count {
            return;
        }
        let empty = empty
            .into_iter()
            .flatten()
            .fold(0, |mask, stage| mask | stage as u8);
        // A BSP that supplied work earlier can do so again on a revisit.
        let previous = bsps[index].unwrap_or(u8::MAX);
        bsps[index] = Some(previous & empty);
        let map_empty = bsps
            .iter()
            .fold(u8::MAX, |mask, entry| mask & entry.unwrap_or(0));
        // Publish the bits while holding the activation lock. Diagnostics
        // may write to a full pipe and must never hold up a map switch.
        let previous = self.standing_down.fetch_or(map_empty, Ordering::Relaxed);
        drop(bsps);
        for stage in BSP_STAGES {
            if map_empty & !previous & stage as u8 != 0 {
                stage.log_stand_down(map);
            }
        }
    }

    fn map_stage_empty(&self, generation: u64, map: &str, stage: Stage) {
        let bsps = self.bsps.lock().unwrap_or_else(|e| e.into_inner());
        if self.generation() == generation {
            let bit = stage as u8;
            let previous = self.standing_down.fetch_or(bit, Ordering::Relaxed);
            drop(bsps);
            if previous & bit == 0 { stage.log_stand_down(map); }
        }
    }

}

// Also distinguishes a reload into the same BSP address on revert or switch.
static BSP_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn bsp_generation() -> u64 {
    BSP_GENERATION.load(Ordering::Acquire)
}

static MAP: MapWork = MapWork::new();
pub(crate) static BSP_LOAD_PENDING: AtomicBool = AtomicBool::new(true);

/// Discard host-side coverage and queued mask restores on activation or
/// after boot restores guest RAM from a machine snapshot.
pub fn reset() {
    crate::native_world::forget_pending();
    crate::native_fog::forget_pending();
    crate::native_layers::forget_pending();
    crate::native_water::forget_pending();
    crate::native_decorators::forget_pending();
    crate::native_objects::forget_pending();
    MAP.reset();
    BSP_GENERATION.fetch_add(1, Ordering::Release);
    crate::scene_step::map_changed();
    retire_bsp();
    crate::native_lights::retire_map();
    crate::native_objects::retire_map();
    crate::native_first_person::retire_map();
    crate::native_post::retire_map();
    request_bsp_load();
}

pub(crate) fn request_bsp_load() {
    BSP_LOAD_PENDING.store(true, Ordering::Release);
}

/// Reloads the active BSP's native stages from the map file, as a BSP
/// switch does: the loaded copy stops drawing at once (its key is no
/// longer current) and the reload publishes when ready. A live edit the
/// native copy cannot take in place asks for it
/// (docs/editor/realtime-edits.md, "The native refresh").
pub fn reload_bsp() {
    BSP_GENERATION.fetch_add(1, Ordering::Release);
    retire_bsp();
    request_bsp_load();
}

fn retire_bsp() {
    crate::native_world::retire_bsp();
    crate::native_transparents::retire_bsp();
}

/// A cache load's identity, including reloads of the same map.
pub fn generation() -> u64 {
    MAP.generation()
}

/// Reports which enabled stages cannot draw on the loaded BSP. A stage
/// stands down for the map when every BSP reports it empty.
pub fn bsp_loaded(
    generation: u64,
    map: &str,
    index: usize,
    count: usize,
    empty: [Option<Stage>; 4],
) {
    MAP.bsp_loaded(generation, map, index, count, empty);
}

/// Reports an empty stage whose coverage is known across all BSPs.
pub fn map_stage_empty(generation: u64, map: &str, stage: Stage) {
    MAP.map_stage_empty(generation, map, stage);
}

/// Keeps the stand-down flags and shared BSP loader in step with map changes.
pub struct NativeMapWork;

impl Feature for NativeMapWork {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native render map lifecycle").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let needs_bsp = crate::native_world::on(b.switches())
            || crate::native_lights::on(b.switches())
            || crate::native_fog::on(b.switches())
            || crate::native_water::on(b.switches());
        // Boot resets this derived state after loading the feature sections.
        b.snapshot(SnapshotPolicy::Stateless);
        b.game().on_start("native_map_reset", |_| reset());
        if needs_bsp {
            b.raw()
                .observe_before(&points::RENDER_VIEWS, "native_bsp_publish", |_| {
                    crate::native_world::begin_frame();
                })
                .context_only();
            for point in [&points::BSP_SWITCH, &points::GAME_STATE_REVERT] {
                b.raw()
                    .observe_after(point, "native_bsp_reload", |_| reload_bsp())
                    .optional();
            }
            b.raw()
                .observe_before(&points::LAYER_DRAW, "native_bsp_load", |o| {
                    if BSP_LOAD_PENDING.load(Ordering::Acquire) {
                        crate::native_world::load_bsp(&*o.guest(), o.cpu.esp);
                    }
                })
                .after_feature(crate::render_timing::ID)
                .context_only();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_change_resets_stages_and_rejects_old_workers() {
        let work = MapWork::new();
        let first = work.generation();
        work.bsp_loaded(first, "menu", 0, 1, BSP_STAGES.map(Some));
        work.map_stage_empty(first, "menu", Stage::Objects);
        assert!(BSP_STAGES.into_iter().all(|stage| work.standing_down(stage)));
        assert!(work.standing_down(Stage::Objects));

        work.reset();
        assert!(BSP_STAGES.into_iter().all(|stage| !work.standing_down(stage)));
        assert!(!work.standing_down(Stage::Objects));
        work.bsp_loaded(first, "menu", 0, 1, BSP_STAGES.map(Some));
        work.map_stage_empty(first, "menu", Stage::Objects);
        assert!(BSP_STAGES.into_iter().all(|stage| !work.standing_down(stage)));
        assert!(!work.standing_down(Stage::Objects));

        let second = work.generation();
        work.bsp_loaded(second, "menu", 0, 1, [None, Some(Stage::Lights), None, None]);
        assert!(work.standing_down(Stage::Lights));
        assert!(!work.standing_down(Stage::World));
    }

    #[test]
    fn stand_down_requires_every_bsp_and_preserves_earlier_positive_results() {
        let work = MapWork::new();
        let map = work.generation();
        work.bsp_loaded(
            map,
            "level",
            0,
            2,
            [None, Some(Stage::Lights), Some(Stage::Fog), Some(Stage::Water)],
        );
        work.bsp_loaded(map, "level", 0, 2, BSP_STAGES.map(Some));
        assert!(!work.standing_down(Stage::Lights));

        work.bsp_loaded(
            map,
            "level",
            1,
            2,
            [Some(Stage::World), None, Some(Stage::Fog), Some(Stage::Water)],
        );
        assert!(!work.standing_down(Stage::World));
        assert!(!work.standing_down(Stage::Lights));
        assert!(work.standing_down(Stage::Fog));
        assert!(work.standing_down(Stage::Water));
    }
}
