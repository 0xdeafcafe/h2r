//! Two guest threads, one spinning in generated code on a flag the other
//! sets: the shape of the boot's stream-block purge wait (0x213fa0 on
//! [0x55bd0c]). Synthetic x86 is lifted, emitted, compiled with clang and
//! run by the real runtime under `Machine`. With preemption the spinner
//! gives up its slice at the loop's back-edge, the setter runs, and the
//! spinner sees the flag; with `preempt_slice = None` it never does.
//! Also: interrupts reaching a spinning loop, and which preemptions may let
//! logical time advance.
mod common;
use common::*;
use iced_x86::code_asm::*;
use iced_x86::{Decoder, DecoderOptions};
use kernel::{Machine, RunOutcome};
use runtime::{CpuAbi, ExecCtxOpaque, H2GenModule};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_char;
use std::path::Path;

/// `spin(flag, bound)` (stdcall): 1 once `*flag != 0`, 0 after `bound`
/// polls without it. The bound only keeps the cooperative case finite.
const SPIN: u32 = CODE + 0x1000;
/// `set(flag)` (stdcall): `*flag = 1`.
const SET: u32 = CODE + 0x2000;
/// `wait(flag)` (stdcall): spin until `*flag != 0`, then return 1. Its
/// state at the loop header is the same every trip.
const WAIT: u32 = CODE + 0x3000;
/// `sleep_set(flag)` (stdcall, a Rust override): sleep an hour of logical
/// time, then `*flag = 1`.
const SLEEP_SET: u32 = CODE + 0x300;
const FLAG: u32 = DATA + 0x40;
const HOUR: u32 = DATA + 0x120;
const BOUND: u32 = 2_000_000;

/// Assemble at `base`, lower, and split into blocks at every branch target
/// and after every transfer.
fn function(
    base: u32,
    body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> ir::IrFunction {
    let mut a = CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(base as u64).unwrap();
    let env = lift::Env::default();
    let insns: Vec<_> = Decoder::with_ip(32, &code, base as u64, DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    let mut starts = BTreeSet::from([base]);
    for i in &insns {
        match &i.control {
            ir::Control::Next => {}
            ir::Control::Branch {
                taken, fallthrough, ..
            } => starts.extend([taken.pc, fallthrough.pc]),
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: BTreeMap<u32, ir::IrBlock> = BTreeMap::new();
    let mut current = base;
    for i in insns {
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks
            .entry(current)
            .or_insert_with(|| ir::IrBlock {
                start: current,
                insns: vec![],
            })
            .insns
            .push(i);
    }
    let f = ir::IrFunction {
        entry: base,
        blocks,
        interior_entries: vec![],
        errors: vec![],
    };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

fn guest_functions() -> Vec<ir::IrFunction> {
    let spin = function(SPIN, |a| {
        let mut poll = a.create_label();
        let mut seen = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(esp + 8))?;
        a.set_label(&mut poll)?;
        a.cmp(dword_ptr(eax), 0)?;
        a.jne(seen)?;
        a.dec(ecx)?;
        a.jnz(poll)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        a.set_label(&mut seen)?;
        a.mov(eax, 1)?;
        a.ret_1(8)
    });
    let set = function(SET, |a| {
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(eax), 1)?;
        a.ret_1(4)
    });
    let wait = function(WAIT, |a| {
        let mut poll = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.set_label(&mut poll)?;
        a.cmp(dword_ptr(eax), 0)?;
        a.je(poll)?;
        a.mov(eax, 1)?;
        a.ret_1(4)
    });
    vec![spin, set, wait]
}

/// emit's test bridge (`c/bridge.c`) forwards the ABI imports through this
/// table; here it points at the runtime's own exports.
#[repr(C)]
struct Bridge {
    read: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, *const c_char, *const c_char) -> u32,
    rmw: unsafe extern "C" fn(
        *mut CpuAbi,
        *mut ExecCtxOpaque,
        u32,
        u32,
        u32,
        u32,
        u32,
        *mut u32,
    ) -> u32,
    xchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
    cmpxchg:
        unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32, u32, u32, u32, *mut u32) -> u32,
}

/// The module, built once per test process (tests run in parallel and
/// share one output directory).
fn module() -> Option<&'static libloading::Library> {
    static LIB: std::sync::OnceLock<Option<libloading::Library>> = std::sync::OnceLock::new();
    LIB.get_or_init(build).as_ref()
}

/// Build the module for this host and load it, wired to the runtime.
fn build() -> Option<libloading::Library> {
    let header = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/include/h2abi.h");
    if emit::compile::compiler().is_err() {
        eprintln!("SKIPPED preempt: no clang");
        return None;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("kernel-preempt");
    let src = dir.join("src");
    let owners = emit::plan(guest_functions());
    assert!(owners.iter().any(|o| !o.loops.back_edges.is_empty()));
    emit::write_module(&owners, &src).unwrap();
    emit::compile::install_header(&src, &header).unwrap();
    let arch = emit::compile::Arch::host();
    let b = emit::compile::build(&src, &dir, arch, emit::compile::default_jobs())
        .unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join("libkernel_preempt.dylib");
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib)
        .unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now by emit for this host.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let bridge = Bridge {
        read: runtime::ffi::h2abi_v1_read,
        write: runtime::ffi::h2abi_v1_write,
        event: runtime::generated::h2shim_v1_event,
        unsupported: runtime::generated::h2shim_v1_unsupported,
        rmw: runtime::ffi::h2abi_v1_atomic_rmw,
        xchg: runtime::ffi::h2abi_v1_xchg,
        cmpxchg: runtime::ffi::h2abi_v1_cmpxchg,
    };
    // SAFETY: bridge.c copies the table; the signatures match h2abi.h.
    unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const Bridge)> =
            lib.get(b"h2bridge_install").unwrap();
        install(&bridge);
    }
    Some(lib)
}

