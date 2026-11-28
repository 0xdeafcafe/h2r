//! The first-person lights at `FIRST_PERSON_LIGHTS` (`0x000404b0`) and
//! each slot's record replay at `FIRST_PERSON_LIGHT_RECORDS`
//! (`0x000402b0`; docs/graphics/native-light-gather.md, slices L6 and L8).
//!
//! The loop is Rust's (`first_person::run_scene`): each slot's volume
//! and records as the guest makes them, then [`scene_slot`] draws the slot
//! from its groups on the first person's target and puts it into the
//! scene. The guest's constants call and layer draws never run. A slot
//! the scene cannot take, and every slot while no host draws the scene's
//! light layers, is dropped for the frame and counted by cause
//! (`scene_dropped_first_person_<cause>`). A plan that refuses runs the
//! guest function whole.
//!
//! At the entry E3's chosen first-person lights that have no slot are
//! queued over the first-person parts (`chosen::first_person::at_pass`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_constants::{Call, Fpu};
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light_gather::first_person::{self, SlotRecords};
use engine::render::light_gather::items::Made;
use engine::render::light_gather::typed;
use frame_scene::GroupSite;
use hooks::{Builder, Cx, EntryFuture};
use native_render::light_passes::{self, Steps};
use runtime::TransferAbi;
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use super::scene_frame::Dropped;
use super::{Ro, SceneGroup};
use crate::native_map_work::Stage;

/// Nothing is kept across calls (`native_lights::forget_runs`).
pub(super) fn forget() {}

pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::FIRST_PERSON_LIGHTS,
            "native_lights_first_person",
            move |cpu, ctx, addr| {
                // Any earlier invocation has returned.
                forget();
                if addr != points::FIRST_PERSON_LIGHTS.base {
                    return original(cpu, ctx, addr);
                }
                if Stage::Lights.standing_down() {
                    return drop_loop(cpu, ctx, LoopDropped::StandingDown);
                }
                {
                    // E3's chosen first-person lights that have no slot
                    // here: queued as E3 stages before the slots' passes,
                    // whatever runs the body.
                    let runtime = Arc::clone(ctx.runtime());
                    let mut mem = runtime.memory();
                    super::chosen::first_person::at_pass(&mut *mem);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    first_person::plan(&*mem, cpu.esp, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::FirstPersonLights, why);
                        return drop_loop(cpu, ctx, LoopDropped::Plan);
                    }
                };
                // Each slot stops after its records, draws from its groups
                // at once on the first person's target (bound as the loop
                // begins) and goes into the scene. With no host drawing the
                // scene's light layers every slot is dropped.
                let ready = super::scene_path_ready();
                let target = super::bound_target();
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, _) = first_person::run_scene(
                            &cx,
                            plan,
                            &mut |m, p, call| {
                                if ready {
                                    scene_slot(m, p, call, target)
                                } else {
                                    super::finish_candidate();
                                    super::FIRST_PERSON.set(None);
                                    drop_slot(Dropped::Host);
                                    true
                                }
                            },
                            // Each taken slot drew at its place in the loop.
                            &mut |_, _| {},
                        )
                        .await?;
                        SceneList::FirstPersonLights.count(SceneListResult::Native);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
    b.raw()
        .implement(
            &points::FIRST_PERSON_LIGHT_RECORDS,
            "native_lights_first_person_records",
            |cpu, ctx, addr| {
                if addr != points::FIRST_PERSON_LIGHT_RECORDS.base || Stage::Lights.standing_down()
                {
                    return original(cpu, ctx, addr);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    first_person::plan_records(&*mem, cpu.esp, cpu.eax, cpu.edx, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::FirstPersonRecords, why);
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, records) = first_person::run_records(&cx, plan).await?;
                        count(&records);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
}

/// Why the whole loop drew nothing. The guest body never runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopDropped {
    /// The lights stage stands down on this map.
    StandingDown,
    /// The loop's plan refused (`first_person::plan`).
    Plan,
}

const LOOP_DROPPED_NAMES: [&str; 2] = [
    "scene_dropped_first_person_loop_standing_down",
    "scene_dropped_first_person_loop_plan",
];

static LOOP_DROPPED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Counts the dropped loop and returns from `0x000404b0` as its empty
/// loop's exit does (`ret 4`, AL 1), the body not run.
fn drop_loop(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    why: LoopDropped,
) -> Result<TransferAbi, runtime::GuestFault> {
    LOOP_DROPPED[why as usize].fetch_add(1, Ordering::Relaxed);
    let mut guest = ctx.memory();
    cpu.eax = (cpu.eax & !0xff) | 1;
    guest.ret(cpu, 4)
}

