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

use super::frame::{original, refused};
use super::scene_frame::{FrameDropped, drop_frame};
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
                    drop_frame(FrameDropped::StandingDown);
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
    let mut guest = ctx.memory();
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
        Err(why) => {
            refused(SceneList::LightPasses, why);
            super::scene_frame::refused();
            return ret(cpu, ctx);
        }
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
/// (`ret 0x10`) with nothing drawn by the guest, whether the scene took the
/// light or dropped it ([`super::scene_frame::light`] counts a drop).
pub(super) fn after_lists(
    _taken: Result<usize, super::scene_frame::Dropped>,
) -> (Ret, passes::LightPasses) {
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
    use crate::native_lights::GroupRefusal;
    use crate::native_lights::scene_frame::{self, Dropped, FrameDropped, GROUP_NAMES, drop_frame, drop_light};
    use scene_extract::light_groups::Cause;

    fn counter(name: &str) -> u64 {
        scene_frame::counters().into_iter().find(|(k, _)| *k == name).map_or(0, |(_, v)| v)
    }

    /// A light the scene takes returns after its item lists, drawing
    /// nothing; so does one it drops: the guest's remaining passes never
    /// run.
    #[test]
    fn a_light_the_scene_cannot_take_returns_after_its_lists_and_the_guest_draws_nothing() {
        assert_eq!(after_lists(Ok(3)).0.pop, POP, "in the scene: return");
        assert_eq!(after_lists(Ok(0)).0.pop, POP, "no drawn layer: return");
        for dropped in [
            Dropped::Unread,
            Dropped::Constants,
            Dropped::Projection,
            Dropped::Group(GroupRefusal::Entry(Cause::Material)),
            Dropped::Light,
            Dropped::AdditionalCasters,
            Dropped::Host,
        ] {
            assert_eq!(after_lists(Err(dropped)).0.pop, POP, "{dropped:?}: return");
        }
    }

    /// Each dropped light is counted by cause, a group's also by its
    /// refusal, and a whole frame by its own cause; no row says kept.
    #[test]
    fn a_dropped_light_is_counted_by_cause_and_group_refusal() {
        let (group, state) = (counter("scene_dropped_light_group"), counter("scene_dropped_light_group_entry_state"));
        drop_light(Dropped::Group(GroupRefusal::Entry(Cause::State)));
        assert!(counter("scene_dropped_light_group") > group);
        assert!(counter("scene_dropped_light_group_entry_state") > state);
        for why in [FrameDropped::StandingDown, FrameDropped::Host, FrameDropped::Plan, FrameDropped::Passes] {
            let before = counter(why.name());
            drop_frame(why);
            assert!(counter(why.name()) > before, "{why:?}");
        }
        assert!(scene_frame::counters().iter().all(|(k, _)| !k.contains("kept")));
        for (name, refusal) in GROUP_NAMES.iter().zip(GroupRefusal::NAMES) {
            assert_eq!(*name, format!("scene_dropped_light_group_{refusal}"));
        }
    }
}
