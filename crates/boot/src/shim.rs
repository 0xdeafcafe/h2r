//! The two emitter imports the runtime does not export yet
//! (RUNTIME-CONTRACT.md §1.3, §1.4), and the bridge table that forwards the
//! rest to the runtime's own `h2abi_v1_*` functions.
//!
//! The runtime has no `Event` variant for a guest exception or an
//! unsupported instruction, so these record an `UnrecordedFault { pc }`
//! (which gives a valid event id for the FAULT/STOP transfer) and keep the
//! real description in a side table keyed by thread and PC.

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
}

pub fn bridge() -> Bridge {
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
    }
}

const NAMES: [&str; 13] = [
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
    "unmodelled popfd",
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

unsafe extern "C" fn event(c: *mut ExecCtxOpaque, pc: u32, saved_eip: u32, code: u32, arg: u32) -> u32 {
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
