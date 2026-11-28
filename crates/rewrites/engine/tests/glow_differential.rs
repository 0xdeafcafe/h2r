//! `engine::glow` against the original, lifted and run on the IR
//! interpreter from the same seeded 64 MB state:
//!
//! - `blur_combiner`: `0x00020f30` from its entry (XMM0 `y`, XMM3 the
//!   strength) to its last store (`0x0002116a`); the 60-word block at
//!   `0x00484f68` must be the port's.
//! - `pass_a`: `0x000214f0` from `0x00021775` to its `0x0001ccf0` call
//!   (`0x000218d5`); `pass_b`: from `0x0002198f` to `0x00021a16`. The
//!   block must be the port's (`0x00131e50` runs lifted inside).
//! - `blur`: `0x000211a0` from its entry to its return, with the calls the
//!   port does not take over stubbed (`0x00020f30`, the bind `0x00014f60`,
//!   the quad draw `0x00034a90`, the copy `0x000351a0`; each returns with
//!   its cleanup and is recorded): the draws (the target bound at stage 0,
//!   the target drawn into, the spread argument), the copy and the
//!   returned target must be the port's. Steps at infinity: the
//!   original never leaves its loop (it runs to the step limit), the port
//!   stops at `glow::MAX_BLUR_STEPS` with `Error::Unbounded`.
//! - `wide`: `0x00021360` from its entry to its return, stubbed the same
//!   way (`0x00020f30`, the bind, its quad draw `0x00034d90`, the copy):
//!   the draws (the target bound at stage 0, EAX the target drawn into,
//!   the four offsets EDI points at, and the quad's stack words), whether
//!   and how it set the combiner, the copy and the returned target must
//!   be the port's. Past `glow::MAX_WIDE_TAPS` the port stops with
//!   `Error::Unbounded` and the original draws every tap.
//!
//! Each case seeds the floats with now and then an edge (zeros, signed
//! zero, infinities, NaNs, denormals) under one of several MXCSR and x87
//! control words. Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`),
//! pinned by hash; skips without it. Nothing is copied from it.
//! `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it glow_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::colour_pass::Parameters;
use engine::glow::{self, BlurArgs, BlurDraw, PSDEF, PSDEF_WORDS, WideArgs, WideDraw};
use engine::light_constants::Error;
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 100_000;

const BLUR_COMBINER: u32 = 0x0002_0f30;
/// After its last store into the block (`0x0002116a`).
const BLUR_COMBINER_EXIT: u32 = 0x0002_116f;
const COLOUR_PASS: u32 = 0x0002_14f0;
const PASS_A: (u32, u32) = (0x0002_1775, 0x0002_18d5);
const PASS_B: (u32, u32) = (0x0002_198f, 0x0002_1a16);
const BLUR: u32 = 0x0002_11a0;
const BIND: u32 = 0x0001_4f60;
const QUAD: u32 = 0x0003_4a90;
const COPY: u32 = 0x0003_51a0;
const WIDE: u32 = 0x0002_1360;
const WIDE_QUAD: u32 = 0x0003_4d90;
const DIRTY: u32 = 0x0040_7490;

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
    /// A float in 0..`top` mostly, and now and then an edge.
    fn float(&mut self, top: f32) -> u32 {
        match self.below(14) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000,
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => 0x3f80_0000,
            6 => (-(self.below(1000) as f32) / 500.0).to_bits(),
            _ => (self.below(1 << 20) as f32 / (1 << 20) as f32 * top).to_bits(),
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

fn block(mem: &FlatMemory) -> Vec<u32> {
    (0..PSDEF_WORDS as u32).map(|i| word(mem, PSDEF + 4 * i)).collect()
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

    /// Steps from `cpu.eip` until one of `exits`; a stubbed entry is
    /// handed to `seen` and returns at once with its cleanup.
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

fn cpu(r: &mut Rng, esp: u32, eip: u32, mx: u32, cw: u16) -> Cpu {
    let mut c = Cpu {
        gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), r.u32()],
        fs_base: RAM as u32,
        eip,
        ..Cpu::default()
    };
    c.mxcsr = mx;
    c.x87.control = cw;
    c
}

