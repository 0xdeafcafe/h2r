//! Shared by the pass tests: build functions from assembled synthetic code,
//! and check a pass against the unoptimised IR in the interpreter.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};

use iced_x86::code_asm::{CodeAssembler, IcedError};
use iced_x86::{Decoder, DecoderOptions};
use ir::interp::{self, Cpu, Exit, FlatMemory};
use ir::{Control, IrBlock, IrFunction, Target};
use opt::{Passes, Shape};

pub const CODE: u32 = 0x0040_0000;
pub const MEM: u32 = 0x0010_0000;
pub const MEM_LEN: usize = 0x1_0000;
/// A return here ends a run.
pub const EXIT: u32 = 0xffff_fffc;

pub type Asm = fn(&mut CodeAssembler) -> Result<(), IcedError>;

/// Assemble `body` at [`CODE`] and split it into blocks at branch targets
/// and after transfers (jumps local, calls returning).
pub fn function(body: Asm) -> IrFunction {
    let mut a = CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(CODE as u64).unwrap();
    let env = lift::Env::default();
    let insns: Vec<_> = Decoder::with_ip(32, &code, CODE as u64, DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    let mut starts = BTreeSet::from([CODE]);
    for i in &insns {
        match &i.control {
            Control::Next => {}
            Control::Branch { taken, fallthrough, .. } => starts.extend([taken.pc, fallthrough.pc]),
            Control::Jump { target: Target::Direct(t), .. } => starts.extend([*t, i.next_pc()]),
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: BTreeMap<u32, IrBlock> = BTreeMap::new();
    let mut current = CODE;
    for i in insns {
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks.entry(current).or_insert_with(|| IrBlock { start: current, insns: vec![] }).insns.push(i);
    }
    // A trailing block whose fall-through leaves the code: drop dangling
    // starts so every local successor exists.
    let f = IrFunction { entry: CODE, blocks, interior_entries: vec![], errors: vec![] };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

/// Loop headers: targets of backward local branches and jumps, which is
/// where the emitter's DFS puts them for these straight-line fixtures.
pub fn headers(f: &IrFunction) -> BTreeSet<u32> {
    let mut h = BTreeSet::new();
    for b in f.blocks.values() {
        let last = b.insns.last().unwrap();
        let mut back = |pc: u32, tail: bool| {
            if !tail && pc <= b.start {
                h.insert(pc);
            }
        };
        match &last.control {
            Control::Branch { taken, fallthrough, .. } => {
                back(taken.pc, taken.tail);
                back(fallthrough.pc, fallthrough.tail);
            }
            Control::Jump { target: Target::Direct(t), tail, .. } => back(*t, *tail),
            _ => {}
        }
    }
    h
}

/// Run the passes over a copy of `f` with its natural shape.
pub fn optimized(f: &IrFunction, passes: Passes) -> (IrFunction, opt::Report) {
    let mut g = f.clone();
    let selectors = BTreeSet::from([f.entry]);
    let polled = headers(f);
    let r = opt::optimize(&mut g, &Shape { selectors: &selectors, polled: &polled }, passes);
    g.validate().unwrap_or_else(|e| panic!("optimised IR invalid: {e}\n{g}"));
    (g, r)
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    Returned,
    Left(u32),
    /// Preempted on arrival at a loop header.
    Polled(u32),
    Exit(String),
}

/// xorshift64*: deterministic.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }
}

/// Interpret `f` from `cpu`. On each arrival at a polled header, `poll`
/// decides whether to stop there, as a preempted handler would.
pub fn run(f: &IrFunction, cpu: &mut Cpu, mem: &mut FlatMemory, poll: &mut dyn FnMut() -> bool) -> End {
    let insns: HashMap<u32, &ir::IrInsn> = f.insns().map(|i| (i.pc, i)).collect();
    let polled = headers(f);
    let mut pc = f.entry;
    for _ in 0..200_000 {
        if pc == EXIT {
            return End::Returned;
        }
        let Some(insn) = insns.get(&pc) else {
            return End::Left(pc);
        };
        match interp::step(cpu, mem, insn) {
            Ok(flow) => {
                pc = flow.target();
                if polled.contains(&pc) && f.blocks.contains_key(&pc) && poll() {
                    return End::Polled(pc);
                }
            }
            Err(e) => {
                return End::Exit(match e {
                    Exit::Memory { pc, fault } => format!("memory {pc:#x} {fault:?}"),
                    e => format!("{e:?}"),
                });
            }
        }
    }
    panic!("did not finish")
}

/// A random state: registers mostly pointing into memory, random flags,
/// the stack near the top with [`EXIT`] as the return address.
pub fn state(rng: &mut Rng) -> (Cpu, FlatMemory) {
    let mut mem = FlatMemory::new(MEM, MEM_LEN);
    for b in mem.bytes.iter_mut() {
        *b = rng.next() as u8;
    }
    let mut cpu = Cpu::default();
    for r in cpu.gpr.iter_mut() {
        *r = match rng.next() % 4 {
            0 => rng.next(),
            1 => rng.next() % 64,
            _ => MEM + 0x100 + rng.next() % (MEM_LEN as u32 - 0x400),
        };
    }
    let top = MEM + MEM_LEN as u32 - 0x200;
    cpu.gpr[4] = top;
    mem.load(top, &EXIT.to_le_bytes());
    cpu.eflags = 2 | (rng.next() & 0x0cd5);
    cpu.flags_valid = rng.next() | !0x0cd5;
    (cpu, mem)
}

/// Run `f` and its optimised form from many random states (with random
/// preemption at loop headers) and require identical ends, CPU state
/// (flags and the defined mask included) and memory.
pub fn equivalent(name: &str, f: &IrFunction, passes: Passes, states: usize) {
    let (g, _) = optimized(f, passes);
    let mut rng = Rng(0x6f70_7469_6d69_7365 ^ name.len() as u64);
    for s in 0..states {
        let (cpu0, mem0) = state(&mut rng);
        let seed = rng.next() as u64 | 1;
        let mut ends = Vec::new();
        for func in [f, &g] {
            let (mut cpu, mut mem) = (cpu0.clone(), mem0.clone());
            let mut prng = Rng(seed);
            let end = run(func, &mut cpu, &mut mem, &mut || prng.next().is_multiple_of(7));
            ends.push((end, cpu, mem.bytes));
        }
        let (a, b) = (&ends[0], &ends[1]);
        assert_eq!(a.0, b.0, "{name} state {s}: end differs");
        assert_eq!(a.1, b.1, "{name} state {s}: cpu differs after {:?}\n{g}", a.0);
        assert!(a.2 == b.2, "{name} state {s}: memory differs");
    }
}

/// Synthetic functions covering what the passes reason about: flag chains
/// with partial kills, flags read by branches, setcc, adc, pushfd/lahf,
/// shifts by CL (conditional flag writes), loads and stores that fault,
/// loops with polled headers, string instructions, DF, and atomics.
pub fn fixtures() -> Vec<(&'static str, Asm)> {
    use iced_x86::code_asm::*;
    vec![
        ("alu-chain", |a| {
            a.add(eax, ebx)?;
            a.sub(eax, ecx)?;
            a.inc(eax)?;
            a.xor(edx, eax)?;
            a.ret()
        }),
        ("flags-across-loads", |a| {
            a.add(eax, ebx)?;
            a.mov(ecx, dword_ptr(esi))?;
            a.sub(ecx, 3)?;
            a.mov(dword_ptr(edi), ecx)?;
            a.inc(ecx)?;
            a.ret()
        }),
        ("partial-kill", |a| {
            a.cmp(eax, ebx)?;
            a.inc(ecx)?;
            a.dec(edx)?;
            a.adc(eax, 0)?;
            a.setb(bl)?;
            a.ret()
        }),
        ("branchy", |a| {
            let (mut l1, mut l2) = (a.create_label(), a.create_label());
            a.test(eax, eax)?;
            a.js(l1)?;
            a.add(eax, ecx)?;
            a.jo(l2)?;
            a.sub(eax, edx)?;
            a.set_label(&mut l1)?;
            a.xor(eax, 5)?;
            a.set_label(&mut l2)?;
            a.neg(eax)?;
            a.ret()
        }),
        ("eflags-image", |a| {
            a.add(eax, ebx)?;
            a.pushfd()?;
            a.sub(eax, ecx)?;
            a.lahf()?;
            a.cmp(eax, edx)?;
            a.sahf()?;
            a.popfd()?;
            a.ret()
        }),
        ("shift-by-cl", |a| {
            a.and(ecx, 3)?;
            a.add(eax, ebx)?;
            a.shl(eax, cl)?;
            a.rcl(edx, cl)?;
            a.sar(ebx, cl)?;
            a.ret()
        }),
        ("loop-register", |a| {
            let mut l = a.create_label();
            a.and(ecx, 31)?;
            a.inc(ecx)?;
            a.set_label(&mut l)?;
            a.add(eax, ecx)?;
            a.xor(edx, eax)?;
            a.shl(edx, 3)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        ("loop-memory", |a| {
            let mut l = a.create_label();
            a.and(ecx, 15)?;
            a.inc(ecx)?;
            a.set_label(&mut l)?;
            a.add(eax, dword_ptr(esi))?;
            a.adc(edx, 0)?;
            a.mov(dword_ptr(edi), eax)?;
            a.add(esi, 4)?;
            a.add(edi, 4)?;
            a.dec(ecx)?;
            a.jnz(l)?;
            a.ret()
        }),
        ("strcmp", |a| {
            let (mut l, mut done) = (a.create_label(), a.create_label());
            a.mov(ecx, 64)?;
            a.set_label(&mut l)?;
            a.mov(al, byte_ptr(esi))?;
            a.cmp(al, byte_ptr(edi))?;
            a.jne(done)?;
            a.inc(esi)?;
            a.inc(edi)?;
            a.test(al, al)?;
            a.loopne(l)?;
            a.set_label(&mut done)?;
            a.sbb(eax, eax)?;
            a.ret()
        }),
        ("string-ops", |a| {
            a.and(ecx, 7)?;
            a.cld()?;
            a.add(eax, ebx)?;
            a.rep().movsb()?;
            a.std()?;
            a.cmp(eax, edx)?;
            a.lodsd()?;
            a.cld()?;
            a.repe().cmpsb()?;
            a.ret()
        }),
        ("stack-frame", |a| {
            a.push(ebp)?;
            a.mov(ebp, esp)?;
            a.sub(esp, 16)?;
            a.mov(dword_ptr(esp + 4), eax)?;
            a.add(eax, dword_ptr(esp + 4))?;
            a.mov(dword_ptr(ebp - 4), eax)?;
            a.mov(ecx, dword_ptr(ebp - 4))?;
            a.cmp(ecx, dword_ptr(esp + 4))?;
            a.mov(esp, ebp)?;
            a.pop(ebp)?;
            a.ret()
        }),
        ("divide-and-mul", |a| {
            a.and(ebx, 3)?;
            a.add(eax, 1)?;
            a.mul(ecx)?;
            a.div(ebx)?;
            a.imul_3(eax, edx, 7)?;
            a.ret()
        }),
        ("atomic", |a| {
            a.add(eax, ebx)?;
            a.lock().xadd(dword_ptr(esi), ecx)?;
            a.lock().inc(dword_ptr(edi))?;
            a.cmovb(eax, edx)?;
            a.ret()
        }),
    ]
}
