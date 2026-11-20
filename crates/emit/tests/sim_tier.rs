//! The sim tier (`emit::sim`) against the IR interpreter, on synthetic
//! code only: integer and flag work, `rep movsd`, packed SSE, x87 with
//! `fnstsw`, and direct calls between tier functions. Every run starts from
//! a seeded random state; a run the tier completes must leave memory and
//! every CPU field but MXCSR exactly as the interpreter does. A run it
//! hands back (a rare event) is counted, and must be the minority.
//!
//! `cargo test -p emit --test sim_tier`

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use emit::compile::{self, Arch};
use emit::host::{BRIDGE, CpuAbi, Generated};
use emit::sim::{self, Closure};
use iced_x86::code_asm::*;
use ir::IrFunction;
use ir::interp::{self, FlatMemory};

const RAM: usize = 1 << 20;
const CODE: u32 = 0x10000;
const DATA: u32 = 0x20000;
const STACK: u32 = 0x80000;
/// Where every root returns: the harness's exit.
const EXIT: u32 = 0xffff_fffc;

const INT: u32 = CODE;
const COPY: u32 = CODE + 0x100;
const CALLER: u32 = CODE + 0x200;
const SSE: u32 = CODE + 0x300;
const X87: u32 = CODE + 0x400;
const X87_CALLER: u32 = CODE + 0x500;

type Boundary = unsafe extern "C" fn(*mut CpuAbi, *mut u8, *mut u32) -> u32;

fn functions() -> Vec<IrFunction> {
    vec![
        // Integer and flags: two stack arguments, `ret 8`.
        common::function(INT, |a| {
            let mut skip = a.create_label();
            a.mov(eax, dword_ptr(esp + 4))?;
            a.add(eax, dword_ptr(esp + 8))?;
            a.adc(ecx, eax)?;
            a.setc(dl)?;
            a.imul_2(eax, ecx)?;
            a.shl(eax, cl)?;
            a.sub(eax, ebx)?;
            a.jz(skip)?;
            a.xor(eax, 0x55)?;
            a.set_label(&mut skip)?;
            a.mov(dword_ptr(esi), eax)?;
            a.mov(byte_ptr(esi + 4), dl)?;
            a.ret_1(8)
        }),
        // `rep movsd` of up to 15 dwords, then one byte.
        common::function(COPY, |a| {
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.and(ecx, 15)?;
            a.mov(edi, esi)?;
            a.add(edi, 0x100)?;
            a.cld()?;
            a.rep().movsd()?;
            a.mov(al, byte_ptr(esi))?;
            a.mov(byte_ptr(edi), al)?;
            a.ret()
        }),
        // Calls INT twice; one argument, `ret 4`.
        common::function(CALLER, |a| {
            a.push(3)?;
            a.push(dword_ptr(esp + 8))?;
            a.call(INT as u64)?;
            a.mov(ebx, eax)?;
            a.push(eax)?;
            a.push(7)?;
            a.call(INT as u64)?;
            a.add(eax, ebx)?;
            a.mov(dword_ptr(esi + 8), eax)?;
            a.ret_1(4)
        }),
        // Packed and scalar SSE through memory at ESI.
        common::function(SSE, |a| {
            a.movups(xmm0, xmmword_ptr(esi + 0x100))?;
            a.movups(xmm1, xmmword_ptr(esi + 0x110))?;
            a.mulps(xmm0, xmm1)?;
            a.addps(xmm0, xmm1)?;
            a.shufps(xmm0, xmm0, 0x1b)?;
            a.movaps(xmm2, xmm0)?;
            a.minps(xmm2, xmm1)?;
            a.comiss(xmm0, xmm1)?;
            a.seta(al)?;
            a.movups(xmmword_ptr(esi + 0x120), xmm2)?;
            a.cvttss2si(edx, xmm0)?;
            a.rcpss(xmm3, xmm1)?;
            a.movss(dword_ptr(esi + 0x130), xmm3)?;
            a.ret()
        }),
        // x87 in the exact subset, with a compare read through `fnstsw`.
        common::function(X87, |a| {
            let mut less = a.create_label();
            a.fld(dword_ptr(esi + 0x200))?;
            a.fmul(dword_ptr(esi + 0x204))?;
            a.fld(qword_ptr(esi + 0x208))?;
            a.fadd(qword_ptr(esi + 0x208))?;
            a.fabs()?;
            a.fsqrt()?;
            a.fcom(dword_ptr(esi + 0x200))?;
            a.fnstsw(ax)?;
            a.test(ah, 0x41)?;
            a.jnz(less)?;
            a.fld1()?;
            a.fdiv(dword_ptr(esi + 0x204))?;
            a.fstp(dword_ptr(esi + 0x210))?;
            a.set_label(&mut less)?;
            a.fild(dword_ptr(esi + 0x220))?;
            a.fdiv(dword_ptr(esi + 0x200))?;
            a.fstp(qword_ptr(esi + 0x228))?;
            a.fstp(dword_ptr(esi + 0x214))?;
            a.fstp(qword_ptr(esi + 0x218))?;
            a.ret()
        }),
        // An x87 caller of an x87 callee: the stack is balanced across it.
        common::function(X87_CALLER, |a| {
            a.fld(dword_ptr(esi + 0x204))?;
            a.call(X87 as u64)?;
            a.fmul(dword_ptr(esi + 0x200))?;
            a.fstp(dword_ptr(esi + 0x230))?;
            a.ret()
        }),
    ]
}

