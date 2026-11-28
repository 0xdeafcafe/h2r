//! `engine::light_passes` against the original, lifted and run on the IR
//! interpreter from the same seeded 64 MB state:
//!
//! - the flag code of `render_light_passes` (`0x000311c8`..`0x0003128e`,
//!   run from the return of its `0x0004a1e0` call to either successor of
//!   the `je` at `0x00031293`) must leave memory byte for byte as
//!   `pass_flags` plus `PassFlags::write` do;
//! - layer 12's chooser `0x00030da0` and layer 13's `0x00030e00` must
//!   return what `diffuse` and `specular` do, and pop their arguments.
//!
//! Each case seeds the context's first word, the definition's words `+4`
//! and `+0x14` and first byte, the override pointer and its byte `+5`,
//! colour B near the image's threshold, falloff word 4, the old flags, the
//! shadow byte (with junk in EBX's upper bytes) and the choosers' arguments
//! (junk above their low bytes), under one of several MXCSR words.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it. `H2_DIFF_SEEDS=1,2,3`
//! picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it light_passes_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_constants::{self as lc, FRAME_WORDS, LightGlobals};
use engine::light_passes::{
    self as lp, CHOOSE_DIFFUSE, CHOOSE_DIFFUSE_CLEANUP, CHOOSE_SPECULAR, CHOOSE_SPECULAR_CLEANUP, FLAGS_EXITS,
    FLAGS_START, RENDER_LIGHT_PASSES, constant, flag, global,
};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 10_000;

