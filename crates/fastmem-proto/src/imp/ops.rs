//! Single arm64 memory instructions, written in inline assembly so tests
//! know exactly which instruction faults and is retried.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::sys::*;

/// `str x, [p]`.
///
/// # Safety
/// `p` must be 8-byte aligned memory that is mapped (protection may fault).
pub unsafe fn str64(p: *mut u8, v: u64) {
    // SAFETY: per the caller's contract.
    unsafe { asm!("str {v}, [{p}]", p = in(reg) p, v = in(reg) v, options(nostack)) };
}

/// `strb w, [p]`.
///
/// # Safety
/// `p` must be mapped memory.
pub unsafe fn str8(p: *mut u8, v: u8) {
    // SAFETY: per the caller's contract.
    unsafe { asm!("strb {v:w}, [{p}]", p = in(reg) p, v = in(reg) v as u32, options(nostack)) };
}

/// Unaligned `str x, [p]`; may straddle two pages.
///
/// # Safety
/// `p..p+8` must be mapped memory.
pub unsafe fn str64_unaligned(p: *mut u8, v: u64) {
    // SAFETY: per the caller's contract; arm64 allows unaligned normal stores.
    unsafe { asm!("str {v}, [{p}]", p = in(reg) p, v = in(reg) v, options(nostack)) };
}

/// `stp x, x, [p]`.
///
/// # Safety
/// `p..p+16` must be mapped memory.
pub unsafe fn stp(p: *mut u8, a: u64, b: u64) {
    // SAFETY: per the caller's contract.
    unsafe {
        asm!("stp {a}, {b}, [{p}]", p = in(reg) p, a = in(reg) a, b = in(reg) b, options(nostack))
    };
}

/// A 16-byte NEON store, `str q0, [p]`, of `v` repeated.
///
/// # Safety
/// `p..p+16` must be mapped memory.
pub unsafe fn str_q(p: *mut u8, v: u8) {
    // SAFETY: per the caller's contract.
    unsafe {
        asm!(
            "dup v0.16b, {v:w}",
            "str q0, [{p}]",
            p = in(reg) p, v = in(reg) v as u32, out("v0") _, options(nostack),
        )
    };
}

/// An LSE atomic add (`ldaddal`), returning the old value.
///
/// # Safety
/// `p` must be 8-byte aligned mapped memory.
pub unsafe fn ldadd(p: *mut u8, v: u64) -> u64 {
    let old: u64;
    // SAFETY: per the caller's contract.
    unsafe {
        asm!("ldaddal {v}, {old}, [{p}]", p = in(reg) p, v = in(reg) v, old = out(reg) old, options(nostack))
    };
    old
}

/// A load-exclusive / store-exclusive increment loop.
///
/// # Safety
/// `p` must be 8-byte aligned mapped memory.
pub unsafe fn exclusive_inc(p: *mut u8) {
    // SAFETY: per the caller's contract.
    unsafe {
        asm!(
            "2:",
            "ldxr {t}, [{p}]",
            "add {t}, {t}, #1",
            "stxr {s:w}, {t}, [{p}]",
            "cbnz {s:w}, 2b",
            p = in(reg) p, t = out(reg) _, s = out(reg) _, options(nostack),
        )
    };
}

/// `dc zva` on the block holding `p` (zeroes 64 bytes on Apple cores).
///
/// # Safety
/// The zeroed block must be mapped memory the caller may clobber.
pub unsafe fn dc_zva(p: *mut u8) {
    // SAFETY: per the caller's contract.
    unsafe { asm!("dc zva, {p}", p = in(reg) p, options(nostack)) };
}

/// `ldrb w, [p]`.
///
/// # Safety
/// `p` must be an address; an unmapped one faults (used deliberately).
pub unsafe fn ldr8(p: *const u8) -> u8 {
    let v: u32;
    // SAFETY: per the caller's contract.
    unsafe { asm!("ldrb {v:w}, [{p}]", p = in(reg) p, v = out(reg) v, options(nostack, readonly)) };
    v as u8
}

/// Reads an aligned u64 through an atomic, for checks after retried stores.
///
/// # Safety
/// `p` must be 8-byte aligned readable memory.
pub unsafe fn load64(p: *const u8) -> u64 {
    // SAFETY: per the caller's contract.
    unsafe { (*p.cast::<AtomicU64>()).load(Ordering::SeqCst) }
}

/// The calling thread's `EXC_BAD_ACCESS` handler port (null when none).
pub fn thread_bad_access_port() -> mach_port_t {
    let mut masks = [0; EXC_TYPES_COUNT];
    let mut ports = [0; EXC_TYPES_COUNT];
    let mut behaviors = [0; EXC_TYPES_COUNT];
    let mut flavors = [0; EXC_TYPES_COUNT];
    let mut count = 0u32;
    // SAFETY: arrays are EXC_TYPES_COUNT long.
    let kr = unsafe {
        thread_get_exception_ports(
            thread_self(),
            EXC_MASK_BAD_ACCESS,
            masks.as_mut_ptr(),
            &mut count,
            ports.as_mut_ptr(),
            behaviors.as_mut_ptr(),
            flavors.as_mut_ptr(),
        )
    };
    assert_eq!(kr, KERN_SUCCESS, "thread_get_exception_ports");
    let mut found = MACH_PORT_NULL;
    for i in 0..(count as usize).min(EXC_TYPES_COUNT) {
        if ports[i] != MACH_PORT_NULL && ports[i] != MACH_PORT_DEAD {
            if masks[i] & EXC_MASK_BAD_ACCESS != 0 {
                found = ports[i];
            }
            // SAFETY: the query gave us a reference; the name stays valid
            // while the server holds its own rights.
            unsafe { mach_port_deallocate(task_self(), ports[i]) };
        }
    }
    found
}

/// Number of port names in this task, for leak checks in an isolated process.
pub fn port_name_count() -> usize {
    let mut names = core::ptr::null_mut();
    let mut names_count = 0;
    let mut types = core::ptr::null_mut();
    let mut types_count = 0;
    // SAFETY: the kernel returns two out-of-line arrays we then free.
    unsafe {
        let kr = mach_port_names(
            task_self(),
            &mut names,
            &mut names_count,
            &mut types,
            &mut types_count,
        );
        assert_eq!(kr, KERN_SUCCESS, "mach_port_names");
        vm_deallocate(task_self(), names as usize, names_count as usize * 4);
        vm_deallocate(task_self(), types as usize, types_count as usize * 4);
    }
    names_count as usize
}
