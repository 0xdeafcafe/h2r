//! Native replacement counters share the game/worker frame boundary with D3D.
use crate::phase;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Stage { World, Objects, Transparents, Lights, Post, Particles, Decals, Hud, Gels, Fog, Water, Shadows, FirstPerson, Effects, Ui }

const STAGES: [&str; 15] = ["world", "objects", "transparents", "lights", "post", "particles", "decals", "hud", "gels", "fog", "water", "shadows", "first_person", "effects", "ui"];
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Metric {
    Examined, Suppressed, RefusedOwnership, RefusedResources, RefusedRanges, RefusedMasks, RefusedInputs, RefusedValidation, RefusedReplay,
    PrepareNs, EncodeNs, Buffers, BindGroups, UploadBytes, PipelineMisses,
    WorkerRoundTrips, ReferencePasses, DisplayPasses,
    WalkExamined, WalkSuppressed, WalkRefusedInputs, WalkRefusedReservation,
    BuildsSkipped, WalkSections, EntryPassesSkipped, SectionsPrepared, StateCallsSkipped, CacheHits, CacheMisses,
    /// Committed recordings with a private command buffer (`record_ranges`):
    /// each also ends the open encoder and adds a render pass of its own.
    CommandBuffers,
    /// Game-thread nanoseconds a scene stage spends deciding what the
    /// guest skips: guest reads and every check that can refuse
    /// (docs/graphics/framescene.md, slice 2).
    ExtractNs,
    /// [`Metric::PrepareNs`] charged on the render thread
    /// ([`mark_render_thread`]): `prepare_ns` is the game thread's alone.
    RenderPrepareNs,
    /// Scene stages queued to the render thread.
    SceneStages,
    /// Scene stages the render thread could not record after the guest
    /// was told to skip their items: a named error, not a fallback.
    SceneDropped,
}
const METRICS: [&str; 34] = ["examined", "suppressed", "refused_ownership", "refused_resources", "refused_ranges",
    "refused_masks", "refused_inputs", "refused_validation", "refused_replay", "prepare_ns", "encode_ns", "buffers", "bind_groups", "upload_bytes", "pipeline_misses",
    "worker_round_trips", "reference_passes", "display_passes",
    "walk_examined", "walk_suppressed", "walk_refused_inputs", "walk_refused_reservation",
    "builds_skipped", "walk_sections", "entry_passes_skipped", "sections_prepared", "state_calls_skipped", "cache_hits", "cache_misses",
    "command_buffers", "extract_ns", "render_prepare_ns", "scene_stages", "scene_dropped"];

// Four Xbox player views and a bucket for non-player views.
const VIEWS: usize = 5;
const VIEW_METRICS: [&str; 9] = [
    "reference_ranges", "display_ranges", "invariants", "reference_recorded", "display_recorded",
    "reference_invariants", "display_invariants", "reference_refused", "display_refused",
];
const STRIDE: usize = METRICS.len() + VIEWS * VIEW_METRICS.len();
const STAGE_COLUMNS: usize = STAGES.len() * STRIDE;
const OBJECT_WALK_COLUMNS: [&str; 13] = [
    "walk_native", "walk_fallback", "walk_unsupported", "walk_order", "walk_unreadable",
    "walk_model", "walk_inputs", "walk_state", "walk_reservation",
    "walk_part_aux_state", "walk_part_subparts", "walk_part_topology", "walk_unresident",
];
const WORLD_OPAQUE_COLUMNS: [&str; 11] = [
    "prereserved_draws", "prereserved_sections", "guest_indexed_draws", "guest_indexed_sections",
    "handlers_taken", "handlers_fallback", "handler_draws", "handlers_indexed", "handler_indexed_draws",
    "gpu_draws", "backend_draws",
];
const WORLD_OPAQUE_BASE: usize = STAGE_COLUMNS + OBJECT_WALK_COLUMNS.len();
const WORLD_SECTION_COLUMNS: [&str; 2] = ["taken_sections", "fallback_sections"];
const WORLD_SECTION_BASE: usize = WORLD_OPAQUE_BASE + WORLD_OPAQUE_COLUMNS.len();

