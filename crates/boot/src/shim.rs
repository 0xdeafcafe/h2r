//! The bridge table that forwards the emitter's imports to the runtime's
//! `h2abi_v1_*` and `h2shim_v1_rdtsc`, and boot's own `h2shim_v1_event` and
//! `h2shim_v1_unsupported` (RUNTIME-CONTRACT.md §1.3, §1.4).
//!
//! The runtime exports those two as well; boot keeps its own because the
//! fallback needs the description. They record an `UnrecordedFault { pc }`
//! (a valid event id for the FAULT/STOP transfer) and keep the text in a
//! side table keyed by thread and PC, which `is_unsupported` reads.

use std::ffi::{CStr, c_char};

use runtime::{CpuAbi, Event, ExecCtx, ExecCtxOpaque};

use crate::SHIM_EVENTS;

/// Layout of `H2BootBridge` in `c/boot_bridge.c`.
#[repr(C)]
pub struct Bridge {
    read: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, *const c_char, *const c_char) -> u32,
    rdtsc: unsafe extern "C" fn(*mut ExecCtxOpaque, u32) -> u64,
    rmw: unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, u32, *mut u32) -> u32,
    xchg: unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg: unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg8b: unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32) -> u32,
    profile: unsafe extern "C" fn(*mut ExecCtxOpaque, u64, u64),
    fast_acquire: unsafe extern "C" fn(*mut ExecCtxOpaque) -> *mut runtime::ffi::FastAccess,
    fast_release: unsafe extern "C" fn(*mut ExecCtxOpaque),
    read_counted: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write_counted: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    charge: unsafe extern "C" fn(*mut ExecCtxOpaque, u64),
    budget: unsafe extern "C" fn(*mut ExecCtxOpaque) -> u64,
    /// New in `h2boot_install_v3`: a v2 table is this one without it.
    exit: unsafe extern "C" fn(*mut ExecCtxOpaque, u64),
}

impl Bridge {
    /// The size `h2boot_install_v2` takes: a module built before `exit`
    /// still calls `charge` and `fast_release`, which stay in the table.
    pub const V2_SIZE: usize = std::mem::offset_of!(Bridge, exit);
}

pub fn bridge() -> Bridge {
    // A measurement switch, read where the module's helpers are installed:
    // FSIN/FCOS/FSINCOS/FYL2X/F2XM1 in native f64 instead of the correctly
    // rounded series (x87's trans_f64.rs; tests/f64_trans.rs measures it).
    if std::env::var("H2_X87_FAST_TRANSCENDENTALS").as_deref() == Ok("1") {
        x87::set_f64_transcendentals(true);
        eprintln!("boot: H2_X87_FAST_TRANSCENDENTALS=1: x87 transcendentals in f64, not the x87's 64-bit results");
    }
    bridge_with_fast_memory(true)
}

/// The bridge with inline fast memory on, or with every access through the
/// checked accessors (tests compare the two on one image). Selecting the
/// callback once also covers new guest threads and DPCs.
pub fn bridge_with_fast_memory(enabled: bool) -> Bridge {
    use runtime::ffi;
    Bridge {
        read: ffi::h2abi_v1_read,
        write: ffi::h2abi_v1_write,
        event,
        unsupported,
        rdtsc: runtime::generated::h2shim_v1_rdtsc,
        rmw: ffi::h2abi_v1_atomic_rmw,
        xchg: ffi::h2abi_v1_xchg,
        cmpxchg: ffi::h2abi_v1_cmpxchg,
        cmpxchg8b: ffi::h2abi_v1_cmpxchg8b,
        profile: ffi::h2abi_v1_profile,
        fast_acquire: if enabled { ffi::h2abi_v1_fast_acquire } else { no_fast_memory },
        fast_release: ffi::h2abi_v1_fast_release,
        read_counted: ffi::h2abi_v1_read_counted,
        write_counted: ffi::h2abi_v1_write_counted,
        charge: ffi::h2abi_v1_charge,
        budget: ffi::h2abi_v1_budget,
        exit: ffi::h2abi_v1_exit,
    }
}

unsafe extern "C" fn no_fast_memory(_: *mut ExecCtxOpaque) -> *mut runtime::ffi::FastAccess {
    std::ptr::null_mut()
}

