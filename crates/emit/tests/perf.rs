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
    compile_module(kind, funcs, instrumented, fast)
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
    for kind in ["register", "read", "write", "x87", "call", "tail"] {
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
        assert_eq!(p.runner_safe_points, transfers + 1);
        assert_eq!(p.generated_batches, transfers);
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
