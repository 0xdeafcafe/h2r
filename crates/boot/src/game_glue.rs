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

/// A boot-owned RW page (the yield interval in wrappers.rs, the stream
/// purge interval below).
pub const BOOT_SCRATCH: u32 = 0xa0ff_3000;

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
