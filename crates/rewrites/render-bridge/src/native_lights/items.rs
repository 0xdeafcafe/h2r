//! A light's item lists at `SCENE_ITEMS` (`0x00040e90`;
//! docs/graphics/native-light-gather.md, slice L4): in a light's kind the
//! body `engine::render::light_gather::items` makes the call, with the
//! guest's per-part calls for each record it does not take. The main
//! view's kind, and a plan that refuses, run the guest function whole.
//! The passes' own call (`handoff::ITEMS_RETURN`) of a light the frame
//! body handed over then puts that light into the scene (slice L5).

use guest_layout::points;
use engine::render::light_gather::Refusal;
use engine::render::light_gather::items::{self, LightItems, Made};
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use super::handoff;
use crate::native_map_work::Stage;

pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::SCENE_ITEMS,
            "native_lights_items",
            |cpu, ctx, addr| {
                if addr != points::SCENE_ITEMS.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                let runtime = std::sync::Arc::clone(ctx.runtime());
                let (plan, taken) = {
                    let mem = runtime.memory();
                    let ret =
                        engine::render::light_gather::passes::caller(&*mem, cpu.esp).unwrap_or(0);
                    (items::plan(&*mem, cpu.esp, cpu.eax), handoff::take(ret))
                };
                let plan = match plan {
                    Ok(Some(plan)) => plan,
                    // The main view's kind: its list 0 is not this body's.
                    Ok(None) => {
                        if let Some(taken) = &taken {
                            handoff::guest_made(taken, Refusal::Layout);
                        }
                        return original(cpu, ctx, addr);
                    }
                    Err(why) => {
                        refused(SceneList::LightItems, why);
                        if let Some(taken) = &taken {
                            handoff::guest_made(taken, why);
                        }
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, light_items) = items::run(&cx, plan).await?;
                        count(&light_items);
                        // The passes' own call: their light goes into the
                        // scene from the frame body's hand-off and these
                        // lists (slice L5).
                        if let Some(taken) = taken {
                            cx.read(|m| handoff::made(m, &taken, &light_items));
                        }
                        Ok::<_, guest_layout::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
}

/// One count per record that passed the filters: native for a record
/// appended in Rust, the fallback and its reason for one the guest's
/// per-part calls made.
fn count(light_items: &LightItems) {
    for &(_, made) in &light_items.made {
        match made {
            Made::Rust => SceneList::LightItems.count(SceneListResult::Native),
            Made::Guest(why) => refused(SceneList::LightItems, why),
        }
    }
}
