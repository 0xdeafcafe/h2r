//! `engine::sim_anim` against the lifted guest: `0x2798a0` and everything
//! under it (dispatcher, evaluators, codecs) run on the IR interpreter
//! with nothing stubbed. Private XBE as the oracle, synthetic animations.
//!
//! The native body replaces a whole subtree, so the comparison is final
//! state, not the store transcript: guest memory outside the dead stack
//! and the family's scratch globals (docs/engine/anim-sampling.md
//! "Writes"), the callee-saved registers, `esp`, the return target,
//! MXCSR and the x87 control, status and tag words. Each case runs the
//! native body cold (empty clip store) on host lanes and warm (the clip
//! the cold run built) on the scalar loop; both must match the guest.
#![cfg(feature = "differential")]

mod common;

use ::sim_anim::{ClipStore, RotOp, VecOp};
use engine::sim_anim::{self as adapter, CODECS, rotation_op, scale_op, translation_op};
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use runtime::{CpuAbi, GuestFault};
use sim_core::Epoch;
use std::collections::{BTreeMap, BTreeSet};

const STACK: u32 = 0x007f_f000;
const VIEW: u32 = 0x0062_0000;
const ELEMENT: u32 = VIEW + 0x100;
const SIZES: u32 = ELEMENT + 0x3c;
const MASK: u32 = VIEW + 0x400;
const REMAP: u32 = VIEW + 0x600;
const GRAPH: u32 = VIEW + 0x800;
const OUT: u32 = 0x0063_0000;
const DATA: u32 = 0x0064_0000;

/// Written by the original, read by nothing outside the family.
const UNCOMPARED: [(u32, u32); 5] = [
    (STACK - 0x400, STACK),
    (STACK + 4, STACK + 4 + 0x28),
    (0x0050_4400, 0x0050_4500),
    (0x0055_e500, 0x0055_e600),
    (0x0055_eb80, 0x0055_ec00),
];

#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    overlay: BTreeMap<u32, u8>,
}

impl Ram<'_> {
    fn get(&self, p: u32) -> u8 {
        self.overlay
            .get(&p)
            .copied()
            .unwrap_or(self.image[p as usize])
    }
    fn read(&self, a: u32, dst: &mut [u8]) {
        for (i, byte) in dst.iter_mut().enumerate() {
            *byte = self.get(a + i as u32);
        }
    }
    fn store(&mut self, a: u32, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let p = a + i as u32;
            assert!((p as usize) < self.image.len());
            self.overlay.insert(p, byte);
        }
    }
    fn word(&mut self, a: u32, v: u32) {
        self.store(a, &v.to_le_bytes());
    }
    /// Addresses either side wrote, outside the uncompared ranges.
    fn touched<'b>(&'b self, other: &'b Ram<'_>) -> BTreeSet<u32> {
        self.overlay
            .keys()
            .chain(other.overlay.keys())
            .copied()
            .filter(|&p| !UNCOMPARED.iter().any(|&(lo, hi)| (lo..hi).contains(&p)))
            .collect()
    }
}

impl engine::sim_native::Memory for Ram<'_> {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _align: u32) -> Result<(), GuestFault> {
        self.read(a, dst);
        Ok(())
    }
    fn write_bytes(&mut self, a: u32, data: &[u8], _align: u32) -> Result<(), GuestFault> {
        self.store(a, data);
        Ok(())
    }
}

impl interp::Memory for Ram<'_> {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        Ram::read(self, a, dst);
        Ok(())
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        self.store(a, data);
        Ok(())
    }
}

fn ir_cpu(c: &CpuAbi) -> Cpu {
    Cpu {
        gpr: [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi],
        eip: c.eip,
        fs_base: c.fs_base,
        eflags: c.eflags,
        flags_valid: c.eflags_defined,
        xmm: c.xmm.map(u128::from_le_bytes),
        mxcsr: c.mxcsr,
        x87: interp::X87 {
            st: c.x87.st,
            control: c.x87.control,
            status: c.x87.status,
            tag: c.x87.tag,
            opcode: c.x87.opcode,
            instruction_pointer: c.x87.instruction_pointer,
            data_pointer: c.x87.data_pointer,
            instruction_selector: c.x87.instruction_selector,
            data_selector: c.x87.data_selector,
        },
        ..Cpu::default()
    }
}

