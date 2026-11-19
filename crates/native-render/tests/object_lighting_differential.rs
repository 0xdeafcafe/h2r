//! `native_render::object_lighting` against the original: the vector input
//! evaluator `0x00033980` and the scalar one `0x000336f0`, lifted and run
//! on the IR interpreter from a seeded state, must give, for every
//! lighting input the port takes, the same bits the port gives from the
//! same memory (a NaN matches any NaN: the port's float arithmetic is not
//! bound to the original's NaN payloads).
//!
//! Each case seeds the current lighting pointer `[0x00485a58]` (null now
//! and then), the lighting's `0x78` bytes with any float bits (zeros,
//! infinities, NaNs, denormals), its colour count byte, the two default
//! records and the shader's two values, under one of three x87 precisions
//! (the colour unpack `0x00131bb0` runs on the x87).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it: the constants the code
//! reads stay in the image. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p native-render --features differential \
//!     --test object_lighting_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

#[path = "../../engine/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};
use native_render::camera::GuestRead;
use native_render::object_lighting::{
    self as ol, CURRENT_LIGHTING, DEFAULT_COLOUR, DEFAULT_VECTOR, Defaults, LIGHTING_BYTES, Lighting, SCALAR_INPUTS,
    SHADER_SCALAR_21, SHADER_VECTOR_24, ShaderValues, VECTOR_INPUTS,
};

const RAM: u32 = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 20_000;

const LIGHTING: u32 = 0x0300_0000;
const VECTOR_RECORD: u32 = 0x0300_1000;
const COLOUR: u32 = 0x0300_2000;
const OUT: u32 = 0x0300_3000;

/// The vector inputs the port takes.
const VECTORS: [u32; 15] = [3, 4, 14, 15, 16, 17, 21, 22, 23, 24, 35, 42, 43, 44, 45];

struct Read<'a>(&'a FlatMemory);

impl GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        let a = addr as usize;
        Some(u32::from_le_bytes(self.0.bytes.get(a..a + 4)?.try_into().ok()?))
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
    /// Mostly modest floats, sometimes the edges.
    fn float(&mut self) -> u32 {
        match self.below(40) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000 | (self.u32() & 0x8000_0000),
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => self.u32(),
            _ => {
                let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
                ((unit * 2.0 - 1.0) * 3.0).to_bits()
            }
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

/// One seeded state; the x87 control word to run under.
fn case(image: &FlatMemory, seed: u64, n: u32) -> (FlatMemory, u16) {
    let mut mem = FlatMemory {
        base: image.base,
        bytes: image.bytes.clone(),
    };
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ u64::from(n));
    for i in 0..LIGHTING_BYTES / 4 {
        let v = r.float();
        put(&mut mem, LIGHTING + 4 * i, v);
    }
    // The colour words: packed colours mostly; the count byte 0..5.
    for slot in 0..4 {
        let v = if r.below(4) == 0 { r.u32() } else { r.u32() & 0x00ff_ffff };
        put(&mut mem, LIGHTING + 0x64 + 4 * slot, v);
    }
    let count = r.u32() & 0xffff_ff00 | r.below(6);
    put(&mut mem, LIGHTING + 0x74, count);
    put(&mut mem, CURRENT_LIGHTING, if r.below(6) == 0 { 0 } else { LIGHTING });
    put(&mut mem, DEFAULT_VECTOR, VECTOR_RECORD);
    put(&mut mem, DEFAULT_COLOUR, COLOUR);
    for i in 0..4 {
        let (a, b) = (r.float(), r.float());
        put(&mut mem, VECTOR_RECORD + 4 * i, a);
        put(&mut mem, COLOUR + 4 * i, b);
    }
    let (s21, s24) = (r.float(), r.float());
    put(&mut mem, SHADER_SCALAR_21, s21);
    put(&mut mem, SHADER_VECTOR_24, s24);
    let x87 = [0x037f, 0x027f, 0x007f][r.below(3) as usize];
    (mem, x87)
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    /// Runs the evaluator at `entry` with `EAX = input`, `ESI = OUT`;
    /// returns the CPU at its return.
    fn run(&mut self, mem: &mut FlatMemory, entry: u32, input: u32, x87: u16) -> Result<Cpu, String> {
        let esp = STACK_TOP - 0x400;
        put(mem, esp, HARNESS_EXIT);
        let mut cpu = Cpu {
            gpr: [input, 0, 0, 0, esp, 0, OUT, 0],
            fs_base: RAM,
            eip: entry,
            ..Cpu::default()
        };
        cpu.x87.control = x87;
        let mut pc = entry;
        let mut steps = 0;
        while pc != HARNESS_EXIT {
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
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(&mut cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        if cpu.gpr[4] != esp + 4 {
            return Err(format!("esp at return {:#x}", cpu.gpr[4]));
        }
        Ok(cpu)
    }
}

fn same(a: u32, b: u32) -> bool {
    a == b || (f32::from_bits(a).is_nan() && f32::from_bits(b).is_nan())
}

#[test]
fn lighting_inputs_match_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED lighting_inputs_match_the_lifted_original: no XBE (set H2_XBE)");
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
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for &seed in &seeds {
        for n in 0..64 {
            let (base, x87) = case(&image, seed, n);
            // The port's view of the same state.
            let read = Read(&base);
            let lighting = match word(&base, CURRENT_LIGHTING) {
                0 => None,
                at => Some(Lighting::read(&read, at).unwrap()),
            };
            let defaults = Defaults::read(&read).unwrap();
            let shader = ShaderValues {
                scalar_21: word(&base, SHADER_SCALAR_21),
                vector_24: word(&base, SHADER_VECTOR_24),
            };
            *seen.entry(if lighting.is_some() { "lit" } else { "no lighting" }).or_default() += 1;
            for input in VECTORS {
                let mut mem = FlatMemory {
                    base: base.base,
                    bytes: base.bytes.clone(),
                };
                lifted
                    .run(&mut mem, VECTOR_INPUTS, input, x87)
                    .unwrap_or_else(|e| panic!("seed {seed} case {n} vector {input}: {e}"));
                let original = [word(&mem, OUT), word(&mem, OUT + 4), word(&mem, OUT + 8)];
                let port = ol::vector_input(input, lighting.as_ref(), &defaults, shader).expect("ported");
                assert!(
                    original.iter().zip(port).all(|(&a, b)| same(a, b)),
                    "seed {seed} case {n} vector {input}: original {original:08x?}, port {port:08x?}"
                );
            }
            let mut mem = FlatMemory {
                base: base.base,
                bytes: base.bytes.clone(),
            };
            let cpu = lifted
                .run(&mut mem, SCALAR_INPUTS, 23, x87)
                .unwrap_or_else(|e| panic!("seed {seed} case {n} scalar 23: {e}"));
            let original = cpu.xmm[0] as u32;
            let port = ol::scalar_input(23, lighting.as_ref()).expect("ported");
            assert!(same(original, port), "seed {seed} case {n} scalar 23: original {original:08x}, port {port:08x}");
        }
    }
    eprintln!("cases: {seen:?}");
    assert!(seen.len() == 2, "both with and without a lighting: {seen:?}");
}
