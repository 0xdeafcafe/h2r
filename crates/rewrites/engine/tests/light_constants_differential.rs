//! `engine::light_constants` against the original: `0x0004a1e0` (and
//! `0x000497a0`, which it calls), lifted and run on the IR interpreter from
//! the same seeded 64 MB state, must leave memory byte for byte as
//! `light_globals` plus `LightGlobals::write` do.
//!
//! The two callees the port does not take over are stubs on the original's
//! side: `0x001650a0` (the sphere's screen bounds) answers from the case's
//! seeded generator, the same answer the port's `bounds` gets, and
//! `0x0003a550` (shader inputs, frame upload and optional gel) only returns. Each case seeds the light
//! array, a record, the definition, the view context, the view, the special
//! frame's globals, the viewport words, the old frame block and the
//! arguments, with any float bits now and then (zeros, infinities, NaNs,
//! denormals), under one of several MXCSR and x87 control words. The stack
//! below the entry is not compared (the original's locals live there).
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it: the constants the code
//! reads stay in the image. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it light_constants_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_constants::{
    self as lc, BoundsQuery, Call, Fpu, SCREEN_BOUNDS, SCREEN_BOUNDS_CLEANUP, SET_LIGHT,
    UPLOAD_LIGHT_FRAME, global,
};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 200_000;

