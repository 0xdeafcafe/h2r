//! `engine::colour_pass` against the original, lifted and run on the IR
//! interpreter from the same seeded 64 MB state:
//!
//! - `arguments`: `render_screen_effects` from `0x000138b3` (the colour
//!   byte set) to the call of `0x000214f0` (`0x000139b5`) or the skip
//!   (`0x000139c5`); the seven words pushed must be the port's, and the
//!   skip its `None`. `0x000137a0` runs lifted inside.
//! - `parameters`: `0x000214f0` from its entry to the draw
//!   (`0x000216fb`) or the early return (`0x00021a73`); the arguments and
//!   the locals `x` and `y` must be the port's, and memory (the override
//!   record's reset) byte for byte as `Parameters::write_reset` leaves it.
//! - `pack_colour`: `0x00131e50` to its return; EAX must be the port's.
//!
//! Each case seeds the view time, the curves record and its set byte, the
//! screen record's fade, the blend weight and targets, the view kind, the
//! override gate and record (fields unset or not, a pending reset), the
//! arguments and the four lanes, with any float bits now and then (zeros,
//! infinities, NaNs, denormals), under one of several MXCSR and x87
//! control words. The stack below the entry is not compared.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it. `H2_DIFF_SEEDS=1,2,3`
//! picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it colour_pass_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::colour_pass::{self as cp, constant, global};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 100_000;

const RENDER_SCREEN_EFFECTS: u32 = 0x0001_3870;
const ARGUMENTS_START: u32 = 0x0001_38b3;
const ARGUMENTS_EXITS: [u32; 3] = [0x0001_39b5, 0x0001_39bd, 0x0001_39c5];
const COLOUR_PASS: u32 = 0x0002_14f0;
const PARAMETERS_EXITS: [u32; 2] = [0x0002_16fb, 0x0002_1a73];
const PACK: u32 = 0x0013_1e50;

const CURVES: u32 = 0x0305_0000;
const SCREEN: u32 = 0x0306_0000;
const GATE: u32 = 0x0307_0000;
const RECORD: u32 = 0x0308_0000;
const LANES: u32 = 0x0309_0000;

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
    /// A float in 0..2 mostly, and now and then an edge.
    fn float(&mut self) -> u32 {
        match self.below(14) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000,
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => 0x3f80_0000,
            6 => (-(self.below(1000) as f32) / 500.0).to_bits(),
            _ => (self.below(1 << 20) as f32 / (1 << 19) as f32).to_bits(),
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

fn mxcsr(r: &mut Rng) -> u32 {
    match r.below(5) {
        0 => 0x1f80 | 0x0040,
        1 => 0x1f80 | 0x8000,
        2 => 0x1f80 | 0x6000,
        _ => 0x1f80,
    }
}

fn x87_control(r: &mut Rng) -> u16 {
    [0x027f, 0x037f, 0x007f, 0x067f, 0x0a7f, 0x0e7f][r.below(6) as usize]
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

    /// Steps from `cpu.eip` until one of `exits`; returns the exit.
    fn run(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory, exits: &[u32]) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
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

/// Runs `f` with the image and a lifter over it; `false` without the XBE.
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

fn cpu(r: &mut Rng, esp: u32, esi: u32, eip: u32, mx: u32, cw: u16) -> Cpu {
    let mut c = Cpu {
        gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), esi, r.u32()],
        fs_base: RAM as u32,
        eip,
        ..Cpu::default()
    };
    c.mxcsr = mx;
    c.x87.control = cw;
    c
}

