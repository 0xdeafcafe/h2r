//! One dynamic light at `0x00031170` (docs/graphics/native-lights.md, "The
//! light frame under the cut"). Its only caller is the cut light frame's
//! Rust body ([`super::frame`]), and the guest's body never runs: the
//! passes make their item lists, put the light and its groups into the
//! scene ([`super::scene_frame`]) and return (`ret 0x10`). A light the
//! scene cannot take is dropped for the frame and counted by cause.

use std::sync::Arc;

use engine::entry::Ret;
use engine::light_constants::Fpu;
use engine::points;
use engine::render::light_gather::{Refusal, passes};
use hooks::{Builder, Call, Cx, EntryFuture, Flow};
use runtime::TransferAbi;
use timeline::native::{SceneList, SceneListResult};

use super::frame::original;
use crate::native_map_work::Stage;

/// The passes' `ret 0x10`.
const POP: u16 = 0x10;

pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::RENDER_LIGHT_PASSES,
            "native_lights_passes",
            move |cpu, ctx, addr| {
                if addr != points::RENDER_LIGHT_PASSES.base {
                    return original(cpu, ctx, addr);
                }
                if Stage::Lights.standing_down() {
                    return ret(cpu, ctx);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    passes::plan(&*mem, cpu.esp, cpu.eax, cpu.ecx, fpu)
                };
                scene_passes(cpu, ctx, plan)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::LIGHT_CONSTANTS,
            "native_lights_constants_observer",
            |c| {
                at_constants(c);
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

/// Returns from the passes as `0x00031170` does, with nothing made.
fn ret(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
) -> Result<TransferAbi, runtime::GuestFault> {
    let guest = ctx.memory();
    cpu.eax = 0;
    guest.ret(cpu, u32::from(POP))
}

/// The passes of a light of the cut frame ([`super::scene_frame`]): the
/// item lists, then the light's groups into the scene, and the return
/// (`ret 0x10`). A refused plan returns at once; a light whose groups do
/// not make, or that the host does not draw, returns after its item lists
/// ([`after_lists`]). Both are counted, and neither draws.
fn scene_passes(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    plan: Result<passes::Plan, Refusal>,
) -> Result<TransferAbi, runtime::GuestFault> {
    let plan = match plan {
        Ok(plan) => plan,
        Err(why) => super::scene_frame::refused(why),
    };
    hooks::entry::start_render(cpu, ctx, points::RENDER_LIGHT_PASSES.base, move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let (ret, _) = passes::run_with(&cx, plan, |mem, p| {
                Ok(Some(after_lists(super::scene_frame::light(mem, p))))
            })
            .await?;
            SceneList::LightPasses.count(SceneListResult::Native);
            Ok::<_, engine::entry::EntryError>(ret)
        })
    })
}

/// What the cut passes do once the item lists are made: return
/// (`ret 0x10`) with nothing drawn by the guest; the scene took the light
/// ([`super::scene_frame::light`] panics on one it cannot take).
pub(super) fn after_lists(_groups: usize) -> (Ret, passes::LightPasses) {
    (Ret::new(0, POP), passes::LightPasses::default())
}

/// The constants' entry, called by the first-person lights
/// (`0x0004057e`): the hand-off and the slot visibility observe the call.
/// Every register and word stays as the guest left them.
fn at_constants(c: &mut Call<'_>) {
    let rt = Arc::clone(c.ctx.runtime());
    let mem = rt.memory();
    if !Stage::Lights.standing_down()
        && passes::caller(&*mem, c.cpu.esp) == Some(passes::site::CONSTANTS)
    {
        if let Some((entry, serial)) = super::handoff::at_constants(c.cpu.esp) {
            super::observe(&*mem, entry, serial);
        }
        super::slot_visibility::collect(&*mem, c.cpu.edx);
    }
}

#[cfg(test)]
mod tests {
    use super::{POP, after_lists};

    /// A light the scene takes returns after its item lists, drawing
    /// nothing: the guest's remaining passes never run.
    #[test]
    fn a_light_the_scene_takes_returns_after_its_lists_and_the_guest_draws_nothing() {
        assert_eq!(after_lists(3).0.pop, POP, "in the scene: return");
        assert_eq!(after_lists(0).0.pop, POP, "no drawn layer: return");
    }
}
