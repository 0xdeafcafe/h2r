//! Stage 7a's proof: the Rust `render_views` (`engine::render_loop`)
//! against the original, lifted and run on the IR interpreter, from the
//! same seeded 64 MB state (docs/render-loop.md).
//!
//! Every callee is a stub on both sides: when the original reaches a
//! callee's entry, and when the port asks for a call, the same stub logs
//! the call (target, every general register, `xmm0`, the stack arguments)
//! and answers from a seeded generator (EAX, ECX, EDX, `xmm0`, the camera
//! block `render_game_view` leaves behind). So the comparison is the call
//! stream into the recompiled pass bodies, which is what decides the draw
//! stream: the logs must be equal, then the registers at return, then all
//! of memory byte for byte (the port pushes the original's return
//! addresses here, so even the dead stack matches).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test render_loop_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::{Machine, Regs, Step, views};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const VIEWS: u32 = 0x0300_0000;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

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

/// One call into a callee, as the callee sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallLog {
    target: u32,
    regs: [u32; 8],
    xmm0: u128,
    args: Vec<u32>,
    /// The bytes EAX points at, for the frame record.
    record: Vec<u8>,
}

/// The stub both sides call. `esp` points at the return address.
fn stub(
    seed: u64,
    n: usize,
    target: u32,
    gpr: &mut [u32; 8],
    xmm0: &mut u128,
    mem: &mut FlatMemory,
) -> CallLog {
    let esp = gpr[4];
    let word =
        |a: u32| u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap());
    let args = (0..views::cleanup(target) / 4)
        .map(|i| word(esp + 4 + 4 * i))
        .collect();
    let record = if target == views::FRAME_BEGIN {
        mem.bytes[gpr[0] as usize..gpr[0] as usize + 0x18].to_vec()
    } else {
        Vec::new()
    };
    let log = CallLog {
        target,
        regs: *gpr,
        xmm0: *xmm0,
        args,
        record,
    };
    let mut r = Rng(seed ^ (n as u64).wrapping_mul(0x1000_0001) ^ target as u64);
    gpr[0] = r.u32();
    gpr[1] = r.u32();
    gpr[2] = r.u32();
    *xmm0 = (r.next() as u128) << 64 | r.next() as u128;
    if target == views::VIEW_TEST {
        gpr[0] = (gpr[0] & !0xff) | r.below(2);
    }
    if target == views::GAME_VIEW {
        for i in 0..3 {
            let v = r.u32().to_le_bytes();
            let a = 0x004b_9dac + 4 * i;
            mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v);
        }
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
        seed: u64,
        mem: &mut FlatMemory,
        r: Regs,
    ) -> Result<(Regs, Vec<CallLog>), String> {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: 0x0002_b5d0,
            ..Cpu::default()
        };
        cpu.xmm[0] = r.xmm0;
        let mut pc = 0x0002_b5d0;
        let mut log = Vec::new();
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if views::CALLEES.contains(&pc) {
                let mut x = cpu.xmm[0];
                log.push(stub(seed, log.len(), pc, &mut cpu.gpr, &mut x, mem));
                cpu.xmm[0] = x;
                let esp = cpu.gpr[4];
                pc = u32::from_le_bytes(
                    mem.bytes[esp as usize..esp as usize + 4]
                        .try_into()
                        .unwrap(),
                );
                cpu.gpr[4] = esp + 4 + views::cleanup(log.last().unwrap().target);
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
            },
            log,
        ))
    }
}

fn port(seed: u64, mem: &mut FlatMemory, mut r: Regs) -> Result<(Regs, Vec<CallLog>), String> {
    let mut log = Vec::new();
    let (mut m, mut step) = Machine::start(&mut r, &mut Eng(mem)).map_err(|e| format!("{e:?}"))?;
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
                assert_eq!(cleanup, views::cleanup(target));
                r.esp -= 4;
                Eng(mem).set_u32(r.esp, site).unwrap();
                let mut gpr = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];
                log.push(stub(seed, log.len(), target, &mut gpr, &mut r.xmm0, mem));
                [r.eax, r.ecx, r.edx, r.ebx, _, r.ebp, r.esi, r.edi] = gpr;
                r.esp += 4 + cleanup;
                step = m
                    .resume(&mut r, &mut Eng(mem))
                    .map_err(|e| format!("{e:?}"))?;
            }
        }
    }
}