#[test]
fn arguments_match_the_lifted_original() {
    if !with_image(arguments_cases) {
        eprintln!("SKIPPED arguments_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn arguments_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(RENDER_SCREEN_EFFECTS);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut a = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            // The time, near the keys (0..2) as an f64, now and then odd.
            let t = match r.below(8) {
                0 => f64::NAN,
                1 => -1.0,
                _ => r.below(1 << 20) as f64 / (1 << 19) as f64,
            };
            put(&mut a, global::TIME, t.to_bits() as u32);
            put(&mut a, global::TIME + 4, (t.to_bits() >> 32) as u32);
            put(&mut a, global::CURVES, CURVES);
            a.bytes[CURVES as usize] = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            for off in (4..0x34).step_by(4) {
                let v = r.float();
                put(&mut a, CURVES + off, v);
            }
            // Ordered keys most of the time.
            for k in [4u32, 0x1c] {
                if r.below(3) != 0 {
                    let (lo, hi) = (r.below(1 << 19) as f32 / (1 << 19) as f32, 1.0 + r.below(4) as f32 / 4.0);
                    put(&mut a, CURVES + k, lo.to_bits());
                    put(&mut a, CURVES + k + 4, hi.to_bits());
                }
            }
            let screen = if r.below(3) == 0 { 0 } else { SCREEN };
            put(&mut a, global::SCREEN_RECORD, screen);
            let v = r.float();
            put(&mut a, SCREEN + 0x10, v);
            let w = if r.below(2) == 0 { 0 } else { r.float() };
            put(&mut a, global::BLEND, w);
            for off in [4, 8] {
                let v = r.float();
                put(&mut a, global::BLEND + off, v);
            }
            let kind = if r.below(3) == 0 { 6 } else { r.u32() };
            put(&mut a, global::VIEW_KIND, kind);
            let (mx, cw) = (mxcsr(&mut r), x87_control(&mut r));

            let want = cp::arguments(&Eng(&mut a.clone()), mx, cw)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));

            let esp = STACK_TOP - 0x400;
            // The colour byte, with junk above it.
            put(&mut a, esp + 0x6c, (r.u32() & !0xff) | 1);
            let mut c = cpu(&mut r, esp, screen, ARGUMENTS_START, mx, cw);
            let exit = lifted
                .run(&mut c, &mut a, &ARGUMENTS_EXITS)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            match want {
                None => assert_eq!(exit, ARGUMENTS_EXITS[2], "seed {seed} case {n}: drawn by the original"),
                Some(args) => {
                    assert_eq!(exit, ARGUMENTS_EXITS[0], "seed {seed} case {n}: skipped by the original");
                    let pushed: Vec<u32> = (0..7).map(|i| word(&a, c.gpr[4] + 4 * i)).collect();
                    assert_eq!(pushed, args.to_vec(), "seed {seed} case {n}");
                }
            }
            *seen
                .entry(match want {
                    None => "skipped",
                    Some(a) if a[6] & 0xff == 1 => "drawn, kind 6",
                    Some(_) => "drawn",
                })
                .or_default() += 1;
        }
    }
    eprintln!("arguments: all equal; {seen:#?}");
    for k in ["skipped", "drawn", "drawn, kind 6"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

#[test]
fn parameters_match_the_lifted_original() {
    if !with_image(parameters_cases) {
        eprintln!("SKIPPED parameters_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn parameters_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(COLOUR_PASS);
    let unset = word(image, constant::UNSET);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut a = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ n as u64);
            let gate = if r.below(4) == 0 { 0 } else { GATE };
            put(&mut a, global::OVERRIDE_GATE, gate);
            a.bytes[(GATE + 5) as usize] = if r.below(4) == 0 { 0 } else { r.u32() as u8 | 1 };
            put(&mut a, global::OVERRIDE, RECORD);
            a.bytes[RECORD as usize] = if r.below(3) == 0 { 0 } else { r.u32() as u8 | 1 };
            a.bytes[RECORD as usize + 1] = if r.below(4) == 0 { r.u32() as u8 | 1 } else { 0 };
            for off in [4u32, 8, 0xc, 0x10, 0x14, 0x1c, 0x20] {
                let v = if r.below(2) == 0 { unset } else { r.float() };
                put(&mut a, RECORD + off, v);
            }
            for off in [0x18u32, 0x19, 0x1a] {
                a.bytes[(RECORD + off) as usize] = r.u32() as u8;
            }
            let args: [u32; 7] = [r.float(), r.float(), r.float(), r.float(), r.float(), r.u32(), r.u32()];
            let mx = mxcsr(&mut r);

            let mut b = a.clone();
            let p = cp::parameters(&Eng(&mut b), args, mx).unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            p.write_reset(&mut Eng(&mut b)).unwrap();

            let esp = STACK_TOP - 0x400;
            put(&mut a, esp, HARNESS_EXIT);
            for (i, v) in args.iter().enumerate() {
                put(&mut a, esp + 4 + 4 * i as u32, *v);
            }
            let esi = r.u32();
            let mut c = cpu(&mut r, esp, esi, COLOUR_PASS, mx, 0x027f);
            let exit = lifted
                .run(&mut c, &mut a, &PARAMETERS_EXITS)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(exit == PARAMETERS_EXITS[0], p.draws, "seed {seed} case {n}: exit {exit:#x}");
            // At either exit ESP is the entry's less 0x24 (the locals and
            // EBX).
            let base = esp - 0x24;
            let got_args: Vec<u32> = (0..7).map(|i| word(&a, base + 0x28 + 4 * i)).collect();
            assert_eq!(got_args, p.args.to_vec(), "seed {seed} case {n}: arguments");
            assert_eq!(word(&a, base + 4), p.x, "seed {seed} case {n}: x");
            assert_eq!(word(&a, base + 8), p.y, "seed {seed} case {n}: y");
            let low = (STACK_TOP - 0x1000) as usize;
            a.bytes[low..STACK_TOP as usize].fill(0);
            b.bytes[low..STACK_TOP as usize].fill(0);
            if let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {n}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)",
                    a.bytes[i], b.bytes[i]
                );
            }
            *seen
                .entry(match (p.reset.is_some(), p.draws) {
                    (true, _) => "reset",
                    (false, true) => "drawn",
                    (false, false) => "not drawn",
                })
                .or_default() += 1;
        }
    }
    eprintln!("parameters: all equal; {seen:#?}");
    for k in ["reset", "drawn", "not drawn"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

#[test]
fn pack_colour_matches_the_lifted_original() {
    if !with_image(pack_cases) {
        eprintln!("SKIPPED pack_colour_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn pack_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let scale = word(image, constant::PACK_SCALE);
    for seed in seeds() {
        for n in 0..256u32 {
            let mut a = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x94d0_49bb_1331_11eb) ^ n as u64);
            let lanes = [r.float(), r.float(), r.float(), r.float()];
            for (i, v) in lanes.iter().enumerate() {
                put(&mut a, LANES + 4 * i as u32, *v);
            }
            let cw = x87_control(&mut r);
            let want = cp::pack_colour(lanes, scale, cw);
            let esp = STACK_TOP - 0x400;
            put(&mut a, esp, HARNESS_EXIT);
            put(&mut a, esp + 4, LANES);
            let mut c = cpu(&mut r, esp, 0, PACK, 0x1f80, cw);
            lifted
                .run(&mut c, &mut a, &[HARNESS_EXIT])
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            match want {
                Ok(v) => assert_eq!(c.gpr[0], v, "seed {seed} case {n}: lanes {lanes:#x?} control {cw:#x}"),
                Err(e) => panic!("seed {seed} case {n}: the port stopped ({e:?}) where the original returned"),
            }
        }
    }
    eprintln!("pack_colour: all equal");
}
