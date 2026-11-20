//! The Windows x86-64 sampler (the Xbox app; docs/runtime/xbox.md, "Where
//! frame time goes").
//!
//! While the target is suspended: `GetThreadContext`, then a copy of its
//! stack from RSP to the stack's base into a buffer allocated beforehand.
//! Nothing else: the target may hold the heap lock or the loader lock.
//! After `ResumeThread`, the copy is unwound with `RtlLookupFunctionEntry`
//! and `RtlVirtualUnwind` (each module's `.pdata`), with every word and
//! register that pointed into the live stack moved to point into the copy
//! ([`super::relocate`]), the way Chromium's sampling profiler does it.
//! Windows x64 code keeps no frame-pointer chain to walk, and the table
//! lookup can take a lock, so neither can happen with the target stopped.
//!
//! All of it is in `OneCoreUAP.lib` (the app container's API set; the
//! package build checks, `uwp_host::imports`): `SuspendThread`,
//! `ResumeThread`, `GetThreadContext`, `DuplicateHandle`,
//! `GetCurrentThreadStackLimits`, `QueryThreadCycleTime`,
//! `RtlLookupFunctionEntry`, `RtlVirtualUnwind`.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Captured, ThreadTarget, relocate};

/// `CONTEXT` for x64 (winnt.h): 1232 bytes, 16-byte aligned. Only the
/// offsets below are read or written here; `RtlVirtualUnwind` uses the
/// rest.
#[repr(C, align(16))]
struct Context([u64; CONTEXT_WORDS]);

const CONTEXT_WORDS: usize = 1232 / 8;
/// `ContextFlags` (a u32 at byte 0x30).
const FLAGS_BYTE: usize = 0x30;
/// `Rax` through `R15` (bytes 0x78 to 0xF0), in winnt.h's order: Rax Rcx
/// Rdx Rbx Rsp Rbp Rsi Rdi R8..R15.
const GPR_FIRST: usize = 0x78 / 8;
const GPR_COUNT: usize = 16;
const RSP: usize = 0x98 / 8;
const RIP: usize = 0xF8 / 8;
/// `CONTEXT_AMD64 | CONTEXT_CONTROL | CONTEXT_INTEGER`: RIP, RSP, flags and
/// the general registers, which is all the unwinder needs.
const CONTEXT_CONTROL_INTEGER: u32 = 0x0010_0003;

const _: () = assert!(std::mem::size_of::<Context>() == 1232);

impl Context {
    fn new(flags: u32) -> Context {
        let mut c = Context([0; CONTEXT_WORDS]);
        // SAFETY: inside the array; ContextFlags is a u32 at 0x30.
        unsafe {
            c.0.as_mut_ptr()
                .cast::<u8>()
                .add(FLAGS_BYTE)
                .cast::<u32>()
                .write(flags)
        };
        c
    }
}

/// `RUNTIME_FUNCTION`.
#[repr(C)]
struct RuntimeFunction {
    begin: u32,
    end: u32,
    unwind: u32,
}

const DUPLICATE_SAME_ACCESS: u32 = 2;
const UNW_FLAG_NHANDLER: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> isize;
    fn GetCurrentProcess() -> isize;
    fn DuplicateHandle(
        source_process: isize,
        source: isize,
        target_process: isize,
        target: *mut isize,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    fn SuspendThread(thread: isize) -> u32;
    fn ResumeThread(thread: isize) -> u32;
    fn GetThreadContext(thread: isize, context: *mut Context) -> i32;
    fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    fn RtlLookupFunctionEntry(
        pc: u64,
        image_base: *mut u64,
        history: *mut c_void,
    ) -> *mut RuntimeFunction;
    fn RtlVirtualUnwind(
        handler_type: u32,
        image_base: u64,
        pc: u64,
        entry: *mut RuntimeFunction,
        context: *mut Context,
        handler_data: *mut *mut c_void,
        establisher_frame: *mut u64,
        context_pointers: *mut c_void,
    ) -> *mut c_void;
}

/// The most of a stack copied per sample. A deeper stack keeps its
/// innermost 512 KiB, which is where the frames are.
const COPY_WORDS: usize = 512 * 1024 / 8;
/// Zeroed words past the copy, so an unwind that reads a little beyond
/// the stack's base reads zeros, not past the buffer.
const SLACK_WORDS: usize = 8 * 1024 / 8;

