//! A light's passes under the cut (docs/graphics/native-lights.md, "The
//! light frame under the cut"). When the host draws the scene's light
//! layers ([`super::scene_path_ready`]) the guest's light frame body never
//! runs: the Rust frame body gathers every drawn light as the guest does,
//! and each light's passes (`0x00031170`) stop after their first step, the
//! item lists `0x00040e90`. What the guest's remaining steps would draw
//! goes into the scene instead: the light and its steps (from the
//! hand-off, [`super::handoff`]) and, here, one group per drawn layer,
//! made from the lists the item body just built as the guest's layer
//! builds would sort them ([`super::typed_groups::plan`]). No constant,
//! target, stencil or draw call is made.
//!
//! The light's pass flags and shadow projection are computed, not read:
//! the constants `0x0004a1e0` and the shadow setup `0x00048ee0` do not
//! run. The setup's shadow-target resize is laid over guest memory for the
//! groups' read only ([`super::typed::Mirror`]); nothing is written.
//!
//! A light it takes draws at once, at its passes, from its groups
//! ([`super::draw_cut_light`]); only then do the groups go into the
//! scene. A light this cannot make whole, or the host does not draw,
//! dropped for the frame: its passes return after the item lists, and it
//! is counted by cause (`scene_dropped_light_<cause>` in `render.scene`; a
//! group's refusal also `scene_dropped_light_group_<refusal>`). The guest's
//! remaining passes never run.

use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_constants as lc;
use engine::memory::GuestMemory;
use engine::render::light_gather::{Writes, passes};
use engine::shadow_constants as sc;
use frame_scene::{GroupSite, PROJECTION_ROWS, SceneLight};
use native_render::light_passes::{self, SHADOW, Steps};

use super::typed::Mirror;
use super::typed_groups::GroupRefusal;
use super::{Entry, SceneGroup, Source, source_constants, typed_groups};

/// Why a light's passes put nothing into the scene. The light is not drawn
/// this frame and is counted by cause; the guest's passes never run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The passes' arguments, the light's lists or its tags did not read.
    Unread,
    /// The light's constants or flags did not compute.
    Constants,
    /// The shadow projection did not compute.
    Projection,
    /// A layer's entries are not all of a kind the scene holds whole.
    Group(GroupRefusal),
    /// The scene's open light is not this call's (the hand-off refused).
    Light,
    /// The additional casters' pair (`0x00030ed0`) would make a suffix:
    /// a candidate's first match in the current visibility list is marked
    /// `0x1000`, or the candidates do not read. Its record walk is not
    /// built.
    AdditionalCasters,
    /// The host did not draw the light's groups (`Host::draw_scene_light`).
    Host,
}

impl Dropped {
    pub const COUNT: usize = 7;

    pub(super) fn index(self) -> usize {
        match self {
            Dropped::Unread => 0,
            Dropped::Constants => 1,
            Dropped::Projection => 2,
            Dropped::Group(_) => 3,
            Dropped::Light => 4,
            Dropped::AdditionalCasters => 5,
            Dropped::Host => 6,
        }
    }

    pub const NAMES: [&'static str; Dropped::COUNT] = [
        "scene_dropped_light_unread",
        "scene_dropped_light_constants",
        "scene_dropped_light_projection",
        "scene_dropped_light_group",
        "scene_dropped_light_light",
        "scene_dropped_light_additional_casters",
        "scene_dropped_light_host",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }
}

static DROPPED: [AtomicU64; Dropped::COUNT] = [const { AtomicU64::new(0) }; Dropped::COUNT];
/// `scene_dropped_light_group_<refusal>`, in [`GroupRefusal::NAMES`]' order.
pub const GROUP_NAMES: [&str; GroupRefusal::NAMES.len()] = [
    "scene_dropped_light_group_layer",
    "scene_dropped_light_group_memory",
    "scene_dropped_light_group_suffix",
    "scene_dropped_light_group_mask",
    "scene_dropped_light_group_sort",
    "scene_dropped_light_group_pass",
    "scene_dropped_light_group_projection",
    "scene_dropped_light_group_entry_memory",
    "scene_dropped_light_group_entry_callback",
    "scene_dropped_light_group_entry_owner",
    "scene_dropped_light_group_entry_section",
    "scene_dropped_light_group_entry_range",
    "scene_dropped_light_group_entry_state",
    "scene_dropped_light_group_entry_subparts",
    "scene_dropped_light_group_entry_material",
    "scene_dropped_light_group_entry_pose",
];

static DROPPED_GROUP: [AtomicU64; GroupRefusal::NAMES.len()] =
    [const { AtomicU64::new(0) }; GroupRefusal::NAMES.len()];
