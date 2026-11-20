//! Hot-loop benchmark for the IR passes (synthetic code, no game data):
//! each loop is built once per pass selection, and run interleaved with
//! the others, best of several trials. Every build must end with the same
//! registers, flags and memory as the unoptimised one, so the numbers are
//! for code that does the same work.
//!
//! `cargo test -p emit --release --test opt_bench -- --ignored --nocapture`
//! (`H2_OPT` picks the clang level, as for the image; default `-O1`).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, TransferAbi};
use emit::opt::Passes;
use iced_x86::code_asm::*;
use iced_x86::{Decoder, DecoderOptions};
use ir::interp::FlatMemory;
use ir::{IrBlock, IrFunction};

const A: u32 = 0x0040_0000;
const MEM: u32 = 0x0010_0000;
const MEM_LEN: usize = 0x2_0000;
const STACK: u32 = MEM + MEM_LEN as u32 - 0x100;
const SRC: u32 = MEM + 0x1000;
const DST: u32 = MEM + 0x9000;
/// Words in the source buffer the memory loops walk.
const WORDS: u32 = 0x1000;
/// Interleaved trials per build; the best counts. Short trials, many of
/// them: on a loaded machine some run undisturbed.
const TRIALS: usize = 61;

type Body = Box<dyn Fn(&mut CodeAssembler) -> Result<(), IcedError>>;

