//! The vertical blank and the engine's frame wait, on the 60 Hz logical
//! clock, with no D3D device.
//!
//! Halo sets its vblank callback (`0x12b2a0`) in its vblank install
//! (`0x12b070`, [`crate::display`]), which keeps it as
//! [`crate::display::vblank_callback`] and not in a device. On the
//! Xbox the GPU interrupt calls it once per vblank as
//! `cdecl callback(D3DVBLANKDATA *)` (`{VBlank, Swap, Flags}`); it bumps the
//! 64-bit counter at `0x485AB0`. The frame wait `0x12b2e0` spins on that
//! counter until it reaches `[0x4E6398] - 1`.
//!
//! [`clocked_vblanks`] is a kernel periodic-source handler: it counts the
//! vblank and returns the callback as a guest interrupt, which the kernel
//! runs like the GPU's DPC between thread slices. [`register`] overrides
//! the frame wait: it calls `NtYieldExecution` until the counter arrives,
//! then produces the original's exit effects. `Swap` is the frame step's
//! seal ([`swapped`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use runtime::{CallConvention, CpuAbi, Dispatch, ExecCtx, Guest, GuestFault, GuestInterrupt, TransferAbi};

/// `D3DVBLANKDATA::Flags` for the first vblank after a `Swap`.
const VBLANK_SWAPDONE: u32 = 1;
/// `NtYieldExecution`.
const NT_YIELD_EXECUTION: u32 = 238;
/// Diagnostic name of the vblank interrupt.
pub const VBLANK_SOURCE: &str = "vertical-blank callback";

/// Yields in a row with neither a vblank nor any logical time passing
/// after which the wait panics.
pub const MAX_IDLE_YIELDS: u32 = 100_000;
/// Logical time (100 ns ticks) the wait lets pass with no vblank before it
/// panics: two seconds.
pub const MAX_VBLANK_GAP: u64 = 20_000_000;
/// Vblanks one wait may be short by.
pub const MAX_VBLANKS: i64 = 600;

static VBLANKS: AtomicU32 = AtomicU32::new(0);
static SWAPS: AtomicU32 = AtomicU32::new(0);
static SWAPPED: AtomicBool = AtomicBool::new(false);

static ON_SWAP: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Counts one `Swap`: the frame step sealed a frame. Then runs the
/// [`on_swap`] callback.
pub fn swapped() {
    SWAPS.fetch_add(1, Ordering::Relaxed);
    SWAPPED.store(true, Ordering::Relaxed);
    if let Some(f) = ON_SWAP.get() {
        f();
    }
}

/// Sets what runs after each `Swap` (the game's present count). Once.
pub fn on_swap(f: fn()) {
    let _ = ON_SWAP.set(f);
}

/// `Swap`s so far.
pub fn swaps() -> u32 {
    SWAPS.load(Ordering::Relaxed)
}

/// The periodic-source handler (`kernel::periodic::Handler`, boxed by the
/// caller): counts the vblank (and `missed` dropped periods) and returns
/// the guest callback with its record at `scratch`. No callback set yet:
/// counted, nothing to call.
pub fn clocked_vblanks()
-> impl FnMut(&mut Guest, u32, u64) -> Result<Option<GuestInterrupt>, GuestFault> + Send + 'static {
    move |guest, scratch, missed| {
        let missed = u32::try_from(missed).unwrap_or(u32::MAX);
        let vblank = VBLANKS.fetch_add(missed.wrapping_add(1), Ordering::Relaxed).wrapping_add(missed).wrapping_add(1);
        let flags = if SWAPPED.swap(false, Ordering::Relaxed) { VBLANK_SWAPDONE } else { 0 };
        let callback = crate::display::vblank_callback();
        if callback == 0 {
            return Ok(None);
        }
        for (i, w) in [vblank, swaps(), flags].into_iter().enumerate() {
            guest.set_u32(scratch.wrapping_add(4 * i as u32), w)?;
        }
        Ok(Some(GuestInterrupt {
            target: callback,
            args: vec![scratch],
            convention: CallConvention::Cdecl,
            thread: None,
            coalesce: false,
            name: VBLANK_SOURCE,
        }))
    }
}

/// Halo 2's frame wait and the words it reads and writes.
#[derive(Clone, Copy, Debug)]
struct Halo2;

