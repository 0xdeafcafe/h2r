//! Read the current invocation's geometry before a layer begin or handler
//! runs. The caller owns the item producer: after the light's ITEMS, the
//! slot's RECORDS, or the mask caster's ITEMS. The additional caster's
//! receiver interval exists only after its own record walk.

use guest_layout::memory::GuestMemory;
use engine::shadow_constants::global::{TARGET_STRIDE, TARGETS};
use frame_scene::{EntryState, GroupSite, LightGroup, LightStep, LightTarget, PROJECTION_ROWS, ShadowPass};
use native_render::shadows;
use scene_extract::light_groups;

use super::{Ro, SceneGroup, walk};

const VIEW_TARGET: u32 = 0x0048_58b8;

// Raster mirrors, including the words inherited by a begin. The selected
// target, pass flags, stencil and projection come from this invocation.
const CLIP_CONTROL: u32 = 0x004b_8344;
const RASTER: u32 = 0x004b_8464;
const OFFSET_ENABLE: u32 = 0x004b_8340;
const OFFSET_CONSTANT: u32 = 0x004b_8334;
const OFFSET_SLOPE: u32 = 0x004b_8330;
const SHADOW_FUNCTION: u32 = 0x004b_846c;

/// The state at layer draw. Layer 10's begin writes the bias constants
/// that survive its end; its end disables offset and resets clip and
/// raster. Layer 11's end restores the compare and clip. 0x00015680
/// controls different words from these.
fn pass(
    mem: &dyn GuestMemory,
    step: &LightStep,
    flags: u32,
    caster_ended: bool,
    receiver_ended: bool,
) -> Option<ShadowPass> {
    let word = |at| mem.u32(at).ok();
    let target = match (step.layer, step.target) {
        (10, LightTarget::ShadowMap) => {
            if shadows::colour_mask(flags) {
                10
            } else {
                9
            }
        }
        (11, LightTarget::View) => word(VIEW_TARGET)?,
        (14, LightTarget::Target(1)) => 1,
        _ => return None,
    };
    let entry = TARGETS.checked_add(target.checked_mul(TARGET_STRIDE)?)?;
    let size = [
        word(entry.checked_add(0x80)?)?,
        word(entry.checked_add(0x84)?)?,
    ];
    if size.iter().any(|&n| n == 0 || n > i32::MAX as u32) {
        return None;
    }
    // Read the receiver's GREATER and its end's NEVER operands from
    // their push instructions, and the bias from the image's floats.
    let shadow_function = if step.layer == 11 {
        word(0x0004_96f1)?
    } else if receiver_ended {
        word(0x0004_9741)?
    } else {
        word(SHADOW_FUNCTION)?
    };
    let casting = step.layer == 10;
    let clip_control = match step.layer {
        10 => {
            if flags & (1 << 4) != 0 {
                16
            } else {
                1
            }
        }
        11 => word(0x0004_9705)?,
        _ if caster_ended || receiver_ended => 0,
        _ => word(CLIP_CONTROL)?,
    };
    let (raster, offset_enable) = if casting {
        (u32::from(mem.u8(0x0047_fe84).ok()? != 0) << 1, 1)
    } else if caster_ended {
        (0, 0)
    } else {
        (word(RASTER)?, word(OFFSET_ENABLE)?)
    };
    let (offset_constant, offset_slope) = if casting || caster_ended {
        (word(0x0045_e46c)?, word(0x0045_dc0c)?)
    } else {
        (word(OFFSET_CONSTANT)?, word(OFFSET_SLOPE)?)
    };
    Some(ShadowPass {
        target,
        size,
        flags,
        clip_control,
        raster,
        offset_enable,
        offset_constant,
        offset_slope,
        stencil_mode: 0,
        shadow_function,
    })
}

/// Why a site's groups were not made: the first step that could not be
/// read whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupRefusal {
    /// A layer the site does not gather (the first person's casters, a
    /// layer outside 10 to 14).
    Layer,
    /// The list does not read.
    Memory,
    /// A base list whose receiver suffix is open (`+12` not zero).
    Suffix,
    /// The list's layer mask and the step's draw disagree.
    Mask,
    /// The layer's sort did not read.
    Sort,
    /// A sorted entry the scene cannot hold whole, by its cause; an
    /// entry whose state ([`EntryState`]) its layer does not take or the
    /// host does not hold now is `Entry(State)`.
    Entry(light_groups::Cause),
    /// The layer's target or raster state did not read.
    Pass,
    /// Layer 10 with no shadow projection.
    Projection,
}

impl GroupRefusal {
    /// Every name, for a counter per cause.
    pub const NAMES: [&'static str; 16] = [
        "layer", "memory", "suffix", "mask", "sort", "pass", "projection",
        "entry_memory", "entry_callback", "entry_owner", "entry_section", "entry_range",
        "entry_state", "entry_subparts", "entry_material", "entry_pose",
    ];