/// Count at the front-end boundary, independently of which backend
/// records the GPU draw. Section counts are deduplicated by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldOpaque { Prereserved, GuestIndexed }

impl WorldOpaque {
    pub fn count(self, draws: u64, sections: u64) {
        let offset = match self { Self::Prereserved => 0, Self::GuestIndexed => 2 };
        let start = phase::Counter::ALL.len() + WORLD_OPAQUE_BASE + offset;
        phase::count_index(start, draws);
        phase::count_index(start + 1, sections);
    }
}

/// Section identity at the sorted handler, across layers within one frame.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorldSection {
    Cluster { view: u32, bsp: u32, index: u32 },
    Placement { view: u32, bsp: u32, index: u32 },
}

impl WorldSection {
    pub fn taken(self) {
        phase::world_section(|frame, _| { frame.taken_sections.insert(self); });
    }

    pub fn fallback(self, shader: u32, reason: &'static str) {
        phase::world_section(|frame, details| {
            frame.fallback_sections.insert(self);
            if details {
                frame.sections_by_refusal.entry((shader, reason)).or_default().insert(self);
            }
        });
    }
}

/// Sorted standard geometry handlers, counted only once their disposition
/// is known. A single masked strip handler can submit several ranges.
pub fn world_handler_taken(shader: u32, draws: usize) {
    consumed_handler(shader, draws, None);
}

/// A native sorted handler completed, emitting its ranges through the
/// indexed backend. Its geometry handler was skipped in the guest.
pub fn world_handler_indexed(shader: u32, reason: &str, draws: usize) {
    consumed_handler(shader, draws, Some(reason));
}

fn consumed_handler(shader: u32, draws: usize, indexed: Option<&str>) {
    if !phase::enabled() { return; }
    let start = phase::Counter::ALL.len() + WORLD_OPAQUE_BASE;
    phase::count_index(start + 4, 1);
    phase::count_index(start + 6, draws as u64);
    if indexed.is_some() {
        phase::count_index(start + 7, 1);
        phase::count_index(start + 8, draws as u64);
    }
    phase::world_material(|counts| {
        let material = counts.rows.entry(shader).or_default();
        material.taken += 1;
        material.draws += draws as u64;
        if let Some(reason) = indexed {
            material.indexed_draws += draws as u64;
            count_reason(&mut material.indexed, reason);
        }
    });
}

/// Physical routing of packets produced by the native sorted handler.
pub fn world_packet(shader: u32, fallback: Option<&str>) {
    if !phase::enabled() { return; }
    let offset = if fallback.is_some() { 10 } else { 9 };
    phase::count_index(phase::Counter::ALL.len() + WORLD_OPAQUE_BASE + offset, 1);
    if let Some(reason) = fallback {
        phase::world_material(|counts| count_reason(&mut counts.rows.entry(shader).or_default().backend, reason));
    }
}

fn count_reason(reasons: &mut BTreeMap<String, u64>, reason: &str) {
    if let Some(n) = reasons.get_mut(reason) {
        *n += 1;
    } else {
        reasons.insert(reason.to_owned(), 1);
    }
}

/// Charge the ordinary shader datum once, when its sorted handler goes to the guest.
pub fn world_handler_fallback(shader: u32, reason: &str) {
    if !phase::enabled() { return; }
    phase::count_index(phase::Counter::ALL.len() + WORLD_OPAQUE_BASE + 5, 1);
    phase::world_material(|counts| {
        count_reason(&mut counts.rows.entry(shader).or_default().refused, reason);
    });
}