impl Halo2 {
    const ENTRY: u32 = 0x0012_b2e0;
    /// The vblank counter the callback increments.
    const COUNTER: u32 = 0x0048_5ab0;
    /// The counter value the wait is for.
    const TARGET: u32 = 0x004e_6398;
    /// Written with the counter + 1 on entry.
    const ENTRY_COUNT: u32 = 0x004e_63a8;
    /// Written with the counter + 1 on exit.
    const EXIT_COUNT: u32 = 0x004e_63b0;
    /// Signed word read on exit.
    const FRAME_GATE: u32 = 0x004e_63b8;
    /// Byte written on exit.
    const LATE_FLAG: u32 = 0x004e_63bc;
}

const NAME: &str = "engine frame wait (0x12b2e0)";

/// One wait in progress.
#[derive(Clone, Copy)]
struct Wait {
    /// CPU after the wait's own frame is popped.
    saved: CpuAbi,
    ret_pc: u32,
    last: Option<i64>,
    idle: u32,
    time: Option<u64>,
    since: Option<u64>,
}

/// Overrides the engine frame wait.
pub fn register(dispatch: &mut Dispatch) {
    let _ = dispatch.register_override(Halo2::ENTRY, NAME, move |cpu, ctx, _| {
        let ret_pc = ctx.memory().u32(cpu.esp)?;
        let mut saved = *cpu;
        saved.esp = cpu.esp.wrapping_add(4);
        let start = read_i64(ctx, Halo2::COUNTER)?;
        write_i64(ctx, Halo2::ENTRY_COUNT, start.wrapping_add(1))?;
        step(cpu, ctx, Wait { saved, ret_pc, last: None, idle: 0, time: None, since: None })
    });
}

fn step(cpu: &mut CpuAbi, ctx: &mut ExecCtx, mut w: Wait) -> Result<TransferAbi, GuestFault> {
    let counter = read_i64(ctx, Halo2::COUNTER)?;
    let target = read_i64(ctx, Halo2::TARGET)?;
    let goal = target.wrapping_sub(1);
    if counter >= goal {
        return finish(cpu, ctx, &w, counter, target);
    }
    let short = goal.wrapping_sub(counter);
    if w.last.is_none() && short > MAX_VBLANKS {
        unimplemented!("{NAME}: the counter ({counter}) is {short} vblanks short of {goal}");
    }
    let now = ctx.runtime().guest_time();
    if w.last != Some(counter) {
        w.idle = 0;
        w.last = Some(counter);
        w.since = now;
    } else if now.is_some() && now > w.time {
        w.idle = 0;
    } else {
        w.idle += 1;
    }
    w.time = now;
    if let (Some(now), Some(since)) = (now, w.since)
        && now.saturating_sub(since) > MAX_VBLANK_GAP
    {
        unimplemented!("{NAME}: {} ticks without a vblank (counter {counter}, waiting for {goal})", now - since);
    }
    if w.idle >= MAX_IDLE_YIELDS {
        unimplemented!("{NAME}: {MAX_IDLE_YIELDS} yields without a vblank (counter {counter}, waiting for {goal})");
    }
    if crate::display::vblank_callback() == 0 {
        unimplemented!("{NAME}: no vblank callback is set (counter {counter}, waiting for {goal})");
    }
    // Give the CPU up: other threads run, or the clock moves to the next
    // deadline, whose vblank the kernel delivers before this thread's next
    // slice.
    let runtime = Arc::clone(ctx.runtime());
    let mut guard = runtime.memory();
    *cpu = w.saved;
    let token = runtime::kernel::function_token(NT_YIELD_EXECUTION);
    ctx.call_guest(cpu, &mut guard, token, move |cpu, ctx| step(cpu, ctx, w))
}

fn finish(cpu: &mut CpuAbi, ctx: &mut ExecCtx, w: &Wait, counter: i64, target: i64) -> Result<TransferAbi, GuestFault> {
    let n = counter.wrapping_add(1);
    write_i64(ctx, Halo2::EXIT_COUNT, n)?;
    let d = n.wrapping_sub(target);
    let eax = if d < 0 { 0 } else { d.min(0x7fff) as u32 };
    let gate = {
        let m = ctx.memory();
        i16::from_le_bytes([m.u8(Halo2::FRAME_GATE)?, m.u8(Halo2::FRAME_GATE.wrapping_add(1))?])
    };
    let late = gate > 0 && eax as u16 == 0;
    ctx.memory().set_u8(Halo2::LATE_FLAG, late as u8)?;
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