/// Why a whole light frame (`0x00031c80`) or a light's passes drew
/// nothing, before any light of it reached [`plan`]. The guest's body never
/// runs for any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDropped {
    /// The lights stage stands down on this map (`Stage::Lights`).
    StandingDown,
    /// No host draws the scene's light layers ([`super::scene_path_ready`]).
    Host,
    /// The frame's plan refused (`frame::plan`).
    Plan,
    /// A light's passes' entry plan refused (`passes::plan`).
    Passes,
}

impl FrameDropped {
    pub const NAMES: [&'static str; 4] = [
        "scene_dropped_light_frame_standing_down",
        "scene_dropped_light_frame_host",
        "scene_dropped_light_frame_plan",
        "scene_dropped_light_frame_passes",
    ];

    fn index(self) -> usize {
        match self {
            FrameDropped::StandingDown => 0,
            FrameDropped::Host => 1,
            FrameDropped::Plan => 2,
            FrameDropped::Passes => 3,
        }
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }
}

static FRAME_DROPPED: [AtomicU64; FrameDropped::NAMES.len()] =
    [const { AtomicU64::new(0) }; FrameDropped::NAMES.len()];

/// Counts a light frame or passes call dropped under `why`.
pub fn drop_frame(why: FrameDropped) {
    FRAME_DROPPED[why.index()].fetch_add(1, Ordering::Relaxed);
}

static LIGHTS: AtomicU64 = AtomicU64::new(0);
static GROUPS: AtomicU64 = AtomicU64::new(0);

/// Counts a light dropped under `why`.
pub fn drop_light(why: Dropped) {
    DROPPED[why.index()].fetch_add(1, Ordering::Relaxed);
    drop_group(why);
}

/// A dropped light's or first-person slot's group refusal, counted in
/// `scene_dropped_light_group_<refusal>`.
pub(super) fn drop_group(why: Dropped) {
    if let Dropped::Group(refusal) = why
        && let Some(n) = GroupRefusal::NAMES.iter().position(|&n| n == refusal.name())
    {
        DROPPED_GROUP[n].fetch_add(1, Ordering::Relaxed);
    }
}

/// `render.scene`'s columns for the cut light frame: lights and groups
/// the scene took, and lights dropped by cause.
pub fn counters() -> Vec<(&'static str, u64)> {
    [
        ("scene_cut_lights", LIGHTS.load(Ordering::Relaxed)),
        ("scene_cut_light_groups", GROUPS.load(Ordering::Relaxed)),
    ]
    .into_iter()
    .chain(Dropped::NAMES.into_iter().zip(DROPPED.iter().map(|c| c.load(Ordering::Relaxed))))
    .chain(GROUP_NAMES.into_iter().zip(DROPPED_GROUP.iter().map(|c| c.load(Ordering::Relaxed))))
    .chain(
        FrameDropped::NAMES
            .into_iter()
            .zip(FRAME_DROPPED.iter().map(|c| c.load(Ordering::Relaxed))),
    )
    .collect()
}

/// What one light's passes put into the scene.
#[derive(Clone, Debug)]
pub struct Planned {
    /// The scene's light as the passes would set it.
    pub light: SceneLight,
    pub steps: Steps,
    /// One group per drawn layer, in step order.
    pub groups: Vec<SceneGroup>,
    /// The additional casters would run.
    pub additional_casters: bool,
    /// The light's record, constants and gels as the host draws it, put
    /// in the scene's light for the draw at the passes (the view's
    /// candidates are read only once its light pass is over).
    pub candidate: frame_scene::LightCandidate,
}

/// The passes' registers at their entry, as the hand-off reads them.
pub fn entry(p: &passes::Plan) -> Entry {
    Entry {
        eax: p.definition,
        ecx: p.context,
        esp: p.entry_esp,
        mxcsr: p.fpu.mxcsr,
        x87_control: p.fpu.x87_control,
    }
}

/// The scene's light and groups for the passes entered with `e`, once the
/// item lists are made. Reads only.
pub fn plan(mem: &dyn GuestMemory, e: &Entry) -> Result<Planned, Dropped> {
    let source = Source::observed(mem, e).ok_or(Dropped::Unread)?;
    let fpu = e.fpu();
    let c = source_constants(mem, &source, fpu).ok_or(Dropped::Constants)?;
    let inputs = source
        .inputs(mem, c.flags, c.definition_flags)
        .ok_or(Dropped::Unread)?;
    let steps = light_passes::steps(&inputs);
    let shadow = if c.flags & SHADOW != 0 {
        let g = lc::light_globals(mem, &source.call, fpu, &mut |_| None)
            .map_err(|_| Dropped::Constants)?;
        let call = sc::Call {
            context: source.call.context,
            frame: source.call.frame,
            definition: source.call.definition,
        };
        Some(
            sc::shadow_globals_for_light(mem, &call, fpu, &g, c.flags)
                .map_err(|_| Dropped::Projection)?,
        )
    } else {
        None
    };
    let groups = groups(mem, &steps, c.flags, c.definition_flags, shadow.as_ref())?;
    let record = super::read_light(
        mem,
        source.call.light,
        source.call.shadow & 0xff != 0,
        f32::from_bits(source.call.weight),
    )
    .ok_or(Dropped::Unread)?;
    let candidate = super::first_person::candidate(mem, &record, source.call.definition, c);
    Ok(Planned {
        light: SceneLight {
            handle: source.call.light,
            definition: source.definition,
            flags: inputs.flags,
            definition_flags: inputs.definition_flags,
            additional_casters: inputs.additional_casters,
            guest_weight: Some(f32::from_bits(source.call.weight)),
            ..Default::default()
        },
        steps,
        groups,
        additional_casters: inputs.additional_casters,
        candidate,
    })
}

