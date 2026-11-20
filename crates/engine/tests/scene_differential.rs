//! Stage 7a, continued: the Rust `render_game_view` and
//! `render_scene_passes` (`engine::render_loop::scene`) against the
//! originals, lifted and run on the IR interpreter, from the same seeded
//! 64 MB state (docs/graphics/render-loop.md).
//!
//! As in `render_loop_differential.rs`, every callee is a stub on both
//! sides that logs the call (target, every general register, `xmm0`,
//! `xmm1`, the stack arguments) and answers from a seeded generator: EAX,
//! ECX, EDX, `xmm0` and `xmm1` always, and EBX or EDI in some calls (the
//! callees are LTCG code; the port must use what they leave, not what it
//! set). Predicates answer 0 or 1 in AL, the object lookups small indices
//! into synthetic tables. The logs must be equal, then the registers at
//! return, then all 64 MB byte for byte (the port's driver pushes the
//! original's return addresses, so the dead stack matches too). Equal call
//! streams into the pass bodies with equal memory mean equal draws.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it scene_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::scene::{GameView, ScenePasses, game_view as gv, scene_passes as sp};
use engine::render_loop::{Regs, Step};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

/// Synthetic data, above the image.
const VIEW: u32 = 0x0300_0000;
const TABLES: u32 = 0x0280_0000;
const T_OBJECTS: u32 = TABLES;
const T_DATA: u32 = TABLES + 0x100;
const B_DATA: u32 = TABLES + 0x1000;
const T_300: u32 = TABLES + 0x200;
const B_300: u32 = TABLES + 0x400;
const OBJS: u32 = TABLES + 0x10000;
const T_TAGS: u32 = TABLES + 0x300;
const SHADERS: u32 = TABLES + 0x20000;
const PAIR_SRC: u32 = TABLES + 0x500;
const OBSERVER: u32 = TABLES + 0x600;

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
    fn pick(&mut self, xs: &[u32]) -> u32 {
        xs[self.below(xs.len() as u32) as usize]
    }
    /// One of `xs`, or a random word.
    fn pick_or_any(&mut self, xs: &[u32]) -> u32 {
        let any = self.u32();
        match self.below(xs.len() as u32 + 1) as usize {
            i if i < xs.len() => xs[i],
            _ => any,
        }
    }
}

/// Which function is under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fun {
    GameView,
    ScenePasses,
}

impl Fun {
    fn entry(self) -> u32 {
        match self {
            Fun::GameView => 0x0002_b790,
            Fun::ScenePasses => 0x0002_bcd0,
        }
    }
    fn callees(self) -> &'static [u32] {
        match self {
            Fun::GameView => &gv::CALLEES,
            Fun::ScenePasses => &sp::CALLEES,
        }
    }
    fn cleanup(self, t: u32) -> u32 {
        match self {
            Fun::GameView => gv::cleanup(t),
            Fun::ScenePasses => sp::cleanup(t),
        }
    }
}

/// One call into a callee, as the callee sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallLog {
    target: u32,
    regs: [u32; 8],
    xmm0: u128,
    xmm1: u128,
    args: Vec<u32>,
}

/// A small index with random high bits (the callers mask with 0xffff).
fn index(r: &mut Rng) -> u32 {
    (r.u32() & 0xffff_0000) | r.below(4)
}