/// Thread 1 creates thread 2 (which will run `set(FLAG)`), then calls
/// `spin(FLAG, bound)` and exits with its result.
fn machine(lib: &libloading::Library, bound: u32) -> Machine {
    machine_with(lib, SET, SPIN, vec![FLAG, bound])
}

/// Thread 1 creates thread 2 running `start(FLAG)`, then calls
/// `spinner(args)` (stdcall) and exits with its result.
fn machine_with(lib: &libloading::Library, start: u32, spinner: u32, args: Vec<u32>) -> Machine {
    let h = Harness::new();
    h.runtime
        .memory()
        .set_u64(HOUR, (-36_000_000_000i64) as u64)
        .unwrap();
    // SAFETY: the module's tables and probes follow the current ABI and live as long
    // as `lib`, which the caller keeps loaded for the whole run.
    unsafe {
        let module: libloading::Symbol<*const H2GenModule> = lib.get(b"h2gen_module").unwrap();
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            lib.get(b"h2gen_module_abi_version").unwrap();
        let (module, version) = (&**module, *version);
        h.runtime
            .edit_dispatch(|d| d.register_generated_module(module, version))
            .unwrap();
    }
    h.code(0, move |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255), // PsCreateSystemThreadEx
            &[
                DATA,
                0,
                0x4000,
                0,
                DATA + 4,
                start,
                FLAG,
                0,
                0,
                CODE + 0x200,
            ],
            CODE + 0x100,
        )
    });
    h.code(0x100, move |cpu, ctx, _| {
        assert_eq!(cpu.eax, kernel::status::SUCCESS);
        guest_call(cpu, ctx, spinner, &args, CODE + 0x110)
    });
    // Exit code = spin's result.
    h.code(0x110, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    // The system routine: StartRoutine(StartContext), then return.
    h.code(0x200, |cpu, ctx, _| {
        let (start, context) = {
            let m = ctx.memory();
            (m.u32(cpu.esp + 4)?, m.u32(cpu.esp + 8)?)
        };
        guest_call(cpu, ctx, start, &[context], CODE + 0x210)
    });
    h.code(0x210, |cpu, ctx, _| ctx.memory().ret(cpu, 8));
    // sleep_set(flag): KeDelayExecutionThread for an hour, then set it.
    h.code(0x300, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, HOUR],
            CODE + 0x310,
        )
    });
    h.code(0x310, |cpu, ctx, _| {
        let flag = ctx.memory().u32(cpu.esp + 4)?;
        ctx.memory().set_u32(flag, 1)?;
        ctx.memory().ret(cpu, 4)
    });
    Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap()
}

#[test]
fn a_generated_spin_wait_gives_way_to_the_thread_it_waits_for() {
    let Some(lib) = module() else { return };

    let mut m = machine(lib, BOUND);
    let outcome = m.run(100).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Complete {
            exits: [(1, 1), (2, 0)].into()
        },
        "the spinner saw the flag"
    );
    assert!(m.preemptions() >= 1);
    assert_eq!(&m.schedule()[..3], &[1, 2, 1], "spin, set, spin again");

    // The purely cooperative schedule, for contrast: the spinner exhausts
    // its bound before thread 2 ever runs.
    let mut m = machine(lib, BOUND);
    m.preempt_slice = None;
    assert_eq!(
        m.run(100).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert_eq!(m.preemptions(), 0);
}

#[test]
fn an_interrupt_raised_from_another_host_thread_reaches_a_spinning_loop() {
    let Some(lib) = module() else { return };
    // Cooperative only: nothing but the interrupt can end the spin before
    // its bound, which is far longer than the raise delay.
    let mut m = machine(lib, 200_000_000);
    m.preempt_slice = None;
    let runtime = m.runtime.clone();
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        runtime.raise_interrupt(runtime::GuestInterrupt {
            target: SET,
            args: vec![FLAG],
            convention: runtime::CallConvention::Stdcall,
            thread: Some(1),
            coalesce: true,
            name: "test vblank",
        })
    });
    let start = std::time::Instant::now();
    let outcome = m.run(100).unwrap();
    assert_eq!(raiser.join().unwrap(), runtime::Raise::Queued);
    assert_eq!(
        outcome,
        RunOutcome::Complete {
            exits: [(1, 1), (2, 0)].into()
        },
        "the interrupt set the flag inside the loop"
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(100));
    assert_eq!(m.preemptions(), 0);
    assert_eq!(runtime_pending(&m), 0);
}

fn runtime_pending(m: &Machine) -> usize {
    m.runtime.pending_interrupts()
}

#[test]
fn work_that_runs_out_of_slices_does_not_move_logical_time() {
    let Some(lib) = module() else { return };
    // Thread 1 counts down 100000 polls while thread 2 sleeps an hour. Its
    // registers change every slice, so it is never taken for a spinner and
    // the hour does not pass under it: it finishes first, flag unseen.
    let mut m = machine_with(lib, SLEEP_SET, SPIN, vec![FLAG, 100_000]);
    m.preempt_slice = Some(64);
    assert_eq!(
        m.run(10_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert!(m.preemptions() > 1000, "{}", m.preemptions());
}

#[test]
fn a_spinner_waiting_on_a_sleeper_lets_logical_time_reach_it() {
    let Some(lib) = module() else { return };
    // The same wait with nothing changing between preemptions: the second
    // identical preemption marks it polling, the hour passes, the sleeper
    // sets the flag and the spinner sees it.
    let mut m = machine_with(lib, SLEEP_SET, WAIT, vec![FLAG]);
    m.preempt_slice = Some(64);
    assert_eq!(
        m.run(100).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 1), (2, 0)].into()
        }
    );
}