#[derive(Default)]
pub(crate) struct WorldMaterials {
    rows: BTreeMap<u32, MaterialCount>,
    taken_sections: BTreeSet<WorldSection>,
    fallback_sections: BTreeSet<WorldSection>,
    sections_by_refusal: BTreeMap<(u32, &'static str), BTreeSet<WorldSection>>,
}

#[derive(Default)]
struct MaterialCount {
    taken: u64,
    draws: u64,
    refused: BTreeMap<String, u64>,
    refused_sections: BTreeMap<String, u64>,
    indexed: BTreeMap<String, u64>,
    indexed_draws: u64,
    backend: BTreeMap<String, u64>,
}

impl WorldMaterials {
    pub(crate) const fn new() -> Self {
        Self {
            rows: BTreeMap::new(),
            taken_sections: BTreeSet::new(),
            fallback_sections: BTreeSet::new(),
            sections_by_refusal: BTreeMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool { self.rows.is_empty() }

    /// A worker may finish before its game sample. Union identities until
    /// all contributors have arrived at the same frame boundary.
    pub(crate) fn merge(&mut self, mut other: Self) {
        self.taken_sections.append(&mut other.taken_sections);
        self.fallback_sections.append(&mut other.fallback_sections);
        for (key, mut sections) in other.sections_by_refusal {
            self.sections_by_refusal.entry(key).or_default().append(&mut sections);
        }
        if self.rows.is_empty() {
            self.rows = other.rows;
            return;
        }
        for (shader, src) in other.rows {
            let dst = self.rows.entry(shader).or_default();
            dst.taken += src.taken;
            dst.draws += src.draws;
            dst.indexed_draws += src.indexed_draws;
            for (target, reasons) in [(&mut dst.refused, src.refused), (&mut dst.indexed, src.indexed), (&mut dst.backend, src.backend),
                (&mut dst.refused_sections, src.refused_sections)] {
                for (reason, n) in reasons {
                    *target.entry(reason).or_default() += n;
                }
            }
        }
    }

    /// Fold once before the completed sample goes to CSV, captures, and
    /// totals. The run report sums these per-frame section counts.
    pub(crate) fn finish_sections(&mut self, counts: &mut [u64; phase::COUNTERS]) {
        let at = phase::Counter::ALL.len() + WORLD_SECTION_BASE;
        counts[at] += self.taken_sections.len() as u64;
        counts[at + 1] += self.fallback_sections.len() as u64;
        self.taken_sections.clear();
        self.fallback_sections.clear();
        for ((shader, why), sections) in std::mem::take(&mut self.sections_by_refusal) {
            self.rows.entry(shader).or_default().refused_sections.insert(why.to_owned(), sections.len() as u64);
        }
    }
}

impl std::fmt::Display for WorldMaterials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, (shader, count)) in self.rows.iter().enumerate() {
            if index > 0 { f.write_str("; ")?; }
            write!(f, "{shader:#010x} taken={} draws={}", count.taken, count.draws)?;
            for (why, n) in &count.refused {
                write!(f, " refused[{why:?}]={n}")?;
            }
            for (why, n) in &count.refused_sections {
                write!(f, " refused_sections[{why:?}]={n}")?;
            }
            for (why, n) in &count.backend {
                write!(f, " backend[{why:?}]={n}")?;
            }
            if !count.indexed.is_empty() {
                write!(f, " indexed_draws={}", count.indexed_draws)?;
                for (why, n) in &count.indexed {
                    write!(f, " indexed[{why:?}]={n}")?;
                }
            }
        }
        Ok(())
    }
}

const SCENE_LIST_START: usize = WORLD_SECTION_BASE + WORLD_SECTION_COLUMNS.len();
const SCENE_LISTS: [&str; 14] = [
    "view_objects", "light_objects", "object_order", "object_setup", "object_record",
    "view_geometry", "light_geometry_gather", "visible_lights", "light_geometry",
    "light_extra_caster", "transparent_effect", "transparent_polygon",
    "transparent_geometry", "transparents",
];
const SCENE_LIST_RESULTS: [&str; 11] = [
    "native", "fallback", "memory", "layout", "unsupported", "residency", "lighting",
    "palette", "object_model", "compared", "mismatch",
];