/// The stub both sides call. `esp` points at the return address.
fn stub(
    fun: Fun,
    seed: u64,
    n: usize,
    target: u32,
    gpr: &mut [u32; 8],
    xmm: &mut [u128; 2],
    mem: &mut FlatMemory,
) -> CallLog {
    let esp = gpr[4];
    let word =
        |a: u32| u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap());
    let args = (0..fun.cleanup(target) / 4)
        .map(|i| word(esp + 4 + 4 * i))
        .collect();
    let log = CallLog {
        target,
        regs: *gpr,
        xmm0: xmm[0],
        xmm1: xmm[1],
        args,
    };
    let mut r = Rng(seed ^ (n as u64).wrapping_mul(0x1000_0001) ^ target as u64);
    gpr[0] = r.u32();
    gpr[1] = r.u32();
    gpr[2] = r.u32();
    xmm[0] = (r.next() as u128) << 64 | r.next() as u128;
    xmm[1] = (r.next() as u128) << 64 | r.next() as u128;
    // Callee-saved by convention, not by contract: some calls change them.
    if r.below(5) == 0 {
        gpr[3] = r.pick_or_any(&[0, 1, 2, 3]);
    }
    if r.below(5) == 0 {
        gpr[7] = r.pick_or_any(&[0, 2, 7]);
    }
    let bit = |r: &mut Rng, g: u32| (g & !0xff) | r.below(2);
    match (fun, target) {
        (Fun::GameView, gv::PREPARE | gv::OBSERVER) => gpr[0] = bit(&mut r, gpr[0]),
        (Fun::ScenePasses, sp::SKIP_A | sp::SKIP_B) => {
            gpr[0] = (gpr[0] & !0xff) | (r.below(8) == 0) as u32
        }
        (Fun::ScenePasses, sp::LAYER_BEGIN | sp::OBJ_14DDC0 | sp::PASS_3BA90) => {
            gpr[0] = bit(&mut r, gpr[0])
        }
        (Fun::ScenePasses, sp::OBJ_155760) => {
            if r.below(3) != 0 {
                gpr[0] = 0
            }
        }
        (Fun::ScenePasses, sp::OBJ_14DE70) => gpr[0] = index(&mut r),
        (Fun::ScenePasses, sp::OBJ_CBD50) => {
            gpr[0] = if r.below(4) == 0 {
                u32::MAX
            } else {
                index(&mut r)
            }
        }
        _ => {}
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
        fun: Fun,
        seed: u64,
        mem: &mut FlatMemory,
        r: Regs,
    ) -> Result<(Regs, Vec<CallLog>), String> {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: fun.entry(),
            ..Cpu::default()
        };
        cpu.xmm[0] = r.xmm0;
        cpu.xmm[1] = r.xmm1;
        let mut pc = fun.entry();
        let mut log = Vec::new();
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if fun.callees().contains(&pc) {
                let mut x = [cpu.xmm[0], cpu.xmm[1]];
                log.push(stub(fun, seed, log.len(), pc, &mut cpu.gpr, &mut x, mem));
                [cpu.xmm[0], cpu.xmm[1]] = x;
                let esp = cpu.gpr[4];
                pc = u32::from_le_bytes(
                    mem.bytes[esp as usize..esp as usize + 4]
                        .try_into()
                        .unwrap(),
                );
                cpu.gpr[4] = esp + 4 + fun.cleanup(pc_target(&log));
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

fn pc_target(log: &[CallLog]) -> u32 {
    log.last().unwrap().target
}

enum Port {
    GameView(GameView),
    ScenePasses(ScenePasses),
}

fn port(
    fun: Fun,
    seed: u64,
    mem: &mut FlatMemory,
    mut r: Regs,
) -> Result<(Regs, Vec<CallLog>), String> {
    let mut log = Vec::new();
    let e = |e: MemFault| format!("{e:?}");
    let (mut m, mut step) = match fun {
        Fun::GameView => {
            let (m, s) = GameView::start(&mut r, &mut Eng(mem)).map_err(e)?;
            (Port::GameView(m), s)
        }
        Fun::ScenePasses => {
            let (m, s) = ScenePasses::start(&mut r, &mut Eng(mem)).map_err(e)?;
            (Port::ScenePasses(m), s)
        }
    };
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
                assert_eq!(cleanup, fun.cleanup(target));
                r.esp -= 4;
                Eng(mem).set_u32(r.esp, site).unwrap();
                let mut gpr = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];
                let mut x = [r.xmm0, r.xmm1];
                log.push(stub(fun, seed, log.len(), target, &mut gpr, &mut x, mem));
                [r.eax, r.ecx, r.edx, r.ebx, _, r.ebp, r.esi, r.edi] = gpr;
                [r.xmm0, r.xmm1] = x;
                r.esp += 4 + cleanup;
                step = match &mut m {
                    Port::GameView(m) => m.resume(&mut r, &mut Eng(mem)),
                    Port::ScenePasses(m) => m.resume(&mut r, &mut Eng(mem)),
                }
                .map_err(e)?;
            }
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes())
}

