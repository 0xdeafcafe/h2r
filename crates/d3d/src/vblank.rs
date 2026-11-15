//! Delivering the vertical-blank callback to the guest.
//!
//! Halo registers a callback at device `+0x1DB8` (`0x12b093`, the inlined
//! `SetVerticalBlankCallback`). On the Xbox the GPU interrupt calls it once
//! per vblank as `cdecl callback(D3DVBLANKDATA *)`, where `D3DVBLANKDATA` is
//! `{VBlank, Swap, Flags}`. Halo's (`0x12b2a0`) calls `0x14280`. That
//! function bumps the 64-bit counter at `0x485AB0` and, when `Swap` changed
//! (compared with `[0x55E6B8]`), records the counter and swap at
//! `0x485AB8`/`0x55E6B8`. The frame wait `0x12b2e0` spins on that counter
//! (`0x12b330..0x12b342`) until it reaches `[0x4E6398] - 1`. Nothing else
//! advances it, so without an interrupt the spin never ends.
//!
//! Delivery on the logical clock (the faithful one): [`clocked_vblanks`]
//! is a handler for a kernel periodic source (`kernel::periodic`, 60 Hz).
//! The kernel runs it like the GPU interrupt's DPC, at DISPATCH_LEVEL on
//! its DPC stack between thread slices, so the vblank count follows logical
//! time and a guest spinning on the counter (`0x12b330`) sees it advance
//! once its work has carried the clock to the next deadline. With it, the
//! frame wait override in [`Delivery::Clock`] mode yields until the count
//! arrives instead of synthesising vblanks, so the wait costs no work and
//! the clock jumps to the vblank when nothing else is runnable.
//!
//! Without a clock source the callback runs at one of two safe points,
//! both guest-visible returns from an override, so no guest code is
//! interrupted mid-instruction:
//!
//! * **After `Swap`** (always installed by [`crate::register`]): `Swap`
//!   counts [`Device::set_vblanks_per_swap`] vblanks (default 0). When the
//!   `Swap` override returns, the glue calls the callback once for each
//!   ([`deliver_then_return`]): the record is written below the caller's
//!   stack pointer, its address is pushed as the cdecl argument, and the
//!   callback is entered through `ExecCtx::call_guest`. After the last one,
//!   every register is restored to its value at `Swap`'s return (`EAX` =
//!   `Swap`'s result), and execution returns to `Swap`'s caller.
//! * **In the frame wait** (opt-in, [`register_frame_wait`] with
//!   [`Delivery::Inline`]): an override of `0x12b2e0` delivers vblanks until
//!   the counter reaches the target and then produces the original's exit
//!   effects ([`FrameWait`]). No logical time passes for them, so frame time
//!   (`0x12b0e0` divides the vblanks since the last frame by 60) and the
//!   kernel clock drift apart; pacing to real time is optional.
//!
//! [`Device::set_vblanks_per_swap`]: crate::Device::set_vblanks_per_swap

use std::collections::VecDeque;
use std::time::Duration;

use runtime::{
    CallConvention, CpuAbi, Dispatch, ExecCtx, Guest, GuestFault, GuestInterrupt, TransferAbi,
};

use crate::abi::SharedDevice;
use crate::backend::Backend;
use crate::device::VBlank;
use crate::events::HleStop;

/// Calls the guest callback for each of `queue` in order, then restores
/// `saved` and returns to `ret_pc`.
///
/// `saved` is the CPU as the caller should see it after the override:
/// arguments popped and `EAX` set. Each callback sees `ESP = saved.esp -
/// 16`. At `[ESP]` is the record pointer, which points at the record at
/// `saved.esp - 12`.
pub fn deliver_then_return(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut queue: VecDeque<VBlank>,
    saved: CpuAbi,
    ret_pc: u32,
) -> Result<TransferAbi, GuestFault> {
    let Some(v) = queue.pop_front() else {
        *cpu = saved;
        return Ok(TransferAbi::ret(ret_pc));
    };
    let runtime = std::sync::Arc::clone(ctx.runtime());
    let mut guard = runtime.memory();
    let esp = enter_callback(cpu, &mut guard, &saved, v)?;
    ctx.call_guest(cpu, &mut guard, v.callback, move |cpu, ctx| {
        // cdecl: the callback leaves its argument for the caller to pop;
        // the restore below drops it with the rest of the frame.
        debug_assert_eq!(cpu.esp, esp);
        deliver_then_return(cpu, ctx, queue, saved, ret_pc)
    })
}

