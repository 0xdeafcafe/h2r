//! `engine::render::light_gather` against the original, lifted and run on
//! the IR interpreter from the same seeded 64 MB state
//! (docs/graphics/native-light-gather.md, slices L1 and L2).
//!
//! **The leaves (L1).** Each root runs lifted with its whole tree (no
//! stub) to its return; the port runs on a copy and its writes are
//! applied. Memory must then be equal byte for byte outside the dead
//! stack, and the answer equal:
//!
//! - `0x00031910`, the visible list (its tree `0x00031590`, `0x0004a7c0`,
//!   `0x0004ac30`, `0x0004b160`, `0x0003e9c0`, `0x000baf80` and what they
//!   call): EAX the count;
//! - `0x00031660`, the geometry flags;
//! - `0x00031c20`, the extra caster (with `0x00163c80`);
//! - `0x000c17f0`, the light volume: AL.
//!
//! The state is a seeded pool of lights, owners, definitions,
//! placements, a candidate list and the view's history: more than 32 and
//! more than 128 candidates, the frozen override, and each definition bit
//! the code tests. A case the port refuses is counted, not compared; each
//! root must answer most of its cases.
//!
//! **The frame (L2).** See [`frame_transcripts_match_the_lifted_original`].
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`), pinned by hash;
//! skips without it. Nothing is copied from it: thresholds and constants
//! are the image's own words. `H2_DIFF_SEEDS=1,2,3` picks seeds.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it light_gather_differential:: -- --nocapture
//! ```

#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_constants::Fpu;
use engine::memory::{GuestMemory, MemFault};
use engine::render::light_gather::{self as gather, LIGHT_VIS, extra_caster, geometry, history, visible, volume};
use ir::IrInsn;
use ir::interp::{self, Cpu, FlatMemory};

const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const STACK_LOW: u32 = STACK_TOP - 0x0004_0000;
const STEP_LIMIT: u64 = 20_000_000;

const VISIBLE_LIST: u32 = 0x0003_1910;
const GEOMETRY_FLAGS: u32 = 0x0003_1660;
const EXTRA_CASTER: u32 = 0x0003_1c20;
const LIGHT_VOLUME: u32 = 0x000c_17f0;

// Guest globals the gather reads (research points, as the port names them).
const LIGHTS_POINTER: u32 = engine::render::light::ARRAY_POINTER;
const OBJECTS_POINTER: u32 = 0x004e_0300;
const TAGS_POINTER: u32 = engine::tags::TAG_ENTRIES_GLOBAL;
const SCENARIO_POINTER: u32 = 0x004e_0350;
const CANDIDATES_POINTER: u32 = 0x0054_7f98;
const FRAME: u32 = 0x004b_a034;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const LOCAL_PLAYER: u32 = 0x004b_9ed8;
const CAMERA_CONTROLLER: u32 = 0x004e_8c44;
const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
const PLAYER_IDENTITIES: u32 = 0x004e_8c20;
const LIGHT_OPTIONS: u32 = 0x0051_07e8;
const VISIBILITY_OPTIONS: u32 = 0x004e_9ae8;
const LIGHT_BSP_OVERRIDES: u32 = 0x0055_e4d0;
const RENDER_MODE: u32 = 0x0048_5602;
const OVERRIDE: u32 = 0x0051_0c50;
const PROJECTION_DEPTH: [u32; 4] = visible::PROJECTION_DEPTH;
const PIXEL_SCALE: u32 = visible::PIXEL_SCALE;
// Image words the seeds aim near (read, never written).
const EPSILON: u32 = 0x0045_dbdc;
const ZERO: u32 = 0x0045_dbd8;
const COLOUR_THRESHOLD: u32 = 0x0045_dc1c;
const DRAWN_THRESHOLD: u32 = 0x0045_dc58;