fn put8(mem: &mut FlatMemory, a: u32, v: u8) {
    mem.bytes[a as usize] = v;
}

/// Floats the compares care about: zero both ways, NaN, small and large.
fn float(r: &mut Rng) -> u32 {
    r.pick(&[
        0.0f32.to_bits(),
        (-0.0f32).to_bits(),
        f32::NAN.to_bits(),
        1.0f32.to_bits(),
        (-2.5f32).to_bits(),
        1.0e9f32.to_bits(),
        0.01f32.to_bits(),
    ])
}

/// The object tables both functions reach through `0x004e8c20`,
/// `0x004e8c24`, `0x004e0300` and `0x004e3b44`, indexed by small indices.
fn tables(mem: &mut FlatMemory, r: &mut Rng) {
    put(mem, 0x004e_8c20, T_OBJECTS);
    put(mem, 0x004e_8c24, T_DATA);
    put(mem, 0x004e_0300, T_300);
    put(mem, 0x004e_3b44, T_TAGS);
    put(mem, T_DATA + 0x44, B_DATA);
    put(mem, T_300 + 0x44, B_300);
    for i in 0..4 {
        put(mem, T_OBJECTS + 0xc + 4 * i, index(r));
        let rec = B_DATA + i * 0x21c;
        put(mem, rec + 0x24, r.u32());
        let v = if r.below(4) == 0 { u32::MAX } else { index(r) };
        put(mem, rec + 0x2c, v);
        let obj = OBJS + i * 0x400;
        put(mem, B_300 + i * 12 + 8, obj);
        put8(mem, obj + 0x212, r.u32() as u8);
        let v = if r.below(4) == 0 { u32::MAX } else { index(r) };
        put(mem, obj, v);
        let shader = SHADERS + i * 0x400;
        put(mem, T_TAGS + i * 16 + 8, shader);
        put(mem, shader + 0x314, float(r));
        put(mem, shader + 0x318, float(r));
    }
}

/// Random bytes under the stack, so the locals the callees would fill
/// (and the bytes the originals test) vary.
fn stack(mem: &mut FlatMemory, r: &mut Rng) -> u32 {
    for a in (STACK_TOP - 0x1000..STACK_TOP).step_by(8) {
        mem.bytes[a as usize..a as usize + 8].copy_from_slice(&r.next().to_le_bytes());
    }
    STACK_TOP - 0x400 - 4 * r.below(8)
}

fn regs(r: &mut Rng, esp: u32) -> Regs {
    Regs {
        eax: r.u32(),
        ecx: r.u32(),
        edx: r.u32(),
        ebx: r.u32(),
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: r.u32(),
        xmm0: (r.next() as u128) << 64 | r.next() as u128,
        xmm1: (r.next() as u128) << 64 | r.next() as u128,
    }
}

fn game_view_state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64);
    tables(&mut mem, &mut r);
    put(&mut mem, 0x0046_86d4, PAIR_SRC);
    put(&mut mem, 0x0046_86cc, OBSERVER);
    for i in 0..4 {
        put(&mut mem, PAIR_SRC + 4 * i, r.u32());
        put(&mut mem, OBSERVER + 4 + 4 * i, r.u32());
        put(&mut mem, 0x004b_a054 + 4 * i, r.u32());
        put(&mut mem, 0x004b_a064 + 4 * i, r.u32());
    }
    for w in 0..0x118 / 4 {
        put(&mut mem, VIEW + 4 * w, r.u32());
    }
    put(&mut mem, VIEW + 4, r.below(4));
    let obj = if r.below(3) == 0 {
        u32::MAX
    } else {
        r.below(4)
    };
    put(&mut mem, VIEW + 8, obj);
    for f in [0x34, 0x4c, 0x50] {
        put(&mut mem, VIEW + f, float(&mut r));
    }
    let esp = stack(&mut mem, &mut r);
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, VIEW);
    put(&mut mem, esp + 8, r.u32());
    let regs = regs(&mut r, esp);
    (mem, regs)
}

