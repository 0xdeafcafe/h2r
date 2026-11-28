//! `engine::shadow_constants` against the original: `0x00048ee0` (with
//! its callees `0x00048e70` and `0x0001ddd0`), lifted and run on the IR
//! interpreter from the same seeded 64 MB state, must leave memory byte
//! for byte as `shadow_globals` plus `ShadowGlobals::write` do.
//!
//! The two device calls are stubs on the original's side: `0x0001cf50`
//! (binding the desired textures) only returns, and `0x003f7330` (the
//! constants upload, reached by a tail jump) checks its arguments and
//! returns with `ret 4`. Each case seeds the view context, the light's
//! frame block, the definition, the scene and pass flags, the falloff
//! words, the frame copies, the old block, the shadow target's entry and
//! the desired textures, with any float bits now and then, under one of
//! several MXCSR and x87 control words. The stack is not compared.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it. `H2_DIFF_SEEDS=1,2,3`
//! picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it shadow_constants_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_constants::Fpu;
use guest_layout::memory::{GuestMemory, MemFault};
use engine::shadow_constants::{
    self as sc, BIND_TEXTURES, CONSTANTS_DWORDS, CONSTANTS_REGISTER, Call, SET_CONSTANTS,
    SET_CONSTANTS_CLEANUP, SHADOW_SETUP, global,
};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 200_000;

const CONTEXT: u32 = 0x0302_0000;
const DEFINITION: u32 = 0x0303_0000;
const FRAME: u32 = 0x0304_0000;

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
    /// `|span(r)|`.
    fn up_to(&mut self, r: f32) -> f32 {
        f32::from_bits(self.span(r)).abs()
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

fn put16(mem: &mut FlatMemory, a: u32, v: u16) {
    mem.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

struct Case {
    mem: FlatMemory,
    call: Call,
    fpu: Fpu,
    esp: u32,
}

fn case(image: &FlatMemory, seed: u64, n: u32) -> Case {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);

    // The context: a positive square (mostly), a field of view (half the
    // time), a reciprocal source that is zero now and then.
    r.fill(&mut mem, CONTEXT, 0x80 / 4);
    let c8 = if r.below(10) == 0 {
        r.float()
    } else {
        (0.25 + r.up_to(4.0)).to_bits()
    };
    put(&mut mem, CONTEXT + 8, c8);
    let c0c = match r.below(4) {
        0 => r.float(),
        1 => r.up_to(1.0).to_bits() | 0x8000_0000,
        _ => (0.1 + r.up_to(2.5)).to_bits(),
    };
    put(&mut mem, CONTEXT + 0xc, c0c);
    let c4 = if r.below(8) == 0 {
        r.float()
    } else {
        r.up_to(60.0).to_bits()
    };
    put(&mut mem, CONTEXT + 4, c4);
    let c64 = if r.below(5) == 0 { 0 } else { r.span(4.0) };
    put(&mut mem, CONTEXT + 0x64, c64);

    // The light's frame block and the definition.
    r.fill(&mut mem, FRAME, 0x40 / 4);
    r.fill(&mut mem, DEFINITION, 0x100 / 4);
    let kind = if r.below(2) == 0 { 1 } else { r.below(4) as u16 };
    put16(&mut mem, DEFINITION + 0x8e, kind);
    let w14 = if r.below(2) == 0 { 0 } else { r.u32() as u16 };
    put16(&mut mem, DEFINITION + 0x14, w14);
    let d10 = r.span(200.0);
    put(&mut mem, DEFINITION + 0x10, d10);
    let near = r.span(4.0);
    put(&mut mem, DEFINITION + 0x2c, near);
    let far = if r.below(6) == 0 {
        r.float()
    } else {
        (f32::from_bits(near) + 0.5 + r.up_to(50.0)).to_bits()
    };
    put(&mut mem, DEFINITION + 0x30, far);

    // Flags, falloff words, frame copies, the old block, the target, the
    // textures.
    let v = r.u32();
    put(&mut mem, global::SCENE_FLAGS, v);
    let v = r.u32();
    put(&mut mem, global::PASS_FLAGS, v);
    let v = if r.below(8) == 0 {
        r.float()
    } else {
        r.up_to(2.0).to_bits()
    };
    put(&mut mem, global::FALLOFF_1, v);
    let v = r.float();
    put(&mut mem, global::FALLOFF_4, v);
    r.fill(&mut mem, global::FRAME_COPIES, 8);
    for i in 0..sc::BLOCK_WORDS as u32 {
        let v = r.u32();
        put(&mut mem, global::BLOCK + 4 * i, v);
    }
    let e = global::TARGETS + global::SHADOW_TARGET * global::TARGET_STRIDE;
    for i in 0..global::TARGET_STRIDE / 4 {
        let v = r.u32();
        put(&mut mem, e + 4 * i, v);
    }
    mem.bytes[(e + 0x94) as usize] = if r.below(3) == 0 { 0 } else { 1 };
    for i in 0..4 {
        let v = r.u32();
        put(&mut mem, global::TEXTURES + 4 * i, v);
    }

    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    let call = Call {
        context: CONTEXT,
        frame: FRAME,
        definition: DEFINITION,
    };
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, call.definition);
    // Round to nearest for the x87 (the crate stops on a transcendental
    // under any other mode); every precision.
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
            gpr: [0, c.call.context, 0, c.call.frame, c.esp, 0, 0, 0],
            fs_base: RAM as u32,
            eip: SHADOW_SETUP,
            ..Cpu::default()
        };
        cpu.mxcsr = c.fpu.mxcsr;
        cpu.x87.control = c.fpu.x87_control;
        let mem = &mut c.mem;
        let mut pc = SHADOW_SETUP;
        let (mut binds, mut uploads) = (0, 0);
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if pc == BIND_TEXTURES || pc == SET_CONSTANTS {
                let esp = cpu.gpr[4];
                let mut cleanup = 0;
                if pc == SET_CONSTANTS {
                    uploads += 1;
                    cleanup = SET_CONSTANTS_CLEANUP;
                    if cpu.gpr[2] != global::BLOCK
                        || cpu.gpr[1] != CONSTANTS_REGISTER
                        || word(mem, esp + 4) != CONSTANTS_DWORDS
                    {
                        return Err(format!(
                            "upload with edx {:#x} ecx {:#x} register {:#x}",
                            cpu.gpr[2],
                            cpu.gpr[1],
                            word(mem, esp + 4)
                        ));
                    }
                } else {
                    binds += 1;
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
        if cpu.gpr[4] != c.esp + 4 + sc::SHADOW_SETUP_CLEANUP {
            return Err(format!("esp at return {:#x}", cpu.gpr[4]));
        }
        Ok((binds, uploads))
    }
}

