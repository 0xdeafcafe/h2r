//! The Rust layer entry loop (`engine::render_loop::layer::Entries`,
//! `0x0003bc30`) against the original, lifted and run on the IR
//! interpreter, from the same seeded 64 MB state (docs/graphics/layer-draw.md).
//! The native lights' split test drives the loop through it.
//!
//! Every callee is a stub on both sides that logs the call (target, every general
//! register, the stack arguments) and answers from a seeded generator.
//! The entry build `0x0003bab0` writes a seeded list of entries into the
//! loop's buffer and returns their count; the entry handlers are stubs at
//! invented addresses in the list's handler table. The call logs, the
//! registers at return and all 64 MB must be equal (the port pushes the
//! original's return addresses here). The original's `_chkstk` runs on
//! the interpreter; the port does its probes and writes itself.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it layer_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::layer::{self, Entries};
use engine::render_loop::{Regs, Step};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;
/// The list, its handler table, entry targets and the handlers' addresses.
const LIST: u32 = 0x0300_0000;
const TABLE: u32 = 0x0300_1000;
const TARGETS: u32 = 0x0300_4000;
const HANDLERS: u32 = 0x03f0_0000;
const KINDS: u32 = 6;

struct Eng<'a>(&'a mut FlatMemory);

impl GuestMemory for Eng<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let src = self.0.bytes.get(a..a + buf.len()).ok_or(MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })?;
        buf.copy_from_slice(src);
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let dst = self.0.bytes.get_mut(a..a + data.len()).ok_or(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })?;
        dst.copy_from_slice(data);
        Ok(())
    }
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn u32(&mut self) -> u32 {
        self.next() as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes())
}
fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

fn is_handler(a: u32) -> bool {
    (HANDLERS..HANDLERS + 0x100 * KINDS).contains(&a)
}

fn is_callee(a: u32) -> bool {
    a == layer::LAYER_BUILD || is_handler(a)
}

fn cleanup(target: u32) -> u32 {
    match target {
        layer::LAYER_BUILD => 16,
        t if is_handler(t) => layer::HANDLER_CLEANUP,
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallLog {
    target: u32,
    regs: [u32; 8],
    args: Vec<u32>,
}

/// The stub both sides call; `esp` points at the return address.
fn stub(seed: u64, n: usize, target: u32, gpr: &mut [u32; 8], mem: &mut FlatMemory) -> CallLog {
    let esp = gpr[4];
    let args = (0..cleanup(target) / 4)
        .map(|i| word(mem, esp + 4 + 4 * i))
        .collect();
    let log = CallLog {
        target,
        regs: *gpr,
        args,
    };
    let mut r = Rng(seed ^ (n as u64).wrapping_mul(0x1000_0001) ^ target as u64);
    gpr[0] = r.u32();
    gpr[1] = r.u32();
    gpr[2] = r.u32();
    if target == layer::LAYER_BUILD {
        // Write 0 to 40 entries, sometimes a negative count.
        let buffer = word(mem, esp + 16);
        let n = r.below(41);
        for i in 0..n {
            let e = buffer + i * layer::ENTRY_SIZE;
            for w in 0..layer::ENTRY_SIZE / 4 {
                put(mem, e + 4 * w, r.u32());
            }
            let kind = r.below(KINDS) as u16;
            mem.bytes[e as usize..e as usize + 2].copy_from_slice(&kind.to_le_bytes());
            put(mem, e + 0x10, TARGETS + 0x200 * r.below(8));
        }
        gpr[0] = if r.below(13) == 0 {
            r.u32() | 0x8000_0000
        } else {
            n
        };
    }
    log
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    fn run(
        &mut self,
        entry: u32,
        seed: u64,
        mem: &mut FlatMemory,
        r: Regs,
    ) -> Result<(Regs, Vec<CallLog>), String> {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: entry,
            ..Cpu::default()
        };
        let mut pc = entry;
        let mut log = Vec::new();
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if is_callee(pc) {
                log.push(stub(seed, log.len(), pc, &mut cpu.gpr, mem));
                let esp = cpu.gpr[4];
                pc = word(mem, esp);
                cpu.gpr[4] = esp + 4 + cleanup(log.last().unwrap().target);
                continue;
            }
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
            }
            if !self.cache.contains_key(&pc)
                && let Some(f) = self.lifter.lift(pc)
            {
                for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                    self.cache.entry(insn.pc).or_insert(insn);
                }
            }
            let insn = self
                .cache
                .get(&pc)
                .ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(&mut cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        let g = cpu.gpr;
        Ok((
            Regs {
                eax: g[0],
                ecx: g[1],
                edx: g[2],
                ebx: g[3],
                esp: g[4],
                ebp: g[5],
                esi: g[6],
                edi: g[7],
                xmm0: 0,
                xmm1: 0,
            },
            log,
        ))
    }
}