fn scene_state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ case as u64);
    tables(&mut mem, &mut r);
    put(&mut mem, 0x004b_a03c, r.u32());
    put(&mut mem, 0x004c_1a50, r.u32());
    put8(&mut mem, 0x0048_5a75, (r.below(4) == 0) as u8);
    put8(&mut mem, 0x0048_5a76, (r.below(4) == 0) as u8);
    let kind = r.pick(&[0, 3, 4, 5, 7, 8, 0xffff_fffe]);
    put(&mut mem, 0x0048_5898, kind);
    put(&mut mem, 0x0048_58b8, r.u32());
    put8(&mut mem, 0x004b_a022, r.pick(&[0, 1, 0x80]) as u8);
    put8(&mut mem, 0x0048_575c, r.below(2) as u8);
    put8(&mut mem, 0x004b_9fe0, r.u32() as u8);
    put8(&mut mem, 0x004b_9f88, r.below(2) as u8);
    put(
        &mut mem,
        0x004c_19ac,
        r.pick(&[0, 1, 5, 0xffff_ffff, 0x8000_0000]),
    );
    let player = if r.below(4) == 0 { u32::MAX } else { r.u32() };
    put(&mut mem, 0x004b_9ed8, player);
    let esp = stack(&mut mem, &mut r);
    put(&mut mem, esp, HARNESS_EXIT);
    // (view kind, transparents, flag, last layer, view, mode)
    put(&mut mem, esp + 4, r.pick(&[0, 1, 2, 3, 5]));
    put(&mut mem, esp + 8, r.pick_or_any(&[0, 1, 0x100]));
    put(&mut mem, esp + 0xc, r.pick_or_any(&[0, 1]));
    put(&mut mem, esp + 0x10, r.pick_or_any(&[0, 1]));
    put(&mut mem, esp + 0x14, r.u32());
    put(&mut mem, esp + 0x18, r.pick(&[0, 1, 2, 3]));
    let regs = regs(&mut r, esp);
    (mem, regs)
}

fn check(fun: Fun, cases: u32) {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED {fun:?}: no XBE (set H2_XBE)");
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
    let (mut states, mut calls) = (0, 0);
    let mut targets: BTreeMap<u32, u64> = BTreeMap::new();
    for &seed in &seeds {
        for case in 0..cases {
            let (mut a, regs) = match fun {
                Fun::GameView => game_view_state(&image, seed, case),
                Fun::ScenePasses => scene_state(&image, seed, case),
            };
            let mut b = a.clone();
            let (ra, la) = lifted.run(fun, seed, &mut a, regs).unwrap();
            let (rb, lb) = port(fun, seed, &mut b, regs).unwrap();
            for (i, (x, y)) in la.iter().zip(&lb).enumerate() {
                assert_eq!(x, y, "{fun:?} seed {seed} case {case}: call {i}");
            }
            assert_eq!(
                la.len(),
                lb.len(),
                "{fun:?} seed {seed} case {case}: call count"
            );
            assert_eq!(
                ra, rb,
                "{fun:?} seed {seed} case {case}: registers at return"
            );
            // memcmp first: the byte search is slow without optimisation.
            if a.bytes != b.bytes
                && let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y)
            {
                panic!(
                    "{fun:?} seed {seed} case {case}: memory differs first at {i:#x}: {:#04x} vs {:#04x}",
                    a.bytes[i], b.bytes[i]
                );
            }
            states += 1;
            calls += la.len();
            for c in &la {
                *targets.entry(c.target).or_default() += 1;
            }
        }
    }
    eprintln!(
        "{fun:?}: {states} states, {calls} calls equal, registers and all 64 MB equal; calls by target: {targets:#x?}"
    );
    for t in fun.callees() {
        assert!(
            targets.contains_key(t),
            "{t:#x} never called: widen the states"
        );
    }
}

#[test]
fn render_game_view_matches_the_lifted_original() {
    check(Fun::GameView, 128);
}

#[test]
fn render_scene_passes_matches_the_lifted_original() {
    check(Fun::ScenePasses, 512);
}
