//! Synthetic generated-code measurements through the real runtime, never retail bytes.
mod common;
use iced_x86::{Decoder, DecoderOptions, code_asm::*};
use ir::{IrBlock, IrFunction};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runner, Runtime};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Instant,
};
const A: u32 = 0x400000;
const DATA: u32 = 0x108000;
const STACK: u32 = 0x109000;
fn function(
    base: u32,
    body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> IrFunction {
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
            } => {
                starts.extend([taken.pc, fallthrough.pc]);
            }
            ir::Control::Jump {
                target: ir::Target::Direct(t),
                ..
            } => {
                starts.extend([*t, i.next_pc()]);
            }
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: BTreeMap<u32, IrBlock> = BTreeMap::new();
    let mut current = base;
    for mut i in insns {
        if let ir::Control::Jump {
            target: ir::Target::Direct(target),
            tail,
            ..
        } = &mut i.control
        {
            *tail =
                (*target as u64) < base as u64 || *target as u64 >= base as u64 + code.len() as u64;
        }
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks
            .entry(current)
            .or_insert_with(|| IrBlock {
                start: current,
                insns: vec![],
            })
            .insns
            .push(i);
    }
    let f = IrFunction {
        entry: base,
        blocks,
        interior_entries: vec![],
        errors: vec![],
    };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

fn module(kind: &str, n: u32, instrumented: bool, fast: bool) -> (libloading::Library, Dispatch) {
    let f = function(A, |a| {
        let mut l = a.create_label();
        a.mov(ecx, n)?;
        a.xor(eax, eax)?;
        a.mov(esi, DATA)?;
        a.set_label(&mut l)?;
        match kind {
            "read" => a.add(eax, dword_ptr(esi))?,
            "write" => a.mov(dword_ptr(esi), eax)?,
            "x87" => {
                a.fld1()?;
                a.fstp(st0)?;
            }
            // A compiled function's usual run: saves, loads, a store, restores.
            "frame" => {
                a.push(ebx)?;
                a.push(edi)?;
                a.mov(ebx, dword_ptr(esi))?;
                a.add(ebx, dword_ptr(esi + 4))?;
                a.mov(dword_ptr(esi + 8), ebx)?;
                a.pop(edi)?;
                a.pop(ebx)?;
            }
            "call" | "tail" => a.call((A + 0x100) as u64)?,
            _ => a.add(eax, ecx)?,
        }
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    });
    let mut funcs = vec![f];
    if kind == "call" || kind == "tail" {
        funcs.push(function(A + 0x100, |a| {
            if kind == "tail" {
                a.jmp((A + 0x200) as u64)
            } else {
                a.ret()
            }
        }));
        if kind == "tail" {
            funcs.push(function(A + 0x200, |a| a.ret()));
        }
    }
    // The benchmark builds as the image does: accounting unless H2_ACCESS_COUNT=0.
    let codegen = if kind == "frame" || std::env::var_os("H2_ACCESS_COUNT").is_some() {
        emit::Codegen::from_env().unwrap()
    } else {
        emit::Codegen::default()
    };
    compile_module_with(kind, funcs, instrumented, fast, emit::opt::Passes::from_env().unwrap(), codegen)
}
fn compile_module(kind: &str, funcs: Vec<IrFunction>, instrumented: bool, fast: bool) -> (libloading::Library, Dispatch) {
    let passes = emit::opt::Passes::from_env().unwrap();
    compile_module_with(kind, funcs, instrumented, fast, passes, emit::Codegen::default())
}
fn compile_module_with(
    kind: &str,
    funcs: Vec<IrFunction>,
    instrumented: bool,
    fast: bool,
    passes: emit::opt::Passes,
    codegen: emit::Codegen,
) -> (libloading::Library, Dispatch) {
    let owners = emit::plan_with(funcs, passes);
    let accounting = codegen.accounting;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("perf-{kind}-{instrumented}-{fast}-{accounting}-{}", passes.spec()));
    let src = dir.join("src");
    emit::write_module_with(&owners, &src, codegen).unwrap();
    if fast {
        let header = src.join("h2gen.h");
        let content = std::fs::read_to_string(&header).unwrap();
        std::fs::write(header, format!("#define H2_FASTMEM 1\n{content}")).unwrap();
    }
    if instrumented {
        let header = src.join("h2gen.h");
        let content = std::fs::read_to_string(&header).unwrap();
        std::fs::write(header, format!("#define H2_PERF 1\n{content}")).unwrap();
    }
    emit::compile::install_header(&src, &common::abi_header().unwrap()).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap();
    println!(
        "{kind}: clang {:.3}s, {} compiled, {} object bytes",
        b.compile_secs, b.compiled, b.object_bytes
    );
    let dylib = dir.join("perf.dylib");
    let probe = r#"#include "h2gen.h"
uint32_t h2probe_read(ExecCtxOpaque *c, uint32_t a, uint32_t n, uint32_t align, uint8_t *out) {
    H2FastAccess *f = h2abi_v1_fast_acquire(c);
    uint32_t status = h2_mem_read(f, c, 0, a, n, align, out);
    h2abi_v1_fast_release(c);
    return status;
}
uint32_t h2probe_write(ExecCtxOpaque *c, uint32_t a, uint32_t n, uint32_t align, const uint8_t *in) {
    H2FastAccess *f = h2abi_v1_fast_acquire(c);
    uint32_t status = h2_mem_write(f, c, 0, a, n, align, in);
    h2abi_v1_fast_release(c);
    return status;
}
"#;
    let mut extras = vec![("bridge.c", emit::host::BRIDGE)];
    if kind == "fast-semantics" { extras.push(("probe.c", probe)); }
    emit::compile::host_dylib(&b, &src, &extras, &dylib).unwrap();
    // Initialize x87's helper bridge, then replace the memory/event callbacks
    // with the production runtime. Both bridges implement h2abi.h exactly.
    let _test_host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let mut dispatch = Dispatch::default();
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
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout_v2").unwrap();
        assert_eq!(layout(callbacks.len() as u32), size_of_val(&callbacks));
        assert_eq!(install(std::ptr::null(), 0), 0);
        assert_eq!(install(callbacks.as_ptr(), size_of_val(&callbacks)), 1);
        let table: libloading::Symbol<*const runtime::generated::H2GenModule> =
            lib.get(b"h2gen_module").unwrap();
        let version = lib
            .get::<runtime::ffi::GeneratedAbiVersion>(b"h2gen_module_abi_version")
            .unwrap();
        dispatch
            .register_generated_module(&**table, *version)
            .unwrap();
    }
    (lib, dispatch)
}

