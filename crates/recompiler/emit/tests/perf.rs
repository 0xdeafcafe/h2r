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
            // Many access sites in one handler, as in the image's shards
            // (where clang declines to inline a generic accessor): loads
            // and stores of every width at varied offsets.
            "wide" => {
                for k in 0..16u32 {
                    a.movzx(ebx, byte_ptr(esi + k * 16))?;
                    a.movzx(edx, word_ptr(esi + k * 16 + 2))?;
                    a.add(ebx, edx)?;
                    a.add(ebx, dword_ptr(esi + k * 16 + 4))?;
                    a.mov(byte_ptr(esi + k * 16 + 0x401), bl)?;
                    a.mov(word_ptr(esi + k * 16 + 0x402), bx)?;
                    a.mov(dword_ptr(esi + k * 16 + 0x404), ebx)?;
                }
            }
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
    let codegen = if kind == "frame" || kind == "wide" || std::env::var_os("H2_ACCESS_COUNT").is_some() {
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
    compile_module_defines(kind, funcs, instrumented, fast, passes, codegen, "")
}

/// [`compile_module_with`] with `defines` (C lines) ahead of h2gen.h.
fn compile_module_defines(
    kind: &str,
    funcs: Vec<IrFunction>,
    instrumented: bool,
    fast: bool,
    passes: emit::opt::Passes,
    codegen: emit::Codegen,
    defines: &str,
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
    if !defines.is_empty() {
        let header = src.join("h2gen.h");
        let content = std::fs::read_to_string(&header).unwrap();
        std::fs::write(header, format!("{defines}{content}")).unwrap();
    }
    emit::compile::install_header(&src, &common::abi_header().unwrap()).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap();
    println!(
        "{kind}: clang {:.3}s, {} compiled, {} object bytes",
        b.compile_secs, b.compiled, b.object_bytes
    );
    let dylib = dir.join("perf.dylib");
    let probe = r#"#include "h2gen.h"
H2_EXPORT uint32_t h2probe_read(ExecCtxOpaque *c, uint32_t a, uint32_t n, uint32_t align, uint8_t *out) {
    H2FastAccess *f = h2abi_v1_fast_acquire(c);
    uint32_t status = h2_mem_read(f, c, 0, a, n, align, out);
    h2abi_v1_fast_release(c);
    return status;
}
H2_EXPORT uint32_t h2probe_write(ExecCtxOpaque *c, uint32_t a, uint32_t n, uint32_t align, const uint8_t *in) {
    H2FastAccess *f = h2abi_v1_fast_acquire(c);
    uint32_t status = h2_mem_write(f, c, 0, a, n, align, in);
    h2abi_v1_fast_release(c);
    return status;
}
"#;
    let mut extras = vec![("bridge.c", emit::host::BRIDGE)];
    if kind == "fast-semantics" { extras.push(("probe.c", probe)); }
    if kind == "flat-store-order" {
        extras.push(("flat-probe.c", r#"#include "h2gen.h"
H2_EXPORT uint32_t h2probe_flat(ExecCtxOpaque *ctx, uint32_t addr, uint8_t *value) {
    H2FastAccess *f = h2abi_v1_fast_acquire(ctx);
    if (!f) return 0;
    H2Gate gate = {0};
    h2_gate_adopt(&gate, h2_gate_try(f));
    uint8_t *read = h2_flat_span(&gate, addr, 1u, 1u);
    uint32_t mask = (read != NULL) | ((uint32_t)(h2_flat_span(&gate, addr, 1u, 2u) != NULL) << 1);
    if (read && value) *value = *read;
    h2_gate_drop(f, &gate);
    h2abi_v1_fast_release(ctx);
    return mask;
}
"#));
    }
    emit::compile::host_dylib(&b, &src, &extras, &dylib).unwrap();
    // Initialize x87's helper bridge, then replace the memory/event callbacks
    // with the production runtime. Both bridges implement h2abi.h exactly.
    let _test_host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let mut dispatch = Dispatch::default();
    unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const *const (), usize) -> u32> =
            lib.get(b"h2bridge_install_v3").unwrap();
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
            // As the image's bridge: one exit call, not the pair.
            runtime::ffi::h2abi_v1_exit as *const (),
        ];
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout_v3").unwrap();
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
    for kind in ["register", "read", "write", "frame", "wide", "x87", "call", "tail"] {
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

/// Runner-routed transfers. A loop makes eight indirect calls per trip,
/// each one a CALL the handler hands to the runner, into one of `targets`
/// distinct callees that only `ret`, which is a RETURN the runner resolves
/// to the caller's resume. So every iteration is 16 runner transfers, each
/// resolving its target, checking it is executable and entering a handler
/// whose first access (the `ret`'s stack read) starts a gate hold. With
/// many targets the runner's per-context resolve cache stops covering them,
/// as in the game (thousands of functions and return sites a frame).
///
/// `cargo test -p emit --release --test perf runner_transfer_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "benchmark; run release with --nocapture"]
fn runner_transfer_benchmark() {
    const CALLEES: u32 = 4096;
    const CALLEE_BASE: u32 = A + 0x1000;
    const TABLE: u32 = 0x0020_0000;
    const BENCH_STACK: u32 = 0x0030_0ff0;
    const SITES: u32 = 8;
    let n: u32 = std::env::var("H2_PERF_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let trials: usize = std::env::var("H2_PERF_TRIALS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let mut funcs = vec![function(A, |a| {
        let mut l = a.create_label();
        a.mov(ecx, n)?;
        a.mov(esi, TABLE)?;
        a.set_label(&mut l)?;
        for s in 0..SITES {
            a.lea(eax, dword_ptr(ecx + s * 37))?;
            a.and(eax, edx)?;
            a.call(dword_ptr(esi + eax * 4))?;
        }
        a.dec(ecx)?;
        a.jnz(l)?;
        a.ret()
    })];
    funcs.extend((0..CALLEES).map(|i| function(CALLEE_BASE + i * 16, |a| a.ret())));
    let (_lib, dispatch) = compile_module_with(
        "runner-transfers",
        funcs,
        false,
        std::env::var_os("H2_FASTMEM").is_none_or(|v| v != "0"),
        emit::opt::Passes::from_env().unwrap(),
        emit::Codegen::from_env().unwrap(),
    );
    let mut mem = Guest::new();
    mem.map_zeroed(A, 0x20000, Permissions::RX).unwrap();
    mem.map_zeroed(TABLE, CALLEES * 4, Permissions::RW).unwrap();
    mem.map_zeroed(BENCH_STACK & !0xfff, 0x1000, Permissions::RW).unwrap();
    for i in 0..CALLEES {
        mem.set_u32(TABLE + i * 4, CALLEE_BASE + i * 16).unwrap();
    }
    mem.set_u32(BENCH_STACK, runtime::exec::HARNESS_EXIT).unwrap();
    let mut ctx = ExecCtx::new(Runtime::new(mem, dispatch), 1);
    ctx.enable_direct_calls(false);
    for targets in [16u32, 256, 1024, 4096] {
        let mut times = Vec::new();
        for _ in 0..trials {
            let mut cpu = CpuAbi {
                esp: BENCH_STACK,
                edx: targets - 1,
                ..CpuAbi::default()
            };
            let start = Instant::now();
            let t = Runner {
                max_transfers: 2 * SITES as u64 * n as u64 + 10,
            }
            .run(&mut cpu, &mut ctx, A);
            times.push(start.elapsed().as_secs_f64() * 1e9 / (2 * SITES * n) as f64);
            assert_eq!(t.kind, runtime::abi::STOP, "{t:?}, {:?}", ctx.events());
            assert_eq!(cpu.eip, runtime::exec::HARNESS_EXIT);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "runner transfers, {targets} targets: n={n}x{}, trials={trials}, min={:.2} median={:.2} max={:.2} ns/transfer",
            2 * SITES,
            times[0],
            times[trials / 2],
            times[trials - 1]
        );
    }
}

/// A handler's first access after it is entered, as most of the game's
/// handlers make it: callees with a frame (push, a load and a store of
/// data, pop, ret), eight a loop trip, entered either by the runner (an
/// indirect call, `runner`) or directly by the caller's handler (a static
/// call, `direct`). Every callee starts with its stack store, so each
/// entry pays whatever it takes to start a gate hold (`H2_ENTRY_GATE`), and
/// a counting module settles its count at every exit (`H2_VIEW_CHARGE`).
///
/// `cargo test -p emit --release --test perf entry_access_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "benchmark; run release with --nocapture"]
fn entry_access_benchmark() {
    const CALLEES: u32 = 64;
    const CALLEE_BASE: u32 = A + 0x1000;
    const TABLE: u32 = 0x0020_0000;
    const BENCH_STACK: u32 = 0x0030_0ff0;
    const SITES: u32 = 8;
    let n: u32 = std::env::var("H2_PERF_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let trials: usize = std::env::var("H2_PERF_TRIALS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let callee = |i: u32| {
        function(CALLEE_BASE + i * 32, move |a| {
            a.push(ebp)?;
            a.mov(ebp, esp)?;
            a.mov(eax, dword_ptr(edi + i * 4))?;
            a.add(eax, 1)?;
            a.mov(dword_ptr(edi + i * 4), eax)?;
            a.pop(ebp)?;
            a.ret()
        })
    };
    for how in ["runner", "direct"] {
        if std::env::var("H2_PERF_KIND").is_ok_and(|k| k != how) {
            continue;
        }
        let mut funcs = vec![function(A, |a| {
            let mut l = a.create_label();
            a.mov(ecx, n)?;
            a.mov(esi, TABLE)?;
            a.mov(edi, TABLE + 0x1000)?;
            a.set_label(&mut l)?;
            for s in 0..SITES {
                if how == "runner" {
                    a.lea(eax, dword_ptr(ecx + s * 37))?;
                    a.and(eax, CALLEES - 1)?;
                    a.call(dword_ptr(esi + eax * 4))?;
                } else {
                    a.call((CALLEE_BASE + s * 32) as u64)?;
                }
            }
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        })];
        funcs.extend((0..CALLEES).map(callee));
        let (_lib, dispatch) = compile_module_with(
            &format!("entry-access-{how}"),
            funcs,
            false,
            std::env::var_os("H2_FASTMEM").is_none_or(|v| v != "0"),
            emit::opt::Passes::from_env().unwrap(),
            emit::Codegen::from_env().unwrap(),
        );
        let mut mem = Guest::new();
        mem.map_zeroed(A, 0x20000, Permissions::RX).unwrap();
        mem.guard_code(A, 0x20000).unwrap();
        mem.map_zeroed(TABLE, 0x2000, Permissions::RW).unwrap();
        mem.map_zeroed(BENCH_STACK & !0xfff, 0x1000, Permissions::RW).unwrap();
        for i in 0..CALLEES {
            mem.set_u32(TABLE + i * 4, CALLEE_BASE + i * 32).unwrap();
        }
        mem.set_u32(BENCH_STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let rt = Runtime::new(mem, dispatch);
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        ctx.enable_direct_calls(how == "direct");
        let mut times = Vec::new();
        for _ in 0..trials {
            let mut cpu = CpuAbi { esp: BENCH_STACK, ..CpuAbi::default() };
            let start = Instant::now();
            let t = Runner { max_transfers: 2 * SITES as u64 * n as u64 + 10 }.run(&mut cpu, &mut ctx, A);
            times.push(start.elapsed().as_secs_f64() * 1e9 / (SITES * n) as f64);
            assert_eq!(t.kind, runtime::abi::STOP, "{t:?}, {:?}", ctx.events());
            assert_eq!(cpu.eip, runtime::exec::HARNESS_EXIT);
        }
        // Every call ran its callee once: the counters add up.
        let total: u64 = (0..CALLEES).map(|i| rt.memory().u32(TABLE + 0x1000 + i * 4).unwrap() as u64).sum();
        assert_eq!(total, (SITES * n) as u64 * trials as u64);
        times.sort_by(f64::total_cmp);
        println!(
            "entry access, {how}: n={n}x{SITES}, trials={trials}, min={:.2} median={:.2} max={:.2} ns/call",
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
        mem.guard_code(A, 4096).unwrap();
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
        // With direct calls (on unless H2_DIRECT_CALLS=0) every trip calls
        // B directly from the first (the entry table needs no warming), B
        // tail-jumps to C directly, and the return to A continues inline.
        // Transfers count the same; those made directly pass no runner
        // safe point, and the caller's handler does not exit in between.
        let direct = std::env::var("H2_DIRECT_CALLS").as_deref() != Ok("0");
        let (direct_transfers, inline_returns) = match kind {
            "call" if direct => (22, 11),
            "tail" if direct => (33, 11),
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
        let built = compile_module_with("fast-accounting", vec![f()], false, true, passes, emit::Codegen::accesses(accounting));
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
            emit::write_module_with(&owners, &dir, emit::Codegen::accesses(accounting)).unwrap();
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
            emit::opt::Passes::ALL, emit::Codegen::accesses(true),
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
    // The switches that change how a direct call settles the gate and the
    // count: none, the lent gate, the lent count, and both.
    // Then the handler's own entry and exit: the gate taken at entry, the
    // count spent in the view, both, and everything at once.
    const GATE: &str = "#define H2_DIRECT_GATE 1\n";
    const COUNT: &str = "#define H2_DIRECT_COUNT 1\n";
    const ENTRY: &str = "#define H2_ENTRY_GATE 1\n";
    const CHARGE: &str = "#define H2_VIEW_CHARGE 1\n";
    let switches = [
        ("", String::new()),
        ("-lent", GATE.into()),
        ("-count", COUNT.into()),
        ("-both", format!("{GATE}{COUNT}")),
        ("-entry", ENTRY.into()),
        ("-charge", CHARGE.into()),
        ("-entry-charge", format!("{ENTRY}{CHARGE}")),
        ("-all", format!("{GATE}{COUNT}{ENTRY}{CHARGE}")),
    ];
    for (accounting, (tag, defines)) in [false, true].into_iter().flat_map(|a| switches.clone().map(|s| (a, s))) {
        let gate = tag;
        // A counting handler with H2_VIEW_CHARGE spends its whole count in
        // the view, so the runtime settles all of it from there.
        let view_charge = accounting && defines.contains(CHARGE);
        let defines = format!("#undef H2_VIEW_CHARGE\n{defines}");
        let (_lib, dispatch) = compile_module_defines(
            &format!("held-gate{tag}"),
            held_gate_functions(),
            false,
            true,
            emit::opt::Passes::ALL,
            emit::Codegen::accesses(accounting),
            &defines,
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
                    let key = format!("accounting {accounting} gate {gate} fault {fault} fast {fast} slice {slice}");
                    assert_eq!(end.kind, if fault { abi::FAULT } else { abi::STOP }, "{key}: {:?}", ctx.events());
                    let spent = stops.iter().map(|s| s.1).sum::<u64>() + ctx.take_slice_spent();
                    let p = ctx.take_profile().unwrap();
                    assert_eq!(p.memory_accessors, spent, "{key}");
                    // Only the page-crossing read and the faulting store miss
                    // (a counting module does not report its inline accesses,
                    // unless it spends its count in the view).
                    let inline = match (fast, accounting) {
                        (true, false) => spent - 1 - u64::from(fault),
                        (true, true) if view_charge => spent,
                        _ => 0,
                    };
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
            assert!(run(false) == run(true), "accounting {accounting} gate {gate} slice {slice}: stops differ");
        }
    }
}

/// The causes `H2_COLD_STATS` gives, in `h2gen_cold_counts` order.
const COLD_CAUSES: [&str; 8] = ["entry", "lent-entry", "after-dcall", "flush", "drop", "tick", "miss", "no-gate"];

/// Runs the mixed-memory main and leaf once, without contention or preemption.
fn run_held_gate_fixture(accounting: bool, kind: &str, defines: &str) -> libloading::Library {
    let (lib, dispatch) = compile_module_defines(
        kind,
        held_gate_functions(),
        false,
        true,
        emit::opt::Passes::ALL,
        emit::Codegen::accesses(accounting),
        defines,
    );
    let mut mem = Guest::new();
    mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
    // Lifted code, as the loader guards an XBE's: only then does the entry
    // table hold the leaf, so main's calls of it are direct (runtime
    // direct.rs).
    mem.guard_code(A, 4096).unwrap();
    mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
    mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
    let rt = Runtime::new(mem, dispatch);
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.enable_fast_memory(true);
    let mut cpu = CpuAbi { esp: STACK, esi: DATA + 0x100, edi: DATA + 0x300, ecx: 40, ebp: DATA + 0xd10, ..CpuAbi::default() };
    let t = Runner::default().run(&mut cpu, &mut ctx, A);
    assert_eq!(t.kind, runtime::abi::STOP, "{:?}", ctx.events());
    lib
}

fn cold_counts(accounting: bool, kind: &str, defines: &str) -> [u64; 8] {
    let lib = run_held_gate_fixture(accounting, kind, &format!("#define H2_COLD_STATS 1\n{defines}"));
    let counts: libloading::Symbol<*const [u64; 8]> = unsafe { lib.get(b"h2gen_cold_counts").unwrap() };
    unsafe { **counts }
}

#[test]
fn flat_owner_coverage() {
    let lib = run_held_gate_fixture(false, "flat-owner-coverage",
        "#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 1\n#undef H2_FLAT_STATS_OWNER\n#define H2_FLAT_STATS_OWNER 0x400000u\n");
    let counts: libloading::Symbol<*const [u64; 3]> = unsafe { lib.get(b"h2gen_flat_counts").unwrap() };
    let [total, flat, bulk] = unsafe { **counts };
    // Main only: 2 pushes, 40 ten-access calls/trips (excluding the leaf),
    // 100 and 7 two-access trips, 70 STOSD, 2 pops, 4 stores and RET.
    assert_eq!((total, bulk), (693, 0));
    let enabled = std::env::var("H2_FLAT_RAM").as_deref() == Ok("1")
        && cfg!(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")));
    // DATA is at VA mod 16 KiB = 0 and PA mod 16 KiB = 4096.
    // The one read straddling its two guest pages still takes the helper.
    assert_eq!(flat, if enabled { total - 1 } else { 0 });
}

/// Why accesses take the cold path, by cause, over a run of
/// [`held_gate_functions`]: 40 trips of a loop that calls a leaf directly
/// (all 40: the entry table needs no warming), a 100-trip loop with no
/// call, a page-crossing loop, a REP and stores. A handler's first access
/// is cold (it starts without the gate), and so is the first after a direct
/// call returns (the caller drops the gate to charge its accesses and
/// call); `miss` counts the cold ones taken with the gate held (a
/// cross-page read), `no-gate` those that leave without it. `--nocapture`
/// prints the table.
#[test]
fn cold_accesses_by_cause() {
    // Isolate entry-gate and direct-gate savings from the flat emitter's
    // span preflight, which can acquire the gate before the first access.
    const PAGED: &str = "#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 0\n";
    const FLAT: &str = "#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 1\n";
    const OFF: &str = "#undef H2_ENTRY_GATE\n#define H2_ENTRY_GATE 0\n#undef H2_VIEW_CHARGE\n#define H2_VIEW_CHARGE 0\n";
    for accounting in [false, true] {
        let c = cold_counts(accounting, &format!("cold-stats-{accounting}"), &format!("{PAGED}{OFF}"));
        let table: Vec<_> = COLD_CAUSES.iter().zip(c).map(|(k, n)| format!("{k} {n}")).collect();
        println!("accounting {accounting}: {}", table.join(", "));
        // main, from the runner; all 40 calls are direct, each leaf from
        // cold, then main's first access after it.
        assert_eq!(c[0], 1, "{table:?}");
        assert_eq!(c[1], 40, "{table:?}");
        assert_eq!(c[2], 40, "{table:?}");
        assert!(c[6] >= 1, "the cross-page read: {table:?}");
        // The leaf opens with a read and write of [esp+4]. Flat code
        // keeps the gate for that span only when the backend is enabled.
        // Otherwise the flat-capable image follows the paged entry policy.
        let flat = cold_counts(accounting, &format!("cold-stats-flat-{accounting}"), &format!("{FLAT}{OFF}"));
        let flat_runtime = cfg!(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))
            && std::env::var("H2_FLAT_RAM").as_deref() == Ok("1");
        let expected = if flat_runtime { [1, 0, 40] } else { [1, 40, 40] };
        assert_eq!(flat[..3], expected, "{flat:?}");
        // H2_DIRECT_GATE lends the hold across the calls: the leaves and
        // main after them start inline, but for the calls a tick ended the
        // hold before (one in 32 trips, calls and loop headers alike).
        let g = cold_counts(accounting, &format!("cold-stats-gate-{accounting}"), &format!("{PAGED}{OFF}#define H2_DIRECT_GATE 1\n"));
        let gt: Vec<_> = COLD_CAUSES.iter().zip(g).map(|(k, n)| format!("{k} {n}")).collect();
        println!("accounting {accounting} H2_DIRECT_GATE: {}", gt.join(", "));
        assert_eq!(g[0], c[0], "{gt:?}");
        assert!(g[1] + g[2] <= 2, "{gt:?}");
        assert!(g.iter().sum::<u64>() + 60 <= c.iter().sum::<u64>(), "{gt:?} against {table:?}");
        // H2_ENTRY_GATE: every handler takes the gate as it starts, whether
        // the runner or a direct caller invoked it, and main takes it again
        // as each call returns inline. No access is cold for those causes;
        // the cross-page read still is.
        for (extra, name) in [("", "H2_ENTRY_GATE"), ("#define H2_VIEW_CHARGE 1\n", "H2_ENTRY_GATE+H2_VIEW_CHARGE")] {
            let e = cold_counts(
                accounting,
                &format!("cold-stats-entry-{}-{accounting}", extra.len()),
                &format!("{PAGED}#undef H2_ENTRY_GATE\n#define H2_ENTRY_GATE 1\n#undef H2_VIEW_CHARGE\n{extra}"),
            );
            let et: Vec<_> = COLD_CAUSES.iter().zip(e).map(|(k, n)| format!("{k} {n}")).collect();
            println!("accounting {accounting} {name}: {}", et.join(", "));
            assert_eq!(e[..3], [0, 0, 0], "{et:?}");
            assert!(e[6] >= 1, "the cross-page read: {et:?}");
            assert!(e.iter().sum::<u64>() + 80 <= c.iter().sum::<u64>(), "{et:?} against {table:?}");
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
            emit::Codegen::accesses(accounting),
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

/// The image's transfer sites by what the emitter makes of them: a call or
/// tail jump it invokes directly (`H2_DCALL`/`H2_DJUMP`), one through a
/// kernel import or a pointer (`H2_ICALL`/`H2_IJUMP`: direct when the entry
/// table or the host-call thunk takes it), or one it leaves to the runner,
/// and why: to an address that is not a function entry, or (a call) with a
/// continuation another handler owns. Every return leaves for the runner
/// unless its handler was invoked directly. Static counts: which runner
/// entries could become direct or stay in C.
/// `cargo test -p emit --release --test perf image_transfer_sites -- --ignored --nocapture`
#[test]
#[ignore = "reads the retail XBE (H2_XBE)"]
fn image_transfer_sites() {
    use ir::{CallKind, Control, Target};
    use std::collections::{BTreeMap, BTreeSet};
    let Some(path) = common::xbe_path() else {
        common::skip("image_transfer_sites", "no XBE (set H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let owners = emit::plan_image(&analysis, &x, emit::opt::Passes::ALL).unwrap();
    let functions: BTreeSet<u32> = owners.iter().map(|o| o.func.entry).collect();
    let mut n: BTreeMap<&str, u64> = BTreeMap::new();
    // Indirect calls by how many distinct sites share a continuation owner.
    for o in &owners {
        for b in o.func.blocks.values() {
            let Some(last) = b.insns.last() else { continue };
            let what = match &last.control {
                Control::Call { target: Target::Direct(t), continuation, .. } => {
                    if !functions.contains(t) {
                        "call: static, not a function entry"
                    } else if !o.selectors.contains(continuation) {
                        "call: static, continuation owned elsewhere"
                    } else {
                        "call: direct (H2_DCALL)"
                    }
                }
                Control::Call { kind: CallKind::Kernel { .. }, continuation, .. } => {
                    if o.selectors.contains(continuation) {
                        "call: kernel import (H2_ICALL)"
                    } else {
                        "call: kernel import, continuation owned elsewhere"
                    }
                }
                Control::Call { .. } => "call: indirect",
                Control::Jump { target: Target::Direct(t), tail: true, .. } => {
                    if functions.contains(t) { "jump: direct tail (H2_DJUMP)" } else { "jump: static tail, not an entry" }
                }
                Control::Jump { tail: false, .. } => continue,
                Control::Jump { kind: CallKind::Kernel { .. }, .. } => "jump: kernel import",
                Control::Jump { .. } => "jump: indirect",
                Control::Return { .. } => "return",
                _ => continue,
            };
            *n.entry(what).or_default() += 1;
        }
    }
    println!("owners {}, function entries {}", owners.len(), functions.len());
    for (k, v) in &n {
        println!("{v:>8} {k}");
    }
    assert!(!n.is_empty());
}

/// Every width the emitter loads and stores (1, 2, 4, 8, 10 and 16 bytes),
/// at every alignment near both ends of a page, across a page boundary
/// into RAM and into nothing, from and to unmapped, read-only and MMIO
/// addresses, and onto a write-watched page twice (the second store hits
/// the hold's last-dirtied page): the inline path and the helper path leave
/// the same transfer, registers, memory, dirty pages, events and access
/// count, accounting or not, with a slice that stops at every access or
/// never. Each run first reads the stack, so the probe runs with the gate
/// already held.
#[test]
fn inline_accesses_match_the_helper_path_at_every_width_and_edge() {
    use runtime::{TransferAbi, abi, watch::CONTIGUOUS_BASE};
    const SRC: u32 = 0x110000;
    const DST: u32 = 0x120000;
    const DST_PHYS: u32 = 0x8000;
    const RO: u32 = 0x130000;
    const UNMAPPED: u32 = 0x140000;
    const MMIO: u32 = 0xfd00_0000;
    let probe = |width: u32| {
        function(A, |a| {
            a.mov(ebx, dword_ptr(esp))?;
            match width {
                1 => {
                    a.mov(al, byte_ptr(esi))?;
                    a.mov(byte_ptr(edi), al)?;
                    a.mov(byte_ptr(edi + 32), al)?;
                    a.mov(cl, byte_ptr(edi))?;
                }
                2 => {
                    a.mov(ax, word_ptr(esi))?;
                    a.mov(word_ptr(edi), ax)?;
                    a.mov(word_ptr(edi + 32), ax)?;
                    a.mov(cx, word_ptr(edi))?;
                }
                4 => {
                    a.mov(eax, dword_ptr(esi))?;
                    a.mov(dword_ptr(edi), eax)?;
                    a.mov(dword_ptr(edi + 32), eax)?;
                    a.mov(ecx, dword_ptr(edi))?;
                }
                8 => {
                    a.movq(xmm0, qword_ptr(esi))?;
                    a.movq(qword_ptr(edi), xmm0)?;
                    a.movq(qword_ptr(edi + 32), xmm0)?;
                    a.movq(xmm1, qword_ptr(edi))?;
                }
                10 => {
                    a.fld(tword_ptr(esi))?;
                    a.fld(st0)?;
                    a.fstp(tword_ptr(edi))?;
                    a.fstp(tword_ptr(edi + 32))?;
                    a.fld(tword_ptr(edi))?;
                }
                _ => {
                    a.movups(xmm0, xmmword_ptr(esi))?;
                    a.movups(xmmword_ptr(edi), xmm0)?;
                    a.movups(xmmword_ptr(edi + 32), xmm0)?;
                    a.movups(xmm1, xmmword_ptr(edi))?;
                }
            }
            a.ret()
        })
    };
    let memory = || {
        let mut mem = Guest::new();
        mem.map(A, 4096, 0, Permissions::RX).unwrap();
        mem.map(SRC, 8192, 0x2000, Permissions::RW).unwrap();
        mem.map(DST, 8192, DST_PHYS, Permissions::RW).unwrap();
        mem.map(RO, 4096, 0xa000, Permissions::READ).unwrap();
        mem.map(STACK & !4095, 4096, 0xb000, Permissions::RW).unwrap();
        let bytes: Vec<u8> = (0..8192u32).map(|i| (i.wrapping_mul(0x9e37_79b9) >> 13) as u8).collect();
        mem.write_bytes(SRC, &bytes, 1).unwrap();
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let watch = mem.subscribe_writes("inline-edges");
        mem.watch_writes(watch, CONTIGUOUS_BASE + DST_PHYS, 8192).unwrap();
        (mem, watch)
    };
    let edges = |base: u32, width: u32| {
        let mut v: Vec<u32> = [0, 1, 2, 3, 5, 7, 8, 15]
            .into_iter()
            .chain([4096 - width - 1, 4096 - width, 4096 - width + 1, 4095])
            .map(|o| base + o)
            .collect();
        // The span crosses from RAM into nothing: the whole access fails.
        v.push(base + 8192 - width + 1);
        v.dedup();
        v
    };
    for accounting in [false, true] {
        for width in [1u32, 2, 4, 8, 10, 16] {
            let (_lib, dispatch) = compile_module_with(
                &format!("inline-edges-{width}"),
                vec![probe(width)],
                false,
                true,
                emit::opt::Passes::ALL,
                emit::Codegen::accesses(accounting),
            );
            let rt = Runtime::new(memory().0, dispatch);
            let good_src = SRC + 0x100;
            let good_dst = DST + 0x200;
            let cases = edges(SRC, width)
                .into_iter()
                .chain([UNMAPPED, MMIO, RO + 8])
                .map(|s| (s, good_dst))
                .chain(edges(DST, width).into_iter().chain([UNMAPPED, MMIO, RO + 8]).map(|d| (good_src, d)));
            for (src, dst) in cases {
                for slice in [1, 1_000_000] {
                    let mut results = Vec::new();
                    let mut inline = 0;
                    for fast in [false, true] {
                        let (mem, watch) = memory();
                        *rt.memory() = mem;
                        let mut ctx = ExecCtx::new(rt.clone(), 1);
                        ctx.enable_fast_memory(fast);
                        ctx.enable_profiling(true);
                        let mut cpu = CpuAbi { esp: STACK, esi: src, edi: dst, ..CpuAbi::default() };
                        let mut t = TransferAbi::jump(A);
                        let mut stops = Vec::new();
                        let end = loop {
                            ctx.arm_preemption(slice);
                            let end = Runner::default().run_from(&mut cpu, &mut ctx, t);
                            let spent = ctx.take_slice_spent();
                            let Some(next) = ctx.take_preempted() else { break (end, spent) };
                            stops.push((next.target_pc, spent, cpu));
                            t = next;
                        };
                        let profile = ctx.take_profile().unwrap();
                        if fast && !accounting {
                            inline = profile.fast_memory_accesses;
                        }
                        let mut bytes = vec![0; 3 * 8192];
                        rt.memory().read_bytes(SRC, &mut bytes[..8192], 1).unwrap();
                        rt.memory().read_bytes(DST, &mut bytes[8192..2 * 8192], 1).unwrap();
                        rt.memory().read_bytes(RO, &mut bytes[2 * 8192..2 * 8192 + 4096], 1).unwrap();
                        let dirty = rt.memory().take_dirty(watch).unwrap();
                        let events = format!("{:?}", ctx.events());
                        results.push((end.0.kind, end.0.target_pc, end.1, profile.memory_accessors, cpu, stops, bytes, dirty, events));
                    }
                    let key = format!("accounting={accounting} width={width} src={src:#x} dst={dst:#x} slice={slice}");
                    assert!(matches!(results[0].0, abi::STOP | abi::FAULT), "{key}");
                    assert!(results[0] == results[1], "{key}: {:?}\n{:?}", results[0].0, results[1].0);
                    // The fast run took the inline path where it could: the
                    // stack read and, for a good pair, all four accesses.
                    if !accounting && slice == 1_000_000 {
                        let clean = src == good_src && dst == good_dst;
                        assert!(inline > 4 * u64::from(clean), "{key}: {inline} inline");
                    }
                }
            }
        }
    }
}

/// The inline path skips re-marking the page a hold marked last, which is
/// sound only because Rust drains dirty bits under the gate: a new hold
/// must forget it. Here a host thread drains the watch while the guest is
/// looping and storing to a watched page; the guest then retakes the gate
/// on a read and stores to the same page inline. That store must reach the
/// host's next drain.
#[test]
fn a_new_hold_marks_the_page_the_last_hold_dirtied() {
    use runtime::watch::CONTIGUOUS_BASE;
    use std::time::{Duration, Instant};
    const FLAGS: u32 = DATA;
    const PAGE: u32 = 0x120000;
    const PAGE_PHYS: u32 = 0x8000;
    const MAGIC: u32 = 0x5eed_0001;
    let f = function(A, |a| {
        let (mut top, mut done) = (a.create_label(), a.create_label());
        a.mov(ebx, MAGIC)?;
        a.set_label(&mut top)?;
        // Two reads first: after the host's guard, whichever retakes the
        // gate, the store below runs inline in the new hold.
        a.mov(eax, dword_ptr(esi))?;
        a.mov(edx, dword_ptr(esi + 8))?;
        a.mov(dword_ptr(edi), ebx)?;
        a.test(eax, eax)?;
        a.jnz(done)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret()?;
        a.set_label(&mut done)?;
        a.mov(dword_ptr(edi + 4), ebx)?;
        a.mov(dword_ptr(esi + 4), 1u32)?;
        a.ret()
    });
    for accounting in [false, true] {
        let (_lib, dispatch) = compile_module_with(
            "new-hold-dirty",
            vec![f.clone()],
            false,
            true,
            emit::opt::Passes::ALL,
            emit::Codegen::accesses(accounting),
        );
        let mut mem = Guest::new();
        mem.map(A, 4096, 0, Permissions::RX).unwrap();
        mem.map(DATA, 8192, 0x2000, Permissions::RW).unwrap();
        mem.map(PAGE, 4096, PAGE_PHYS, Permissions::RW).unwrap();
        mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
        let watch = mem.subscribe_writes("new-hold");
        mem.watch_writes(watch, CONTIGUOUS_BASE + PAGE_PHYS, 4096).unwrap();
        let rt = Runtime::new(mem, dispatch);
        let host_rt = rt.clone();
        let host = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let wait = |addr: u32, want: u32| loop {
                assert!(Instant::now() < deadline, "host timed out waiting for {addr:#x}");
                if host_rt.memory().u32(addr).unwrap() == want {
                    return;
                }
                std::thread::yield_now();
            };
            // The guest is in its loop, storing to the page inline.
            wait(PAGE, MAGIC);
            {
                let mut m = host_rt.memory();
                m.take_dirty(watch).unwrap();
                m.set_u32(FLAGS, 1).unwrap();
            }
            wait(FLAGS + 4, 1);
            host_rt.memory().take_dirty(watch).unwrap()
        });
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = CpuAbi { esp: STACK, esi: FLAGS, edi: PAGE, ecx: 0x7fff_ffff, ..CpuAbi::default() };
        let t = Runner::default().run(&mut cpu, &mut ctx, A);
        assert_eq!((t.kind, cpu.eip), (runtime::abi::STOP, runtime::exec::HARNESS_EXIT), "{:?}", ctx.events());
        let dirty = host.join().unwrap();
        assert!(!dirty.is_empty(), "accounting {accounting}: the stores after the drain were not marked");
    }
}

/// Stores that alternate between pages (so a hold's last-marked page never
/// matches), through an alias, and into different words of the dirty
/// bitmap, mark exactly the physical pages they reach, run after run: each
/// drain clears the bits, and the next run must set them again rather than
/// find them marked. With and without `H2_DIRTY_INLINE` (the store site
/// tests the bit itself) and access accounting.
#[test]
fn stores_mark_exactly_their_pages_with_and_without_dirty_inline() {
    use runtime::watch::{CONTIGUOUS_BASE, DirtyRange};
    const V: u32 = 0x120000;
    // V, V+1, V+2 and the alias V+3 (of V's page); V+4 is only read.
    const PHYS: [u32; 5] = [0x10000, 0x40000, 0x100_0000, 0x10000, 0x14000];
    let f = function(A, |a| {
        let mut top = a.create_label();
        a.mov(eax, 0x5eed_0001u32)?;
        a.set_label(&mut top)?;
        a.mov(dword_ptr(edi), eax)?;
        a.mov(dword_ptr(edi + 0x1000), eax)?;
        a.mov(dword_ptr(edi + 8), eax)?;
        a.mov(dword_ptr(edi + 0x2000), eax)?;
        a.mov(dword_ptr(edi + 0x3010), eax)?;
        a.add(eax, dword_ptr(edi + 0x4000))?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret()
    });
    let want: Vec<DirtyRange> =
        [0x10000, 0x40000, 0x100_0000].map(|p| DirtyRange { addr: CONTIGUOUS_BASE | p, len: 0x1000 }).to_vec();
    for (tag, defines) in [("", "#undef H2_DIRTY_INLINE\n#define H2_DIRTY_INLINE 0\n"), ("-inline", "#undef H2_DIRTY_INLINE\n#define H2_DIRTY_INLINE 1\n")] {
        for accounting in [false, true] {
            let (_lib, dispatch) = compile_module_defines(
                &format!("dirty-pages{tag}"),
                vec![f.clone()],
                false,
                true,
                emit::opt::Passes::ALL,
                emit::Codegen::accesses(accounting),
                defines,
            );
            let mut mem = Guest::new();
            mem.map(A, 4096, 0x2000, Permissions::RX).unwrap();
            mem.map(STACK & !0xfff, 4096, 0x3000, Permissions::RW).unwrap();
            for (i, p) in PHYS.into_iter().enumerate() {
                mem.map(V + i as u32 * 0x1000, 4096, p, Permissions::RW).unwrap();
            }
            mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
            let watch = mem.subscribe_writes("dirty-pages");
            for p in [0x10000, 0x14000, 0x40000, 0x100_0000] {
                mem.watch_writes(watch, CONTIGUOUS_BASE + p, 4096).unwrap();
            }
            let rt = Runtime::new(mem, dispatch);
            let mut ctx = ExecCtx::new(rt.clone(), 1);
            for run in 0..3 {
                // More trips than a hold lasts (H2_GATE_TRIPS): several holds.
                let mut cpu = CpuAbi { esp: STACK, edi: V, ecx: 100, ..CpuAbi::default() };
                let t = Runner::default().run(&mut cpu, &mut ctx, A);
                assert_eq!((t.kind, cpu.eip), (runtime::abi::STOP, runtime::exec::HARNESS_EXIT), "{:?}", ctx.events());
                let got = rt.memory().take_dirty(watch).unwrap();
                assert_eq!(got, want, "dirty inline {tag:?}, accounting {accounting}, run {run}");
            }
            assert_eq!(rt.memory().u32(V + 0x10).unwrap(), 0x5eed_0001);
        }
    }
}

/// What a store to a page other than the hold's last-marked one costs,
/// with a write watch active (`watching`): the loop stores to two pages in
/// turn, both already marked after the first trip, as hot pages stay
/// between drains. `H2_DIRTY_INLINE=1` tests the bit at the site; without
/// it the site calls `h2_mark_dirty`, which finds the bit set and returns.
/// `nowatch` is the same loop with no watch, for reference.
///
/// `cargo test -p emit --release --test perf dirty_store_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "benchmark; run release with --nocapture"]
fn dirty_store_benchmark() {
    use runtime::watch::CONTIGUOUS_BASE;
    const V: u32 = 0x120000;
    let n: u32 = std::env::var("H2_PERF_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(5_000_000);
    let trials: usize = std::env::var("H2_PERF_TRIALS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let f = function(A, |a| {
        let mut top = a.create_label();
        a.mov(ecx, n)?;
        a.set_label(&mut top)?;
        a.mov(dword_ptr(edi), ecx)?;
        a.mov(dword_ptr(edi + 0x1000), ecx)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret()
    });
    let (_lib, dispatch) = compile_module_with(
        "dirty-store",
        vec![f],
        false,
        true,
        emit::opt::Passes::from_env().unwrap(),
        emit::Codegen::from_env().unwrap(),
    );
    let mut mem = Guest::new();
    mem.map(A, 4096, 0x2000, Permissions::RX).unwrap();
    mem.map(STACK & !0xfff, 4096, 0x3000, Permissions::RW).unwrap();
    mem.map(V, 0x2000, 0x10000, Permissions::RW).unwrap();
    mem.set_u32(STACK, runtime::exec::HARNESS_EXIT).unwrap();
    let rt = Runtime::new(mem, dispatch);
    // No watch first: a subscription, once made, stays for the rest.
    for watched in [false, true] {
        let watch = watched.then(|| {
            let mut m = rt.memory();
            let w = m.subscribe_writes("dirty-store");
            m.watch_writes(w, CONTIGUOUS_BASE + 0x10000, 0x2000).unwrap();
            w
        });
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut times = Vec::new();
        for _ in 0..trials {
            let mut cpu = CpuAbi { esp: STACK, edi: V, ..CpuAbi::default() };
            let start = Instant::now();
            let t = Runner::default().run(&mut cpu, &mut ctx, A);
            times.push(start.elapsed().as_secs_f64() * 1e9 / n as f64);
            assert_eq!((t.kind, cpu.eip), (runtime::abi::STOP, runtime::exec::HARNESS_EXIT), "{:?}", ctx.events());
        }
        if let Some(w) = watch {
            let mut m = rt.memory();
            assert!(!m.take_dirty(w).unwrap().is_empty());
        }
        times.sort_by(f64::total_cmp);
        println!(
            "dirty store, {}: n={n}, trials={trials}, min={:.3} median={:.3} max={:.3} ns/iteration (two stores)",
            if watched { "watch" } else { "nowatch" },
            times[0],
            times[trials / 2],
            times[trials - 1]
        );
    }
}

/// What `H2_DIRTY_INLINE=1` costs in code: every 16th owner of the lifted
/// image, emitted and compiled at -O2 with the inline RAM path, with and
/// without the switch, and the `__text` bytes of both (only objects under
/// Cargo's scratch directory; nothing is linked).
/// `cargo test -p emit --release --test perf image_dirty_inline_code_size -- --ignored --nocapture`
#[test]
#[ignore = "reads the retail XBE (H2_XBE)"]
fn image_dirty_inline_code_size() {
    image_switch_code_size("H2_DIRTY_INLINE", emit::Codegen::default());
}

/// As [`image_dirty_inline_code_size`], for `H2_DIRECT_GATE=1` in a module
/// that counts its accesses (as the game's build does).
/// `cargo test -p emit --release --test perf image_direct_gate_code_size -- --ignored --nocapture`
#[test]
#[ignore = "reads the retail XBE (H2_XBE)"]
fn image_direct_gate_code_size() {
    image_switch_code_size("H2_DIRECT_GATE", emit::Codegen::accesses(true));
}

/// As [`image_direct_gate_code_size`], for `H2_DIRECT_COUNT=1`.
/// `cargo test -p emit --release --test perf image_direct_count_code_size -- --ignored --nocapture`
#[test]
#[ignore = "reads the retail XBE (H2_XBE)"]
fn image_direct_count_code_size() {
    image_switch_code_size("H2_DIRECT_COUNT", emit::Codegen::accesses(true));
}

/// `switch`: the names to set, joined by `+`.
fn image_switch_code_size(switch: &str, codegen: emit::Codegen) {
    let name = format!("{switch} code size");
    let Some(path) = common::xbe_path() else {
        common::skip(&name, "no XBE (set H2_XBE)");
        return;
    };
    let Some(header) = common::abi_header() else {
        common::skip(&name, "no h2abi.h");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let owners = emit::plan_image(&analysis, &x, emit::opt::Passes::ALL).unwrap();
    let sample: Vec<_> = owners.into_iter().step_by(16).collect();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{switch}-size"));
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&dir);
    emit::write_module_with(&sample, &src, codegen).unwrap();
    emit::compile::install_header(&src, &header).unwrap();
    let cc = emit::compile::compiler().unwrap();
    let mut sources: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    sources.sort();
    // The __text section's size of a Mach-O object, from `size -m`.
    let text = |obj: &std::path::Path| -> u64 {
        let out = std::process::Command::new("size").arg("-m").arg(obj).output().unwrap();
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .find_map(|l| l.trim().strip_prefix("Section (__TEXT, __text): "))
            .map_or(0, |n| n.trim().parse().unwrap())
    };
    let mut sizes = [0u64; 2];
    for (i, inline) in [false, true].into_iter().enumerate() {
        let objs = dir.join(format!("obj-{inline}"));
        std::fs::create_dir_all(&objs).unwrap();
        for s in &sources {
            let obj = objs.join(s.file_stem().unwrap()).with_extension("o");
            let mut c = std::process::Command::new(&cc);
            c.args(emit::compile::CFLAGS);
            c.args(["-O2", "-DH2_FASTMEM=1", "-DH2_X87_NATIVE=1", "-DH2_SSE_NATIVE=1", "-DH2_DIRECT_CALLS=1"]);
            if inline {
                c.args(switch.split('+').map(|s| format!("-D{s}=1")));
            }
            let status = c.arg("-c").arg("-I").arg(&src).arg(s).arg("-o").arg(&obj).status().unwrap();
            assert!(status.success(), "{}", s.display());
            sizes[i] += text(&obj);
        }
    }
    println!(
        "{} of the image's owners in {} files: __text {} bytes, {} with {switch} ({:+.2}%)",
        sample.len(),
        sources.len(),
        sizes[0],
        sizes[1],
        100.0 * (sizes[1] as f64 - sizes[0] as f64) / sizes[0] as f64
    );
    assert!(sizes[0] > 0);
}

/// Two adjacent stores can share a span even if a later instruction uses
/// another virtual alias. A rejected span must preserve the first store and
/// fault at the second instruction, including its exact guest PC and charge.
#[test]
fn flat_span_matches_checked_stores_aliases_and_fault_order() {
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    if std::env::var("H2_FLAT_RAM").as_deref() != Ok("1") {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "flat_span_matches_checked_stores_aliases_and_fault_order", "--nocapture"])
            .env("H2_FLAT_RAM", "1").status().unwrap();
        assert!(status.success());
        return;
    }
    use runtime::{Event, abi, watch::CONTIGUOUS_BASE};
    const ALIAS: u32 = 0x0200_0000;
    const SP: u32 = 0x0300_0000;
    let f = function(A, |a| {
        a.mov(dword_ptr(esi), eax)?;
        a.mov(dword_ptr(esi + 4), ecx)?;
        a.mov(edx, dword_ptr(edi + 4))?;
        a.add(dword_ptr(edi + 8), edx)?;
        a.ret()
    });
    let owners = emit::plan(vec![f.clone()]);
    let o = &owners[0];
    let text = emit::c::function(&o.func, &emit::c::OwnerShape {
        selectors: &o.selectors, loops: &o.loops, accounting: false, block_clock: false, functions: None,
    });
    assert!(text.contains("H2_SPAN_ST(sp_"), "{text}");
    let (lib, dispatch) = compile_module_defines(
        "flat-store-order", vec![f], false, true, emit::opt::Passes::ALL, emit::Codegen::default(),
        "#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 1\n",
    );
    type Probe = unsafe extern "C" fn(*mut runtime::ExecCtxOpaque, u32, *mut u8) -> u32;
    // SAFETY: the extra C source above exports this signature.
    let probe: libloading::Symbol<Probe> = unsafe { lib.get(b"h2probe_flat") }.unwrap();
    let rt = Runtime::new(Guest::new(), dispatch);
    // Exercise every VA-to-physical offset on a 16 KiB host. Unwatched
    // writes actually use the shifted VA. Watched writes use the mirror.
    for (address, backing, watched, ro, mapped, fault, trap) in [
        (DATA, 0, true, false, true, false, false),
        (DATA, 4096, false, false, true, false, false),
        (DATA, 8192, false, false, true, false, false),
        (DATA, 12288, false, false, true, false, false),
        (DATA, 4096, true, false, true, false, false),
        (DATA, 0, true, false, true, false, true),
        (DATA + 4092, 8192, true, true, true, true, false),
        (DATA + 4092, 12288, true, false, false, true, false),
        (0xfd00_0000, 4096, true, false, true, true, false),
        (0xffff_ffff, 4096, true, false, true, true, false),
    ] {
        let run = |fast| {
            let mut g = Guest::new();
            g.map(A, 16384, 0x10000, Permissions::RX).unwrap();
            g.map(SP, 16384, 0x14000, Permissions::RW).unwrap();
            g.map(DATA, 16384, backing, Permissions::RW).unwrap();
            g.map(ALIAS, 16384, backing, Permissions::RW).unwrap();
            if ro { g.protect_range(DATA + 4096, 4096, Permissions::READ).unwrap(); }
            if !mapped { g.unmap(DATA + 4096, 4096).unwrap(); }
            g.set_u32(SP, runtime::exec::HARNESS_EXIT).unwrap();
            let w = watched.then(|| {
                let id = g.subscribe_writes("compare");
                g.watch_writes(id, CONTIGUOUS_BASE + backing, 16384).unwrap();
                id
            });
            if trap {
                g.install_trap(64);
                g.arm_all(true, true);
            }
            *rt.memory() = g;
            let mut ctx = ExecCtx::new(rt.clone(), 1);
            ctx.enable_fast_memory(fast);
            let experimental = fast && !trap && cfg!(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")));
            let watch = std::env::var("H2_FLAT_WRITE_WATCH").as_deref() != Ok("0");
            let mask = unsafe { probe((&mut ctx as *mut ExecCtx).cast(), DATA, std::ptr::null_mut()) };
            assert_eq!(mask, if experimental { 1 | (u32::from(!watched || watch) << 1) } else { 0 });
            ctx.arm_preemption(1000);
            let mut cpu = CpuAbi { eax: 13, ecx: 29, esi: address, edi: ALIAS, esp: SP, ..CpuAbi::default() };
            let status = Runner::default().run(&mut cpu, &mut ctx, A);
            let error = ctx.events().iter().find_map(|e| match e {
                Event::MemoryFault { pc, fault } => Some((*pc, fault.clone())),
                _ => None,
            });
            assert_eq!(status.kind, if fault { abi::FAULT } else { abi::STOP });
            let mut g = rt.memory();
            let hits: Vec<_> = g.take_trap_hits().hits.into_iter().filter(|h| h.pc != 0)
                .map(|h| (h.pc, h.addr, h.len, h.write)).collect();
            assert_eq!(!hits.is_empty(), trap);
            if !fault {
                assert_eq!(cpu.edx, 29);
                assert_eq!(g.u32(DATA + 8).unwrap(), 29);
            } else if address == DATA + 4092 {
                assert_eq!(g.u32(ALIAS + 4092).unwrap(), 13);
                assert_eq!(g.u32(ALIAS + 4096).unwrap(), 0);
                assert_eq!(error.as_ref().unwrap().1.addr, DATA + 4096);
            }
            let dirty = w.map(|id| g.take_dirty(id).unwrap());
            let bytes = g.physical_ram()[backing as usize..backing as usize + 16384].to_vec();
            (cpu, ctx.take_slice_spent(), error, hits, dirty, bytes)
        };
        assert_eq!(run(true), run(false), "address {address:#x}, backing={backing:#x}, ro={ro}, mapped={mapped}");
    }

    // Drain at different rates while both VAs name the same shifted page.
    // The generated RMW must execute once on a protection retry, and keep
    // seeing the preceding store after the page moves to the software barrier.
    let epochs = |fast| {
        let mut g = Guest::new();
        g.map(A, 16384, 0x10000, Permissions::RX).unwrap();
        g.map(SP, 16384, 0x14000, Permissions::RW).unwrap();
        for va in [DATA, ALIAS] { g.map(va, 8192, 4096, Permissions::RW).unwrap(); }
        g.set_u32(SP, runtime::exec::HARNESS_EXIT).unwrap();
        let frequent = g.subscribe_writes("each tick");
        let sparse = g.subscribe_writes("every third tick");
        for id in [frequent, sparse] { g.watch_writes(id, CONTIGUOUS_BASE + 4096, 8192).unwrap(); }
        *rt.memory() = g;
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        ctx.enable_fast_memory(fast);
        let mut samples = Vec::new();
        let mut sum = 0;
        for tick in 1..=6 {
            let (src, dst) = if tick % 2 == 0 { (ALIAS, DATA) } else { (DATA, ALIAS) };
            // The two stores share the last eight bytes of a page. The
            // RMW is the first write to the next protected physical page.
            let (src, dst) = (src + 4088, dst + 4088);
            ctx.arm_preemption(1000);
            let mut cpu = CpuAbi { eax: 13, ecx: tick, esi: src, edi: dst, esp: SP, ..CpuAbi::default() };
            assert_eq!(Runner::default().run(&mut cpu, &mut ctx, A).kind, abi::STOP);
            let mut g = rt.memory();
            sum += tick;
            assert_eq!(g.u32(src + 8).unwrap(), sum);
            assert_eq!(g.u32(dst + 4).unwrap(), tick);
            let first = g.take_dirty(frequent).unwrap();
            assert_eq!(first, [runtime::DirtyRange { addr: CONTIGUOUS_BASE + 4096, len: 8192 }]);
            if tick % 3 == 0 { assert_eq!(g.take_dirty(sparse).unwrap(), first); }
            samples.push((cpu, ctx.take_slice_spent(), sum));
        }
        samples
    };
    assert_eq!(epochs(true), epochs(false));

    // Refresh a shifted host page at each end of the 4 GiB guest range.
    // Check the affected entry and the neighbors rebuilt outside it.
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    for base in [0, 0xffff_c000u32] {
        let mut g = Guest::new();
        g.map(base, 16384, 4096, Permissions::RW).unwrap();
        for i in 0..4 { g.set_u8(base + i * 4096 + 4095, (i + 1) as u8).unwrap(); }
        *rt.memory() = g;
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        ctx.enable_fast_memory(true);
        let mut read = |page: u32| {
            let mut value = 0;
            let mask = unsafe { probe((&mut ctx as *mut ExecCtx).cast(), base + page * 4096 + 4095, &mut value) };
            (mask, value)
        };
        for i in 0..4 { assert_eq!(read(i), (3, (i + 1) as u8)); }
        rt.memory().protect_range(base + 4096, 4096, Permissions::READ).unwrap();
        assert_eq!(read(1), (1, 2));
        rt.memory().unmap(base + 8192, 4096).unwrap();
        assert_eq!(read(2), (0, 0));
        {
            let mut g = rt.memory();
            g.map(base + 8192, 4096, 0xc000, Permissions::RW).unwrap();
            g.set_u8(base + 12287, 97).unwrap();
        }
        assert_eq!([read(0), read(1), read(2), read(3)], [(3, 1), (1, 2), (3, 97), (3, 4)]);
    }
}
