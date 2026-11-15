//! Two guest threads, one spinning in generated code on a flag the other
//! sets: the shape of the boot's stream-block purge wait (0x213fa0 on
//! [0x55bd0c]). Synthetic x86 is lifted, emitted, compiled with clang and
//! run by the real runtime under `Machine`. With preemption the spinner
//! gives up its slice at the loop's back-edge, the setter runs, and the
//! spinner sees the flag; with `preempt_slice = None` it never does.
//! Also: interrupts reaching a spinning loop, and logical time moving with
//! the work threads do (`kernel::threads::ACCESSES_PER_TICK`).
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
/// `count_wait(flag)` (stdcall): as `wait`, counting trips in EDI.
const COUNT_WAIT: u32 = CODE + 0x4000;
/// `tsc_wait(flag)` (stdcall): as `wait`, reading the TSC every trip.
const TSC_WAIT: u32 = CODE + 0x5000;
/// `count(counter, flag)` (stdcall): increment `*counter` to `COUNT` in
/// memory, registers unchanged at the loop header, then return `*flag`.
const COUNT_UP: u32 = CODE + 0x6000;
const COUNT: u32 = 100_000;
/// `query_wait()` (stdcall): poll KeQueryPerformanceCounter until it reads
/// at least 10 ticks (1), or give up after 100000 polls (0, EDI = 0).
const QUERY_WAIT: u32 = CODE + 0x7000;
/// `sleep_set(flag)` (stdcall, a Rust override): sleep for the machine's
/// delay of logical time, then `*flag = 1`.
const SLEEP_SET: u32 = CODE + 0x300;
/// `vblank_callback(record)` (cdecl), the shape of Halo's `0x12b2a0` ->
/// `0x14280`: count the vblank in `VCOUNT`; when the record's Swap differs
/// from `LAST_SWAP`, keep it there and note the count in `SWAP_AT`.
const VBLANK_CALLBACK: u32 = CODE + 0x8000;
/// `vblank_wait(n)` (stdcall): spin, calling nothing, until `VCOUNT >= n`
/// (the frame wait's loop at `0x12b330`); return `VCOUNT`.
const VBLANK_WAIT: u32 = CODE + 0x9000;
/// `swap_wait(n)` (stdcall): spin until `LAST_SWAP >= n`; return `SWAP_AT`.
const SWAP_WAIT: u32 = CODE + 0xA000;
/// `yield_wait(n)` (stdcall): as `vblank_wait`, calling NtYieldExecution
/// every trip.
const YIELD_WAIT: u32 = CODE + 0xB000;
/// `work_yield(counter)` (stdcall): `ROUNDS` times, add `WORK` to
/// `*counter` one at a time, then NtYieldExecution; return `*counter`.
const WORK_YIELD: u32 = CODE + 0xC000;
const ROUNDS: u32 = 60;
const WORK: u32 = 100;
const VCOUNT: u32 = DATA + 0x200;
const LAST_SWAP: u32 = DATA + 0x204;
const SWAP_AT: u32 = DATA + 0x208;
const FLAG: u32 = DATA + 0x40;
const COUNTER: u32 = DATA + 0x80;
const DELAY: u32 = DATA + 0x120;
const BOUND: u32 = 2_000_000;
/// Logical-time units (100 ns).
const HOUR: u64 = 36_000_000_000;
const MILLISECOND: u64 = 10_000;

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
    let count_wait = function(COUNT_WAIT, |a| {
        let mut poll = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.xor(edi, edi)?;
        a.set_label(&mut poll)?;
        a.inc(edi)?;
        a.cmp(dword_ptr(eax), 0)?;
        a.je(poll)?;
        a.mov(eax, 1)?;
        a.ret_1(4)
    });
    let tsc_wait = function(TSC_WAIT, |a| {
        let mut poll = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut poll)?;
        a.rdtsc()?;
        a.cmp(dword_ptr(ecx), 0)?;
        a.je(poll)?;
        a.mov(eax, 1)?;
        a.ret_1(4)
    });
    let count_up = function(COUNT_UP, |a| {
        let mut again = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(edx, dword_ptr(esp + 8))?;
        a.set_label(&mut again)?;
        a.inc(dword_ptr(eax))?;
        a.cmp(dword_ptr(eax), COUNT as i32)?;
        a.jb(again)?;
        a.mov(eax, dword_ptr(edx))?;
        a.ret_1(8)
    });
    let query_wait = function(QUERY_WAIT, |a| {
        let mut poll = a.create_label();
        let mut seen = a.create_label();
        a.mov(edi, 100_000)?;
        a.set_label(&mut poll)?;
        a.call(runtime::kernel::function_token(126) as u64)?; // KeQueryPerformanceCounter
        a.cmp(eax, 10)?;
        a.jae(seen)?;
        a.dec(edi)?;
        a.jnz(poll)?;
        a.xor(eax, eax)?;
        a.ret()?;
        a.set_label(&mut seen)?;
        a.mov(eax, 1)?;
        a.ret()
    });
    let vblank_callback = function(VBLANK_CALLBACK, |a| {
        let mut done = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.inc(dword_ptr(VCOUNT))?;
        a.mov(ecx, dword_ptr(eax + 4))?;
        a.cmp(ecx, dword_ptr(LAST_SWAP))?;
        a.je(done)?;
        a.mov(dword_ptr(LAST_SWAP), ecx)?;
        a.mov(edx, dword_ptr(VCOUNT))?;
        a.mov(dword_ptr(SWAP_AT), edx)?;
        a.set_label(&mut done)?;
        a.ret()
    });
    let vblank_wait = function(VBLANK_WAIT, |a| {
        let mut poll = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut poll)?;
        a.cmp(dword_ptr(VCOUNT), ecx)?;
        a.jb(poll)?;
        a.mov(eax, dword_ptr(VCOUNT))?;
        a.ret_1(4)
    });
    let swap_wait = function(SWAP_WAIT, |a| {
        let mut poll = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4))?;
        a.set_label(&mut poll)?;
        a.cmp(dword_ptr(LAST_SWAP), ecx)?;
        a.jb(poll)?;
        a.mov(eax, dword_ptr(SWAP_AT))?;
        a.ret_1(4)
    });
    let yield_wait = function(YIELD_WAIT, |a| {
        let mut poll = a.create_label();
        let mut done = a.create_label();
        a.push(ebx)?;
        a.mov(ebx, dword_ptr(esp + 8))?;
        a.cmp(dword_ptr(VCOUNT), ebx)?;
        a.jae(done)?;
        a.set_label(&mut poll)?;
        a.call(runtime::kernel::function_token(238) as u64)?; // NtYieldExecution
        a.cmp(dword_ptr(VCOUNT), ebx)?;
        a.jb(poll)?;
        a.set_label(&mut done)?;
        a.mov(eax, dword_ptr(VCOUNT))?;
        a.pop(ebx)?;
        a.ret_1(4)
    });
    let work_yield = function(WORK_YIELD, |a| {
        let mut round = a.create_label();
        let mut unit = a.create_label();
        a.push(ebx)?;
        a.push(esi)?;
        a.push(edi)?;
        a.mov(edi, dword_ptr(esp + 16))?;
        a.mov(ebx, ROUNDS)?;
        a.set_label(&mut round)?;
        a.mov(esi, WORK)?;
        a.set_label(&mut unit)?;
        a.inc(dword_ptr(edi))?;
        a.dec(esi)?;
        a.jnz(unit)?;
        a.call(runtime::kernel::function_token(238) as u64)?; // NtYieldExecution
        a.dec(ebx)?;
        a.jnz(round)?;
        a.mov(eax, dword_ptr(edi))?;
        a.pop(edi)?;
        a.pop(esi)?;
        a.pop(ebx)?;
        a.ret_1(4)
    });
    vec![
        work_yield,
        spin,
        set,
        wait,
        count_wait,
        tsc_wait,
        count_up,
        query_wait,
        vblank_callback,
        vblank_wait,
        swap_wait,
        yield_wait,
    ]
}