#[test]
#[ignore = "benchmark; run release with --nocapture"]
fn generated_runtime_benchmark() {
    let n: u32 = std::env::var("H2_PERF_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000_000);
    let trials: usize = std::env::var("H2_PERF_TRIALS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    for kind in ["register", "read", "write", "frame", "x87", "call", "tail"] {
        if std::env::var("H2_PERF_KIND").is_ok_and(|k| k != kind) {
            continue;
        }
        let (_lib, dispatch) = module(
            kind,
            n,
            std::env::var_os("H2_PERF").is_some_and(|v| v == "1"),
            std::env::var_os("H2_FASTMEM").is_none_or(|v| v != "0"),
        );
        let mut mem = Guest::new();
        mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
        let mappings: u32 = std::env::var("H2_PERF_MAPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        for i in 0..mappings {
            mem.map_zeroed(0x1000000 + i * 8192, 4096, Permissions::RW)
                .unwrap();
        }
        mem.set_u32(DATA, 1).unwrap();
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let rt = Runtime::new(mem, dispatch);
        let mut ctx = ExecCtx::new(rt, 1);
        ctx.enable_profiling(std::env::var_os("H2_PERF").is_some_and(|v| v == "1"));
        let mut times = Vec::new();
        for _ in 0..trials {
            let mut cpu = CpuAbi {
                esp: STACK,
                ..CpuAbi::default()
            };
            let start = Instant::now();
            let t = Runner {
                max_transfers: 4 * n as u64 + 10,
            }
            .run(&mut cpu, &mut ctx, A);
            times.push(start.elapsed().as_secs_f64() * 1e9 / n as f64);
            assert_eq!(t.kind, runtime::abi::STOP, "{t:?}, {:?}", ctx.events());
            assert_eq!(cpu.eip, runtime::exec::HARNESS_EXIT);
            if kind == "read" {
                assert_eq!(cpu.eax, n);
            }
        }
        if let Some(profile) = ctx.profile() {
            println!("{profile}");
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{kind}: n={n}, trials={trials}, min={:.3} median={:.3} max={:.3} ns/iteration",
            times[0],
            times[trials / 2],
            times[trials - 1]
        );
    }
}

#[test]
fn generated_counts_match_executed_work() {
    for kind in ["read", "x87", "call", "tail"] {
        let (_lib, dispatch) = module(kind, 11, true, true);
        let mut mem = Guest::new();
        mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let mut ctx = ExecCtx::new(Runtime::new(mem, dispatch), 1);
        ctx.enable_profiling(true);
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        Runner::default().run(&mut cpu, &mut ctx, A);
        assert_eq!(cpu.eip, runtime::exec::HARNESS_EXIT);
        let p = ctx.take_profile().unwrap();
        let transfers = match kind {
            "call" => 23,
            "tail" => 34,
            _ => 1,
        };
        assert_eq!(p.transfers, transfers);
        // With direct calls (on unless H2_DIRECT_CALLS=0) the first trip
        // goes through the runner, which fills the context's slots; the
        // other ten call B directly (and B tail-jumps to C directly), and
        // the return to A continues inline. Transfers count the same; those
        // made directly pass no runner safe point, and the caller's handler
        // does not exit in between.
        let direct = std::env::var("H2_DIRECT_CALLS").as_deref() != Ok("0");
        let (direct_transfers, inline_returns) = match kind {
            "call" if direct => (20, 10),
            "tail" if direct => (30, 10),
            _ => (0, 0),
        };
        assert_eq!(p.direct_transfers, direct_transfers);
        assert_eq!(p.runner_safe_points, transfers + 1 - direct_transfers);
        assert_eq!(p.generated_batches, transfers - inline_returns);
        assert_eq!(
            p.loop_safe_points,
            if kind == "call" || kind == "tail" {
                0
            } else {
                11
            }
        );
        assert_eq!(p.x87_helpers, if kind == "x87" { 22 } else { 0 });
        assert_eq!(
            p.memory_accessors,
            match kind {
                "read" => 12,
                "call" | "tail" => 23,
                _ => 1,
            }
        );
        assert_eq!(p.timing(runtime::perf::Scope::Runner).calls, 1);
        assert_eq!(ctx.profile().unwrap(), &runtime::perf::Profile::default());
    }
}

/// Access accounting composes with the inline RAM path: a module that counts
/// its own accesses (inline or through the slow accessors, one access here
/// crossing a page) gives up its slice at the same safe points with the same
/// accesses spent as the charging module, with inline RAM on or off and
/// with the IR passes off or on (lazy flags settled on the way out of a
/// poll, alongside the count).
#[test]
fn counted_inline_ram_gives_up_its_slice_where_charged_inline_ram_does() {
    use runtime::{TransferAbi, abi};
    let f = || {
        function(A, |a| {
            let mut l = a.create_label();
            a.xor(eax, eax)?;
            a.set_label(&mut l)?;
            a.add(eax, dword_ptr(esi))?;
            a.mov(dword_ptr(edi), eax)?;
            a.add(esi, 4)?;
            a.add(edi, 4)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        })
    };
    use emit::opt::Passes;
    // The charging build without the passes first: every build must match it.
    let combos = [(false, Passes::NONE), (true, Passes::NONE), (false, Passes::ALL), (true, Passes::ALL)];
    let builds = combos.map(|(accounting, passes)| {
        let built = compile_module_with("fast-accounting", vec![f()], false, true, passes, emit::Codegen { accounting });
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("perf-fast-accounting-false-true-{accounting}-{}", passes.spec()))
            .join("src");
        let c: String = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "c"))
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        // The combination under test is really there: the loop's flags are
        // left pending, and the count is kept in the handler.
        assert_eq!(c.contains("lzm = "), passes.lazy_flags, "accounting {accounting} passes {passes:?}");
        assert_eq!(c.contains("#define H2_ACCOUNTING 1"), accounting, "accounting {accounting} passes {passes:?}");
        ((accounting, passes), built)
    });
    // The 20th read crosses into the stack's page; the writes stay below it.
    let (src, dst, n) = (DATA + 0xfb2, DATA + 0x400, 20);
    let mut runs = Vec::new();
    let memory = || {
        let mut mem = Guest::new();
        mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
        for i in 0..2048 {
            mem.set_u32(DATA + 4 * i, i.wrapping_mul(0x9e37_79b9)).unwrap();
        }
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        mem
    };
    for ((accounting, passes), (_lib, dispatch)) in builds {
        let rt = Runtime::new(memory(), dispatch);
        for fast in [false, true] {
            for slice in [1, 2, 3, 5, 7, 64, 1000] {
                *rt.memory() = memory();
                let mut ctx = ExecCtx::new(rt.clone(), 1);
                ctx.enable_fast_memory(fast);
                ctx.enable_profiling(true);
                ctx.arm_preemption(slice);
                let mut cpu = CpuAbi { esp: STACK, esi: src, edi: dst, ecx: n, ..CpuAbi::default() };
                let mut t = TransferAbi::jump(A);
                let mut stops = Vec::new();
                let end = loop {
                    let end = Runner::default().run_from(&mut cpu, &mut ctx, t);
                    let Some(next) = ctx.take_preempted() else { break end };
                    stops.push((next.target_pc, ctx.take_slice_spent()));
                    ctx.arm_preemption(slice);
                    t = next;
                };
                assert_eq!((end.kind, cpu.eip), (abi::STOP, runtime::exec::HARNESS_EXIT), "{end:?}");
                let spent = stops.iter().map(|s| s.1).sum::<u64>() + ctx.take_slice_spent();
                let key = format!("accounting {accounting} passes {} fast {fast} slice {slice}", passes.spec());
                assert_eq!(spent, 2 * n as u64 + 1, "{key}");
                let p = ctx.take_profile().unwrap();
                assert_eq!(p.memory_accessors, spent);
                let mut out = vec![0; 8192];
                rt.memory().read_bytes(DATA, &mut out, 1).unwrap();
                runs.push((key, (stops, cpu, out)));
            }
        }
    }
    let (charging, others) = runs.split_at(runs.len() / combos.len());
    for (i, (key, got)) in others.iter().enumerate() {
        let (base, want) = &charging[i % charging.len()];
        assert_eq!(got, want, "{key} against {base}");
    }
    for (key, want) in charging {
        // Every slice ends in the same state.
        assert_eq!(&charging[0].1.1, &want.1, "{key}");
        assert_eq!(&charging[0].1.2, &want.2, "{key}");
    }
}

