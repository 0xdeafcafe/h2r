//! `engine::view_projection` against the original `0x0002fb70`, lifted and
//! run on the IR interpreter from the same seeded 64 MB state
//! (docs/graphics/native-visibility.md, "The view records"). Each case
//! seeds a render camera (field of view, viewport), null or given extents,
//! the camera override (none, its lens test held or not), the widescreen
//! byte and the display settings (none, flag off or on), under several
//! MXCSR and x87 control words. The port's buffer is applied to a copy;
//! memory must then be equal byte for byte outside the dead stack, and
//! EAX, ECX, EDX, ESI, EDI and ESP equal. A case the port refuses is
//! counted, not compared; most must be answered.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it: the constants are the
//! image's own words. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it view_projection_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::view_projection::{self as vp, Call, guest as g};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 1_000_000;

const BUILD: u32 = 0x0002_fb70;
const CAMERA: u32 = 0x0300_0000;
const EXTENTS: u32 = 0x0300_0100;
const OVERRIDE: u32 = 0x0300_0200;
const SETTINGS: u32 = 0x0300_0300;

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
        Err(MemFault {
            addr,
            len: 0,
            write: true,
        })
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
            5 => self.u32() & 0x807f_ffff,
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
            let insn = self
                .cache
                .get(&pc)
                .ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
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
fn the_projection_matches_the_lifted_original() {
    if !with_image(projection_cases) {
        eprintln!("SKIPPED the_projection_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn projection_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(BUILD);
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let (mut answered, mut refused) = (0u32, 0u32);
    for seed in seeds() {
        for n in 0..128u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            // Mostly the state the port takes; now and then one it refuses.
            let mx = if r.below(8) == 0 {
                [0x1f80 | 0x8040, 0x1f80 | 0x6000][r.below(2) as usize]
            } else {
                0x1f80 | r.below(0x40)
            };
            let cw: u16 = if r.below(8) == 0 {
                [0x037f, 0x0e7f][r.below(2) as usize]
            } else {
                0x027f
            };

            let fov = match r.below(6) {
                0 => r.float(3.2),
                _ => (0.3 + r.below(1 << 16) as f32 / (1 << 16) as f32 * 2.5).to_bits(),
            };
            put(&mut m, CAMERA + g::CAMERA_FOV, fov);
            let viewport: [u16; 4] = match r.below(5) {
                0 => [0, 0, 480, 640],
                1 => [0, 0, 240, 640],
                2 => [0, 0, 480, 320],
                3 => [240, 320, 480, 640],
                _ => [
                    r.below(480) as u16,
                    r.below(640) as u16,
                    r.below(480) as u16,
                    r.below(640) as u16,
                ],
            };
            for (k, v) in viewport.iter().enumerate() {
                put16(&mut m, CAMERA + g::CAMERA_VIEWPORT + 2 * k as u32, *v);
            }
            let extents = if r.below(3) == 0 {
                for k in 0..4 {
                    let v = r.float(2.0);
                    put(&mut m, EXTENTS + 4 * k, v);
                }
                EXTENTS
            } else {
                0
            };
            let override_kind = r.below(4);
            match override_kind {
                0 => put(&mut m, g::CAMERA_OVERRIDE, 0),
                _ => {
                    put(&mut m, g::CAMERA_OVERRIDE, OVERRIDE);
                    m.bytes[(OVERRIDE + 1) as usize] = [0u8, 1, 0x80][r.below(3) as usize];
                    put16(&mut m, OVERRIDE + 2, [0xffffu16, 0, 7][r.below(3) as usize]);
                    put(&mut m, OVERRIDE + 8, [3u32, 2][r.below(2) as usize]);
                }
            }
            m.bytes[g::WIDESCREEN as usize] = u8::from(r.below(2) == 0);
            match r.below(3) {
                0 => put(&mut m, g::DISPLAY, 0),
                _ => {
                    put(&mut m, g::DISPLAY, SETTINGS);
                    m.bytes[(SETTINGS + 5) as usize] = [0u8, 1][r.below(2) as usize];
                }
            }

            let esp = STACK_TOP - 0x400;
            let out = esp + 0x40;
            for k in 0..16 {
                put(&mut m, out + 4 * k, r.u32());
            }
            put(&mut m, esp, HARNESS_EXIT);
            let (esi, edi, ebx, ebp) = (r.u32(), r.u32(), r.u32(), r.u32());
            let edx = extents;
            let call = Call {
                camera: CAMERA,
                extents: edx,
                output: out,
            };
            let port = vp::build(&Eng(&m), call, mx, cw);

            let mut want = m.clone();
            let mut c = Cpu {
                gpr: [out, CAMERA, edx, ebx, esp, ebp, esi, edi],
                fs_base: RAM as u32,
                eip: BUILD,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            // The x87 model stops (`Unmodelled`) on `fptan` under a directed
            // rounding mode, as the game never runs one: there is no original
            // to compare, so only a case the port refuses may end there.
            if let Err(e) = lifted.run(&mut c, &mut m) {
                let directed = cw & 0x0c00 != 0 && e.contains("Unmodelled");
                assert!(directed && port.is_err(), "seed {seed} case {n}: {e}");
            }
            let built = match port {
                Ok(built) => built,
                Err(why) => {
                    refused += 1;
                    *seen.entry(format!("refused {why:?}")).or_default() += 1;
                    continue;
                }
            };
            answered += 1;
            for (k, w) in built.words.iter().enumerate() {
                put(&mut want, out + 4 * k as u32, *w);
            }
            // The dead stack: the builder's three pushes.
            for a in esp - 12..esp {
                want.bytes[a as usize] = m.bytes[a as usize];
            }
            let got_words: Vec<u32> = (0..16).map(|k| word(&m, out + 4 * k)).collect();
            assert_eq!(
                got_words,
                built.words.to_vec(),
                "seed {seed} case {n}: the buffer (fov {fov:#x}, viewport {viewport:?}, extents {edx:#x}, mxcsr {mx:#x}, cw {cw:#x})"
            );
            if want.bytes != m.bytes {
                let at = want
                    .bytes
                    .iter()
                    .zip(&m.bytes)
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                panic!("seed {seed} case {n}: memory differs first at {at:#x}");
            }
            assert_eq!(c.gpr[0], out, "seed {seed} case {n}: eax");
            assert_eq!(c.gpr[1], built.ecx, "seed {seed} case {n}: ecx");
            assert_eq!(c.gpr[2], built.edx, "seed {seed} case {n}: edx");
            assert_eq!(
                [c.gpr[3], c.gpr[5], c.gpr[6], c.gpr[7]],
                [ebx, ebp, esi, edi],
                "seed {seed} case {n}: saved registers"
            );
            assert_eq!(c.gpr[4], esp + 4, "seed {seed} case {n}: esp");
            *seen.entry(format!("extents {}", extents != 0)).or_default() += 1;
            *seen.entry(format!("override {override_kind}")).or_default() += 1;
            *seen
                .entry(format!("widescreen {}", m.bytes[g::WIDESCREEN as usize]))
                .or_default() += 1;
            *seen
                .entry(format!("display {:#x}", word(&m, g::DISPLAY)))
                .or_default() += 1;
        }
    }
    eprintln!("projection: {answered} equal, {refused} refused; {seen:#?}");
    assert!(answered > refused, "the port answers most cases");
    for k in [
        "extents true",
        "extents false",
        "widescreen 0",
        "widescreen 1",
        "override 0",
        "override 1",
    ] {
        assert!(seen.contains_key(k), "{k} never answered: widen the states");
    }
}