/// emit's test bridge (`c/bridge.c`) forwards the ABI imports through this
/// table; here it points at the runtime's own exports.
#[repr(C)]
struct Bridge {
    read: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *mut u8) -> u32,
    write: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32, *const u8) -> u32,
    event: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, u32, u32, u32) -> u32,
    unsupported: unsafe extern "C" fn(*mut ExecCtxOpaque, u32, *const c_char, *const c_char) -> u32,
    rdtsc: unsafe extern "C" fn(*mut ExecCtxOpaque, u32) -> u64,
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

impl Bridge {
    /// What bridge.c's `h2bridge_layout` must report: each slot's offset in
    /// declaration order, then the size.
    const LAYOUT: [usize; 9] = [
        std::mem::offset_of!(Bridge, read),
        std::mem::offset_of!(Bridge, write),
        std::mem::offset_of!(Bridge, event),
        std::mem::offset_of!(Bridge, unsupported),
        std::mem::offset_of!(Bridge, rdtsc),
        std::mem::offset_of!(Bridge, rmw),
        std::mem::offset_of!(Bridge, xchg),
        std::mem::offset_of!(Bridge, cmpxchg),
        size_of::<Bridge>(),
    ];
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
        rdtsc: runtime::generated::h2shim_v1_rdtsc,
        rmw: runtime::ffi::h2abi_v1_atomic_rmw,
        xchg: runtime::ffi::h2abi_v1_xchg,
        cmpxchg: runtime::ffi::h2abi_v1_cmpxchg,
    };
    // SAFETY: bridge.c copies the table, whose layout is checked first;
    // the signatures match h2abi.h.
    unsafe {
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout").unwrap();
        let c: Vec<usize> = (0..Bridge::LAYOUT.len() as u32)
            .map(|i| layout(i))
            .collect();
        assert_eq!(
            c,
            Bridge::LAYOUT,
            "bridge.c's H2Bridge and this Bridge differ"
        );
        let install: libloading::Symbol<unsafe extern "C" fn(*const Bridge)> =
            lib.get(b"h2bridge_install").unwrap();
        install(&bridge);
    }
    Some(lib)
}

