//! The runtime's second executor (docs/runtime/second-executor.md) on
//! synthetic generated code, never retail bytes: lifted, emitted with the
//! inline RAM path, compiled and run through the real runtime, with the
//! main runtime mutating live RAM on one host thread while the executor
//! runs against its copy-on-write snapshot on another.

mod common;

use iced_x86::code_asm::*;
use runtime::executor::{ExecutorError, SecondExecutor};
use runtime::guest::{PAGE_SIZE, Permissions, WrittenPage};
use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT, kernel};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Executor side: sums SRC `PASSES` times over (so a run lasts long enough
/// to overlap the main side's), stores the sum into three lines of DST.
const SUM: u32 = 0x0040_0000;
/// Main side: adds 1 to every SRC word, bumps a counter in DST line 2.
const BUMP: u32 = 0x0040_0100;
/// Calls the kernel import whose token is at IMPORT.
const KCALL: u32 = 0x0040_0200;
const SRC: u32 = 0x0010_0000;
const WORDS: u32 = 4096;
const PASSES: u32 = 32;
const DST: u32 = 0x0012_0000;
const IMPORT: u32 = 0x0013_0000;
const MAIN_STACK: u32 = 0x0015_0000;
const EXEC_STACK: u32 = 0x0017_0000;

fn functions() -> Vec<ir::IrFunction> {
    vec![
        common::function(SUM, |a| {
            let mut pass = a.create_label();
            let mut l = a.create_label();
            a.xor(eax, eax)?;
            a.mov(edx, PASSES)?;
            a.set_label(&mut pass)?;
            a.mov(esi, SRC)?;
            a.mov(ecx, WORDS)?;
            a.set_label(&mut l)?;
            a.add(eax, dword_ptr(esi))?;
            a.add(esi, 4)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.dec(edx)?;
            a.jnz(pass)?;
            a.mov(edi, DST)?;
            a.mov(dword_ptr(edi), eax)?;
            a.mov(dword_ptr(edi + 0x7c0), eax)?;
            a.mov(dword_ptr(edi + 0x1040), eax)?;
            a.ret()
        }),
        common::function(BUMP, |a| {
            let mut l = a.create_label();
            a.mov(esi, SRC)?;
            a.mov(ecx, WORDS)?;
            a.set_label(&mut l)?;
            a.add(dword_ptr(esi), 1)?;
            a.add(esi, 4)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.mov(edi, DST)?;
            a.add(dword_ptr(edi + 0x80), 1)?;
            a.ret()
        }),
        common::function(KCALL, |a| {
            a.mov(eax, dword_ptr(IMPORT))?;
            a.push(0x1234)?;
            a.call(eax)?;
            a.ret()
        }),
    ]
}

/// Emit, compile and load the module with the inline RAM path, and its
/// bridge pointed at the production runtime (as perf.rs does).
/// Each test builds its own copy under `name` (tests run in parallel).
fn module(name: &str) -> Option<(libloading::Library, emit::host::Generated, Dispatch)> {
    let header = common::abi_header()?;
    let owners = emit::plan(functions());
    let codegen = emit::Codegen::from_env().unwrap();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("second-executor-{name}-{}", codegen.accounting));
    let src = dir.join("src");
    emit::write_module_with(&owners, &src, codegen).unwrap();
    let h = src.join("h2gen.h");
    let content = std::fs::read_to_string(&h).unwrap();
    std::fs::write(&h, format!("#define H2_FASTMEM 1\n{content}")).unwrap();
    emit::compile::install_header(&src, &header).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap();
    let dylib = dir.join("second-executor.dylib");
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib).unwrap();
    // SAFETY: built just now for this host.
    let host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    // SAFETY: as above.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let mut dispatch = Dispatch::default();
    // SAFETY: the callbacks implement h2abi.h; the table is the module's.
    unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const *const (), usize) -> u32> =
            lib.get(b"h2bridge_install_v2").unwrap();
        let callbacks = [
            runtime::ffi::h2abi_v1_read as *const (),
            runtime::ffi::h2abi_v1_write as *const (),
            runtime::generated::h2shim_v1_event as *const (),
            runtime::generated::h2shim_v1_unsupported as *const (),
            runtime::generated::h2shim_v1_rdtsc as *const (),
            runtime::ffi::h2abi_v1_atomic_rmw as *const (),
            runtime::ffi::h2abi_v1_xchg as *const (),
            runtime::ffi::h2abi_v1_cmpxchg as *const (),
            runtime::ffi::h2abi_v1_profile as *const (),
            runtime::ffi::h2abi_v1_fast_acquire as *const (),
            runtime::ffi::h2abi_v1_fast_release as *const (),
            runtime::ffi::h2abi_v1_read_counted as *const (),
            runtime::ffi::h2abi_v1_write_counted as *const (),
            runtime::ffi::h2abi_v1_charge as *const (),
            runtime::ffi::h2abi_v1_budget as *const (),
        ];
        assert_eq!(install(callbacks.as_ptr(), size_of_val(&callbacks)), 1);
        let table: libloading::Symbol<*const runtime::generated::H2GenModule> = lib.get(b"h2gen_module").unwrap();
        let version = lib.get::<runtime::ffi::GeneratedAbiVersion>(b"h2gen_module_abi_version").unwrap();
        dispatch.register_generated_module(&**table, *version).unwrap();
    }
    Some((lib, host, dispatch))
}

