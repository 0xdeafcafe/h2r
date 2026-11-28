//! `engine::ocean_grid` against the original, lifted and run on the IR
//! interpreter from the same seeded 64 MB state: `0x0003ca90` from its
//! entry to its return, with `0x0003c9a0` and `0x00030bf0` lifted inside.
//! The camera's matrix, the grid's rows, the visibility quad and EAX, ECX
//! and EDX must match the port's word for word, over random cameras,
//! bases (now and then degenerate), water definitions, raster
//! rectangles, fields of view, lens bytes and lens records, with any
//! float bits now and then (zeros, infinities, NaNs, denormals), under
//! several MXCSR and x87 control words. The port's lens is 1 (the
//! original runs without the widening site hook).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it. `H2_DIFF_SEEDS=1,2,3`
//! picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it ocean_grid_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::ocean_grid::{self as og, Grid};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 100_000;

const SETUP: u32 = 0x0003_ca90;
const DEF: u32 = 0x0300_0000;
const UP: u32 = 0x0300_1000;
const STATE: u32 = 0x0300_2000;

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
    /// Uniform in `(lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
        lo + unit * (hi - lo)
    }
    /// `lo..hi` mostly, and now and then an edge.
    fn float(&mut self, lo: f32, hi: f32) -> u32 {
        match self.below(48) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000 | (self.u32() & 0x8000_0000),
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            _ => self.range(lo, hi).to_bits(),
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

fn words(mem: &FlatMemory, at: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| word(mem, at + 4 * i)).collect()
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    fn lift(&mut self, entry: u32) {
        if let Some(f) = self.lifter.lift(entry) {
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.entry(insn.pc).or_insert(insn);
            }
        }
    }

    fn run(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory) -> Result<(), String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(format!("step limit ({STEP_LIMIT} instructions) at {pc:#x}"));
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        Ok(())
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

/// One seeded state.
fn seed(image: &FlatMemory, r: &mut Rng) -> FlatMemory {
    let mut m = image.clone();
    for i in 0..3 {
        put(&mut m, og::CAMERA + 4 * i, r.float(-50.0, 50.0));
    }
    put(&mut m, og::FOV, r.float(0.3, 2.4));
    // The basis: forward, and up through its pointer; sometimes parallel.
    let up: [u32; 3] = match r.below(4) {
        0 => [0, 0, 1.0f32.to_bits()],
        _ => [r.float(-1.0, 1.0), r.float(-1.0, 1.0), r.float(-1.0, 1.0)],
    };
    for (i, v) in up.iter().enumerate() {
        put(&mut m, UP + 4 * i as u32, *v);
    }
    put(&mut m, og::BASIS_UP, UP);
    let forward = if r.below(12) == 0 { up } else { [r.float(-1.0, 1.0), r.float(-1.0, 1.0), r.float(-0.5, 0.5)] };
    for (i, v) in forward.iter().enumerate() {
        put(&mut m, og::BASIS_FORWARD + 4 * i as u32, *v);
    }
    // The raster rectangle.
    let raster: [u16; 4] = match r.below(4) {
        0 => [0, 0, 480, 640],
        1 => [24, 32, 456, 608],
        2 => [r.below(480) as u16, r.below(640) as u16, r.below(480) as u16, r.below(640) as u16],
        _ => [0, 0, 240, 640],
    };
    for (o, v) in raster.iter().enumerate() {
        put16(&mut m, og::RASTER + 2 * o as u32, *v);
    }
    m.bytes[og::WIDESCREEN as usize] = u8::from(r.below(2) == 0);
    // The lens record: none, keeping retail, or each way of not.
    let record = match r.below(6) {
        0 => 0,
        _ => {
            m.bytes[(STATE + 1) as usize] = u8::from(r.below(4) != 0);
            put16(&mut m, STATE + 2, if r.below(4) == 0 { 0xffff } else { r.below(0x100) as u16 });
            put(&mut m, STATE + 8, [3, 3, 0, 2][r.below(4) as usize]);
            STATE
        }
    };
    put(&mut m, og::LENS_STATE, record);
    // The definition.
    let angle = match r.below(10) {
        0 => 0,
        1 => 0x8000_0000,
        _ => r.float(0.05, 2.5),
    };
    put(&mut m, DEF + og::ANGLE, angle);
    put(&mut m, DEF + og::WIDTH, r.float(0.05, 4.0));
    put(&mut m, DEF + og::LEVEL, r.float(-50.0, 50.0));
    put(&mut m, DEF + og::FAR, r.float(-4.0, 4.0));
    put(&mut m, DEF + og::DEPTH, r.float(-1.0, 1.0));
    m
}

