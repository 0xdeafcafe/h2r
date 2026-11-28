//! Native replacement counters share the game/worker frame boundary with D3D.
use crate::phase;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Stage {
    World,
    Objects,
    Transparents,
    Lights,
    Post,
    Particles,
    Decals,
    Hud,
    Gels,
    Fog,
    Water,
    Shadows,
    FirstPerson,
    Effects,
    Ui,
    Flares,
    Sky,
}

const STAGES: [&str; Stage::Sky as usize + 1] = [
    "world",
    "objects",
    "transparents",
    "lights",
    "post",
    "particles",
    "decals",
    "hud",
    "gels",
    "fog",
    "water",
    "shadows",
    "first_person",
    "effects",
    "ui",
    "flares",
    "sky",
];
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Metric {
    Examined,
    Suppressed,
    RefusedOwnership,
    RefusedResources,
    RefusedRanges,
    RefusedMasks,
    RefusedInputs,
    RefusedValidation,
    RefusedReplay,
    PrepareNs,
    EncodeNs,
    Buffers,
    BindGroups,
    UploadBytes,
    PipelineMisses,
    WorkerRoundTrips,
    ReferencePasses,
    DisplayPasses,
    WalkExamined,
    WalkSuppressed,
    WalkRefusedInputs,
    WalkRefusedReservation,
    BuildsSkipped,
    WalkSections,
    EntryPassesSkipped,
    SectionsPrepared,
    StateCallsSkipped,
    CacheHits,
    CacheMisses,
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
    /// A sorted range whose end the guest cannot be resumed at: the image
    /// has no resume point there. Not a reservation refusal.
    WalkRefusedResume,
    /// The five causes `walk_refused_reservation` splits into
    /// ([`ReservationCause`]), counted with [`Stage::reservation_cause`].
    WalkRefusedReservationDeclined,
    WalkRefusedReservationError,
    WalkRefusedReservationStale,
    WalkRefusedReservationConsumed,
    WalkRefusedReservationUnbound,
    WalkRefusedReservationBoundary,
    WalkRefusedReservationGeometry,
    WalkRefusedReservationAdmission,
    WalkRefusedReservationBinding,
    WalkRefusedReservationResources,
    WalkRefusedReservationSink,
    WalkRefusedReservationBackend,
}
const METRICS: [&str; 47] = [
    "examined",
    "suppressed",
    "refused_ownership",
    "refused_resources",
    "refused_ranges",
    "refused_masks",
    "refused_inputs",
    "refused_validation",
    "refused_replay",
    "prepare_ns",
    "encode_ns",
    "buffers",
    "bind_groups",
    "upload_bytes",
    "pipeline_misses",
    "worker_round_trips",
    "reference_passes",
    "display_passes",
    "walk_examined",
    "walk_suppressed",
    "walk_refused_inputs",
    "walk_refused_reservation",
    "builds_skipped",
    "walk_sections",
    "entry_passes_skipped",
    "sections_prepared",
    "state_calls_skipped",
    "cache_hits",
    "cache_misses",
    "command_buffers",
    "extract_ns",
    "render_prepare_ns",
    "scene_stages",
    "scene_dropped",
    "walk_refused_resume",
    "walk_refused_reservation_declined",
    "walk_refused_reservation_error",
    "walk_refused_reservation_stale",
    "walk_refused_reservation_consumed",
    "walk_refused_reservation_unbound",
    "walk_refused_reservation_boundary",
    "walk_refused_reservation_geometry",
    "walk_refused_reservation_admission",
    "walk_refused_reservation_binding",
    "walk_refused_reservation_resources",
    "walk_refused_reservation_sink",
    "walk_refused_reservation_backend",
];

/// Why a native walk's reservation did not commit. Each cause has its own
/// column under every stage; the stage's `walk_refused_reservation` (or the
/// objects walk's `walk_reservation`) is their sum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationCause {
    /// The device or the scene sink does not take the pass: the boundary is
    /// not drained, the sink will not keep it, or no host is installed.
    Declined,
    /// The device returned an error, or its lock was poisoned.
    Error,
    /// The load the run was planned against is no longer the current one.
    Stale,
    /// The guest loop's cursor no longer matched at the commit.
    Consumed,
    /// The device could not bind the pass's targets or pipeline.
    Unbound,
    /// A per-range draw (`d3d::NativeRefusal`): the device is not at a
    /// producer boundary.
    Boundary,
    /// The index range's geometry did not resolve.
    Geometry,
    /// The producer's admission returned no pass for the draw.
    Admission,
    /// The pass's targets do not bind as the guest draw's.
    Binding,
    /// A sampled texture or independent samples are not available.
    Resources,
    /// The scene sink would not keep the pass.
    Sink,
    /// The backend did not record every copy, or the guest cursor moved.
    Backend,
}