/// Writes the record and argument below `saved.esp` and points `cpu` at
/// them. Returns the `ESP` the callback starts with (after its return
/// address is popped).
fn enter_callback(
    cpu: &mut CpuAbi,
    guest: &mut runtime::Guest,
    saved: &CpuAbi,
    v: VBlank,
) -> Result<u32, GuestFault> {
    let record = saved.esp.wrapping_sub(12);
    for (i, w) in v.record.iter().enumerate() {
        guest.set_u32(record.wrapping_add(4 * i as u32), *w)?;
    }
    let esp = saved.esp.wrapping_sub(16);
    guest.set_u32(esp, record)?;
    *cpu = *saved;
    cpu.esp = esp;
    Ok(esp)
}

/// The engine frame wait `0x12b2e0` and the globals it touches (all 64-bit
/// little-endian except the two flags).
///
/// The original (no arguments, plain `ret`) works like this:
///
/// 1. `[entry_count] = [counter] + 1`.
/// 2. Spin while `[counter] < [target] - 1` (signed 64-bit).
/// 3. `n = [counter] + 1`; `[exit_count] = n`; `d = n - [target]`.
/// 4. `EAX = 0` if `d < 0`, else `min(d, 0x7FFF)`. `ECX` is the high word
///    of `d`. `EDX` is the low word of the counter at the last check.
/// 5. Byte `[late_flag] = (word [frame_gate] > 0 && AX == 0)`.
///
/// `EBX`, `ESI`, `EDI` and `EBP` are preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameWait {
    /// The function overridden.
    pub entry: u32,
    /// The vblank counter the callback increments (`0x485AB0`).
    pub counter: u32,
    /// The counter value the wait is for (`0x4E6398`).
    pub target: u32,
    /// Written with the counter + 1 on entry (`0x4E63A8`).
    pub entry_count: u32,
    /// Written with the counter + 1 on exit (`0x4E63B0`).
    pub exit_count: u32,
    /// Signed word read on exit (`0x4E63B8`).
    pub frame_gate: u32,
    /// Byte written on exit (`0x4E63BC`).
    pub late_flag: u32,
    /// Vblanks one wait may deliver before it stops (a guard against a
    /// target far ahead of the counter).
    pub max_vblanks: u32,
    /// Real time per delivered vblank (`None`: no pacing, as fast as the
    /// guest runs). [`Delivery::Inline`] only.
    pub pace: Option<Duration>,
    /// Where the vblanks the wait needs come from.
    pub delivery: Delivery,
}

/// Where [`register_frame_wait`] gets its vblanks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The override calls the callback itself, at once, with no logical
    /// time passing.
    Inline,
    /// A clocked source ([`clocked_vblanks`] on a kernel periodic source)
    /// delivers them; the override calls `NtYieldExecution` until the
    /// counter arrives. It stops after [`MAX_IDLE_YIELDS`] yields in a row
    /// see no vblank (no source installed).
    Clock,
}

/// Yields in a row without a vblank after which a [`Delivery::Clock`] wait
/// stops. Each yield gives every other ready thread a slice first, or lets
/// the clock jump to the next deadline, so a running source delivers well
/// within this.
pub const MAX_IDLE_YIELDS: u32 = 100_000;

/// `NtYieldExecution`, the kernel export the clocked frame wait calls.
const NT_YIELD_EXECUTION: u32 = 238;

/// Diagnostic name of the clocked vblank call.
pub const VBLANK_SOURCE: &str = "D3D vertical-blank callback";

/// A kernel periodic-source handler (`kernel::periodic::Handler`, boxed by
/// the caller) that delivers the vertical blank on logical time: it counts
/// the vblank ([`crate::Device::vblank`], plus any `missed` periods the
/// source dropped from a debt over `kernel::periodic::MAX_DEBT_SECONDS`,
/// [`crate::Device::skip_vblanks`]), writes
/// `D3DVBLANKDATA` `{VBlank, Swap, Flags}` into the kernel's scratch area
/// and returns the cdecl call `callback(&record)` for the callback at
/// `+0x1DB8`. No callback registered (or no device yet): nothing to call,
/// but the vblank is still counted.
///
/// ```ignore
/// kernel.add_periodic(kernel::periodic::Source {
///     name: "vblank",
///     rate: kernel::periodic::Rate::HZ_60,
///     max_catch_up: 4,
///     handler: Box::new(d3d::vblank::clocked_vblanks(device.clone())),
/// })?;
/// ```
pub fn clocked_vblanks<B>(
    device: SharedDevice<B>,
) -> impl FnMut(&mut Guest, u32, u64) -> Result<Option<GuestInterrupt>, GuestFault> + Send + 'static
where
    B: Backend + Send + 'static,
{
    move |guest, scratch, missed| {
        let poisoned = GuestFault {
            addr: scratch,
            len: 12,
            access: runtime::guest::Access::Write,
            reason: runtime::guest::FaultReason::InvalidOperation,
        };
        let v = {
            let mem = crate::runtime_glue::GuestAdapter::new(guest);
            let mut d = device.lock().map_err(|_| poisoned.clone())?;
            d.skip_vblanks(u32::try_from(missed).unwrap_or(u32::MAX));
            match d.vblank(&mem) {
                Ok(v) => v,
                Err(_) => return Err(mem.take_fault().unwrap_or(poisoned)),
            }
        };
        let Some(v) = v else {
            return Ok(None);
        };
        for (i, w) in v.record.iter().enumerate() {
            guest.set_u32(scratch.wrapping_add(4 * i as u32), *w)?;
        }
        Ok(Some(GuestInterrupt {
            target: v.callback,
            args: vec![scratch],
            convention: CallConvention::Cdecl,
            thread: None,
            coalesce: false,
            name: VBLANK_SOURCE,
        }))
    }
}

