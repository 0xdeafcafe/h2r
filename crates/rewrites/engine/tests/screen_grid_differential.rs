//! `engine::screen_grid` against the original, lifted and run on the IR
//! interpreter from the same seeded 64 MB state: `0x00034d90` from its
//! entry to its return, `0x0001dd60`, the grid `0x00034770`, the attribute
//! function `0x00034100` and `0x000350e0` lifted inside; the render-state
//! calls stubbed (`0x00014bc0`, `0x000142f0`, `0x003f5cf0`, `0x003f54f0`,
//! `0x003f59b0`, `0x003f53d0`, `0x003f5590`, `0x0001c590`, `0x00016b10`,
//! `0x0001cf50`, `0x0001c710`, the push buffer's flush and reserve, and
//! `0x003fa680`). Every word it pushes must be `screen_grid::pushed` of
//! the port's strips, over random targets, rectangles, tap floats, radii
//! (equal and apart), depths and bound sizes, under several MXCSR and x87
//! control words. Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`),
//! pinned by hash; skips without it. Nothing is copied from it.
//! `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it screen_grid_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::screen_grid::{self as sg, Grid};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 20_000_000;

const QUAD: u32 = 0x0003_4d90;
const PUSH: u32 = 0x0040_7488;
const DIRTY: u32 = 0x0040_7490;
const VIEW_TARGET: u32 = 0x0048_58b8;
const TARGETS: u32 = 0x004b_4b58;
const TARGET_STRIDE: u32 = 0x98;
const PB: u32 = 0x0300_0000;
const BUF: u32 = 0x0310_0000;
const TAPS: u32 = 0x0300_0100;

struct Eng<'a>(&'a FlatMemory);

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
    fn write(&mut self, addr: u32, _data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault { addr, len: 0, write: true })
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
    /// A float in -top..top mostly, and now and then an edge.
    fn float(&mut self, top: f32) -> u32 {
        match self.below(16) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x3f80_0000,
            3 => 0x7fc0_0000,
            4 => 0x7f80_0000,
            5 => self.u32() & 0x807f_ffff,
            _ => ((self.below(1 << 20) as f32 / (1 << 20) as f32 * 2.0 - 1.0) * top).to_bits(),
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(mem: &mut FlatMemory, a: u32, v: u16) {
    mem.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

/// A call stubbed on the original's side: its stack cleanup.
type Stubs = BTreeMap<u32, u32>;

impl Lifted<'_> {
    fn lift(&mut self, entry: u32) {
        if let Some(f) = self.lifter.lift(entry) {
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.entry(insn.pc).or_insert(insn);
            }
        }
    }

    fn run(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory, exits: &[u32], stubs: &Stubs) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(format!("step limit ({STEP_LIMIT} instructions) at {pc:#x}"));
            }
            if let Some(&cleanup) = stubs.get(&pc) {
                let esp = cpu.gpr[4];
                pc = word(mem, esp);
                cpu.gpr[4] = esp + 4 + cleanup;
                cpu.eip = pc;
                continue;
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        Ok(pc)
    }
}

fn with_image(f: impl FnOnce(&FlatMemory, Lifted<'_>)) -> bool {
    let Some(path) = xbe_path() else {
        return false;
    };
    let data = std::fs::read(&path).unwrap();
    assert_eq!(sha256(&data), IMAGE_SHA256, "the retail XBE only");
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    f(
        &image,
        Lifted {
            lifter: &lifter,
            cache: BTreeMap::new(),
        },
    );
    true
}

fn seeds() -> Vec<u64> {
    std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99])
}

/// `0x0001dd60` of `t`.
fn target_size(m: &FlatMemory, t: u32) -> (i32, i32) {
    let e = TARGETS.wrapping_add(t.wrapping_mul(TARGET_STRIDE));
    if word(m, e + 0x88) != 0 && m.bytes[(e + 0x95) as usize] == 0 {
        (word(m, e + 0x80) as i32, word(m, e + 0x84) as i32)
    } else if [0, 3, 0x18].contains(&t) {
        (0x280, 0x1e0)
    } else {
        (0, 0)
    }
}