/// Thread 1 creates thread 2 (which will run `set(FLAG)`), then calls
/// `spin(FLAG, bound)` and exits with its result.
fn machine(lib: &libloading::Library, bound: u32) -> Machine {
    machine_with(lib, SET, SPIN, vec![FLAG, bound], HOUR)
}

/// A harness with the module registered and `DELAY` holding a relative
/// `delay` (100 ns units).
fn harness(lib: &libloading::Library, delay: u64) -> Harness {
    let h = Harness::new();
    h.runtime
        .memory()
        .set_u64(DELAY, (delay as i64).wrapping_neg() as u64)
        .unwrap();
    // SAFETY: the module's tables and probes follow the current ABI and
    // live as long as `lib`, which the caller keeps loaded for the run.
    unsafe {
        let module: libloading::Symbol<*const H2GenModule> = lib.get(b"h2gen_module").unwrap();
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            lib.get(b"h2gen_module_abi_version").unwrap();
        let (module, version) = (&**module, *version);
        h.runtime
            .edit_dispatch(|d| d.register_generated_module(module, version))
            .unwrap();
    }
    h
}

fn machine_of(h: &Harness) -> Machine {
    Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap()
}

/// One thread: `entry(args)`, then exit with EAX (or `then` it, if set).
fn alone(lib: &libloading::Library, entry: u32, args: Vec<u32>, delay: u64) -> Harness {
    let h = harness(lib, delay);
    h.code(0, move |cpu, ctx, _| {
        guest_call(cpu, ctx, entry, &args, CODE + 0x110)
    });
    h.code(0x110, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    h
}

/// Thread 1 creates thread 2 running `start(FLAG)`, then calls
/// `spinner(args)` (stdcall) and exits with its result. `sleep_set`
/// sleeps for `delay` (100 ns units).
fn machine_with(
    lib: &libloading::Library,
    start: u32,
    spinner: u32,
    args: Vec<u32>,
    delay: u64,
) -> Machine {
    let h = harness(lib, delay);
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
            &[0, 0, DELAY],
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
fn work_that_runs_out_of_slices_moves_time_only_by_its_cost() {
    let Some(lib) = module() else { return };
    // Thread 1 counts a register down 100000 polls while thread 2 sleeps an
    // hour. Its work is worth about a millisecond, so it finishes first,
    // flag unseen.
    let mut m = machine_with(lib, SLEEP_SET, SPIN, vec![FLAG, 100_000], HOUR);
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
fn memory_only_work_advances_time_by_exactly_its_charged_cost() {
    let Some(lib) = module() else { return };
    // The loop's registers are the same at every header; only memory
    // changes. Against an hour's sleeper it still finishes first.
    let mut m = machine_with(lib, SLEEP_SET, COUNT_UP, vec![COUNTER, FLAG], HOUR);
    m.preempt_slice = Some(3);
    assert_eq!(
        m.run(1_000_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    // With an instant setter, the clock ends at the work's cost: three
    // accesses a trip (read and write of the counter, the compare), plus
    // a handful on entry and exit.
    let mut m = machine_with(lib, SET, COUNT_UP, vec![COUNTER, FLAG], HOUR);
    m.preempt_slice = Some(3);
    assert_eq!(
        m.run(1_000_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 1), (2, 0)].into()
        }
    );
    assert_eq!(m.runtime.memory().u32(COUNTER).unwrap(), COUNT);
    let ticks = m.kernel.clock().counter();
    let cost = 3 * COUNT as u64 / kernel::threads::ACCESSES_PER_TICK;
    assert!((cost..cost + 2).contains(&ticks), "{ticks} vs {cost}");
}

/// `spinner(FLAG)` against a setter that sleeps a millisecond: the spin's
/// own work carries logical time to the setter's deadline.
fn spin_reaches_a_sleeper(spinner: u32) {
    let Some(lib) = module() else { return };
    let mut m = machine_with(lib, SLEEP_SET, spinner, vec![FLAG], MILLISECOND);
    m.preempt_slice = Some(4096);
    assert_eq!(
        m.run(10_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 1), (2, 0)].into()
        }
    );
    let ticks = m.kernel.clock().counter();
    assert!((MILLISECOND..2 * MILLISECOND).contains(&ticks), "{ticks}");
}

#[test]
fn a_spinner_waiting_on_a_sleeper_lets_logical_time_reach_it() {
    spin_reaches_a_sleeper(WAIT);
}

#[test]
fn a_counting_spinner_waiting_on_a_sleeper_lets_logical_time_reach_it() {
    spin_reaches_a_sleeper(COUNT_WAIT);
}

#[test]
fn an_rdtsc_spinner_waiting_on_a_sleeper_lets_logical_time_reach_it() {
    spin_reaches_a_sleeper(TSC_WAIT);
}

#[test]
fn a_counter_wait_at_dispatch_level_sees_its_own_work_move_time() {
    let Some(lib) = module() else { return };
    // Raised to DISPATCH_LEVEL the thread keeps the CPU across its slices;
    // their work must still move the counter it polls.
    let h = alone(lib, QUERY_WAIT, vec![], 0);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(129), // KeRaiseIrqlToDpcLevel
            &[],
            CODE + 0x108,
        )
    });
    h.code(0x108, |cpu, ctx, _| {
        guest_call(cpu, ctx, QUERY_WAIT, &[], CODE + 0x110)
    });
    let mut m = machine_of(&h);
    assert_eq!(
        m.run(1_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 1)].into()
        }
    );
    assert_eq!(m.preemptions(), 0);
    assert!(m.thread(1).unwrap().cpu.edi > 0);
}

