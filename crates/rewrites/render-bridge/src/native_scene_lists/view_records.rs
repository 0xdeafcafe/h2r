//! The view records in Rust at `render_game_views` `0x002234e0`
//! (docs/graphics/native-visibility.md, "The view records"): the port is
//! `engine::view_records`, held to the lifted original by `engine`'s
//! `view_records_differential`, and here to the running guest by a byte
//! compare ([`engine::shadow_arm`]).
//!
//! The point takes one implementation. On a sampled frame the guest's
//! body runs, and the shadow buffer is compared at its call of
//! `render_views` (an observer there, before the features that move the
//! cameras), each differing field counted by name. Once proven, an
//! unsampled frame is Rust's: the records, the split globals and the
//! countdowns are written, the guest's frame at `0x00223714` is laid on
//! the stack, and `render_views` is called with the guest's return
//! address, so the body's own tail returns. A refusal, or a dispatch
//! without that entry and return, runs the body for that frame.

use std::sync::Mutex;

use engine::shadow_arm::{ByteArm, Plan};
use engine::view_records::{self as vr, Built, Float};
use hooks::Builder;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use super::*;

/// The arm and the shadow buffer of the sampled call in flight: its entry
/// `ESP` and what Rust built.
struct State {
    arm: ByteArm,
    pending: Option<(u32, u32, Built)>,
    on: bool,
}

static STATE: Mutex<State> = Mutex::new(State { arm: ByteArm::new(), pending: None, on: true });

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()))
}

const PREFIX: &str = "scene_view_records";

/// `render.scene`'s counters of the arm (`scene_view_records_*`).
pub(crate) fn counters() -> Vec<(&'static str, u64)> {
    with(|s| s.arm.counters(PREFIX))
}

/// A map load or a snapshot restore: the arm learns again.
pub(crate) fn new_map() {
    with(|s| {
        s.arm.new_map();
        s.pending = None;
    });
}

/// The guest's frame at the `render_views` call (`0x00223714`) under the
/// entry `esp`: the split local, the four saved registers, the three
/// arguments and the return address. Returns the new `esp`.
fn lay_frame(m: &mut dyn GuestMemory, esp: u32, cpu: &CpuAbi, built: &Built) -> Result<u32, MemFault> {
    let c = &built.call;
    let words = [
        c.args[1],
        cpu.ebx,
        cpu.ebp,
        cpu.esi,
        cpu.edi,
        c.args[2],
        c.args[1],
        c.args[0],
        vr::RENDER_VIEWS_RETURN,
    ];
    let low = esp.wrapping_sub(4 * words.len() as u32);
    m.bytes(low, 4 * words.len())?;
    for (k, w) in words.into_iter().enumerate() {
        m.set_u32(esp.wrapping_sub(4 * (k as u32 + 1)), w)?;
    }
    Ok(low)
}

fn enter_render_views(cpu: &mut CpuAbi, built: &Built, esp: u32) {
    let c = &built.call;
    cpu.eax = c.eax;
    cpu.ecx = c.ecx;
    cpu.edx = c.edx;
    cpu.ebx = c.ebx;
    cpu.esi = c.esi;
    cpu.edi = c.edi;
    cpu.ebp = c.ebp;
    cpu.esp = esp;
    let mut xmm0 = [0u8; 16];
    xmm0[..4].copy_from_slice(&c.xmm0.to_le_bytes());
    cpu.xmm[0] = xmm0;
}

/// Rust answers the call: the writes, the frame, and the call of
/// `render_views`. `None` when it cannot (counted); the body runs.
fn answer(cpu: &mut CpuAbi, ctx: &mut ExecCtx, built: &Built) -> Option<TransferAbi> {
    let runtime = Arc::clone(ctx.runtime());
    if !runtime.read_dispatch(|d| d.has_generated(vr::RENDER_VIEWS) && d.has_resume(vr::RENDER_VIEWS_RETURN)) {
        with(|s| s.arm.refuse("dispatch"));
        return None;
    }
    let mut mem = runtime.memory();
    if !built.proposal.writable(&*mem) {
        with(|s| s.arm.refuse("unread"));
        return None;
    }
    let esp = cpu.esp;
    let low = match lay_frame(&mut *mem, esp, cpu, built) {
        Ok(low) => low,
        Err(_) => {
            with(|s| s.arm.refuse("stack"));
            return None;
        }
    };
    if built.proposal.apply(&mut *mem).is_err() {
        // `writable` held under the same guard: not reached.
        with(|s| s.arm.refuse("unread"));
        return None;
    }
    enter_render_views(cpu, built, low);
    Some(TransferAbi::call(vr::RENDER_VIEWS, vr::RENDER_VIEWS_RETURN))
}

