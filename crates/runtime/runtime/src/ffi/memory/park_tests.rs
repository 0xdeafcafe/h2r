//! The state word and PARK (contract change section 4.1, "No gate in
//! generated code"): native fixtures stand in for generated code.
use super::*;
use crate::cpu::ABI_VERSION;
use crate::guest::Permissions;
use crate::preempt::PARK;
use crate::{CpuAbi, Dispatch, ExecCtxOpaque, Runner, Runtime, TransferAbi, abi};
use std::sync::Mutex as StdMutex;
use std::thread::JoinHandle;

const CODE: u32 = 0x10000;
const HEADER: u32 = CODE + 0x10;
const DATA: u32 = 0x20000;

fn runtime(handlers: &[(u32, crate::GuestFn)]) -> Arc<Runtime> {
    let mut dispatch = Dispatch::default();
    for &(pc, f) in handlers {
        // SAFETY: the fixtures keep the ABI and touch the context only
        // through the pointers they are given, for the call.
        unsafe { dispatch.register_generated(pc, ABI_VERSION, f).unwrap() };
    }
    let rt = Runtime::new(Guest::new(), dispatch);
    {
        let mut m = rt.memory();
        m.map_zeroed(CODE, 0x1000, Permissions::RX).unwrap();
        m.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
    }
    rt.enable_parking();
    rt
}

/// The context behind a fixture's handle.
///
/// # Safety
/// `exec` is the live handle the runner passed, used by nothing else.
unsafe fn context<'a>(exec: *mut ExecCtxOpaque) -> &'a mut ExecCtx {
    unsafe { &mut *exec.cast::<ExecCtx>() }
}

/// Spin as a loop header polls, until a guard asks this context to park.
/// For a fixture that holds no reference to its context.
fn until_parked(exec: *mut ExecCtxOpaque) {
    // SAFETY: the runner's live handle; only the word is read.
    while unsafe { context(exec) }.leave_word() & PARK == 0 {
        std::hint::spin_loop();
    }
}

static WRITER: StdMutex<Option<JoinHandle<()>>> = StdMutex::new(None);

/// A loop at CODE: starts a guard on another thread, polls until it asks
/// for a park, and leaves to its header as a loop does. The header reads
/// what the guard wrote and stops.
unsafe extern "C" fn parked_loop(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: the runner's exclusive pair, for this call.
    let (cpu, ctx) = unsafe { (&mut *cpu, context(exec)) };
    if pc == CODE {
        let rt = ctx.runtime().clone();
        *WRITER.lock().unwrap() =
            Some(std::thread::spawn(move || rt.memory().set_u32(DATA, 7).unwrap()));
        // As a loop header polls.
        while ctx.leave_word() & PARK == 0 {
            std::hint::spin_loop();
        }
        cpu.eax += 1;
        return TransferAbi::jump(HEADER);
    }
    cpu.ebx = ctx.memory().u32(DATA).unwrap();
    TransferAbi::stop(0)
}

/// A park is not a preemption: the armed context's slice neither ends nor
/// is spent, nothing is stashed for a later run, and the run carries on
/// from the loop's header once the guard that asked has had the Guest.
#[test]
fn park_does_not_end_a_slice() {
    let rt = runtime(&[
        (CODE, parked_loop as crate::GuestFn),
        (HEADER, parked_loop as crate::GuestFn),
    ]);
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.arm_preemption(1000);
    let mut cpu = CpuAbi::default();
    let t = Runner::default().run(&mut cpu, &mut ctx, CODE);
    WRITER.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(t.kind, abi::STOP);
    assert_eq!((cpu.eax, cpu.ebx), (1, 7), "the guard went first, then the header");
    assert!(ctx.take_preempted().is_none(), "no slice ended");
    assert!(!ctx.slice.expired);
    assert_eq!(ctx.slice.left, 1000);
    assert_eq!(ctx.preempt_pending(), 0, "the park was taken");
    assert_eq!(rt.memory_state().state.load(Ordering::Relaxed), IDLE);
}

/// A handler that takes a guard on its own thread while a guard on
/// another thread waits for it to park: its helper goes "in Rust", so the
/// waiting guard has the Guest first, then the helper, and neither waits
/// for the other.
unsafe extern "C" fn helper_under_a_waiting_guard(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: the runner's live handle; the runtime is cloned out of it.
    let rt = unsafe { context(exec) }.runtime().clone();
    let writer = std::thread::spawn(move || rt.memory().set_u32(DATA, 9).unwrap());
    until_parked(exec);
    let mut out = [0u8; 4];
    // SAFETY: the live handle and a 4-byte output, as generated code calls it.
    let status = unsafe { crate::ffi::h2abi_v1_read(exec, pc, DATA, 4, 1, out.as_mut_ptr()) };
    writer.join().unwrap();
    // SAFETY: the runner's CPU, for this call.
    let cpu = unsafe { &mut *cpu };
    cpu.eax = u32::from_le_bytes(out);
    cpu.ecx = status;
    TransferAbi::stop(0)
}

#[test]
fn guard_on_helper_thread_does_not_deadlock() {
    let rt = runtime(&[(CODE, helper_under_a_waiting_guard as crate::GuestFn)]);
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let mut cpu = CpuAbi::default();
    let t = Runner::default().run(&mut cpu, &mut ctx, CODE);
    assert_eq!(t.kind, abi::STOP);
    assert_eq!((cpu.eax, cpu.ecx), (9, 0), "the waiting guard wrote first");
    assert_eq!(rt.memory_state().state.load(Ordering::Relaxed), IDLE);
    // And a guard now, with nothing running, is an ordinary one.
    rt.memory().set_u32(DATA, 10).unwrap();
    assert_eq!(rt.memory().u32(DATA).unwrap(), 10);
}
