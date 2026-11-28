//! The gather's setup in Rust (docs/graphics/native-visibility.md, "The
//! gather's setup"): `0x00132b80`, the caller's store of the active struct
//! and the reset `0x001348e0`, ported by `engine::visibility_setup` and
//! held to the running guest by a byte arm ([`engine::shadow_arm`]).
//!
//! The hook sits on the setup's entry and takes only the gather's call
//! (return address `0x0013212e`, ESI the struct). On a sampled frame the
//! guest's code runs and the shadow buffer is compared at the entry of
//! the producer the gather then calls (`0x00173130`, `0x00132d60` or
//! `0x001733e0`), an observer, before any producer writes; the producer
//! entered must be the one the Rust mode names. Once proven, an unsampled
//! call is Rust's: the writes are applied, the stack drops the setup's
//! return and arguments, and the gather resumes at the reset's return
//! (`0x0013213d`) with the registers the guest leaves there. A refusal
//! (`scene_visibility_setup_refused_<why>`) runs the guest for that call.

use std::sync::Mutex;

use engine::shadow_arm::Plan;
use engine::visibility_setup::{self as vs, Built, Call, arm};
use hooks::Builder;

use super::*;

/// The sampled call in flight: whose, its entry `ESP`, and what Rust built.
struct Pending {
    thread: u32,
    esp: u32,
    vis: u32,
    frame: u32,
    built: Built,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<Pending>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// A map load or a snapshot restore: the arm learns again.
pub(crate) fn new_map() {
    arm::new_map();
    *pending() = None;
}

/// The producer the gather calls for each mode: its point, the gather's
/// return address there, and `ESP` at its entry less the setup's entry
/// `ESP` (the setup's `ret 24h`, the gather's `pop edi`, then its pushes
/// and the call).
struct Producer {
    point: &'static Point,
    mode: u32,
    ret: u32,
    esp: u32,
}

fn producers() -> [Producer; 3] {
    [
        Producer { point: &points::VIEW_FRUSTA_REGIONS, mode: 0, ret: 0x0013_2171, esp: 0x18 },
        Producer { point: &points::VIEW_SPHERE_REGIONS, mode: 1, ret: 0x0013_2160, esp: 0x28 },
        Producer { point: &points::VIEW_PVS_REGIONS, mode: 2, ret: 0x0013_2157, esp: 0x24 },
    ]
}

/// The registers at the reset's return after Rust answered the call
/// whose entry state is `cpu`: the setup's arguments and return popped,
/// EAX and ECX as the reset leaves them, EDX the struct, EDI the mode, and
/// the setup's XMM0, XMM1 and XMM3.
fn resume_state(cpu: &mut CpuAbi, call: &Call, built: &Built) {
    let xmm = |bits: u32| {
        let mut x = [0u8; 16];
        x[..4].copy_from_slice(&bits.to_le_bytes());
        x
    };
    cpu.esp = cpu.esp.wrapping_add(4 + 36);
    cpu.eax = u32::MAX;
    cpu.ecx = 0;
    cpu.edx = call.vis;
    cpu.edi = built.mode;
    cpu.xmm[0] = xmm(call.clip_offset);
    cpu.xmm[3] = xmm(call.radius);
    if let Some(sum) = built.xmm1 {
        cpu.xmm[1] = xmm(sum);
    }
}

fn setup(c: &mut hooks::Call) -> Result<Flow, GuestFault> {
    if c.addr != points::VISIBILITY_SETUP.base {
        return Ok(Flow::Continue);
    }
    let thread = c.ctx.thread_id;
    let stale = pending().take_if(|p| p.thread == thread);
    if let Some(p) = stale {
        // The guest entered no producer after the sampled call.
        arm::compared(vec!["producer"], p.frame, p.vis);
    }
    let runtime = Arc::clone(c.ctx.runtime());
    let mut mem = runtime.memory();
    if mem.u32(c.cpu.esp).ok() != Some(vs::guest::SETUP_RETURN) {
        arm::refuse("caller");
        return Ok(Flow::Continue);
    }
    let frame = mem.u32(FRAME).unwrap_or(0);
    let plan = arm::plan(frame);
    if plan == Plan::Guest {
        return Ok(Flow::Continue);
    }
    let call = match Call::read(&*mem, c.cpu.esp, c.cpu.ecx, c.cpu.ebx) {
        Ok(call) if call.vis == c.cpu.esi => call,
        Ok(_) => {
            arm::refuse("caller");
            return Ok(Flow::Continue);
        }
        Err(why) => {
            arm::refuse(why.name());
            return Ok(Flow::Continue);
        }
    };
    let built = match vs::build(&*mem, &call, c.cpu.mxcsr) {
        Ok(built) => built,
        Err(why) => {
            arm::refuse(why.name());
            return Ok(Flow::Continue);
        }
    };
    if plan == Plan::Compare {
        *pending() = Some(Pending { thread, esp: c.cpu.esp, vis: call.vis, frame, built });
        return Ok(Flow::Continue);
    }
    if !runtime.read_dispatch(|d| d.has_resume(vs::guest::RESET_RETURN)) {
        arm::refuse("dispatch");
        return Ok(Flow::Continue);
    }
    if !built.proposal.writable(&*mem) || built.proposal.apply(&mut *mem).is_err() {
        // `writable` held under the same guard, so `apply` cannot fail
        // part way.
        arm::refuse("unread");
        return Ok(Flow::Continue);
    }
    resume_state(c.cpu, &call, &built);
    arm::answered();
    Ok(Flow::Return(TransferAbi::resume(vs::guest::RESET_RETURN)))
}

/// A producer's entry: compare the sampled call's shadow buffer.
fn compare(o: &hooks::Obs, producer: &Producer) {
    let mut slot = pending();
    let Some(p) = slot.as_ref() else { return };
    if p.thread != o.thread_id || o.cpu.esp != p.esp.wrapping_add(producer.esp) || o.cpu.esi != p.vis {
        return;
    }
    let mem = o.guest();
    if mem.u32(o.cpu.esp).ok() != Some(producer.ret) {
        return;
    }
    let Some(p) = slot.take() else { return };
    drop(slot);
    let mut differing = p.built.proposal.differing(&*mem);
    if producer.mode != p.built.mode && !differing.contains(&"mode") {
        differing.push("mode");
    }
    arm::compared(differing, p.frame, p.vis);
}

/// Places the arm: the setup's entry and an observer at each producer's.
/// With `on` false, the arm compares and never answers.
pub(super) fn install(b: &mut Builder, on: bool) {
    arm::set_on(on);
    b.raw()
        .before(&points::VISIBILITY_SETUP, "native visibility setup", setup)
        .context_only()
        .optional()
        .last();
    for producer in producers() {
        let point = producer.point;
        b.raw()
            .observe_before(point, "compare the visibility setup", move |o| compare(o, &producer))
            .context_only()
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_resumes_with_the_registers_the_reset_leaves() {
        let mut cpu = CpuAbi { esp: 0x1000, esi: 0x0054_7f88, edi: 0xd1, ebx: 2, ..CpuAbi::xbox_thread() };
        let call = Call { vis: 0x0054_7f88, radius: 8f32.to_bits(), clip_offset: 0.5f32.to_bits(), ..Call::default() };
        let built = Built { mode: 1, xmm1: None, ..Built::default() };
        resume_state(&mut cpu, &call, &built);
        assert_eq!((cpu.esp, cpu.eax, cpu.ecx, cpu.edx, cpu.edi), (0x1028, u32::MAX, 0, 0x0054_7f88, 1));
        assert_eq!((cpu.ebx, cpu.esi), (2, 0x0054_7f88), "the setup and the reset keep EBX and ESI");
        assert_eq!(cpu.xmm[0][..4], 0.5f32.to_le_bytes());
        assert_eq!(cpu.xmm[3][..4], 8f32.to_le_bytes());
    }

    #[test]
    fn each_mode_has_one_producer_below_the_setups_frame() {
        let modes: Vec<u32> = producers().iter().map(|p| p.mode).collect();
        assert_eq!(modes, [0, 1, 2]);
        // Each producer's ESP lies under the setup's popped frame (+0x28)
        // plus the gather's `pop edi` (+4), less its pushes and the call.
        let pushes = [4 * 4 + 4, 4, 4 + 4];
        for (p, pushed) in producers().iter().zip(pushes) {
            assert_eq!(p.esp, 0x2c - pushed, "{}", p.point.name);
        }
    }
}
