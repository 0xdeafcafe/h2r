//! `engine::screen_effects` against the original, lifted and run on the
//! IR interpreter from the same seeded 64 MB state:
//!
//! - `function_value`: `0x00134fc0` from its entry (ECX the index) to its
//!   return, the evaluator `0x0013b390` and its helpers lifted inside, over
//!   random scenario functions of every type (with and without a parent,
//!   ranged or not, raw or through their range) and states;
//! - `drawn`: `0x00013870` from `0x000139ba` (after the colour pass) to the
//!   end of its screen-effect loop, `0x00022710` and `0x00134fc0` lifted
//!   inside and `0x0002d140` stubbed (`ret 0x10`, recorded: EAX the datum
//!   and the four values), over random gates and entries;
//! - `screen_effect::plan`: `0x0002d140` from its entry to its return,
//!   `0x0001dd60` lifted inside; stubbed: `0x00016610` (the template pass,
//!   drawing; its arguments recorded), `0x0001ddd0` (the size recorded and
//!   stored as it stores it), the blur `0x000211a0` and `0x00021360`
//!   (their registers and arguments recorded; returning `glow::blur`'s
//!   and `glow::wide`'s results, which the next convolution reads), the push
//!   buffer's flush, `End` and the draw-state reset; over random egor
//!   passes, offsets, convolutions, targets and windows. The template
//!   passes, the words the strips push, the `End`s, the sizes set and the
//!   convolutions must be the plan's.
//!
//! The evaluator's wave and shape tables are the image's (read from it,
//! never copied). Results are bit-equal for every type, NaNs included.
//! The CRT's error paths (`fmod` and `pow` on NaNs, infinities, domain
//! errors, overflow and underflow) run lifted too; only `_getptd`
//! (`0x00326079`, which reads the thread block through FS) is stubbed,
//! returning a scratch per-thread block for `errno`. Needs the user's XBE
//! (`H2_XBE`, or `disc/default.xbe`), pinned by hash; skips without it.
//! `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it screen_effects_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::glow::{self, BlurArgs, WideArgs};
use engine::screen_effect::{self as fx, Convolution};
use engine::screen_effects::{self as se, Drawn};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

const FUNCTION_VALUE: u32 = 0x0013_4fc0;
/// `_getptd`, stubbed: EAX a scratch per-thread block.
const GETPTD: u32 = 0x0032_6079;
const PTD: u32 = 0x03d0_0000;
const SCREEN_EFFECT: u32 = 0x0002_d140;
const LOOP: (u32, [u32; 2]) = (0x0001_39ba, [0x0001_3b72, 0x0001_3b73]);