#[test]
fn a_relative_delay_starts_after_the_work_before_it() {
    let Some(lib) = module() else { return };
    // 300004 accesses of work (3000 ticks), then a relative delay: the run
    // ends at the work's cost plus the delay, with the default budget and
    // with one slice covering it all.
    for (budget, delay) in [(runtime::preempt::DEFAULT_SLICE, 200), (1_000_000, 2_000)] {
        let h = alone(lib, COUNT_UP, vec![COUNTER, FLAG], delay);
        h.code(0x110, |cpu, ctx, _| {
            guest_call(
                cpu,
                ctx,
                runtime::kernel::function_token(99), // KeDelayExecutionThread
                &[0, 0, DELAY],
                CODE + 0x120,
            )
        });
        h.code(0x120, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
        let mut m = machine_of(&h);
        m.preempt_slice = Some(budget);
        assert!(matches!(m.run(100).unwrap(), RunOutcome::Complete { .. }));
        assert_eq!(m.kernel.clock().counter(), 3_000 + delay, "budget {budget}");
    }
}

#[test]
fn charged_work_keeps_one_history_entry_however_long_it_runs() {
    let Some(lib) = module() else { return };
    // A wait nobody ends, one tick a slice: 4097 slices of work, one entry.
    let h = alone(lib, WAIT, vec![FLAG], 0);
    let mut m = machine_of(&h);
    m.preempt_slice = Some(100);
    assert_eq!(m.run(4_097).unwrap(), RunOutcome::SliceLimit);
    let clock = m.kernel.clock();
    assert!(clock.counter() >= 4_096, "{}", clock.counter());
    assert_eq!(clock.advances(), [clock.counter()]);
    assert_eq!(clock.dropped_advances(), 0);
    assert_eq!(
        kernel::clock::Clock::replay(0, clock.advances())
            .unwrap()
            .counter(),
        clock.counter()
    );
}

#[test]
fn charges_coalesce_only_until_another_advance() {
    let mut clock = kernel::clock::Clock::new(0);
    clock.charge(2);
    clock.charge(3);
    clock.advance_to(10).unwrap();
    clock.charge(1);
    clock.charge(0);
    clock.charge(4);
    assert_eq!(clock.advances(), [5, 10, 15]);
    assert_eq!(clock.counter(), 15);
}

/// Deadline `n` of a source at `hz`: `ceil(n * 10 MHz / hz)` ticks.
fn deadline_at(hz: u64, n: u64) -> u64 {
    (n * 10_000_000).div_ceil(hz)
}

fn vblank_deadline(n: u64) -> u64 {
    deadline_at(60, n)
}

/// The spin tests run their "display" at 6 kHz: a spin works its way to
/// every deadline at `ACCESSES_PER_TICK`, 16.7 million accesses for one
/// 60 Hz vblank, which an unoptimised test runtime takes seconds over.
/// The shape is the same at any rate.
const FAST: u64 = 6_000;

/// Swaps so far (the `swap` override bumps it) and the `missed` count each
/// delivery saw.
#[derive(Clone, Default)]
struct Display {
    swaps: std::sync::Arc<std::sync::atomic::AtomicU32>,
    missed: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
}

/// A 60 Hz source standing in for D3D's: `{VBlank, Swap, Flags}` in the
/// scratch area, `VBLANK_CALLBACK(&record)` (cdecl).
fn vblank_source(display: &Display, max_catch_up: u32) -> kernel::periodic::Source {
    vblank_source_at(display, max_catch_up, kernel::periodic::Rate::HZ_60)
}

fn vblank_source_at(
    display: &Display,
    max_catch_up: u32,
    rate: kernel::periodic::Rate,
) -> kernel::periodic::Source {
    let display = display.clone();
    let (mut vblanks, mut last_swap) = (0u32, 0u32);
    kernel::periodic::Source {
        name: "test vblank",
        rate,
        max_catch_up,
        handler: Box::new(move |guest, scratch, missed| {
            vblanks += 1 + missed as u32;
            let swap = display.swaps.load(std::sync::atomic::Ordering::SeqCst);
            let flags = (swap != last_swap) as u32;
            last_swap = swap;
            guest.set_u32(scratch, vblanks)?;
            guest.set_u32(scratch + 4, swap)?;
            guest.set_u32(scratch + 8, flags)?;
            display.missed.lock().unwrap().push(missed);
            Ok(Some(runtime::GuestInterrupt {
                target: VBLANK_CALLBACK,
                args: vec![scratch],
                convention: runtime::CallConvention::Cdecl,
                thread: None,
                coalesce: false,
                name: "test vblank",
            }))
        }),
    }
}

#[test]
fn a_spin_on_the_vblank_count_ends_at_that_vblank_on_the_logical_clock() {
    let Some(lib) = module() else { return };
    // The frame wait's shape: a loop that calls nothing, waiting for the
    // callback to count three vblanks. Its own work carries the clock to
    // each deadline, and the callback runs at the next slice boundary.
    let h = alone(lib, VBLANK_WAIT, vec![3], 0);
    let display = Display::default();
    let rate = kernel::periodic::Rate { num: FAST, den: 1 };
    let id = h
        .kernel
        .add_periodic(vblank_source_at(&display, 4, rate))
        .unwrap();
    let mut m = machine_of(&h);
    m.preempt_slice = Some(4096);
    assert_eq!(
        m.run(100_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 3)].into()
        }
    );
    let ticks = m.kernel.clock().counter();
    let slice = 4096 / kernel::threads::ACCESSES_PER_TICK + 1;
    assert!(
        (deadline_at(FAST, 3)..deadline_at(FAST, 3) + slice).contains(&ticks),
        "{ticks}"
    );
    let stats = m.kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped), (3, 0));
    assert_eq!(stats.next_deadline, deadline_at(FAST, 4));
    assert_eq!(*display.missed.lock().unwrap(), [0, 0, 0]);
    assert!(m.preemptions() > 100, "{}", m.preemptions());
    // DPCs ran on their own context, between the spinner's slices.
    assert!(m.schedule().contains(&kernel::threads::DPC_THREAD_ID));
}