/// What the comparison covers: `ebx`, `esp`, `ebp`, `esi`, `edi`, the
/// next pc, MXCSR, x87 control/status/tag.
fn visible(c: &Cpu) -> [u32; 10] {
    [
        c.gpr[3],
        c.gpr[4],
        c.gpr[5],
        c.gpr[6],
        c.gpr[7],
        c.eip,
        c.mxcsr,
        u32::from(c.x87.control),
        u32::from(c.x87.status),
        u32::from(c.x87.tag),
    ]
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
    fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u32) as usize]
    }
    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        if self.chance(8) {
            return 0.0;
        }
        lo + (hi - lo) * (self.below(1 << 16) as f32 / 65536.0)
    }
}

/// How a channel's data is laid out, from its codec function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Static,
    /// Bytes per rotation key (8 int16, 16 float); 0 for vectors.
    Frames(u32),
    Keys {
        wide: bool,
    },
}

fn rot_shape(op: RotOp) -> Shape {
    match op {
        RotOp::Static16 => Shape::Static,
        RotOp::Frame16 | RotOp::Blend16 => Shape::Frames(8),
        RotOp::FrameRaw | RotOp::BlendF32 => Shape::Frames(16),
        RotOp::KeyInt { wide } | RotOp::KeyInterp { wide } => Shape::Keys { wide },
    }
}

fn vec_shape(op: VecOp) -> Shape {
    match op {
        VecOp::Static => Shape::Static,
        VecOp::FrameRaw | VecOp::FrameLerp => Shape::Frames(0),
        VecOp::KeyInt { wide } | VecOp::KeyInterp { wide } => Shape::Keys { wide },
    }
}

/// The codecs whose six table functions the native body knows, with each
/// channel's layout (both slots must agree on it).
fn codecs(image: &[u8]) -> Vec<(u8, [Shape; 3])> {
    let word = |a: u32| u32::from_le_bytes(image[a as usize..a as usize + 4].try_into().unwrap());
    (0..16u8)
        .filter_map(|codec| {
            let entry = CODECS + 40 * u32::from(codec);
            let slot = |base: u32| -> Option<[Shape; 3]> {
                Some([
                    rot_shape(rotation_op(word(entry + base))?),
                    vec_shape(translation_op(word(entry + base + 4))?),
                    vec_shape(scale_op(word(entry + base + 8))?),
                ])
            };
            let (int, interp) = (slot(0)?, slot(12)?);
            assert_eq!(int, interp, "codec {codec}: slots disagree on layout");
            Some((codec, int))
        })
        .collect()
}

/// One stream under construction.
struct Stream(Vec<u8>);

