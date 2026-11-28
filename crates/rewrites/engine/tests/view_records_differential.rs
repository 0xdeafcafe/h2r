//! `engine::view_records` against the original `render_game_views`
//! `0x002234e0`, lifted and run on the IR interpreter up to its call of
//! `render_views` (docs/graphics/native-visibility.md, "The view
//! records"). Each case seeds the player count, the local players, the
//! display and widescreen flags, the camera override, the countdowns, the
//! one-shot byte, the observers, the active structure and the screen. The
//! port's shadow buffer is applied to a copy; memory must then equal the
//! lifted body's byte for byte outside the stack, and the registers,
//! arguments and saved frame at `render_views`' entry must be the ones
//! the port gives. A case the port refuses is counted, not compared.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it view_records_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::view_records::{self as vr, Float};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

const ENTRY: u32 = 0x0022_34e0;
const PLAYERS: u32 = 0x0300_0000;
const OVERRIDE: u32 = 0x0300_1000;
const DISPLAY: u32 = 0x0300_3000;
const DEFAULT_CAMERA: u32 = 0x0300_4000;

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

struct EngMut<'a>(&'a mut FlatMemory);

impl GuestMemory for EngMut<'_> {
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
}

fn put(m: &mut FlatMemory, a: u32, v: u32) {
    m.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(m: &mut FlatMemory, a: u32, v: u16) {
    m.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn word(m: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(m.bytes[a as usize..a as usize + 4].try_into().unwrap())
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

    /// Runs until `stop` is reached, before it executes.
    fn run_to(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory, stop: u32) -> Result<(), String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while pc != stop {
            if pc == HARNESS_EXIT {
                return Err("returned without calling render_views".into());
            }
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(format!("step limit at {pc:#x}"));
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn).map_err(|e| format!("{pc:#x}: {e:?}"))?.target();
        }
        cpu.eip = pc;
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
fn the_view_records_equal_what_the_lifted_body_writes() {
    if !with_image(cases) {
        eprintln!("SKIPPED the_view_records_equal_what_the_lifted_body_writes: no XBE (set H2_XBE)");
    }
}

/// One seeded state.
fn seed(m: &mut FlatMemory, r: &mut Rng, seen: &mut BTreeMap<String, u32>) {
    let count = [1u16, 1, 2, 2, 3, 4, 0, 5][r.below(8) as usize];
    put(m, vr::PLAYER_GLOBALS, PLAYERS);
    put16(m, PLAYERS + 8, count);
    for k in 0..4 {
        let present = r.below(4) != 0;
        put(m, PLAYERS + 0xc + 4 * k, if present { 0x0001_0000 | k } else { u32::MAX });
    }
    match r.below(4) {
        0 => {
            put(m, vr::DISPLAY, DISPLAY);
            m.bytes[(DISPLAY + 5) as usize] = 1;
        }
        1 => {
            put(m, vr::DISPLAY, DISPLAY);
            m.bytes[(DISPLAY + 5) as usize] = 0;
        }
        _ => put(m, vr::DISPLAY, 0),
    }
    m.bytes[vr::WIDESCREEN as usize] = r.below(2) as u8;
    match r.below(6) {
        0 => {
            // Active: the port refuses.
            put(m, vr::CAMERA_OVERRIDE, OVERRIDE);
            m.bytes[(OVERRIDE + 0x1120) as usize] = 1;
            put(m, OVERRIDE + 8, 3);
        }
        1 => {
            // Its lens test only.
            put(m, vr::CAMERA_OVERRIDE, OVERRIDE);
            m.bytes[(OVERRIDE + 1) as usize] = 1;
            put16(m, OVERRIDE + 2, [0u16, 0xffff][r.below(2) as usize]);
            m.bytes[(OVERRIDE + 0x1120) as usize] = 0;
            put(m, OVERRIDE + 8, [3u32, 2][r.below(2) as usize]);
        }
        _ => put(m, vr::CAMERA_OVERRIDE, 0),
    }
    for c in [vr::COUNTDOWN, vr::SPLIT_COUNTDOWN] {
        put(m, c, [0u32, 1, 5, u32::MAX][r.below(4) as usize]);
    }
    m.bytes[vr::SPLIT_ONE_SHOT as usize] = u8::from(r.below(3) == 0);
    put16(m, vr::ACTIVE_BSP_INDEX, if r.below(5) == 0 { 0xffff } else { 0 });
    for p in 0..4 {
        let o = vr::OBSERVERS + vr::OBSERVER_SIZE * p;
        for k in 0..0x60 / 4 {
            put(m, o + 4 * k, (r.unit() * 100.0).to_bits());
        }
        put(m, o + 0x4c, (0.5 + r.unit().abs() * 1.5).to_bits());
    }
    let screen: [u16; 8] = match r.below(3) {
        0 => [0, 0, 480, 640, 24, 32, 456, 608],
        1 => [0, 0, 720, 1280, 36, 64, 684, 1216],
        _ => [0, 0, 480, 640, 0, 0, 480, 640],
    };
    for (k, v) in screen.iter().enumerate() {
        put16(m, vr::SCREEN + 2 * k as u32, *v);
    }
    put(m, vr::NEAR, 0.0625f32.to_bits());
    put(m, vr::FAR, 1024f32.to_bits());
    put(m, vr::DEFAULT_FOV, (0.6 + r.unit().abs()).to_bits());
    for (k, p) in [vr::DEFAULT_POSITION, vr::DEFAULT_FORWARD, vr::DEFAULT_UP].into_iter().enumerate() {
        let at = DEFAULT_CAMERA + 0x10 * k as u32;
        put(m, p, at);
        for j in 0..3 {
            put(m, at + 4 * j, r.unit().to_bits());
        }
    }
    // The records keep what an earlier frame left in their untouched bytes.
    for a in (vr::RECORDS..vr::RECORDS + vr::RECORD_SIZE * 6).step_by(4) {
        put(m, a, r.u32());
    }
    *seen.entry(format!("count {count}")).or_default() += 1;
}

fn cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(ENTRY);
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let (mut answered, mut refused) = (0u32, 0u32);
    for s in seeds() {
        for n in 0..96u32 {
            let mut r = Rng(s.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let mut m = image.clone();
            seed(&mut m, &mut r, &mut seen);
            let esp = STACK_TOP - 0x400;
            put(&mut m, esp, HARNESS_EXIT);
            let (ebx, ebp, esi, edi, ecx) = (r.u32(), r.u32(), r.u32(), r.u32(), r.u32());
            let float = Float { mxcsr: 0x1f80, x87_control: 0x027f, x87_tag: u16::MAX };
            let port = vr::build(&Eng(&m), float);
            let mut want = m.clone();
            let mut c = Cpu {
                gpr: [r.u32(), ecx, r.u32(), ebx, esp, ebp, esi, edi],
                fs_base: RAM as u32,
                eip: ENTRY,
                ..Cpu::default()
            };
            c.mxcsr = float.mxcsr;
            c.x87.control = float.x87_control;
            lifted
                .run_to(&mut c, &mut m, vr::RENDER_VIEWS)
                .unwrap_or_else(|e| panic!("seed {s} case {n}: {e}"));
            let built = match port {
                Ok(b) => b,
                Err(why) => {
                    refused += 1;
                    *seen.entry(format!("refused {}", why.name())).or_default() += 1;
                    continue;
                }
            };
            answered += 1;
            built.proposal.apply(&mut EngMut(&mut want)).unwrap();
            // The stack is checked as the frame below; the rest byte for byte.
            let dead = (STACK_TOP - 0x1000) as usize..STACK_TOP as usize;
            for a in dead.clone() {
                want.bytes[a] = m.bytes[a];
            }
            let differing = built.proposal.differing(&Eng(&m));
            assert!(differing.is_empty(), "seed {s} case {n}: fields differ: {differing:?}");
            if want.bytes != m.bytes {
                let at = want.bytes.iter().zip(&m.bytes).position(|(a, b)| a != b).unwrap_or(0);
                panic!("seed {s} case {n}: memory differs first at {at:#x} (a write the port does not make)");
            }
            let call = built.call;
            assert_eq!(
                [c.gpr[0], c.gpr[1], c.gpr[2], c.gpr[3], c.gpr[5], c.gpr[6], c.gpr[7]],
                [call.eax, call.ecx, call.edx, call.ebx, call.ebp, call.esi, call.edi],
                "seed {s} case {n}: registers at render_views"
            );
            assert_eq!(c.xmm[0] as u32, call.xmm0, "seed {s} case {n}: xmm0");
            let sp = c.gpr[4];
            assert_eq!(sp, esp - 36, "seed {s} case {n}: esp");
            assert_eq!(word(&m, sp), vr::RENDER_VIEWS_RETURN);
            assert_eq!([word(&m, sp + 4), word(&m, sp + 8), word(&m, sp + 12)], call.args, "seed {s} case {n}: arguments");
            assert_eq!(
                [word(&m, esp - 4), word(&m, esp - 8), word(&m, esp - 12), word(&m, esp - 16), word(&m, esp - 20)],
                [call.args[1], ebx, ebp, esi, edi],
                "seed {s} case {n}: the saved frame"
            );
        }
    }
    eprintln!("view records: {answered} equal, {refused} refused; {seen:#?}");
    assert!(answered > refused, "the port answers most cases");
    for k in ["count 1", "count 2", "count 3", "count 4"] {
        assert!(seen.contains_key(k), "{k} never seeded");
    }
}
