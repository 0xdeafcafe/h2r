//! `engine::particle_walk` against the original `0x00175610`, lifted and
//! run on the IR interpreter from the same seeded state, to its return:
//! the real callback `0x00224550` runs lifted for each particle, its call
//! of the emitter `0x0023ed30` returned at once (`ret 0x20`). At each
//! callback entry the seven arguments (the vectors read through) must be
//! the model's [`Emitted`], at each emitter call the block's sixteen inputs
//! its `inputs`, and at the return the input block and every seeded record
//! the model's (its writes applied), the particle-model singletons among
//! them. A walk the model refuses by name must reach what it names
//! (`0x0003eb70`, `0x00248450`) before any callback.
//!
//! Each case seeds an `effe` or `bsdt` system, often under one or two
//! `prt3` or `PRTM` parents (a child system: `0x001751d0` asks the
//! parent's particle tag's singleton for its systems block), now and then
//! a scaled or a local-space one, up to three emitters of up to four
//! particles each, emitter properties per emitter or per particle reading
//! any input, the input block's cache and records, the clock, the record
//! table and the view, and a definition the callback reads its inputs for,
//! under several MXCSR and x87 control words (exceptions masked).
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo xtask test -p engine --features differential \
//!     --test it particle_walk_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

use std::collections::BTreeMap;

mod common;

use common::{IMAGE_SHA256, sha256, xbe_path};
use guest_layout::memory::{GuestMemory, MemFault};
use engine::particle_emission::{self as pe, EMIT};
use engine::particle_walk::{self as pw, Call, EMITTERS, ENTRY, PARTICLES, Refused, TAGS};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STEP_LIMIT: u64 = 4_000_000;
/// The calls a named refusal stands for.
const SCALE: u32 = 0x0003_eb70;
const TRANSFORM: u32 = 0x0024_8450;

const DEF: u32 = 0x0300_0000;
const STATE: u32 = 0x0301_0000;
const DRAW: u32 = 0x0302_0000;
const BLOBS: u32 = 0x0303_0000;
const SYSTEM: u32 = 0x0304_0000;
const LOCATION: u32 = 0x0304_0100;
/// A child system's parent and its parent.
const PARENT: u32 = 0x0304_0200;
const GRANDPARENT: u32 = 0x0304_0300;
const TAG_TABLE: u32 = 0x0305_0000;
const TAG_DATA: u32 = 0x0305_0100;
const EVENTS: u32 = 0x0305_0200;
/// The particle tags' data: `prt3` `+0x88` and `PRTM` `+0x7c` the systems.
const PARTICLE_DATA: u32 = 0x0305_0300;
const SYSTEMS: u32 = 0x0305_0400;
const EMITTER_DEFS: u32 = 0x0306_0000;
const HEADERS: u32 = 0x0307_0000;
const EMITTER_ARRAY: u32 = 0x0307_1000;
const PARTICLE_ARRAY: u32 = 0x0307_2000;
const CLOCK: u32 = 0x0308_0000;
const RECORD_TABLE: u32 = 0x0308_1000;
/// The seeded records: each compared whole after the run.
const REGIONS: [(u32, u32); 18] = [
    (DEF, 0x100),
    (STATE, 0x60),
    (DRAW, 0x10),
    (SYSTEM, 0x50),
    (LOCATION, 0x40),
    (PARENT, 0x50),
    (GRANDPARENT, 0x50),
    (PARTICLE_DATA, 0x90),
    (pw::SINGLETONS[0].1, 0x18),
    (TAG_TABLE, 0x40),
    (TAG_DATA, 0x20),
    (EVENTS, 0x80),
    (SYSTEMS, 0x80),
    (EMITTER_DEFS, 3 * 0xb8),
    (EMITTER_ARRAY, 8 * 0x4c),
    (PARTICLE_ARRAY, 16 * 0x40),
    (CLOCK, 0x10),
    (RECORD_TABLE, 0x800),
];

struct Ro<'a>(&'a FlatMemory);

