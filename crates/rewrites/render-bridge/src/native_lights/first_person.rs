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

use engine::light_constants::Fpu;
use engine::points;
use engine::render::light_gather::first_person::{self, SlotRecords};
use engine::render::light_gather::items::Made;
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
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
                        let (ret, _) = first_person::run(&cx, plan).await?;
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