/// Assemble `body` at `base`, split into blocks at branch targets and
/// after transfers.
fn function(base: u32, body: &Body) -> IrFunction {
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
            ir::Control::Branch { taken, fallthrough, .. } => starts.extend([taken.pc, fallthrough.pc]),
            ir::Control::Jump { target: ir::Target::Direct(t), .. } => starts.extend([*t, i.next_pc()]),
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: BTreeMap<u32, IrBlock> = BTreeMap::new();
    let mut current = base;
    for i in insns {
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks.entry(current).or_insert_with(|| IrBlock { start: current, insns: vec![] }).insns.push(i);
    }
    let f = IrFunction { entry: base, blocks, interior_entries: vec![], errors: vec![] };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

/// The loops: (name, iterations, body). Each returns to the harness.
fn loops() -> Vec<(&'static str, u32, Body)> {
    vec![
        // Register arithmetic: every instruction writes flags, one jnz
        // reads them.
        (
            "alu",
            5_000_000,
            Box::new(|a| {
                let mut l = a.create_label();
                a.mov(ecx, 5_000_000u32)?;
                a.xor(eax, eax)?;
                a.mov(edx, 0x1234_5678u32)?;
                a.set_label(&mut l)?;
                a.add(eax, ecx)?;
                a.xor(edx, eax)?;
                a.shl(edx, 3)?;
                a.sub(ebx, edx)?;
                a.and(ebx, 0x7fff_ffff)?;
                a.or(eax, ebx)?;
                a.dec(ecx)?;
                a.jnz(l)?;
                a.ret()
            }),
        ),
        // Sum a buffer of words, indexing and comparing like compiled C.
        (
            "sum",
            2_000_000,
            Box::new(|a| {
                let (mut outer, mut inner) = (a.create_label(), a.create_label());
                a.mov(edi, 2_000_000u32 / WORDS)?;
                a.xor(eax, eax)?;
                a.set_label(&mut outer)?;
                a.xor(ecx, ecx)?;
                a.set_label(&mut inner)?;
                a.mov(edx, dword_ptr(ecx * 4 + SRC))?;
                a.add(eax, edx)?;
                a.adc(ebx, 0)?;
                a.inc(ecx)?;
                a.cmp(ecx, WORDS)?;
                a.jb(inner)?;
                a.dec(edi)?;
                a.jnz(outer)?;
                a.ret()
            }),
        ),
        // Copy words, with the loop counter in memory like a spilled local.
        (
            "copy",
            2_000_000,
            Box::new(|a| {
                let (mut outer, mut inner) = (a.create_label(), a.create_label());
                a.sub(esp, 8)?;
                a.mov(dword_ptr(esp), 2_000_000u32 / WORDS)?;
                a.set_label(&mut outer)?;
                a.mov(esi, SRC)?;
                a.mov(edi, DST)?;
                a.mov(ecx, WORDS)?;
                a.set_label(&mut inner)?;
                a.mov(eax, dword_ptr(esi))?;
                a.mov(dword_ptr(edi), eax)?;
                a.add(esi, 4)?;
                a.add(edi, 4)?;
                a.dec(ecx)?;
                a.jnz(inner)?;
                a.dec(dword_ptr(esp))?;
                a.jnz(outer)?;
                a.add(esp, 8)?;
                a.ret()
            }),
        ),
        // Compare two equal byte strings, strcmp style, then start over.
        (
            "strcmp",
            2_000_000,
            Box::new(|a| {
                let (mut outer, mut inner, mut done) = (a.create_label(), a.create_label(), a.create_label());
                a.mov(edi, 2_000_000u32 / 0x400)?;
                a.set_label(&mut outer)?;
                a.mov(ecx, SRC)?;
                a.mov(esi, SRC + 0x2000)?;
                a.set_label(&mut inner)?;
                a.mov(bl, byte_ptr(ecx))?;
                a.mov(dl, bl)?;
                a.cmp(bl, byte_ptr(esi))?;
                a.jne(done)?;
                a.inc(ecx)?;
                a.inc(esi)?;
                a.test(dl, dl)?;
                a.jnz(inner)?;
                a.set_label(&mut done)?;
                a.dec(edi)?;
                a.jnz(outer)?;
                a.ret()
            }),
        ),
    ]
}

/// CPU time of this thread: on a shared machine, wall time mostly measures
/// the other processes.
fn thread_time() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid out-pointer for a supported clock.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

fn state() -> (CpuAbi, Ctx) {
    let mut mem = FlatMemory::new(MEM, MEM_LEN);
    for i in 0..WORDS {
        mem.load(SRC + 4 * i, &(i.wrapping_mul(0x9e37_79b9)).to_le_bytes());
    }
    // Two equal NUL-terminated 1023-byte strings for strcmp.
    for i in 0..0x3ff {
        let b = [b'a' + (i % 26) as u8];
        mem.load(SRC + i, &b);
        mem.load(SRC + 0x2000 + i, &b);
    }
    mem.load(SRC + 0x3ff, &[0]);
    mem.load(SRC + 0x23ff, &[0]);
    mem.load(STACK, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK;
    (cpu, Ctx::new(mem))
}

#[test]
#[ignore = "benchmark"]
fn bench_passes() {
    let Some(abi) = common::abi_header() else {
        return;
    };
    let configs: Vec<(String, Passes)> = std::env::var("H2_BENCH_PASSES")
        .unwrap_or_else(|_| "none;all".into())
        .split(';')
        .map(|s| (s.to_string(), Passes::parse(s).unwrap()))
        .collect();
    for (name, n, body) in loops() {
        let f = function(A, &body);
        let built: Vec<_> = configs
            .iter()
            .map(|(cn, p)| {
                let owners = emit::plan_with(vec![f.clone()], *p);
                let tag: String = cn.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                common::build(&owners, &abi, &format!("opt-bench-{name}-{tag}"))
            })
            .collect();
        let mut times = vec![f64::MAX; built.len()];
        let mut ends: Vec<Option<(CpuAbi, Vec<u8>)>> = vec![None; built.len()];
        for _ in 0..TRIALS {
            for (k, g) in built.iter().enumerate() {
                let (mut cpu, mut ctx) = state();
                let t0 = thread_time();
                let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 4);
                times[k] = times[k].min((thread_time() - t0) / n as f64 * 1e9);
                assert_eq!(t.target_pc, HARNESS_EXIT, "{name} {}: {t:x?}", configs[k].0);
                ends[k] = Some((cpu, ctx.mem.bytes.clone()));
            }
        }
        for e in &ends[1..] {
            assert!(e == &ends[0], "{name}: a pass selection changed the result");
        }
        let line: Vec<String> =
            configs.iter().zip(&times).map(|((cn, _), t)| format!("{cn} {t:.3} ns ({:.2}x)", times[0] / t)).collect();
        println!("{name:8} {}", line.join(", "));
    }
}
