//! The Rust qsort (`engine::render_loop::sort`, `0x0013da70`) against the
//! original, lifted and run on the IR interpreter, from the same seeded
//! 64 MB state (docs/graphics/layer-draw.md).
//!
//! The layer build's two comparators (`0x0003bb80`, `0x0003bbd0`) run for
//! real on the interpreter on the original's side and natively in the port.
//! A third, foreign comparator is a stub at an invented address on both
//! sides; it logs every call (every register and its three arguments) and
//! answers from the entries' keys, with seeded junk in EAX's upper bytes,
//! ECX and EDX, so the port's guest-call path is checked too.
//!
//! Keys are drawn from small ranges, so most arrays hold many equal
//! entries: the qsort is not stable, and only the same algorithm leaves
//! ties in the same order. The sorted buffer is the order the entry loop
//! draws in, so equal buffers mean an equal draw stream from the layer.
//! Required equal: the buffer, the foreign comparator's call log, the
//! registers at return and all 64 MB (dead stack included).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test sort_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::sort::{self, Sort};
use engine::render_loop::{Regs, Step};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 200_000_000;
const BASE: u32 = 0x0300_0000;
const FOREIGN: u32 = 0x03f0_0000;
const CONTEXT: u32 = 0x0380_0000;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallLog {
    regs: [u32; 8],
    args: [u32; 3],
}

/// The foreign comparator: `a` after `b` by the first byte of each, with
/// junk elsewhere. `esp` points at the return address.
fn stub(seed: u64, n: usize, gpr: &mut [u32; 8], mem: &FlatMemory) -> CallLog {
    let esp = gpr[4];
    let args = [word(mem, esp + 4), word(mem, esp + 8), word(mem, esp + 12)];
    let log = CallLog { regs: *gpr, args };
    let after = mem.bytes[args[0] as usize] > mem.bytes[args[1] as usize];
    let mut r = Rng(seed ^ (n as u64).wrapping_mul(0x1000_0001));
    gpr[0] = (r.u32() & !0xff) | after as u32;
    gpr[1] = r.u32();
    gpr[2] = r.u32();
    log
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    fn run(
        &mut self,
        seed: u64,
        mem: &mut FlatMemory,
        r: Regs,
    ) -> Result<(Regs, Vec<CallLog>), String> {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: sort::QSORT,
            ..Cpu::default()
        };
        let mut pc = sort::QSORT;
        let mut log = Vec::new();
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if pc == FOREIGN {
                log.push(stub(seed, log.len(), &mut cpu.gpr, mem));
                let esp = cpu.gpr[4];
                pc = word(mem, esp);
                cpu.gpr[4] = esp + 4 + sort::COMPARE_CLEANUP;
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
                ..r
            },
            log,
        ))
    }
}

fn port(seed: u64, mem: &mut FlatMemory, mut r: Regs) -> Result<(Regs, Vec<CallLog>), String> {
    let mut log = Vec::new();
    let (mut s, mut step) = Sort::start(&mut r, &mut Eng(mem)).map_err(|e| format!("{e:?}"))?;
    loop {
        match step {
            Step::Return { to } => {
                assert_eq!(to, HARNESS_EXIT);
                return Ok((r, log));
            }
            Step::Call {
                target,
                site,
                cleanup,
            } => {
                assert_eq!(target, FOREIGN, "only the foreign comparator is a call");
                assert_eq!(cleanup, sort::COMPARE_CLEANUP);
                r.esp -= 4;
                Eng(mem).set_u32(r.esp, site).unwrap();
                let mut gpr = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];
                log.push(stub(seed, log.len(), &mut gpr, mem));
                [r.eax, r.ecx, r.edx, r.ebx, _, r.ebp, r.esi, r.edi] = gpr;
                r.esp += 4 + cleanup;
                step = s
                    .resume(&mut r, &mut Eng(mem))
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
    }
}

/// What a case sorts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Layer,
    Layer5,
    Foreign,
}