/// The seeded structures.
const ARRAY: u32 = 0x0300_0000;
const RECORDS: u32 = 0x0300_1000;
const VIEW: u32 = 0x0301_0000;
const CONTEXT: u32 = 0x0302_0000;
const DEFINITION: u32 = 0x0303_0000;
const LOOSE_FRAME: u32 = 0x0304_0000;

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
    /// Mostly modest floats, sometimes the edges.
    fn float(&mut self) -> u32 {
        match self.below(40) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000 | (self.u32() & 0x8000_0000),
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => 0x7f80_0001 | (self.u32() & 0x803f_ffff),
            5 => self.u32() & 0x807f_ffff,
            6 => self.u32(),
            _ => self.span(8.0),
        }
    }
    /// Uniform in `(-r, r)`.
    fn span(&mut self, r: f32) -> u32 {
        let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
        ((unit * 2.0 - 1.0) * r).to_bits()
    }
    fn fill(&mut self, mem: &mut FlatMemory, at: u32, words: u32) {
        for i in 0..words {
            let v = self.float();
            put(mem, at + 4 * i, v);
        }
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

/// One case: its state, its call, its floating-point modes, and what
/// `0x001650a0` answers.
struct Case {
    mem: FlatMemory,
    call: Call,
    fpu: Fpu,
    bounds: Option<[u32; 6]>,
    esp: u32,
}

fn case(image: &FlatMemory, seed: u64, n: u32) -> Case {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ n as u64);

    // The light array and four records.
    put(&mut mem, global::LIGHT_ARRAY, ARRAY);
    put(&mut mem, ARRAY + 0x44, RECORDS);
    r.fill(&mut mem, RECORDS, 4 * 0x110 / 4);
    let light = if r.below(4) == 0 {
        u32::MAX
    } else {
        r.u32() & 0xffff_0000 | r.below(4)
    };
    for i in 0..4 {
        let rec = RECORDS + i * 0x110;
        // The driver word: none in some records.
        let w = if r.below(3) == 0 { 0xffff } else { r.u32() & 0xffff };
        mem.bytes[(rec + 0x54) as usize..(rec + 0x56) as usize].copy_from_slice(&(w as u16).to_le_bytes());
        // Weights and falloff words near the colour range.
        for off in [0xd4, 0xd8, 0xdc, 0xe0, 0xe4, 0xe8, 0xec, 0x100, 0x104] {
            if r.below(6) != 0 {
                let v = r.span(1.5);
                put(&mut mem, rec + off, f32::from_bits(v).abs().to_bits());
            }
        }
    }
    let frame = if light == u32::MAX || r.below(8) == 0 {
        r.fill(&mut mem, LOOSE_FRAME, 0x90 / 4);
        LOOSE_FRAME
    } else {
        RECORDS + (light & 0xffff) * 0x110 + 0x84
    };

    // The definition: flags with the two special bits in some cases, and
    // the projective word 1 in half.
    r.fill(&mut mem, DEFINITION, 0x100 / 4);
    let mut flags = r.u32() & !0x0004_0020;
    if r.below(3) == 0 {
        flags |= 0x20;
    }
    if r.below(2) == 0 {
        flags |= 0x0004_0000;
    }
    put(&mut mem, DEFINITION, flags);
    let kind: u16 = if r.below(2) == 0 { 1 } else { r.below(4) as u16 };
    mem.bytes[(DEFINITION + 0x8e) as usize..(DEFINITION + 0x90) as usize]
        .copy_from_slice(&kind.to_le_bytes());

    // The context: type 0, 1, 2, or a word-zero type the projective path
    // reads through a null pointer.
    r.fill(&mut mem, CONTEXT, 0x80 / 4);
    let ty = [0, 1, 2, 0x0001_0000][r.below(4) as usize];
    put(&mut mem, CONTEXT, ty);

    // The view: projection bounds ordered, sometimes degenerate.
    put(&mut mem, global::VIEW, VIEW);
    r.fill(&mut mem, VIEW, 0xc0 / 4);
    let (lo_x, lo_y) = (r.span(2.0), r.span(2.0));
    let wide = |r: &mut Rng, lo: u32| {
        if r.below(10) == 0 {
            lo
        } else {
            (f32::from_bits(lo) + 0.25 + f32::from_bits(r.span(2.0)).abs()).to_bits()
        }
    };
    let hi_x = wide(&mut r, lo_x);
    let hi_y = wide(&mut r, lo_y);
    if r.below(12) != 0 {
        put(&mut mem, VIEW + 0xa4, lo_x);
        put(&mut mem, VIEW + 0xa8, hi_x);
        put(&mut mem, VIEW + 0xac, lo_y);
        put(&mut mem, VIEW + 0xb0, hi_y);
    }

    // The special frame's globals, the viewport and the old frame block.
    r.fill(&mut mem, global::SPECIAL_ORIGIN, 9);
    for i in 0..4 {
        let v = (r.below(1400) as i32 - 100) as i16;
        let a = (global::VIEWPORT + 2 * i) as usize;
        mem.bytes[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }
    for i in 0..lc::FRAME_WORDS as u32 {
        let v = r.u32();
        put(&mut mem, global::FRAME + 4 * i, v);
    }

    // What 0x001650a0 answers: off screen now and then; otherwise bounds
    // around the view's, inside or straddling it.
    let bounds = if r.below(5) == 0 {
        None
    } else {
        let mut b = [0u32; 6];
        for w in b.iter_mut() {
            *w = if r.below(12) == 0 { r.float() } else { r.span(3.0) };
        }
        if r.below(2) == 0 {
            // Inside: x0 <= x1 within [lo_x, hi_x], -y within [lo_y, hi_y].
            let (a, c) = (f32::from_bits(lo_x), f32::from_bits(hi_x));
            let (d, e) = (f32::from_bits(lo_y), f32::from_bits(hi_y));
            let t = |r: &mut Rng| (f32::from_bits(r.span(1.0)).abs()).min(1.0);
            let (u0, u1) = (t(&mut r), t(&mut r));
            b[0] = (a + (c - a) * u0.min(u1) * 0.9).to_bits();
            b[1] = (a + (c - a) * u0.max(u1)).to_bits();
            let (v0, v1) = (t(&mut r), t(&mut r));
            b[2] = (-(d + (e - d) * v0.max(v1))).to_bits();
            b[3] = (-(d + (e - d) * v0.min(v1) * 0.9)).to_bits();
        }
        Some(b)
    };

    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    let call = Call {
        frame,
        context: CONTEXT,
        light,
        definition: DEFINITION,
        shadow: r.u32(),
        weight: if r.below(8) == 0 { r.float() } else { r.span(2.0) },
    };
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, call.definition);
    put(&mut mem, esp + 8, 1024f32.to_bits());
    put(&mut mem, esp + 12, call.shadow);
    put(&mut mem, esp + 16, call.weight);

    // Round to nearest for the x87 (the crate stops on a transcendental
    // under any other mode, as the game never runs one); every precision.
    let x87_control = [0x037f, 0x027f, 0x007f][r.below(3) as usize];
    let mxcsr = match r.below(6) {
        0 => 0x1f80 | 0x2000,
        1 => 0x1f80 | 0x6000,
        2 => 0x1f80 | 0x8000,
        _ => 0x1f80,
    };
    Case {
        mem,
        call,
        fpu: Fpu { mxcsr, x87_control },
        bounds,
        esp,
    }
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    /// Runs the original; returns how often each stub was reached.
    fn run(&mut self, c: &mut Case) -> Result<(u32, u32), String> {
        let mut cpu = Cpu {
            gpr: [c.call.frame, c.call.context, c.call.light, 0, c.esp, 0, 0, 0],
            fs_base: RAM as u32,
            eip: SET_LIGHT,
            ..Cpu::default()
        };
        cpu.mxcsr = c.fpu.mxcsr;
        cpu.x87.control = c.fpu.x87_control;
        let mem = &mut c.mem;
        let mut pc = SET_LIGHT;
        let (mut bounds_calls, mut uploads) = (0, 0);
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if pc == SCREEN_BOUNDS || pc == UPLOAD_LIGHT_FRAME {
                let esp = cpu.gpr[4];
                let mut cleanup = 0;
                if pc == SCREEN_BOUNDS {
                    bounds_calls += 1;
                    cleanup = SCREEN_BOUNDS_CLEANUP;
                    match c.bounds {
                        Some(b) => {
                            for (i, w) in b.iter().enumerate() {
                                put(mem, cpu.gpr[7] + 4 * i as u32, *w);
                            }
                            cpu.gpr[0] = 1;
                        }
                        None => cpu.gpr[0] = 0x100,
                    }
                } else {
                    uploads += 1;
                }
                pc = word(mem, esp);
                cpu.gpr[4] = esp + 4 + cleanup;
                continue;
            }
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
            let insn = self
                .cache
                .get(&pc)
                .ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(&mut cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        if cpu.gpr[4] != c.esp + 4 + lc::SET_LIGHT_CLEANUP {
            return Err(format!("esp at return {:#x}", cpu.gpr[4]));
        }
        Ok((bounds_calls, uploads))
    }
}

#[test]
fn light_globals_match_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED light_globals_match_the_lifted_original: no XBE (set H2_XBE)");
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
        for n in 0..96 {
            let mut a = case(&image, seed, n);
            let mut b = FlatMemory {
                base: a.mem.base,
                bytes: a.mem.bytes.clone(),
            };
            let (bounds_calls, uploads) = lifted.run(&mut a).unwrap();
            assert_eq!(uploads, 1, "seed {seed} case {n}: the frame uploads once");

            let mut asked = Vec::new();
            let answer = a.bounds;
            let g = lc::light_globals(&Eng(&mut b), &a.call, a.fpu, &mut |q: BoundsQuery| {
                asked.push(q);
                answer
            })
            .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            g.write(&mut Eng(&mut b)).unwrap();
            assert_eq!(asked.len() as u32, bounds_calls, "seed {seed} case {n}: bounds calls");
            if let Some(q) = asked.first() {
                assert_eq!(q.view, VIEW + 4);
            }

            let low = (STACK_TOP - 0x1000) as usize;
            a.mem.bytes[low..STACK_TOP as usize].fill(0);
            b.bytes[low..STACK_TOP as usize].fill(0);
            if let Some(i) = a.mem.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {n}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)\n{g:#x?}",
                    a.mem.bytes[i], b.bytes[i]
                );
            }

            let flags = word(&a.mem, DEFINITION);
            let special = flags & 0x20 != 0 && flags & 0x40000 != 0;
            *seen.entry(if special { "special frame" } else { "plain frame" }).or_default() += 1;
            *seen.entry(match word(&a.mem, CONTEXT) {
                0 => "context 0",
                1 => "context 1",
                _ => "context other",
            }).or_default() += 1;
            *seen.entry(match g.mode {
                0 => "mode 0",
                1 => "mode 1",
                _ => "mode 2",
            }).or_default() += 1;
            *seen.entry(if g.sphere.is_some() { "record" } else { "no record" }).or_default() += 1;
            let projected = g.sphere.is_some() && a.bounds.is_some_and(|b| g.rectangle[4..] == b[4..]);
            *seen.entry(if projected { "projected" } else { "whole viewport" }).or_default() += 1;
        }
    }
    eprintln!("light_globals: all equal; {seen:#?}");
    for k in [
        "special frame",
        "plain frame",
        "context 0",
        "context 1",
        "context other",
        "mode 0",
        "mode 1",
        "mode 2",
        "record",
        "no record",
        "projected",
        "whole viewport",
    ] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