#[test]
fn the_grid_matches_the_lifted_original() {
    if !with_image(grid_cases) {
        eprintln!("SKIPPED the_grid_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn grid_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(QUAD);
    let stubs: Stubs = [
        (0x0001_4bc0, 8),
        (0x0001_42f0, 0),
        (0x003f_5cf0, 0),
        (0x003f_54f0, 4),
        (0x003f_59b0, 4),
        (0x003f_53d0, 4),
        (0x003f_5590, 4),
        (0x0001_c590, 4),
        (0x0001_6b10, 0),
        (0x0001_cf50, 0),
        (0x0001_c710, 4),
        (0x0040_0fa0, 0),
        (0x003f_aae0, 4),
        (0x003f_a680, 4),
    ]
    .into_iter()
    .collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..32u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            let mx = [0x1f80, 0x1f80 | 0x8040, 0x1f80 | 0x6000][r.below(3) as usize];
            let cw: u16 = [0x027f, 0x037f, 0x007f, 0x0e7f][r.below(4) as usize];
            for t in [0u32, 3, 0x19, 0x1a] {
                let e = TARGETS + t * TARGET_STRIDE;
                put(&mut m, e + 0x88, if r.below(5) == 0 { 0 } else { 0x0100_0000 });
                m.bytes[(e + 0x95) as usize] = u8::from(r.below(8) == 0);
                put(&mut m, e + 0x80, [640u32, 320, 160, 1][r.below(4) as usize]);
                put(&mut m, e + 0x84, [480u32, 240, 120, 1][r.below(4) as usize]);
            }
            put(&mut m, VIEW_TARGET, [0u32, 3][r.below(2) as usize]);
            let target = [0x19u32, 0x1a, u32::MAX][r.below(3) as usize];
            let window: [u16; 4] = if r.below(3) == 0 { [10, 20, 401, 603] } else { [0, 0, 480, 640] };
            for (o, v) in window.iter().enumerate() {
                put16(&mut m, sg::WINDOW + 2 * o as u32, *v);
            }
            for s in 0..4u32 {
                put(&mut m, sg::constant::BOUND_WIDTH + 4 * s, [640u32, 320, 160, 0][r.below(4) as usize]);
                put(&mut m, sg::constant::BOUND_HEIGHT + 4 * s, [480u32, 240, 120, 0][r.below(4) as usize]);
            }
            put(&mut m, sg::constant::NEAR, (r.below(100) as f32 / 400.0 + 0.01).to_bits());
            put(&mut m, sg::constant::FAR, (r.below(1000) as f32 + 10.0).to_bits());
            let taps = [r.float(1.5), r.float(1.5), r.float(1.5), r.float(1.5)];
            for (k, v) in taps.iter().enumerate() {
                put(&mut m, TAPS + 4 * k as u32, *v);
            }
            let radii = match r.below(4) {
                0 => [0, 0],
                1 => {
                    let v = r.float(1.0);
                    [v, v]
                }
                _ => [r.float(1.0), r.float(1.5)],
            };
            let depth = [0x3f80_0000, r.float(4.0)][r.below(2) as usize];
            let full = (r.u32() & !0xff) | r.below(2);
            put(&mut m, PUSH, PB);
            put(&mut m, PB, BUF);
            put(&mut m, PB + 4, BUF + 0x0080_0000);
            put(&mut m, PB + 8, 0);
            put(&mut m, DIRTY, 0);

            let size = target_size(&m, if target == u32::MAX { word(&m, VIEW_TARGET) } else { target });
            let grid = Grid {
                size,
                bound: Grid::bound(&Eng(&m)).unwrap(),
                taps,
                depth,
                radii,
                full: full & 0xff != 0,
            };
            let want = sg::strips(&Eng(&m), &grid, mx, cw).map(|s| sg::pushed(&s));

            let esp = STACK_TOP - 0x400;
            let stack = [HARNESS_EXIT, 0xa, 0x0101_0101, 0, 0, depth, radii[0], radii[1], full, 0];
            for (i, v) in stack.iter().enumerate() {
                put(&mut m, esp + 4 * i as u32, *v);
            }
            let mut c = Cpu {
                gpr: [target, r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), TAPS],
                fs_base: RAM as u32,
                eip: QUAD,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            lifted
                .run(&mut c, &mut m, &[HARNESS_EXIT], &stubs)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(c.gpr[4], esp + 4 + 0x24, "seed {seed} case {n}: esp");
            let got: Vec<u32> = (BUF..word(&m, PB)).step_by(4).map(|a| word(&m, a)).collect();
            let want = want.unwrap_or_else(|e| panic!("seed {seed} case {n}: the port {e:?}"));
            if got != want {
                let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
                panic!(
                    "seed {seed} case {n}: {} words, the port {}; first difference at word {at}: original {:#x?}, port {:#x?}\n{grid:#x?}",
                    got.len(),
                    want.len(),
                    &got[at.saturating_sub(4)..(at + 4).min(got.len())],
                    &want[at.saturating_sub(4)..(at + 4).min(want.len())],
                );
            }
            let radial = radii[0] != radii[1];
            *seen.entry(format!("radial {radial}")).or_default() += 1;
            *seen.entry(format!("full {}", full & 0xff != 0)).or_default() += 1;
        }
    }
    eprintln!("grid: all equal; {seen:#?}");
    for k in ["radial true", "radial false", "full true", "full false"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