#[test]
fn inline_ram_checks_track_mapping_changes_faults_writes_and_slice_budget() {
    use runtime::{abi, guest::FaultReason, watch::CONTIGUOUS_BASE};
    const SP: u32 = 0x200000;
    const ALIAS: u32 = 0x300000;
    let f = function(A, |a| {
        a.mov(eax, dword_ptr(esi))?;
        a.mov(dword_ptr(edi), eax)?;
        a.ret()
    });
    let (lib, dispatch) = compile_module("fast-semantics", vec![f], true, true);
    let make_memory = || {
        let mut m = Guest::new();
        m.map(A, 4096, 0, Permissions::RX).unwrap();
        m.map(DATA, 8192, 4096, Permissions::RW).unwrap();
        m.map(SP, 4096, 12288, Permissions::RW).unwrap();
        m.map(ALIAS, 4096, 4096, Permissions::RW).unwrap();
        m.set_u32(SP, runtime::exec::HARNESS_EXIT).unwrap();
        m.set_u32(DATA, 0x12345678).unwrap();
        m
    };
    let rt = Runtime::new(make_memory(), dispatch);
    let watch = {
        let mut m = rt.memory();
        let watch = m.subscribe_writes("fast test");
        m.watch_writes(watch, CONTIGUOUS_BASE + 4096, 4096).unwrap();
        watch
    };
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.enable_profiling(true);
    let mut cpu = CpuAbi { esp: SP, esi: DATA, edi: ALIAS + 8, ..CpuAbi::default() };
    let run = |cpu: &mut CpuAbi, ctx: &mut ExecCtx| {
        cpu.esp = SP;
        Runner::default().run(cpu, ctx, A)
    };
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::STOP);
    assert_eq!(rt.memory().u32(DATA + 8).unwrap(), 0x12345678);
    let p = ctx.take_profile().unwrap();
    assert_eq!((p.memory_accessors, p.fast_memory_accesses), (3, 3));
    let dirty = rt.memory().take_dirty(watch).unwrap();
    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0].addr, CONTIGUOUS_BASE + 4096);

    // A slow cross-page read followed by fast stores still expires at the
    // second access, and accounts for the return-address read past expiry.
    rt.memory().set_u32(DATA + 4094, 0xaabbccdd).unwrap();
    cpu.esi = DATA + 4094;
    ctx.arm_preemption(2);
    let stop = run(&mut cpu, &mut ctx);
    assert_eq!((stop.kind, stop.event_id), (abi::STOP, 0));
    assert_eq!(ctx.take_preempted().unwrap().target_pc, runtime::exec::HARNESS_EXIT);
    assert_eq!(ctx.take_slice_spent(), 3);
    let p = ctx.take_profile().unwrap();
    assert_eq!((p.memory_accessors, p.fast_memory_accesses), (3, 2));
    assert_eq!(rt.memory().u32(DATA + 8).unwrap(), 0xaabbccdd);
    ctx.disarm_preemption();

    // The page table must immediately observe protection changes; slow
    // cross-page failure must not write its valid prefix.
    rt.memory().protect_range(DATA + 4096, 4096, Permissions::NONE).unwrap();
    let before = rt.memory().u16(DATA + 4094).unwrap();
    cpu.esi = DATA;
    cpu.edi = DATA + 4094;
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::FAULT);
    assert_eq!(rt.memory().u16(DATA + 4094).unwrap(), before);
    assert_eq!(ctx.take_profile().unwrap().memory_accessors, 2);
    rt.memory().protect_range(DATA, 4096, Permissions::READ).unwrap();
    cpu.edi = DATA + 8;
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::FAULT);
    rt.memory().unmap(DATA, 4096).unwrap();
    assert_eq!(rt.memory().u32(DATA).unwrap_err().reason, FaultReason::Unmapped);
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::FAULT);
    rt.memory().map(DATA, 4096, 16384, Permissions::RW).unwrap();
    rt.memory().set_u32(DATA, 0x87654321).unwrap();
    cpu.edi = ALIAS + 8;
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::STOP);
    assert_eq!(rt.memory().u32(ALIAS + 8).unwrap(), 0x87654321);

    // A safe caller can replace Guest wholesale. Cached context views must
    // neither retain the freed RAM/page table nor miss the new storage.
    *rt.memory() = make_memory();
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::STOP);
    assert_eq!(rt.memory().u32(DATA + 8).unwrap(), 0x12345678);

    rt.memory().protect(A, Permissions::READ).unwrap();
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::FAULT);
    rt.memory().unmap(A, 4096).unwrap();
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::FAULT);
    rt.memory().map(A, 4096, 0, Permissions::RX).unwrap();
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::STOP);
    ctx.take_profile();
    ctx.enable_fast_memory(false);
    assert_eq!(run(&mut cpu, &mut ctx).kind, abi::STOP);
    let p = ctx.take_profile().unwrap();
    assert_eq!((p.memory_accessors, p.fast_memory_accesses), (3, 0));
    ctx.enable_fast_memory(true);

    // Exercise C's actual checks and copies for every accessor width.
    type Read = unsafe extern "C" fn(*mut runtime::ExecCtxOpaque, u32, u32, u32, *mut u8) -> u32;
    type Write = unsafe extern "C" fn(*mut runtime::ExecCtxOpaque, u32, u32, u32, *const u8) -> u32;
    // SAFETY: these test probes are compiled above with exactly these signatures.
    let read: libloading::Symbol<Read> = unsafe { lib.get(b"h2probe_read") }.unwrap();
    let write: libloading::Symbol<Write> = unsafe { lib.get(b"h2probe_write") }.unwrap();
    let ptr = (&mut ctx as *mut ExecCtx).cast();
    for width in [1u32, 2, 4, 8, 10, 16] {
        let input = [0x5a; 16];
        let mut output = [0xa5; 16];
        // SAFETY: disjoint valid buffers and exclusive invocation context.
        unsafe {
            assert_eq!(write(ptr, DATA + 1, width, 1, input.as_ptr()), 0);
            assert_eq!(read(ptr, DATA + 1, width, 1, output.as_mut_ptr()), 0);
        }
        assert_eq!(&output[..width as usize], &input[..width as usize]);
        assert!(output[width as usize..].iter().all(|v| *v == 0xa5));
    }
    let input = [0x11; 16];
    let mut output = [0xa5; 16];
    unsafe {
        assert_ne!(read(ptr, DATA + 1, 16, 16, output.as_mut_ptr()), 0);
        assert_ne!(write(ptr, DATA + 1, 16, 16, input.as_ptr()), 0);
        assert_ne!(read(ptr, 0xfd000000, 4, 1, output.as_mut_ptr()), 0);
        assert_ne!(read(ptr, u32::MAX, 4, 1, output.as_mut_ptr()), 0);
    }
    assert_eq!(output, [0xa5; 16]);

    // Byte-granular virtual mappings and shifted physical backing cannot
    // grant access to the rest of a page or lose cross-physical-page dirt.
    const PART: u32 = 0x500001;
    let mut partial = Guest::new();
    partial.map(PART, 4096, 0x1001, Permissions::RW).unwrap();
    partial.set_u32(PART, 0x11223344).unwrap();
    let partial_rt = Runtime::new(partial, Dispatch::default());
    let mut partial_ctx = ExecCtx::new(partial_rt.clone(), 3);
    partial_ctx.enable_profiling(true);
    let partial_ptr = (&mut partial_ctx as *mut ExecCtx).cast();
    let mut bytes = [0xa5; 4];
    // SAFETY: the probe holds only its scoped view and uses disjoint locals.
    unsafe {
        assert_eq!(read(partial_ptr, PART, 4, 1, bytes.as_mut_ptr()), 0);
        assert_eq!(bytes, 0x11223344u32.to_le_bytes());
        assert_ne!(read(partial_ptr, PART - 1, 1, 1, bytes.as_mut_ptr()), 0);
        assert_ne!(write(partial_ptr, PART + 4095, 4, 1, input.as_ptr()), 0);
    }
    assert_eq!(partial_ctx.take_profile().unwrap().fast_memory_accesses, 0);
    partial_rt.memory().protect(PART, Permissions::READ).unwrap();
    partial_rt.memory().protect(PART, Permissions::RW).unwrap();
    unsafe {
        assert_ne!(read(partial_ptr, PART - 1, 1, 1, bytes.as_mut_ptr()), 0);
        assert_eq!(read(partial_ptr, PART, 4, 1, bytes.as_mut_ptr()), 0);
    }
    assert_eq!(partial_ctx.take_profile().unwrap().fast_memory_accesses, 0);
    // Even a full virtual page falls back when its physical page is shifted.
    const SHIFT: u32 = 0x600000;
    partial_rt.memory().map(SHIFT, 4096, 0x4001, Permissions::RW).unwrap();
    let w = partial_rt.memory().subscribe_writes("shift");
    partial_rt.memory().watch_writes(w, CONTIGUOUS_BASE + 0x4000, 8192).unwrap();
    unsafe { assert_eq!(write(partial_ptr, SHIFT + 4094, 2, 1, input.as_ptr()), 0); }
    let dirty = partial_rt.memory().take_dirty(w).unwrap();
    assert_eq!(dirty.len(), 1);
    assert_eq!((dirty[0].addr, dirty[0].len), (CONTIGUOUS_BASE + 0x4000, 8192));
    partial_rt.memory().protect_range(SHIFT, 4096, Permissions::READ).unwrap();
    unsafe { assert_eq!(read(partial_ptr, SHIFT, 4, 1, bytes.as_mut_ptr()), 0); }
    assert_eq!(partial_ctx.take_profile().unwrap().fast_memory_accesses, 0);

    // A compound Rust guard is indivisible to C, even though the host writes
    // two separate words. A C read must never observe a torn pair.
    rt.memory().set_u64(DATA, 0).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let host_rt = rt.clone();
    let host_barrier = barrier.clone();
    let writer = std::thread::spawn(move || {
        host_barrier.wait();
        for i in 1..10000u32 {
            let mut m = host_rt.memory();
            m.set_u32(DATA, i).unwrap();
            m.set_u32(DATA + 4, i).unwrap();
        }
    });
    let mut concurrent = ExecCtx::new(rt.clone(), 2);
    let ptr = (&mut concurrent as *mut ExecCtx).cast();
    barrier.wait();
    for _ in 0..10000 {
        let mut pair = [0u8; 8];
        // SAFETY: the probe runs synchronously with a valid exclusive context;
        // its RAM gate synchronizes the separate host thread's memory guard.
        assert_eq!(unsafe { read(ptr, DATA, 8, 1, pair.as_mut_ptr()) }, 0);
        assert_eq!(&pair[..4], &pair[4..]);
    }
    writer.join().unwrap();
}