const STATE: u32 = 0x0300_0000;
const SCEN: u32 = 0x0310_0000;
const FUNCS: u32 = 0x0320_0000;
const BLOBS: u32 = 0x0330_0000;
const EFFECTS: u32 = 0x0340_0000;
const BLOB_STRIDE: u32 = 0x200;

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

    fn run(
        &mut self,
        cpu: &mut Cpu,
        mem: &mut FlatMemory,
        exits: &[u32],
        stubs: &Stubs,
        seen: &mut dyn FnMut(u32, &mut Cpu, &mut FlatMemory),
    ) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(format!("step limit ({STEP_LIMIT} instructions) at {pc:#x}"));
            }
            if let Some(&cleanup) = stubs.get(&pc) {
                seen(pc, cpu, mem);
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

fn cpu(r: &mut Rng, esp: u32, eip: u32) -> Cpu {
    let mut c = Cpu {
        gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
        fs_base: RAM as u32,
        eip,
        ..Cpu::default()
    };
    c.mxcsr = 0x1f80;
    c.x87.control = 0x027f;
    c
}

/// Words per set of each type (the evaluator's table at `0x0044178c`).
fn set_words(kind: u8) -> u32 {
    match kind {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        4 => 6,
        5 => 0x14,
        6 => 0x20,
        7 | 10 => 12,
        8 => 4,
        9 => 3,
        _ => 4,
    }
}

/// `n` random scenario functions, their blobs and states; returns each
/// one's type and its parent's (or its own).
fn functions(r: &mut Rng, m: &mut FlatMemory, n: u32) -> Vec<(u8, u8)> {
    put(m, se::SCENARIO, SCEN);
    put(m, se::FUNCTION_STATE, STATE);
    put(m, SCEN + se::SCENARIO_FUNCTIONS, n);
    put(m, SCEN + se::SCENARIO_FUNCTIONS + 4, FUNCS);
    let mut kinds = Vec::new();
    for i in 0..n {
        let f = FUNCS + i * se::FUNCTION_SIZE;
        let blob = BLOBS + i * BLOB_STRIDE;
        let kind = r.below(12) as u8;
        let flags = [0u8, 1, 0x10, 0x11, 0x20][r.below(5) as usize];
        let subs = [r.below(12) as u8, r.below(12) as u8];
        let sub = if kind == 2 { [subs[0] % 6, subs[1] % 6] } else { subs };
        let len = 0x14 + 8 * set_words(kind);
        m.bytes[blob as usize] = kind;
        m.bytes[blob as usize + 1] = flags;
        m.bytes[blob as usize + 2] = sub[0];
        m.bytes[blob as usize + 3] = sub[1];
        put(m, blob + 4, r.float(4.0));
        put(m, blob + 8, r.float(4.0));
        for w in 0..(len - 0xc) / 4 {
            let v = r.float(2.0);
            put(m, blob + 0xc + 4 * w, v);
        }
        // Size: mostly the blob's, now and then empty.
        put(m, f + 0xc, if r.below(12) == 0 { 0 } else { len });
        put(m, f + 0x10, if r.below(16) == 0 { 0 } else { blob });
        let parent = match r.below(4) {
            0 if n > 1 => r.below(n) as u16,
            1 => r.below(n + 3) as u16,
            _ => 0xffff,
        };
        put16(m, f + 0x16, parent);
        put(m, STATE + 0x20 * i + 4, r.float(1.5));
        put(m, STATE + 0x20 * i + 0x1c, r.float(0.5));
        kinds.push(kind);
    }
    kinds
        .iter()
        .enumerate()
        .map(|(i, &k)| {
            let p = u32::from(u16::from_le_bytes(
                m.bytes[(FUNCS + i as u32 * se::FUNCTION_SIZE + 0x16) as usize..][..2].try_into().unwrap(),
            ));
            (k, kinds.get(p as usize).copied().unwrap_or(k))
        })
        .collect()
}

#[test]
fn function_values_match_the_lifted_original() {
    if !with_image(function_cases) {
        eprintln!("SKIPPED function_values_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn function_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(FUNCTION_VALUE);
    let stubs: Stubs = [(GETPTD, 0)].into_iter().collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut wrong: Vec<String> = Vec::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            let count = 1 + r.below(6);
            let kinds = functions(&mut r, &mut m, count);
            for index in 0..count {
                let want = se::function_value(&Eng(&m), index as i32, 0x1f80, 0x027f);
                let mut a = m.clone();
                let esp = STACK_TOP - 0x400;
                put(&mut a, esp, HARNESS_EXIT);
                let mut c = cpu(&mut r, esp, FUNCTION_VALUE);
                c.gpr[1] = index;
                let ran = lifted.run(&mut c, &mut a, &[HARNESS_EXIT], &stubs, &mut |_, c, _| c.gpr[0] = PTD);
                let got = c.xmm[0] as u32;
                let k = kinds[index as usize];
                match (want, ran) {
                    (Ok(w), Ok(_)) => {
                        if w != got {
                            wrong.push(format!(
                                "seed {seed} case {n} function {index} types {k:?}: port {w:#x} ({}), original {got:#x} ({})",
                                f32::from_bits(w),
                                f32::from_bits(got)
                            ));
                        }
                    }
                    (Err(e), Ok(_)) => wrong.push(format!("seed {seed} case {n} function {index}: port {e:?}, original {got:#x}")),
                    (Ok(w), Err(e)) => wrong.push(format!("seed {seed} case {n} function {index}: port {w:#x}, original {e}")),
                    (Err(_), Err(_)) => {}
                }
                *seen.entry(format!("type {}", k.0)).or_default() += 1;
            }
        }
    }
    eprintln!("function values: {seen:#?}");
    let mut by_type: BTreeMap<String, u32> = BTreeMap::new();
    for w in &wrong {
        let t = w.split("types ").nth(1).and_then(|s| s.split(':').next()).unwrap_or("?").to_string();
        *by_type.entry(t).or_default() += 1;
    }
    for w in wrong.iter().take(40) {
        eprintln!("  {w}");
    }
    assert!(wrong.is_empty(), "{} differ, by types: {by_type:?}", wrong.len());
    for t in 0..=11 {
        assert!(seen.contains_key(&format!("type {t}")), "type {t} never seen");
    }
}

#[test]
fn the_entries_drawn_match_the_lifted_original() {
    if !with_image(drawn_cases) {
        eprintln!("SKIPPED the_entries_drawn_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn drawn_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(0x0001_3870);
    lifted.lift(FUNCTION_VALUE);
    let stubs: Stubs = [(SCREEN_EFFECT, 0x10), (GETPTD, 0)].into_iter().collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n));
            let mut m = image.clone();
            let count = 1 + r.below(4);
            functions(&mut r, &mut m, count);
            // The gates, mostly open.
            let gate = r.below(10);
            put(&mut m, se::VIEW_KIND, if gate == 0 { 5 + r.below(3) } else { r.below(5) });
            m.bytes[se::OFF_BYTE as usize] = u8::from(gate == 1);
            put(&mut m, se::OFF_WORD, u32::from(gate == 2));
            for (a, v) in [(0x0048_5648u32, 0u16), (0x0048_564a, 0), (0x0048_564c, 0x1e0), (0x0048_564e, 0x280)] {
                put16(&mut m, a, if gate == 3 && a == 0x0048_564c { 0x1df } else { v });
            }
            put(&mut m, 0x004b_a04c, if gate == 4 { 2 } else { r.below(2) });
            if gate == 5 {
                put(&mut m, se::SCENARIO, 0);
            }
            let entries = r.below(6);
            put(&mut m, SCEN + se::SCENARIO_SCREEN_EFFECTS, entries);
            put(&mut m, SCEN + se::SCENARIO_SCREEN_EFFECTS + 4, EFFECTS);
            for i in 0..entries {
                let e = EFFECTS + i * se::ENTRY_SIZE;
                put(&mut m, e + 0x14, if r.below(6) == 0 { u32::MAX } else { r.u32() & 0xffff_fff0 });
                let idx = |r: &mut Rng| if r.below(3) == 0 { 0xffff } else { r.below(count) as u16 };
                let (a, b) = (idx(&mut r), idx(&mut r));
                put16(&mut m, e + 0x20, a);
                put16(&mut m, e + 0x22, b);
            }
            let want: Vec<(u32, [u32; 4])> = se::drawn(&Eng(&m), 0x1f80, 0x027f)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"))
                .into_iter()
                .map(|Drawn { datum, values, .. }| (datum, values))
                .collect();

            let mut a = m.clone();
            let esp = STACK_TOP - 0x400;
            let mut c = cpu(&mut r, esp, LOOP.0);
            let mut got = Vec::new();
            let mut record = |pc: u32, c: &mut Cpu, m: &mut FlatMemory| {
                if pc == GETPTD {
                    c.gpr[0] = PTD;
                } else if pc == SCREEN_EFFECT {
                    let values = word(m, c.gpr[4] + 4);
                    got.push((c.gpr[0], [0, 4, 8, 12].map(|o| word(m, values + o))));
                }
            };
            lifted
                .run(&mut c, &mut a, &LOOP.1, &stubs, &mut record)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(got, want, "seed {seed} case {n}: gate {gate}");
            *seen.entry(format!("{} drawn", want.len().min(3))).or_default() += 1;
        }
    }
    eprintln!("entries drawn: {seen:#?}");
    for k in ["0 drawn", "1 drawn", "2 drawn"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

const EFFECT: u32 = 0x0002_d140;
const TEMPLATE_PASS: u32 = 0x0001_6610;
const SET_SIZE: u32 = 0x0001_ddd0;
const BLUR: u32 = 0x0002_11a0;
const WIDE: u32 = 0x0002_1360;
const FLUSH: u32 = 0x0040_0fa0;
const SPACE: u32 = 0x003f_aae0;
const END: u32 = 0x003f_b480;
const RESET: u32 = 0x0001_6b10;
/// The push buffer: `+0` where the next word goes, `+4` its end.
const PUSH: u32 = 0x0040_7488;

const TT: u32 = 0x0380_0000;
const EGOR: u32 = 0x0390_0000;
const SDEF: u32 = 0x0391_0000;
const TDEF: u32 = 0x0392_0000;
const X: u32 = 0x0393_0000;
const PASSES: u32 = 0x0394_0000;
const OFFSETS: u32 = 0x0395_0000;
const CONVS: u32 = 0x0396_0000;
const VECTOR: u32 = 0x0397_0000;
const PB: u32 = 0x0398_0000;
const BUF: u32 = 0x03a0_0000;
const VALUES: u32 = 0x03b0_0000;
const CORNERS: u32 = 0x03b0_0100;
const DATUM: u32 = 0xe000_0003;
const SHADER: u32 = 0xe001_0005;

/// What the original called, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Did {
    /// `0x00016610`'s six arguments.
    Pass([u32; 6]),
    /// `0x0001ddd0`: ECX the target, ESI the width, EDI the height.
    Size(u32, i32, i32),
    /// The blur: EAX, then its first seven stack words.
    Blur(u32, [u32; 7]),
    /// `0x00021360`: EAX, ECX, XMM0, then its first six stack words.
    Wide(u32, i32, u32, [u32; 6]),
}