fn next(r: &mut u64) -> u64 {
    *r ^= *r << 13;
    *r ^= *r >> 7;
    *r ^= *r << 17;
    *r
}

/// A float in the tier's exact range: sign, exponent within +-20, 23 random
/// fraction bits; one in eight is zero.
fn float(r: &mut u64) -> u32 {
    let x = next(r);
    if x & 7 == 0 {
        return (x as u32) & 0x8000_0000;
    }
    let exp = 127 - 20 + (x >> 8) % 41;
    ((x >> 3) as u32 & 0x8000_0000) | (exp as u32) << 23 | (x >> 32) as u32 & 0x7f_ffff
}

fn double(r: &mut u64) -> u64 {
    (f64::from(f32::from_bits(float(r))) * 1.25).to_bits()
}

/// A state and memory for one run of `root`.
fn case(seed: u64, root: u32) -> (interp::Cpu, FlatMemory) {
    let mut r = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut mem = FlatMemory::new(0, RAM);
    let mut cpu = interp::Cpu::default();
    for g in &mut cpu.gpr {
        *g = next(&mut r) as u32;
    }
    cpu.gpr[ir::Gpr::Esi.index()] = DATA;
    cpu.gpr[ir::Gpr::Esp.index()] = STACK;
    cpu.eflags = 2 | (next(&mut r) as u32 & 0xcd5) & !0x400;
    cpu.x87.control = 0x027f;
    for i in 0..4u32 {
        let v = (next(&mut r) as u32 >> (i * 3)) & 0xffff;
        assert!(mem.load(STACK + 4 + 4 * i, &v.to_le_bytes()));
    }
    assert!(mem.load(STACK, &EXIT.to_le_bytes()));
    for k in 0..0x100u32 {
        assert!(mem.load(DATA + 4 * k, &(next(&mut r) as u32).to_le_bytes()));
    }
    for k in 0..8u32 {
        assert!(mem.load(DATA + 0x100 + 4 * k, &float(&mut r).to_le_bytes()));
    }
    let f0 = float(&mut r);
    // A divisor in [1, 2).
    let f1 = 0x3f80_0000 | (next(&mut r) as u32 & 0x7f_ffff);
    assert!(mem.load(DATA + 0x200, &f0.to_le_bytes()));
    assert!(mem.load(DATA + 0x204, &f1.to_le_bytes()));
    assert!(mem.load(DATA + 0x208, &double(&mut r).to_le_bytes()));
    assert!(mem.load(DATA + 0x220, &((next(&mut r) as i32) >> 8).to_le_bytes()));
    let _ = root;
    (cpu, mem)
}

/// Run `root` on the interpreter until it returns to [`EXIT`].
fn oracle(funcs: &[IrFunction], cpu: &mut interp::Cpu, mem: &mut FlatMemory, root: u32) {
    let insns: BTreeMap<u32, &ir::IrInsn> = funcs.iter().flat_map(|f| f.insns().map(|i| (i.pc, i))).collect();
    cpu.eip = root;
    for _ in 0..100_000 {
        if cpu.eip == EXIT {
            return;
        }
        let insn = insns.get(&cpu.eip).unwrap_or_else(|| panic!("no instruction at {:#x}", cpu.eip));
        let flow = interp::step(cpu, mem, insn).unwrap_or_else(|e| panic!("{e:?}"));
        cpu.eip = flow.target();
    }
    panic!("{root:#x} did not return");
}

