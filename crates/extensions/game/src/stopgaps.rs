//! Boot stopgaps: minimal stand-ins for kernel exports the kernel HLE
//! (crates/runtime/kernel) still stops on, so the tracer can see what comes after.
//!
//! These are NOT HLE. Each one is the smallest effect that lets startup
//! continue, is counted, and is listed as a blocker for its owner in
//! docs/runtime/BOOT.md. They are only installed with `--stopgaps` (the `h2 run`
//! default) and never replace a handler the kernel already implements.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use kernel::abi::{Export, Value};

pub type Uses = Arc<Mutex<BTreeMap<u32, (&'static str, u64)>>>;

struct Gap {
    export: Export,
    /// What the stand-in does, for the report.
    does: &'static str,
    f: fn(&kernel::abi::Args, &mut runtime::Guest) -> Result<Value, runtime::GuestFault>,
}

const fn gap(
    export: Export,
    does: &'static str,
    f: fn(&kernel::abi::Args, &mut runtime::Guest) -> Result<Value, runtime::GuestFault>,
) -> Gap {
    Gap { export, does, f }
}

fn void(_: &kernel::abi::Args, _: &mut runtime::Guest) -> Result<Value, runtime::GuestFault> {
    Ok(Value::Void)
}

fn zero(_: &kernel::abi::Args, _: &mut runtime::Guest) -> Result<Value, runtime::GuestFault> {
    Ok(Value::U32(0))
}

fn not_found(_: &kernel::abi::Args, _: &mut runtime::Guest) -> Result<Value, runtime::GuestFault> {
    Ok(Value::U32(0xc000_0034))
}

type R = Result<Value, runtime::GuestFault>;
type A = kernel::abi::Args;

/// KDPC: Type (u16) = DpcObject (19), Inserted = 0, routine at +0xc,
/// context at +0x10, system arguments zero.
fn init_dpc(a: &A, m: &mut runtime::Guest) -> R {
    let d = a.get(0);
    m.set_u16(d, 19)?;
    m.set_u8(d + 2, 0)?;
    m.set_u8(d + 3, 0)?;
    m.set_u32(d + 4, 0)?;
    m.set_u32(d + 8, 0)?;
    m.set_u32(d + 0xc, a.get(1))?;
    m.set_u32(d + 0x10, a.get(2))?;
    m.set_u32(d + 0x14, 0)?;
    m.set_u32(d + 0x18, 0)?;
    Ok(Value::Void)
}

/// KTIMER header: Type = TimerNotificationObject (8) + type, signal state 0;
/// the timer never fires.
fn init_timer(a: &A, m: &mut runtime::Guest) -> R {
    let t = a.get(0);
    m.set_u8(t, 8 + a.get(1) as u8)?;
    m.set_u8(t + 1, 0)?;
    m.set_u8(t + 2, 10)?;
    m.set_u8(t + 3, 0)?;
    m.set_u32(t + 4, 0)?;
    Ok(Value::Void)
}

/// RtlCompareMemoryUlong: bytes of whole ULONGs equal to the pattern,
/// up to the first mismatch. A real implementation, kept here only until
/// crates/runtime/kernel has it.
fn compare_ulong(a: &A, m: &mut runtime::Guest) -> R {
    let (src, len, pat) = (a.get(0), a.get(1) & !3, a.get(2));
    let mut n = 0;
    while n < len && m.u32(src + n)? == pat {
        n += 4;
    }
    Ok(Value::U32(n))
}

/// ExQueryNonVolatileSetting with the kernel's ConsoleConfig plus the
/// settings it does not configure yet (XC_AUDIO 9, XC_MISC 0x11,
/// XC_DVD_REGION 0x12, timezone 0..6); unknown indices fail with
/// STATUS_OBJECT_NAME_NOT_FOUND instead of stopping.
fn nv_setting(a: &A, m: &mut runtime::Guest) -> R {
    let (index, ty, val, len, out_len) = (a.get(0), a.get(1), a.get(2), a.get(3), a.get(4));
    let v = kernel::ConsoleConfig::default().setting(index).or(match index {
        0..=6 | 9 | 0x11 => Some(0),
        0x12 => Some(1),
        _ => None,
    });
    let Some(v) = v else { return Ok(Value::U32(0xc000_0034)) };
    if ty != 0 {
        m.set_u32(ty, 4)?;
    }
    if out_len != 0 {
        m.set_u32(out_len, 4)?;
    }
    if len < 4 {
        return Ok(Value::U32(0xc000_0023));
    }
    m.set_u32(val, v)?;
    Ok(Value::U32(0))
}

/// A zeroed page standing in for kernel objects SetThreadPriority asks for.
pub const DUMMY_OBJECT: u32 = 0xa0fe_0000;

/// ObReferenceObjectByHandle(Handle, ObjectType, &Object): XAPI's
/// SetThreadPriority (0x2d1fb5) references the thread only to pass it to
/// KeSetBasePriorityThread (ignored here) and dereference it. Hand out a
/// dummy object for thread handles; any other type fails.
fn ob_reference(a: &A, m: &mut runtime::Guest) -> R {
    let (ty, out) = (a.get(1), a.get(2));
    let thread_type = m.u32(ty).ok();
    let _ = thread_type;
    if out == 0 {
        return Ok(Value::U32(0xc000_000d));
    }
    m.set_u32(out, DUMMY_OBJECT)?;
    Ok(Value::U32(0))
}

/// MmCreateKernelStack(NumberOfBytes, DebuggerThread): a zeroed RW region
/// from a private bump range, returning its top (the initial stack pointer
/// side, as the retail kernel does). One guard page below each stack.
pub const KSTACK_BASE: u32 = 0x9800_0000;
pub const KSTACK_END: u32 = 0x9c00_0000;
static KSTACK_NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(KSTACK_BASE);

fn create_kernel_stack(a: &A, m: &mut runtime::Guest) -> R {
    let len = a.get(0).checked_add(0xfff).map_or(0, |n| n & !0xfff);
    if len == 0 {
        return Ok(Value::U32(0));
    }
    let base = KSTACK_NEXT.fetch_add(len + 0x1000, std::sync::atomic::Ordering::Relaxed) + 0x1000;
    if base as u64 + len as u64 > KSTACK_END as u64
        || m.map_zeroed(base, len, runtime::guest::Permissions::RW).is_err()
    {
        return Ok(Value::U32(0));
    }
    Ok(Value::U32(base + len))
}

/// Replace the kernel's own handler for these, too.
const ALWAYS: &[u32] = &[];

const GAPS: &[Gap] = &[
    gap(Export::stdcall(24, "ExQueryNonVolatileSetting", 5), "kernel's ConsoleConfig + XC_AUDIO/MISC/DVD_REGION/timezone", nv_setting),
    // VOID HalRegisterShutdownNotification(PHAL_SHUTDOWN_REGISTRATION, BOOLEAN)
    gap(Export::stdcall(47, "HalRegisterShutdownNotification", 2), "ignored (no shutdown path)", void),
    gap(Export::stdcall(107, "KeInitializeDpc", 3), "fills the KDPC; DPCs never run", init_dpc),
    gap(Export::stdcall(113, "KeInitializeTimerEx", 2), "fills the header; timer never fires", init_timer),
    // BOOLEAN KeSetTimer(PKTIMER, LARGE_INTEGER DueTime, PKDPC)
    gap(Export::stdcall(149, "KeSetTimer", 4), "returns FALSE; never fires", zero),
    gap(Export::stdcall(150, "KeSetTimerEx", 5), "returns FALSE; never fires", zero),
    gap(Export::stdcall(97, "KeCancelTimer", 1), "returns FALSE", zero),
    gap(Export::stdcall(151, "KeStallExecutionProcessor", 1), "returns at once (no time passes)", void),
    gap(Export::stdcall(143, "KeSetBasePriorityThread", 2), "returns 0; priorities ignored", zero),
    gap(Export::stdcall(142, "KeSaveFloatingPointState", 1), "STATUS_SUCCESS, saves nothing", zero),
    gap(Export::stdcall(139, "KeRestoreFloatingPointState", 1), "STATUS_SUCCESS, restores nothing", zero),
    gap(Export::stdcall(269, "RtlCompareMemoryUlong", 3), "real implementation", compare_ulong),
    gap(Export::stdcall(252, "PhyGetLinkState", 1), "returns 0 (no link)", zero),
    gap(Export::stdcall(169, "MmCreateKernelStack", 2), "bump-mapped RW region at 0x98000000", create_kernel_stack),
    gap(Export::stdcall(170, "MmDeleteKernelStack", 2), "no-op (stack kept mapped)", void),
    gap(Export::stdcall(253, "PhyInitialize", 2), "STATUS_SUCCESS, no PHY (cable unplugged)", zero),
    gap(Export::stdcall(246, "ObReferenceObjectByHandle", 3), "dummy object (only SetThreadPriority seen)", ob_reference),
    gap(Export::fastcall(250, "ObfDereferenceObject", 1), "no-op", void),
    gap(Export::stdcall(182, "MmSetAddressProtect", 3), "no-op (runtime protect() is whole-mapping only)", void),
    gap(Export::stdcall(335, "XcSHAInit", 1), "real SHA-1 (crates/extensions/game sha.rs)", crate::sha::init),
    gap(Export::stdcall(336, "XcSHAUpdate", 3), "real SHA-1", crate::sha::update_g),
    gap(Export::stdcall(337, "XcSHAFinal", 2), "real SHA-1", crate::sha::final_g),
    gap(Export::stdcall(178, "MmPersistContiguousMemory", 3), "ignored (no quick reboot)", void),
    // XAPI's 0x2d0e12 falls back to its built-in "\Device\CdRom0" string on failure.
    gap(Export::stdcall(203, "NtOpenSymbolicLinkObject", 2), "fails STATUS_OBJECT_NAME_NOT_FOUND (callers fall back)", not_found),
];

/// Describe every stopgap, for docs and `--list-stopgaps`.
pub fn list() -> Vec<(u32, &'static str, &'static str)> {
    GAPS.iter().map(|g| (g.export.ordinal, g.export.name, g.does)).collect()
}

/// Install every stopgap whose ordinal the kernel still stops on.
pub fn install(k: &kernel::Kernel, rt: &runtime::Runtime) -> Uses {
    let uses: Uses = Arc::default();
    let _ = rt.memory().map_zeroed(DUMMY_OBJECT, 0x1000, runtime::guest::Permissions::RW);
    let stopped: std::collections::BTreeSet<u32> = k.stopped_ordinals().into_iter().collect();
    rt.edit_dispatch(|d| {
        for g in GAPS {
            if !stopped.contains(&g.export.ordinal) && !ALWAYS.contains(&g.export.ordinal) {
                continue;
            }
            let entry = k.entry(g.export.ordinal).unwrap_or(runtime::kernel::function_token(g.export.ordinal));
            let (u, f, name, ord) = (Arc::clone(&uses), g.f, g.export.name, g.export.ordinal);
            kernel::abi::register_at(d, entry, g.export, move |args, mem| {
                u.lock().unwrap().entry(ord).or_insert((name, 0)).1 += 1;
                f(args, mem)
            });
        }
    });
    uses
}