impl ReservationCause {
    pub const ALL: [ReservationCause; 12] = [
        Self::Declined, Self::Error, Self::Stale, Self::Consumed, Self::Unbound,
        Self::Boundary, Self::Geometry, Self::Admission, Self::Binding,
        Self::Resources, Self::Sink, Self::Backend,
    ];

    /// The column's name suffix.
    pub fn name(self) -> &'static str {
        match self {
            Self::Declined => "declined",
            Self::Error => "error",
            Self::Stale => "stale",
            Self::Consumed => "consumed",
            Self::Unbound => "unbound",
            Self::Boundary => "boundary",
            Self::Geometry => "geometry",
            Self::Admission => "admission",
            Self::Binding => "binding",
            Self::Resources => "resources",
            Self::Sink => "sink",
            Self::Backend => "backend",
        }
    }

    /// The column this cause counts in.
    pub fn metric(self) -> Metric {
        match self {
            Self::Declined => Metric::WalkRefusedReservationDeclined,
            Self::Error => Metric::WalkRefusedReservationError,
            Self::Stale => Metric::WalkRefusedReservationStale,
            Self::Consumed => Metric::WalkRefusedReservationConsumed,
            Self::Unbound => Metric::WalkRefusedReservationUnbound,
            Self::Boundary => Metric::WalkRefusedReservationBoundary,
            Self::Geometry => Metric::WalkRefusedReservationGeometry,
            Self::Admission => Metric::WalkRefusedReservationAdmission,
            Self::Binding => Metric::WalkRefusedReservationBinding,
            Self::Resources => Metric::WalkRefusedReservationResources,
            Self::Sink => Metric::WalkRefusedReservationSink,
            Self::Backend => Metric::WalkRefusedReservationBackend,
        }
    }
}

// Four Xbox player views and a bucket for non-player views.
const VIEWS: usize = 5;
const VIEW_METRICS: [&str; 9] = [
    "reference_ranges",
    "display_ranges",
    "invariants",
    "reference_recorded",
    "display_recorded",
    "reference_invariants",
    "display_invariants",
    "reference_refused",
    "display_refused",
];
const STRIDE: usize = METRICS.len() + VIEWS * VIEW_METRICS.len();
const STAGE_COLUMNS: usize = STAGES.len() * STRIDE;
const OBJECT_WALK_COLUMNS: [&str; 16] = [
    "walk_native",
    "walk_fallback",
    "walk_unsupported",
    "walk_order",
    "walk_unreadable",
    "walk_model",
    "walk_inputs",
    "walk_state",
    "walk_reservation",
    "walk_part_aux_state",
    "walk_part_subparts",
    "walk_part_shader_flag",
    "walk_part_argument",
    "walk_part_program",
    "walk_part_layout",
    "walk_unresident",
];
const WORLD_OPAQUE_COLUMNS: [&str; 11] = [
    "prereserved_draws",
    "prereserved_sections",
    "guest_indexed_draws",
    "guest_indexed_sections",
    "handlers_taken",
    "handlers_fallback",
    "handler_draws",
    "handlers_indexed",
    "handler_indexed_draws",
    "gpu_draws",
    "backend_draws",
];
const WORLD_OPAQUE_BASE: usize = STAGE_COLUMNS + OBJECT_WALK_COLUMNS.len();
const WORLD_SECTION_COLUMNS: [&str; 2] = ["taken_sections", "fallback_sections"];
const WORLD_SECTION_BASE: usize = WORLD_OPAQUE_BASE + WORLD_OPAQUE_COLUMNS.len();

/// Count at the front-end boundary, independently of which backend
/// records the GPU draw. Section counts are deduplicated by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldOpaque {
    Prereserved,
    GuestIndexed,
}

impl WorldOpaque {
    pub fn count(self, draws: u64, sections: u64) {
        let offset = match self {
            Self::Prereserved => 0,
            Self::GuestIndexed => 2,
        };
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
        phase::world_section(|frame, _| {
            frame.taken_sections.insert(self);
        });
    }

