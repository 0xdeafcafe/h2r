//! The push-buffer callback `0x00024970` never runs on the retail image
//! (docs/engine/sim-render-split.md, coupling 5).
//!
//! `0x000245c0` inserts it (`0x0002460e`) only for a mode in `BX` of 1 to
//! 3. Its one caller, `render_screen_effects` at `0x000138a8`, clears EBX
//! first (`0x000138a1`), so every call takes the reset branch
//! (`0x00024642`): the screen record at `[0x00485890]` gets the same six
//! floats each frame and no texel is read back for it.
//!
//! - `the_only_call_passes_mode_zero`: the image holds one `call` or `jmp`
//!   to `0x000245c0`, at `0x000138a8` after `xor ebx, ebx` and
//!   `mov eax, 0x0051ecf0`, and no word holding its address; the
//!   callback's address appears only as the insert's immediate.
//! - `mode_zero_resets_the_record`: `0x000245c0` lifted and run on the IR
//!   interpreter from any record and counter with BX 0 reaches the call
//!   of `0x00024ee0` without passing the insert, the record holding
//!   `{0, [0x0045dc2c], [0x0045dd30]}` twice, EAX at `+0x18` and the
//!   counter 0. With BX 1 to 3 and the counter's low bits clear it takes
//!   the insert path, so the run tells the two apart.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it screen_record_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 10_000;

/// `0x000245c0`: ESI the record, EAX the histogram, BX the mode.
const RECORD_UPDATE: u32 = 0x0002_45c0;
/// Its one call, in `render_screen_effects`.
const CALL_SITE: u32 = 0x0001_38a8;
/// `xor ebx, ebx; mov eax, 0x0051ecf0` just before it.
const CALL_SETUP: u32 = 0x0001_38a1;
const CALL_SETUP_BYTES: [u8; 7] = [0x33, 0xdb, 0xb8, 0xf0, 0xec, 0x51, 0x00];
/// The callback and the insert's word that holds its address.
const CALLBACK: u32 = 0x0002_4970;
const INSERT_FUNCTION_WORD: u32 = 0x0002_4617;
/// The histogram `render_screen_effects` passes in EAX.
const HISTOGRAM: u32 = 0x0051_ecf0;
/// The mode check passed with the counter's low bits clear: the downsample
/// and the insert follow.
const INSERT_PATH: u32 = 0x0002_45d9;
/// The call of `0x00024ee0` after either branch.
const RECORD_READER_CALL: u32 = 0x0002_4688;
/// The counter `+0x20` with bits 0..2 set: neither the insert nor the
/// reader, straight to the increment.
const SKIP: u32 = 0x0002_468d;
/// The reset's floats: `+4` and `+0x10`, `+8` and `+0x14`.
const RESET_A: u32 = 0x0045_dc2c;
const RESET_B: u32 = 0x0045_dd30;

const RECORD: u32 = 0x0308_0000;

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

/// Every `call rel32` (`e8`) or `jmp rel32` (`e9`) in the loaded image
/// whose target is `target`, and every aligned or unaligned word equal to
/// it. Data bytes can only add sites, so an exact answer is a bound.
fn references(image: &FlatMemory, target: u32) -> (Vec<u32>, Vec<u32>) {
    let bytes = &image.bytes[..RAM.min(image.bytes.len())];
    let mut branches = Vec::new();
    let mut words = Vec::new();
    for at in 0..bytes.len().saturating_sub(5) {
        let w = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        if w == target {
            words.push(at as u32);
        }
        if matches!(bytes[at], 0xe8 | 0xe9) {
            let rel = i32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap());
            if (at as u32).wrapping_add(5).wrapping_add(rel as u32) == target {
                branches.push(at as u32);
            }
        }
    }
    (branches, words)
}

#[test]
fn the_only_call_passes_mode_zero() {
    if !with_image(|image, _| only_call_cases(image)) {
        eprintln!("SKIPPED the_only_call_passes_mode_zero: no XBE (set H2_XBE)");
    }
}

fn only_call_cases(image: &FlatMemory) {
    let (branches, words) = references(image, RECORD_UPDATE);
    assert_eq!(branches, vec![CALL_SITE], "the branches to {RECORD_UPDATE:#x}");
    assert!(words.is_empty(), "words holding {RECORD_UPDATE:#x} at {words:#x?}");
    let setup = &image.bytes[CALL_SETUP as usize..CALL_SETUP as usize + CALL_SETUP_BYTES.len()];
    assert_eq!(setup, &CALL_SETUP_BYTES[..], "xor ebx, ebx; mov eax, {HISTOGRAM:#x}");
    let (branches, words) = references(image, CALLBACK);
    assert!(branches.is_empty(), "branches to the callback at {branches:#x?}");
    assert_eq!(words, vec![INSERT_FUNCTION_WORD], "words holding the callback's address");
}

#[test]
fn mode_zero_resets_the_record() {
    if !with_image(mode_cases) {
        eprintln!("SKIPPED mode_zero_resets_the_record: no XBE (set H2_XBE)");
    }
}

fn mode_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(RECORD_UPDATE);
    let (a, b) = (word(image, RESET_A), word(image, RESET_B));
    let exits = [INSERT_PATH, RECORD_READER_CALL, SKIP];
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut m = image.clone();
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            for off in (0..0x24).step_by(4) {
                let v = r.u32();
                put(&mut m, RECORD + off, v);
            }
            let counter = word(&m, RECORD + 0x20);
            // Mode 0 with junk above BX most of the time, else 1..3.
            let mode = if r.below(3) == 0 { 1 + r.below(3) } else { 0 };
            let ebx = (r.u32() & 0xffff_0000) | mode;
            let mut c = Cpu {
                gpr: [HISTOGRAM, r.u32(), r.u32(), ebx, STACK_TOP - 0x400, r.u32(), RECORD, r.u32()],
                fs_base: RAM as u32,
                eip: RECORD_UPDATE,
                ..Cpu::default()
            };
            c.mxcsr = 0x1f80;
            let exit = lifted
                .run(&mut c, &mut m, &exits)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(word(&m, RECORD + 0x18), HISTOGRAM, "seed {seed} case {n}: +0x18");
            if mode == 0 {
                assert_eq!(exit, RECORD_READER_CALL, "seed {seed} case {n}: mode 0 left at {exit:#x}");
                let six: Vec<u32> = (0..6).map(|i| word(&m, RECORD + 4 * i)).collect();
                assert_eq!(six, vec![0, a, b, 0, a, b], "seed {seed} case {n}: the reset");
                assert_eq!(word(&m, RECORD + 0x20), 0, "seed {seed} case {n}: the counter");
                *seen.entry("mode 0, reset").or_default() += 1;
            } else if counter & 7 == 0 {
                assert_eq!(exit, INSERT_PATH, "seed {seed} case {n}: mode {mode} left at {exit:#x}");
                *seen.entry("mode 1..3, insert").or_default() += 1;
            } else {
                // Mode 2 still calls the reader; 1 and 3 skip it.
                let want = if mode == 2 { RECORD_READER_CALL } else { SKIP };
                assert_eq!(exit, want, "seed {seed} case {n}: mode {mode} left at {exit:#x}");
                *seen.entry("mode 1..3, no insert").or_default() += 1;
            }
        }
    }
    eprintln!("screen record: {seen:#?}");
    for k in ["mode 0, reset", "mode 1..3, insert", "mode 1..3, no insert"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