#[test]
fn the_grid_matches_the_lifted_original() {
    if !with_image(grid_cases) {
        eprintln!("SKIPPED the_grid_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn grid_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(SETUP);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for s in seeds() {
        for n in 0..64u32 {
            let mut r = Rng(s.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mx = [0x1f80, 0x1f80 | 0x8040, 0x1f80 | 0x6000, 0x1f80 | 0x2000][r.below(4) as usize];
            let cw: u16 = [0x027f, 0x037f, 0x007f][r.below(3) as usize];
            let seeded = seed(image, &mut r);
            let (ecx, edx) = (r.u32(), r.u32());

            let mut port = seeded.clone();
            let mut port_mx = mx;
            let out = og::grid(&mut Eng(&mut port), DEF, &mut port_mx, cw, 1.0)
                .unwrap_or_else(|e| panic!("seed {s} case {n}: the port {e:?}"));

            let mut m = seeded;
            let esp = STACK_TOP - 0x400;
            put(&mut m, esp, HARNESS_EXIT);
            let mut c = Cpu {
                gpr: [DEF, ecx, edx, r.u32(), esp, r.u32(), r.u32(), r.u32()],
                fs_base: RAM as u32,
                eip: SETUP,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            let saved = (c.gpr[3], c.gpr[5], c.gpr[6], c.gpr[7]);
            lifted.run(&mut c, &mut m).unwrap_or_else(|e| panic!("seed {s} case {n}: {e}"));
            assert_eq!(c.gpr[4], esp + 4, "seed {s} case {n}: esp");
            assert_eq!((c.gpr[3], c.gpr[5], c.gpr[6], c.gpr[7]), saved, "seed {s} case {n}: callee-saved");
            assert_eq!(c.x87.status & 0x3800, 0, "seed {s} case {n}: the original's x87 stack");

            let (eax, ecx, edx) = match out {
                Grid::Skipped => (DEF, ecx, edx),
                Grid::Built { eax, edx, .. } => (eax, 0, edx),
            };
            assert_eq!((c.gpr[0], c.gpr[1], c.gpr[2]), (eax, ecx, edx), "seed {s} case {n}: registers");
            assert_eq!(c.mxcsr, port_mx, "seed {s} case {n}: MXCSR");
            for (name, at, len) in [("matrix", og::MATRIX, 13), ("rows", og::ROWS, 12), ("quad", og::QUAD, 12)] {
                let (got, want) = (words(&m, at, len), words(&port, at, len));
                assert_eq!(got, want, "seed {s} case {n}: the {name}, original then port");
            }
            // The original's locals and return word below the entry ESP.
            let (lo, hi) = ((esp - 0x60) as usize, (esp + 4) as usize);
            port.bytes[lo..hi].copy_from_slice(&m.bytes[lo..hi]);
            assert!(m.bytes == port.bytes, "seed {s} case {n}: memory outside the stack");
            *seen.entry(if out == Grid::Skipped { "skipped" } else { "built" }).or_default() += 1;
            if word(&m, og::MATRIX + 4) == 0 && word(&m, og::MATRIX + 8) == 0 {
                *seen.entry("zero basis").or_default() += 1;
            }
            if word(&m, og::ROWS + 0x10) != 0 && f32::from_bits(word(&m, og::ROWS + 0x14)).is_finite() {
                *seen.entry("finite").or_default() += 1;
            }
        }
    }
    eprintln!("ocean grid: all equal; {seen:#?}");
    for k in ["skipped", "built", "zero basis", "finite"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