    pub fn fallback(self, shader: u32, reason: &'static str) {
        phase::world_section(|frame, details| {
            frame.fallback_sections.insert(self);
            if details {
                frame
                    .sections_by_refusal
                    .entry((shader, reason))
                    .or_default()
                    .insert(self);
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
    if !phase::enabled() {
        return;
    }
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
    if !phase::enabled() {
        return;
    }
    let offset = if fallback.is_some() { 10 } else { 9 };
    phase::count_index(phase::Counter::ALL.len() + WORLD_OPAQUE_BASE + offset, 1);
    if let Some(reason) = fallback {
        phase::world_material(|counts| {
            count_reason(&mut counts.rows.entry(shader).or_default().backend, reason)
        });
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
    if !phase::enabled() {
        return;
    }
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

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// A worker may finish before its game sample. Union identities until
    /// all contributors have arrived at the same frame boundary.
    pub(crate) fn merge(&mut self, mut other: Self) {
        self.taken_sections.append(&mut other.taken_sections);
        self.fallback_sections.append(&mut other.fallback_sections);
        for (key, mut sections) in other.sections_by_refusal {
            self.sections_by_refusal
                .entry(key)
                .or_default()
                .append(&mut sections);
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
            for (target, reasons) in [
                (&mut dst.refused, src.refused),
                (&mut dst.indexed, src.indexed),
                (&mut dst.backend, src.backend),
                (&mut dst.refused_sections, src.refused_sections),
            ] {
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
            self.rows
                .entry(shader)
                .or_default()
                .refused_sections
                .insert(why.to_owned(), sections.len() as u64);
        }
    }
}

impl std::fmt::Display for WorldMaterials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, (shader, count)) in self.rows.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(
                f,
                "{shader:#010x} taken={} draws={}",
                count.taken, count.draws
            )?;
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
const SCENE_LISTS: &[&str] = &[
    "view_objects",
    "light_objects",
    "object_order",
    "object_setup",
    "object_record",
    "view_geometry",
    "light_geometry_gather",
    "visible_lights",
    "light_geometry",
    "light_extra_caster",
    "transparent_effect",
    "transparent_polygon",
    "transparent_geometry",
    "transparents",
    "light_frame",
    "light_volume",
    "light_records",
    "light_items",
    "light_handoff",
    "light_passes",
    "first_person_lights",
    "first_person_records",
    "object_shadow_list",
    "object_shadow_rows",
    "object_shadow_caster",
    "light_additional_casters",
    "typed_light_passes",
    "typed_first_person",
    "portal_walk",
    "portal_window_merge",
    "pvs_regions",
    "portal_regions",
    "sphere_regions",
    "window_record",
    "view_projection",
];
const SCENE_LIST_RESULTS: [&str; 12] = [
    "native",
    "fallback",
    "memory",
    "layout",
    "unsupported",
    "residency",
    "lighting",
    "palette",
    "object_model",
    "compared",
    "mismatch",
    "dropped",
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
    /// One call of `0x00031c80` by the light frame body
    /// (docs/graphics/native-light-gather.md, slice L2).
    LightFrame,
    /// One light's volume (`0x000c17f0`) in the frame body.
    LightVolume,
    /// One of a light's record walks (`0x00133390`, `0x00133520`) in the
    /// frame body (slice L3).
    LightRecords,
    /// One record of a light's item lists (`0x00040e90` in the light's
    /// kind, slice L4), or a call the body declines.
    LightItems,
    /// One light the frame body hands to the scene at its passes (slice
    /// L5): native when the scene's light is made from the hand-off and
    /// the light's item lists, the fallback and its reason when the scene
    /// reads it at the passes' entry; `compared` and `mismatch` check the
    /// hand-off against guest memory after the lists are made.
    LightHandoff,
    /// One light's passes (`0x00031170`) by the passes body (slice L5).
    LightPasses,
    /// One call of the first-person lights (`0x000404b0`) by their body
    /// (slice L6).
    FirstPersonLights,
    /// One record of a first-person light's slot that passes the mode's
    /// filter (`0x000402b0`, slice L6), or a call the body declines.
    FirstPersonRecords,
    /// Calls of the object-shadow outer loop, its row gather and its caster.
    ObjectShadowList,
    ObjectShadowRows,
    ObjectShadowCaster,
    LightAdditionalCasters,
    /// One whole dynamic pass after its item lists are made (slice L8).
    TypedLightPasses,
    /// One first-person slot after its records are replayed.
    TypedFirstPerson,
    PortalWalk,
    PortalWindowMerge,
    PvsRegions,
    PortalRegions,
    SphereRegions,
    WindowRecord,
    /// One call of the view projection builder `0x0002fb70`.
    ViewProjection,
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
    /// A cut producer that could not build its output: the call is
    /// answered by the "not drawn" exit and counted here under its cause
    /// (`Memory`, `Layout`, ...), never handed to the guest body.
    Dropped,
}

impl SceneList {
    pub fn count(self, result: SceneListResult) {
        phase::count_index(
            phase::Counter::ALL.len()
                + SCENE_LIST_START
                + self as usize * SCENE_LIST_RESULTS.len()
                + result as usize,
            1,
        );
    }
}

const UI_COLUMNS: [&str; Ui::OverlayMenuRevealUnwritten as usize + 1] = [
    "items",
    "item_refused",
    "fallback_geometry",
    "fallback_order",
    "fallback_target",
    "fallback_state",
    "fallback_shader",
    "fallback_texture",
    "fallback_upload",
    "fallback_reservation",
    "guest_builder_draw",
    "model_guest_builder_draw",
    "backdrop_native",
    "backdrop_fallback_entry",
    "backdrop_fallback_inputs",
    "backdrop_fallback_reservation",
    "model_idle",
    "model_native_group",
    "model_fallback_widget",
    "model_fallback_entry",
    "model_fallback_camera",
    "model_fallback_caller",
    "model_fallback_geometry",
    "model_fallback_range_limit",
    "model_fallback_texture_commit",
    "model_fallback_reservation",
    "outline_native",
    "outline_fallback_entry",
    "outline_fallback_inputs",
    "outline_fallback_reservation",
    "overlay_items",
    "overlay_quads",
    "overlay_text_runs",
    "overlay_fills",
    "overlay_glyphs",
    "overlay_batches",
    "overlay_refused_material",
    "overlay_refused_clip",
    "overlay_refused_glyph",
    "overlay_refused_texture",
    "overlay_blurs",
    "overlay_model_views",
    "overlay_refused_model_view",
    "overlay_refused_model_view_camera",
    "overlay_refused_blur",
    "overlay_hud_tag_quads",
    "overlay_hud_tag_fallback",
    "overlay_hud_tag_recorded",
    "overlay_hud_tag_record_failed",
    "overlay_hud_text_quads",
    "overlay_hud_text_fallback",
    "overlay_menu_shader_quads",
    "overlay_menu_shader_fallback",
    "overlay_fade_quads",
    "overlay_bar_quads",
    "overlay_loading_quads",
    "overlay_widget_quads",
    "overlay_screen_fallback",
    "overlay_menu_claimed",
    "overlay_menu_skipped",
    "overlay_menu_passes",
    "overlay_menu_mismatch",
    "overlay_menu_refused_frame",
    "overlay_menu_refused_mismatched",
    "overlay_menu_refused_bitmap_state",
    "overlay_menu_refused_text",
    "overlay_menu_refused_model",
    "overlay_menu_refused_draw",
    "overlay_menu_refused_unreadable",
    "overlay_menu_refused_unlisted",
    "overlay_menu_refused_pass",
    "overlay_modal_blurs",
    "overlay_modal_blur_replayed",
    "overlay_modal_blur_refused_pass",
    "overlay_modal_blur_refused_arguments",
    "overlay_modal_blur_refused_steps",
    "overlay_modal_blur_refused_combiner",
    "overlay_modal_blur_refused_targets",
    "overlay_hud_rust_drawn",
    "overlay_hud_rust_matched",
    "overlay_hud_rust_proven",
    "overlay_hud_rust_mismatch_words",
    "overlay_hud_rust_mismatch_state",
    "overlay_hud_rust_mismatch_stages",
    "overlay_hud_rust_mismatch_corners",
    "overlay_hud_rust_refused",
    "overlay_hud_rust_recheck_failed",
    "overlay_movie_frames",
    "overlay_refused_movie",
    "movie_placed",
    "movie_refused_record",
    "movie_refused_frame",
    "movie_refused_picture",
    "movie_refused_host",
    "movie_refused_pass",
    "model_view_rust_matched",
    "model_view_rust_mismatch_objects",
    "model_view_rust_mismatch_camera",
    "model_view_rust_refused",
    "model_view_rust_fallback",
    "overlay_menu_reveal_unwritten",
];
const UI_OFFSET: usize = SCENE_LIST_START + SCENE_LISTS.len() * SCENE_LIST_RESULTS.len();
const ANIMATION_OFFSET: usize = UI_OFFSET + UI_COLUMNS.len();
const ANIMATION_COLUMNS: [&str; ShaderAnimation::Layer3Refused as usize + 1] = [
    "layers_drawn",
    "layers_refused",
    "objects_drawn",
    "objects_refused",
    "layer3_drawn",
    "layer3_refused",
];
pub(crate) const COLUMNS: usize = ANIMATION_OFFSET + ANIMATION_COLUMNS.len();

/// Material passes with animated constants (`shad +0x3c`, sampled at the
/// render clock, docs/formats/shader-instances.md "Animated constants"),
/// by the native path that planned them with their sampled registers or
/// refused them: the structure's layer planner (`layers`), the object
/// passes of layer 3 and the world layers (`objects`), and the world's
/// layer 3 (`layer3`, per selected material).
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum ShaderAnimation {
    LayersDrawn,
    LayersRefused,
    ObjectsDrawn,
    ObjectsRefused,
    Layer3Drawn,
    Layer3Refused,
}

impl ShaderAnimation {
    pub fn count(self, n: u64) {
        phase::count_index(phase::Counter::ALL.len() + ANIMATION_OFFSET + self as usize, n);
    }
}

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
    /// The overlay stage (docs/graphics/retire-d3d.md, slice R6): items in
    /// the scene's list by kind, and its glyphs.
    OverlayItems,
    OverlayQuads,
    OverlayTextRuns,
    OverlayFills,
    OverlayGlyphs,
    /// Draws `native_render::ui_gpu::OverlayGpu` recorded.
    OverlayBatches,
    /// Items dropped, by reason (`native_render::overlay::OverlayStats`):
    /// there is no guest draw behind them.
    OverlayRefusedMaterial,
    OverlayRefusedClip,
    OverlayRefusedGlyph,
    OverlayRefusedTexture,
    /// The menus' items (slice R7): blurs under a dialog and model
    /// widgets' views.
    OverlayBlurs,
    OverlayModelViews,
    /// Model views planned with their camera: no stage draws one yet.
    OverlayRefusedModelView,
    /// Model views with no camera or scissor
    /// (`native_render::model_view::Refusal`).
    OverlayRefusedModelViewCamera,
    /// Blurs over a target the GPU cannot copy or sample.
    OverlayRefusedBlur,
    OverlayHudTagQuads,
    OverlayHudTagFallback,
    /// Display-copy overlay recordings (bitmap and glyph).
    OverlayHudTagRecorded,
    /// Failed overlay recordings, per attempted copy.
    OverlayHudTagRecordFailed,
    /// Glyph quads admitted with the text widget's evaluated pixel shader.
    OverlayHudTextQuads,
    OverlayHudTextFallback,
    /// Menu shader bitmaps' quads drawn from the overlay with their tag
    /// shader's combiner, and those the UI fan path kept.
    OverlayMenuShaderQuads,
    OverlayMenuShaderFallback,
    /// The fade's, the bars' and the loading screen's draws drawn from the
    /// overlay with the guest's combiner and blend at the draw, and those
    /// the UI fan path kept.
    OverlayFadeQuads,
    OverlayBarQuads,
    OverlayLoadingQuads,
    OverlayWidgetQuads,
    OverlayScreenFallback,
    /// Menu widget draws the overlay claimed at their sorted list
    /// (docs/graphics/native-post-hud.md, "The menus on the overlay").
    OverlayMenuClaimed,
    /// The game's widget draws left out because the overlay drew them.
    OverlayMenuSkipped,
    /// Overlay passes placed for runs of claimed widgets with items.
    OverlayMenuPasses,
    /// Lists whose left-out draws did not match their claim; that screen
    /// is not claimed again on the map.
    OverlayMenuMismatch,
    /// Lists the game draws whole: the screen quads' viewport not the
    /// full screen; a screen that mismatched before.
    OverlayMenuRefusedFrame,
    OverlayMenuRefusedMismatched,
    /// Drawn bitmap widgets the game keeps: one whose state the overlay
    /// does not draw as the game does (the debug labels on, an image set
    /// from elsewhere, a texture not resident, a tint constant not white,
    /// a meter's bitmap with no second image).
    OverlayMenuRefusedBitmapState,
    /// Drawn widgets the game keeps, by the draw not read yet: text, a
    /// model scene, another draw (a list's, a button's); one that did not
    /// read; one the overlay refused.
    OverlayMenuRefusedText,
    OverlayMenuRefusedModel,
    OverlayMenuRefusedDraw,
    OverlayMenuRefusedUnreadable,
    OverlayMenuRefusedUnlisted,
    /// Lists or runs whose pass the host did not place.
    OverlayMenuRefusedPass,
    /// The blur under a dialog (docs/graphics/native-post-hud.md, "The
    /// blur under a dialog"): calls whose draws the overlay's blur
    /// replaced; calls armed whose draws were given back (a draw off the
    /// plan, a pass that did not record); calls the host did not arm.
    OverlayModalBlurs,
    OverlayModalBlurReplayed,
    OverlayModalBlurRefusedPass,
    /// Calls the game draws, by what did not read: a word not the one
    /// `0x00235756` pushes, endless steps, a combiner the blur step does
    /// not draw, a draw outside targets 0 and 1.
    OverlayModalBlurRefusedArguments,
    OverlayModalBlurRefusedSteps,
    OverlayModalBlurRefusedCombiner,
    OverlayModalBlurRefusedTargets,
    /// The HUD's widget quads with their material read in Rust
    /// (docs/graphics/native-hud-overlay.md, "The material in Rust"):
    /// quads drawn with no guest shader setup; captured quads whose Rust
    /// material matched the guest's, and shaders proven by them; captured
    /// quads that did not match, by the first part that differed (the
    /// combiner words, the blend and write state, the stages and
    /// samplers, the corners); quads whose shader the material refused;
    /// proven shaders a later check found different.
    OverlayHudRustDrawn,
    OverlayHudRustMatched,
    OverlayHudRustProven,
    OverlayHudRustMismatchWords,
    OverlayHudRustMismatchState,
    OverlayHudRustMismatchStages,
    OverlayHudRustMismatchCorners,
    OverlayHudRustRefused,
    OverlayHudRustRecheckFailed,
    /// Movie frames (docs/graphics/retire-d3d-display.md, "Movies"): items
    /// the overlay drew over their movie's upload, and those it refused
    /// for want of one.
    OverlayMovieFrames,
    OverlayRefusedMovie,
    /// Movie draws `0x00156960` placed on the overlay in place of the
    /// game's quad; those refused, by reason: the movie's record or
    /// rectangle does not read as the overlay draws it, the screen quads'
    /// viewport is not the full screen, no decoded picture, no host to
    /// place it, the host did not place the pass. A refused draw shows
    /// nothing: the game's texture is never filled.
    MoviePlaced,
    MovieRefusedRecord,
    MovieRefusedFrame,
    MovieRefusedPicture,
    MovieRefusedHost,
    MovieRefusedPass,
    /// Active model widgets' views read in Rust at the item call
    /// (docs/graphics/native-ui-producers.md, "The model view in Rust"), compared
    /// with the game's at its camera call: views that matched; views that
    /// differed, by the first part (the objects, the camera and viewport),
    /// after which the session takes the game's; views the Rust read
    /// refused (lights, no live object, a camera that does not read);
    /// views taken from the game's read after a mismatch.
    ModelViewRustMatched,
    ModelViewRustMismatchObjects,
    ModelViewRustMismatchCamera,
    ModelViewRustRefused,
    ModelViewRustFallback,
    /// Left-out text draws whose reveal cursor step could not be read or
    /// written (docs/graphics/native-menus-overlay.md, "What the list
    /// reads").
    OverlayMenuRevealUnwritten,
}

impl Ui {
    pub fn count(self) {
        self.count_n(1);
    }

    pub fn count_n(self, n: u64) {
        phase::count_index(phase::Counter::ALL.len() + UI_OFFSET + self as usize, n);
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
    /// the record's depth-scale toggle and its visible-subpart ranges,
    /// whose inputs do not read. Counted with [`ObjectWalk::count`].
    PartAuxState,
    PartSubparts,
    /// Parts refused for a flagged shader on a record with a structure
    /// lightmap index, for an item's other pass, level or part, for a
    /// program with flag bit 20, and for any other cause (the item's
    /// layout, a check against the guest's choice, memory that does not
    /// read).
    PartShaderFlag,
    PartArgument,
    PartProgram,
    PartLayout,
    /// Objects returned to the guest in a run whose validated native model
    /// had no uploaded GPU model. The run has not been consumed.
    Unresident,
}

impl ObjectWalk {
    pub fn count(self, n: usize) {
        phase::count_index(
            phase::Counter::ALL.len() + STAGE_COLUMNS + self as usize,
            n as u64,
        );
    }

    pub fn fallback(self, n: usize) {
        Self::Fallback.count(n);
        self.count(n);
    }

    /// The cause's name: its column without `walk_` (`state`, `order`).
    pub fn name(self) -> &'static str {
        OBJECT_WALK_COLUMNS[self as usize].trim_start_matches("walk_")
    }

}
pub(crate) fn labels() -> &'static [String] {
    static LABELS: OnceLock<Vec<String>> = OnceLock::new();
    LABELS.get_or_init(|| {
        let mut out = Vec::with_capacity(COLUMNS);
        for stage in STAGES {
            for name in METRICS {
                out.push(format!("native_{stage}_{name}"));
            }
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
        out.extend(ANIMATION_COLUMNS.map(|name| format!("native_shader_animation_{name}")));
        out
    })
}

impl Stage {
    /// Number of stages with timeline counter columns.
    pub const COUNT: usize = STAGES.len();

    /// The stage's name in its counters (`native_<name>_...`).
    pub fn name(self) -> &'static str {
        STAGES[self as usize]
    }

    /// Count one refused reservation under its cause. The caller counts the
    /// sum (`walk_refused_reservation`) where it does today.
    pub fn reservation_cause(self, cause: ReservationCause) {
        self.count(cause.metric(), 1);
    }

    pub fn count(self, metric: Metric, n: u64) {
        phase::count_index(
            phase::Counter::ALL.len() + self as usize * STRIDE + charged(metric) as usize,
            n,
        );
    }

    fn view_index(self, view: u32) -> usize {
        phase::Counter::ALL.len()
            + self as usize * STRIDE
            + METRICS.len()
            + (view as usize).min(VIEWS - 1) * VIEW_METRICS.len()
    }

    pub fn recorded(self, view: u32, display: bool, ranges: usize) {
        phase::count_index(self.view_index(view) + usize::from(display), ranges as u64);
    }

    pub fn attempted(self, view: u32, display: bool, ranges: usize) {
        phase::count_index(
            self.view_index(view) + 3 + usize::from(display),
            ranges as u64,
        );
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
    if phase::enabled()
        && let Some(stage) = ACTIVE.with(std::cell::Cell::get)
    {
        stage.count(metric, n);
    }
}

pub struct Timer(Option<(Stage, Metric, std::time::Instant, Option<Stage>)>);

impl Timer {
    pub fn start(stage: Stage, metric: Metric) -> Self {
        Self(phase::enabled().then(|| {
            (
                stage,
                metric,
                std::time::Instant::now(),
                ACTIVE.with(|a| a.replace(Some(stage))),
            )
        }))
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
    fn each_reservation_cause_counts_its_own_column() {
        let columns: Vec<_> = phase::counter_columns().collect();
        let mut seen = std::collections::BTreeSet::new();
        for stage in [Stage::Objects, Stage::Transparents] {
            for cause in ReservationCause::ALL {
                let name = cause.name();
                // The column `Stage::reservation_cause` counts into.
                let index = phase::Counter::ALL.len() + stage as usize * STRIDE + cause.metric() as usize;
                let label = format!("native_{}_walk_refused_reservation_{name}", stage.name());
                assert_eq!(columns[index].1, label);
                assert!(seen.insert(index), "{label} shares a column");
                assert_ne!(cause.metric() as usize, Metric::WalkRefusedReservation as usize);
            }
        }
        assert_eq!(seen.len(), 2 * ReservationCause::ALL.len());
    }

    #[test]
    fn capture_columns_identify_each_view_and_copy() {
        let columns: Vec<_> = phase::counter_columns().collect();
        assert_eq!(columns.len(), phase::COUNTERS);
        for (i, (slot, _)) in columns.iter().enumerate() {
            assert_eq!(*slot, i);
        }
        for (metric, label) in [
            (Metric::WorkerRoundTrips, "native_lights_worker_round_trips"),
            (Metric::ReferencePasses, "native_lights_reference_passes"),
            (Metric::DisplayPasses, "native_lights_display_passes"),
            (Metric::WalkRefusedResume, "native_lights_walk_refused_resume"),
            (Metric::WalkRefusedReservation, "native_lights_walk_refused_reservation"),
        ] {
            assert_eq!(
                columns
                    [phase::Counter::ALL.len() + Stage::Lights as usize * STRIDE + metric as usize]
                    .1,
                label
            );
        }
        let i = Stage::Objects.view_index(2);
        assert_eq!(columns[i].1, "native_objects_v2_reference_ranges");
        assert_eq!(columns[i + 1].1, "native_objects_v2_display_ranges");
        assert_eq!(columns[i + 2].1, "native_objects_v2_invariants");
        let lists = phase::Counter::ALL.len() + SCENE_LIST_START;
        assert_eq!(columns[lists].1, "native_scene_lists_view_objects_native");
        let ui = phase::Counter::ALL.len() + UI_OFFSET;
        assert_eq!(ui, lists + SCENE_LISTS.len() * SCENE_LIST_RESULTS.len());
        assert_eq!(
            columns[ui - 1].1,
            "native_scene_lists_view_projection_dropped"
        );
        for (stage, label) in [
            (SceneList::LightAdditionalCasters, "native_scene_lists_light_additional_casters_native"),
            (SceneList::TypedLightPasses, "native_scene_lists_typed_light_passes_native"),
            (SceneList::TypedFirstPerson, "native_scene_lists_typed_first_person_native"),
            (SceneList::ObjectShadowList, "native_scene_lists_object_shadow_list_native"),
            (SceneList::ObjectShadowRows, "native_scene_lists_object_shadow_rows_native"),
            (SceneList::ObjectShadowCaster, "native_scene_lists_object_shadow_caster_native"),
        ] {
            assert_eq!(columns[lists + stage as usize * SCENE_LIST_RESULTS.len()].1, label);
        }
        let first_person = lists + SceneList::FirstPersonLights as usize * SCENE_LIST_RESULTS.len();
        assert_eq!(
            columns[first_person].1,
            "native_scene_lists_first_person_lights_native"
        );
        let frame = lists + SceneList::LightFrame as usize * SCENE_LIST_RESULTS.len();
        assert_eq!(columns[frame].1, "native_scene_lists_light_frame_native");
        assert_eq!(columns[ui].1, "native_ui_items");
        let animation = phase::Counter::ALL.len() + ANIMATION_OFFSET;
        assert_eq!(
            columns[animation - 1].1,
            "native_ui_overlay_menu_reveal_unwritten"
        );
        assert_eq!(columns[animation].1, "native_shader_animation_layers_drawn");
        assert_eq!(
            columns[animation + ShaderAnimation::ObjectsRefused as usize].1,
            "native_shader_animation_objects_refused"
        );
        assert_eq!(
            columns[phase::Counter::ALL.len() + COLUMNS - 1].1,
            "native_shader_animation_layer3_refused"
        );
        assert_eq!(columns[ui + Ui::OverlayMenuClaimed as usize].1, "native_ui_overlay_menu_claimed");
        assert_eq!(
            columns[ui + Ui::OverlayMenuRefusedPass as usize].1,
            "native_ui_overlay_menu_refused_pass"
        );
        assert_eq!(columns[ui + Ui::OverlayRefusedTexture as usize].1, "native_ui_overlay_refused_texture");
        assert_eq!(columns[ui + Ui::OverlayModelViews as usize].1, "native_ui_overlay_model_views");
        assert_eq!(columns[ui + Ui::OverlayRefusedBlur as usize].1, "native_ui_overlay_refused_blur");
        assert_eq!(columns[ui + Ui::OverlayMovieFrames as usize].1, "native_ui_overlay_movie_frames");
        assert_eq!(columns[ui + Ui::MovieRefusedPass as usize].1, "native_ui_movie_refused_pass");
        assert_eq!(columns[ui + Ui::ModelViewRustMismatchCamera as usize].1, "native_ui_model_view_rust_mismatch_camera");
        assert_eq!(columns[ui + Ui::OverlayHudTagQuads as usize].1, "native_ui_overlay_hud_tag_quads");
        assert_eq!(
            columns[ui + Ui::OutlineReservation as usize].1,
            "native_ui_outline_fallback_reservation"
        );
        assert_eq!(columns[ui + Ui::OverlayItems as usize].1, "native_ui_overlay_items");
        assert_eq!(columns[ui + Ui::OverlayBatches as usize].1, "native_ui_overlay_batches");
        let walk = phase::Counter::ALL.len() + STAGE_COLUMNS;
        assert_eq!(
            columns[walk + ObjectWalk::Native as usize].1,
            "native_objects_walk_native"
        );
        assert_eq!(
            columns[walk + ObjectWalk::Reservation as usize].1,
            "native_objects_walk_reservation"
        );
        assert_eq!(
            columns[walk + ObjectWalk::PartAuxState as usize].1,
            "native_objects_walk_part_aux_state"
        );
        assert_eq!(
            columns[walk + ObjectWalk::Unresident as usize].1,
            "native_objects_walk_unresident"
        );
        assert_eq!(
            columns[walk + ObjectWalk::Unresident as usize + 1].1,
            "native_world_opaque_prereserved_draws"
        );
        let world = phase::Counter::ALL.len() + Stage::World as usize * STRIDE;
        for (metric, label) in [
            (Metric::ExtractNs, "native_world_extract_ns"),
            (Metric::RenderPrepareNs, "native_world_render_prepare_ns"),
            (Metric::SceneStages, "native_world_scene_stages"),
            (Metric::SceneDropped, "native_world_scene_dropped"),
        ] {
            assert_eq!(columns[world + metric as usize].1, label);
        }
    }

    #[test]
    fn flares_have_a_stage_of_their_own() {
        assert_eq!(Stage::Flares.name(), "flares");
        assert_eq!(STAGES[Stage::Flares as usize], "flares");
    }

    #[test]
    fn preparation_on_the_render_thread_is_charged_apart_from_the_game_thread() {
        assert!(matches!(charged(Metric::PrepareNs), Metric::PrepareNs));
        std::thread::spawn(|| {
            mark_render_thread();
            assert!(on_render_thread());
            assert!(matches!(
                charged(Metric::PrepareNs),
                Metric::RenderPrepareNs
            ));
            assert!(matches!(charged(Metric::EncodeNs), Metric::EncodeNs));
        })
        .join()
        .unwrap();
        assert!(!on_render_thread(), "the mark is the marked thread's alone");
    }
}
