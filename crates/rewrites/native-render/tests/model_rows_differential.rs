//! `native_render::world::model_rows` against the original: the structure
//! draw's model-row upload `0x0004d830`, lifted and run on the IR
//! interpreter with `EBX` a render record whose `+0x1c` points at a seeded
//! placement, must hand the constant upload `0x003f7330` the same twelve
//! floats, bit for bit, that the port reads from the same bytes (the
//! native pass uploads them per placement at `c[50..52]`).
//!
//! The upload is intercepted, not run: at its entry the test checks the
//! register (`ECX = 0x32`, hardware `c[50]`), the count pushed (`0xc`
//! floats) and takes the floats at `EDX`, then returns as it would
//! (`stdcall`, one argument). The placement's bytes are any bits: zeros,
//! infinities, NaNs with payloads, denormals, plain values.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it. `H2_DIFF_SEEDS=1,2,3` picks
//! seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p native-render --features differential \
//!     --test model_rows_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

#[path = "../../engine/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};
use native_render::world::{PLACEMENT_ROW_BYTES, model_rows};

const RAM: u32 = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 20_000;

/// The model-row upload.
const MODEL_ROW_UPLOAD: u32 = 0x0004_d830;
/// The vertex constant upload it calls (`ECX` the register, `EDX` the
/// floats, the count pushed).
const SET_CONSTANTS: u32 = 0x003f_7330;
/// Where the native pass puts the rows (`native_render::program::MODEL_ROWS`,
/// hardware numbering).
const MODEL_ROWS: u32 = 50;
/// The render record's pointer to the placement.
const RECORD_PLACEMENT: u32 = 0x1c;

const RECORD: u32 = 0x0300_0000;
const PLACEMENT: u32 = 0x0300_1000;
/// A placement element's size (`sbsp+0x144`, `0x58` each).
const PLACEMENT_SIZE: usize = 0x58;

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
        match self.below(24) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000 | (self.u32() & 0x8000_0000),
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => 0x7f80_0001 | (self.u32() & 0x803f_ffff),
            5 => self.u32() & 0x807f_ffff,
            6 => self.u32(),
            _ => {
                let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
                ((unit * 2.0 - 1.0) * 4096.0).to_bits()
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

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    /// Runs the upload with `EBX = RECORD`; returns the floats handed to
    /// the constant upload, and the register and count it was given.
    fn run(&mut self, mem: &mut FlatMemory) -> Result<([u32; 12], u32, u32), String> {
        let esp = STACK_TOP - 0x400;
        put(mem, esp, HARNESS_EXIT);
        let mut cpu = Cpu {
            gpr: [0, 0, 0, RECORD, esp, 0, 0, 0],
            fs_base: RAM,
            eip: MODEL_ROW_UPLOAD,
            ..Cpu::default()
        };
        let mut pc = MODEL_ROW_UPLOAD;
        let mut steps = 0;
        let mut upload = None;
        while pc != HARNESS_EXIT {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
            }
            if pc == SET_CONSTANTS {
                if upload.is_some() {
                    return Err("two uploads".into());
                }
                let (ecx, edx, sp) = (cpu.gpr[1], cpu.gpr[2], cpu.gpr[4]);
                let floats: [u32; 12] = std::array::from_fn(|i| word(mem, edx + 4 * i as u32));
                upload = Some((floats, ecx, word(mem, sp + 4)));
                // `ret 4`.
                pc = word(mem, sp);
                cpu.gpr[4] = sp + 8;
                cpu.eip = pc;
                continue;
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
        upload.ok_or_else(|| "no upload".into())
    }
}

#[test]
fn model_rows_match_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED model_rows_match_the_lifted_original: no XBE (set H2_XBE)");
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
    let mut cases = 0;
    for &seed in &seeds {
        for n in 0..256u32 {
            let mut mem = FlatMemory {
                base: image.base,
                bytes: image.bytes.clone(),
            };
            let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ u64::from(n));
            let mut element = vec![0u8; PLACEMENT_SIZE];
            for i in 0..PLACEMENT_SIZE / 4 {
                let v = r.float();
                element[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut mem, PLACEMENT + 4 * i as u32, v);
            }
            // The record's other words: any bits, the placement pointer set.
            for i in 0..8 {
                let v = r.u32();
                put(&mut mem, RECORD + 4 * i, v);
            }
            put(&mut mem, RECORD + RECORD_PLACEMENT, PLACEMENT);
            let (original, register, count) =
                lifted.run(&mut mem).unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(register, MODEL_ROWS, "seed {seed} case {n}: the register");
            assert_eq!(count, 12, "seed {seed} case {n}: the float count");
            let port = model_rows(&element[..PLACEMENT_ROW_BYTES]).expect("the element is long enough");
            let port: Vec<u32> = port.iter().flatten().map(|f| f.to_bits()).collect();
            assert_eq!(&port[..], &original[..], "seed {seed} case {n}: port vs original");
            cases += 1;
        }
    }
    eprintln!("cases: {cases}");
}