/// The groups of a light's drawn `steps` from the item lists in `mem`, as
/// the guest's layer builds would sort them with the passes' `flags`.
/// `shadow` is what the shadow setup would store: its block is the
/// casters' and receivers' projection, and its target words are laid over
/// `mem` for the read (the setup's resize), not written.
pub fn groups(
    mem: &dyn GuestMemory,
    steps: &Steps,
    flags: u32,
    definition_flags: u32,
    shadow: Option<&sc::ShadowGlobals>,
) -> Result<Vec<SceneGroup>, Dropped> {
    let mut mirror = Mirror {
        mem,
        writes: Writes::new(),
    };
    let projection = match shadow {
        Some(shadow) => {
            shadow.write(&mut mirror).map_err(|_| Dropped::Unread)?;
            Some(projection_rows(&shadow.block))
        }
        None => None,
    };
    typed_groups::plan_or_refusal(
        &mirror,
        GroupSite::Light,
        steps.as_slice(),
        flags,
        definition_flags,
        projection,
    )
    .map_err(Dropped::Group)
}

/// A cut light's layers as the host draws them: one pass per group, in
/// step order, each with the light.
pub(super) fn cut_passes(
    slot: frame_scene::ViewSlot,
    light: &frame_scene::Light,
    groups: &[SceneGroup],
) -> Vec<super::ScenePass> {
    groups
        .iter()
        .map(|group| super::ScenePass { slot, light: light.clone(), group: group.clone() })
        .collect()
}

/// The projection block's rows as the scene's groups hold them.
fn projection_rows(block: &[u32]) -> [[u32; 4]; PROJECTION_ROWS] {
    core::array::from_fn(|row| core::array::from_fn(|col| block.get(4 * row + col).copied().unwrap_or(0)))
}

/// A passes call whose plan refused: the light is dropped.
pub(super) fn refused() {
    drop_frame(FrameDropped::Passes);
    drop_light(Dropped::Unread);
}

/// After the item lists of the passes entered with `p`: the light's
/// groups into the scene, behind the light the hand-off opened. Returns
/// the groups put in, or why the light is dropped (counted by
/// [`drop_light`]).
pub(super) fn light(mem: &mut dyn GuestMemory, p: &passes::Plan) -> Result<usize, Dropped> {
    let e = entry(p);
    // A light whose lists the guest made was left for the constants call,
    // which does not run here.
    if let Some((deferred, serial)) = super::handoff::take_deferred(e.esp) {
        super::observe(&*mem, deferred, serial);
    }
    // The scene draws the light or nothing does: no layer of this light is
    // a candidate of the resolved-draw path.
    super::CANDIDATE.take();
    let taken = take(mem, &e);
    if let Err(why) = taken {
        drop_light(why);
    }
    taken
}

/// The light drawn and its groups into the scene, or why not. Nothing is
/// drawn and nothing goes in on a refusal.
fn take(mem: &mut dyn GuestMemory, e: &Entry) -> Result<usize, Dropped> {
    let planned = plan(&*mem, e)?;
    if !crate::scene_step::current_light_matches(&planned.light, planned.steps.as_slice()) {
        return Err(Dropped::Light);
    }
    // The additional casters' pair (`0x00030ed0`) makes no suffix when no
    // candidate's first match in the current visibility list is marked
    // `0x1000` (`passes::additional::empty`): it draws nothing, and its one
    // store puts back the cursor it saved, so the light draws its own
    // steps. A pair that would make a suffix needs the marker lookup and
    // the suffix's record walk, which are not built.
    if planned.additional_casters && passes::additional::empty(&*mem).is_err() {
        return Err(Dropped::AdditionalCasters);
    }
    let mut light = crate::scene_step::current_light().ok_or(Dropped::Light)?;
    light.candidate = Some(planned.candidate.clone());
    let passes = cut_passes(super::LIGHT_SLOT, &light, &planned.groups);
    if !super::draw_cut_light(mem, &passes, super::view_target()) {
        return Err(Dropped::Host);
    }
    let n = planned.groups.len();
    crate::scene_step::typed_light_groups(planned.groups);
    LIGHTS.fetch_add(1, Ordering::Relaxed);
    GROUPS.fetch_add(n as u64, Ordering::Relaxed);
    Ok(n)
}