impl GuestMemory for Ro<'_> {
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
        Err(MemFault {
            addr,
            len: data.len() as u32,
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
    fn chance(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
    /// A float in -1..2 mostly, and now and then an edge.
    fn float(&mut self) -> u32 {
        match self.below(24) {
            0 => 0,
            1 => 0x8000_0000,
            2 => 0x7f80_0000,
            3 => 0x7fc0_0000 | (self.u32() & 0x803f_ffff),
            4 => self.u32() & 0x807f_ffff,
            5 => 0x3f80_0000,
            _ => ((self.below(3000) as f32 - 1000.0) / 1000.0).to_bits(),
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

impl Lifted<'_> {
    fn lift(&mut self, entry: u32) {
        if let Some(f) = self.lifter.lift(entry) {
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.entry(insn.pc).or_insert(insn);
            }
        }
    }

    /// From `cpu.eip` to the first of `exits`.
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
        cpu.eip = pc;
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

/// A property at `at` reading any of the seventeen inputs: a function of
/// type 0 (identity), 1 (constant) or 4 (linear), ranged or not, a
/// modifier now and then.
fn property(a: &mut FlatMemory, r: &mut Rng, at: u32, blob: u32) {
    put16(a, at, r.below(17) as u16);
    put16(a, at + 2, r.below(17) as u16);
    put16(a, at + 4, r.below(3) as u16);
    put16(a, at + 6, if r.chance(2) { u16::MAX } else { r.below(17) as u16 });
    put(a, at + 8, 1);
    put(a, at + 0xc, blob);
    let kind = [0u8, 0, 1, 4][r.below(4) as usize];
    a.bytes[blob as usize] = kind;
    a.bytes[blob as usize + 1] = (r.below(2) as u8) | if r.chance(4) { 0x10 } else { 0 };
    for o in (4..0x40).step_by(4) {
        let v = r.float();
        put(a, blob + o, v);
    }
}

/// A colour property: no function, a single colour or a grey ramp.
fn colour(a: &mut FlatMemory, r: &mut Rng, at: u32, blob: u32) {
    property(a, r, at, blob);
    let (kind, flags) = [(1u8, 0x10u8), (0, 0x00), (0, 0x10), (2, 0x20)][r.below(4) as usize];
    a.bytes[blob as usize] = kind;
    a.bytes[blob as usize + 1] = flags | (r.below(2) as u8);
    put(a, blob + 4, r.u32());
    put(a, blob + 8, r.u32());
    put(a, blob + 0x10, r.u32());
}

/// `n` distinct indices below `of`.
fn distinct(r: &mut Rng, n: u32, of: u32) -> Vec<u32> {
    let mut v: Vec<u32> = (0..of).collect();
    for i in 0..of as usize {
        let j = i + r.below(of - i as u32) as usize;
        v.swap(i, j);
    }
    v.truncate(n as usize);
    v
}

/// A list of `indices` through `+4`-style links at `link` of `record(i)`;
/// returns the head.
fn chain(a: &mut FlatMemory, r: &mut Rng, indices: &[u32], record: impl Fn(u32) -> u32, link: u32) -> u32 {
    for w in indices.windows(2) {
        put(a, record(w[0]) + link, w[1] | (r.below(4) << 16));
    }
    if let Some(&last) = indices.last() {
        put(a, record(last) + link, u32::MAX);
    }
    indices.first().map_or(u32::MAX, |&i| i | (r.below(4) << 16))
}

/// A case's state and the parents its system's definition goes through.
fn case(image: &FlatMemory, r: &mut Rng) -> (FlatMemory, u32) {
    let mut a = image.clone();
    let mut blob = BLOBS;
    let mut next_blob = || {
        blob += 0x100;
        blob
    };
    // The definition the callback draws: its own window, no bitmap.
    let flags = (r.u32() & !0x1_31be) | 0x1_0000 | if r.chance(2) { 0x2006 } else { 0 };
    put(&mut a, DEF, flags);
    put(&mut a, DEF + 0x98, r.u32() | if r.chance(3) { 0xf80f } else { 0 });
    colour(&mut a, r, DEF + 0x1c, next_blob());
    for at in [0x2c, 0x3c, 0x4c, 0x5c] {
        property(&mut a, r, DEF + at, next_blob());
    }
    for o in (0x9c..0xbc).step_by(4) {
        let v = r.float();
        put(&mut a, DEF + o, v);
    }
    put(&mut a, DRAW, DEF);
    put(&mut a, DRAW + 0xc, r.float());

    // The tags and the system's definition. Entry 0 the root's (`effe`
    // or `bsdt`), entry 1 a child system's (`prt3` or `PRTM`), entries 2
    // and 3 the particle tags the systems name at `+4`, one of each group.
    const PRT3: u32 = 0x7072_7433;
    const PRTM: u32 = 0x5052_544d;
    put(&mut a, TAGS, TAG_TABLE);
    let root = if r.chance(3) { 0x6273_6474 } else { 0x6566_6665 };
    put(&mut a, TAG_TABLE, root);
    put(&mut a, TAG_TABLE + 8, TAG_DATA);
    put(&mut a, TAG_TABLE + 16, if r.chance(2) { PRT3 } else { PRTM });
    let models = if r.chance(2) { [PRT3, PRTM] } else { [PRTM, PRT3] };
    for (i, g) in models.into_iter().enumerate() {
        let entry = TAG_TABLE + 16 * (2 + i as u32);
        put(&mut a, entry, g);
        put(&mut a, entry + 8, PARTICLE_DATA);
    }
    put(&mut a, PARTICLE_DATA + 0x7c, SYSTEMS);
    put(&mut a, PARTICLE_DATA + 0x88, SYSTEMS);
    for (_, object, vtable, _) in pw::SINGLETONS {
        put(&mut a, object, vtable);
        put(&mut a, object + 4, r.u32());
        put(&mut a, object + 8, r.u32());
    }
    put(&mut a, TAG_DATA + 0x18, if root == 0x6566_6665 { EVENTS } else { SYSTEMS });
    for e in 0..2 {
        put(&mut a, EVENTS + 0x38 * e + 0x34, SYSTEMS);
    }
    let depth = match r.below(10) {
        0..=3 => 1,
        4 | 5 => 2,
        _ => 0,
    };
    let lineage = [SYSTEM, PARENT, GRANDPARENT];
    for &at in &lineage[1..] {
        for o in (0..0x50).step_by(4) {
            let v = if r.chance(2) { r.u32() } else { r.float() };
            put(&mut a, at + o, v);
        }
    }
    for o in (0..0x50).step_by(4) {
        let v = if r.chance(2) { r.u32() } else { r.float() };
        put(&mut a, SYSTEM + o, v);
    }
    put16(&mut a, SYSTEM + 0xc, if r.chance(8) { 0x400 } else { r.u32() as u16 & !0x400 });
    put(&mut a, SYSTEM + 0x18, if r.chance(4) { u32::MAX } else { r.below(4) | (r.below(4) << 16) });
    for (level, &at) in lineage.iter().enumerate().take(depth as usize + 1) {
        let entry = if level == depth as usize { 0 } else { 1 };
        put16(&mut a, at + 0xe, r.below(2) as u16);
        put(&mut a, at + 0x10, entry | (r.below(0x1000) << 16));
        put(&mut a, at + 0x14, r.below(2));
        if level < depth as usize {
            put(&mut a, at + 0x48, lineage[level + 1]);
        }
    }
    for s in 0..2 {
        let d = SYSTEMS + 0x38 * s;
        put(&mut a, d + 4, (2 + r.below(2)) | (r.below(0x1000) << 16));
        put16(&mut a, d + 0xc, if r.chance(8) { 1 } else { 0 });
        put(&mut a, d + 0x34, EMITTER_DEFS);
    }

    // The emitters and particles.
    let emitters = r.below(4);
    let mut particles = distinct(r, 12, 16).into_iter();
    let eids = distinct(r, emitters, 8);
    for (k, &i) in eids.iter().enumerate() {
        let e = EMITTER_DEFS + 0xb8 * k as u32;
        property(&mut a, r, e + 0x48, next_blob());
        colour(&mut a, r, e + 0x58, next_blob());
        property(&mut a, r, e + 0x68, next_blob());
        put(&mut a, e + 0xb0, r.u32());
        put(&mut a, e + 0xb4, r.u32());
        let n = r.below(5);
        let mine: Vec<u32> = (0..n).filter_map(|_| particles.next()).collect();
        for &p in &mine {
            let at = PARTICLE_ARRAY + 0x40 * p;
            for o in (0..0x40).step_by(4) {
                let v = if r.chance(4) { r.u32() } else { r.float() };
                put(&mut a, at + o, v);
            }
        }
        let head = chain(&mut a, r, &mine, |p| PARTICLE_ARRAY + 0x40 * p, 4);
        put(&mut a, EMITTER_ARRAY + 0x4c * i + 4, head);
    }
    put(&mut a, EMITTERS, HEADERS);
    put(&mut a, HEADERS + 0x44, EMITTER_ARRAY);
    put(&mut a, PARTICLES, HEADERS + 0x80);
    put(&mut a, HEADERS + 0x80 + 0x44, PARTICLE_ARRAY);
    for o in (0..0x40).step_by(4) {
        let v = if r.chance(2) { r.u32() } else { r.float() };
        put(&mut a, LOCATION + o, v);
    }
    let head = chain(&mut a, r, &eids, |e| EMITTER_ARRAY + 0x4c * e, 8);
    put(&mut a, LOCATION + 4, head);

    // The inputs' other records.
    put(&mut a, pe::constant::CLOCK, CLOCK);
    put(&mut a, CLOCK + 4, r.float());
    put(&mut a, CLOCK + 8, r.u32());
    put(&mut a, pe::constant::RECORDS, RECORD_TABLE);
    put(&mut a, RECORD_TABLE + 0x44, RECORD_TABLE + 0x100);
    for o in (0x100..0x800).step_by(4) {
        let v = r.float();
        put(&mut a, RECORD_TABLE + o, v);
    }
    for at in (pe::constant::CAMERA..pe::constant::UP + 0xc).step_by(4) {
        let v = r.float();
        put(&mut a, at, v);
    }

    // The input block: its cache, the mask, the records.
    for o in (0..0x44).step_by(4) {
        let v = r.float();
        put(&mut a, STATE + o, v);
    }
    put(&mut a, STATE + 0x44, if r.chance(3) { 0 } else { r.u32() });
    put(&mut a, STATE + 0x48, if r.chance(2) { SYSTEM } else { r.u32() });
    put(&mut a, STATE + 0x4c, if r.chance(2) { LOCATION } else { r.u32() });
    let first = word(&a, EMITTER_ARRAY + 0x4c * eids.first().copied().unwrap_or(0) + 4);
    let first = PARTICLE_ARRAY + 0x40 * (first & 0xffff);
    put(&mut a, STATE + 0x50, if r.chance(3) { first } else { r.u32() });
    (a, depth)
}

#[test]
fn walk_matches_the_lifted_original() {
    if !with_image(cases) {
        eprintln!("SKIPPED walk_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

/// The lifted walk's callbacks: the arguments at entry, the inputs at the
/// emitter call.
struct Seen {
    calls: Vec<([u32; 7], [u32; 3], [u32; 3], [u32; 4])>,
    inputs: Vec<[u32; 16]>,
    /// The first named call reached before any callback.
    named: Option<u32>,
}

fn run(lifted: &mut Lifted<'_>, c: &mut Cpu, a: &mut FlatMemory) -> Result<Seen, String> {
    let mut seen = Seen { calls: Vec::new(), inputs: Vec::new(), named: None };
    let exits = [pe::ENTRY, EMIT, SCALE, TRANSFORM, HARNESS_EXIT];
    loop {
        let pc = lifted.run(c, a, &exits)?;
        let esp = c.gpr[4];
        match pc {
            HARNESS_EXIT => return Ok(seen),
            SCALE | TRANSFORM if seen.calls.is_empty() => {
                seen.named = Some(pc);
                return Ok(seen);
            }
            pe::ENTRY => {
                let args: [u32; 7] = std::array::from_fn(|i| word(a, esp + 4 + 4 * i as u32));
                let v3 = |p: u32| [0, 4, 8].map(|o| word(a, p + o));
                let tint = [0, 4, 8, 12].map(|o| word(a, args[5] + o));
                seen.calls.push((args, v3(args[2]), v3(args[3]), tint));
            }
            EMIT => {
                seen.inputs.push(std::array::from_fn(|i| word(a, STATE + 4 * i as u32)));
                // The emitter returns at once: `ret 0x20`.
                c.eip = word(a, esp);
                c.gpr[4] = esp + 4 + 0x20;
                continue;
            }
            _ => return Err(format!("{pc:#x} after a callback")),
        }
        // Step past the hook's own address.
        let insn = lifted.cache.get(&pc).cloned().ok_or("not lifted")?;
        c.eip = interp::step(c, a, &insn).map_err(|e| format!("{pc:#x}: {e:?}"))?.target();
    }
}

fn cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    lifted.lift(ENTRY);
    lifted.lift(pe::ENTRY);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for seed in seeds() {
        for n in 0..512u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ n as u64);
            let (mut a, depth) = case(image, &mut r);
            let (mx, cw) = (mxcsr(&mut r), x87_control(&mut r));
            let esp = STACK_TOP - 0x400;
            put(&mut a, esp, HARNESS_EXIT);
            for (i, v) in [SYSTEM, LOCATION, pe::ENTRY, DRAW].into_iter().enumerate() {
                put(&mut a, esp + 4 + 4 * i as u32, v);
            }
            let call = Call::at(&Ro(&a), esp, STATE).unwrap();
            let want = pw::walk(&Ro(&a), call, mx, cw);
            let before = a.clone();
            let mut c = Cpu {
                gpr: [r.u32(), r.u32(), r.u32(), r.u32(), esp, r.u32(), r.u32(), STATE],
                fs_base: RAM as u32,
                eip: ENTRY,
                ..Cpu::default()
            };
            c.mxcsr = mx;
            c.x87.control = cw;
            let ran = run(&mut lifted, &mut c, &mut a);
            let want = match (want, ran) {
                (Ok(w), Ok(s)) if s.named.is_none() => (w, s),
                (Err(e @ (Refused::Scale | Refused::LocalSpace)), ran) => {
                    let call = if e == Refused::Scale { SCALE } else { TRANSFORM };
                    let named = ran.map(|s| s.named);
                    assert_eq!(named, Ok(Some(call)), "seed {seed} case {n}: {e:?} is the original's call");
                    *seen.entry(e.name()).or_default() += 1;
                    continue;
                }
                (Err(e), _) => {
                    *seen.entry(e.name()).or_default() += 1;
                    continue;
                }
                (Ok(_), other) => panic!(
                    "seed {seed} case {n}: the original did not walk to its return: {:?}",
                    other.map(|s| s.named)
                ),
            };
            let (w, s) = want;
            assert_eq!(c.gpr[4], esp + 4 + 0x10, "seed {seed} case {n}: ret 0x10");
            assert_eq!(s.calls.len(), w.emitted.len(), "seed {seed} case {n}: the callbacks");
            assert_eq!(s.inputs.len(), w.emitted.len(), "seed {seed} case {n}: the emitter calls");
            for (i, ((args, position, axis, tint), e)) in s.calls.iter().zip(&w.emitted).enumerate() {
                let at = format!("seed {seed} case {n} particle {i}");
                assert_eq!((args[0], args[1], args[6]), (e.particle, STATE, DRAW), "{at}: particle, block, draw");
                assert_eq!(*position, e.position, "{at}: the position");
                assert_eq!(*axis, e.axis, "{at}: the axis");
                assert_eq!(args[4], e.size, "{at}: the size");
                assert_eq!(*tint, e.tint, "{at}: the tint");
                assert_eq!(s.inputs[i], e.inputs, "{at}: the inputs");
                assert_eq!(word(&before, e.particle + 0x14), e.words, "{at}: the words");
            }
            let mut expect = before;
            for (&addr, &b) in &w.writes {
                expect.bytes[addr as usize] = b;
            }
            for (base, len) in REGIONS {
                let r = base as usize..(base + len) as usize;
                assert_eq!(a.bytes[r.clone()], expect.bytes[r], "seed {seed} case {n}: the record at {base:#x}");
            }
            let shape = match w.emitted.len() {
                0 => "no particle",
                1 => "one particle",
                _ => "particles",
            };
            *seen.entry(shape).or_default() += 1;
            if depth > 0 && !w.emitted.is_empty() {
                *seen.entry(["nested", "nested twice"][depth as usize - 1]).or_default() += 1;
            }
        }
    }
    eprintln!("walk: all equal; {seen:#?}");
    for k in ["no particle", "particles", "scale", "local_space", "nested", "nested twice"] {
        assert!(seen.contains_key(k), "{k} never seen: widen the states");
    }
}
