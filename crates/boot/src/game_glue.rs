//! Boot-only overrides of Halo 2 game functions (not XDK library code).
//!
//! Each one replaces a whole game function at its entry, with the
//! function's own ABI, and reproduces a path the retail code already takes.
//! See docs/BOOT.md for the evidence behind each.

use crate::stopgaps::Uses;

/// Halo 2's voice (XHV) start-up, `0x00055810`. Register ABI: `edi` is the
/// voice-state block (`0x476fc8`), plain `ret`, returns a BOOL in `al`, which
/// both callers store as the "voice initialised" flag (`0x533d3` into
/// `0x476fcc`, `0x550a9` into `[edi+4]`).
///
/// Its first step, `0x55780`, returns `al = 0` and touches nothing but a
/// stack-local parameter block when the DSound effects-image descriptor
/// (`[[0x510c90]]`) is null; `0x55810` then returns 0 straight away. This
/// override takes that same path: every voice call in the game is gated on
/// the flag, so voice stays off.
///
/// Why: the non-zero path calls `XHVEngineCreate` (`0x333055`), whose
/// `EnableProcessingMode` (`0x33275e` -> `0x3334bb` -> `0x333330`) reads
/// `desc->aEffectMaps[0xb + i].lpvStateSegment` (`0x332706`, 32-byte maps
/// at `desc + 8`) and then the effect's input mixbin pointer at
/// `state + 0x14`. crates/dsound's dry image has 16-byte zeroed maps, so
/// that is a null read (the `0x33337d` fault). Failing `XHVEngineCreate`
/// instead is not safe: `0x55810` returns 1 even then, and the gated
/// callers use the null engine (`0x5395c`, `0x583a1`, `0x584f9`).
pub const VOICE_INIT: u32 = 0x0005_5810;

pub fn install(rt: &runtime::Runtime, uses: &Uses) {
    if std::env::var_os("H2_BOOT_VOICE").is_some() {
        return;
    }
    let u = std::sync::Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(VOICE_INIT, "boot:voice-init-off", move |cpu, ctx, _| {
            u.lock().unwrap().entry(VOICE_INIT).or_insert(("voice init 0x55810 (voice off)", 0)).1 += 1;
            cpu.eax &= !0xff;
            ctx.runtime().memory().ret(cpu, 0)
        });
    });
}

/// Halo's frame-pacing wait, `0x0012b2e0` (callers 0x12b77b, 0x163a1f,
/// 0x163ad2, 0x223288; no args, plain `ret`, returns the frame delta in
/// EAX). It spins (0x12b330..0x12b342, no kernel call, no yield) until the
/// 64-bit vertical-blank counter at 0x485ab0 reaches `[0x4e6398] - 1`. Only
/// the D3D vertical-blank callback the game registers at device+0x1db8
/// (0x12b093: 0x12b2a0 -> 0x14280) advances that counter, and nothing calls
/// it (crates/d3d's Swap does not). In generated code the spin is a native
/// loop, so the runner never regains control: the boot hangs silently.
///
/// Boot stopgap: deliver the missing vertical blanks, as fast as the game
/// asks. While the counter is short of the target, call the registered
/// callback with a `D3DVBLANKDATA { VBlank, Swap, Flags }` record (Swap =
/// device+0x2478) and re-enter; then do the rest of the body exactly
/// (0x12b344..0x12b3b5): store counter+1 at 0x4e63b0, clamp counter+1 -
/// `[0x4e6398]` to [0, 0x7fff] as the result, and set the byte at 0x4e63bc.
/// Owner: d3d + runtime/kernel (a vertical-blank source on the clock that
/// runs the callback at a safe point), docs/BOOT.md.
pub const FRAME_WAIT: u32 = 0x0012_b2e0;
const VBLANK_COUNTER: u32 = 0x0048_5ab0;
/// A boot-owned RW page: the D3DVBLANKDATA record here, the yield interval
/// in wrappers.rs.
pub const BOOT_SCRATCH: u32 = 0xa0ff_3000;
const VBLANK_RECORD: u32 = BOOT_SCRATCH;

fn u64_at(mem: &runtime::Guest, a: u32) -> Result<u64, runtime::GuestFault> {
    Ok(mem.u32(a)? as u64 | (mem.u32(a + 4)? as u64) << 32)
}
fn set_u64(mem: &mut runtime::Guest, a: u32, v: u64) -> Result<(), runtime::GuestFault> {
    mem.set_u32(a, v as u32)?;
    mem.set_u32(a + 4, (v >> 32) as u32)
}