impl FrameWait {
    /// Halo 2's frame wait, unpaced, at most 600 vblanks (10 s) per wait.
    pub const HALO2: FrameWait = FrameWait {
        entry: 0x0012_b2e0,
        counter: 0x0048_5AB0,
        target: 0x004E_6398,
        entry_count: 0x004E_63A8,
        exit_count: 0x004E_63B0,
        frame_gate: 0x004E_63B8,
        late_flag: 0x004E_63BC,
        max_vblanks: 600,
        pace: None,
        delivery: Delivery::Inline,
    };

    /// [`FrameWait::HALO2`] waiting for a clocked source instead.
    pub const HALO2_CLOCKED: FrameWait = FrameWait {
        delivery: Delivery::Clock,
        ..FrameWait::HALO2
    };

    /// NTSC pacing: one vblank every 1/60 s.
    pub const NTSC_VBLANK: Duration = Duration::from_nanos(16_683_333);
}

/// One wait in progress.
#[derive(Clone, Copy)]
struct Wait {
    fw: FrameWait,
    /// CPU after the wait's own frame is popped (return address gone).
    saved: CpuAbi,
    ret_pc: u32,
    delivered: u32,
    last: Option<i64>,
    /// Clocked: yields since the counter last moved.
    idle: u32,
}

/// Installs the opt-in frame-wait override at `fw.entry`. Stops go through
/// `on_stop`, as in [`crate::runtime_glue::register_with`].
///
/// The wait stops (rather than spinning forever) when:
///
/// * no callback is registered (the device is not created or `+0x1DB8` is 0);
/// * a callback returned without advancing the counter (inline), or
///   [`MAX_IDLE_YIELDS`] yields passed without a vblank (clocked);
/// * more than `max_vblanks` vblanks would be needed.
pub fn register_frame_wait<B>(
    dispatch: &mut Dispatch,
    device: SharedDevice<B>,
    fw: FrameWait,
    on_stop: crate::runtime_glue::StopHook,
) where
    B: Backend + Send + 'static,
{
    const NAME: &str = "engine frame wait (0x12b2e0)";
    let _ = dispatch.register_override(fw.entry, NAME, move |cpu, ctx, _| {
        let ret_pc = ctx.memory().u32(cpu.esp)?;
        let mut saved = *cpu;
        saved.esp = cpu.esp.wrapping_add(4);
        let start = read_i64(ctx, fw.counter)?;
        write_i64(ctx, fw.entry_count, start.wrapping_add(1))?;
        let wait = Wait {
            fw,
            saved,
            ret_pc,
            delivered: 0,
            last: None,
            idle: 0,
        };
        step(cpu, ctx, wait, device.clone(), on_stop.clone(), NAME)
    });
}

