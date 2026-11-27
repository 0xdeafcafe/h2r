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
//! A light this cannot make whole keeps the guest's draw: its passes run
//! on past the item lists, and it is counted by cause
//! (`scene_kept_light_<cause>` in `render.scene`; a group's refusal also
//! `scene_kept_light_group_<refusal>`). Nothing goes undrawn.

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

/// Why a light's passes put nothing into the scene: the guest's passes
/// draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
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
    /// The additional casters' layers (`0x00030ed0`), whose record walk
    /// the cut does not make: the guest draws the whole light.
    AdditionalCasters,
}

impl Kept {
    pub const COUNT: usize = 6;

    fn index(self) -> usize {
        match self {
            Kept::Unread => 0,
            Kept::Constants => 1,
            Kept::Projection => 2,
            Kept::Group(_) => 3,
            Kept::Light => 4,
            Kept::AdditionalCasters => 5,
        }
    }

    pub const NAMES: [&'static str; Kept::COUNT] = [
        "scene_kept_light_unread",
        "scene_kept_light_constants",
        "scene_kept_light_projection",
        "scene_kept_light_group",
        "scene_kept_light_light",
        "scene_kept_light_additional_casters",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }
}

static KEPT: [AtomicU64; Kept::COUNT] = [const { AtomicU64::new(0) }; Kept::COUNT];
/// `scene_kept_light_group_<refusal>`, in [`GroupRefusal::NAMES`]' order.
pub const GROUP_NAMES: [&str; GroupRefusal::NAMES.len()] = [
    "scene_kept_light_group_layer",
    "scene_kept_light_group_memory",
    "scene_kept_light_group_suffix",
    "scene_kept_light_group_mask",
    "scene_kept_light_group_sort",
    "scene_kept_light_group_pass",
    "scene_kept_light_group_projection",
    "scene_kept_light_group_entry_memory",
    "scene_kept_light_group_entry_callback",
    "scene_kept_light_group_entry_owner",
    "scene_kept_light_group_entry_section",
    "scene_kept_light_group_entry_range",
    "scene_kept_light_group_entry_state",
    "scene_kept_light_group_entry_subparts",
    "scene_kept_light_group_entry_material",
    "scene_kept_light_group_entry_pose",
];

static KEPT_GROUP: [AtomicU64; GroupRefusal::NAMES.len()] =
    [const { AtomicU64::new(0) }; GroupRefusal::NAMES.len()];
static LIGHTS: AtomicU64 = AtomicU64::new(0);
static GROUPS: AtomicU64 = AtomicU64::new(0);

/// Counts a light the guest's passes draw, under `why`.
pub fn keep_light(why: Kept) {
    KEPT[why.index()].fetch_add(1, Ordering::Relaxed);
    if let Kept::Group(refusal) = why
        && let Some(n) = GroupRefusal::NAMES.iter().position(|&n| n == refusal.name())
    {
        KEPT_GROUP[n].fetch_add(1, Ordering::Relaxed);
    }
}

/// `render.scene`'s columns for the cut light frame: lights and groups
/// the scene took, and lights the guest draws by cause.
pub fn counters() -> Vec<(&'static str, u64)> {
    [
        ("scene_cut_lights", LIGHTS.load(Ordering::Relaxed)),
        ("scene_cut_light_groups", GROUPS.load(Ordering::Relaxed)),
    ]
    .into_iter()
    .chain(Kept::NAMES.into_iter().zip(KEPT.iter().map(|c| c.load(Ordering::Relaxed))))
    .chain(GROUP_NAMES.into_iter().zip(KEPT_GROUP.iter().map(|c| c.load(Ordering::Relaxed))))
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
pub fn plan(mem: &dyn GuestMemory, e: &Entry) -> Result<Planned, Kept> {
    let source = Source::observed(mem, e).ok_or(Kept::Unread)?;
    let fpu = e.fpu();
    let c = source_constants(mem, &source, fpu).ok_or(Kept::Constants)?;
    let inputs = source
        .inputs(mem, c.flags, c.definition_flags)
        .ok_or(Kept::Unread)?;
    let steps = light_passes::steps(&inputs);
    let shadow = if c.flags & SHADOW != 0 {
        let g = lc::light_globals(mem, &source.call, fpu, &mut |_| None)
            .map_err(|_| Kept::Constants)?;
        let call = sc::Call {
            context: source.call.context,
            frame: source.call.frame,
            definition: source.call.definition,
        };
        Some(
            sc::shadow_globals_for_light(mem, &call, fpu, &g, c.flags)
                .map_err(|_| Kept::Projection)?,
        )
    } else {
        None
    };
    let groups = groups(mem, &steps, c.flags, c.definition_flags, shadow.as_ref())?;
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
) -> Result<Vec<SceneGroup>, Kept> {
    let mut mirror = Mirror {
        mem,
        writes: Writes::new(),
    };
    let projection = match shadow {
        Some(shadow) => {
            shadow.write(&mut mirror).map_err(|_| Kept::Unread)?;
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
    .map_err(Kept::Group)
}

/// The projection block's rows as the scene's groups hold them.
fn projection_rows(block: &[u32]) -> [[u32; 4]; PROJECTION_ROWS] {
    core::array::from_fn(|row| core::array::from_fn(|col| block.get(4 * row + col).copied().unwrap_or(0)))
}

/// A passes call whose plan refused: the guest's whole passes draw it.
pub(super) fn refused() {
    keep_light(Kept::Unread);
}

/// After the item lists of the passes entered with `p`: the light's
/// groups into the scene, behind the light the hand-off opened. Returns
/// the groups put in, or why the guest's remaining passes draw the light
/// (counted by [`keep_light`]).
pub(super) fn light(mem: &dyn GuestMemory, p: &passes::Plan) -> Result<usize, Kept> {
    let e = entry(p);
    // A light whose lists the guest made was left for the constants call,
    // which does not run here.
    if let Some((deferred, serial)) = super::handoff::take_deferred(e.esp) {
        super::observe(mem, deferred, serial);
    }
    // The scene draws the light or the guest's passes do: no layer of this
    // light is a candidate of the resolved-draw path.
    super::CANDIDATE.take();
    let taken = take(mem, &e);
    if let Err(why) = taken {
        keep_light(why);
    }
    taken
}

/// The light's groups into the scene, or why not. Nothing goes in on a
/// refusal.
fn take(mem: &dyn GuestMemory, e: &Entry) -> Result<usize, Kept> {
    let planned = plan(mem, e)?;
    if !crate::scene_step::current_light_matches(&planned.light, planned.steps.as_slice()) {
        return Err(Kept::Light);
    }
    if planned.additional_casters {
        return Err(Kept::AdditionalCasters);
    }
    let n = planned.groups.len();
    crate::scene_step::typed_light_groups(planned.groups);
    LIGHTS.fetch_add(1, Ordering::Relaxed);
    GROUPS.fetch_add(n as u64, Ordering::Relaxed);
    Ok(n)
}
