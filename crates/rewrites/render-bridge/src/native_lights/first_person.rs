//! The first-person lights at `FIRST_PERSON_LIGHTS` (`0x000404b0`) and
//! each slot's record replay at `FIRST_PERSON_LIGHT_RECORDS`
//! (`0x000402b0`; docs/graphics/native-light-gather.md, slice L6): the
//! bodies of `engine::render::light_gather::first_person` make the
//! guest's steps in its order, every callee a guest call, so the layer
//! draws and the walk's builds that tell the first-person layers apart by
//! their caller read the guest's site through `passes::caller`, and the
//! light goes into the first person's view at its first layer draw as
//! under the guest (`native_lights::first_person_layer`). A plan that
//! refuses runs the guest function whole.

use engine::entry::EntryError;
use engine::light_constants::{Call, Fpu};
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light_gather::first_person::{self, SlotRecords};
use engine::render::light_gather::items::Made;
use engine::render::light_gather::{Refusal, typed, typed_tail};
use frame_scene::GroupSite;
use hooks::{Builder, Cx, EntryFuture};
use native_render::light_graph;
use native_render::light_passes::{self, Steps};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use super::{Ro, SceneGroup};
use crate::native_map_work::Stage;

pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::FIRST_PERSON_LIGHTS,
            "native_lights_first_person",
            |cpu, ctx, addr| {
                if addr != points::FIRST_PERSON_LIGHTS.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                let typed_fpu =
                    super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control);
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    first_person::plan(&*mem, cpu.esp, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::FirstPersonLights, why);
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, _) = first_person::run_with(&cx, plan, &mut |mem, p, call| {
                            if !typed_fpu {
                                refused(SceneList::TypedFirstPerson, Refusal::Unsupported);
                                return Ok(None);
                            }
                            try_passes(mem, p, call)
                        })
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

struct Slot {
    globals: typed::Globals,
    constants: frame_scene::LightConstants,
    steps: Steps,
    groups: Vec<SceneGroup>,
    scene_light: frame_scene::SceneLight,
    tail: typed_tail::Tail,
}

impl Slot {
    /// The replay has produced this slot's item lists. Plan the whole slot,
    /// including its output ranges and the final target, before asking the
    /// host to reserve any geometry.
    fn read(mem: &dyn GuestMemory, call: &Call, fpu: Fpu) -> Result<Self, Refusal> {
        let globals =
            typed::plan(mem, call, fpu, typed::Site::FirstPerson).map_err(|error| match error {
                engine::light_constants::Error::Memory(_) => Refusal::Memory,
                _ => Refusal::Unsupported,
            })?;
        let constants = super::constants_of(mem, &globals.light, globals.flags, call.definition)
            .ok_or(Refusal::Layout)?;
        if constants.animated_gel {
            return Err(Refusal::Unsupported);
        }
        let inputs = light_passes::read(&Ro(mem), globals.flags, constants.definition_flags)
            .ok_or(Refusal::Layout)?;
        let steps = light_passes::first_person_steps(&inputs);
        if steps.drawn() != 0 {
            // The accepted slot opens this camera's first-person scene view.
            scene_extract::extract::camera(&Ro(mem)).ok_or(Refusal::Layout)?;
        }
        let tail = typed_tail::plan_layers(mem, steps.drawn()).ok_or(Refusal::Unsupported)?;
        let groups = super::typed_groups::plan(
            mem,
            GroupSite::FirstPerson,
            steps.as_slice(),
            globals.flags,
            constants.definition_flags,
            None,
        )
        .ok_or(Refusal::Unsupported)?;
        let light = super::read_light(mem, call.light, false, f32::from_bits(call.weight))
            .ok_or(Refusal::Layout)?;
        let scene_light = super::scene_light(&light, &inputs);
        Ok(Self {
            globals,
            constants,
            steps,
            groups,
            scene_light,
            tail,
        })
    }
}

fn try_passes(
    mem: &mut dyn GuestMemory,
    plan: &first_person::Plan,
    call: &Call,
) -> Result<Option<first_person::SlotPasses>, EntryError> {
    let slot = match Slot::read(mem, call, plan.fpu) {
        Ok(slot) => slot,
        Err(why) => {
            refused(SceneList::TypedFirstPerson, why);
            return Ok(None);
        }
    };
    let graph = light_graph::Pass::Light {
        site: GroupSite::FirstPerson,
        steps: slot.steps.as_slice(),
        additional: None,
        constants: &slot.constants,
        scissor: None,
        projection: None,
    };
    if !super::typed::submit(
        mem,
        graph,
        &slot.groups,
        &slot.globals,
        &slot.tail,
        Some((
            plan.draw_byte(),
            u8::from(slot.steps.as_slice().last().is_some_and(|step| step.draws)),
        )),
    ) {
        refused(SceneList::TypedFirstPerson, Refusal::Unsupported);
        return Ok(None);
    }

    let drawn = slot.steps.drawn();
    if drawn != 0 {
        // The ordinary loop opens the light at its first drawn layer.
        // The graph has already consumed all layers of this slot.
        super::finish_candidate();
        super::FIRST_PERSON.set(None);
        if let Some(host) = super::HOST
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            host.clear_admission();
        }
        if crate::scene_step::first_person_light(mem, slot.scene_light, slot.steps.as_slice()) {
            crate::scene_step::typed_light_groups(&slot.groups);
        }
    }
    SceneList::TypedFirstPerson.count(SceneListResult::Native);
    Ok(Some(first_person::SlotPasses {
        flags: slot.globals.flags,
        reached: slot
            .steps
            .iter()
            .fold(0, |mask, step| mask | 1 << step.layer),
        drawn,
    }))
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
