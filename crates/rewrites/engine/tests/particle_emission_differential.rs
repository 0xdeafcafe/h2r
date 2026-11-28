//! `engine::particle_emission` against the original `0x00224550`, lifted
//! and run on the IR interpreter from the same seeded state, from its
//! entry to its call of the emitter `0x0023ed30`: the emitter's ESP, the
//! eight words, EBX, the three vectors they point at, and every byte of
//! the seeded records must be the port's (the model's writes applied).
//! Its callees (`0x00173ba0`, `0x00248df0`, `0x0004efb0`, `0x00246cd0`,
//! `0x00246d80`, `0x0013b390`, `0x0013bc00`, `0x00131bb0`, `0x00131e50`,
//! `0x00016ae0`) run lifted inside.
//!
//! Each case seeds a definition (flags, the input mask, five properties
//! with identity, constant or linear functions, ranged or not, with or
//! without a modifier), the input block and its cache, the records the
//! inputs read, the view, a sequence bitmap or none, and the arguments,
//! with odd float bits now and then, under several MXCSR and x87 control
//! words (exceptions masked).
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it particle_emission_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

use std::collections::BTreeMap;

mod common;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::particle_emission::{self as pe, EMIT, EMIT_DEPTH, ENTRY};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 200_000;

const DEF: u32 = 0x0300_0000;
const PART: u32 = 0x0301_0000;
const STATE: u32 = 0x0302_0000;
const RECORD: u32 = 0x0303_0000;
const DRAW: u32 = 0x0304_0000;
const TINT: u32 = 0x0305_0000;
const BLOBS: u32 = 0x0306_0000;
const BITMAP: u32 = 0x0307_0000;
const VECTORS: u32 = 0x0308_0000;
/// The seeded records: each compared whole after the run.
const REGIONS: [(u32, u32); 8] = [
    (DEF, 0x100),
    (PART, 0x40),
    (STATE, 0x60),
    (RECORD, 0x40),
    (DRAW, 0x10),
    (TINT, 0x10),
    (BITMAP, 0x1000),
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
        match self.below(24) {
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

/// A property at `at`: an input of the sixteen, a function of type 0
/// (identity), 1 (constant) or 4 (linear), ranged or not, the range lerp
/// on or off, a modifier now and then.
fn property(a: &mut FlatMemory, r: &mut Rng, at: u32, blob: u32) {
    put16(a, at, r.below(16) as u16);
    put16(a, at + 2, r.below(16) as u16);
    put16(a, at + 4, r.below(3) as u16);
    put16(a, at + 6, if r.chance(2) { u16::MAX } else { r.below(16) as u16 });
    put(a, at + 8, if r.chance(12) { 0 } else { 1 });
    put(a, at + 0xc, blob);
    let kind = [0u8, 0, 1, 4][r.below(4) as usize];
    a.bytes[blob as usize] = kind;
    a.bytes[blob as usize + 1] = (r.below(2) as u8) | if r.chance(4) { 0x10 } else { 0 };
    for o in (4..0x40).step_by(4) {
        let v = r.float();
        put(a, blob + o, v);
    }
}

/// The colour property: no function, a single colour or a grey ramp.
fn colour(a: &mut FlatMemory, r: &mut Rng, at: u32, blob: u32) {
    property(a, r, at, blob);
    let (kind, flags) = [(1u8, 0x10u8), (0, 0x00), (0, 0x10), (2, 0x20)][r.below(4) as usize];
    a.bytes[blob as usize] = kind;
    a.bytes[blob as usize + 1] = flags | (r.below(2) as u8);
    put(a, blob + 4, r.u32());
    put(a, blob + 8, r.u32());
    put(a, blob + 0x10, r.u32());
}

fn case(image: &FlatMemory, r: &mut Rng) -> FlatMemory {
    let mut a = image.clone();
    let flag_bits = [2u32, 4, 8, 0x10, 0x20, 0x80, 0x100, 0x2000, 0x1_0000];
    let mut flags = r.u32() & !(flag_bits.iter().fold(0, |m, b| m | b));
    for b in flag_bits {
        if r.chance(2) {
            flags |= b;
        }
    }
    put(&mut a, DEF, flags);
    put(&mut a, DEF + 8, (r.below(5) as i32 - 2) as u32 & 0xffff | (r.below(4) << 16));
    put(&mut a, DEF + 0x98, r.u32() | if r.chance(3) { 0xf80f } else { 0 });
    colour(&mut a, r, DEF + 0x1c, BLOBS);
    for (i, at) in [0x2c, 0x3c, 0x4c, 0x5c].into_iter().enumerate() {
        property(&mut a, r, DEF + at, BLOBS + 0x100 * (i as u32 + 1));
    }
    for o in (0x9c..0xbc).step_by(4) {
        let v = if r.chance(3) { r.float() } else { r.unit() };
        put(&mut a, DEF + o, v);
    }
    for o in (0..0x40).step_by(4) {
        let v = r.u32();
        put(&mut a, PART + o, v);
    }
    put16(&mut a, PART + 0x16, r.below(64) as u16);
    // The input block: its cache, the mask, the records.
    for o in (0..0x40).step_by(4) {
        let v = r.float();
        put(&mut a, STATE + o, v);
    }
    put(&mut a, STATE + 0x44, if r.chance(3) { 0 } else { r.u32() });
    put(&mut a, STATE + 0x48, r.u32());
    put(&mut a, STATE + 0x4c, if r.chance(5) { 0 } else { r.u32() | 1 });
    put(&mut a, STATE + 0x50, if r.chance(6) { 0 } else { RECORD });
    for o in (0..0x40).step_by(4) {
        let v = if r.chance(4) { r.u32() } else { r.float() };
        put(&mut a, RECORD + o, v);
    }
    for at in (pe::constant::CAMERA..pe::constant::UP + 0xc).step_by(4) {
        let v = r.float();
        put(&mut a, at, v);
    }
    // The draw: the definition, a sequence bitmap or none, a scale.
    put(&mut a, DRAW, DEF);
    let bitmap = if r.chance(4) { 0 } else { BITMAP };
    put(&mut a, DRAW + 4, bitmap);
    put(&mut a, DRAW + 8, if r.chance(3) { 0 } else { BITMAP + 0x100 });
    put(&mut a, DRAW + 0xc, r.float());
    put16(&mut a, BITMAP, if r.chance(5) { 2 } else { 3 });
    put(&mut a, BITMAP + 0x3c, r.below(4));
    put(&mut a, BITMAP + 0x40, BITMAP + 0x100);
    for s in 0..4u32 {
        let seq = BITMAP + 0x100 + 0x3c * s;
        put(&mut a, seq + 0x34, if r.chance(5) { 0 } else { 1 + r.below(5) });
        put(&mut a, seq + 0x38, BITMAP + 0x400 + 0x100 * s);
    }
    for o in (0x400..0x800).step_by(4) {
        let v = if r.chance(6) { r.float() } else { r.unit() };
        put(&mut a, BITMAP + o, v);
    }
    for o in (0..0x10).step_by(4) {
        let v = r.float();
        put(&mut a, TINT + o, v);
        let v = r.float();
        put(&mut a, VECTORS + o, v);
    }
    a
}

#[test]
fn emission_matches_the_lifted_original() {
    if !with_image(cases) {
        eprintln!("SKIPPED emission_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(ENTRY);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..512u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let mut a = case(image, &mut r);
            let (mx, cw) = (mxcsr(&mut r), x87_control(&mut r));
            let esp = STACK_TOP - 0x400;
            let args = [PART, STATE, VECTORS, VECTORS + 0x10, r.float(), TINT, DRAW];
            put(&mut a, esp, HARNESS_EXIT);
            for (i, v) in args.into_iter().enumerate() {
                put(&mut a, esp + 4 + 4 * i as u32, v);
            }
            let want = pe::predict(&Ro(&a), esp, mx, cw);
            let before = a.clone();
            let mut c = Cpu {
                gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
                fs_base: RAM as u32,
                eip: ENTRY,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            let ran = lifted.run(&mut c, &mut a, &[EMIT, HARNESS_EXIT]);
            let want = match (want, ran) {
                (Ok(w), Ok(EMIT)) => w,
                (Err(e), _) => {
                    *seen.entry(pe::refusal(e)).or_default() += 1;
                    continue;
                }
                (Ok(_), other) => panic!("seed {seed} case {n}: the original did not reach the emitter: {other:?}"),
            };
            let at = c.gpr[4];
            assert_eq!(at, esp - EMIT_DEPTH, "seed {seed} case {n}: the emitter's ESP");
            let words: Vec<u32> = (0..8).map(|i| word(&a, at + 4 + 4 * i)).collect();
            let e = &want.emission;
            assert_eq!(words, e.words.to_vec(), "seed {seed} case {n}: the words");
            assert_eq!(c.gpr[3], e.ebx, "seed {seed} case {n}: EBX");
            let vec4 = |p: u32| [0, 4, 8, 12].map(|o| word(&a, p + o));
            assert_eq!(vec4(e.words[6]), e.window, "seed {seed} case {n}: the window");
            assert_eq!(vec4(e.words[7]), e.next_window, "seed {seed} case {n}: the next window");
            assert_eq!(vec4(e.ebx), e.corners, "seed {seed} case {n}: the corners");
            let mut expect = before;
            for (&addr, &b) in &want.writes {
                expect.bytes[addr as usize] = b;
            }
            for (base, len) in REGIONS {
                let r = base as usize..(base + len) as usize;
                assert_eq!(a.bytes[r.clone()], expect.bytes[r], "seed {seed} case {n}: the record at {base:#x}");
            }
            let path = if flags_of(&a) & 0x1_0000 != 0 {
                "definition window"
            } else if word(&a, DRAW + 4) != 0 && e.words[5] != 0 {
                "bitmap, blended"
            } else {
                "bitmap or none"
            };
            *seen.entry(path).or_default() += 1;
        }
    }
    eprintln!("emission: all equal; {seen:#?}");
    for k in ["definition window", "bitmap, blended", "bitmap or none"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

fn flags_of(a: &FlatMemory) -> u32 {
    word(a, DEF)
}