#[test]
fn shadow_globals_match_the_lifted_original() {
    let Some(path) = xbe_path() else {
        eprintln!("SKIPPED shadow_globals_match_the_lifted_original: no XBE (set H2_XBE)");
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
            let mut b = a.mem.clone();
            let (binds, uploads) = lifted
                .run(&mut a)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            assert_eq!((binds, uploads), (1, 1), "seed {seed} case {n}: device calls");

            let g = sc::shadow_globals(&Eng(&mut b), &a.call, a.fpu)
                .unwrap_or_else(|e| panic!("seed {seed} case {n}: {e:?}"));
            g.write(&mut Eng(&mut b)).unwrap();

            let low = (STACK_TOP - 0x1000) as usize;
            a.mem.bytes[low..STACK_TOP as usize].fill(0);
            b.bytes[low..STACK_TOP as usize].fill(0);
            if let Some(i) = a.mem.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y) {
                panic!(
                    "seed {seed} case {n}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)\n{g:#x?}",
                    a.mem.bytes[i], b.bytes[i]
                );
            }

            let mode = match g.mode {
                0 => "mode 0",
                1 => "mode 1",
                _ => "mode 2",
            };
            *seen.entry(mode).or_default() += 1;
            let map = if g.target.is_some() { "full map" } else { "small map" };
            *seen.entry(map).or_default() += 1;
            let tan = f32::from_bits(word(&a.mem, CONTEXT + 0xc)) > 0.0;
            let fov = if tan { "field of view" } else { "no field of view" };
            *seen.entry(fov).or_default() += 1;
        }
    }
    eprintln!("shadow_globals: all equal; {seen:#?}");
    for k in [
        "mode 0",
        "mode 1",
        "mode 2",
        "full map",
        "small map",
        "field of view",
        "no field of view",
    ] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
