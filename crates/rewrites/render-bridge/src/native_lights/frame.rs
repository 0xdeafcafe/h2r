//! The light frame body at `RENDER_DYNAMIC_LIGHTS` (`0x00031c80`;
//! docs/graphics/native-light-gather.md, slice L2): the plan reads and
//! refuses before any write. The guest function never runs: a refused
//! plan, a stage standing down or no host for the scene's light layers is
//! answered as the cut and counted (`scene_dropped_light_frame_<cause>`). The body's guest calls go through their own hook chains, so
//! the scene-list seams of the geometry flags and the extra caster
//! count their own fallbacks when a step goes to the guest.

use engine::light_constants::Fpu;
use engine::points;
use engine::render::light_gather::Refusal;
use engine::render::light_gather::frame::{self, By, Gathered, LightFrame};
use hooks::{Builder, Cx, EntryFuture};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};
use timeline::native::{SceneList, SceneListResult};

use super::scene_frame::{FrameDropped, drop_frame};
use crate::native_map_work::Stage;

/// Places the body. One `implement` at the point; the observers before
/// and after it (the view, the carried slots, the census) keep running
/// around it.
pub(super) fn install(b: &mut Builder) {
    b.raw()
        .implement(
            &points::RENDER_DYNAMIC_LIGHTS,
            "native_lights_frame",
            |cpu, ctx, addr| {
                if addr != points::RENDER_DYNAMIC_LIGHTS.base {
                    return original(cpu, ctx, addr);
                }
                // The frame is a cut: the guest's body never runs. A frame
                // the scene cannot take draws no light and is counted.
                let cut = crate::cut::cut("dynamic_lights").expect("dynamic_lights is a cut");
                let dropped = if Stage::Lights.standing_down() {
                    Some(FrameDropped::StandingDown)
                } else if !super::scene_path_ready() {
                    Some(FrameDropped::Host)
                } else {
                    None
                };
                if let Some(why) = dropped {
                    drop_frame(why);
                    return crate::cut::answer(cut, cpu, ctx);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    frame::plan(&*mem, cpu.esp, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::LightFrame, why);
                        drop_frame(FrameDropped::Plan);
                        return crate::cut::answer(cut, cpu, ctx);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        // Each drawn light is handed to the scene before
                        // its passes (slice L5).
                        let (ret, light_frame) = frame::run_with(&cx, plan, &mut |g: &Gathered| {
                            super::handoff::handed(g)
                        })
                        .await?;
                        // The view's held run draws before its light
                        // frame returns.
                        cx.write(|m| super::flush_light_run(m));
                        count(&light_frame);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
}

/// The frame's counters: the frame, its visible list, and each light's
/// Rust-made steps. A step given to the guest counts its fallback here
/// only for the volume and the record walks, which have no seam of their
/// own.
fn count(light_frame: &LightFrame) {
    SceneList::LightFrame.count(SceneListResult::Native);
    SceneList::VisibleLights.count(SceneListResult::Native);
    for gathered in light_frame
        .lights
        .iter()
        .filter_map(|l| l.gathered.as_ref())
    {
        match gathered.volume {
            By::Rust => SceneList::LightVolume.count(SceneListResult::Native),
            By::Guest(why) => refused(SceneList::LightVolume, why),
        }
        if gathered.geometry == By::Rust {
            SceneList::LightGeometry.count(SceneListResult::Native);
        }
        if gathered.extra_caster == Some(By::Rust) {
            SceneList::LightExtraCaster.count(SceneListResult::Native);
        }
        for walk in [gathered.clusters, gathered.placements] {
            match walk {
                By::Rust => SceneList::LightRecords.count(SceneListResult::Native),
                By::Guest(why) => refused(SceneList::LightRecords, why),
            }
        }
    }
}

pub(super) fn refused(stage: SceneList, why: Refusal) {
    stage.count(SceneListResult::Fallback);
    stage.count(match why {
        Refusal::Memory => SceneListResult::Memory,
        Refusal::Layout => SceneListResult::Layout,
        Refusal::Unsupported => SceneListResult::Unsupported,
        Refusal::Residency => SceneListResult::Residency,
    });
}

/// The original at `addr`, for a call the body declines.
pub(super) fn original(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
) -> Result<TransferAbi, GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(TransferAbi::fault(event))
}
