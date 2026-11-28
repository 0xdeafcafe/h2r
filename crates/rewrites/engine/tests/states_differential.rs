//! The shader draw's render-state list (`engine::render_loop::states`,
//! `0x0001a170`) against the original, lifted and run on the IR
//! interpreter, from the same seeded 64 MB state
//! (docs/graphics/render-loop.md, "The render-state list").
//!
//! Callees are stubs on both sides: `SetRenderState_Simple` writes its value
//! into a model of the GPU's registers (method to value), seeded to agree
//! with D3D's shadow, as the device's pushed state does in a game; the
//! setters and the colour function log and answer from a generator.
//!
//! - `Mode::Exact` is Port grade: equal call logs (target, every register,
//!   `xmm0`, stack arguments), registers at return, and all 64 MB.
//! - `Mode::SkipRedundant` may make fewer Simple calls. What must be equal
//!   is the effect: the GPU register model, D3D's shadow, dirty mask
//!   and `0x004076b8`, every non-Simple call (the
//!   setters and colours, with all their registers and arguments), the
//!   registers at return, and all memory outside the dead stack. Every
//!   Simple call it makes is one the original makes, in the same order, and
//!   every one it skips would have written the value the register already
//!   held.
//!
//! The original's copy of state `0x88` to the device's `+0x790` only fed
//! D3D; the port makes none, so the device word is put back on the
//! original's side before memory is compared.
//!
//! The control word varies (rounding and precision control), and the
//! parameters include NaN, infinities, ties and out-of-range values, for
//! the `fistp` paths.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`); skips without it.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it states_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::states::{
    self, COLOUR, Counts, DEFS, DIRTY, HEADERS, Mode, RenderStates, SETTERS, SHADOW,
    SIMPLE, STATE_88,
};
use engine::render_loop::{Regs, Step};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 2_000_000;

const CTX: u32 = 0x0280_0000;
const PASS: u32 = 0x0281_0000;
const ENTRIES: u32 = 0x0282_0000;
const LIST_WORD: u32 = 0x0283_0000;
const DEV: u32 = 0x0284_0000;
/// `D3D__pDevice`, which only the original reads (its `+0x790` copy of
/// state `0x88`). The port has no device.
const ORIGINAL_DEVICE: u32 = 0x0040_7488;
/// Shadow dwords the states reach (0 to 0xb0).
const SHADOW_LEN: u32 = 0xb1 * 4;

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
    fn pick(&mut self, xs: &[u32]) -> u32 {
        xs[self.below(xs.len() as u32) as usize]
    }
}

fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes())
}

/// One call into a callee, as the callee sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallLog {
    target: u32,
    regs: [u32; 8],
    xmm0: u128,
    args: Vec<u32>,
}

/// One side's world: the call log and the GPU register model.
#[derive(Default)]
struct Side {
    log: Vec<CallLog>,
    gpu: BTreeMap<u32, u32>,
    /// Simple calls that wrote the value the register already held.
    redundant: u32,
}

/// The stub both sides call; `n` counts non-Simple calls only, so the
/// answers line up when one side makes fewer Simple calls. `esp` points at
/// the return address.
fn stub(
    seed: u64,
    target: u32,
    gpr: &mut [u32; 8],
    xmm0: &mut u128,
    mem: &FlatMemory,
    side: &mut Side,
) {
    let esp = gpr[4];
    if target == SIMPLE {
        let old = side.gpu.insert(gpr[1], gpr[2]);
        side.redundant += (old == Some(gpr[2])) as u32;
        side.log.push(CallLog {
            target,
            regs: *gpr,
            xmm0: *xmm0,
            args: Vec::new(),
        });
        // The D3D entry leaves EAX, ECX, EDX as it likes.
        gpr[0] = 0x5151_0000 | gpr[2] & 0xffff;
        gpr[1] ^= 0x00f0_0f00;
        gpr[2] = !gpr[2];
        return;
    }
    // A setter's value, or the colour's pointer: one stack argument each.
    let args = vec![word(mem, esp + 4)];
    let n = side.log.iter().filter(|c| c.target != SIMPLE).count();
    side.log.push(CallLog {
        target,
        regs: *gpr,
        xmm0: *xmm0,
        args,
    });
    let mut r = Rng(seed ^ (n as u64).wrapping_mul(0x1000_0001) ^ target as u64);
    gpr[0] = r.u32();
    if target != COLOUR {
        // The colour function keeps ECX (the caller reads through it).
        gpr[1] = r.u32();
    }
    gpr[2] = r.u32();
    *xmm0 = (r.next() as u128) << 64 | r.next() as u128;
}

struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

fn is_callee(pc: u32) -> bool {
    pc == SIMPLE || pc == COLOUR || SETTERS.contains(&pc)
}

impl Lifted<'_> {
    fn run(
        &mut self,
        seed: u64,
        mem: &mut FlatMemory,
        r: Regs,
        fcw: u16,
    ) -> Result<(Regs, Side), String> {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: states::RENDER_STATES,
            ..Cpu::default()
        };
        cpu.xmm[0] = r.xmm0;
        cpu.x87.control = fcw;
        let mut pc = states::RENDER_STATES;
        let mut side = Side::default();
        seed_gpu(mem, &mut side);
        let mut steps = 0;
        while pc != HARNESS_EXIT {
            if is_callee(pc) {
                let mut x = cpu.xmm[0];
                stub(seed, pc, &mut cpu.gpr, &mut x, mem, &mut side);
                cpu.xmm[0] = x;
                let esp = cpu.gpr[4];
                let target = pc;
                pc = word(mem, esp);
                cpu.gpr[4] = esp + 4 + states::cleanup(target);
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
        let g = cpu.gpr;
        Ok((
            Regs {
                eax: g[0],
                ecx: g[1],
                edx: g[2],
                ebx: g[3],
                esp: g[4],
                ebp: g[5],
                esi: g[6],
                edi: g[7],
                xmm0: cpu.xmm[0],
                xmm1: 0,
            },
            side,
        ))
    }
}

/// The GPU agrees with D3D's shadow for every Simple state.
fn seed_gpu(mem: &FlatMemory, side: &mut Side) {
    for s in 0..0x5c {
        side.gpu
            .insert(word(mem, HEADERS + 4 * s), word(mem, SHADOW + 4 * s));
    }
}

fn port(
    seed: u64,
    mem: &mut FlatMemory,
    mut r: Regs,
    fcw: u16,
    mode: Mode,
) -> Result<(Regs, Side, Counts), String> {
    let mut side = Side::default();
    seed_gpu(mem, &mut side);
    let e = |e: MemFault| format!("{e:?}");
    let (mut m, mut step) = RenderStates::start(&mut r, &mut Eng(mem), fcw, mode).map_err(e)?;
    loop {
        match step {
            Step::Return { to } => {
                assert_eq!(to, HARNESS_EXIT);
                return Ok((r, side, m.counts));
            }
            Step::Call {
                target,
                site,
                cleanup,
            } => {
                assert_eq!(cleanup, states::cleanup(target));
                r.esp -= 4;
                Eng(mem).set_u32(r.esp, site).unwrap();
                let mut gpr = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];
                stub(seed, target, &mut gpr, &mut r.xmm0, mem, &mut side);
                [r.eax, r.ecx, r.edx, r.ebx, _, r.ebp, r.esi, r.edi] = gpr;
                r.esp += 4 + cleanup;
                step = m.resume(&mut r, &mut Eng(mem)).map_err(e)?;
            }
        }
    }
}

