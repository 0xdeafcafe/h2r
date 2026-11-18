//! The Rust index sort (`engine::render_loop::sort_indexed`, `0x0013df80`)
//! against the original, lifted and run on the IR interpreter, from the same
//! seeded 64 MB state (docs/graphics/render-loop.md).
//!
//! The transparents' comparator (`0x0001e700`) runs for real on the
//! interpreter on the original's side and natively in the port; its records
//! (0x70 bytes) hold keys from small ranges, the draw-order flag in about
//! half, and floats that in some states include both zeros, infinities and
//! NaNs, so the dot-product paths and x86's NaN rules are exercised. A
//! foreign comparator is a stub at an invented address on both sides; it
//! logs every call and answers with seeded junk in EAX's upper bytes, ECX
//! and EDX.
//!
//! Required equal: the index array (the transparents' draw order), the
//! foreign comparator's call log, the registers at return (`xmm0` and
//! `xmm1` included) and all 64 MB (dead stack included).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test transparent_sort_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::sort_indexed::{self as sort, IndexSort};
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

/// The foreign comparator: `a` after `b` by the low byte of each index,
/// with junk elsewhere. `esp` points at the return address.
fn stub(seed: u64, n: usize, gpr: &mut [u32; 8], mem: &FlatMemory) -> CallLog {
    let esp = gpr[4];
    let args = [word(mem, esp + 4), word(mem, esp + 8), word(mem, esp + 12)];
    let log = CallLog { regs: *gpr, args };
    let after = (args[0] & 0xff) > (args[1] & 0xff);
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
            eip: sort::INDEX_SORT,
            ..Cpu::default()
        };
        cpu.xmm[0] = r.xmm0;
        cpu.xmm[1] = r.xmm1;
        let mut pc = sort::INDEX_SORT;
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
                xmm0: cpu.xmm[0],
                xmm1: cpu.xmm[1],
            },
            log,
        ))
    }
}

fn port(seed: u64, mem: &mut FlatMemory, mut r: Regs) -> Result<(Regs, Vec<CallLog>), String> {
    let mut log = Vec::new();
    let (mut s, mut step) =
        IndexSort::start(&mut r, &mut Eng(mem)).map_err(|e| format!("{e:?}"))?;
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

const FLOATS: [u32; 10] = [
    0x0000_0000, // 0
    0x8000_0000, // -0
    0x3f80_0000, // 1
    0xbf80_0000, // -1
    0x4020_0000, // 2.5
    0x3e80_0000, // 0.25
    0x7f80_0000, // inf
    0xff80_0000, // -inf
    0x7fc0_0001, // quiet NaN with a payload
    0x7f80_0001, // signalling NaN
];

/// A seeded state: records with small-range keys, an index array, the
/// stack arguments and the registers.
fn state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs, bool, u32) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64);
    let foreign = case % 4 == 3;
    let count = match case {
        0..=5 => case,
        _ if case % 10 == 9 => 300 + r.below(301),
        _ if case % 5 == 4 => 9 + r.below(100),
        _ => r.below(40),
    };
    let records = count.max(2);
    let range = 1 + r.below(4);
    // Mostly ordinary floats; the specials in some states.
    let specials = r.below(3) == 0;
    let float = |r: &mut Rng| {
        if specials {
            FLOATS[r.below(10) as usize]
        } else {
            FLOATS[r.below(6) as usize]
        }
    };
    for i in 0..records {
        let rec = CONTEXT + i * sort::RECORD_SIZE;
        for w in 0..sort::RECORD_SIZE / 4 {
            put(&mut mem, rec + 4 * w, r.u32());
        }
        put(&mut mem, rec, r.below(range));
        put(&mut mem, rec + 4, float(&mut r));
        put(&mut mem, rec + 8, r.below(range));
        mem.bytes[(rec + 0xc) as usize] = if r.below(2) == 0 {
            0
        } else {
            1 + r.below(255) as u8
        };
        for off in [0x10, 0x14, 0x18, 0x1c, 0x64, 0x68, 0x6c] {
            put(&mut mem, rec + off, float(&mut r));
        }
    }
    for k in 0..3 {
        put(&mut mem, sort::CAMERA + 4 * k, float(&mut r));
    }
    // The index array, with repeats; sometimes at an odd word.
    let base = BASE + 2 * r.below(2);
    for i in 0..count + 2 {
        let v = r.below(records) as u16;
        mem.bytes[(base + 2 * i) as usize..(base + 2 * i + 2) as usize]
            .copy_from_slice(&v.to_le_bytes());
    }
    let target = if foreign {
        FOREIGN
    } else {
        sort::COMPARE_TRANSPARENT
    };
    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, r.u32());
    put(&mut mem, esp + 8, target);
    put(&mut mem, esp + 12, CONTEXT);
    let regs = Regs {
        eax: count,
        ecx: base,
        edx: r.u32(),
        ebx: r.u32(),
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: r.u32(),
        xmm0: (r.next() as u128) << 64 | r.next() as u128,
        xmm1: (r.next() as u128) << 64 | r.next() as u128,
    };
    (mem, regs, foreign, count)
}

#[test]
fn index_sort_matches_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED index_sort_matches_the_lifted_original: no XBE (set H2_XBE)");
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
    let (mut cases, mut native, mut entries, mut foreign_calls, mut max_count) = (0, 0, 0u64, 0, 0);
    for &seed in &seeds {
        for case in 0..60 {
            let (mut a, regs, foreign, count) = state(&image, seed, case);
            let mut b = a.clone();
            let (ra, la) = lifted.run(seed, &mut a, regs).unwrap();
            let (rb, lb) = port(seed, &mut b, regs).unwrap();
            let base = regs.ecx as usize;
            let span = base..base + 2 * count as usize;
            assert_eq!(
                a.bytes[span.clone()],
                b.bytes[span.clone()],
                "seed {seed} case {case} (foreign {foreign}, {count} entries): sorted indices"
            );
            assert_eq!(la, lb, "seed {seed} case {case}: foreign comparator calls");
            assert_eq!(ra, rb, "seed {seed} case {case}: registers at return");
            // memcmp first: the byte search is slow without optimisation.
            if a.bytes != b.bytes
                && let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y)
            {
                panic!(
                    "seed {seed} case {case}: memory differs first at {i:#x}: {:#04x} vs {:#04x}",
                    a.bytes[i], b.bytes[i]
                );
            }
            cases += 1;
            native += !foreign as u32;
            entries += count as u64;
            max_count = max_count.max(count);
            foreign_calls += la.len();
        }
    }
    eprintln!(
        "index sort: {cases} states ({native} with the native comparator), {entries} entries \
         (largest {max_count}), {foreign_calls} foreign comparator calls equal; indices, \
         registers and all 64 MB equal"
    );
    assert!(foreign_calls > 0);
    assert!(max_count >= 300);
}