/// A finite float in 0..top.
fn finite(r: &mut Rng, top: f32) -> u32 {
    (r.below(1 << 20) as f32 / (1 << 20) as f32 * top).to_bits()
}

fn stack_arg(m: &FlatMemory, c: &Cpu, i: u32) -> u32 {
    word(m, c.gpr[4] + 4 + 4 * i)
}

/// A random egor and the state `0x0002d140` reads; returns the datum, the
/// values and the corners (2 x 2).
fn effect_world(r: &mut Rng, m: &mut FlatMemory) -> (u32, [u32; 4], Vec<[u32; 4]>) {
    put(m, fx::TAG_TABLE, TT);
    put(m, TT + 3 * 16 + 8, EGOR);
    put(m, TT + 5 * 16, 0x7264_6873);
    put(m, TT + 5 * 16 + 8, SDEF);
    put(m, SDEF + 0x24, SDEF + 0x100);
    put(m, SDEF + 0x100, 0xe002_0006);
    put(m, TT + 6 * 16 + 8, TDEF);
    put(m, TDEF + 0x5c, X);
    put(m, X + 4, X + 0x100);
    let at = r.below(4);
    put16(m, X + 0x100, (r.u32() as u16 & 0xfe00) | at as u16);
    put(m, X + 0xc, X + 0x200);
    put16(m, X + 0x200 + 2 * at + 0x1e, (r.below(5) << 9) as u16 | r.below(0x1ff) as u16);
    put(m, EGOR + 0x44, SHADER);
    let passes = r.below(4);
    put(m, EGOR + 0x88, passes);
    put(m, EGOR + 0x8c, PASSES);
    for i in 0..passes {
        let p = PASSES + i * fx::PASS_SIZE;
        put16(m, p + 8, if r.below(3) == 0 { 0xffff } else { r.below(6) as u16 });
        for b in 0..4 {
            m.bytes[(p + 0xc + b) as usize] = [0xff, 0, 1, 2, 5][r.below(5) as usize];
        }
        for s in 0..4 {
            put16(m, p + 0x50 + 2 * s, r.below(5) as u16);
        }
        let offsets = OFFSETS + i * 0x100;
        put(m, p + 0x58, [0u32, 1, 2, 0xffff_ffff][r.below(4) as usize]);
        put(m, p + 0x5c, offsets);
        for s in 0..4 {
            put16(m, offsets + 2 * s, r.below(4) as u16);
            for k in 0..4 {
                let v = r.float(2.0);
                put(m, offsets + 8 + 0x10 * s + 4 * k, v);
            }
        }
        put16(m, p + 0x60, r.below(4) as u16);
        let convs = r.below(3);
        put(m, p + 0xa4, convs);
        let base = CONVS + i * 0x200;
        put(m, p + 0xa8, base);
        for n in 0..convs {
            let c = base + n * fx::CONVOLUTION_SIZE;
            put16(m, c, r.below(8) as u16);
            put(m, c + 0x44, finite(r, 5.0));
            put(m, c + 0x48, r.float(2.0));
            put(m, c + 0x4c, finite(r, 1.0));
            put(m, c + 0x50, r.u32());
            put(m, c + 0x54, r.u32());
            put(m, c + 0x58, [0.125f32, 0.25, 0.5, 1.0, 2.0][r.below(5) as usize].to_bits());
        }
    }
    put(m, fx::constant::VECTOR, VECTOR);
    for k in 0..4 {
        let v = r.float(3.0);
        put(m, VECTOR + 4 * k, v);
    }
    put(m, fx::constant::OFFSET_SCALE, [1.0f32, 2.0, 0.5][r.below(3) as usize].to_bits());
    // The window (top, left, bottom, right): mostly the screen.
    let w: [u16; 4] = match r.below(10) {
        0 => [0, 0, 0, 640],
        1 => [0, 640, 480, 0],
        2 => [10, 20, 401, 603],
        _ => [0, 0, 480, 640],
    };
    for (o, v) in w.iter().enumerate() {
        put16(m, fx::WINDOW + 2 * o as u32, *v);
    }
    put(m, fx::TARGET_OVERRIDE, u32::from(r.below(12) == 0));
    for t in [0u32, 1, 0x19, 0x1a, 0x1b] {
        let e = fx::TARGETS + t * fx::TARGET_STRIDE;
        put(m, e + 0x88, if r.below(4) == 0 { 0 } else { 0x0100_0000 });
        m.bytes[(e + 0x95) as usize] = u8::from(r.below(6) == 0);
        put(m, e + 0x80, [640u32, 320, 1280, 100][r.below(4) as usize]);
        put(m, e + 0x84, [480u32, 240, 960, 50][r.below(4) as usize]);
    }
    put(m, PUSH, PB);
    put(m, PB, BUF);
    put(m, PB + 4, BUF + 0x0008_0000);
    let value =|r: &mut Rng| if r.below(3) == 0 { 0 } else { r.float(1.0) };
    let values = [value(r), value(r), r.float(1.0), r.float(1.0)];
    let corners = (0..4)
        .map(|_| [r.float(700.0), r.float(700.0), r.float(1.0), r.float(1.0)])
        .collect();
    (if r.below(16) == 0 { u32::MAX } else { DATUM }, values, corners)
}