fn port(seed: u64, mem: &mut FlatMemory, mut r: Regs) -> Result<(Regs, Vec<CallLog>), String> {
    let e = |f: MemFault| format!("{f:?}");
    let mut log = Vec::new();
    let (mut m, mut step) = Entries::start(&mut r, &mut Eng(mem)).map_err(e)?;
    loop {
        match step {
            Step::Return { to } => {
                assert_eq!(to, HARNESS_EXIT);
                return Ok((r, log));
            }
            Step::Call {
                target,
                site,
                cleanup: c,
            } => {
                assert_eq!(c, cleanup(target), "{target:#x}");
                r.esp -= 4;
                put(mem, r.esp, site);
                let mut gpr = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];
                log.push(stub(seed, log.len(), target, &mut gpr, mem));
                [r.eax, r.ecx, r.edx, r.ebx, _, r.ebp, r.esi, r.edi] = gpr;
                r.esp += 4 + c;
                step = m.resume(&mut r, &mut Eng(mem)).map_err(e)?;
            }
        }
    }
}

/// A seeded state for the entry loop.
fn state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64 ^ layer::LAYER_ENTRIES as u64);
    // The list's handler table: KINDS records, each handler a stub.
    put(&mut mem, LIST + 0x14, TABLE);
    for k in 0..KINDS {
        let rec = TABLE + k * layer::RECORD_SIZE;
        for w in 0..layer::RECORD_SIZE / 4 {
            put(&mut mem, rec + 4 * w, r.u32());
        }
        put(&mut mem, rec + 4, HANDLERS + 0x100 * k);
    }
    // Entry targets: `+0x100` is -1 (skipped) in some.
    for t in 0..8 {
        let v = if r.below(4) == 0 { u32::MAX } else { r.u32() };
        put(&mut mem, TARGETS + 0x200 * t + 0x100, v);
    }
    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, r.u32());
    let regs = Regs {
        eax: r.u32(),
        ecx: r.u32(),
        edx: r.u32(),
        ebx: r.below(32),
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: LIST,
        xmm0: 0,
        xmm1: 0,
    };
    (mem, regs)
}

#[test]
fn layer_entries_match_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED layer_entries_match_the_lifted_original: no XBE (set H2_XBE)");
        return;
    };
    let data = std::fs::read(&path).unwrap();
    assert_eq!(sha256(&data), IMAGE_SHA256, "the retail XBE only");
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    let mut lifted = Lifted {
        lifter: &lifter,
        cache: BTreeMap::new(),
    };
    let seeds: Vec<u64> = std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99]);
    let entry = layer::LAYER_ENTRIES;
    {
        let (mut cases, mut calls) = (0, 0);
        let mut targets: BTreeMap<u32, u64> = BTreeMap::new();
        for &seed in &seeds {
            for case in 0..64 {
                let (mut a, regs) = state(&image, seed, case);
                let mut b = a.clone();
                let (ra, la) = lifted.run(entry, seed, &mut a, regs).unwrap();
                let (rb, lb) = port(seed, &mut b, regs).unwrap();
                assert_eq!(
                    la.len(),
                    lb.len(),
                    "{entry:#x} seed {seed} case {case}: call count\n{la:#x?}\n{lb:#x?}"
                );
                for (i, (x, y)) in la.iter().zip(&lb).enumerate() {
                    assert_eq!(x, y, "{entry:#x} seed {seed} case {case}: call {i}");
                }
                assert_eq!(ra, rb, "{entry:#x} seed {seed} case {case}: registers");
                if let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                    panic!(
                        "{entry:#x} seed {seed} case {case}: memory differs first at {i:#x}: {:#04x} vs {:#04x}",
                        a.bytes[i], b.bytes[i]
                    );
                }
                cases += 1;
                calls += la.len();
                for c in &la {
                    let key = if is_handler(c.target) {
                        HANDLERS
                    } else {
                        c.target
                    };
                    *targets.entry(key).or_default() += 1;
                }
            }
        }
        eprintln!(
            "{entry:#010x}: {cases} states, {calls} calls equal, registers and all 64 MB equal; calls by target (handlers as {HANDLERS:#x}): {targets:#x?}"
        );
        for t in &[layer::LAYER_BUILD, HANDLERS] {
            assert!(targets.contains_key(t), "{t:#x} never called");
        }
    }
}