thread_local! {
    /// The sampler thread's stack copy, allocated before any suspend.
    static COPY: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

impl ThreadTarget {
    /// The calling thread, named `name` in the profile.
    pub fn current(name: impl Into<String>) -> ThreadTarget {
        let mut handle = 0isize;
        let (mut low, mut high) = (0usize, 0usize);
        // SAFETY: plain queries about the calling thread; `handle` and the
        // limits are ours to write.
        unsafe {
            let me = GetCurrentThread();
            let process = GetCurrentProcess();
            if DuplicateHandle(
                process,
                me,
                process,
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                handle = 0;
            }
            GetCurrentThreadStackLimits(&mut low, &mut high);
        }
        ThreadTarget {
            name: name.into(),
            port: 0,
            stack: (low as u64, high as u64),
            handle,
            cycles: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Whether the thread used CPU since its last sample.
fn ran(t: &ThreadTarget) -> bool {
    let mut now = 0u64;
    // SAFETY: a thread handle of this process; `now` is ours.
    if unsafe { QueryThreadCycleTime(t.handle, &mut now) } == 0 {
        return true;
    }
    let before = t.cycles.swap(now, Ordering::Relaxed);
    now > before
}

pub(super) fn capture(t: &ThreadTarget, buf: &mut [u64]) -> Option<Captured> {
    if t.handle == 0 || buf.is_empty() {
        return None;
    }
    let on_cpu = ran(t);
    COPY.with(|copy| {
        let mut copy = copy.borrow_mut();
        if copy.len() < COPY_WORDS + SLACK_WORDS {
            // Before any suspend: never allocate with a target stopped.
            copy.resize(COPY_WORDS + SLACK_WORDS, 0);
        }
        let mut ctx = Context::new(CONTEXT_CONTROL_INTEGER);
        // SAFETY: a thread of this process. A failed suspend
        // (u32::MAX) is a thread that has gone.
        if unsafe { SuspendThread(t.handle) } == u32::MAX {
            return None;
        }
        // GetThreadContext also waits for the suspend to take hold.
        // SAFETY: `ctx` is an aligned CONTEXT with its flags set.
        let ok = unsafe { GetThreadContext(t.handle, &mut ctx) } != 0;
        let rsp = ctx.0[RSP];
        let (low, high) = t.stack;
        let mut words = 0;
        if ok && rsp >= low && rsp < high && rsp.is_multiple_of(8) {
            words = (((high - rsp) / 8) as usize).min(COPY_WORDS);
            // SAFETY: [rsp, high) is the suspended thread's committed
            // stack (everything between RSP and the base is), and `copy`
            // has room for COPY_WORDS.
            unsafe { std::ptr::copy_nonoverlapping(rsp as *const u64, copy.as_mut_ptr(), words) };
        }
        // SAFETY: balances the suspend above.
        unsafe { ResumeThread(t.handle) };
        if !ok {
            return None;
        }
        // Zero the slack after this copy (an earlier, deeper one left
        // words there).
        let end = (words + SLACK_WORDS).min(copy.len());
        copy[words..end].fill(0);
        let depth = unwind(&mut ctx, &mut copy[..], words, buf);
        Some(Captured {
            depth,
            lr: 0,
            on_cpu,
        })
    })
}

/// Unwinds `ctx` over `copy[..words]`, the stack from the context's RSP up:
/// the interrupted PC, then each return address, into `buf`. Returns how
/// many it wrote.
fn unwind(ctx: &mut Context, copy: &mut [u64], words: usize, buf: &mut [u64]) -> usize {
    buf[0] = ctx.0[RIP];
    let mut depth = 1;
    if words == 0 {
        return depth;
    }
    let live = ctx.0[RSP];
    let base = copy.as_ptr() as u64;
    let range = (live, live + 8 * words as u64);
    relocate(&mut copy[..words], range, base);
    relocate(&mut ctx.0[GPR_FIRST..GPR_FIRST + GPR_COUNT], range, base);
    let top = base + 8 * words as u64;
    while depth < buf.len() {
        let pc = ctx.0[RIP];
        let sp = ctx.0[RSP];
        if sp < base || sp >= top || !sp.is_multiple_of(8) {
            break;
        }
        let mut image = 0u64;
        // SAFETY: a lookup in the loaded modules' tables; the target is
        // running again, so a lock it held is only waited on.
        let entry = unsafe { RtlLookupFunctionEntry(pc, &mut image, std::ptr::null_mut()) };
        if entry.is_null() {
            // A leaf function: no frame, the return address at RSP.
            // SAFETY: base <= sp < top, inside the copy.
            ctx.0[RIP] = unsafe { (sp as *const u64).read() };
            ctx.0[RSP] = sp + 8;
        } else {
            let mut data = std::ptr::null_mut();
            let mut frame = 0u64;
            // SAFETY: `ctx`'s stack pointer and frame registers point into
            // the copy (relocated above), which has zeroed slack past its
            // end for frames that reach a little beyond it.
            unsafe {
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    image,
                    pc,
                    entry,
                    ctx,
                    &mut data,
                    &mut frame,
                    std::ptr::null_mut(),
                )
            };
        }
        let (next_pc, next_sp) = (ctx.0[RIP], ctx.0[RSP]);
        // The stack only unwinds upwards; anything else is a bad table or
        // a frame outside the copy.
        if next_pc == 0 || next_sp <= sp || next_sp > top {
            break;
        }
        buf[depth] = next_pc;
        depth += 1;
    }
    depth
}
