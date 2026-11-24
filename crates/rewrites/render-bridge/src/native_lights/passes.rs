//! One light's passes at `RENDER_LIGHT_PASSES` (`0x00031170`;
//! docs/graphics/native-light-gather.md, slice L5): the body
//! `engine::render::light_gather::passes` makes the guest's steps in its
//! order, with the item lists, the constants, the projection and each
//! layer's target, stencil, draw and end as guest calls, so every hook on
//! them runs as under the guest. A hook that tests the return address of
//! one of those calls reads it through `passes::caller`. A plan that
//! refuses runs the guest function whole.

use engine::light_constants::Fpu;
use engine::points;
use engine::render::light_gather::passes;
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use crate::native_map_work::Stage;

pub(super) fn install(b: &mut Builder) {
    let walk = super::walk::on(b.switches());
    b.raw()
        .implement(
            &points::RENDER_LIGHT_PASSES,
            "native_lights_passes",
            move |cpu, ctx, addr| {
                if addr != points::RENDER_LIGHT_PASSES.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                let typed_fpu = super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control);
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    passes::plan(&*mem, cpu.esp, cpu.eax, cpu.ecx, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::LightPasses, why);
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, _) = passes::run_with(&cx, plan, |mem, p| {
                            if !walk || !typed_fpu || Stage::Lights.standing_down() {
                                refused(SceneList::TypedLightPasses, engine::render::light_gather::Refusal::Unsupported);
                                return Ok(None);
                            }
                            super::typed::dynamic(mem, p)
                        }).await?;
                        SceneList::LightPasses.count(SceneListResult::Native);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
}
