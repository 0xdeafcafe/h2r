//! One dynamic light at `0x00031170`. With the walk on, the entry runs the
//! lifted original whole. When a renderer can take typed passes (L8) it
//! first keeps its plan; the body's own call to the constants
//! (`points::LIGHT_CONSTANTS`, return word `site::CONSTANTS`, after the
//! item lists and the shadow byte) then offers the complete passes. A
//! commit resumes the guest at `site::VIEW_TARGET`, whose epilogue restores
//! the saved registers and returns; a refusal runs the constants and the
//! rest of the body untouched. With the walk off (opted out, or render
//! timing) L5 runs the body in Rust, its guest calls naming their
//! original return sites through `passes::caller`. Refusal at the entry
//! itself runs the entire original function.

use std::cell::Cell;
use std::sync::Arc;

use engine::light_constants::Fpu;
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light_gather::{Refusal, passes};
use hooks::{Builder, Call, Cx, EntryFuture, Flow};
use runtime::TransferAbi;
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use crate::native_map_work::{Stage, bsp_generation};

/// The entry plan of the invocation running the whole guest body, the
/// return address it was called with, and the map generation it began
/// in (a load or a restored snapshot moves it on).
struct Pending {
    plan: passes::Plan,
    ret: u32,
    generation: u64,
}

thread_local! {
    /// Set only by the entry that runs the guest body, cleared by every
    /// entry and taken by the first constants call that follows. The
    /// thread that runs the guest owns it; another thread never sees it.
    static PENDING: Cell<Option<Pending>> = const { Cell::new(None) };
}

pub(super) fn forget() {
    PENDING.take();
}

fn guest_pass(why: Refusal) {
    refused(SceneList::TypedLightPasses, why);
    refused(SceneList::LightPasses, why);
}

pub(super) fn install(b: &mut Builder) {
    let walk = super::walk::on(b.switches());
    b.raw()
        .implement(
            &points::RENDER_LIGHT_PASSES,
            "native_lights_passes",
            move |cpu, ctx, addr| {
                // Any earlier invocation has passed its constants call.
                forget();
                if addr != points::RENDER_LIGHT_PASSES.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                let typed_fpu =
                    super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control);
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let (plan, ret) = {
                    let runtime = Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    (
                        passes::plan(&*mem, cpu.esp, cpu.eax, cpu.ecx, fpu),
                        GuestMemory::u32(&*mem, cpu.esp),
                    )
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        if walk {
                            guest_pass(why);
                        } else {
                            refused(SceneList::LightPasses, why);
                        }
                        return original(cpu, ctx, addr);
                    }
                };
                if walk {
                    match ret {
                        Ok(ret) if typed_fpu && super::typed::host_ready() => {
                            PENDING.set(Some(Pending {
                                plan,
                                ret,
                                generation: bsp_generation(),
                            }));
                        }
                        Ok(_) => guest_pass(Refusal::Unsupported),
                        Err(_) => guest_pass(Refusal::Memory),
                    }
                    return original(cpu, ctx, addr);
                }
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, _) = passes::run_with(&cx, plan, |_, _| {
                            refused(SceneList::TypedLightPasses, Refusal::Unsupported);
                            Ok(None)
                        })
                        .await?;
                        SceneList::LightPasses.count(SceneListResult::Native);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::LIGHT_CONSTANTS,
            "native_lights_typed_passes",
            |c| Ok(at_constants(c).map_or(Flow::Continue, Flow::Return)),
        )
        .context_only();
}

/// The constants' entry. Acts only on the whole guest body's own call
/// from the kept invocation; `None` leaves every register and word as the
/// guest left them, and the constants run.
fn at_constants(c: &mut Call<'_>) -> Option<TransferAbi> {
    let rt = Arc::clone(c.ctx.runtime());
    let call = {
        let mem = rt.memory();
        if !Stage::Lights.standing_down()
            && passes::caller(&*mem, c.cpu.esp) == Some(passes::site::CONSTANTS)
        {
            super::slot_visibility::collect(&*mem, c.cpu.edx);
        }
        passes::ConstantsCall::read(&*mem, c.cpu.esp).ok()??
    };
    let pending = PENDING.take()?;
    let p = &pending.plan;
    if pending.generation != bsp_generation() || Stage::Lights.standing_down() {
        guest_pass(Refusal::Unsupported);
        return None;
    }
    let regs = [c.cpu.eax, c.cpu.ecx, c.cpu.edx];
    let same_call = {
        let mem = rt.memory();
        call.matches(&*mem, p, regs).unwrap_or(false)
            && GuestMemory::u32(&*mem, p.entry_esp).ok() == Some(pending.ret)
    };
    // The bounds port runs under the entry's float modes, which the
    // constants would also see, with an empty x87 stack.
    let cpu = &*c.cpu;
    if !same_call
        || (cpu.mxcsr ^ p.fpu.mxcsr) & !0x3f != 0
        || cpu.x87.control != p.fpu.x87_control
        || !super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control)
    {
        guest_pass(Refusal::Layout);
        return None;
    }
    if !rt.read_dispatch(|dispatch| dispatch.has_resume(passes::site::VIEW_TARGET)) {
        guest_pass(Refusal::Unsupported);
        return None;
    }
    let issued = {
        let mut mem = rt.memory();
        super::typed::dynamic(&mut *mem, p)
    };
    let eax = match issued {
        Ok(eax) => eax,
        Err(why) => {
            guest_pass(why);
            return None;
        }
    };
    SceneList::TypedLightPasses.count(SceneListResult::Native);
    SceneList::LightPasses.count(SceneListResult::Native);
    // As the tail leaves them at the view target's return site: EAX what
    // the target call left, EBX zero (`0x000314d4`), whose low byte and
    // word the epilogue stores into the pass flags.
    c.cpu.eax = eax;
    c.cpu.ebx = 0;
    c.cpu.esp = call.view_target_esp();
    Some(TransferAbi::resume(passes::site::VIEW_TARGET))
}