/// The plan's calls, in the original's order (before the restores).
fn expected(e: &fx::Effect) -> Vec<Did> {
    let mut v = Vec::new();
    for p in &e.passes {
        if let Some((w, h)) = p.size {
            v.push(Did::Size(1, w, h));
        }
        v.push(Did::Pass([e.shader, 15, p.implementation, p.selector as u32, u32::from(p.kind != 0), 0]));
        for c in &p.convolutions {
            match c {
                Convolution::Kept { .. } => {}
                Convolution::Blur { size, args, .. } => {
                    v.push(Did::Size(args.dest, size.0, size.1));
                    v.push(Did::Size(args.other, size.0, size.1));
                    v.push(Did::Blur(
                        args.source,
                        [args.steps, args.scale, args.growth, args.dest, args.other, args.last, 1],
                    ));
                }
                Convolution::Wide { size, args, .. } => {
                    v.push(Did::Size(args.dest, size.0, size.1));
                    v.push(Did::Size(args.other, size.0, size.1));
                    v.push(Did::Wide(
                        args.source,
                        args.taps,
                        args.scale,
                        [args.words[0], args.words[1], args.dest, args.other, args.last, 1],
                    ));
                }
            }
        }
    }
    v
}

/// The four sizes `0x0002d140` puts back at its end: targets 1, `0x19`,
/// `0x19` (`0x1a`'s size) and `0x1b`, each as it found it (0 x 0 without
/// a surface).
fn restores(m: &FlatMemory) -> Vec<Did> {
    let size = |t: u32| {
        let e = fx::TARGETS + t * fx::TARGET_STRIDE;
        if word(m, e + 0x88) != 0 && m.bytes[(e + 0x95) as usize] == 0 {
            (word(m, e + 0x80) as i32, word(m, e + 0x84) as i32)
        } else {
            (0, 0)
        }
    };
    [(1, 1), (0x19, 0x19), (0x19, 0x1a), (0x1b, 0x1b)]
        .into_iter()
        .map(|(t, from)| {
            let (w, h) = size(from);
            Did::Size(t, w, h)
        })
        .collect()
}