fn body(cpu: &mut CpuAbi, ctx: &mut ExecCtx, addr: u32) -> Result<TransferAbi, GuestFault> {
    if addr == points::RENDER_GAME_VIEWS.base {
        let runtime = Arc::clone(ctx.runtime());
        let frame = runtime.memory().u32(FRAME).unwrap_or(0);
        let plan = with(|s| s.arm.shadow.plan(frame, s.on));
        if plan == Plan::Guest {
            with(|s| {
                s.arm.shadow.unsampled(Plan::Guest);
            });
            return original(cpu, ctx, addr);
        }
        let float = Float { mxcsr: cpu.mxcsr, x87_control: cpu.x87.control, x87_tag: cpu.x87.tag };
        let built = vr::build(&*runtime.memory(), float);
        match (plan, built) {
            (_, Err(why)) => with(|s| s.arm.refuse(why.name())),
            (Plan::Answer, Ok(built)) => {
                if let Some(t) = answer(cpu, ctx, &built) {
                    with(|s| {
                        s.arm.shadow.unsampled(Plan::Answer);
                    });
                    return Ok(t);
                }
            }
            (_, Ok(built)) => with(|s| s.pending = Some((cpu.esp, frame, built))),
        }
    }
    original(cpu, ctx, addr)
}

/// The sampled call reaches `render_views`: compare the shadow buffer.
fn compare(o: &hooks::Obs) {
    let Some((esp, frame, built)) = with(|s| s.pending.take()) else { return };
    let mem = o.guest();
    let ours = o.cpu.esp == esp.wrapping_sub(36) && mem.u32(o.cpu.esp).ok() == Some(vr::RENDER_VIEWS_RETURN);
    with(|s| {
        if !ours {
            s.arm.refuse("stale");
            return;
        }
        let on = s.on;
        s.arm.compare(&built.proposal, &*mem, on, || format!("frame {frame}"));
    });
}

fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, addr: u32) -> Result<TransferAbi, GuestFault> {
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

/// Places the arm: the body at `RENDER_GAME_VIEWS` and the comparison at
/// `RENDER_VIEWS`'s entry, an observer, so it runs before the features
/// whose hooks there move the cameras. With `on` false, the arm compares
/// and never answers.
pub(super) fn install(b: &mut Builder, on: bool) {
    with(|s| s.on = on);
    b.raw().implement(&points::RENDER_GAME_VIEWS, "native_view_records", body).optional();
    b.raw().observe_before(&points::RENDER_VIEWS, "compare the view records", compare).optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn the_frame_at_the_call_is_the_guests() {
        let mut m = FakeGuest::new();
        let esp = 0x0010_0100;
        m.map(esp - 0x40, 0x40);
        let cpu = CpuAbi { ebx: 0xb, ebp: 0xbb, esi: 0x51, edi: 0xd1, ..CpuAbi::xbox_thread() };
        let built = Built {
            proposal: Default::default(),
            call: vr::RenderViewsCall {
                eax: 2, ecx: 3, edx: 2, ebx: 7, esi: 0, edi: 0, ebp: 1, xmm0: 0, args: [1, 0, vr::RECORDS],
            },
        };
        let low = lay_frame(&mut m, esp, &cpu, &built).unwrap();
        assert_eq!(low, esp - 36);
        let words: Vec<u32> = (0..9).map(|k| m.u32(low + 4 * k).unwrap()).collect();
        assert_eq!(words, [vr::RENDER_VIEWS_RETURN, 1, 0, vr::RECORDS, 0xd1, 0x51, 0xbb, 0xb, 0]);
        let mut c = cpu;
        enter_render_views(&mut c, &built, low);
        assert_eq!((c.eax, c.ecx, c.ebx, c.ebp, c.esp), (2, 3, 7, 1, low));
    }
}