/// crates/x87's helpers in `h2x87bridge_install` order (emit's
/// `X87_BRIDGE_SYMBOLS`, which the test below checks).
macro_rules! x87_helpers {
    ($($name:ident,)*) => {
        #[cfg(test)]
        const X87_HELPER_NAMES: [&str; 35] = [$(stringify!($name)),*];
        pub fn x87_helpers() -> [*const std::ffi::c_void; 35] {
            [$(x87::abi::ffi::$name as *const std::ffi::c_void),*]
        }
    };
}
x87_helpers! {
    h2x87_v1_ld_f32,
    h2x87_v1_ld_f64,
    h2x87_v1_ld_f80,
    h2x87_v1_ld_i16,
    h2x87_v1_ld_i32,
    h2x87_v1_ld_i64,
    h2x87_v1_ld_bcd,
    h2x87_v1_ld_const,
    h2x87_v1_ld_st,
    h2x87_v1_st_f32,
    h2x87_v1_st_f64,
    h2x87_v1_st_f80,
    h2x87_v1_st_i16,
    h2x87_v1_st_i32,
    h2x87_v1_st_i64,
    h2x87_v1_st_bcd,
    h2x87_v1_st_i32_trunc,
    h2x87_v1_store_commit,
    h2x87_v1_st_st,
    h2x87_v1_ftol2,
    h2x87_v1_arith,
    h2x87_v1_unary,
    h2x87_v1_cmp,
    h2x87_v1_xam,
    h2x87_v1_xch,
    h2x87_v1_free,
    h2x87_v1_adj_top,
    h2x87_v1_cmov,
    h2x87_v1_ldcw,
    h2x87_v1_wait,
    h2x87_v1_clex,
    h2x87_v1_init,
    h2x87_v1_save_image,
    h2x87_v1_save_commit,
    h2x87_v1_load_env,
}

const NAMES: [&str; 16] = [
    "?",
    "divide error (#DE)",
    "breakpoint (#BP)",
    "invalid opcode (#UD)",
    "software interrupt",
    "overflow (#OF)",
    "bound range (#BR)",
    "general protection (#GP)",
    "hlt",
    "bad entry selector",
    "unknown switch target",
    "descent error (analysis could not follow control)",
    "unmodelled (popfd, or an unmasked x87 OE/UE/PE)",
    "SIMD floating-point exception (#XM)",
    "x87 floating-point error (#MF, arg = status word)",
    "unobserved host module owner (arg = entry address)",
];

fn record(c: *mut ExecCtxOpaque, pc: u32, text: String) -> u32 {
    // SAFETY: generated code passes back the live, exclusive context it was
    // handed (h2abi.h); no other borrow of it exists during this call.
    let ctx = unsafe { &mut *c.cast::<ExecCtx>() };
    record_ctx(ctx, pc, text)
}

/// Record a diagnostic and return its event id.
pub fn record_ctx(ctx: &mut ExecCtx, pc: u32, text: String) -> u32 {
    let mut ev = SHIM_EVENTS.lock().unwrap();
    if ev.len() > 8192 {
        ev.drain(..4096);
    }
    ev.push((ctx.thread_id, pc, text));
    drop(ev);
    ctx.emit(Event::UnrecordedFault { pc })
}

const UNSUPPORTED: &str = "unsupported instruction at ";
/// Whether the newest record for `pc` is a lifter "unsupported" stop.
pub fn is_unsupported(thread: u32, pc: u32) -> bool {
    lookup(thread, pc).is_some_and(|s| s.starts_with(UNSUPPORTED))
}

/// `H2SHIM_UNKNOWN_TARGET`: a switch loaded a target outside its cases.
pub const UNKNOWN_TARGET: u32 = 10;

/// The last unknown switch target per (thread, saved EIP), for the
/// fallback's allowlisted recovery.
static UNKNOWN_SWITCH: std::sync::Mutex<Option<std::collections::HashMap<(u32, u32), u32>>> = std::sync::Mutex::new(None);

/// Remember an unknown switch target (`arg`) stopped at `saved_eip`.
pub fn note_unknown_switch(thread: u32, saved_eip: u32, target: u32) {
    UNKNOWN_SWITCH.lock().unwrap().get_or_insert_default().insert((thread, saved_eip), target);
}

/// Take the unknown switch target recorded for `thread` at `pc`.
pub fn take_unknown_switch(thread: u32, pc: u32) -> Option<u32> {
    UNKNOWN_SWITCH.lock().unwrap().as_mut()?.remove(&(thread, pc))
}

unsafe extern "C" fn event(c: *mut ExecCtxOpaque, pc: u32, saved_eip: u32, code: u32, arg: u32) -> u32 {
    if code == UNKNOWN_TARGET {
        // SAFETY: the live, exclusive context generated code was handed.
        let thread = unsafe { &*c.cast::<ExecCtx>() }.thread_id;
        note_unknown_switch(thread, saved_eip, arg);
    }
    let name = NAMES.get(code as usize).copied().unwrap_or("?");
    record(c, pc, format!("guest exception/stop {code} {name} at {pc:#x} (saved eip {saved_eip:#x}, arg {arg:#x})"))
}

unsafe extern "C" fn unsupported(c: *mut ExecCtxOpaque, pc: u32, m: *const c_char, d: *const c_char) -> u32 {
    // SAFETY: static NUL-terminated strings owned by the module.
    let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    record(c, pc, format!("{UNSUPPORTED}{pc:#x}: {} ({})", s(m), s(d)))
}

/// The newest shim record for `pc` on `thread`.
pub fn lookup(thread: u32, pc: u32) -> Option<String> {
    SHIM_EVENTS.lock().unwrap().iter().rev().find(|(t, p, _)| *t == thread && *p == pc).map(|e| e.2.clone())
}

#[cfg(test)]
mod tests {
    #[test]
    fn x87_helpers_match_the_emit_bridge_order() {
        assert_eq!(super::X87_HELPER_NAMES, emit::host::X87_BRIDGE_SYMBOLS);
    }
}
