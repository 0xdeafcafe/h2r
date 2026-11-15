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

fn module(kind: &str, n: u32, instrumented: bool) -> (libloading::Library, Dispatch) {
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
    let owners = emit::plan(funcs);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("perf-{kind}-{instrumented}"));
    let src = dir.join("src");
    emit::write_module(&owners, &src).unwrap();
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
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib).unwrap();
    // Initialize x87's helper bridge, then replace the memory/event callbacks
    // with the production runtime. Both bridges implement h2abi.h exactly.
    let _test_host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let mut dispatch = Dispatch::default();
    unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const *const ())> =
            lib.get(b"h2bridge_install").unwrap();
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
        ];
        let layout: libloading::Symbol<unsafe extern "C" fn(u32) -> usize> =
            lib.get(b"h2bridge_layout").unwrap();
        assert_eq!(layout(callbacks.len() as u32), size_of_val(&callbacks));
        install(callbacks.as_ptr());
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
        );
        let mut mem = Guest::new();
        mem.map_zeroed(A, 4096, Permissions::RX).unwrap();
        mem.map_zeroed(DATA, 8192, Permissions::RW).unwrap();
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
        let (_lib, dispatch) = module(kind, 11, true);
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