fn build(funcs: &[IrFunction], roots: &[u32], name: &str) -> (Generated, sim::Module) {
    let mut closure = Closure::build(roots, 64, |pc| funcs.iter().find(|f| f.entry == pc).cloned());
    assert!(closure.missing.is_empty(), "{:?}", closure.missing);
    closure.truncated = false;
    let m = sim::write(&closure, roots);
    assert!(m.tier.refused.is_empty(), "{:?}", m.tier.refused);
    let header = common::abi_header().expect("runtime/include/h2abi.h");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let src = dir.join("src");
    emit::write_module_with(&emit::plan(funcs.to_vec()), &src, emit::Codegen::default()).unwrap();
    compile::install_header(&src, &header).unwrap();
    std::fs::write(dir.join("tier.c"), &m.c).unwrap();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let b = compile::build(&src, &dir, Arch::host(), jobs).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    compile::host_dylib(&b, &src, &[("bridge.c", BRIDGE), ("tier.c", m.c.as_str())], &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now from this crate's output for this host.
    let g = unsafe { Generated::load(&dylib) }.unwrap();
    (g, m)
}

fn boundary(g: &Generated, root: u32) -> Boundary {
    let p = g.symbol(&sim::entry_symbol(root)).unwrap_or_else(|| panic!("no boundary for {root:#x}"));
    // SAFETY: `emit::sim` exports every boundary with this signature.
    unsafe { std::mem::transmute::<*const std::ffi::c_void, Boundary>(p) }
}

/// The interpreter's CPU as the comparison sees it: MXCSR and the TSC are
/// outside the tier's contract.
fn comparable(mut c: interp::Cpu) -> interp::Cpu {
    c.mxcsr = 0;
    c.tsc = 0;
    c
}

#[test]
fn tier_matches_the_interpreter_on_random_states() {
    let funcs = functions();
    let roots = [INT, COPY, CALLER, SSE, X87, X87_CALLER];
    let (g, m) = build(&funcs, &roots, "sim_tier");
    assert_eq!(m.roots, roots);
    const RUNS: u64 = 400;
    for root in roots {
        let call = boundary(&g, root);
        let mut rare: BTreeMap<u32, u32> = BTreeMap::new();
        for seed in 0..RUNS {
            let (start, mem0) = case(seed ^ u64::from(root) << 20, root);
            let (mut want, mut want_mem) = (start.clone(), mem0.clone());
            oracle(&funcs, &mut want, &mut want_mem, root);

            let mut mem = mem0.clone();
            let mut cpu = CpuAbi::from_interp(&start);
            cpu.eip = root;
            let mut at = 0u32;
            // SAFETY: `mem` is the whole flat guest space from address 0;
            // every fixture's accesses stay inside it.
            let rc = unsafe { call(&mut cpu, mem.bytes.as_mut_ptr(), &mut at) };
            if rc != 0 {
                *rare.entry(rc).or_default() += 1;
                continue;
            }
            let got = cpu.to_interp();
            assert_eq!(comparable(got), comparable(want.clone()), "{root:#x} seed {seed}: CPU");
            if let Some(i) = (0..RAM).find(|&i| mem.bytes[i] != want_mem.bytes[i]) {
                panic!("{root:#x} seed {seed}: memory differs first at {i:#x}");
            }
        }
        let handed_back: u32 = rare.values().sum();
        eprintln!("{root:#x}: {} of {RUNS} handed back {rare:?}", handed_back);
        assert!(u64::from(handed_back) * 4 < RUNS, "{root:#x}: too many rare events: {rare:?}");
    }
}

#[test]
fn an_x87_state_outside_the_contract_is_refused_at_entry() {
    let funcs = functions();
    let (g, _) = build(&funcs, &[X87], "sim_tier_entry");
    let call = boundary(&g, X87);
    let (start, mem0) = case(1, X87);
    let mut cpu = CpuAbi::from_interp(&start);
    // PC=64: not the tick's control word.
    cpu.x87.control = 0x037f;
    let before = cpu;
    let mut mem = mem0.clone();
    // SAFETY: as in the main test.
    let rc = unsafe { call(&mut cpu, mem.bytes.as_mut_ptr(), std::ptr::null_mut()) };
    assert_eq!(rc, 1, "H2T_RARE_ENTRY");
    assert_eq!(cpu, before);
    assert!(mem.bytes == mem0.bytes);
}
