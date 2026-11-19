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
//!   and the four values), over random gates and entries.
//!
//! The evaluator's wave and shape tables are the image's (read from it,
//! never copied). Results are bit-equal, except where the evaluator port
//! (`tags::effects::function`) documents an approximation: type 9 (an f64
//! power for the x87's) and type 10 (an exact reciprocal square root for
//! `rsqrtss`), which must agree within 1e-3. Needs the user's XBE
//! (`H2_XBE`, or `disc/default.xbe`), pinned by hash; skips without it.
//! `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test screen_effects_differential -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::memory::{GuestMemory, MemFault};
use engine::screen_effects::{self as se, Drawn};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

const FUNCTION_VALUE: u32 = 0x0013_4fc0;
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
        seen: &mut dyn FnMut(u32, &Cpu, &FlatMemory),
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

/// Whether `a` and `b` agree: bit-equal, or within 1e-3 (both NaN
/// counting) when either type is one the evaluator port approximates.
fn agree(a: u32, b: u32, kinds: (u8, u8)) -> bool {
    if a == b {
        return true;
    }
    let approximate = [9, 10].contains(&kinds.0) || [9, 10].contains(&kinds.1);
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    approximate && ((x.is_nan() && y.is_nan()) || (x - y).abs() <= 1e-3 * x.abs().max(y.abs()).max(1.0))
}

#[test]
fn function_values_match_the_lifted_original() {
    if !with_image(function_cases) {
        eprintln!("SKIPPED function_values_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn function_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(FUNCTION_VALUE);
    let tables = se::tables(&Eng(image)).expect("the image's tables");
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut wrong: Vec<String> = Vec::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            let count = 1 + r.below(6);
            let kinds = functions(&mut r, &mut m, count);
            for index in 0..count {
                let want = se::function_value(&Eng(&m), index as i32, 0x1f80, &tables);
                let mut a = m.clone();
                let esp = STACK_TOP - 0x400;
                put(&mut a, esp, HARNESS_EXIT);
                let mut c = cpu(&mut r, esp, FUNCTION_VALUE);
                c.gpr[1] = index;
                let none = Stubs::new();
                let ran = lifted.run(&mut c, &mut a, &[HARNESS_EXIT], &none, &mut |_, _, _| {});
                let got = c.xmm[0] as u32;
                let k = kinds[index as usize];
                match (want, ran) {
                    (Ok(w), Ok(_)) => {
                        if !agree(w, got, k) {
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
    let tables = se::tables(&Eng(image)).expect("the image's tables");
    let stubs: Stubs = [(SCREEN_EFFECT, 0x10)].into_iter().collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n));
            let mut m = image.clone();
            let count = 1 + r.below(4);
            let kinds = functions(&mut r, &mut m, count);
            // Only bit-exact types: an entry's draw follows its values.
            if kinds.iter().any(|k| [9, 10].contains(&k.0) || [9, 10].contains(&k.1)) {
                continue;
            }
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
            let want: Vec<(u32, [u32; 4])> = se::drawn(&Eng(&m), 0x1f80, &tables)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"))
                .into_iter()
                .map(|Drawn { datum, values, .. }| (datum, values))
                .collect();

            let mut a = m.clone();
            let esp = STACK_TOP - 0x400;
            let mut c = cpu(&mut r, esp, LOOP.0);
            let mut got = Vec::new();
            let mut record = |pc: u32, c: &Cpu, m: &FlatMemory| {
                if pc == SCREEN_EFFECT {
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