fn parameters(second: u32, third: u32, x: u32) -> Parameters {
    Parameters {
        args: [0, second, third, 0, 0, 0, 0],
        x,
        y: 0,
        draws: true,
        reset: None,
        unset: 0,
    }
}

fn no_stubs() -> (Stubs, impl FnMut(u32, &Cpu, &FlatMemory)) {
    (Stubs::new(), |_: u32, _: &Cpu, _: &FlatMemory| {})
}

#[test]
fn combiners_match_the_lifted_original() {
    if !with_image(combiner_cases) {
        eprintln!("SKIPPED combiners_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn combiner_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(COLOUR_PASS);
    lifted.lift(BLUR_COMBINER);
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let (mx, cw) = (mxcsr(&mut r), x87_control(&mut r));
            let esp = STACK_TOP - 0x400;
            let mut a = image.clone();
            a.bytes[glow::COMBINER_BYTE as usize] = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            // Junk in the block: every word must be written or cleared.
            for i in 0..PSDEF_WORDS as u32 {
                let v = r.u32();
                put(&mut a, PSDEF + 4 * i, v);
            }

            // The blur's combiner.
            let (strength, y) = (r.float(1.5), r.float(1.5));
            let want = glow::blur_combiner(&Eng(&mut a.clone()), strength, y, mx, cw)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            let mut m = a.clone();
            let mut c = cpu(&mut r, esp, BLUR_COMBINER, mx, cw);
            c.xmm[0] = u128::from(y);
            c.xmm[3] = u128::from(strength);
            let (stubs, mut none) = no_stubs();
            lifted
                .run(&mut c, &mut m, &[BLUR_COMBINER_EXIT], &stubs, &mut none)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(block(&m), want.0.to_vec(), "seed {seed} case {n}: blur combiner {strength:#x} {y:#x}");
            *seen.entry(format!("blur stages {}", want.0[(0x0048_503c - PSDEF) as usize / 4] & 0xf)).or_default() += 1;

            // The two passes' combiners: the frame `0x000214f0` has there
            // (EBP the dirty bits it stores back, left as they are).
            let (second, third, x) = (r.float(1.0), r.float(1.0), r.float(1.0));
            let p = parameters(second, third, x);
            for (pass, (start, exit)) in [("a", PASS_A), ("b", PASS_B)] {
                let want = if pass == "a" {
                    glow::pass_a(&Eng(&mut a.clone()), &p, cw)
                } else {
                    glow::pass_b(&Eng(&mut a.clone()), &p, cw)
                }
                .unwrap_or_else(|e| panic!("seed {seed} case {n} pass {pass}: {e:?}"));
                let mut m = a.clone();
                put(&mut m, esp + 0x38, second);
                put(&mut m, esp + 0x10, x);
                put(&mut m, esp + 0x3c, third);
                let mut c = cpu(&mut r, esp, start, mx, cw);
                c.gpr[5] = word(&m, DIRTY);
                let (stubs, mut none) = no_stubs();
                lifted
                    .run(&mut c, &mut m, &[exit], &stubs, &mut none)
                    .unwrap_or_else(|e| panic!("seed {seed} case {n} pass {pass}: {e}"));
                assert_eq!(block(&m), want.0.to_vec(), "seed {seed} case {n}: pass {pass}");
            }
        }
    }
    eprintln!("combiners: all equal; {seen:#?}");
    for k in ["blur stages 3", "blur stages 8"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

#[test]
fn blur_matches_the_lifted_original() {
    if !with_image(blur_cases) {
        eprintln!("SKIPPED blur_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn blur_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(BLUR);
    let stubs: Stubs = [(BLUR_COMBINER, 0), (BIND, 0), (QUAD, 0x2c), (COPY, 8)].into_iter().collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ n as u64);
            let mx = mxcsr(&mut r);
            let args = BlurArgs {
                source: 7,
                steps: r.float(6.0),
                scale: r.float(2.0),
                growth: r.float(1.0),
                dest: 8,
                other: if r.below(2) == 0 { u32::MAX } else { 9 },
                last: [u32::MAX, 7, 8, 10][r.below(4) as usize],
            };
            let mut a = image.clone();
            let want = match glow::blur(&Eng(&mut a.clone()), &args, mx) {
                Ok(w) => Some(w),
                Err(Error::Unbounded) => None,
                Err(e) => panic!("seed {seed} case {n}: {e:?}"),
            };

            let esp = STACK_TOP - 0x400;
            let stack = [
                HARNESS_EXIT,
                args.steps,
                args.scale,
                args.growth,
                args.dest,
                args.other,
                args.last,
                (r.u32() & !0xff) | r.below(2),
                r.u32(),
                r.u32(),
                r.u32(),
            ];
            for (i, v) in stack.iter().enumerate() {
                put(&mut a, esp + 4 * i as u32, *v);
            }
            let mut c = cpu(&mut r, esp, BLUR, mx, 0x027f);
            c.gpr[0] = args.source;
            let mut bound = 0;
            let mut draws = Vec::new();
            let mut copy = None;
            let mut record = |pc: u32, c: &Cpu, m: &FlatMemory| match pc {
                // EDI the target, EBX the stage.
                BIND if c.gpr[3] == 0 => bound = c.gpr[7],
                QUAD => draws.push(BlurDraw {
                    source: bound,
                    dest: word(m, c.gpr[4] + 4),
                    spread: word(m, c.gpr[4] + 4 * 7),
                }),
                COPY => copy = Some((word(m, c.gpr[4] + 4), word(m, c.gpr[4] + 8))),
                _ => {}
            };
            let ran = lifted.run(&mut c, &mut a, &[HARNESS_EXIT], &stubs, &mut record);
            let Some(want) = want else {
                // Past the port's budget (steps at infinity): the
                // original must not finish either, and must have drawn
                // more than the budget's draws by its step limit.
                assert!(
                    matches!(&ran, Err(e) if e.starts_with("step limit")),
                    "seed {seed} case {n}: the port stopped at {} steps, the original {ran:?}: {args:#x?}",
                    glow::MAX_BLUR_STEPS
                );
                assert!(draws.len() > glow::MAX_BLUR_STEPS as usize, "seed {seed} case {n}: {} draws", draws.len());
                *seen.entry("unbounded".into()).or_default() += 1;
                continue;
            };
            ran.unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(c.gpr[4], esp + 4 + 0x28, "seed {seed} case {n}: esp");
            assert_eq!(draws, want.draws, "seed {seed} case {n}: {args:#x?}");
            // 0x000351a0(edi, esi): the result, then where it goes.
            assert_eq!(copy, want.copy, "seed {seed} case {n}: copy");
            assert_eq!(c.gpr[0], want.result, "seed {seed} case {n}: result");
            *seen.entry(format!("{} draws", want.draws.len().min(4))).or_default() += 1;
        }
    }
    eprintln!("blur: all equal; {seen:#?}");
    for k in ["0 draws", "1 draws", "4 draws"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}

#[test]
fn wide_matches_the_lifted_original() {
    if !with_image(wide_cases) {
        eprintln!("SKIPPED wide_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn wide_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(WIDE);
    let stubs: Stubs = [(BLUR_COMBINER, 0), (BIND, 0), (WIDE_QUAD, 0x24), (COPY, 8)].into_iter().collect();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..256u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let mx = mxcsr(&mut r);
            let args = WideArgs {
                source: 1,
                taps: [-2, 0, 1, 2, 3, 4, 7, glow::MAX_WIDE_TAPS, glow::MAX_WIDE_TAPS + 3][r.below(9) as usize],
                scale: r.float(4.0),
                words: [r.u32(), r.u32()],
                dest: 0x19,
                other: if r.below(3) == 0 { u32::MAX } else { 0x1a },
                last: [u32::MAX, 1, 0x19, 0x1a, 0x1b][r.below(5) as usize],
            };
            let mut a = image.clone();
            let want = match glow::wide(&Eng(&mut a.clone()), &args, mx) {
                Ok(w) => Some(w),
                Err(Error::Unbounded) => None,
                Err(e) => panic!("seed {seed} case {n}: {e:?}"),
            };

            let esp = STACK_TOP - 0x400;
            let combine = (r.u32() & !0xff) | r.below(2);
            let (sixth, eighth) = (r.u32(), r.float(1.0));
            let stack = [
                HARNESS_EXIT,
                args.words[0],
                args.words[1],
                args.dest,
                args.other,
                args.last,
                combine,
                sixth,
                eighth,
            ];
            for (i, v) in stack.iter().enumerate() {
                put(&mut a, esp + 4 * i as u32, *v);
            }
            let mut c = cpu(&mut r, esp, WIDE, mx, 0x027f);
            c.gpr[0] = args.source;
            c.gpr[1] = args.taps as u32;
            c.xmm[0] = (u128::from(r.u32()) << 32) | u128::from(args.scale);
            let mut bound = 0;
            let mut draws = Vec::new();
            let mut quad_words = Vec::new();
            let mut combiner = None;
            let mut copy = None;
            let mut record = |pc: u32, c: &Cpu, m: &FlatMemory| match pc {
                BLUR_COMBINER => combiner = Some((c.xmm[0] as u32, c.xmm[3] as u32)),
                // EDI the target, EBX the stage.
                BIND if c.gpr[3] == 0 => bound = c.gpr[7],
                WIDE_QUAD => {
                    draws.push(WideDraw {
                        source: bound,
                        dest: c.gpr[0],
                        offsets: std::array::from_fn(|k| word(m, c.gpr[7] + 4 * k as u32)),
                    });
                    quad_words.push((0..9).map(|k| word(m, c.gpr[4] + 4 + 4 * k)).collect::<Vec<u32>>());
                }
                COPY => copy = Some((word(m, c.gpr[4] + 4), word(m, c.gpr[4] + 8))),
                _ => {}
            };
            let ran = lifted.run(&mut c, &mut a, &[HARNESS_EXIT], &stubs, &mut record);
            ran.unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!(combiner, ((combine & 0xff) != 0).then_some((0, eighth)), "seed {seed} case {n}: the combiner");
            let Some(want) = want else {
                // Past the port's budget: the original draws every tap.
                assert_eq!(draws.len(), args.taps as usize, "seed {seed} case {n}");
                *seen.entry("unbounded".into()).or_default() += 1;
                continue;
            };
            assert_eq!(c.gpr[4], esp + 4 + 0x20, "seed {seed} case {n}: esp");
            assert_eq!(draws, want.draws, "seed {seed} case {n}: {args:#x?}");
            let words = vec![0xa, 0x0101_0101, 0, 0, 0x3f80_0000, args.words[0], args.words[1], sixth, 0];
            for w in &quad_words {
                assert_eq!(*w, words, "seed {seed} case {n}: the quad's stack words");
            }
            // 0x000351a0(edi, esi): the result, then where it goes.
            assert_eq!(copy, want.copy, "seed {seed} case {n}: copy");
            assert_eq!(c.gpr[0], want.result, "seed {seed} case {n}: result");
            *seen.entry(format!("{} draws", want.draws.len().min(4))).or_default() += 1;
            if want.copy.is_some() {
                *seen.entry("copied".into()).or_default() += 1;
            }
        }
    }
    eprintln!("wide: all equal; {seen:#?}");
    for k in ["0 draws", "1 draws", "4 draws", "unbounded", "copied"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