/// A seeded state: the globals `render_views` touches, a view array and
/// the registers.
fn state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64);
    let put = |mem: &mut FlatMemory, a: u32, v: u32| {
        mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes())
    };
    // Debug hook on in a third of the cases.
    let debug = if r.below(3) == 0 { r.u32() | 1 } else { 0 };
    put(&mut mem, 0x0047_989c, debug);
    for a in [
        0x004b_a034,
        0x004b_a030,
        0x0045_dd78,
        0x004b_9dac,
        0x004b_9db0,
        0x004b_9db4,
    ] {
        put(&mut mem, a, r.u32());
    }
    // The frame time: any bits, a signalling NaN in some cases.
    let (lo, mut hi) = (r.u32(), r.u32());
    if case % 7 == 3 {
        hi = 0x7ff0_0000 | (hi & 0x0007_ffff) | 1;
    }
    put(&mut mem, 0x004b_a040, lo);
    put(&mut mem, 0x004b_a044, hi);
    for i in 0..16 {
        put(&mut mem, 0x004c_56c0 + 4 * i, r.u32());
    }
    mem.bytes[0x0050_93fc] = if r.below(2) == 0 {
        0
    } else {
        r.u32() as u8 | 1
    };
    // 0 to 5 views; sometimes a negative count.
    let count = if case % 11 == 5 {
        r.u32() | 0x8000_0000
    } else {
        r.below(6)
    };
    for v in 0..6u32 {
        let rec = VIEWS + v * views::VIEW_STRIDE;
        for w in 0..views::VIEW_STRIDE / 4 {
            put(&mut mem, rec + 4 * w, r.u32());
        }
        if r.below(3) != 0 {
            put(&mut mem, rec, 0);
        }
    }
    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    put(&mut mem, esp, HARNESS_EXIT);
    let (a0, a1) = (r.u32(), r.u32());
    put(&mut mem, esp + 4, a0);
    put(&mut mem, esp + 8, a1);
    put(&mut mem, esp + 12, VIEWS);
    let regs = Regs {
        eax: count,
        ecx: r.u32(),
        edx: r.u32(),
        ebx: r.u32(),
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: r.u32(),
        xmm0: (r.next() as u128) << 64 | r.next() as u128,
    };
    (mem, regs)
}

#[test]
fn render_views_matches_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED render_views_matches_the_lifted_original: no XBE (set H2_XBE)");
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
    let (mut cases, mut calls, mut views_drawn, mut debug_cases) = (0, 0, 0, 0);
    let mut targets: BTreeMap<u32, u64> = BTreeMap::new();
    for &seed in &seeds {
        for case in 0..64 {
            let (mut a, regs) = state(&image, seed, case);
            let mut b = a.clone();
            let (ra, la) = lifted.run(seed, &mut a, regs).unwrap();
            let (rb, lb) = port(seed, &mut b, regs).unwrap();
            assert_eq!(
                la.len(),
                lb.len(),
                "seed {seed} case {case}: call count\n{la:#x?}\n{lb:#x?}"
            );
            for (i, (x, y)) in la.iter().zip(&lb).enumerate() {
                assert_eq!(x, y, "seed {seed} case {case}: call {i}");
            }
            assert_eq!(ra, rb, "seed {seed} case {case}: registers at return");
            if let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {case}: memory differs first at {i:#x}: {:#04x} vs {:#04x}",
                    a.bytes[i], b.bytes[i]
                );
            }
            cases += 1;
            calls += la.len();
            views_drawn += la.iter().filter(|c| c.target == views::GAME_VIEW).count();
            debug_cases += la.iter().any(|c| c.target == views::DEBUG_BEGIN) as usize;
            for c in &la {
                *targets.entry(c.target).or_default() += 1;
            }
        }
    }
    eprintln!(
        "render_views: {cases} states, {calls} calls equal ({views_drawn} game views, {debug_cases} with the debug hook), registers and all 64 MB equal; calls by target: {targets:#x?}"
    );
    for t in views::CALLEES {
        assert!(
            targets.contains_key(&t),
            "{t:#x} never called: widen the states"
        );
    }
}
