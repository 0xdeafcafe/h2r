//! Boot-only overrides of Halo 2 game functions (not XDK library code).
//!
//! Each one replaces a whole game function at its entry, with the
//! function's own ABI, and reproduces a path the retail code already takes.
//! See docs/runtime/BOOT.md for the evidence behind each.

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