    pub fn name(self) -> &'static str {
        use light_groups::Cause;
        match self {
            GroupRefusal::Layer => "layer",
            GroupRefusal::Memory => "memory",
            GroupRefusal::Suffix => "suffix",
            GroupRefusal::Mask => "mask",
            GroupRefusal::Sort => "sort",
            GroupRefusal::Pass => "pass",
            GroupRefusal::Projection => "projection",
            GroupRefusal::Entry(c) => match c {
                Cause::Memory => "entry_memory",
                Cause::Callback => "entry_callback",
                Cause::Owner => "entry_owner",
                Cause::Section => "entry_section",
                Cause::Range => "entry_range",
                Cause::State => "entry_state",
                Cause::Subparts => "entry_subparts",
                Cause::Material => "entry_material",
                Cause::Pose => "entry_pose",
            },
        }
    }
}

/// All the drawn groups of one site, in step order, or `None` (see
/// [`plan_or_refusal`] for why).
#[cfg(test)]
pub(super) fn plan(
    mem: &dyn GuestMemory,
    site: GroupSite,
    steps: &[LightStep],
    flags: u32,
    definition_flags: u32,
    projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
) -> Option<Vec<SceneGroup>> {
    plan_or_refusal(mem, site, steps, flags, definition_flags, projection).ok()
}

/// All the drawn groups of one site, in step order. A single unreadable
/// or unsupported entry discards the entire collection, named by the
/// first refusal. Only call with this invocation's item lists, including
/// any pending shadow-target resize in `mem` (or after proving its size
/// is unchanged). An entry with state is taken in layers 10 to 14 only
/// while the host holds what its handler's epilogue would leave
/// ([`super::held_state`], asked once and only for such an entry).
pub(super) fn plan_or_refusal(
    mem: &dyn GuestMemory,
    site: GroupSite,
    steps: &[LightStep],
    flags: u32,
    definition_flags: u32,
    projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
) -> Result<Vec<SceneGroup>, GroupRefusal> {
    let mut held = None;
    plan_held(mem, site, steps, flags, definition_flags, projection, &mut || {
        *held.get_or_insert_with(|| super::held_state(mem))
    })
}

/// Whether an entry's `state` draws in `layer` with the host holding
/// `held`.
pub(super) fn state_drawn(layer: u8, state: EntryState, held: &mut dyn FnMut() -> EntryState) -> bool {
    if state.plain() {
        return true;
    }
    if !light_groups::state_taken(layer, state) {
        return false;
    }
    let held = held();
    (!state.aux_depth || held.aux_depth) && (!state.fade || held.fade) && (!state.structure || held.structure)
}

/// [`plan_or_refusal`] with what the host holds given by `held`.
pub(super) fn plan_held(
    mem: &dyn GuestMemory,
    site: GroupSite,
    steps: &[LightStep],
    flags: u32,
    definition_flags: u32,
    projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
    held: &mut dyn FnMut() -> EntryState,
) -> Result<Vec<SceneGroup>, GroupRefusal> {
    use GroupRefusal as R;
    let mut groups = Vec::new();
    let mut caster_ended = false;
    let mut receiver_ended = false;
    for step in steps {
        let layer = u32::from(step.layer);
        let list = match (site, layer) {
            (GroupSite::FirstPerson, 10) => return Err(R::Layer),
            (_, 10) => shadows::CASTERS,
            (_, 11) => shadows::RECEIVERS,
            (GroupSite::Light | GroupSite::FirstPerson, 12..=14) => shadows::RECEIVERS,
            _ => return Err(R::Layer),
        };
        // The two base list producers clear first; only the additional
        // caster opens a receiver suffix within the light's lists.
        if (site != GroupSite::AdditionalCasters || layer == 10) && mem.u32(list + 12).map_err(|_| R::Memory)? != 0 {
            return Err(R::Suffix);
        }
        if (mem.u32(list + 8).map_err(|_| R::Memory)? & (1 << layer) != 0) != step.draws {
            return Err(R::Mask);
        }
        if !step.draws {
            continue;
        }
        let sorted = walk::sorted_for_pass(mem, list, layer, flags, definition_flags).map_err(|_| R::Sort)?;
        let items = mem.u32(list + 20).map_err(|_| R::Memory)?;
        let mut entries = Vec::with_capacity(sorted.len());
        let mut rows = Vec::with_capacity(sorted.len());
        for slot in sorted {
            let (entry, pose) = light_groups::entry(&Ro(mem), items, slot).map_err(R::Entry)?;
            light_groups::taken(step.layer, &entry).map_err(R::Entry)?;
            if !state_drawn(step.layer, entry.state, held) {
                return Err(R::Entry(light_groups::Cause::State));
            }
            entries.push(entry);
            rows.push(pose);
        }
        let pass = match layer {
            10 | 11 | 14 => Some(pass(mem, step, flags, caster_ended, receiver_ended).ok_or(R::Pass)?),
            _ => None,
        };
        let projection = match layer {
            10 => Some(projection.ok_or(R::Projection)?),
            11 => projection,
            _ => None,
        };
        groups.push(SceneGroup {
            group: LightGroup {
                light: 0,
                layer: step.layer,
                site,
                projection,
                pass,
                entries: 0..entries.len() as u32,
                remainder: 0,
            },
            entries,
            rows,
        });
        caster_ended |= layer == 10;
        receiver_ended |= layer == 11;
    }
    Ok(groups)
}
