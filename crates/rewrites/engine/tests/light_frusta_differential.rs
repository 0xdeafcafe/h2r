//! `engine::light_frusta` against the original `0x000c1d90`, lifted and run
//! on the IR interpreter from the same seeded 64 MB state. Each case seeds
//! a posed volume (a cube, kind 0, or a projected kind), the light's pose,
//! the faces' up references, the default rectangle and the window box,
//! the assert byte, garbage in the frusta and the count, and an MXCSR and
//! x87 control word. The port's proposal is applied to a copy; memory must
//! then equal the lifted body's byte for byte outside the dead stack. A
//! case the port refuses is counted, not compared; most must be answered.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`); skips without
//! it. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo xtask test -p engine --features differential \
//!     -- --test it light_frusta_differential:: --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_frusta::{self as lf, Call, guest as g};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

const BODY: u32 = 0x000c_1d90;
const VOLUME: u32 = 0x0300_0000;
const POSE: u32 = 0x0300_0100;
const COUNT: u32 = 0x0300_0200;
const RECT: u32 = 0x0300_0300;
const BOUNDS: u32 = 0x0300_0340;
const UP: u32 = 0x0300_0380;
const ALT: u32 = 0x0300_03c0;
const FRUSTA: u32 = 0x0300_1000;

struct Eng<'a>(&'a FlatMemory);

impl GuestMemory for Eng<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let src = self.0.bytes.get(a..a + buf.len()).ok_or(MemFault { addr, len: buf.len() as u32, write: false })?;
        buf.copy_from_slice(src);
        Ok(())
    }
    fn write(&mut self, addr: u32, _data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault { addr, len: 0, write: true })
    }
}

struct Mut<'a>(&'a mut FlatMemory);

impl GuestMemory for Mut<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        Eng(self.0).read(addr, buf)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let dst = self.0.bytes.get_mut(a..a + data.len()).ok_or(MemFault { addr, len: data.len() as u32, write: true })?;
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
    fn unit(&mut self) -> f32 {
        self.below(1 << 20) as f32 / (1 << 20) as f32 * 2.0 - 1.0
    }
    /// A float in -top..top mostly, and rarely an edge.
    fn float(&mut self, top: f32) -> u32 {
        match self.below(64) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7fc0_0000,
            3 => 0x7f80_0000,
            _ => (self.unit() * top).to_bits(),
        }
    }
    /// A vector of length about one, or an axis.
    fn direction(&mut self) -> [u32; 3] {
        if self.below(3) == 0 {
            let mut v = [0u32; 3];
            v[self.below(3) as usize] = [1.0f32, -1.0][self.below(2) as usize].to_bits();
            return v;
        }
        let v = [self.unit(), self.unit(), self.unit()];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
        v.map(|c| (c / l).to_bits())
    }
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put3(mem: &mut FlatMemory, a: u32, v: [u32; 3]) {
    for (k, w) in v.into_iter().enumerate() {
        put(mem, a + 4 * k as u32, w);
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

    fn run(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory) -> Result<(), String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(format!("step limit ({STEP_LIMIT} instructions) at {pc:#x}"));
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn).map_err(|e| format!("{pc:#x}: {e:?}"))?.target();
        }
        Ok(())
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
    f(&image, Lifted { lifter: &lifter, cache: BTreeMap::new() });
    true
}

fn seeds() -> Vec<u64> {
    std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99])
}

#[test]
fn the_light_frusta_equal_what_the_lifted_body_writes() {
    if !with_image(frusta_cases) {
        eprintln!("SKIPPED the_light_frusta_equal_what_the_lifted_body_writes: no XBE (set H2_XBE)");
    }
}