static SCENE_SLOTS: AtomicU64 = AtomicU64::new(0);

/// First-person slots the scene path drew (`render.scene`'s
/// `scene_first_person_cut_slots`).
pub fn scene_slots() -> u64 {
    SCENE_SLOTS.load(Ordering::Relaxed)
}

static DROPPED: [AtomicU64; Dropped::COUNT] = [const { AtomicU64::new(0) }; Dropped::COUNT];

/// `scene_dropped_first_person_<cause>`, in [`Dropped::NAMES`]' order.
const DROPPED_NAMES: [&str; Dropped::COUNT] = [
    "scene_dropped_first_person_unread",
    "scene_dropped_first_person_constants",
    "scene_dropped_first_person_projection",
    "scene_dropped_first_person_group",
    "scene_dropped_first_person_light",
    "scene_dropped_first_person_additional_casters",
    "scene_dropped_first_person_host",
];

/// Counts a first-person slot the scene path did not draw; a group
/// refusal is also counted by its step (`scene_dropped_light_group_*`).
fn drop_slot(why: Dropped) {
    DROPPED[why.index()].fetch_add(1, Ordering::Relaxed);
    super::scene_frame::drop_group(why);
}

/// `render.scene`'s columns for the first-person slots the scene path
/// dropped, by cause.
pub fn dropped() -> Vec<(&'static str, u64)> {
    DROPPED_NAMES
        .into_iter()
        .zip(DROPPED.iter().map(|c| c.load(Ordering::Relaxed)))
        .chain(
            LOOP_DROPPED_NAMES
                .into_iter()
                .zip(LOOP_DROPPED.iter().map(|c| c.load(Ordering::Relaxed))),
        )
        .collect()
}

/// The scene's candidate for a first-person slot's light: its record,
/// its constants as [`SceneSlot::read`] computed them (no shadow, so both
/// variants are the same), and its definition's gels.
pub(super) fn candidate(
    mem: &dyn GuestMemory,
    light: &super::Light,
    definition: u32,
    constants: frame_scene::LightConstants,
) -> frame_scene::LightCandidate {
    let word = |offset: u32| {
        mem.u32(definition.wrapping_add(offset))
            .ok()
            .filter(|&w| w != u32::MAX)
    };
    frame_scene::LightCandidate {
        position: light.position,
        forward: light.forward,
        up: light.up,
        colour_a: light.colour_a,
        colour_b: light.colour_b,
        intensity: light.intensity,
        bounds: light.bounds,
        owner: None,
        definition_kind: mem.u16(definition.wrapping_add(4)).unwrap_or(0),
        volume: None,
        view_eligible: Some(true),
        priority: None,
        weight: light.weight,
        gel_bitmap: word(engine::light_passes::DEFINITION_GEL),
        gel_shader: word(engine::light_passes::DEFINITION_GEL_SHADER),
        constants: Some(frame_scene::LightVariants {
            unshadowed: constants,
            shadowed: constants,
            projection: None,
        }),
    }
}

/// A first-person slot under the scene path, its records replayed: the
/// slot planned from its lists ([`SceneSlot::read`]), its light (with its
/// constants) into the first person's view, its layers drawn now on
/// `target` ([`super::draw_cut_light`]) and only then its groups put in.
/// Always true: the guest's steps never run. A slot the scene cannot take
/// is not drawn this frame and is counted by cause ([`drop_slot`]).
fn scene_slot(mem: &mut dyn GuestMemory, plan: &first_person::Plan, call: &Call, target: Option<[u32; 8]>) -> bool {
    super::finish_candidate();
    super::FIRST_PERSON.set(None);
    match take_slot(mem, plan, call, target) {
        Ok(true) => {
            SCENE_SLOTS.fetch_add(1, Ordering::Relaxed);
        }
        Ok(false) => {}
        Err(why) => drop_slot(why),
    }
    true
}

/// The slot drawn and its groups into the scene (`Ok(true)`), a slot
/// with no drawn layer (`Ok(false)`), or why it is dropped.
fn take_slot(
    mem: &mut dyn GuestMemory,
    plan: &first_person::Plan,
    call: &Call,
    target: Option<[u32; 8]>,
) -> Result<bool, Dropped> {
    let slot = SceneSlot::read(&*mem, call, plan.fpu)?;
    if slot.steps.drawn() == 0 {
        return Ok(false);
    }
    let mut light = slot.scene_light;
    light.candidate = Some(candidate(&*mem, &slot.light, call.definition, slot.constants));
    if !crate::scene_step::first_person_light(&*mem, light, slot.steps.as_slice()) {
        return Err(Dropped::Light);
    }
    let light = crate::scene_step::current_light().ok_or(Dropped::Light)?;
    let passes = super::scene_frame::cut_passes(super::FIRST_PERSON_SLOT, &light, &slot.groups);
    if !super::draw_cut_light(mem, &passes, target) {
        return Err(Dropped::Host);
    }
    crate::scene_step::typed_light_groups(slot.groups);
    Ok(true)
}