#[derive(Clone, Copy)]
#[repr(usize)]
pub enum SceneList {
    ViewObjects,
    LightObjects,
    ObjectOrder,
    ObjectSetup,
    ObjectRecord,
    ViewGeometry,
    LightGeometryGather,
    VisibleLights,
    LightGeometry,
    LightExtraCaster,
    TransparentEffect,
    TransparentPolygon,
    TransparentGeometry,
    Transparents,
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum SceneListResult {
    Native,
    Fallback,
    Memory,
    Layout,
    Unsupported,
    Residency,
    Lighting,
    Palette,
    ObjectModel,
    Compared,
    Mismatch,
}

impl SceneList {
    pub fn count(self, result: SceneListResult) {
        phase::count_index(
            phase::Counter::ALL.len() + SCENE_LIST_START + self as usize * SCENE_LIST_RESULTS.len()
                + result as usize,
            1,
        );
    }
}

const UI_COLUMNS: [&str; Ui::OutlineReservation as usize + 1] = [
    "items", "item_refused", "fallback_geometry", "fallback_order", "fallback_target",
    "fallback_state", "fallback_shader", "fallback_texture", "fallback_upload",
    "fallback_reservation", "guest_builder_draw", "model_guest_builder_draw",
    "backdrop_native", "backdrop_fallback_entry",
    "backdrop_fallback_inputs", "backdrop_fallback_reservation", "model_idle",
    "model_native_group", "model_fallback_widget", "model_fallback_entry",
    "model_fallback_camera", "model_fallback_caller", "model_fallback_geometry",
    "model_fallback_range_limit", "model_fallback_texture_commit", "model_fallback_reservation",
    "outline_native", "outline_fallback_entry", "outline_fallback_inputs", "outline_fallback_reservation",
];
const UI_OFFSET: usize = SCENE_LIST_START + SCENE_LISTS.len() * SCENE_LIST_RESULTS.len();
pub(crate) const COLUMNS: usize = UI_OFFSET + UI_COLUMNS.len();

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Ui {
    Items,
    ItemRefused,
    Geometry,
    Order,
    Target,
    State,
    Shader,
    Texture,
    Upload,
    Reservation,
    GuestBuilderDraw,
    ModelGuestBuilderDraw,
    BackdropNative,
    BackdropEntry,
    BackdropInputs,
    BackdropReservation,
    ModelIdle,
    ModelNativeGroup,
    ModelWidgetFallback,
    ModelEntry,
    ModelCamera,
    ModelCaller,
    ModelGeometry,
    ModelRangeLimit,
    ModelTextureCommit,
    ModelReservation,
    OutlineNative,
    OutlineEntry,
    OutlineInputs,
    OutlineReservation,
}

impl Ui {
    pub fn count(self) {
        phase::count_index(phase::Counter::ALL.len() + UI_OFFSET + self as usize, 1);
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum ObjectWalk {
    Native,
    Fallback,
    Unsupported,
    Order,
    Unreadable,
    Model,
    Inputs,
    State,
    Reservation,
    /// Parts (not objects) the walk refused by a named cause of
    /// `walk_unsupported` (`scene_extract::layout::object_walk::Refusal`):
    /// the record's depth-scale toggle, its visible-subpart ranges, a
    /// strip part drawn as a list. Counted with [`ObjectWalk::count`].
    PartAuxState,
    PartSubparts,
    PartTopology,
    /// Objects returned to the guest in a run whose validated native model
    /// had no uploaded GPU model. The run has not been consumed.
    Unresident,
}

impl ObjectWalk {
    pub fn count(self, n: usize) {
        phase::count_index(phase::Counter::ALL.len() + STAGE_COLUMNS + self as usize, n as u64);
    }

    pub fn fallback(self, n: usize) {
        Self::Fallback.count(n);
        self.count(n);
    }
}
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
        out.extend(OBJECT_WALK_COLUMNS.map(|name| format!("native_objects_{name}")));
        out.extend(WORLD_OPAQUE_COLUMNS.map(|name| format!("native_world_opaque_{name}")));
        out.extend(WORLD_SECTION_COLUMNS.map(|name| format!("native_world_opaque_{name}")));
        for stage in SCENE_LISTS {
            for result in SCENE_LIST_RESULTS {
                out.push(format!("native_scene_lists_{stage}_{result}"));
            }
        }
        out.extend(UI_COLUMNS.map(|name| format!("native_ui_{name}")));
        out
    })
}