fn frusta_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(BODY);
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let (mut answered, mut refused) = (0u32, 0u32);
    for seed in seeds() {
        for n in 0..96u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            let mx = if r.below(10) == 0 { [0x1f80 | 0x8040, 0x1f80 | 0x6000][r.below(2) as usize] } else { 0x1f80 | r.below(0x40) };
            let cw: u16 = if r.below(10) == 0 { [0x037f, 0x0e7f][r.below(2) as usize] } else { 0x027f };

            // The volume: kind, then every word the builder reads.
            for k in 0..(g::VOLUME_READ / 4) {
                let w = r.float(8.0);
                put(&mut m, VOLUME + 4 * k, w);
            }
            let cube = r.below(2) == 0;
            let kind_word = if cube {
                r.u32() & 0xffff_0000
            } else {
                match r.below(3) {
                    0 => 1,
                    1 => 2,
                    _ => r.u32() | 1,
                }
            };
            put(&mut m, VOLUME, kind_word);
            let centre = [r.float(50.0), r.float(50.0), r.float(50.0)];
            put3(&mut m, VOLUME + 0x2c, centre);
            if cube {
                let h = if r.below(8) == 0 { r.float(4.0) } else { (0.5 + 20.0 * r.unit().abs()).to_bits() };
                put(&mut m, VOLUME + 0x44, h);
            } else {
                let right = r.direction();
                put3(&mut m, VOLUME + 0x1c, right);
                let apex = match r.below(6) {
                    0 => 0,
                    1 => r.float(2.0),
                    _ => (0.1 + 3.0 * r.unit().abs()).to_bits(),
                };
                put(&mut m, VOLUME + 0x28, apex);
                let axis = r.direction();
                put3(&mut m, VOLUME + 0x3c, axis);
                for at in [0x58, 0x5c, 0x64] {
                    let w = (0.1 + 10.0 * r.unit().abs()).to_bits();
                    put(&mut m, VOLUME + at, w);
                }
                let (fwd, up) = (r.direction(), r.direction());
                put3(&mut m, POSE + 0x28, fwd);
                put3(&mut m, POSE + 0x34, up);
            }
            // The image's pointers and what they point at.
            put(&mut m, g::DEFAULT_RECT, RECT);
            let rect = if r.below(3) == 0 {
                [r.float(2.0), r.float(2.0), r.float(2.0), r.float(2.0)]
            } else {
                [f32::MAX, -f32::MAX, f32::MAX, -f32::MAX].map(f32::to_bits)
            };
            for (k, w) in rect.into_iter().enumerate() {
                put(&mut m, RECT + 4 * k as u32, w);
            }
            put(&mut m, g::WINDOW_BOUNDS, BOUNDS);
            for k in 0..6 {
                let w = if r.below(3) == 0 { r.float(100.0) } else { [f32::MAX, -f32::MAX][k % 2].to_bits() };
                put(&mut m, BOUNDS + 4 * k as u32, w);
            }
            put(&mut m, g::UP_PRIMARY, UP);
            put(&mut m, g::UP_ALTERNATE, ALT);
            let (up, alt) = if r.below(2) == 0 {
                ([1.0f32, 0.0, 0.0].map(f32::to_bits), [0.0f32, 1.0, 0.0].map(f32::to_bits))
            } else {
                (r.direction(), r.direction())
            };
            put3(&mut m, UP, up);
            put3(&mut m, ALT, alt);
            let flag = [0u8, 1, 0x80][r.below(3) as usize];
            m.bytes[g::ASSERT_ONCE as usize] = flag;
            // Garbage where the builder writes.
            for k in 0..(6 * g::FRUSTUM / 4) {
                let w = r.u32();
                put(&mut m, FRUSTA + 4 * k, w);
            }
            let count_garbage = r.u32();
            put(&mut m, COUNT, count_garbage);

            let esp = STACK_TOP - 0x800;
            put(&mut m, esp, HARNESS_EXIT);
            for (k, a) in [POSE, VOLUME, FRUSTA, COUNT].into_iter().enumerate() {
                put(&mut m, esp + 4 + 4 * k as u32, a);
            }
            let bits = r.u32() & 0x400;
            let call = Call::from_registers(FRUSTA, VOLUME, COUNT, bits, [POSE, VOLUME, FRUSTA, COUNT]);
            let port = lf::build(&Eng(&m), &call, mx, cw);

            let mut want = m.clone();
            let (ebx, ebp, edi) = (r.u32(), r.u32(), r.u32());
            let mut c = Cpu { gpr: [FRUSTA, VOLUME, COUNT, ebx, esp, ebp, bits, edi], fs_base: RAM as u32, eip: BODY, ..Cpu::default() };
            c.mxcsr = mx;
            c.x87.control = cw;
            lifted.run(&mut c, &mut m).unwrap_or_else(|e| panic!("seed {seed} case {n}: {e}"));
            let built = match port {
                Ok(built) => built,
                Err(why) => {
                    refused += 1;
                    *seen.entry(format!("refused {}", why.name())).or_default() += 1;
                    continue;
                }
            };
            answered += 1;
            built.proposal.apply(&mut Mut(&mut want)).unwrap();
            // The dead stack: the frame, the callees' and their arguments.
            for a in esp - 0x400..esp {
                want.bytes[a as usize] = m.bytes[a as usize];
            }
            let differing = built.proposal.differing(&Eng(&m));
            assert!(differing.is_empty(), "seed {seed} case {n} (kind {kind_word:#x}, flag {flag}): fields {differing:?} differ");
            if want.bytes != m.bytes {
                let at = want.bytes.iter().zip(&m.bytes).position(|(a, b)| a != b).unwrap_or(0);
                panic!("seed {seed} case {n}: memory differs first at {at:#x}");
            }
            assert_eq!([c.gpr[3], c.gpr[5], c.gpr[6], c.gpr[7]], [ebx, ebp, bits, edi], "seed {seed} case {n}: saved registers");
            assert_eq!(c.gpr[4], esp + 0x14, "seed {seed} case {n}: esp after ret 10h");
            *seen.entry(format!("kind {}", if cube { "cube" } else { "projected" })).or_default() += 1;
            let window = built.proposal.fields.iter().any(|f| f.name.ends_with("_window") && f.bytes.iter().any(|&b| b != 0));
            *seen.entry(format!("window {window}")).or_default() += 1;
            let cleared = built.proposal.fields.iter().any(|f| f.name == "assert_once");
            *seen.entry(format!("assert cleared {cleared}")).or_default() += 1;
        }
    }
    eprintln!("light frusta: {answered} equal, {refused} refused; {seen:#?}");
    assert!(answered > refused, "the port answers most cases");
    for k in ["kind cube", "kind projected", "window true"] {
        assert!(seen.contains_key(k), "{k} never answered: widen the states");
    }
}