impl Stream {
    fn at_least(&mut self, len: usize) {
        if self.0.len() < len {
            self.0.resize(len, 0);
        }
    }
    fn put(&mut self, at: u32, bytes: &[u8]) {
        self.at_least(at as usize + bytes.len());
        self.0[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    }
    fn word(&mut self, at: u32, v: u32) {
        self.put(at, &v.to_le_bytes());
    }
    /// Space at the end, 16-aligned.
    fn alloc(&mut self, len: u32) -> u32 {
        let at = self.0.len().next_multiple_of(16) as u32;
        self.at_least((at + len) as usize);
        at
    }
}

fn quat16(rng: &mut Rng) -> Vec<u8> {
    let mut q: [i16; 4] = std::array::from_fn(|_| rng.below(24000) as i16 - 12000);
    if q == [0; 4] {
        q[3] = 1000;
    }
    q.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn floats(rng: &mut Rng, n: usize, lo: f32, hi: f32) -> Vec<u8> {
    (0..n)
        .flat_map(|_| rng.float(lo, hi).to_le_bytes())
        .collect()
}

/// Keyed tracks: per-track words `count | first << 12`, times, values.
fn keys(
    rng: &mut Rng,
    s: &mut Stream,
    tracks: usize,
    frames: u32,
    wide: bool,
    size: u32,
) -> (Vec<u32>, u32, u32) {
    let mut words = Vec::new();
    let mut times = Vec::new();
    for _ in 0..tracks {
        let count = 1 + rng.below(4);
        words.push(count | (times.len() as u32) << 12);
        let mut t = if rng.chance(4) { rng.below(2) } else { 0 };
        for _ in 0..count {
            times.push(t);
            t += 1 + rng.below(frames.max(2));
        }
    }
    // Slack: the integer rotation codec reads one key past a track.
    let total = times.len() as u32 + 2;
    let width = if wide { 2 } else { 1 };
    let time_at = s.alloc(total * width);
    for (i, &t) in times.iter().enumerate() {
        let t = t.min(if wide { 0xffff } else { 0xff });
        if wide {
            s.put(time_at + 2 * i as u32, &(t as u16).to_le_bytes());
        } else {
            s.put(time_at + i as u32, &[t as u8]);
        }
    }
    let value_at = s.alloc(total * size);
    (words, time_at, value_at)
}

fn stream(
    rng: &mut Rng,
    codec: u8,
    shapes: [Shape; 3],
    tracks: [usize; 3],
    frames: u32,
) -> Vec<u8> {
    for c in 1..3 {
        assert!(
            !(matches!(shapes[c], Shape::Keys { .. }) && !matches!(shapes[0], Shape::Keys { .. })),
            "codec {codec}: keyed vectors with a frame rotation layout"
        );
    }
    let mut s = Stream(vec![0; 0x40]);
    s.0[0] = codec;
    match shapes[0] {
        Shape::Static => {
            for k in 0..tracks[0] as u32 {
                let q = quat16(rng);
                s.put(0x20 + 8 * k, &q);
            }
        }
        Shape::Frames(size) => {
            let stride = frames * size;
            s.word(0x14, stride);
            for k in 0..tracks[0] as u32 {
                for a in 0..frames {
                    let q = if size == 8 {
                        quat16(rng)
                    } else {
                        floats(rng, 4, -1.0, 1.0)
                    };
                    s.put(0x20 + stride * k + size * a, &q);
                }
            }
        }
        Shape::Keys { wide } => {
            s.at_least(0x30 + 4 * tracks[0]);
            let (words, times, values) = keys(rng, &mut s, tracks[0], frames, wide, 8);
            s.word(0x14, times);
            s.word(0x20, values);
            let mut n = 0;
            for (k, w) in words.iter().enumerate() {
                s.word(0x30 + 4 * k as u32, *w);
                n += w & 0xfff;
            }
            for i in 0..n + 2 {
                let q = quat16(rng);
                s.put(values + 8 * i, &q);
            }
        }
    }
    for (c, (base, stride, values, width, lo, hi)) in [
        (0x0c, 0x18, 0x24, 3u32, -4.0, 4.0),
        (0x10, 0x1c, 0x28, 1, 0.25, 2.0),
    ]
    .into_iter()
    .enumerate()
    {
        let tracks = tracks[c + 1] as u32;
        let size = 4 * width;
        match shapes[c + 1] {
            Shape::Static => {
                let at = s.alloc(tracks * size);
                s.word(base, at);
                let v = floats(rng, (tracks * width) as usize, lo, hi);
                s.put(at, &v);
            }
            Shape::Frames(_) => {
                let at = s.alloc(tracks * frames * size);
                s.word(base, at);
                s.word(stride, frames * size);
                let v = floats(rng, (tracks * frames * width) as usize, lo, hi);
                s.put(at, &v);
            }
            Shape::Keys { wide } => {
                let word_at = s.alloc(4 * tracks);
                s.word(base, word_at);
                let (words, times, value_at) =
                    keys(rng, &mut s, tracks as usize, frames, wide, size);
                s.word(stride, times);
                s.word(values, value_at);
                let mut n = 0;
                for (k, w) in words.iter().enumerate() {
                    s.word(word_at + 4 * k as u32, *w);
                    n += w & 0xfff;
                }
                let v = floats(rng, ((n + 2) * width) as usize, lo, hi);
                s.put(value_at, &v);
            }
        }
    }
    let len = s.0.len().next_multiple_of(16);
    s.at_least(len);
    s.0
}

/// What a case expects of the native body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// Must match the guest when it runs.
    Match,
    /// Must decline (remap, direct test).
    Decline,
}

/// Build one call in `m` and `c`.
/// `small` and `near_one` are the image's cutoffs (`0x45dbdc`, `0x45dc78`).
fn case(
    rng: &mut Rng,
    codecs: &[(u8, [Shape; 3])],
    (small, near_one): (f32, f32),
    c: &mut CpuAbi,
    m: &mut Ram<'_>,
) -> Expect {
    let nodes = 1 + rng.below(40);
    let frames = 1 + rng.below(6);
    let ty = rng.pick(&[0u8, 0, 1, 2]);
    let flag_bytes = ((nodes + 31) >> 3) & !3;

    let mut blob = Vec::new();
    let mut lens = [0u32; 2];
    let mut flags = Vec::new();
    for (i, len) in lens.iter_mut().enumerate() {
        let mut lists = [0usize; 3];
        let mut bits = vec![0u8; 3 * flag_bytes as usize];
        for (ch, list) in lists.iter_mut().enumerate() {
            for n in 0..nodes {
                if rng.chance(2) {
                    bits[ch * flag_bytes as usize + (n >> 3) as usize] |= 1 << (n & 7);
                    *list += 1;
                }
            }
        }
        let present = if i == 0 {
            rng.chance(2)
        } else {
            !rng.chance(6)
        };
        if present {
            let (codec, shapes) = rng.pick(codecs);
            let s = stream(rng, codec, shapes, lists, frames);
            *len = s.len() as u32;
            blob.extend_from_slice(&s);
        }
        flags.push(bits);
    }
    let mut sizes = [0u8; 16];
    sizes[0] = (3 * flag_bytes) as u8;
    sizes[6..8].copy_from_slice(&(lens[0] as i16).to_le_bytes());
    sizes[12..16].copy_from_slice(&lens[1].to_le_bytes());
    for bits in &flags {
        blob.extend_from_slice(bits);
    }
    m.store(DATA, &blob);
    m.store(SIZES, &sizes);
    m.word(VIEW, DATA);
    m.word(VIEW + 4, SIZES);
    m.store(VIEW + 8, &[nodes as u8]);

    let element_nodes = if rng.chance(4) { rng.below(48) } else { nodes };
    m.store(ELEMENT + 0x10, &[ty]);
    m.store(ELEMENT + 0x13, &[element_nodes as u8]);
    m.store(ELEMENT + 0x14, &(frames as u16).to_le_bytes());
    let direct = rng.chance(10);
    m.word(ELEMENT + 0x64, if direct { 1 } else { 0 });

    let weight = rng.pick(&[0.0f32, 0.00001, 0.3, 0.75, 0.99999, 1.0, 1.0, 1.0]);
    let frame =
        rng.pick(&[0.0f32, 0.5, 1.25, 2.999, 0.000001, -0.5]) + rng.below(frames + 2) as f32;
    c.xmm[1][..4].copy_from_slice(&frame.to_le_bytes());
    c.xmm[4][..4].copy_from_slice(&weight.to_le_bytes());

    let remap = rng.chance(12);
    if remap {
        m.store(REMAP + 0x1c, &[rng.below(2) as u8]);
    }
    let cap = match rng.below(6) {
        0 => rng.below(nodes + 1),
        1 => (-(rng.below(3) as i32)) as u32,
        _ => nodes + rng.below(3),
    };
    let masked = rng.chance(2);
    if masked {
        for k in 0..2 {
            m.word(MASK + 4 * k, rng.next() ^ rng.next() << 16);
        }
    }
    // Mode 0 (full weight, type not 1) with a second frame is the broken
    // evaluator pair: only reproduced when no node is live, and the guest
    // runs garbage otherwise, so it gets no second frame unless the count
    // leaves nothing to do.
    let full = weight > near_one;
    let replace = full && ty != 1;
    let second = rng.chance(3) && (!replace || (cap as i32) <= 0);
    let args = [
        GRAPH,
        ELEMENT,
        cap,
        if remap { REMAP } else { 0 },
        if masked { MASK } else { 0 },
        OUT,
        u32::from(rng.chance(2)),
        u32::from(second),
        (rng.below(frames * 4) as f32 * 0.375).to_bits(),
        rng.pick(&[0.0f32, 0.25, 0.5, 0.875, 1.0]).to_bits(),
    ];
    for (i, v) in args.into_iter().enumerate() {
        m.word(STACK + 4 + 4 * i as u32, v);
    }
    for n in 0..48u32 {
        let rec = OUT + 32 * n;
        let q = floats(rng, 4, -1.0, 1.0);
        m.store(rec, &q);
        let t = floats(rng, 3, -4.0, 4.0);
        m.store(rec + 0x10, &t);
        m.store(rec + 0x1c, &rng.float(0.5, 2.0).to_le_bytes());
    }
    c.esi = VIEW;
    let active = weight > small;
    if active && (remap || (direct && replace && lens[1] != 0)) {
        Expect::Decline
    } else {
        Expect::Match
    }
}

struct Oracle<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, ir::IrInsn>,
}