fn step<B>(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut w: Wait,
    device: SharedDevice<B>,
    on_stop: crate::runtime_glue::StopHook,
    name: &'static str,
) -> Result<TransferAbi, GuestFault>
where
    B: Backend + Send + 'static,
{
    let fw = w.fw;
    let counter = read_i64(ctx, fw.counter)?;
    let target = read_i64(ctx, fw.target)?;
    if counter >= target.wrapping_sub(1) {
        return finish(cpu, ctx, &w, counter, target);
    }
    let stop = |ctx: &mut ExecCtx, detail: String| {
        let error = crate::events::D3dError::unimplemented(name, detail);
        let detail = error.to_string();
        if let Ok(mut d) = device.lock() {
            d.note_stop(HleStop {
                entry: fw.entry,
                name,
                error: detail.clone(),
            });
        }
        let id = on_stop(
            ctx,
            &crate::runtime_glue::Stop {
                entry: fw.entry,
                name,
                error: &error,
                detail: &detail,
            },
        );
        Ok(TransferAbi::stop(id))
    };
    if fw.delivery == Delivery::Clock {
        let short = target.wrapping_sub(1).wrapping_sub(counter);
        if w.last.is_none() && short > fw.max_vblanks as i64 {
            return stop(
                ctx,
                format!(
                    "the counter ({counter}) is {short} vblanks short of {}, more than {} in one wait",
                    target.wrapping_sub(1),
                    fw.max_vblanks
                ),
            );
        }
        if w.last == Some(counter) {
            w.idle += 1;
        } else {
            w.idle = 0;
            w.last = Some(counter);
        }
        if w.idle >= MAX_IDLE_YIELDS {
            return stop(
                ctx,
                format!(
                    "{MAX_IDLE_YIELDS} yields without a vblank (counter {counter}, waiting for {}): is a clocked vblank source installed?",
                    target.wrapping_sub(1)
                ),
            );
        }
        let callback = {
            let runtime = std::sync::Arc::clone(ctx.runtime());
            let mut guard = runtime.memory();
            let mem = crate::runtime_glue::GuestAdapter::new(&mut guard);
            device.lock().expect("device lock").vblank_callback(&mem)
        };
        match callback {
            Ok(Some(_)) => {}
            Ok(None) => {
                return stop(
                    ctx,
                    format!(
                        "the frame wait would spin forever: no vblank callback is registered (counter {counter}, waiting for {})",
                        target.wrapping_sub(1)
                    ),
                );
            }
            Err(e) => return stop(ctx, e.to_string()),
        }
        // Give the CPU up: other threads run, or the clock moves to the
        // next deadline, whose vblank the kernel delivers before this
        // thread's next slice.
        let runtime = std::sync::Arc::clone(ctx.runtime());
        let mut guard = runtime.memory();
        *cpu = w.saved;
        let token = runtime::kernel::function_token(NT_YIELD_EXECUTION);
        return ctx.call_guest(cpu, &mut guard, token, move |cpu, ctx| {
            step(cpu, ctx, w, device, on_stop, name)
        });
    }
    if w.last == Some(counter) {
        return stop(
            ctx,
            format!(
                "the vblank callback returned without advancing the counter at {:#010x} ({counter}; waiting for {})",
                fw.counter,
                target.wrapping_sub(1)
            ),
        );
    }
    if w.delivered >= fw.max_vblanks {
        return stop(
            ctx,
            format!(
                "{} vblanks delivered and the counter ({counter}) is still short of {}",
                w.delivered,
                target.wrapping_sub(1)
            ),
        );
    }
    let v = {
        let runtime = std::sync::Arc::clone(ctx.runtime());
        let mut guard = runtime.memory();
        let mem = crate::runtime_glue::GuestAdapter::new(&mut guard);
        let mut d = device.lock().expect("device lock");
        match d.vblank(&mem) {
            Ok(v) => v,
            Err(e) => {
                drop(d);
                return stop(ctx, e.to_string());
            }
        }
    };
    let Some(v) = v else {
        return stop(
            ctx,
            format!(
                "the frame wait would spin forever: no vblank callback is registered (counter {counter}, waiting for {})",
                target.wrapping_sub(1)
            ),
        );
    };
    if let Some(p) = fw.pace {
        std::thread::sleep(p);
    }
    w.delivered += 1;
    w.last = Some(counter);
    let runtime = std::sync::Arc::clone(ctx.runtime());
    let mut guard = runtime.memory();
    let saved = w.saved;
    enter_callback(cpu, &mut guard, &saved, v)?;
    ctx.call_guest(cpu, &mut guard, v.callback, move |cpu, ctx| {
        step(cpu, ctx, w, device, on_stop, name)
    })
}

/// The exit effects of `0x12b344..0x12b3b5`.
fn finish(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    w: &Wait,
    counter: i64,
    target: i64,
) -> Result<TransferAbi, GuestFault> {
    let fw = w.fw;
    let n = counter.wrapping_add(1);
    write_i64(ctx, fw.exit_count, n)?;
    let d = n.wrapping_sub(target);
    let eax = if d < 0 { 0 } else { d.min(0x7FFF) as u32 };
    let gate = {
        let m = ctx.memory();
        i16::from_le_bytes([m.u8(fw.frame_gate)?, m.u8(fw.frame_gate.wrapping_add(1))?])
    };
    let late = gate > 0 && eax as u16 == 0;
    ctx.memory().set_u8(fw.late_flag, late as u8)?;
    *cpu = w.saved;
    cpu.eax = eax;
    cpu.ecx = (d >> 32) as u32;
    cpu.edx = counter as u32;
    Ok(TransferAbi::ret(w.ret_pc))
}

fn read_i64(ctx: &mut ExecCtx, addr: u32) -> Result<i64, GuestFault> {
    let m = ctx.memory();
    let lo = m.u32(addr)? as u64;
    let hi = m.u32(addr.wrapping_add(4))? as u64;
    Ok((hi << 32 | lo) as i64)
}

fn write_i64(ctx: &mut ExecCtx, addr: u32, v: i64) -> Result<(), GuestFault> {
    let mut m = ctx.memory();
    m.set_u32(addr, v as u32)?;
    m.set_u32(addr.wrapping_add(4), (v >> 32) as u32)
}