impl Stage {
    /// The stage's name in its counters (`native_<name>_...`).
    pub fn name(self) -> &'static str {
        STAGES[self as usize]
    }

    pub fn count(self, metric: Metric, n: u64) {
        phase::count_index(phase::Counter::ALL.len() + self as usize * STRIDE + charged(metric) as usize, n);
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
    static RENDER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the calling thread as the render thread (`d3d`'s
/// `ThreadedBackend` worker): [`Metric::PrepareNs`] counted on it is
/// charged as [`Metric::RenderPrepareNs`].
pub fn mark_render_thread() {
    RENDER_THREAD.with(|r| r.set(true));
}

/// The metric `metric` is charged as on this thread: preparation on the
/// render thread is not the game thread's.
fn charged(metric: Metric) -> Metric {
    match metric {
        Metric::PrepareNs if on_render_thread() => Metric::RenderPrepareNs,
        m => m,
    }
}

/// Whether [`mark_render_thread`] was called on this thread.
pub fn on_render_thread() -> bool {
    RENDER_THREAD.with(std::cell::Cell::get)
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
        let lists = phase::Counter::ALL.len() + SCENE_LIST_START;
        assert_eq!(columns[lists].1, "native_scene_lists_view_objects_native");
        let ui = phase::Counter::ALL.len() + UI_OFFSET;
        assert_eq!(ui, lists + SCENE_LISTS.len() * SCENE_LIST_RESULTS.len());
        assert_eq!(columns[ui - 1].1, "native_scene_lists_transparents_mismatch");
        assert_eq!(columns[ui].1, "native_ui_items");
        assert_eq!(columns.last().unwrap().1, "native_ui_outline_fallback_reservation");
        let walk = phase::Counter::ALL.len() + STAGE_COLUMNS;
        assert_eq!(columns[walk + ObjectWalk::Native as usize].1, "native_objects_walk_native");
        assert_eq!(columns[walk + ObjectWalk::Reservation as usize].1, "native_objects_walk_reservation");
        assert_eq!(columns[walk + ObjectWalk::PartAuxState as usize].1, "native_objects_walk_part_aux_state");
        assert_eq!(columns[walk + ObjectWalk::PartTopology as usize].1, "native_objects_walk_part_topology");
        assert_eq!(columns[walk + ObjectWalk::Unresident as usize].1, "native_objects_walk_unresident");
        assert_eq!(columns[walk + ObjectWalk::Unresident as usize + 1].1, "native_world_opaque_prereserved_draws");
        let world = phase::Counter::ALL.len() + Stage::World as usize * STRIDE;
        for (metric, label) in [(Metric::ExtractNs, "native_world_extract_ns"),
            (Metric::RenderPrepareNs, "native_world_render_prepare_ns"), (Metric::SceneStages, "native_world_scene_stages"),
            (Metric::SceneDropped, "native_world_scene_dropped")] {
            assert_eq!(columns[world + metric as usize].1, label);
        }
    }

    #[test]
    fn preparation_on_the_render_thread_is_charged_apart_from_the_game_thread() {
        assert!(matches!(charged(Metric::PrepareNs), Metric::PrepareNs));
        std::thread::spawn(|| {
            mark_render_thread();
            assert!(on_render_thread());
            assert!(matches!(charged(Metric::PrepareNs), Metric::RenderPrepareNs));
            assert!(matches!(charged(Metric::EncodeNs), Metric::EncodeNs));
        })
        .join()
        .unwrap();
        assert!(!on_render_thread(), "the mark is the marked thread's alone");
    }
}