// The synthetic heap.
const LIGHT_HEADER: u32 = 0x0300_0000;
const LIGHT_BITMAP: u32 = 0x0300_0800;
const LIGHT_DATA: u32 = 0x0300_1000;
const OBJECT_HEADER: u32 = 0x0301_0000;
const OBJECT_DATA: u32 = 0x0301_1000;
const BODIES: u32 = 0x0302_0000;
const BODY: u32 = 0x200;
const TAG_TABLE: u32 = 0x0304_0000;
const DEFINITIONS: u32 = 0x0305_0000;
const DEFINITION: u32 = 0x100;
const BITMAPS: u32 = 0x0305_8000;
const SCENARIO: u32 = 0x0306_0000;
const PLACEMENTS: u32 = 0x0306_1000;
const CANDIDATE_LIST: u32 = 0x0307_0000;
const OPTIONS: u32 = 0x0308_0000;
const VIS_OPTIONS: u32 = 0x0308_1000;
const OVERRIDE_STRUCT: u32 = 0x0308_2000;
const IDENTITIES: u32 = 0x0308_3000;
const VIS_LISTS: u32 = 0x0309_0000;
const SCRATCH: u32 = 0x030a_0000;

const OBJECT_SLOTS: u32 = 48;
const DEFINITION_COUNT: u32 = 24;
const BITMAP_TAG: u32 = 40;
const PLACEMENT_COUNT: u32 = 16;
const NONE: u32 = u32::MAX;

/// Guest memory over the interpreter's flat RAM.
pub struct Ram(pub FlatMemory);

impl GuestMemory for Ram {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let src = self.0.bytes.get(a..a + buf.len()).ok_or(MemFault { addr, len: buf.len() as u32, write: false })?;
        buf.copy_from_slice(src);
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let a = addr as usize;
        let dst =
            self.0.bytes.get_mut(a..a + data.len()).ok_or(MemFault { addr, len: data.len() as u32, write: true })?;
        dst.copy_from_slice(data);
        Ok(())
    }
}

/// SplitMix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn u32(&mut self) -> u32 {
        self.next() as u32
    }
    pub fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
    pub fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }
    /// A float in `[lo, hi)`.
    pub fn float(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.u32() >> 8) as f32 / (1u32 << 24) as f32;
        lo + (hi - lo) * unit
    }
}

pub fn put(mem: &mut FlatMemory, a: u32, v: u32) {
    mem.bytes[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn putf(mem: &mut FlatMemory, a: u32, v: f32) {
    put(mem, a, v.to_bits());
}

pub fn put16(mem: &mut FlatMemory, a: u32, v: u16) {
    mem.bytes[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn put8(mem: &mut FlatMemory, a: u32, v: u8) {
    mem.bytes[a as usize] = v;
}

pub fn word(mem: &FlatMemory, a: u32) -> u32 {
    u32::from_le_bytes(mem.bytes[a as usize..a as usize + 4].try_into().unwrap())
}

pub fn wordf(mem: &FlatMemory, a: u32) -> f32 {
    f32::from_bits(word(mem, a))
}

/// The lifted original, lifted on demand.
pub struct Lifted<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
}

impl Lifted<'_> {
    pub fn lift(&mut self, entry: u32) {
        if let Some(f) = self.lifter.lift(entry) {
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                self.cache.entry(insn.pc).or_insert(insn);
            }
        }
    }

    /// Steps from `cpu.eip` until one of `exits`, or until `stop` takes a
    /// call target (it is handed the CPU at the callee's first
    /// instruction and returns whether it answered the call). Returns the
    /// exit.
    pub fn run(
        &mut self,
        cpu: &mut Cpu,
        mem: &mut FlatMemory,
        exits: &[u32],
        mut stop: impl FnMut(u32, &mut Cpu, &mut FlatMemory) -> bool,
    ) -> Result<u32, String> {
        let mut pc = cpu.eip;
        let mut steps = 0;
        while !exits.contains(&pc) {
            steps += 1;
            if steps > STEP_LIMIT {
                return Err("step limit".into());
            }
            cpu.eip = pc;
            if stop(pc, cpu, mem) {
                pc = cpu.eip;
                continue;
            }
            if !self.cache.contains_key(&pc) {
                self.lift(pc);
            }
            let insn = self.cache.get(&pc).ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn).map_err(|e| format!("{pc:#x}: {e:?}"))?.target();
        }
        cpu.eip = pc;
        Ok(pc)
    }

    /// Calls `entry` with `cpu`'s registers and `args` on the stack below
    /// `esp`, to its return.
    pub fn call(&mut self, cpu: &mut Cpu, mem: &mut FlatMemory, entry: u32, esp: u32, args: &[u32]) -> Result<(), String> {
        let mut sp = esp;
        for &a in args.iter().rev() {
            sp -= 4;
            put(mem, sp, a);
        }
        sp -= 4;
        put(mem, sp, HARNESS_EXIT);
        cpu.gpr[4] = sp;
        cpu.eip = entry;
        self.run(cpu, mem, &[HARNESS_EXIT], |_, _, _| false).map(|_| ())
    }
}