const CONTEXT: u32 = 0x0302_0000;
const DEFINITION: u32 = 0x0303_0000;
const OVERRIDE: u32 = 0x0304_0000;

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
    /// Near `around` (its bits), sometimes exactly it, sometimes an edge.
    fn near(&mut self, around: u32) -> u32 {
        match self.below(12) {
            0 => around,
            1 => 0,
            2 => 0x8000_0000,
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => around.wrapping_add(1),
            6 => around.wrapping_sub(1),
            _ => {
                let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
                (f32::from_bits(around) * unit * 2.0).to_bits()
            }
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

fn mxcsr(r: &mut Rng) -> u32 {
    match r.below(5) {
        0 => 0x1f80 | 0x0040,
        1 => 0x1f80 | 0x8000,
        2 => 0x1f80 | 0x6000,
        _ => 0x1f80,
    }
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
            let insn = self
                .cache
                .get(&pc)
                .ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        Ok(pc)
    }
}

fn globals(mem: &FlatMemory) -> LightGlobals {
    let w = |a: u32| word(mem, a);
    let mut falloff = [0u32; 5];
    for (i, f) in falloff.iter_mut().enumerate() {
        *f = w(lc::global::FALLOFF + 4 * i as u32);
    }
    LightGlobals {
        definition: DEFINITION,
        context_word: w(CONTEXT) as u16,
        driver: 0,
        colour_a: [0; 3],
        colour_b: [0, 4, 8].map(|o| w(lc::global::COLOUR_B + o)),
        scale: 0,
        falloff,
        frame: [0; FRAME_WORDS],
        frame_copy_a: [0; 4],
        frame_copy_b: [0; 4],
        mode: 0,
        rectangle: [0; 6],
        rectangle_valid: false,
        sphere: None,
        shadow: 0,
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

#[test]
fn pass_flags_match_the_lifted_original() {
    if !with_image(pass_flags_cases) {
        eprintln!("SKIPPED pass_flags_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn pass_flags_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    // The flag code starts at a call's return inside the function: lift
    // the whole function first.
    lifted.lift(RENDER_LIGHT_PASSES);
    let threshold = word(image, constant::SPECULAR);

    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut a = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);

            let v = r.u32();
            put(&mut a, CONTEXT, if r.below(2) == 0 { v & 0xffff_0000 } else { v });
            put16(&mut a, DEFINITION + 4, if r.below(2) == 0 { 0 } else { r.u32() as u16 });
            put16(&mut a, DEFINITION + 0x14, if r.below(3) == 0 { 2 } else { r.u32() as u16 });
            let over = match r.below(3) {
                0 => 0,
                _ => OVERRIDE,
            };
            put(&mut a, global::OVERRIDE, over);
            a.bytes[(OVERRIDE + 5) as usize] = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            for i in 0..3 {
                let v = if r.below(3) == 0 { 0 } else { r.near(threshold) };
                put(&mut a, lc::global::COLOUR_B + 4 * i, v);
            }
            let v = r.near(0x3f80_0000);
            put(&mut a, lc::global::FALLOFF + 16, v);
            let v = r.u32();
            put(&mut a, global::PASS_FLAGS, v);
            a.bytes[global::PASSES as usize] = r.u32() as u8;
            let shadow = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            let ebx = (r.u32() & !0xff) | shadow as u32;
            let mx = mxcsr(&mut r);

            let mut b = a.clone();
            let g = globals(&b);
            let f = lp::pass_flags(&Eng(&mut b), &g, shadow, mx)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            f.write(&mut Eng(&mut b)).unwrap();

            let mut cpu = Cpu {
                gpr: [r.u32(), r.u32(), r.u32(), ebx, STACK_TOP - 0x400, r.u32(), DEFINITION, CONTEXT],
                fs_base: RAM as u32,
                eip: FLAGS_START,
                ..Cpu::default()
            };
            cpu.mxcsr = mx;
            let exit = lifted
                .run(&mut cpu, &mut a, &FLAGS_EXITS)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(exit == FLAGS_EXITS[0], f.shadowed(), "seed {seed} case {n}: exit {exit:#x}");

            if let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {n}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)\n{f:#x?}",
                    a.bytes[i], b.bytes[i]
                );
            }
            for (k, on) in [
                ("context zero", f.flags & flag::CONTEXT_ZERO != 0),
                ("specular", f.flags & flag::SPECULAR != 0),
                ("shadow", f.shadowed()),
                ("shadow falloff", f.flags & flag::SHADOW_FALLOFF != 0),
                ("colour b zeroed", f.colour_b_zeroed),
                ("plain", f.flags == 0),
            ] {
                if on {
                    *seen.entry(k).or_default() += 1;
                }
            }
        }
    }
    eprintln!("pass_flags: all equal; {seen:#?}");
    for k in ["context zero", "specular", "shadow", "shadow falloff", "colour b zeroed", "plain"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

#[test]
fn choosers_match_the_lifted_original() {
    if !with_image(chooser_cases) {
        eprintln!("SKIPPED choosers_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn chooser_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {

    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut a = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ n as u64);
            let flags = r.u32();
            put(&mut a, global::PASS_FLAGS, flags);
            put(&mut a, global::DEFINITION, DEFINITION);
            let def = r.u32() as u8;
            a.bytes[DEFINITION as usize] = def;
            let input = r.below(2) == 0;
            let a1 = (r.u32() & !0xff) | input as u32;
            let a2 = (r.u32() & !0xff) | 1;
            let esp = STACK_TOP - 0x400;
            put(&mut a, esp, HARNESS_EXIT);
            put(&mut a, esp + 4, a1);
            put(&mut a, esp + 8, a2);

            for (entry, cleanup) in [
                (CHOOSE_DIFFUSE, CHOOSE_DIFFUSE_CLEANUP),
                (CHOOSE_SPECULAR, CHOOSE_SPECULAR_CLEANUP),
            ] {
                let mut m = a.clone();
                let mut cpu = Cpu {
                    gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
                    fs_base: RAM as u32,
                    eip: entry,
                    ..Cpu::default()
                };
                lifted
                    .run(&mut cpu, &mut m, &[HARNESS_EXIT])
                    .unwrap_or_else(|e| panic!("seed {seed} case {n} {entry:#x}: {e}"));
                assert_eq!(cpu.gpr[4], esp + 4 + cleanup, "seed {seed} case {n} {entry:#x}: esp");
                let want = if entry == CHOOSE_DIFFUSE {
                    lp::diffuse(flags, def, input)
                } else {
                    lp::specular(flags, input).unwrap_or(u32::MAX)
                };
                assert_eq!(
                    cpu.gpr[0], want,
                    "seed {seed} case {n} {entry:#x}: flags {flags:#x} definition {def:#x} input {input}"
                );
                *seen.entry(format!("{entry:#x} -> {want:#x}")).or_default() += 1;
            }
        }
    }
    eprintln!("choosers: all equal; {seen:#?}");
    for k in 0..5 {
        let key = format!("{CHOOSE_DIFFUSE:#x} -> {k:#x}");
        assert!(seen.contains_key(&key), "{key} never seen");
    }
    for k in [0, u32::MAX] {
        let key = format!("{CHOOSE_SPECULAR:#x} -> {k:#x}");
        assert!(seen.contains_key(&key), "{key} never seen");
    }
}