pub fn install_frame_wait(rt: &runtime::Runtime, uses: &Uses) {
    let _ = rt.memory().map_zeroed(BOOT_SCRATCH, 0x1000, runtime::guest::Permissions::RW);
    let u = std::sync::Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(FRAME_WAIT, "boot:frame wait (vblanks delivered)", move |cpu, ctx, _| {
            let rt = std::sync::Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let counter = u64_at(&mem, VBLANK_COUNTER)? as i64;
            let start = u64_at(&mem, 0x004e_6398)? as i64;
            let device = d3d::GuestLayout::HALO2.device;
            let callback = mem.u32(device + 0x1db8)?;
            if counter < start.wrapping_sub(1) {
                u.lock().unwrap().entry(FRAME_WAIT).or_insert(("frame wait: vertical blank delivered", 0)).1 += 1;
                if callback == 0 {
                    set_u64(&mut mem, VBLANK_COUNTER, start.wrapping_sub(1) as u64)?;
                    return Ok(runtime::TransferAbi::jump(FRAME_WAIT));
                }
                let swap = mem.u32(device + 0x2478)?;
                mem.set_u32(VBLANK_RECORD, counter.wrapping_add(1) as u32)?;
                mem.set_u32(VBLANK_RECORD + 4, swap)?;
                mem.set_u32(VBLANK_RECORD + 8, 0)?;
                cpu.esp = cpu.esp.wrapping_sub(4);
                mem.set_u32(cpu.esp, VBLANK_RECORD)?;
                return ctx.call_guest(cpu, &mut mem, callback, |cpu, _| {
                    cpu.esp = cpu.esp.wrapping_add(4);
                    Ok(runtime::TransferAbi::jump(FRAME_WAIT))
                });
            }
            set_u64(&mut mem, 0x004e_63a8, counter.wrapping_add(1) as u64)?;
            let next = counter.wrapping_add(1);
            set_u64(&mut mem, 0x004e_63b0, next as u64)?;
            let delta = next.wrapping_sub(start);
            let ax = delta.clamp(0, 0x7fff) as u32;
            let gate = mem.u16(0x004e_63b8)? as i16 > 0;
            mem.set_u8(0x004e_63bc, (gate && ax == 0) as u8)?;
            cpu.eax = ax;
            mem.ret(cpu, 0)
        });
    });
}

/// The streaming block's purge callback, `0x00213fa0` (stdcall, 2 args,
/// `ret 8`; registered by `0x213fe0` through the pool allocator
/// `0x12d400`). Retail body: while the block `[0x55bd18]` is live, set the
/// release request `[0x55bd20] = 1` and, if the async-read completion byte
/// `[0x55bd0c]` is set, step the streaming state machine `0x2141f0` (which
/// frees the block through `0x214030` once it sees the request). While a
/// read is pending, `[0x55bd0c]` is 0 until the I/O completion sets it
/// (the byte is passed as the done flag to 0x1a0b40/0x1a1550/0x1a16a0), so
/// the loop busy-waits on another thread. On the Xbox the scheduler
/// preempts it. Generated code never yields inside a loop, so the boot
/// hangs in this function natively, with no transfer to end the slice.
///
/// Boot stopgap: the same body, with a 1 ms KeDelayExecutionThread in each
/// waiting iteration, so the other threads run. Owner: emit + runtime (a
/// preemption point on loop back-edges, e.g. a transfer budget check), see
/// docs/BOOT.md.
pub const STREAM_PURGE: u32 = 0x0021_3fa0;

pub fn install_stream_purge(rt: &runtime::Runtime, uses: &Uses) {
    const KE_DELAY_EXECUTION_THREAD: u32 = 99;
    const INTERVAL: u32 = BOOT_SCRATCH + 0x110;
    let _ = rt.memory().map_zeroed(BOOT_SCRATCH, 0x1000, runtime::guest::Permissions::RW);
    let u = std::sync::Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(STREAM_PURGE, "boot:stream purge callback (yielding)", move |cpu, ctx, _| {
            let rt = std::sync::Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            if mem.u32(0x0055_bd18)? == 0 {
                return mem.ret(cpu, 8);
            }
            mem.set_u8(0x0055_bd20, 1)?;
            if mem.u8(0x0055_bd0c)? != 0 {
                return ctx.call_guest(cpu, &mut mem, 0x0021_41f0, |_, _| {
                    Ok(runtime::TransferAbi::jump(STREAM_PURGE))
                });
            }
            u.lock().unwrap().entry(STREAM_PURGE).or_insert(("stream purge: waited 1 ms for the read", 0)).1 += 1;
            let interval = -10_000i64 as u64;
            mem.set_u32(INTERVAL, interval as u32)?;
            mem.set_u32(INTERVAL + 4, (interval >> 32) as u32)?;
            for w in [INTERVAL, 0, 0] {
                cpu.esp = cpu.esp.wrapping_sub(4);
                mem.set_u32(cpu.esp, w)?;
            }
            let delay = runtime::kernel::function_token(KE_DELAY_EXECUTION_THREAD);
            ctx.call_guest(cpu, &mut mem, delay, |_, _| Ok(runtime::TransferAbi::jump(STREAM_PURGE)))
        });
    });
}