#[test]
fn effect_plans_match_the_lifted_original() {
    if !with_image(effect_cases) {
        eprintln!("SKIPPED effect_plans_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn effect_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(EFFECT);
    let stubs: Stubs = [
        (TEMPLATE_PASS, 0x18),
        (SET_SIZE, 0),
        (BLUR, 0x28),
        (WIDE, 0x20),
        (FLUSH, 0),
        (SPACE, 4),
        (END, 0),
        (RESET, 0),
    ]
    .into_iter()
    .collect();
    let depth = word(image, fx::constant::DEPTH);
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x5851_f42d_4c95_7f2d) ^ u64::from(n));
            let mut m = image.clone();
            let (datum, values, corners) = effect_world(&mut r, &mut m);
            let want = fx::plan(&Eng(&m), datum, values, 2, 2, &corners, 0x1f80, 0x027f)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            let restored = restores(&m);

            for (i, v) in values.iter().enumerate() {
                put(&mut m, VALUES + 4 * i as u32, *v);
            }
            for (i, c) in corners.iter().enumerate() {
                for (k, v) in c.iter().enumerate() {
                    put(&mut m, CORNERS + 16 * i as u32 + 4 * k as u32, *v);
                }
            }
            let esp = STACK_TOP - 0x400;
            for (i, v) in [HARNESS_EXIT, VALUES, 2, 2, CORNERS].iter().enumerate() {
                put(&mut m, esp + 4 * i as u32, *v);
            }
            let mut c = cpu(&mut r, esp, EFFECT);
            c.gpr[0] = datum;
            let mut did = Vec::new();
            let mut ends = 0usize;
            let mut record = |pc: u32, c: &mut Cpu, m: &mut FlatMemory| match pc {
                TEMPLATE_PASS => {
                    did.push(Did::Pass(std::array::from_fn(|i| stack_arg(m, c, i as u32))));
                    c.gpr[0] = (c.gpr[0] & !0xff) | 1;
                }
                SET_SIZE => {
                    let (t, w, h) = (c.gpr[1], c.gpr[6], c.gpr[7]);
                    did.push(Did::Size(t, w as i32, h as i32));
                    let e = fx::TARGETS.wrapping_add(t.wrapping_mul(fx::TARGET_STRIDE));
                    put(m, e + 0x80, w);
                    put(m, e + 0x84, h);
                }
                BLUR => {
                    let words: [u32; 7] = std::array::from_fn(|i| stack_arg(m, c, i as u32));
                    did.push(Did::Blur(c.gpr[0], words));
                    let args = BlurArgs {
                        source: c.gpr[0],
                        steps: words[0],
                        scale: words[1],
                        growth: words[2],
                        dest: words[3],
                        other: words[4],
                        last: words[5],
                    };
                    c.gpr[0] = glow::blur(&Eng(&*m), &args, 0x1f80).map(|b| b.result).unwrap_or(u32::MAX);
                }
                WIDE => {
                    let words: [u32; 6] = std::array::from_fn(|i| stack_arg(m, c, i as u32));
                    did.push(Did::Wide(c.gpr[0], c.gpr[1] as i32, c.xmm[0] as u32, words));
                    let args = WideArgs {
                        source: c.gpr[0],
                        taps: c.gpr[1] as i32,
                        scale: c.xmm[0] as u32,
                        words: [words[0], words[1]],
                        dest: words[2],
                        other: words[3],
                        last: words[4],
                    };
                    c.gpr[0] = glow::wide(&Eng(&*m), &args, 0x1f80).map(|w| w.result).unwrap_or(u32::MAX);
                }
                END => ends += 1,
                _ => {}
            };
            lifted
                .run(&mut c, &mut m, &[HARNESS_EXIT], &stubs, &mut record)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            let pushed: Vec<u32> = (BUF..word(&m, PB)).step_by(4).map(|a| word(&m, a)).collect();
            let Some(want) = want else {
                assert!(
                    did.is_empty() && pushed.is_empty() && ends == 0,
                    "seed {seed} case {n}: the plan returns at once, the original {did:x?}"
                );
                *seen.entry("returned".into()).or_default() += 1;
                continue;
            };
            let mut expect = expected(&want);
            expect.extend(restored);
            assert_eq!(did, expect, "seed {seed} case {n}: the calls\nplan {want:#x?}");
            let words: Vec<u32> = want.passes.iter().flat_map(|p| p.pushed(depth).concat()).collect();
            assert_eq!(pushed, words, "seed {seed} case {n}: the strips\nplan {want:#x?}");
            assert_eq!(ends, want.passes.iter().map(|p| p.strips.len()).sum::<usize>(), "seed {seed} case {n}: Ends");
            *seen.entry(format!("{} passes", want.passes.len().min(2))).or_default() += 1;
            for p in &want.passes {
                *seen.entry(format!("kind {}", p.kind)).or_default() += 1;
                for c in &p.convolutions {
                    let k = match c {
                        Convolution::Kept { .. } => "kept",
                        Convolution::Blur { .. } => "blur",
                        Convolution::Wide { .. } => "wide",
                    };
                    *seen.entry(k.into()).or_default() += 1;
                }
            }
        }
    }
    eprintln!("effect plans: {seen:#?}");
    for k in [
        "returned", "0 passes", "1 passes", "2 passes", "kind 0", "kind 1", "kind 2", "kind 3", "kept", "blur", "wide",
    ] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
