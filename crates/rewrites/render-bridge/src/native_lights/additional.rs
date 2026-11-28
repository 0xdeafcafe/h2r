//! The additional-caster loop on a light whose passes still run through
//! L5. Its entry observers see the call even when no suffix draws.

use guest_layout::entry::Ret;
use guest_layout::points;
use engine::render::light_gather::passes::additional;
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use crate::native_map_work::Stage;

pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::ADDITIONAL_SHADOW_CASTERS,
            "native_lights_additional_casters",
            |cpu, ctx, addr| {
                if addr != points::ADDITIONAL_SHADOW_CASTERS.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                let plan = additional::empty(&*ctx.runtime().memory());
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::LightAdditionalCasters, why);
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        cx.write(|m| plan.write(m))?;
                        SceneList::LightAdditionalCasters.count(SceneListResult::Native);
                        Ok(Ret::new(plan.eax(), 0))
                    })
                })
            },
        )
        .context_only();
}