fn main_runtime(dispatch: Dispatch) -> Arc<Runtime> {
    let mut g = Guest::new();
    g.map_zeroed(SUM & !0xfff, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(SRC, WORDS * 4, Permissions::RW).unwrap();
    g.map_zeroed(DST, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(IMPORT, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(MAIN_STACK - 0x1000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(EXEC_STACK - 0x1000, 0x1000, Permissions::RW).unwrap();
    for i in 0..WORDS {
        g.set_u32(SRC + i * 4, i.wrapping_mul(0x9e37_79b9)).unwrap();
    }
    g.set_u32(IMPORT, kernel::function_token(128)).unwrap();
    g.set_u32(MAIN_STACK - 4, HARNESS_EXIT).unwrap();
    Runtime::new(g, dispatch)
}

/// One run of BUMP on the main context; the accesses it was charged.
fn bump(ctx: &mut ExecCtx) -> u64 {
    let mut cpu = CpuAbi {
        esp: MAIN_STACK - 4,
        ..CpuAbi::default()
    };
    ctx.take_slice_spent();
    let t = Runner::default().run(&mut cpu, ctx, BUMP);
    assert_eq!((t.kind, cpu.eip), (abi::STOP, HARNESS_EXIT), "{t:?} {:?}", ctx.events());
    ctx.take_slice_spent()
}

fn physical(rt: &Runtime, va: u32) -> u32 {
    let m = rt.memory();
    let map = m.mappings().iter().find(|m| m.virtual_start <= va && va - m.virtual_start < m.len).unwrap();
    map.physical_start + (va - map.virtual_start)
}

fn frozen_sum(rt: &Runtime) -> u32 {
    let m = rt.memory();
    (0..WORDS).fold(0u32, |s, i| s.wrapping_add(m.u32(SRC + i * 4).unwrap()))
}

#[test]
fn the_executor_runs_generated_code_on_a_frozen_snapshot_while_main_mutates_live_ram() {
    const NAME: &str = "the_executor_runs_generated_code_on_a_frozen_snapshot_while_main_mutates_live_ram";
    let Some((_lib, _host, dispatch)) = module(NAME) else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let main = main_runtime(dispatch);
    let mut ctx = ExecCtx::new(main.clone(), 1);
    // The main context counts its own accesses, as the scheduler's would.
    ctx.arm_preemption(0);
    let per_bump = bump(&mut ctx);
    assert!(per_bump >= WORDS as u64 * 2, "BUMP reads and writes every word: {per_bump}");
    let mut exec = SecondExecutor::new(&main, |_, _| false);

    let dst = physical(&main, DST);
    let stack_page = physical(&main, EXEC_STACK - 4) & !(PAGE_SIZE - 1);
    let expected = vec![
        WrittenPage { physical: dst, lines: 1 | 1 << 31 },
        WrittenPage { physical: dst + 0x1000, lines: 1 << 1 },
        WrittenPage { physical: stack_page, lines: 1 << 63 },
    ];
    let rounds: usize = std::env::var("H2_EXECUTOR_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let mut handed = 0u32;
    // Rounds where a whole BUMP ran while the executor's call had not ended.
    let mut overlaps = 0;
    // BUMPs so far (one measured above): DST+0x80 counts them.
    let mut counter = 1u32;
    for round in 0..rounds {
        exec.snapshot(&main);
        let want = frozen_sum(&main).wrapping_mul(PASSES);
        let bumps = 1 + round % 4;
        let running = AtomicBool::new(false);
        let done = AtomicBool::new(false);
        let (result, cpu, main_spent, overlapped) = std::thread::scope(|s| {
            let exec = &mut exec;
            let (running, done) = (&running, &done);
            let executor = s.spawn(move || {
                let mut cpu = CpuAbi {
                    esp: EXEC_STACK,
                    ..CpuAbi::default()
                };
                running.store(true, Ordering::Release);
                let r = exec.call(&mut cpu, SUM, &[]);
                done.store(true, Ordering::Release);
                (r, cpu)
            });
            // Start mutating once the executor is about to run.
            while !running.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            let mut overlapped = false;
            let spent: Vec<u64> = (0..bumps)
                .map(|_| {
                    let n = bump(&mut ctx);
                    overlapped |= !done.load(Ordering::Acquire);
                    n
                })
                .collect();
            let (r, cpu) = executor.join().unwrap();
            (r, cpu, spent, overlapped)
        });
        overlaps += usize::from(overlapped);
        let outcome = result.unwrap_or_else(|e| panic!("round {round}: {e}"));
        counter += bumps as u32;
        assert_eq!(cpu.eax, want, "round {round}: the executor summed the frozen SRC");
        assert_ne!(frozen_sum(&main).wrapping_mul(PASSES), want, "round {round}: main did change SRC meanwhile");
        assert!(outcome.accesses >= WORDS as u64, "round {round}: {outcome:?}");
        assert_eq!(main_spent, vec![per_bump; bumps], "round {round}: main's charges are its own");
        {
            let m = main.memory();
            assert_eq!(m.u32(DST).unwrap(), handed, "round {round}: the view's store did not leak");
            assert_eq!(m.u32(DST + 0x80).unwrap(), counter);
        }
        let written = exec.written();
        assert_eq!(written, expected, "round {round}");
        let dst_pages: Vec<_> = written.iter().copied().filter(|p| p.physical != stack_page).collect();
        let merged = exec.hand_back(&main, &dst_pages);
        assert_eq!((merged.lines, merged.conflicts), (3, 0), "round {round}");
        handed = want;
        let m = main.memory();
        assert_eq!(
            [m.u32(DST).unwrap(), m.u32(DST + 0x7c0).unwrap(), m.u32(DST + 0x1040).unwrap()],
            [want; 3],
            "round {round}: handed back"
        );
        assert_eq!(m.u32(DST + 0x80).unwrap(), counter, "round {round}: main's line kept");
        assert_eq!(m.u32(EXEC_STACK - 4).unwrap(), 0, "round {round}: the executor's stack stayed private");
    }
    println!("{overlaps} of {rounds} rounds overlapped");
    assert!(overlaps > 0, "the two executors never ran at the same time");
}

#[test]
fn a_kernel_call_from_generated_code_is_refused() {
    const NAME: &str = "a_kernel_call_from_generated_code_is_refused";
    let Some((_lib, _host, dispatch)) = module(NAME) else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let main = main_runtime(dispatch);
    let mut exec = SecondExecutor::new(&main, |_, _| false);
    exec.snapshot(&main);
    let mut cpu = CpuAbi {
        esp: EXEC_STACK,
        ..CpuAbi::default()
    };
    let e = std::thread::scope(|s| s.spawn(|| exec.call(&mut cpu, KCALL, &[])).join().unwrap()).unwrap_err();
    let ExecutorError::Refused(r) = &e else {
        panic!("{e:?}")
    };
    assert_eq!((r.ordinal, r.name.as_str(), r.target), (Some(128), "KeQuerySystemTime", kernel::function_token(128)));
    // The return address the call pushed: KCALL's `ret`, after the
    // 5-byte mov, 5-byte push and 2-byte call.
    assert_eq!(r.return_pc, Some(KCALL + 12));
    assert!(e.to_string().contains("KeQuerySystemTime"), "{e}");
}