/// The bulk path is only reached when the emitter recognizes the lifted MOVS
/// iteration. If the lifter's shape drifts, the comparison below would pass
/// with both runs on the ordinary path, so check the emitted call itself.
#[test]
fn lifted_rep_movs_reaches_the_bulk_copy() {
    for accounting in [false, true] {
        for width in [1u32, 2, 4] {
            let f = function(A, |a| {
                match width { 1 => a.rep().movsb()?, 2 => a.rep().movsw()?, _ => a.rep().movsd()? }
                a.stosd()?;
                a.ret()
            });
            let owners = emit::plan_with(vec![f], emit::opt::Passes::ALL);
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("rep-movs-shape-{width}-{accounting}"));
            emit::write_module_with(&owners, &dir, emit::Codegen { accounting }).unwrap();
            let mut c = String::new();
            for e in std::fs::read_dir(&dir).unwrap() {
                let path = e.unwrap().path();
                if path.extension().is_some_and(|x| x == "c") {
                    c += &std::fs::read_to_string(path).unwrap();
                }
            }
            let calls = c.matches("h2_fast_movs(").count();
            assert_eq!(calls, 1, "width={width} accounting={accounting}");
            assert!(c.contains(&format!("ecx, {width}u, H2_FLAG(10)")), "width={width}");
        }
    }
}

