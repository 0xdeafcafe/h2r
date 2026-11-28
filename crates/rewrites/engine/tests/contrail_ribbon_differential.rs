//! `engine::contrail_ribbon` against the original `0x00042cf0`, lifted and
//! run on the IR interpreter from the same seeded state, from its entry
//! to its return: ESP, AL, the vertices and every byte of the seeded
//! records must be the port's (the model's writes applied). Its callees
//! (`0x000bad50`, `0x00042c50`, `0x0017ca10`, the CRT `fmod`) run lifted
//! inside.
//!
//! Each case seeds a contrail and its definition (the texture step and
//! width, the facing, the end flags, stages with lerped colours and
//! widths), a list of up to seven points in the data array with stages in
//! and out of range and blends, an object with function sources or none,
//! the view, the fade mode, and the stack's old bytes where the normal
//! lives, with odd float bits now and then, under several MXCSR and x87
//! control words (exceptions masked).
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it contrail_ribbon_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

use std::collections::BTreeMap;

mod common;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::contrail_ribbon::{self as cr, ENTRY, NORMAL_DEPTH, POP, constant};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 200_000;
/// `_getptd` (`0x00326079`) reads the thread block through FS, which the
/// harness leaves outside RAM: the CRT's `fmod` error paths (infinities,
/// NaNs) call it, so it is stubbed to hand back a scratch zeroed block,
/// as the screen-effects differential does.
const GETPTD: u32 = 0x0032_6079;
const PTD: u32 = 0x03d0_0000;

const CTX: u32 = 0x0300_0000;
const C: u32 = 0x0301_0000;
const DEF: u32 = 0x0302_0000;
const STAGES: u32 = 0x0303_0000;
const ARRAY: u32 = 0x0304_0000;
const DATA: u32 = 0x0304_1000;
const OUT: u32 = 0x0305_1000;
const MODE: u32 = 0x0306_0000;
const OBJ_ARRAY: u32 = 0x0307_0000;
const OBJ_DATA: u32 = 0x0307_1000;
const DATUM: u32 = 0x0307_2000;
const TAG_ARRAY: u32 = 0x0307_4000;
const TAG: u32 = 0x0307_5000;
const BLOCK: u32 = 0x0307_6000;
const VECTORS: u32 = 0x0307_7000;
/// The seeded records: each compared whole after the run.
const REGIONS: [(u32, u32); 9] = [
    (CTX, 0x20),
    (C, 0x60),
    (DEF, 0x100),
    (STAGES, 0x180),
    (DATA, 0x38 * 8),
    (OUT - 0x1000, 0x2000),
    (MODE, 0x40),
    (DATUM, 0x400),
    (VECTORS, 0x20),
];

struct Ro<'a>(&'a FlatMemory);