/// Floats the conversions care about.
fn float(r: &mut Rng) -> u32 {
    match r.below(12) {
        0 => 0.0f32.to_bits(),
        1 => 0.5f32.to_bits(),
        2 => 1.0f32.to_bits(),
        3 => 2.5f32.to_bits(),
        4 => (-1.5f32).to_bits(),
        5 => 0.50196f32.to_bits(),
        6 => f32::NAN.to_bits(),
        7 => f32::INFINITY.to_bits(),
        8 => 3.0e9f32.to_bits(),
        9 => (1.0f32 + f32::EPSILON).to_bits(),
        10 => 1.0e-40f32.to_bits(),
        _ => r.u32(),
    }
}

const FCWS: [u32; 7] = [0x037f, 0x027f, 0x007f, 0x0f7f, 0x077f, 0x0b7f, 0x087f];

/// A seeded list and its world: `defs` state definitions over the four
/// kinds of state, `count` entries.
fn state(image: &FlatMemory, seed: u64, case: u32) -> (FlatMemory, Regs, u16) {
    let mut mem = image.clone();
    let mut r = Rng(seed.wrapping_mul(0x51_7cc1_b727_220a) ^ case as u64);
    // D3D's shadow, dirty mask, device.
    for s in 0..SHADOW_LEN / 4 {
        let any = r.u32();
        let v = r.pick(&[0, 1, 0xff00_0000, 0x0012_3456, 0x80, any]);
        put(&mut mem, SHADOW + 4 * s, v);
    }
    put(&mut mem, DIRTY, r.u32());
    put(&mut mem, ORIGINAL_DEVICE, DEV);
    put(&mut mem, STATE_88, r.u32());
    // 64 state definitions: Simple, deferred, 0x88, setters, unknown.
    for d in 0..64u32 {
        let state = match r.below(10) {
            0..=3 => r.below(0x5c),
            4 | 5 => 0x5c + r.below(0x2c),
            6 => 0x88,
            7 | 8 => 0x89 + r.below(29),
            _ => 0xa6 + r.below(10),
        };
        let ty = r.pick(&[0, 0, 1, 1, 2, 2, 3]);
        let rec = DEFS + 4 * d;
        mem.bytes[rec as usize..rec as usize + 2].copy_from_slice(&(state as u16).to_le_bytes());
        mem.bytes[rec as usize + 2] = r.u32() as u8;
        mem.bytes[rec as usize + 3] = ty as u8;
    }
    // The context: parameters (and the colour arguments) at +0x5c...
    for i in 0..0x400 / 4 {
        put(&mut mem, CTX + 4 * i, float(&mut r));
    }
    put(&mut mem, CTX + 0x20, PASS);
    put(&mut mem, PASS + 0x3c, ENTRIES);
    let first = r.below(0x200);
    let count = if case.is_multiple_of(13) {
        0
    } else {
        1 + r.below(24)
    };
    mem.bytes[LIST_WORD as usize..LIST_WORD as usize + 2]
        .copy_from_slice(&((count << 9 | first) as u16).to_le_bytes());
    for i in 0..count {
        let e = ENTRIES + 4 * (first + i);
        mem.bytes[e as usize] = r.below(64) as u8;
        mem.bytes[e as usize + 1] = if r.below(5) == 0 {
            r.below(4) as u8 * 2
        } else {
            1
        };
        mem.bytes[e as usize + 2] = r.u32() as u8;
        mem.bytes[e as usize + 3] = r.below(0x40) as u8;
    }
    for a in (STACK_TOP - 0x1000..STACK_TOP).step_by(8) {
        mem.bytes[a as usize..a as usize + 8].copy_from_slice(&r.next().to_le_bytes());
    }
    let esp = STACK_TOP - 0x400 - 4 * r.below(8);
    put(&mut mem, esp, HARNESS_EXIT);
    put(&mut mem, esp + 4, LIST_WORD);
    let regs = Regs {
        eax: r.u32(),
        ecx: r.u32(),
        edx: r.u32(),
        ebx: CTX,
        esp,
        ebp: r.u32(),
        esi: r.u32(),
        edi: r.u32(),
        xmm0: (r.next() as u128) << 64 | r.next() as u128,
        xmm1: 0,
    };
    let fcw = r.pick(&FCWS) as u16;
    (mem, regs, fcw)
}