/// Real RAM, aliases and page faults through both generated REP paths.
#[test]
fn bulk_rep_movs_preserves_overlap_fault_progress_watch_and_accounting() {
    use runtime::{TransferAbi, abi, watch::CONTIGUOUS_BASE};
    const SP: u32 = 0x200000;
    const ALIAS: u32 = 0x300000;
    for width in [1, 2, 4] {
        let f = function(A, |a| {
            match width { 1 => a.rep().movsb()?, 2 => a.rep().movsw()?, _ => a.rep().movsd()? }
            a.ret()
        });
        let (_lib, dispatch) = compile_module_with(
            &format!("bulk-rep-{width}"), vec![f], false, true,
            emit::opt::Passes::ALL, emit::Codegen { accounting: true },
        );
        let rt = Runtime::new(Guest::new(), dispatch);
        for (src, dst, count) in [
            (DATA + 32, DATA + 800, 100),
            (DATA + 32, DATA + 33, 100), // propagating forward overlap
            (DATA + 33, DATA + 32, 100), // memmove-safe forward overlap
            (DATA + 400, DATA + 399, 100), // propagating reverse overlap
            (DATA + 399, DATA + 400, 100),
            (DATA + 32, ALIAS + 33, 100), // physical overlap through aliases
            (DATA + 4080, DATA + 800, 100), // source page crossing
            (DATA + 800, DATA + 4080, 100), // destination page crossing
            (DATA + 800, DATA + 8176, 100), // partial-progress write fault
            (0xfd00_0000, DATA + 32, 5), // MMIO must retain its diagnostic
            (u32::MAX - 1, DATA + 32, 5),
            (DATA + 32, DATA + 800, 0),
        ] {
            for reverse in [false, true] {
                for slice in [1, 3, 10000] {
                    let mut results = Vec::new();
                    for fast in [false, true] {
                        let mut mem = Guest::new();
                        mem.map(A, 4096, 0, Permissions::RX).unwrap();
                        mem.map(DATA, 8192, 4096, Permissions::RW).unwrap();
                        mem.map(ALIAS, 8192, 4096, Permissions::RW).unwrap();
                        mem.map(SP, 4096, 12288, Permissions::RW).unwrap();
                        let bytes: Vec<_> = (0..8192).map(|i| ((i * 71 + 37) >> 3) as u8).collect();
                        mem.write_bytes(DATA, &bytes, 1).unwrap();
                        mem.set_u32(SP, runtime::exec::HARNESS_EXIT).unwrap();
                        let watch = mem.subscribe_writes("bulk-rep");
                        mem.watch_writes(watch, CONTIGUOUS_BASE + 4096, 8192).unwrap();
                        *rt.memory() = mem;
                        let mut ctx = ExecCtx::new(rt.clone(), 1);
                        ctx.enable_fast_memory(fast);
                        let mut cpu = CpuAbi { esp: SP, esi: src, edi: dst, ecx: count,
                            eflags: if reverse { 0x602 } else { 0x202 }, ..CpuAbi::default() };
                        let mut pending = TransferAbi::jump(A);
                        let mut stops = Vec::new();
                        let mut charged = 0;
                        let result = loop {
                            ctx.arm_preemption(slice);
                            let end = Runner::default().run_from(&mut cpu, &mut ctx, pending);
                            charged += ctx.take_slice_spent();
                            if let Some(next) = ctx.take_preempted() {
                                stops.push((cpu, charged));
                                pending = next;
                            } else { break end; }
                        };
                        let mut bytes = vec![0;8192];
                        rt.memory().read_bytes(DATA, &mut bytes, 1).unwrap();
                        let dirty = rt.memory().take_dirty(watch).unwrap();
                        results.push((cpu, result.kind, result.target_pc, charged, stops, bytes, dirty));
                        assert!(matches!(result.kind, abi::STOP | abi::FAULT));
                    }
                    assert_eq!(results[0], results[1], "width={width} src={src:x} dst={dst:x} count={count} reverse={reverse} slice={slice}");
                }
            }
        }
    }
}