impl Oracle<'_> {
    fn run(&mut self, c: &mut Cpu, mem: &mut Ram<'_>, mut pc: u32) {
        for _ in 0..2_000_000 {
            c.eip = pc;
            if pc == HARNESS_EXIT {
                return;
            }
            if !self.cache.contains_key(&pc) {
                let f = self
                    .lifter
                    .lift(pc)
                    .unwrap_or_else(|| panic!("function at {pc:#x}"));
                for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                    self.cache.insert(insn.pc, insn);
                }
            }
            pc = interp::step(c, mem, &self.cache[&pc])
                .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
                .target();
        }
        panic!("step limit at {pc:#x}");
    }
}

/// Load the image and the lifter, then run `f`. `None` without `H2_XBE`.
fn with_oracle(name: &str, f: impl FnOnce(&mut Oracle<'_>, &[u8])) {
    let Some(path) = common::xbe_path() else {
        assert!(
            std::env::var_os("H2_XBE").is_none(),
            "H2_XBE does not name a readable file"
        );
        eprintln!("SKIPPED {name}: set H2_XBE");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let lifter = lift::Lifter::new(&analysis, &xbe).unwrap();
    let image = lift::harness::load_image(&xbe).unwrap().bytes;
    let mut oracle = Oracle {
        lifter: &lifter,
        cache: BTreeMap::new(),
    };
    f(&mut oracle, &image);
}

fn entry_cpu(mxcsr: u32, status: u16) -> CpuAbi {
    let mut c = CpuAbi {
        eax: 0x1234_5678,
        ecx: 0x3456_789a,
        edx: 0x2345_6789,
        ebx: 0x4567_89ab,
        edi: 0x6257_1918,
        ebp: STACK - 0x80,
        esp: STACK,
        eip: adapter::STREAM,
        eflags: 0x293,
        mxcsr,
        ..CpuAbi::default()
    };
    c.x87.control = 0x027f;
    c.x87.status = status;
    c
}

/// Run the native body on `(c, m)` cold on host lanes, then warm on the
/// scalar loop with the same store, and compare each with the guest.
/// The [`adapter::Decline`] when it declined (having written nothing).
fn check(
    oracle: &mut Oracle<'_>,
    c: &CpuAbi,
    m: &Ram<'_>,
    store: &ClipStore,
    case: &str,
) -> Result<(), adapter::Decline> {
    let mut reference: Option<(Cpu, Ram<'_>)> = None;
    for (simd, label) in [(true, "cold, host lanes"), (false, "warm, scalar")] {
        let (mut nc, mut nm) = (*c, m.clone());
        let got = engine::sim_native::with_simd(simd, || adapter::stream(&mut nc, &mut nm, store))
            .unwrap();
        let transfer = match got {
            Ok(transfer) => transfer,
            Err(why) => {
                assert_eq!(nm.overlay, m.overlay, "{case}: a declined call wrote");
                assert_eq!(
                    visible(&ir_cpu(&nc)),
                    visible(&ir_cpu(c)),
                    "{case}: a declined call moved registers"
                );
                assert!(
                    simd,
                    "{case}: the warm call declined after the cold call ran"
                );
                return Err(why);
            }
        };
        assert_eq!(transfer.target_pc, HARNESS_EXIT);
        let (guest_cpu, guest_mem) = reference.get_or_insert_with(|| {
            let mut gm = m.clone();
            let mut gc = ir_cpu(c);
            oracle.run(&mut gc, &mut gm, adapter::STREAM);
            (gc, gm)
        });
        let mut got_cpu = ir_cpu(&nc);
        got_cpu.eip = transfer.target_pc;
        assert_eq!(
            visible(&got_cpu),
            visible(guest_cpu),
            "{case} {label}: registers"
        );
        for p in nm.touched(guest_mem) {
            assert_eq!(
                nm.get(p),
                guest_mem.get(p),
                "{case} {label}: byte at {p:#x}"
            );
        }
    }
    assert!(
        store.stats().builds <= 1,
        "{case}: the warm call rebuilt the clip"
    );
    Ok(())
}

static EPOCH: Epoch = Epoch::new();

#[test]
fn animation_stream_matches_the_guest() {
    with_oracle("animation sampling differential test", |oracle, image| {
        let codecs = codecs(image);
        let float =
            |a: u32| f32::from_le_bytes(image[a as usize..a as usize + 4].try_into().unwrap());
        let cutoffs = (float(0x0045_dbdc), float(0x0045_dc78));
        assert!(codecs.len() >= 4, "known codecs: {codecs:?}");
        const CASES: u32 = 600;
        let (mut native, mut declined) = (0u32, 0u32);
        for n in 0..CASES {
            let mut rng =
                Rng(0x9e37_79b9_7f4a_7c15 ^ u64::from(n).wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
            let mut c = entry_cpu(
                if n % 16 == 15 { 0x1f80 } else { 0x1fa0 },
                ((n % 8) as u16) << 11,
            );
            let mut m = Ram {
                image,
                overlay: BTreeMap::new(),
            };
            m.word(STACK, HARNESS_EXIT);
            let expect = case(&mut rng, &codecs, cutoffs, &mut c, &mut m);
            let store = ClipStore::new(&EPOCH);
            if check(oracle, &c, &m, &store, &format!("case {n}")).is_ok() {
                assert_eq!(expect, Expect::Match, "case {n}: should have declined");
                native += 1;
            } else {
                declined += 1;
            }
        }
        eprintln!("animation stream: {native} native, {declined} declined of {CASES}");
        assert!(
            native * 2 > CASES,
            "the native path ran for {native} of {CASES} cases"
        );
    });
}

/// Ladder step 3: every animation on the disc at frame 0, a middle frame,
/// the last frame and a fractional frame (interpolating), full weight, all
/// nodes, against the lifted path. Prints aggregate counts only.
///
/// H2_XBE=... H2_DISC=... cargo test --release -p engine --features differential --test it sim_anim_differential:: -- --ignored --nocapture
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC) and the XBE ($H2_XBE)"]
fn every_disc_animation_matches_the_guest() {
    use map::{FourCC, Map, RawPointer, ResourceFile};
    use std::fs::File;
    use std::io::{BufReader, Read, Seek, SeekFrom};
    use tags::anim::AnimationGraph;

    let iso_path = std::env::var("H2_DISC")
        .unwrap_or_else(|_| format!("{}/../../../disc/halo2.iso", env!("CARGO_MANIFEST_DIR")));
    with_oracle("animation disc sweep", |oracle, image| {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&iso_path).unwrap())).unwrap();
        let entries = iso.entries().unwrap();
        let part = iso.partition_offset();
        let span = |name: &str| {
            entries
                .iter()
                .find(|e| e.path.trim_start_matches('/').eq_ignore_ascii_case(name))
                .map(|e| (part + u64::from(e.sector) * xiso::SECTOR, u64::from(e.size)))
        };
        let read = |spans: &[Option<(u64, u64)>; 4], p: RawPointer, size: u32| -> Option<Vec<u8>> {
            let slot = match p.file {
                ResourceFile::Local => 0,
                ResourceFile::MainMenu => 1,
                ResourceFile::Shared => 2,
                ResourceFile::SinglePlayerShared => 3,
            };
            let (base, len) = spans[slot]?;
            if u64::from(p.offset) + u64::from(size) > len {
                return None;
            }
            let mut f = File::open(&iso_path).ok()?;
            f.seek(SeekFrom::Start(base + u64::from(p.offset))).ok()?;
            let mut v = vec![0; size as usize];
            f.read_exact(&mut v).ok()?;
            Some(v)
        };
        let (mut animations, mut calls, mut native, mut skipped) = (0u64, 0u64, 0u64, 0u64);
        let mut declined: BTreeMap<&'static str, u64> = BTreeMap::new();
        for entry in entries
            .iter()
            .filter(|e| !e.is_dir && e.path.ends_with(".map"))
        {
            let base = part + u64::from(entry.sector) * xiso::SECTOR;
            let mut map = Map::from_region(
                BufReader::new(File::open(&iso_path).unwrap()),
                base,
                entry.size.into(),
            )
            .unwrap();
            let spans = [
                Some((base, u64::from(entry.size))),
                span("maps/mainmenu.map"),
                span("maps/shared.map"),
                span("maps/single_player_shared.map"),
            ];
            let ids: Vec<_> = map
                .tags()
                .iter()
                .filter(|t| t.group == FourCC(*b"jmad"))
                .map(|t| t.id)
                .collect();
            for id in ids {
                let Ok(graph) = AnimationGraph::read(&mut map, id) else {
                    continue;
                };
                let mut resources: BTreeMap<u32, Option<Vec<u8>>> = BTreeMap::new();
                for a in &graph.animations {
                    let s = &a.sizes;
                    if !a.in_resource() || (s.static_size == 0 && s.animated_size == 0) {
                        continue;
                    }
                    animations += 1;
                    let data = resources.entry(a.resource_index).or_insert_with(|| {
                        let r = graph.resources.get(a.resource_index as usize)?;
                        read(&spans, r.pointer?, r.size)
                    });
                    let mut sizes = [0u8; 16];
                    sizes[0] = s.static_flags;
                    sizes[1] = s.animated_flags;
                    sizes[2..4].copy_from_slice(&s.unknown);
                    sizes[4..6].copy_from_slice(&s.unknown_4.to_le_bytes());
                    sizes[6..8].copy_from_slice(&s.static_size.to_le_bytes());
                    sizes[8..12].copy_from_slice(&s.unknown_8.to_le_bytes());
                    sizes[12..16].copy_from_slice(&s.animated_size.to_le_bytes());
                    let shape = ::sim_anim::Shape {
                        sizes: ::sim_anim::Sizes(sizes),
                        nodes: a.node_count,
                    };
                    let blob = ::sim_anim::Layout::new(&shape).and_then(|l| {
                        let d = data.as_ref()?;
                        d.get(a.data_offset as usize..a.data_offset as usize + l.len as usize)
                    });
                    let Some(blob) = blob else {
                        skipped += 1;
                        continue;
                    };
                    // The cache keeps resources 16-aligned; the blob keeps
                    // its offset's alignment.
                    let at = DATA + (a.data_offset & 15);
                    let last = (a.frame_count.max(1) - 1) as f32;
                    let frames = [
                        (0.0, false),
                        ((last / 2.0).floor(), false),
                        (last, false),
                        ((last * 0.37).max(0.0) + 0.25, true),
                    ];
                    let store = ClipStore::new(&EPOCH);
                    for (k, (frame, interpolate)) in frames.into_iter().enumerate() {
                        let mut c = entry_cpu(0x1fa0, 0);
                        let mut m = Ram {
                            image,
                            overlay: BTreeMap::new(),
                        };
                        m.word(STACK, HARNESS_EXIT);
                        m.store(at, blob);
                        m.store(SIZES, &sizes);
                        m.word(VIEW, at);
                        m.word(VIEW + 4, SIZES);
                        m.store(VIEW + 8, &[a.node_count]);
                        m.store(ELEMENT + 0x10, &[a.anim_type.as_u8()]);
                        m.store(ELEMENT + 0x13, &[a.node_count]);
                        m.store(ELEMENT + 0x14, &a.frame_count.to_le_bytes());
                        m.word(ELEMENT + 0x64, 0);
                        c.esi = VIEW;
                        c.xmm[1][..4].copy_from_slice(&f32::to_le_bytes(frame));
                        c.xmm[4][..4].copy_from_slice(&1.0f32.to_le_bytes());
                        let args = [
                            GRAPH,
                            ELEMENT,
                            u32::from(a.node_count),
                            0,
                            0,
                            OUT,
                            u32::from(interpolate),
                            0,
                            0,
                            0,
                        ];
                        for (i, v) in args.into_iter().enumerate() {
                            m.word(STACK + 4 + 4 * i as u32, v);
                        }
                        for n in 0..u32::from(a.node_count) {
                            let rec = OUT + 32 * n;
                            m.store(rec, &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0x3f]);
                            m.store(rec + 0x10, &[0; 12]);
                            m.store(rec + 0x1c, &1.0f32.to_le_bytes());
                        }
                        calls += 1;
                        let label = format!("animation {animations} frame {k}");
                        match check(oracle, &c, &m, &store, &label) {
                            Ok(()) => native += 1,
                            Err(why) => *declined.entry(why.name()).or_default() += 1,
                        }
                    }
                }
            }
        }
        eprintln!(
            "animation disc sweep: {animations} animations, {skipped} without readable data, \
             {calls} calls, {native} native; declined by reason {declined:?}"
        );
        assert!(native > 0);
    });
}
