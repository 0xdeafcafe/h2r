//! Read the current invocation's geometry before a layer begin or handler
//! runs. The caller owns the item producer: after the light's ITEMS, the
//! slot's RECORDS, or the mask caster's ITEMS. The additional caster's
//! receiver interval exists only after its own record walk.

use engine::memory::GuestMemory;
use engine::shadow_constants::global::{TARGET_STRIDE, TARGETS};
use frame_scene::{GroupSite, LightGroup, LightStep, LightTarget, PROJECTION_ROWS, ShadowPass};
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

/// All the drawn groups of one site, in step order. A single unreadable
/// or unsupported entry discards the entire collection. Only call with
/// this invocation's item lists, including any pending shadow-target
/// resize in `mem` (or after proving its size is unchanged).
pub(super) fn plan(
    mem: &dyn GuestMemory,
    site: GroupSite,
    steps: &[LightStep],
    flags: u32,
    definition_flags: u32,
    projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
) -> Option<Vec<SceneGroup>> {
    let mut groups = Vec::new();
    let mut caster_ended = false;
    let mut receiver_ended = false;
    for step in steps {
        let layer = u32::from(step.layer);
        let list = match (site, layer) {
            (GroupSite::FirstPerson, 10) => return None,
            (_, 10) => shadows::CASTERS,
            (_, 11) => shadows::RECEIVERS,
            (GroupSite::Light | GroupSite::FirstPerson, 12..=14) => shadows::RECEIVERS,
            _ => return None,
        };
        // The two base list producers clear first; only the additional
        // caster opens a receiver suffix within the light's lists.
        if (site != GroupSite::AdditionalCasters || layer == 10) && mem.u32(list + 12).ok()? != 0 {
            return None;
        }
        if (mem.u32(list + 8).ok()? & (1 << layer) != 0) != step.draws {
            return None;
        }
        if !step.draws {
            continue;
        }
        let sorted = walk::sorted_for_pass(mem, list, layer, flags, definition_flags).ok()?;
        let items = mem.u32(list + 20).ok()?;
        let mut entries = Vec::with_capacity(sorted.len());
        let mut rows = Vec::with_capacity(sorted.len());
        for slot in sorted {
            let (entry, pose) = light_groups::entry(&Ro(mem), items, slot).ok()?;
            entries.push(entry);
            rows.push(pose);
        }
        let pass = match layer {
            10 | 11 | 14 => Some(pass(mem, step, flags, caster_ended, receiver_ended)?),
            _ => None,
        };
        let projection = match layer {
            10 => Some(projection?),
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
    Some(groups)
}