/// A handler with runs of accesses, loops longer than one hold, a REP, x87
/// between accesses, calls and a page-crossing access.
fn held_gate_functions() -> Vec<IrFunction> {
    let main = function(A, |a| {
        let (mut top, mut sum, mut cross) = (a.create_label(), a.create_label(), a.create_label());
        a.push(ebx)?;
        a.push(ecx)?;
        a.xor(ebx, ebx)?;
        a.set_label(&mut top)?;
        a.mov(eax, dword_ptr(esi))?;
        a.add(eax, dword_ptr(esi + 4))?;
        a.mov(dword_ptr(edi), eax)?;
        a.add(dword_ptr(edi + 4), eax)?;
        a.fld(dword_ptr(esi))?;
        a.fstp(dword_ptr(edi + 8))?;
        a.push(eax)?;
        a.call((A + 0x400) as u64)?;
        a.pop(edx)?;
        a.add(esi, 4)?;
        a.add(edi, 12)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        // More trips than one hold lasts, with no exit in between.
        a.mov(ecx, 100u32)?;
        a.set_label(&mut sum)?;
        a.mov(edx, dword_ptr(ecx * 4 + DATA + 0x100))?;
        a.add(ebx, edx)?;
        a.mov(dword_ptr(ecx * 4 + DATA + 0x900), ebx)?;
        a.dec(ecx)?;
        a.jnz(sum)?;
        // The fifth read crosses a page: the slow helper, mid-hold.
        a.mov(ecx, 7u32)?;
        a.set_label(&mut cross)?;
        a.add(ebx, dword_ptr(ecx * 4 + DATA + 0xfee))?;
        a.mov(dword_ptr(ecx * 4 + DATA + 0xc00), ebx)?;
        a.dec(ecx)?;
        a.jnz(cross)?;
        a.mov(ecx, 70u32)?;
        a.mov(eax, 0x55aa_1234u32)?;
        a.mov(edi, DATA + 0x600)?;
        a.cld()?;
        a.rep().stosd()?;
        a.pop(ecx)?;
        a.pop(ebx)?;
        // Fast stores, then one that may fault with the gate held.
        a.mov(dword_ptr(DATA + 0xd00), eax)?;
        a.mov(dword_ptr(DATA + 0xd04), ecx)?;
        a.mov(dword_ptr(ebp), ebx)?;
        a.mov(dword_ptr(DATA + 0xd08), ebx)?;
        a.ret()
    });
    let leaf = function(A + 0x400, |a| {
        a.add(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(esp + 4), eax)?;
        a.ret()
    });
    vec![main, leaf]
}