/// A seeded state: an array of entries with small-range keys, the stack
/// arguments and the registers.
fn state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs, Cmp, u32, u32, u32) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64);
    let cmp = match case % 3 {
        0 => Cmp::Layer,
        1 => Cmp::Layer5,
        _ => Cmp::Foreign,
    };
    // Counts: the edges first, then small, then large.
    let count = match case {
        0..=5 => case,
        _ if case % 10 == 9 => 1000 + r.below(1561),
        _ if case % 5 == 4 => 9 + r.below(200),
        _ => r.below(40),
    };
    // Mostly the layer build's 0x18; other sizes and an unaligned base
    // for the byte-swap paths.
    let (size, base) = match (cmp, case % 4) {
        (Cmp::Foreign, 1) => (6, BASE),
        (Cmp::Foreign, 2) => (8, BASE + 2),
        (_, 3) => (0x18, BASE + 1 + r.below(3)),
        _ => (0x18, BASE),
    };
    let size = if case == 7 { 0 } else { size };
    let range = 1 + r.below(4);
    for i in 0..count.max(4) * size / 4 + 4 {
        put(&mut mem, base + 4 * i, r.u32());
    }
    for i in 0..count {
        let e = (base + i * size) as usize;
        let mut k = |n: u32| (r.below(range) * n) as u8;
        if size == 0x18 {
            // Every key field the two comparators read, small ranges.
            put(&mut mem, e as u32 + 0x10, k(1) as u32);
            put(&mut mem, e as u32 + 0x0c, k(1) as u32);
            for off in [0usize, 2, 6, 8] {
                mem.bytes[e + off] = k(1);
                mem.bytes[e + off + 1] = 0;
            }
            mem.bytes[e + 5] = k(1);
        } else {
            mem.bytes[e] = k(1);
        }
    }
    let target = match cmp {
        Cmp::Layer => sort::COMPARE_LAYER,
        Cmp::Layer5 => sort::COMPARE_LAYER_5,
        Cmp::Foreign => FOREIGN,
    };
    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, size);
    put(&mut mem, esp + 8, target);
    put(&mut mem, esp + 12, CONTEXT);
    let regs = Regs {
        eax: base,
        ecx: count,
        edx: r.u32(),
        ebx: r.u32(),
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: r.u32(),
        ..Regs::default()
    };
    (mem, regs, cmp, count, size, base)
}

#[test]
fn qsort_matches_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED qsort_matches_the_lifted_original: no XBE (set H2_XBE)");
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
    let (mut cases, mut entries, mut foreign_calls, mut max_count) = (0, 0u64, 0, 0);
    let mut by_cmp: BTreeMap<String, u32> = BTreeMap::new();
    for &seed in &seeds {
        for case in 0..40 {
            let (mut a, regs, cmp, count, size, base) = state(&image, seed, case);
            let mut b = a.clone();
            let (ra, la) = lifted.run(seed, &mut a, regs).unwrap();
            let (rb, lb) = port(seed, &mut b, regs).unwrap();
            let span = (base as usize)..(base + count * size) as usize;
            assert_eq!(
                a.bytes[span.clone()],
                b.bytes[span.clone()],
                "seed {seed} case {case} ({cmp:?}, {count} x {size:#x}): sorted buffer"
            );
            assert_eq!(la, lb, "seed {seed} case {case}: foreign comparator calls");
            assert_eq!(ra, rb, "seed {seed} case {case}: registers at return");
            if let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {case}: memory differs first at {i:#x}: {:#04x} vs {:#04x}",
                    a.bytes[i], b.bytes[i]
                );
            }
            cases += 1;
            entries += count as u64;
            max_count = max_count.max(count);
            foreign_calls += la.len();
            *by_cmp.entry(format!("{cmp:?}")).or_default() += 1;
        }
    }
    eprintln!(
        "qsort: {cases} states ({by_cmp:?}), {entries} entries (largest {max_count}), \
         {foreign_calls} foreign comparator calls equal; buffers, registers and all 64 MB equal"
    );
    assert!(foreign_calls > 0);
    assert!(max_count >= 1000);
}