#[test]
fn a_yielding_vblank_wait_jumps_straight_to_each_deadline() {
    let Some(lib) = module() else { return };
    // Alone and yielding, the thread is polling: the clock jumps to the
    // next vblank instead of being worked through, and lands exactly on it.
    let h = alone(lib, YIELD_WAIT, vec![3], 0);
    let display = Display::default();
    h.kernel.add_periodic(vblank_source(&display, 4)).unwrap();
    let mut m = machine_of(&h);
    assert_eq!(
        m.run(1_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 3)].into()
        }
    );
    assert_eq!(m.kernel.clock().counter(), vblank_deadline(3));
    assert_eq!(m.preemptions(), 0);
    assert_eq!(*display.missed.lock().unwrap(), [0, 0, 0]);
}

#[test]
fn a_wait_for_a_swap_sees_it_at_the_next_vblank() {
    let Some(lib) = module() else { return };
    // Wait two vblanks, swap, then spin until the callback has seen the
    // swap: it does at the third vblank, whose record says SWAPDONE.
    let h = alone(lib, VBLANK_WAIT, vec![2], 0);
    let display = Display::default();
    let rate = kernel::periodic::Rate { num: FAST, den: 1 };
    h.kernel
        .add_periodic(vblank_source_at(&display, 4, rate))
        .unwrap();
    let swaps = display.swaps.clone();
    h.code(0x110, move |cpu, ctx, _| {
        swaps.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        guest_call(cpu, ctx, SWAP_WAIT, &[1], CODE + 0x120)
    });
    h.code(0x120, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let mut m = machine_of(&h);
    m.preempt_slice = Some(4096);
    assert_eq!(
        m.run(100_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 3)].into()
        }
    );
    assert_eq!(m.runtime.memory().u32(LAST_SWAP).unwrap(), 1);
    let ticks = m.kernel.clock().counter();
    assert!(
        (deadline_at(FAST, 3)..deadline_at(FAST, 4)).contains(&ticks),
        "{ticks}"
    );
}