impl GuestMemory for Ro<'_> {
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
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
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
    fn chance(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
    /// A float in -1..2 mostly, and now and then an edge.
    fn float(&mut self) -> u32 {
        match self.below(32) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000,
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => 0x3f80_0000,
            _ => ((self.below(3000) as f32 - 1000.0) / 1000.0).to_bits(),
        }
    }
    /// A float in 0..1.
    fn unit(&mut self) -> u32 {
        (self.below(1 << 16) as f32 / (1 << 16) as f32).to_bits()
    }
    /// A coordinate: -50..50 mostly.
    fn coordinate(&mut self) -> u32 {
        if self.chance(16) {
            self.float()
        } else {
            ((self.below(100_000) as f32 - 50_000.0) / 1000.0).to_bits()
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(mem: &mut FlatMemory, a: u32, v: u16) {
    mem.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
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

    fn run(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory, exits: &[u32]) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
            }
            if pc == GETPTD {
                let esp = cpu.gpr[4];
                let ret = mem.bytes[esp as usize..esp as usize + 4].try_into().unwrap();
                pc = u32::from_le_bytes(ret);
                cpu.gpr[0] = PTD;
                cpu.gpr[4] = esp + 4;
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

/// What a case was seeded with, for the paths seen.
struct Seeded {
    points: u32,
    facing: i16,
    tinted: bool,
    blended: bool,
    faded: bool,
}

fn case(image: &FlatMemory, r: &mut Rng, esp: u32) -> (FlatMemory, Seeded) {
    let mut a = image.clone();
    // The globals.
    put(&mut a, constant::POINTS, ARRAY);
    put(&mut a, ARRAY + 0x44, DATA);
    put(&mut a, constant::OBJECTS, OBJ_ARRAY);
    put(&mut a, OBJ_ARRAY + 0x44, OBJ_DATA);
    put(&mut a, constant::TAGS, TAG_ARRAY);
    put(&mut a, constant::WHITE, VECTORS);
    put(&mut a, constant::UP, VECTORS + 0x10);
    for o in (0..0x20).step_by(4) {
        let v = if r.chance(4) { r.float() } else { r.unit() };
        put(&mut a, VECTORS + o, v);
    }
    for i in 0..3 {
        let v = r.coordinate();
        put(&mut a, constant::CAMERA + 4 * i, v);
    }
    // The stack's old bytes where the normal lives.
    for o in (0..0x10).step_by(4) {
        let v = r.float();
        put(&mut a, esp - NORMAL_DEPTH + o, v);
    }

    // The draw's context.
    let points = [0, 1, 2, 3, 4, 5, 7][r.below(7) as usize];
    let slot = r.below(4);
    put(&mut a, CTX, C);
    put(&mut a, CTX + 4, DEF);
    put16(&mut a, CTX + 8, slot as u16);
    put(&mut a, CTX + 0xc, MODE);
    a.bytes[(CTX + 0x10) as usize] = if r.chance(2) { 0 } else { r.u32() as u8 | 1 };
    let count = if r.chance(6) { r.below(2 * points + 3) } else { 2 * points };
    put(&mut a, CTX + 0x14, count);
    let mode = [0u16, 1, 2, 3][r.below(4) as usize];
    put16(&mut a, MODE + 0x2c, mode);

    // The contrail.
    for o in (0..0x60).step_by(4) {
        let v = r.float();
        put(&mut a, C + o, v);
    }
    let tinted = r.chance(2);
    let object = if tinted { r.below(4) | (r.u32() << 16) } else { u32::MAX };
    put(&mut a, C + 8, object);
    put16(&mut a, C + 0xc, r.below(4) as u16);
    for s in 0..4 {
        put16(&mut a, C + 0x30 + 2 * s, r.below(5) as u16);
        put(&mut a, C + 0x38 + 4 * s, if r.chance(8) { u32::MAX } else { r.below(points.max(1)) });
    }
    if points == 0 {
        put(&mut a, C + 0x38 + 4 * slot, u32::MAX);
    }

    // The definition.
    for o in (0..0x100).step_by(4) {
        let v = r.float();
        put(&mut a, DEF + o, v);
    }
    let flags = r.u32() & 0xff;
    a.bytes[DEF as usize] = flags as u8;
    a.bytes[(DEF + 2) as usize] = r.u32() as u8;
    let facing: i16 = match r.below(16) {
        0 => 3,
        1 => 5,
        2 => -1,
        n => [0, 1, 2, 4][(n % 4) as usize],
    };
    put16(&mut a, DEF + 0x18, facing as u16);
    let stages = r.below(5);
    put(&mut a, DEF + 0xe8, stages);
    put(&mut a, DEF + 0xec, STAGES);
    for o in (0..0x180).step_by(4) {
        let v = if r.chance(3) { r.float() } else { r.unit() };
        put(&mut a, STAGES + o, v);
    }
    for s in 0..6 {
        a.bytes[(STAGES + 0x40 * s + 0x3c) as usize] = r.u32() as u8;
    }

    // The points: a chain through the array in a shuffled order.
    let mut order: Vec<u32> = (0..points).collect();
    for i in (1..order.len()).rev() {
        let j = r.below(i as u32 + 1) as usize;
        order.swap(i, j);
    }
    let mut blended = false;
    for i in 0..8 {
        let p = DATA + 0x38 * i;
        for o in (0..0x38).step_by(4) {
            let v = r.coordinate();
            put(&mut a, p + o, v);
        }
        let b = r.u32() as u8;
        a.bytes[(p + 2) as usize] = b;
        blended |= b & 2 != 0 && i < points;
        a.bytes[(p + 3) as usize] = [0u8, 1, 2, 3, 0xff, 7][r.below(6) as usize];
        put(&mut a, p + 4, r.unit());
        put(&mut a, p + 0xc, if r.chance(3) { r.float() } else { r.unit() });
    }
    if let Some(&first) = order.first() {
        put(&mut a, C + 0x38 + 4 * slot, first | (r.below(0xffff) << 16));
    }
    for (k, &i) in order.iter().enumerate() {
        let next = order.get(k + 1).map_or(u32::MAX, |&n| n | (r.below(0xffff) << 16));
        put(&mut a, DATA + 0x38 * i + 0x34, next);
    }

    // The object: two to four function sources or a bad index.
    for i in 0..4 {
        put(&mut a, OBJ_DATA + 12 * i + 8, DATUM);
    }
    put(&mut a, DATUM, r.below(4));
    for i in 0..4 {
        put(&mut a, TAG_ARRAY + 16 * i + 8, TAG);
    }
    put(&mut a, TAG + 0x98, BLOCK);
    for i in 0..4 {
        put16(&mut a, BLOCK + 24 * i + 0xc, (r.below(6) as i16 - 1) as u16);
    }
    for o in (0x100..0x400).step_by(4) {
        let v = if r.chance(4) { r.float() } else { r.unit() };
        put(&mut a, DATUM + o, v);
    }
    let words = if r.chance(5) { r.below(200) } else { 24 * (1 + r.below(4)) };
    put16(&mut a, DATUM + 0x124, words as u16);
    put16(&mut a, DATUM + 0x126, 0x100);
    let faded = mode != 0;
    (
        a,
        Seeded {
            points,
            facing,
            tinted: tinted && flags & 0x80 == 0,
            blended,
            faded,
        },
    )
}

#[test]
fn the_ribbon_matches_the_lifted_original() {
    if !with_image(cases) {
        eprintln!("SKIPPED the_ribbon_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(ENTRY);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..512u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let esp = STACK_TOP - 0x400;
            let (mut a, s) = case(image, &mut r, esp);
            let (mx, cw) = (mxcsr(&mut r), x87_control(&mut r));
            put(&mut a, esp, HARNESS_EXIT);
            for (i, v) in [OUT, r.u32(), CTX].into_iter().enumerate() {
                put(&mut a, esp + 4 + 4 * i as u32, v);
            }
            let want = cr::predict(&Ro(&a), esp, mx, cw);
            let before = a.clone();
            let mut c = Cpu {
                gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
                fs_base: RAM as u32,
                eip: ENTRY,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            let ran = lifted.run(&mut c, &mut a, &[HARNESS_EXIT]);
            let want = match (want, ran) {
                (Ok(w), Ok(HARNESS_EXIT)) => w,
                (Err(e), _) => {
                    *seen.entry(cr::refusal(e)).or_default() += 1;
                    continue;
                }
                (Ok(_), other) => panic!("seed {seed} case {n}: the original did not return: {other:?}"),
            };
            assert_eq!(c.gpr[4], esp + POP, "seed {seed} case {n}: ESP");
            assert_eq!(c.gpr[0] as u8, want.ret, "seed {seed} case {n}: AL");
            let span = want.out as usize..want.out as usize + want.vertices.len();
            assert_eq!(a.bytes[span], want.vertices[..], "seed {seed} case {n}: the vertices");
            let mut expect = before;
            for (&addr, &b) in &want.writes {
                expect.bytes[addr as usize] = b;
            }
            for (base, len) in REGIONS {
                let r = base as usize..(base + len) as usize;
                assert_eq!(a.bytes[r.clone()], expect.bytes[r], "seed {seed} case {n}: the record at {base:#x}");
            }
            assert!(want.points <= s.points, "seed {seed} case {n}: points");
            let facing = match s.facing {
                _ if s.points == 0 => "no points",
                0 => "facing vertical",
                1 | 2 => "facing flat",
                4 => "facing the camera",
                _ => "facing unknown",
            };
            *seen.entry(facing).or_default() += 1;
            if want.ret == 1 && want.points > 0 {
                for (k, on) in [("tinted", s.tinted), ("blended", s.blended), ("faded", s.faded)] {
                    if on {
                        *seen.entry(k).or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("ribbon: all equal; {seen:#?}");
    for k in [
        "no points",
        "facing vertical",
        "facing flat",
        "facing the camera",
        "facing unknown",
        "tinted",
        "blended",
        "faded",
    ] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