/// Runs `f` with the image and a lifter over it; `false` without the XBE.
pub fn with_image(f: impl FnOnce(&FlatMemory, Lifted<'_>)) -> bool {
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

pub fn seeds() -> Vec<u64> {
    std::env::var("H2_DIFF_SEEDS")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![1, 7, 99])
}

/// A fresh CPU in the game's float state.
pub fn cpu(r: &mut Rng) -> Cpu {
    let mut cpu = Cpu {
        gpr: [r.u32(), r.u32(), r.u32(), r.u32(), STACK_TOP - 0x2000, r.u32(), r.u32(), r.u32()],
        fs_base: RAM as u32,
        ..Cpu::default()
    };
    cpu.mxcsr = 0x1f80;
    cpu.x87.control = 0x027f;
    cpu
}

pub const FPU: Fpu = Fpu { mxcsr: 0x1f80, x87_control: 0x027f };

/// The first byte where `a` and `b` differ outside `[STACK_LOW, live)`.
pub fn first_difference(a: &FlatMemory, b: &FlatMemory, live: u32) -> Option<usize> {
    let skip = STACK_LOW as usize..live as usize;
    let (x, y) = (&a.bytes, &b.bytes);
    if x[..skip.start] != y[..skip.start] {
        return (0..skip.start).find(|&i| x[i] != y[i]);
    }
    if x[skip.end..] != y[skip.end..] {
        return (skip.end..x.len()).find(|&i| x[i] != y[i]);
    }
    None
}

// ---------------------------------------------------------------------
// The seeded world.

pub fn light_datum(i: u32) -> u32 {
    ((0x4000 | (i * 7 + 1)) << 16) | i
}

pub fn light_record(i: u32) -> u32 {
    LIGHT_DATA + i * 0x110
}

pub fn object_datum(j: u32) -> u32 {
    ((0x2000 | (j * 3 + 1)) << 16) | j
}

pub fn body(j: u32) -> u32 {
    BODIES + j * BODY
}

pub fn definition_datum(k: u32) -> u32 {
    (0xe00a << 16) | k
}

pub fn definition(k: u32) -> u32 {
    DEFINITIONS + k * DEFINITION
}

fn list_header(n: u32) -> u32 {
    VIS_LISTS + n * 0x4000
}

/// Writes a list header at `header` with `capacity`, `count` rows of
/// `handles` (and seeded flags, extras and masks).
fn seed_list(m: &mut FlatMemory, r: &mut Rng, header: u32, capacity: u32, handles: &[u32]) {
    let columns = [header + 0x100, header + 0x1100, header + 0x2100, header + 0x3100];
    put(m, header, capacity);
    put16(m, header + 4, handles.len() as u16);
    put(m, header + 8, columns[0]);
    put(m, header + 0xc, columns[1]);
    put(m, header + 0x10, columns[2]);
    put(m, header + 0x14, columns[3]);
    for (i, &h) in handles.iter().enumerate() {
        let i = i as u32;
        put16(m, columns[0] + 2 * i, r.u32() as u16);
        put(m, columns[1] + 4 * i, h);
        put(m, columns[2] + 4 * i, r.u32());
        put16(m, columns[3] + 2 * i, r.u32() as u16);
    }
}

/// The definition bits the gather and its leaves test.
const DEFINITION_BITS: [u32; 15] =
    [0x4, 0x8, 0x20, 0x40, 0x100, 0x400, 0x800, 0x1000, 0x2000, 0x1_0000, 0x2_0000, 0x4_0000, 0x10_0000, 0x40_0000, 0x1];

/// The seeded pools: `lights` light records, the object pool with parent
/// chains, the definitions with every tested bit, placements, the global
/// options. The candidate list and the history are separate.
pub struct World {
    pub mem: FlatMemory,
    pub lights: u32,
}

impl World {
    pub fn new(image: &FlatMemory, r: &mut Rng, lights: u32) -> Self {
        let mut m = image.clone();
        let eps = wordf(&m, EPSILON);
        let colour = wordf(&m, COLOUR_THRESHOLD).abs().sqrt().max(eps) * 2.0;

        // The light array.
        put(&mut m, LIGHTS_POINTER, LIGHT_HEADER);
        put(&mut m, LIGHT_HEADER + 0x20, engine::render::light::MAXIMUM_COUNT as u32);
        put(&mut m, LIGHT_HEADER + 0x24, 0x110);
        put(&mut m, LIGHT_HEADER + 0x38, lights);
        put(&mut m, LIGHT_HEADER + 0x44, LIGHT_DATA);
        put(&mut m, LIGHT_HEADER + 0x48, LIGHT_BITMAP);
        // The objects.
        put(&mut m, OBJECTS_POINTER, OBJECT_HEADER);
        put(&mut m, OBJECT_HEADER + 0x24, 12);
        put(&mut m, OBJECT_HEADER + 0x38, OBJECT_SLOTS);
        put(&mut m, OBJECT_HEADER + 0x44, OBJECT_DATA);
        for j in 0..OBJECT_SLOTS {
            let slot = OBJECT_DATA + 12 * j;
            put16(&mut m, slot, (object_datum(j) >> 16) as u16);
            put8(&mut m, slot + 3, if r.chance(3) { 2 } else { r.below(4) as u8 });
            put(&mut m, slot + 8, body(j));
            // Parents point to lower slots, so every chain ends.
            let parent = if j == 0 || r.chance(2) { NONE } else { object_datum(r.below(j)) };
            put(&mut m, body(j) + 0x14, parent);
            put8(&mut m, body(j) + 0xaa, r.below(2) as u8);
            put(&mut m, body(j) + 0x13c, r.below(4));
        }
        // The tags: definitions, then gel bitmaps.
        put(&mut m, TAGS_POINTER, TAG_TABLE);
        for k in 0..DEFINITION_COUNT {
            let entry = TAG_TABLE + 16 * k;
            put(&mut m, entry, u32::from_be_bytes(*b"ligh"));
            put(&mut m, entry + 4, definition_datum(k));
            put(&mut m, entry + 8, definition(k));
            let d = definition(k);
            let mut flags = 0;
            for bit in DEFINITION_BITS {
                if r.chance(3) {
                    flags |= bit;
                }
            }
            put(&mut m, d, flags | (r.u32() & 0x0080_0000));
            put16(&mut m, d + 4, if r.chance(16) { 4 + r.below(3) as u16 } else { r.below(4) as u16 });
            let near = r.float(0.5, 4.0);
            putf(&mut m, d + 0x18, near);
            putf(&mut m, d + 0x1c, if r.chance(4) { 0.0 } else { near * r.float(1.0, 8.0) });
            for o in [0x20, 0x24, 0x28, 0x2c, 0x30] {
                putf(&mut m, d + o, r.float(0.05, 6.0));
            }
            putf(&mut m, d + 0x30, r.float(2.0, 40.0));
            put(&mut m, d + 0x7c, if r.chance(3) { (0xe00b << 16) | (BITMAP_TAG + r.below(2)) } else { NONE });
        }
        for b in 0..2 {
            let k = BITMAP_TAG + b;
            let entry = TAG_TABLE + 16 * k;
            let data = BITMAPS + 0x100 * b;
            put(&mut m, entry, u32::from_be_bytes(*b"bitm"));
            put(&mut m, entry + 4, (0xe00b << 16) | k);
            put(&mut m, entry + 8, data);
            put(&mut m, data + 0x48, data + 0x80);
            put16(&mut m, data + 0x80 + 4, 64 << b);
            put16(&mut m, data + 0x80 + 6, 32 << (1 - b));
        }
        // Placements.
        put(&mut m, SCENARIO_POINTER, SCENARIO);
        put(&mut m, SCENARIO + 0xe8, PLACEMENT_COUNT);
        put(&mut m, SCENARIO + 0xec, PLACEMENTS);
        for p in 0..PLACEMENT_COUNT {
            let row = PLACEMENTS + 0x6c * p;
            put16(&mut m, row + 0x3c, r.below(4) as u16);
            put8(&mut m, row + 0x3e, r.below(2) as u8);
            for o in [0x58, 0x5c, 0x60, 0x64] {
                putf(&mut m, row + o, r.float(0.05, 3.0));
            }
            putf(&mut m, row + 0x68, r.float(1.0, 30.0));
        }
        // Options.
        put(&mut m, LIGHT_OPTIONS, OPTIONS);
        put8(&mut m, OPTIONS + 8, u8::from(!r.chance(12)));
        put(&mut m, VISIBILITY_OPTIONS, VIS_OPTIONS);
        put(&mut m, VIS_OPTIONS + 0xc14, r.below(4));
        put8(&mut m, VIS_OPTIONS, r.u32() as u8);
        for bsp in 0..4 {
            put(&mut m, LIGHT_BSP_OVERRIDES + 4 * bsp, if r.chance(3) { 1 } else { 0 });
        }
        put16(&mut m, RENDER_MODE, if r.chance(2) { 2 } else { 1 });
        put(&mut m, OVERRIDE, 0);
        // The local player's camera.
        put(&mut m, LOCAL_PLAYER, if r.chance(6) { NONE } else { r.below(4) });
        for p in 0..4 {
            let camera = CAMERA_CONTROLLER + p * 320;
            put(&mut m, camera + 4, if r.chance(4) { 0x3f00_0000 } else { word(image, ZERO) });
            put(&mut m, camera + 0xc, if r.chance(4) { 0 } else { FIRST_PERSON_CAMERA });
        }
        put(&mut m, PLAYER_IDENTITIES, IDENTITIES);
        for p in 0..4 {
            put(&mut m, IDENTITIES + 0xc + 4 * p, r.below(4));
        }
        // The view.
        for (i, a) in PROJECTION_DEPTH.into_iter().enumerate() {
            putf(&mut m, a, if i == 3 { r.float(-2.0, 2.0) } else { r.float(-1.0, 1.0) });
        }
        putf(&mut m, PIXEL_SCALE, r.float(200.0, 900.0));
        put(&mut m, FRAME, 100 + r.below(1000));
        put(&mut m, VIEW_INDEX, [0, 1, 2, 3, NONE, 5][r.below(6) as usize]);

        // The lights.
        for i in 0..lights {
            let l = light_record(i);
            put16(&mut m, l, (light_datum(i) >> 16) as u16);
            put8(&mut m, l + 2, if r.chance(10) { 0 } else { 2 | (r.u32() as u8 & 0xfd) });
            put(&mut m, l + 4, definition_datum(r.below(DEFINITION_COUNT)));
            put(&mut m, l + 8, if r.chance(3) { r.below(PLACEMENT_COUNT) } else { NONE });
            put(&mut m, l + 0xc, r.below(4));
            for o in 0..3 {
                putf(&mut m, l + 0x18 + 4 * o, r.float(-50.0, 50.0));
            }
            putf(&mut m, l + 0x24, if r.chance(12) { 0.0 } else { r.float(0.5, 20.0) });
            put16(&mut m, l + 0x48, if r.chance(3) { 0xffff } else { r.below(8) as u16 });
            put(&mut m, l + 0x4c, if r.chance(3) { NONE } else { object_datum(r.below(OBJECT_SLOTS)) });
            put16(&mut m, l + 0x54, if r.chance(2) { 0xffff } else { r.below(4) as u16 });
            for o in [0x84, 0x90] {
                for c in 0..3 {
                    putf(&mut m, l + o + 4 * c, r.float(-20.0, 20.0));
                }
            }
            let forward = [r.float(-1.0, 1.0), r.float(-1.0, 1.0), r.float(0.1, 1.0)];
            let up = [r.float(-1.0, 1.0), r.float(-1.0, 1.0), r.float(0.1, 1.0)];
            for c in 0..3 {
                putf(&mut m, l + 0xac + 4 * c, forward[c as usize]);
                putf(&mut m, l + 0xb8 + 4 * c, up[c as usize]);
            }
            putf(&mut m, l + 0xc4, r.float(0.5, 2.0));
            for o in [0xd4, 0xe0] {
                for c in 0..3 {
                    putf(&mut m, l + o + 4 * c, if r.chance(5) { 0.0 } else { colour * r.float(0.5, 2.0) });
                }
            }
            putf(&mut m, l + 0xf8, if r.chance(3) { 0.0 } else { r.float(0.1, 1.0) });
            putf(&mut m, l + 0x100, if r.chance(10) { 0.0 } else { r.float(0.05, 1.0) });
        }
        for w in 0..lights.div_ceil(32) {
            put(&mut m, LIGHT_BITMAP + 4 * w, r.u32() | 0x1111_1111);
        }
        Self { mem: m, lights }
    }

    /// The main view's light list: `n` handles, some -1 or repeated.
    pub fn candidates(&mut self, r: &mut Rng, n: u32) {
        let mut handles = Vec::new();
        for _ in 0..n {
            handles.push(match r.below(20) {
                0 => NONE,
                1 if !handles.is_empty() => handles[r.below(handles.len() as u32) as usize],
                _ => light_datum(r.below(self.lights)),
            });
        }
        put(&mut self.mem, CANDIDATES_POINTER, CANDIDATE_LIST);
        let mut rr = Rng(r.next());
        seed_list(&mut self.mem, &mut rr, CANDIDATE_LIST, 0x200, &handles);
    }

    /// The view's history: `n` rows, some naming lights in the pool.
    pub fn history(&mut self, r: &mut Rng, n: u32) {
        let view = (word(&self.mem, VIEW_INDEX) as i32).clamp(0, 3) as u32;
        let frame = word(&self.mem, FRAME);
        let base = history::LIGHTS + view * history::STRIDE;
        for i in 0..32 {
            let row = base + i * 0x24;
            for b in 0..4 {
                put8(&mut self.mem, row + b, if r.chance(3) { 1 } else { 0 });
            }
            put(&mut self.mem, row + 4, if r.chance(8) { r.u32() } else { light_datum(r.below(self.lights)) });
            put(&mut self.mem, row + 8, if r.chance(2) { NONE } else { object_datum(r.below(OBJECT_SLOTS)) });
            putf(&mut self.mem, row + 0xc, r.float(0.0, 1.0));
            putf(&mut self.mem, row + 0x10, r.float(0.0, 50.0));
            putf(&mut self.mem, row + 0x14, r.float(-0.05, 0.05));
            putf(&mut self.mem, row + 0x18, r.float(0.0, 1.0));
            put(&mut self.mem, row + 0x1c, frame.wrapping_sub(r.below(3)));
            put(&mut self.mem, row + 0x20, frame.wrapping_sub(r.below(3)));
        }
        put(&mut self.mem, base + history::COUNT, n);
        if r.chance(5) {
            put(&mut self.mem, OVERRIDE, OVERRIDE_STRUCT);
            put8(&mut self.mem, OVERRIDE_STRUCT + 5, r.below(2) as u8);
        }
    }

    /// The light's visibility struct: its cluster, object and placement
    /// lists.
    pub fn light_vis(&mut self, r: &mut Rng) {
        let mut rr = Rng(r.next());
        let clusters: Vec<u32> = (0..r.below(5)).map(|_| r.below(64)).collect();
        let objects: Vec<u32> = (0..r.below(8)).map(|_| object_datum(r.below(OBJECT_SLOTS))).collect();
        let placements: Vec<u32> = (0..r.below(4)).map(|_| r.below(64)).collect();
        for (n, (offset, handles)) in [(0xc, &clusters), (0x14, &objects), (0x18, &placements)].into_iter().enumerate() {
            let header = list_header(n as u32);
            put(&mut self.mem, LIGHT_VIS + offset, header);
            let capacity = handles.len() as u32 + if rr.chance(4) { 1 } else { 1 + rr.below(6) };
            seed_list(&mut self.mem, &mut rr, header, capacity, handles);
        }
    }
}

// ---------------------------------------------------------------------
// L1: the leaves.

#[derive(Default)]
struct Tally(BTreeMap<String, u32>);

impl Tally {
    fn add(&mut self, k: impl Into<String>) {
        *self.0.entry(k.into()).or_default() += 1;
    }
    fn get(&self, k: &str) -> u32 {
        self.0.get(k).copied().unwrap_or(0)
    }
}

fn compare(label: &str, lifted: &FlatMemory, port: &FlatMemory, live: u32) {
    if let Some(i) = first_difference(lifted, port, live) {
        panic!("{label}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)", lifted.bytes[i], port.bytes[i]);
    }
}

#[test]
fn visible_list_matches_the_lifted_original() {
    if !with_image(visible_cases) {
        eprintln!("SKIPPED visible_list_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn visible_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..48u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n));
            let lights = [4, 40, 160][(n % 3) as usize];
            let mut w = World::new(image, &mut r, lights);
            let count = match n % 4 {
                0 => r.below(8),
                1 => 20 + r.below(30),
                2 => 130 + r.below(30),
                _ => r.below(64),
            };
            w.candidates(&mut r, count);
            let rows = [0, r.below(8), 32, r.below(33)][r.below(4) as usize];
            w.history(&mut r, rows);
            let label = format!("seed {seed} case {n} ({count} candidates, {rows} history rows)");
            let output = SCRATCH;

            let mut port = Ram(w.mem.clone());
            let list = match visible::list(&port, output, FPU) {
                Ok(list) => list,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            list.writes.apply(&mut port).unwrap();

            let mut a = w.mem;
            let mut c = cpu(&mut r);
            let esp = c.gpr[4];
            lifted.call(&mut c, &mut a, VISIBLE_LIST, esp, &[output]).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[0], list.count, "{label}: the count");
            assert_eq!(c.gpr[4], esp, "{label}: ret 4");
            compare(&label, &a, &port.0, esp);
            tally.add("native");
            if count > 128 {
                tally.add("over 128 candidates");
            }
            if list.count == 32 {
                tally.add("a full history");
            }
        }
    }
    eprintln!("visible list: {:#?}", tally.0);
    assert!(tally.get("native") >= 48, "too few cases answered: {:#?}", tally.0);
    assert!(tally.get("over 128 candidates") > 0, "never over 128 candidates");
}

#[test]
fn geometry_flags_match_the_lifted_original() {
    if !with_image(geometry_cases) {
        eprintln!("SKIPPED geometry_flags_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn geometry_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..128u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n));
            let mut w = World::new(image, &mut r, 8);
            w.light_vis(&mut r);
            let i = r.below(w.lights);
            let l = light_record(i);
            let def_datum = word(&w.mem, l + 4);
            let owner = word(&w.mem, l + 0x4c);
            let root = if owner == NONE || r.chance(4) { NONE } else { owner };
            let args = geometry::Arguments {
                light: light_datum(i),
                definition: def_datum,
                vis: LIGHT_VIS,
                owner,
                root,
                point: (r.u32() & !0xff) | r.below(2),
                shadow: if r.chance(4) { 0 } else { SCRATCH },
            };
            let label = format!("seed {seed} case {n}");

            let mut port = Ram(w.mem.clone());
            let writes = match geometry::flags(&port, args) {
                Ok(w) => w,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            writes.apply(&mut port).unwrap();

            let mut a = w.mem;
            let mut c = cpu(&mut r);
            c.gpr[0] = args.light;
            c.gpr[1] = args.definition;
            c.gpr[6] = args.vis;
            let esp = c.gpr[4];
            lifted
                .call(&mut c, &mut a, GEOMETRY_FLAGS, esp, &[args.owner, args.root, args.point, args.shadow])
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: ret 0x10");
            compare(&label, &a, &port.0, esp);
            tally.add("native");
            let flags = word(&a, definition(def_datum & 0xffff));
            for (bit, name) in [(4, "force clusters"), (0x1_0000, "clusters"), (0x2_0000, "objects"), (0x3800, "owner")] {
                if flags & bit != 0 {
                    tally.add(name);
                }
            }
        }
    }
    eprintln!("geometry flags: {:#?}", tally.0);
    assert!(tally.get("native") >= 128, "too few cases answered: {:#?}", tally.0);
    for k in ["force clusters", "clusters", "objects", "owner"] {
        assert!(tally.get(k) > 0, "{k} never seen");
    }
}

#[test]
fn extra_caster_matches_the_lifted_original() {
    if !with_image(extra_caster_cases) {
        eprintln!("SKIPPED extra_caster_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn extra_caster_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..128u32 {
            let mut r = Rng(seed.wrapping_mul(0x5851_f42d_4c95_7f2d) ^ u64::from(n));
            let mut w = World::new(image, &mut r, 4);
            w.light_vis(&mut r);
            let owner = if r.chance(6) { NONE } else { object_datum(r.below(OBJECT_SLOTS)) };
            let shadow = if r.chance(2) { 0 } else { r.u32() as u8 | 1 };
            let label = format!("seed {seed} case {n}");

            let mut port = Ram(w.mem.clone());
            let writes = match extra_caster::list(&port, shadow, owner, LIGHT_VIS) {
                Ok(w) => w,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            writes.apply(&mut port).unwrap();

            let mut a = w.mem;
            let mut c = cpu(&mut r);
            c.gpr[0] = (r.u32() & !0xff) | u32::from(shadow);
            c.gpr[1] = owner;
            let esp = c.gpr[4];
            lifted.call(&mut c, &mut a, EXTRA_CASTER, esp, &[LIGHT_VIS]).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: ret 4");
            compare(&label, &a, &port.0, esp);
            tally.add(if writes.is_empty() { "nothing appended" } else { "appended" });
        }
    }
    eprintln!("extra caster: {:#?}", tally.0);
    for k in ["nothing appended", "appended"] {
        assert!(tally.get(k) > 0, "{k} never seen");
    }
}

#[test]
fn light_volume_matches_the_lifted_original() {
    if !with_image(volume_cases) {
        eprintln!("SKIPPED light_volume_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn volume_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..192u32 {
            let mut r = Rng(seed.wrapping_mul(0xd1b5_4a32_d192_ed03) ^ u64::from(n));
            let w = World::new(image, &mut r, 8);
            let i = r.below(w.lights);
            let out = SCRATCH;
            let bsp_check = (r.u32() & !0xff) | r.below(2);
            let label = format!("seed {seed} case {n}");

            let mut port = Ram(w.mem.clone());
            let call = match volume::light_volume(&port, out, light_datum(i), bsp_check) {
                Ok(call) => call,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            call.writes.apply(&mut port).unwrap();

            let mut a = w.mem;
            let mut c = cpu(&mut r);
            c.gpr[0] = out;
            c.gpr[2] = light_datum(i);
            let esp = c.gpr[4];
            lifted.call(&mut c, &mut a, LIGHT_VOLUME, esp, &[bsp_check]).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: ret 4");
            assert_eq!(c.gpr[0] as u8 != 0, call.al, "{label}: AL");
            compare(&label, &a, &port.0, esp);
            tally.add(match call.posed {
                None => "gated".to_string(),
                Some(p) => format!("kind {}", p.kind()),
            });
        }
    }
    eprintln!("light volume: {:#?}", tally.0);
    for k in ["gated", "kind 0", "kind 1", "kind 2", "kind 3"] {
        assert!(tally.get(k) > 0, "{k} never seen: {:#?}", tally.0);
    }
}

/// The ports' writes are what the seams publish: one list, in order.
#[test]
fn a_refused_leaf_names_its_reason() {
    let m = Ram(FlatMemory::new(0, 0x1000));
    assert_eq!(visible::list(&m, 0, Fpu { mxcsr: 0x1f80, x87_control: 0x037f }).err(), Some(gather::Refusal::Unsupported));
    assert_eq!(visible::list(&m, 0, FPU).err(), Some(gather::Refusal::Memory));
}