fn xbe_bytes() -> Option<Vec<u8>> {
    let data = std::fs::read(xbe_path()?).unwrap();
    assert_eq!(sha256(&data), IMAGE_SHA256, "the retail XBE only");
    Some(data)
}

fn seeds() -> Vec<u64> {
    std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99])
}

#[test]
fn render_states_match_the_lifted_original() {
    let Some(data) = xbe_bytes() else {
        eprintln!("SKIPPED: no XBE (set H2_XBE)");
        return;
    };
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    let mut lifted = Lifted {
        lifter: &lifter,
        cache: BTreeMap::new(),
    };
    let mut by_header: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for st in 0..0x5c {
        by_header
            .entry(word(&image, HEADERS + 4 * st))
            .or_default()
            .push(st);
    }
    let shared: Vec<u32> = by_header
        .values()
        .filter(|v| v.len() > 1)
        .flatten()
        .copied()
        .collect();
    assert_eq!(
        shared,
        states::SHARED_HEADER,
        "states sharing a push header"
    );
    let (mut states_run, mut exact_calls, mut skipped, mut simple_orig) = (0, 0, 0, 0);
    let mut targets: BTreeMap<u32, u64> = BTreeMap::new();
    for seed in seeds() {
        for case in 0..400 {
            let (mut a, regs, fcw) = state(&image, seed, case);
            let mut b = a.clone();
            let mut c = a.clone();
            let device_word = (DEV + 0x790) as usize..(DEV + 0x794) as usize;
            let seeded = a.bytes[device_word.clone()].to_vec();
            let (ra, sa) = lifted.run(seed, &mut a, regs, fcw).unwrap();
            a.bytes[device_word].copy_from_slice(&seeded);
            let (rb, sb, _) = port(seed, &mut b, regs, fcw, Mode::Exact).unwrap();
            let ctx = format!("seed {seed} case {case} fcw {fcw:#x}");
            for (i, (x, y)) in sa.log.iter().zip(&sb.log).enumerate() {
                assert_eq!(x, y, "{ctx}: call {i}");
            }
            assert_eq!(sa.log.len(), sb.log.len(), "{ctx}: call count");
            assert_eq!(ra, rb, "{ctx}: registers at return");
            if a.bytes != b.bytes
                && let Some(i) = a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y)
            {
                panic!("{ctx}: memory differs first at {i:#x}");
            }
            // Skipping: the same effect.
            let (rc, sc, counts) = port(seed, &mut c, regs, fcw, Mode::SkipRedundant).unwrap();
            assert_eq!(sa.gpu, sc.gpu, "{ctx}: GPU registers");
            let others = |s: &Side| -> Vec<CallLog> {
                s.log
                    .iter()
                    .filter(|c| c.target != SIMPLE)
                    .cloned()
                    .collect()
            };
            assert_eq!(others(&sa), others(&sc), "{ctx}: setter and colour calls");
            let simple = |s: &Side| -> Vec<(u32, u32)> {
                s.log
                    .iter()
                    .filter(|c| c.target == SIMPLE)
                    .map(|c| (c.regs[1], c.regs[2]))
                    .collect()
            };
            let (all, made) = (simple(&sa), simple(&sc));
            let mut it = all.iter();
            for call in &made {
                assert!(
                    it.any(|x| x == call),
                    "{ctx}: Simple {call:x?} not in the original's order"
                );
            }
            assert_eq!(
                all.len() - made.len(),
                counts.skipped as usize,
                "{ctx}: skipped count"
            );
            assert_eq!(
                (
                    rc.ebx, rc.esp, rc.ebp, rc.esi, rc.edi, rc.eax, rc.ecx, rc.edx
                ),
                (
                    ra.ebx, ra.esp, ra.ebp, ra.esi, ra.edi, ra.eax, ra.ecx, ra.edx
                ),
                "{ctx}: registers at return"
            );
            // Everything but the dead stack.
            let lo = (regs.esp - 0x100) as usize;
            let hi = regs.esp as usize;
            if a.bytes[..lo] != c.bytes[..lo] || a.bytes[hi..] != c.bytes[hi..] {
                let i = a
                    .bytes
                    .iter()
                    .zip(&c.bytes)
                    .enumerate()
                    .position(|(i, (x, y))| (i < lo || i >= hi) && x != y)
                    .unwrap();
                panic!("{ctx}: skip mode: memory differs first at {i:#x}");
            }
            states_run += 1;
            exact_calls += sa.log.len();
            skipped += counts.skipped;
            simple_orig += all.len();
            for call in &sa.log {
                *targets.entry(call.target).or_default() += 1;
            }
        }
    }
    eprintln!(
        "render states: {states_run} states, {exact_calls} calls equal (exact); skip mode: {skipped} of {simple_orig} Simple calls skipped, same GPU state, shadow and other calls; targets {}",
        targets.len()
    );
    assert!(targets.contains_key(&SIMPLE) && targets.contains_key(&COLOUR));
    assert!(
        targets.len() > 20,
        "too few setters reached: {}",
        targets.len()
    );
}