#[test]
fn a_clock_jump_delivers_every_vblank_spread_over_slices() {
    let Some(lib) = module() else { return };
    // A second passes with no wake pass in between: 60 periods due. The
    // guest counter (Halo's 0x485AB0 counts calls) must reach 60, but four
    // a drain: thread slices run between the batches, not one burst.
    let h = alone(lib, VBLANK_WAIT, vec![60], 0);
    let display = Display::default();
    let id = h.kernel.add_periodic(vblank_source(&display, 4)).unwrap();
    h.kernel.advance_time(&h.runtime, 10_000_000).unwrap();
    let mut m = machine_of(&h);
    m.preempt_slice = Some(4096);
    assert_eq!(
        m.run(1_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 60)].into()
        }
    );
    assert_eq!(m.runtime.memory().u32(VCOUNT).unwrap(), 60);
    let stats = m.kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped, stats.owed), (60, 0, 0));
    assert_eq!(*display.missed.lock().unwrap(), [0; 60]);
    assert_eq!(stats.next_deadline, vblank_deadline(61));
    // No batch is longer than four, and the spinner ran between them.
    let dpc = kernel::threads::DPC_THREAD_ID;
    let runs: Vec<usize> = m
        .schedule()
        .split(|t| *t != dpc)
        .map(|r| r.len())
        .filter(|n| *n > 0)
        .collect();
    assert_eq!(runs, [4; 15]);
    // The catch-up held the clock: no idle jump while periods were owed.
    assert!(m.kernel.clock().counter() < vblank_deadline(61));
}

#[test]
fn cooperating_workers_are_not_charged_a_vblank_per_round() {
    let Some(lib) = module() else { return };
    // Two threads each do 60 rounds of real work (200 accesses), yielding
    // after each. Their handoffs are productive, so with a 60 Hz source
    // installed the clock moves by their work, not to a vblank per round.
    let display = Display::default();
    let mut m = machine_with(lib, WORK_YIELD, WORK_YIELD, vec![COUNTER], HOUR);
    m.kernel.add_periodic(vblank_source(&display, 4)).unwrap();
    assert_eq!(
        m.run(10_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, ROUNDS * WORK), (2, 0)].into()
        }
    );
    assert_eq!(m.runtime.memory().u32(FLAG).unwrap(), ROUNDS * WORK);
    let ticks = m.kernel.clock().counter();
    let work = 2 * (ROUNDS * WORK * 2) as u64 / kernel::threads::ACCESSES_PER_TICK;
    assert!((work..work + 100).contains(&ticks), "{ticks} vs {work}");
    assert!(ticks < vblank_deadline(1));
    assert!(display.missed.lock().unwrap().is_empty());
}