/// A first-person slot as the scene path draws it. Layer 11 and 14 items
/// and a shader gel are the host's to draw (`draw_cut_light`), and the
/// loop makes its own tail, so neither refuses here.
struct SceneSlot {
    light: super::Light,
    constants: frame_scene::LightConstants,
    steps: Steps,
    groups: Vec<SceneGroup>,
    scene_light: frame_scene::SceneLight,
}

impl SceneSlot {
    fn read(mem: &dyn GuestMemory, call: &Call, fpu: Fpu) -> Result<Self, Dropped> {
        let globals = typed::plan(mem, call, fpu, typed::Site::FirstPerson).map_err(|_| Dropped::Constants)?;
        let constants = super::constants_of(mem, &globals.light, globals.flags, call.definition)
            .ok_or(Dropped::Constants)?;
        let inputs = light_passes::read(&Ro(mem), globals.flags, constants.definition_flags)
            .ok_or(Dropped::Unread)?;
        let steps = light_passes::first_person_steps(&inputs);
        if steps.drawn() != 0 {
            // The accepted slot opens this camera's first-person scene view.
            scene_extract::extract::camera(&Ro(mem)).ok_or(Dropped::Unread)?;
        }
        let groups = super::typed_groups::plan_or_refusal(
            mem,
            GroupSite::FirstPerson,
            steps.as_slice(),
            globals.flags,
            constants.definition_flags,
            None,
        )
        .map_err(Dropped::Group)?;
        let light = super::read_light(mem, call.light, false, f32::from_bits(call.weight))
            .ok_or(Dropped::Unread)?;
        let scene_light = super::scene_light(&light, &inputs);
        Ok(Self { light, constants, steps, groups, scene_light })
    }
}

/// One count per record that passed the filter: native for a record
/// appended in Rust, the fallback and its reason for one the guest's
/// per-part calls made.
fn count(records: &SlotRecords) {
    for &(_, made) in &records.made {
        match made {
            Made::Rust => SceneList::FirstPersonRecords.count(SceneListResult::Native),
            Made::Guest(why) => refused(SceneList::FirstPersonRecords, why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn a_dropped_slot_is_counted_under_the_dynamic_lights_cause_with_its_own_prefix() {
        for (fp, light) in DROPPED_NAMES.iter().zip(Dropped::NAMES) {
            assert_eq!(
                fp.strip_prefix("scene_dropped_first_person_"),
                light.strip_prefix("scene_dropped_light_"),
            );
        }
        let before = dropped();
        drop_slot(Dropped::Host);
        let after = dropped();
        let at = |rows: &[(&str, u64)]| rows.iter().find(|r| r.0 == "scene_dropped_first_person_host").unwrap().1;
        assert!(at(&after) > at(&before));
        assert!(after.iter().any(|r| r.0 == "scene_dropped_first_person_loop_plan"));
    }

    #[test]
    fn a_cut_slots_candidate_carries_its_constants_and_its_definitions_gels() {
        let mut m = FlatMemory::new(0, 0x10000);
        let definition = 0x1000;
        m.set_u16(definition + 4, 3).unwrap();
        m.set_u32(definition + engine::light_passes::DEFINITION_GEL, u32::MAX).unwrap();
        m.set_u32(definition + engine::light_passes::DEFINITION_GEL_SHADER, 0x00e1_0007).unwrap();
        let light = super::super::Light {
            handle: 5,
            definition: 6,
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [0.5; 3],
            colour_b: [0.25; 3],
            intensity: 2.0,
            bounds: ([1.0, 2.0, 3.0], 4.0),
            shadow: false,
            weight: 1.0,
            constants: None,
        };
        let constants = frame_scene::LightConstants {
            frame: [0; 36],
            flags: 3,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: true,
        };
        let c = candidate(&m, &light, definition, constants);
        assert_eq!((c.position, c.weight, c.definition_kind), ([1.0, 2.0, 3.0], 1.0, 3));
        assert_eq!((c.gel_bitmap, c.gel_shader), (None, Some(0x00e1_0007)));
        let v = c.constants.unwrap();
        assert_eq!((v.unshadowed, v.shadowed), (constants, constants), "a first-person light casts no shadow");
        assert!(v.projection.is_none());
    }
}