/// Holding the gate across accesses changes nothing the guest or the
/// runtime can see: against the same module with the inline path off, every
/// slice ends at the same safe point with the same accesses spent, and the
/// run ends (returning, or faulting on a read-only page with the gate held)
/// with the same registers, flags and memory. The memory guard taken after
/// every stop would hang if a handler left with the gate.
#[test]
fn held_gate_matches_the_helper_path_at_every_stop_and_fault() {
    use runtime::{TransferAbi, abi};
    const RO: u32 = 0x10b000;
    let memory = || {
        let mut mem = Guest::new();
        mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
        mem.map_zeroed(RO, 4096, Permissions::READ).unwrap();
        for i in 0..1024 {
            mem.set_u32(DATA + 4 * i, i.wrapping_mul(0x9e37_79b9) ^ 0x3f80_0000).unwrap();
        }
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        mem
    };
    for accounting in [false, true] {
        let (_lib, dispatch) = compile_module_with(
            "held-gate",
            held_gate_functions(),
            false,
            true,
            emit::opt::Passes::ALL,
            emit::Codegen { accounting },
        );
        let rt = Runtime::new(memory(), dispatch);
        for fault in [false, true] {
            let mut want = None;
            for fast in [false, true] {
                for slice in [1, 2, 3, 7, 33, 64, 1_000_000] {
                    *rt.memory() = memory();
                    let mut ctx = ExecCtx::new(rt.clone(), 1);
                    ctx.enable_fast_memory(fast);
                    ctx.enable_profiling(true);
                    ctx.arm_preemption(slice);
                    let mut cpu = CpuAbi {
                        esp: STACK,
                        esi: DATA + 0x100,
                        edi: DATA + 0x300,
                        ecx: 40,
                        ebp: if fault { RO + 0x10 } else { DATA + 0xd10 },
                        ..CpuAbi::default()
                    };
                    let mut t = TransferAbi::jump(A);
                    let mut stops = Vec::new();
                    let end = loop {
                        let end = Runner::default().run_from(&mut cpu, &mut ctx, t);
                        let Some(next) = ctx.take_preempted() else { break end };
                        // A leaked gate would block here for ever.
                        let stack = rt.memory().u32(cpu.esp).unwrap();
                        stops.push((next.target_pc, ctx.take_slice_spent(), cpu, stack));
                        ctx.arm_preemption(slice);
                        t = next;
                    };
                    let key = format!("accounting {accounting} fault {fault} fast {fast} slice {slice}");
                    assert_eq!(end.kind, if fault { abi::FAULT } else { abi::STOP }, "{key}: {:?}", ctx.events());
                    let spent = stops.iter().map(|s| s.1).sum::<u64>() + ctx.take_slice_spent();
                    let p = ctx.take_profile().unwrap();
                    assert_eq!(p.memory_accessors, spent, "{key}");
                    // Only the page-crossing read and the faulting store miss
                    // (a counting module does not report its inline accesses).
                    let inline = if fast && !accounting { spent - 1 - u64::from(fault) } else { 0 };
                    assert_eq!(p.fast_memory_accesses, inline, "{key}");
                    let mut out = vec![0; 8192];
                    rt.memory().read_bytes(DATA, &mut out, 1).unwrap();
                    // Stops differ with the slice; the end state does not.
                    let got = (spent, cpu, out);
                    let want = want.get_or_insert_with(|| got.clone());
                    assert!(&got == want, "{key}: {spent} accesses, {cpu:x?}");
                    if slice == 1_000_000 {
                        assert!(stops.is_empty(), "{key}");
                    }
                }
            }
        }
        // The same stops, slice by slice, fast or not.
        for slice in [1, 5, 33] {
            let run = |fast: bool| {
                *rt.memory() = memory();
                let mut ctx = ExecCtx::new(rt.clone(), 1);
                ctx.enable_fast_memory(fast);
                ctx.arm_preemption(slice);
                let mut cpu =
                    CpuAbi { esp: STACK, esi: DATA + 0x100, edi: DATA + 0x300, ecx: 40, ebp: DATA + 0xd10, ..CpuAbi::default() };
                let mut t = TransferAbi::jump(A);
                let mut stops = Vec::new();
                loop {
                    Runner::default().run_from(&mut cpu, &mut ctx, t);
                    let Some(next) = ctx.take_preempted() else { break };
                    stops.push((next.target_pc, ctx.take_slice_spent(), cpu));
                    ctx.arm_preemption(slice);
                    t = next;
                }
                stops
            };
            assert!(run(false) == run(true), "accounting {accounting} slice {slice}: stops differ");
        }
    }
}