/// A per-draw workload: `draws` shader draws, each applying one of
/// `shaders` lists (8 entries of Simple states, the rest mixed). Counts the
/// Simple calls the original makes and the port skips.
#[test]
fn skipping_on_a_per_draw_workload() {
    let Some(data) = xbe_bytes() else {
        eprintln!("SKIPPED: no XBE (set H2_XBE)");
        return;
    };
    let x = xbe::Xbe::parse(&data).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    for (shaders, label) in [
        (1u32, "one shader"),
        (4, "four shaders, sorted"),
        (4, "four shaders, interleaved"),
    ] {
        let (mut mem, regs, _) = state(&image, 5, 1);
        let fcw = 0x027f;
        let mut r = Rng(42);
        // Shader k's list: 12 entries at first = 16·k, Simple states over
        // 16 states shared between shaders, parameters per shader.
        for k in 0..shaders {
            for i in 0..12u32 {
                let e = ENTRIES + 4 * (16 * k + i);
                let d = (k * 12 + i) % 64;
                let rec = DEFS + 4 * d;
                let state = if i < 8 {
                    r.below(16)
                } else {
                    0x5c + r.below(0x2c)
                };
                mem.bytes[rec as usize..rec as usize + 2]
                    .copy_from_slice(&(state as u16).to_le_bytes());
                mem.bytes[rec as usize + 3] = r.pick(&[1, 2]) as u8;
                mem.bytes[e as usize] = d as u8;
                mem.bytes[e as usize + 1] = 1;
                mem.bytes[e as usize + 3] = (k * 12 + i) as u8;
            }
        }
        let draws = 400u32;
        let (mut made, mut skipped) = (0u32, 0u32);
        for n in 0..draws {
            let k = if label.ends_with("sorted") {
                n * shaders / draws
            } else {
                n % shaders
            };
            let w = (12 << 9) | (16 * k);
            mem.bytes[LIST_WORD as usize..LIST_WORD as usize + 2]
                .copy_from_slice(&(w as u16).to_le_bytes());
            let (_, _, counts) = port(5, &mut mem, regs, fcw, Mode::SkipRedundant).unwrap();
            made += counts.simple;
            skipped += counts.skipped;
        }
        eprintln!(
            "{label}: {draws} draws, {} Simple calls in the original, {skipped} skipped ({:.1}%)",
            made + skipped,
            100.0 * skipped as f64 / (made + skipped) as f64
        );
        assert!(skipped > 0);
    }
}