/// A host thread's memory guards get in while a handler holds the gate, at
/// a loop header and inside a REP, and the handler cannot finish without
/// them. The guest spins (bounded) on a flag only the host sets, and
/// acknowledges it with a store the host must see before it sets the next:
/// so the host's later guards all arrive while the handler is in its loop.
/// The guest then scans (bounded: about 1 GiB of aliased pages) for a word
/// the host writes only once the guest says it has reached the REP. A
/// starved host leaves either loop to run out its count, which fails here;
/// the host gives up after its own timeout.
#[test]
fn held_gate_gives_way_to_a_waiting_memory_guard() {
    use std::time::{Duration, Instant};
    const FLAGS: u32 = DATA + 0x800;
    const BIG: u32 = 0x4000_0000;
    const CHUNK: u32 = 0x20_0000;
    const CHUNKS: u32 = 512;
    let f = function(A, |a| {
        let (mut top, mut scan) = (a.create_label(), a.create_label());
        a.xor(ebx, ebx)?;
        a.set_label(&mut top)?;
        a.mov(eax, dword_ptr(esi))?;
        a.mov(dword_ptr(esi + 4), eax)?;
        a.mov(eax, dword_ptr(esi + 8))?;
        a.test(eax, eax)?;
        a.jnz(scan)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret()?;
        a.set_label(&mut scan)?;
        a.mov(ebx, 1u32)?;
        a.mov(dword_ptr(esi + 12), ebx)?;
        a.mov(eax, 0x5eed_5eedu32)?;
        a.mov(ecx, CHUNKS * (CHUNK / 4))?;
        a.mov(edi, BIG)?;
        a.cld()?;
        a.repne().scasd()?;
        a.ret()
    });
    for accounting in [false, true] {
        let (_lib, dispatch) = compile_module_with(
            "held-gate-host",
            vec![f.clone()],
            false,
            true,
            emit::opt::Passes::ALL,
            emit::Codegen { accounting },
        );
        let mut mem = Guest::new();
        mem.map(A, 4096, 0, Permissions::RX).unwrap();
        mem.map(DATA, 8192, 0x1000, Permissions::RW).unwrap();
        // Every chunk is the same 2 MiB of RAM: a long scan, little memory.
        for i in 0..CHUNKS {
            mem.map(BIG + i * CHUNK, CHUNK, 0x10_0000, Permissions::RW).unwrap();
        }
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let rt = Runtime::new(mem, dispatch);
        let host_rt = rt.clone();
        let host = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            // One guard per step; each waits for the guest's last store.
            let step = |seen: u32, set: u32| {
                let mut guards = 0u32;
                loop {
                    assert!(Instant::now() < deadline, "host starved waiting for {seen:#x}");
                    guards += 1;
                    let mut m = host_rt.memory();
                    if m.u32(seen).unwrap() == 1 {
                        m.set_u32(set, if set == BIG + CHUNK - 4 { 0x5eed_5eed } else { 1 }).unwrap();
                        return guards;
                    }
                    drop(m);
                    std::thread::yield_now();
                }
            };
            host_rt.memory().set_u32(FLAGS, 1).unwrap();
            // Seen the guest's echo, in its loop: let it leave the loop.
            let in_loop = step(FLAGS + 4, FLAGS + 8);
            // Seen it reach the REP: write what the scan looks for.
            let in_rep = step(FLAGS + 12, BIG + CHUNK - 4);
            (in_loop, in_rep)
        });
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        ctx.enable_profiling(true);
        let mut cpu = CpuAbi { esp: STACK, esi: FLAGS, ecx: 0x7fff_ffff, ..CpuAbi::default() };
        let t = Runner::default().run(&mut cpu, &mut ctx, A);
        assert_eq!((t.kind, cpu.eip), (runtime::abi::STOP, runtime::exec::HARNESS_EXIT), "{:?}", ctx.events());
        // The loop ended on the host's flag, and the scan on the host's word.
        assert_eq!(cpu.ebx, 1, "accounting {accounting}: the loop ran out with the host shut out");
        assert_ne!(cpu.ecx, 0, "accounting {accounting}: the REP ran out with the host shut out");
        assert_ne!(cpu.eflags & 0x40, 0, "accounting {accounting}: scan stopped without a match");
        assert_eq!(cpu.edi & (CHUNK - 1), 0, "accounting {accounting}: matched at {:#x}", cpu.edi);
        let (in_loop, in_rep) = host.join().unwrap();
        assert!(in_loop >= 1 && in_rep >= 1);
        // The guest was on the inline path throughout, not the helpers.
        let p = ctx.take_profile().unwrap();
        if !accounting {
            assert!(p.fast_memory_accesses * 10 >= p.memory_accessors * 9, "{p:?}");
        }
    }
}

/// How many of the image's inline accesses take the gate themselves, and
/// how many follow another under the same hold. Static counts over the
/// lifted image (a lower bound on sharing: a hold is taken to end with its
/// block, though it lasts until a context call, an exit or a loop header).
/// `cargo test -p emit --release --test perf image_gate_holds -- --ignored --nocapture`
#[test]
#[ignore = "reads the retail XBE (H2_XBE)"]
fn image_gate_holds() {
    use ir::{Intrinsic, Stmt};
    let Some(path) = common::xbe_path() else {
        common::skip("image_gate_holds", "no XBE (set H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let owners = emit::plan_image(&analysis, &x, emit::opt::Passes::ALL).unwrap();
    // (accesses, gate takes) of a statement list; `held` carries across.
    fn walk(stmts: &[Stmt], held: &mut bool, n: &mut [u64; 2]) {
        for s in stmts {
            match s {
                Stmt::Load { .. } | Stmt::Store { .. } => {
                    n[0] += 1;
                    n[1] += u64::from(!*held);
                    *held = true;
                }
                Stmt::Intrinsic { op: Intrinsic::ReadTsc, .. } | Stmt::Unsupported { .. } => *held = false,
                Stmt::If { then, otherwise, .. } => {
                    let (mut a, mut b) = (*held, *held);
                    walk(then, &mut a, n);
                    walk(otherwise, &mut b, n);
                    *held = a && b;
                }
                // One trip stands for the loop.
                Stmt::Repeat { body, .. } => walk(body, held, n),
                _ => {}
            }
        }
    }
    let (mut n, mut blocks, mut with_access, mut histogram) = ([0u64; 2], 0u64, 0u64, [0u64; 6]);
    for o in &owners {
        for b in o.func.blocks.values() {
            let mut held = false;
            let before = n;
            for i in &b.insns {
                if i.atomic.is_some() {
                    held = false;
                    continue;
                }
                walk(&i.stmts, &mut held, &mut n);
            }
            blocks += 1;
            let accesses = n[0] - before[0];
            if accesses != 0 {
                with_access += 1;
                histogram[(accesses as usize).min(6) - 1] += 1;
            }
        }
    }
    println!("owners {}, blocks {blocks}, with accesses {with_access}", owners.len());
    println!(
        "accesses {}, gate takes {} ({:.1}%), sharing a hold {} ({:.1}%), {:.2} accesses per hold",
        n[0],
        n[1],
        100.0 * n[1] as f64 / n[0] as f64,
        n[0] - n[1],
        100.0 * (n[0] - n[1]) as f64 / n[0] as f64,
        n[0] as f64 / n[1] as f64
    );
    println!("blocks by accesses (1, 2, 3, 4, 5, 6+): {histogram:?}");
    assert!(n[1] <= n[0]);
}
