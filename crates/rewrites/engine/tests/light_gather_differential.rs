//! `engine::render::light_gather` against the original, lifted and run on
//! the IR interpreter from the same seeded 64 MB state
//! (docs/graphics/native-light-gather.md, slices L1 to L8).
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
//! **A light's records (L3).** See
//! [`light_record_walks_match_the_lifted_original`].
//!
//! **The item lists and the first-person table (L4).** See
//! [`item_lists_match_the_lifted_original`].
//!
//! **One light's passes (L5).** See
//! [`passes_transcripts_match_the_lifted_original`].
//!
//! **The first-person lights (L6).** See
//! [`first_person_lights_match_the_lifted_original`] and
//! [`first_person_records_match_the_lifted_original`].
//!
//! **Object shadows (L7).** See [`object_shadow_list_matches_the_lifted_original`]
//! [`object_shadow_rows_match_the_lifted_original`] and
//! [`object_shadow_caster_matches_the_lifted_original`].
//!
//! **Typed passes (L8).** See [`screen_bounds_match_the_lifted_original`] and
//! [`typed_light_tail_matches_the_lifted_layer_wrappers`].
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
use std::sync::{Arc, Mutex};

use common::{IMAGE_SHA256, sha256, xbe_path};
use engine::light_constants::Fpu;
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render::light_gather::frame::{self, LightFrame};
use engine::render::light_gather::{
    self as gather, LIGHT_VIS, extra_caster, geometry, history, visible, volume,
};
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
const TAGS_POINTER: u32 = guest_layout::tags::TAG_ENTRIES_GLOBAL;
const SCENARIO_POINTER: u32 = 0x004e_0350;
const CANDIDATES_POINTER: u32 = 0x0054_7f98;
const FRAME: u32 = 0x004b_a034;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const LOCAL_PLAYER: u32 = 0x004b_9ed8;
const CAMERA_CONTROLLER: u32 = 0x004e_8c44;
const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
const PLAYER_IDENTITIES: u32 = 0x004e_8c20;
const LIGHT_OPTIONS: u32 = 0x0051_07e8;
/// The multiplayer engine globals (`points::GAME_ENGINE`); `+0xc14` the
/// engine index, `+0` a flags byte.
const GAME_ENGINE: u32 = 0x004e_9ae8;
/// The engine table (`points::ENGINE_TABLE`): one engine object per index.
const ENGINE_TABLE: u32 = 0x0055_e4d0;
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
const ENGINE_GLOBALS: u32 = 0x0308_1000;
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
            let insn = self
                .cache
                .get(&pc)
                .ok_or_else(|| format!("not lifted: {pc:#x}"))?;
            pc = interp::step(cpu, mem, insn)
                .map_err(|e| format!("{pc:#x}: {e:?}"))?
                .target();
        }
        cpu.eip = pc;
        Ok(pc)
    }

    /// Calls `entry` with `cpu`'s registers and `args` on the stack below
    /// `esp`, to its return.
    pub fn call(
        &mut self,
        cpu: &mut Cpu,
        mem: &mut FlatMemory,
        entry: u32,
        esp: u32,
        args: &[u32],
    ) -> Result<(), String> {
        let mut sp = esp;
        for &a in args.iter().rev() {
            sp -= 4;
            put(mem, sp, a);
        }
        sp -= 4;
        put(mem, sp, HARNESS_EXIT);
        cpu.gpr[4] = sp;
        cpu.eip = entry;
        self.run(cpu, mem, &[HARNESS_EXIT], |_, _, _| false)
            .map(|_| ())
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
    f(
        &image,
        Lifted {
            lifter: &lifter,
            cache: BTreeMap::new(),
        },
    );
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
        gpr: [
            r.u32(),
            r.u32(),
            r.u32(),
            r.u32(),
            STACK_TOP - 0x2000,
            r.u32(),
            r.u32(),
            r.u32(),
        ],
        fs_base: RAM as u32,
        ..Cpu::default()
    };
    cpu.mxcsr = 0x1f80;
    cpu.x87.control = 0x027f;
    cpu
}

pub const FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};

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
    let columns = [
        header + 0x100,
        header + 0x1100,
        header + 0x2100,
        header + 0x3100,
    ];
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
const DEFINITION_BITS: [u32; 15] = [
    0x4, 0x8, 0x20, 0x40, 0x100, 0x400, 0x800, 0x1000, 0x2000, 0x1_0000, 0x2_0000, 0x4_0000,
    0x10_0000, 0x40_0000, 0x1,
];

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
        put(
            &mut m,
            LIGHT_HEADER + 0x20,
            engine::render::light::MAXIMUM_COUNT as u32,
        );
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
            put8(
                &mut m,
                slot + 3,
                if r.chance(3) { 2 } else { r.below(4) as u8 },
            );
            put(&mut m, slot + 8, body(j));
            // Parents point to lower slots, so every chain ends.
            let parent = if j == 0 || r.chance(2) {
                NONE
            } else {
                object_datum(r.below(j))
            };
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
            put16(
                &mut m,
                d + 4,
                if r.chance(16) {
                    4 + r.below(3) as u16
                } else {
                    r.below(4) as u16
                },
            );
            let near = r.float(0.5, 4.0);
            putf(&mut m, d + 0x18, near);
            putf(
                &mut m,
                d + 0x1c,
                if r.chance(4) {
                    0.0
                } else {
                    near * r.float(1.0, 8.0)
                },
            );
            for o in [0x20, 0x24, 0x28, 0x2c, 0x30] {
                putf(&mut m, d + o, r.float(0.05, 6.0));
            }
            putf(&mut m, d + 0x30, r.float(2.0, 40.0));
            put(
                &mut m,
                d + 0x7c,
                if r.chance(3) {
                    (0xe00b << 16) | (BITMAP_TAG + r.below(2))
                } else {
                    NONE
                },
            );
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
        put(&mut m, GAME_ENGINE, ENGINE_GLOBALS);
        put(&mut m, ENGINE_GLOBALS + 0xc14, r.below(4));
        put8(&mut m, ENGINE_GLOBALS, r.u32() as u8);
        for index in 0..4 {
            put(
                &mut m,
                ENGINE_TABLE + 4 * index,
                if r.chance(3) { 1 } else { 0 },
            );
        }
        put16(&mut m, RENDER_MODE, if r.chance(2) { 2 } else { 1 });
        put(&mut m, OVERRIDE, 0);
        // The local player's camera.
        put(
            &mut m,
            LOCAL_PLAYER,
            if r.chance(6) { NONE } else { r.below(4) },
        );
        for p in 0..4 {
            let camera = CAMERA_CONTROLLER + p * 320;
            put(
                &mut m,
                camera + 4,
                if r.chance(4) {
                    0x3f00_0000
                } else {
                    word(image, ZERO)
                },
            );
            put(
                &mut m,
                camera + 0xc,
                if r.chance(4) { 0 } else { FIRST_PERSON_CAMERA },
            );
        }
        put(&mut m, PLAYER_IDENTITIES, IDENTITIES);
        for p in 0..4 {
            put(&mut m, IDENTITIES + 0xc + 4 * p, r.below(4));
        }
        // The view.
        for (i, a) in PROJECTION_DEPTH.into_iter().enumerate() {
            putf(
                &mut m,
                a,
                if i == 3 {
                    r.float(-2.0, 2.0)
                } else {
                    r.float(-1.0, 1.0)
                },
            );
        }
        putf(&mut m, PIXEL_SCALE, r.float(200.0, 900.0));
        put(&mut m, FRAME, 100 + r.below(1000));
        put(
            &mut m,
            VIEW_INDEX,
            [0, 1, 2, 3, NONE, 5][r.below(6) as usize],
        );

        // The lights.
        for i in 0..lights {
            let l = light_record(i);
            put16(&mut m, l, (light_datum(i) >> 16) as u16);
            put8(
                &mut m,
                l + 2,
                if r.chance(10) {
                    0
                } else {
                    2 | (r.u32() as u8 & 0xfd)
                },
            );
            put(&mut m, l + 4, definition_datum(r.below(DEFINITION_COUNT)));
            put(
                &mut m,
                l + 8,
                if r.chance(3) {
                    r.below(PLACEMENT_COUNT)
                } else {
                    NONE
                },
            );
            put(&mut m, l + 0xc, r.below(4));
            for o in 0..3 {
                putf(&mut m, l + 0x18 + 4 * o, r.float(-50.0, 50.0));
            }
            putf(
                &mut m,
                l + 0x24,
                if r.chance(12) {
                    0.0
                } else {
                    r.float(0.5, 20.0)
                },
            );
            put16(
                &mut m,
                l + 0x48,
                if r.chance(3) {
                    0xffff
                } else {
                    r.below(8) as u16
                },
            );
            put(
                &mut m,
                l + 0x4c,
                if r.chance(3) {
                    NONE
                } else {
                    object_datum(r.below(OBJECT_SLOTS))
                },
            );
            put16(
                &mut m,
                l + 0x54,
                if r.chance(2) {
                    0xffff
                } else {
                    r.below(4) as u16
                },
            );
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
                    putf(
                        &mut m,
                        l + o + 4 * c,
                        if r.chance(5) {
                            0.0
                        } else {
                            colour * r.float(0.5, 2.0)
                        },
                    );
                }
            }
            putf(
                &mut m,
                l + 0xf8,
                if r.chance(3) { 0.0 } else { r.float(0.1, 1.0) },
            );
            putf(
                &mut m,
                l + 0x100,
                if r.chance(10) {
                    0.0
                } else {
                    r.float(0.05, 1.0)
                },
            );
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
            put(
                &mut self.mem,
                row + 4,
                if r.chance(8) {
                    r.u32()
                } else {
                    light_datum(r.below(self.lights))
                },
            );
            put(
                &mut self.mem,
                row + 8,
                if r.chance(2) {
                    NONE
                } else {
                    object_datum(r.below(OBJECT_SLOTS))
                },
            );
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
        let objects: Vec<u32> = (0..r.below(8))
            .map(|_| object_datum(r.below(OBJECT_SLOTS)))
            .collect();
        let placements: Vec<u32> = (0..r.below(4)).map(|_| r.below(64)).collect();
        for (n, (offset, handles)) in [(0xc, &clusters), (0x14, &objects), (0x18, &placements)]
            .into_iter()
            .enumerate()
        {
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
        panic!(
            "{label}: memory differs first at {i:#x}: {:#04x} (original) vs {:#04x} (port)",
            lifted.bytes[i], port.bytes[i]
        );
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
            lifted
                .call(&mut c, &mut a, VISIBLE_LIST, esp, &[output])
                .unwrap_or_else(|e| panic!("{label}: {e}"));
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
    assert!(
        tally.get("native") >= 48,
        "too few cases answered: {:#?}",
        tally.0
    );
    assert!(
        tally.get("over 128 candidates") > 0,
        "never over 128 candidates"
    );
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
            let root = if owner == NONE || r.chance(4) {
                NONE
            } else {
                owner
            };
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
                .call(
                    &mut c,
                    &mut a,
                    GEOMETRY_FLAGS,
                    esp,
                    &[args.owner, args.root, args.point, args.shadow],
                )
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: ret 0x10");
            compare(&label, &a, &port.0, esp);
            tally.add("native");
            let flags = word(&a, definition(def_datum & 0xffff));
            for (bit, name) in [
                (4, "force clusters"),
                (0x1_0000, "clusters"),
                (0x2_0000, "objects"),
                (0x3800, "owner"),
            ] {
                if flags & bit != 0 {
                    tally.add(name);
                }
            }
        }
    }
    eprintln!("geometry flags: {:#?}", tally.0);
    assert!(
        tally.get("native") >= 128,
        "too few cases answered: {:#?}",
        tally.0
    );
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
            let owner = if r.chance(6) {
                NONE
            } else {
                object_datum(r.below(OBJECT_SLOTS))
            };
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
            lifted
                .call(&mut c, &mut a, EXTRA_CASTER, esp, &[LIGHT_VIS])
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: ret 4");
            compare(&label, &a, &port.0, esp);
            tally.add(if writes.is_empty() {
                "nothing appended"
            } else {
                "appended"
            });
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
            lifted
                .call(&mut c, &mut a, LIGHT_VOLUME, esp, &[bsp_check])
                .unwrap_or_else(|e| panic!("{label}: {e}"));
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
    assert_eq!(
        visible::list(
            &m,
            0,
            Fpu {
                mxcsr: 0x1f80,
                x87_control: 0x037f
            }
        )
        .err(),
        Some(gather::Refusal::Unsupported)
    );
    assert_eq!(
        visible::list(&m, 0, FPU).err(),
        Some(gather::Refusal::Memory)
    );
}

// ---------------------------------------------------------------------
// L2: the frame.

const DYNAMIC_LIGHTS: u32 = 0x0003_1c80;
const STUB_RETURN: u32 = 0xFFFF_FFF0;
const FIRST_PERSON_COUNT: u32 = frame::FIRST_PERSON_COUNT;
const NO_SOURCE_VIS: u32 = 0x004b_a010;
const SHADOWS_ON: u32 = 0x004b_a020;
const VIEW_CLUSTER: u32 = 0x004b_9ee4;
const RECORDS_VIS: u32 = 0x004c_0b80;
const RECORDS: u32 = 0x030b_0000;
const STUB_SCRATCH: u32 = 0x030c_0000;

/// The callees the frame calls that are not ported: target, the
/// registers it is handed (`Cpu::gpr` indices: EAX 0, ECX 1, EDX 2,
/// EBX 3, ESI 6), its stack arguments and the bytes it pops.
const STUBS: [(u32, &[usize], u32, u32); 11] = [
    (frame::call::REPOSE_MARKER, &[0, 1], 3, 0),
    (frame::call::FIRST_PERSON_MARKER, &[1], 4, 16),
    (frame::call::REPOSE_BASIS, &[0, 1], 0, 0),
    (frame::call::LIGHT_FRUSTA, &[0, 1, 2, 6], 4, 16),
    (frame::call::SCENE_VISIBILITY, &[0, 1, 2, 6], 7, 28),
    (frame::call::RESET_RECORDS, &[3], 0, 0),
    (frame::call::CLUSTER_RECORDS, &[3], 0, 0),
    (frame::call::PLACEMENT_RECORDS, &[3], 1, 4),
    (frame::call::OBJECT_RECORDS, &[0, 3], 1, 4),
    (frame::call::PART_VISIBILITY, &[3], 0, 0),
    (frame::call::LIGHT_PASSES, &[0, 1, 3], 4, 16),
];

/// The L1 leaves: lifted in both runs, so not in the transcript.
const LEAVES: [u32; 3] = [GEOMETRY_FLAGS, EXTRA_CASTER, LIGHT_VOLUME];

fn stub(target: u32) -> Option<(&'static [usize], u32, u32)> {
    STUBS
        .iter()
        .find(|s| s.0 == target)
        .map(|&(_, regs, args, pop)| (regs, args, pop))
}

/// One call out of the frame: the target, its return site, the
/// registers it is handed and its stack arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Callout {
    target: u32,
    site: u32,
    regs: Vec<u32>,
    args: Vec<u32>,
}

/// The stub's effect, seeded by the call's place in the transcript: the
/// memory a real callee would change that the frame reads after it, and
/// EAX and EDX.
fn stub_effect(seed: u64, k: usize, c: &Callout, m: &mut FlatMemory) -> (u32, u32) {
    let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(c.target));
    let reg = |i: usize| {
        let (regs, _, _) = stub(c.target).unwrap();
        c.regs[regs.iter().position(|&x| x == i).unwrap()]
    };
    let floats = |m: &mut FlatMemory, r: &mut Rng, at: u32| {
        for i in 0..3 {
            putf(m, at + 4 * i, r.float(-1.0, 1.0));
        }
    };
    match c.target {
        frame::call::REPOSE_MARKER => return (r.u32() & 0xffff, r.u32()),
        frame::call::FIRST_PERSON_MARKER => {
            for &at in &c.args[1..] {
                floats(m, &mut r, at);
            }
        }
        frame::call::REPOSE_BASIS => {
            floats(m, &mut r, reg(0));
            floats(m, &mut r, reg(1));
        }
        frame::call::LIGHT_FRUSTA => {
            put16(m, c.args[3], r.below(7) as u16);
            for i in 0..8 {
                put(m, c.args[2] + 4 * i, r.u32());
            }
        }
        frame::call::SCENE_VISIBILITY => return (LIGHT_VIS, r.u32()),
        frame::call::LIGHT_PASSES => {
            // A later row's weight: the frame reads each row when it
            // reaches it.
            let rows = reg(1) - 0x34 + frame::slot::ROWS;
            let j = r.below(32);
            let weight = match r.below(4) {
                0 => f32::NAN,
                1 => 0.0,
                _ => r.float(-0.5, 1.0),
            };
            putf(m, rows + 0x14 * j + 0xc, weight);
        }
        _ => put(m, STUB_SCRATCH + 4 * k as u32, r.u32()),
    }
    (r.u32(), r.u32())
}

/// The frame's private spills, which the port does not keep: the saved
/// registers, the row cursor, S+0x1c..S+0x34, and the stack probe's own
/// two words at the top (its return address and saved ECX; the body
/// makes no probe call).
fn private(s: u32) -> [std::ops::Range<u32>; 4] {
    [
        s..s + 0x10,
        s + 0x14..s + 0x18,
        s + 0x1c..s + 0x34,
        s + frame::FRAME - 8..s + frame::FRAME,
    ]
}

/// The first byte where `a` and `b` differ, outside the dead stack below
/// `s` and the private spills.
fn frame_difference(a: &FlatMemory, b: &FlatMemory, s: u32) -> Option<u32> {
    let mut masked = vec![STACK_LOW..s];
    masked.extend(private(s));
    masked.sort_by_key(|r| r.start);
    let mut from = 0u32;
    let check = |lo: u32, hi: u32| {
        let (x, y) = (
            &a.bytes[lo as usize..hi as usize],
            &b.bytes[lo as usize..hi as usize],
        );
        (x != y).then(|| lo + x.iter().zip(y).position(|(p, q)| p != q).unwrap() as u32)
    };
    for range in masked {
        if let Some(at) = check(from, range.start) {
            return Some(at);
        }
        from = range.end;
    }
    check(from, a.bytes.len() as u32)
}

/// A frozen history of `weights.len()` rows over the pool's lights, all
/// seen this frame, and no candidates: the visible list is these rows.
fn frozen_rows(w: &mut World, r: &mut Rng, weights: &[f32]) {
    w.candidates(r, 0);
    let view = (word(&w.mem, VIEW_INDEX) as i32).clamp(0, 3) as u32;
    let frame = word(&w.mem, FRAME);
    let base = history::LIGHTS + view * history::STRIDE;
    for (i, &weight) in weights.iter().enumerate() {
        let row = base + i as u32 * 0x24;
        for b in 0..4 {
            put8(&mut w.mem, row + b, r.below(2) as u8);
        }
        put(&mut w.mem, row + 4, light_datum(r.below(w.lights)));
        put(&mut w.mem, row + 8, NONE);
        putf(&mut w.mem, row + 0xc, r.float(0.0, 1.0));
        putf(&mut w.mem, row + 0x10, r.float(0.0, 50.0));
        putf(&mut w.mem, row + 0x14, 0.0);
        putf(&mut w.mem, row + 0x18, weight);
        put(&mut w.mem, row + 0x1c, frame);
        put(&mut w.mem, row + 0x20, frame);
    }
    put(&mut w.mem, base + history::COUNT, weights.len() as u32);
    put(&mut w.mem, OVERRIDE, OVERRIDE_STRUCT);
    put8(&mut w.mem, OVERRIDE_STRUCT + 5, 1);
}

/// The gather's globals.
fn frame_globals(w: &mut World, r: &mut Rng) {
    put(&mut w.mem, FIRST_PERSON_COUNT, r.below(11));
    put8(&mut w.mem, NO_SOURCE_VIS, r.below(2) as u8);
    put8(&mut w.mem, SHADOWS_ON, r.below(2) as u8);
    put(&mut w.mem, VIEW_CLUSTER, r.below(64));
    put(&mut w.mem, RECORDS_VIS, RECORDS);
    w.light_vis(r);
}

/// `0x00031c80` lifted, its unported callees stubbed. Returns the
/// transcript and EAX.
fn original_frame(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    cpu: &mut Cpu,
    esp: u32,
    seed: u64,
) -> Result<(Vec<Callout>, u32), String> {
    put(m, esp, HARNESS_EXIT);
    cpu.gpr[4] = esp;
    cpu.eip = DYNAMIC_LIGHTS;
    let mut transcript = Vec::new();
    lifted.run(cpu, m, &[HARNESS_EXIT], |pc, cpu, m| {
        let Some((regs, args, pop)) = stub(pc) else {
            return false;
        };
        let sp = cpu.gpr[4];
        let c = Callout {
            target: pc,
            site: word(m, sp),
            regs: regs.iter().map(|&i| cpu.gpr[i]).collect(),
            args: (0..args).map(|i| word(m, sp + 4 + 4 * i)).collect(),
        };
        let (eax, edx) = stub_effect(seed, transcript.len(), &c, m);
        cpu.gpr[0] = eax;
        cpu.gpr[2] = edx;
        cpu.gpr[4] = sp + 4 + pop;
        cpu.eip = c.site;
        transcript.push(c);
        true
    })?;
    if cpu.gpr[4] != esp + 4 {
        return Err(format!("esp {:#x} after the frame", cpu.gpr[4]));
    }
    Ok((transcript, cpu.gpr[0]))
}

/// The body, driven as the runtime drives it: each guest call is made at
/// [`GuestCall::esp`]; a leaf runs lifted, any other callee is the stub.
fn port_frame(
    lifted: &mut Lifted<'_>,
    m: FlatMemory,
    mut cpu: Cpu,
    esp: u32,
    plan: frame::Plan,
    seed: u64,
) -> (
    Vec<Callout>,
    guest_layout::entry::EntryResult,
    Option<LightFrame>,
    FlatMemory,
) {
    use guest_layout::entry::{Cx, EntryFuture, GuestCall, Regs, Step, poll, resume};
    let mem = Arc::new(Mutex::new(Ram(m)));
    let cx = Cx::new(
        Arc::clone(&mem),
        Regs {
            esp,
            ..Regs::default()
        },
    );
    let out = Arc::new(Mutex::new(None));
    let body = cx.clone();
    let kept = Arc::clone(&out);
    let mut fut: EntryFuture = Box::pin(async move {
        let (ret, light_frame) = frame::run(&body, plan).await?;
        *kept.lock().unwrap() = Some(light_frame);
        Ok::<_, guest_layout::entry::EntryError>(ret)
    });
    let mut transcript = Vec::new();
    let result = loop {
        match poll(&mut fut, &cx) {
            Step::Done(r) => break r,
            Step::Call(c) => {
                let mut g = mem.lock().unwrap();
                let m = &mut g.0;
                let sp = c.esp(esp);
                let set = |c: &GuestCall, i: usize| match i {
                    0 => c.eax,
                    1 => c.ecx,
                    2 => c.edx,
                    3 => c.ebx,
                    6 => c.esi,
                    7 => c.edi,
                    _ => None,
                };
                let regs = if LEAVES.contains(&c.target) {
                    for (i, &a) in c.args.iter().enumerate() {
                        put(m, sp + 4 + 4 * i as u32, a);
                    }
                    put(m, sp, HARNESS_EXIT);
                    for i in [0, 1, 2, 3, 6, 7] {
                        if let Some(v) = set(&c, i) {
                            cpu.gpr[i] = v;
                        }
                    }
                    cpu.gpr[4] = sp;
                    cpu.eip = c.target;
                    lifted
                        .run(&mut cpu, m, &[HARNESS_EXIT], |_, _, _| false)
                        .unwrap_or_else(|e| panic!("leaf {:#x}: {e}", c.target));
                    cpu.gpr
                } else {
                    let (declared, _, _) = stub(c.target)
                        .unwrap_or_else(|| panic!("an unknown callee {:#x}", c.target));
                    for (i, &a) in c.args.iter().enumerate() {
                        put(m, sp + 4 + 4 * i as u32, a);
                    }
                    put(m, sp, STUB_RETURN);
                    let callout = Callout {
                        target: c.target,
                        site: frame::call_site().unwrap_or(0),
                        regs: declared
                            .iter()
                            .map(|&i| set(&c, i).unwrap_or(0xdead_beef))
                            .collect(),
                        args: c.args.clone(),
                    };
                    let (eax, edx) = stub_effect(seed, transcript.len(), &callout, m);
                    transcript.push(callout);
                    let mut gpr = cpu.gpr;
                    gpr[0] = eax;
                    gpr[2] = edx;
                    gpr
                };
                drop(g);
                resume(
                    &cx,
                    Regs {
                        eax: regs[0],
                        ecx: regs[1],
                        edx: regs[2],
                        ebx: regs[3],
                        esp,
                        ebp: regs[5],
                        esi: regs[6],
                        edi: regs[7],
                        xmm0: 0,
                    },
                );
            }
        }
    };
    drop(fut);
    drop(cx);
    let light_frame = out.lock().unwrap().take();
    let m = Arc::try_unwrap(mem)
        .ok()
        .expect("the body kept no memory")
        .into_inner()
        .unwrap()
        .0;
    (transcript, result, light_frame, m)
}

/// Equal transcripts (target, return site, the declared registers, the
/// stack arguments), equal EAX, and equal memory outside the dead stack
/// and the frame's private spills, for seeded frames: 0, 1, 8, 9 and 32
/// drawn lights, NaN and threshold weights, each branch of steps 5, 8
/// and 9, and frames ranked from candidates.
#[test]
fn frame_transcripts_match_the_lifted_original() {
    if !with_image(frame_cases) {
        eprintln!("SKIPPED frame_transcripts_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn frame_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let threshold = wordf(image, DRAWN_THRESHOLD);
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..60u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n) ^ 0x1c80);
            let mut w = World::new(image, &mut r, [8, 40, 160][(n % 3) as usize]);
            frame_globals(&mut w, &mut r);
            let drawn = [0, 1, 8, 9, 32, r.below(33)][(n % 6) as usize];
            if n % 7 == 6 {
                // Ranked from candidates, as a frame without the override.
                let count = 10 + r.below(50);
                w.candidates(&mut r, count);
                let rows = r.below(33);
                w.history(&mut r, rows);
            } else {
                let rows = (drawn + r.below(4)).min(32);
                let mut weights: Vec<f32> = (0..rows)
                    .map(|i| {
                        if i < drawn {
                            threshold + r.float(0.05, 1.0)
                        } else {
                            [threshold, f32::NAN, threshold - 0.5][r.below(3) as usize]
                        }
                    })
                    .collect();
                // Drawn and undrawn rows interleave.
                for i in (1..weights.len()).rev() {
                    weights.swap(i, r.below(i as u32 + 1) as usize);
                }
                frozen_rows(&mut w, &mut r, &weights);
            }
            let label = format!("seed {seed} case {n}");
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 4;
            let s = esp - frame::FRAME;
            let case_seed = r.next();
            // The return address the original is entered with, in both
            // runs: the port never writes it.
            put(&mut w.mem, esp, HARNESS_EXIT);

            let plan = match frame::plan(&Ram(w.mem.clone()), esp, FPU) {
                Ok(plan) => plan,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            // The rows the list hands the frame drawn. Each passes stub
            // rewrites a later row's weight, so fewer can be drawn by the
            // time the frame reaches them.
            let listed = plan
                .list
                .ranked()
                .iter()
                .filter(|row| f32::from_bits(row.weight) > threshold)
                .count();
            let port_cpu = c.clone();
            let (port_calls, result, light_frame, port) =
                port_frame(&mut lifted, w.mem.clone(), port_cpu, esp, plan, case_seed);

            let mut a = w.mem;
            let (calls, eax) = original_frame(&mut lifted, &mut a, &mut c, esp, case_seed)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            for (i, (x, y)) in calls.iter().zip(&port_calls).enumerate() {
                assert_eq!(x, y, "{label}: callout {i} (original, then port)");
            }
            assert_eq!(
                calls.len(),
                port_calls.len(),
                "{label}: the number of callouts"
            );
            assert_eq!(
                result.map(|ret| (ret.eax, ret.pop)),
                Ok((eax, 0)),
                "{label}: the return"
            );
            if let Some(at) = frame_difference(&a, &port, s) {
                panic!(
                    "{label}: memory differs first at {at:#x} (S{:+#x}): {:#04x} (original) vs {:#04x} (port)",
                    at.wrapping_sub(s) as i32,
                    a.bytes[at as usize],
                    port.bytes[at as usize]
                );
            }
            tally.add("native");
            let light_frame = light_frame.expect("the frame");
            tally.add(format!("{listed} drawn"));
            for g in light_frame
                .lights
                .iter()
                .filter_map(|l| l.gathered.as_ref())
            {
                tally.add(match g.repose {
                    frame::Repose::None => "no re-pose",
                    frame::Repose::Asked { .. } => "a re-pose",
                });
                tally.add(if g.shadow { "shadowed" } else { "unshadowed" });
                tally.add(if g.source_vis {
                    "the main view's clusters"
                } else {
                    "no source visibility"
                });
                if g.cluster == word(&a, VIEW_CLUSTER) as i32
                    && word(&a, g.record + 0x48) as i16 == -1
                {
                    tally.add("the view's cluster");
                }
                if g.slot.is_none() {
                    tally.add("no first-person slot");
                }
                for (step, by) in [
                    ("volume", Some(g.volume)),
                    ("geometry", Some(g.geometry)),
                    ("extra caster", g.extra_caster),
                    ("clusters", Some(g.clusters)),
                    ("placements", Some(g.placements)),
                ] {
                    if let Some(by) = by {
                        tally.add(format!("{step} {by:?}"));
                    }
                }
            }
        }
    }
    eprintln!("frame: {:#?}", tally.0);
    assert!(
        tally.get("native") >= 120,
        "too few frames answered: {:#?}",
        tally.0
    );
    for need in [
        "0 drawn",
        "1 drawn",
        "8 drawn",
        "9 drawn",
        "32 drawn",
        "a re-pose",
        "no re-pose",
        "shadowed",
        "unshadowed",
        "the main view's clusters",
        "no source visibility",
        "the view's cluster",
        "no first-person slot",
    ] {
        assert!(tally.get(need) > 0, "never {need}: {:#?}", tally.0);
    }
}

// ---------------------------------------------------------------------
// L3: a light's records.

const CLUSTER_WALK: u32 = 0x0013_3390;
const PLACEMENT_WALK: u32 = 0x0013_3520;
const RECORD_COUNT: u32 = 0x004c_0b78;
const MAIN_RECORD_COUNT: u32 = 0x004c_0b7c;
const SCENE_RECORDS: u32 = 0x004b_a138;
const PIXEL_BYTES: u32 = 0x004c_0b84;
const ATTACHMENT_BYTES: u32 = 0x004c_0ed6;
const ACTIVE_BSP: u32 = 0x004e_0348;
const LIGHTMAP: u32 = 0x004e_0344;
const RESOURCE_CACHE: u32 = 0x004e_649c;
const SCENE_MODE: u32 = 0x004b_9ef0;
const VIEW_FOG_TAG: u32 = 0x004b_9f8c;
const VIEW_FOG_AMOUNT: u32 = 0x004b_9ffc;
const PLANAR_FOG: u32 = 0x004b_a004;
const RECORD_SLOTS: u32 = 0x352;

const BSP: u32 = 0x0320_0000;
const CLUSTER_SECTIONS: u32 = 0x0321_0000;
const DEF_SECTIONS: u32 = 0x0322_0000;
const BSP_PLACEMENTS: u32 = 0x0323_0000;
const CACHE: u32 = 0x0324_0000;
const CACHE_POOL: u32 = 0x0324_1000;
const CACHE_ENTRIES: u32 = 0x0324_2000;
const GEOMETRIES: u32 = 0x0325_0000;
const WALK_LISTS: u32 = 0x0326_0000;
const BSP_CLUSTERS: u32 = 64;
const BSP_DEFINITIONS: u32 = 8;
const BSP_PLACEMENT_COUNT: u32 = 64;
const GEOMETRY_COUNT: u32 = 16;

fn fill(m: &mut FlatMemory, r: &mut Rng, at: u32, len: u32) {
    for i in 0..len {
        put8(m, at + i, r.u32() as u8);
    }
}

/// One structure section: its geometry index, subparts, resource and
/// geometry, and its cluster bounds.
fn seed_section(m: &mut FlatMemory, r: &mut Rng, at: u32) {
    put16(m, at + 0x14, r.below(8) as u16);
    put16(
        m,
        at + 0x24,
        if r.chance(3) { 0 } else { r.below(80) as u16 },
    );
    put8(m, at + 0x28 + 0x1e, u8::from(!r.chance(40)));
    put16(m, at + 0x28 + 0x20, r.below(64) as u16);
    put(
        m,
        at + 0x50,
        if r.chance(30) {
            0
        } else {
            GEOMETRIES + 0x100 * r.below(GEOMETRY_COUNT)
        },
    );
    for i in 0..3 {
        let lo = r.float(-50.0, 50.0);
        putf(m, at + 0x54 + 8 * i, lo);
        putf(m, at + 0x58 + 8 * i, lo + r.float(0.1, 30.0));
    }
}

/// The structure the record walks read, and a light's struct of `kind`
/// whose cluster and placement lists name it; the records cursor where
/// the reset's light arm leaves it. Returns the cursor.
fn walk_world(w: &mut World, r: &mut Rng, kind: u32) -> u32 {
    let m = &mut w.mem;
    put(m, ACTIVE_BSP, BSP);
    put(m, BSP + 8, r.u32());
    put(m, BSP + 0x9c, BSP_CLUSTERS);
    put(m, BSP + 0xa0, CLUSTER_SECTIONS);
    put(m, BSP + 0x138, BSP_DEFINITIONS);
    put(m, BSP + 0x13c, DEF_SECTIONS);
    put(m, BSP + 0x140, BSP_PLACEMENT_COUNT);
    put(m, BSP + 0x144, BSP_PLACEMENTS);
    put(m, LIGHTMAP, 0);
    put(m, RESOURCE_CACHE, CACHE);
    put(m, CACHE + 0x38, r.u32());
    put(m, CACHE + 0x64, CACHE_POOL);
    put(m, CACHE_POOL + 0x44, CACHE_ENTRIES);
    for c in 0..BSP_CLUSTERS {
        seed_section(m, r, CLUSTER_SECTIONS + 0xb0 * c);
    }
    for d in 0..BSP_DEFINITIONS {
        seed_section(m, r, DEF_SECTIONS + 0xc8 * d);
    }
    for p in 0..BSP_PLACEMENT_COUNT {
        let at = BSP_PLACEMENTS + 0x58 * p;
        put16(m, at + 0x34, r.below(BSP_DEFINITIONS) as u16);
        for c in 0..3 {
            putf(m, at + 0x3c + 4 * c, r.float(-50.0, 50.0));
        }
        putf(
            m,
            at + 0x48,
            if r.chance(4) {
                r.float(0.01, 0.5)
            } else {
                r.float(0.5, 20.0)
            },
        );
        put16(m, at + 0x56, r.below(2) as u16);
    }
    // The view the camera's depth plane and pixel scale are read from.
    for (i, a) in PROJECTION_DEPTH.into_iter().enumerate() {
        putf(
            m,
            a,
            if i == 3 {
                r.float(1.0, 4.0)
            } else {
                r.float(-0.1, 0.1)
            },
        );
    }
    putf(m, PIXEL_SCALE, r.float(100.0, 900.0));
    put(m, SCENE_MODE, if r.chance(3) { 3 } else { r.below(3) });
    put(
        m,
        VIEW_FOG_TAG,
        if r.chance(3) {
            NONE
        } else {
            definition_datum(r.below(DEFINITION_COUNT))
        },
    );
    putf(
        m,
        VIEW_FOG_AMOUNT,
        if r.chance(2) { 0.0 } else { r.float(0.0, 1.0) },
    );
    put8(m, PLANAR_FOG, r.below(2) as u8);
    // The light's struct.
    put(m, RECORDS_VIS, LIGHT_VIS);
    put(m, LIGHT_VIS + 0x2a78, kind);
    put8(m, LIGHT_VIS + 0x2a60, r.u32() as u8);
    put16(
        m,
        LIGHT_VIS + 0xa5c,
        [0, r.below(40) as u16, 500 + r.below(20) as u16][r.below(3) as usize],
    );
    fill(m, r, LIGHT_VIS + 0x1c, 512);
    let clusters: Vec<u32> = (0..r.below(12)).map(|_| r.below(BSP_CLUSTERS)).collect();
    let placements: Vec<u32> = (0..r.below(12))
        .map(|_| r.below(BSP_PLACEMENT_COUNT))
        .collect();
    let mut rr = Rng(r.next());
    seed_list(m, &mut rr, WALK_LISTS, clusters.len() as u32 + 1, &clusters);
    seed_list(
        m,
        &mut rr,
        WALK_LISTS + 0x4000,
        placements.len() as u32 + 1,
        &placements,
    );
    put(m, LIGHT_VIS + 0xc, WALK_LISTS);
    put(m, LIGHT_VIS + 0x18, WALK_LISTS + 0x4000);
    // What the records' bytes held before.
    fill(m, r, SCENE_RECORDS, RECORD_SLOTS * 0x20);
    fill(m, r, PIXEL_BYTES, RECORD_SLOTS);
    fill(m, r, ATTACHMENT_BYTES, RECORD_SLOTS);
    let cursor = [0, r.below(0x300), RECORD_SLOTS - 4 + r.below(4)][r.below(3) as usize];
    put(m, RECORD_COUNT, cursor);
    put(m, MAIN_RECORD_COUNT, cursor);
    cursor
}

/// `world_walk::records` in a light's kind against the cluster walk
/// `0x00133390` and the placement walk `0x00133520`, the record writer
/// `0x0002cbf0` and its tree lifted: equal memory, and the main view's
/// count left.
#[test]
fn light_record_walks_match_the_lifted_original() {
    if !with_image(walk_cases) {
        eprintln!("SKIPPED light_record_walks_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn walk_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    use engine::render_loop::world_walk::{self, Walk};
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..96u32 {
            let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ u64::from(n) ^ 0x3390);
            let mut w = World::new(image, &mut r, 4);
            let kind = 1 + r.below(3);
            let cursor = walk_world(&mut w, &mut r, kind);
            let walk = if n % 2 == 0 {
                Walk::Clusters
            } else {
                Walk::Placements
            };
            let label = format!("seed {seed} case {n} ({walk:?}, kind {kind}, cursor {cursor:#x})");

            let mut port = Ram(w.mem.clone());
            let records =
                match world_walk::records(&mut port, LIGHT_VIS, walk, FPU.mxcsr, FPU.x87_control) {
                    Ok(records) => records,
                    Err(why) => {
                        tally.add(format!("refused {why:?}"));
                        continue;
                    }
                };

            let mut a = w.mem;
            let mut c = cpu(&mut r);
            let esp = c.gpr[4];
            match walk {
                Walk::Clusters => {
                    c.gpr[3] = LIGHT_VIS;
                    lifted.call(&mut c, &mut a, CLUSTER_WALK, esp, &[])
                }
                Walk::Placements => lifted.call(&mut c, &mut a, PLACEMENT_WALK, esp, &[LIGHT_VIS]),
            }
            .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(c.gpr[4], esp, "{label}: the walk's ret");
            assert_eq!(
                word(&a, MAIN_RECORD_COUNT),
                cursor,
                "{label}: the main view's count"
            );
            compare(&label, &a, &port.0, esp);
            tally.add("native");
            tally.add(format!("{walk:?}"));
            if records > 0 {
                tally.add("records written");
            }
            if word(&a, RECORD_COUNT) >= RECORD_SLOTS {
                tally.add("at the cap");
            }
        }
    }
    eprintln!("light record walks: {:#?}", tally.0);
    assert!(
        tally.get("native") >= 96,
        "too few walks answered: {:#?}",
        tally.0
    );
    for k in ["Clusters", "Placements", "records written", "refused Cold"] {
        assert!(tally.get(k) > 0, "{k} never seen: {:#?}", tally.0);
    }
}

// ---------------------------------------------------------------------
// L4: a light's item lists and the first-person table.

use engine::render::light_gather::items::{self, FirstPerson, LightItems, Made};

const SCENE_ITEMS: u32 = 0x0004_0e90;
const ATTACHMENTS_SITE: u32 = 0x0004_0f09;
const OVERRIDES: u32 = 0x004c_1228;
const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
const MATERIAL_VIS: u32 = 0x004e_d280;
const PART_PLANES: u32 = 0x0051_f400;
const DOUBLE_LOD: u32 = 0x004b_a019;
const FIRST_PERSON_TABLE: u32 = frame::FIRST_PERSON_TABLE;

const ITEMS_ONE: u32 = 0x0330_0000;
const ITEMS_TWO: u32 = 0x0331_0000;
const MATERIALS: u32 = 0x0332_0000;
const MATERIAL_MASKS: u32 = 0x0333_0000;
const PLANES: u32 = 0x0334_0000;
const PARTS: u32 = 0x0335_0000;
const SUBPART_MASKS: u32 = 0x0336_0000;
const SHADER_DATA: u32 = 0x0337_0000;
const TEMPLATE_DATA: u32 = 0x0338_0000;
const LEVEL_TABLES: u32 = 0x0339_0000;
const INSTANCES: u32 = 0x033a_0000;
const OBJECT_TAGS: u32 = 0x033b_0000;
const OBJECT_REGIONS: u32 = 0x033c_0000;
const MATERIAL_COUNT: u32 = 16;
const SHADERS: u32 = 12;
const SHADER_TAG: u32 = 64;
const TEMPLATE_TAG: u32 = 96;
const OBJECT_TAG: u32 = 128;

fn shader_datum(k: u32) -> u32 {
    (0xe110 << 16) | (SHADER_TAG + k)
}

/// The record walk's leaves in the port's guest calls: lifted in both
/// runs, so not in the transcript. The attachment walk is the stub.
const ITEM_LEAVES: [u32; 4] = [
    items::call::RECORD_GEOMETRY,
    items::call::PART_MATERIAL,
    items::call::PART_VISIBLE,
    items::call::PART_ITEMS,
];

/// Shaders with level tables, materials, part geometries and a light's
/// records over the structure of [`walk_world`]: structure records of
/// both kinds, first-person records, objects' records and overridden
/// shaders. Returns the records' range.
fn items_world(w: &mut World, r: &mut Rng, kind: u32) -> (u32, u32) {
    walk_world(w, r, kind);
    let m = &mut w.mem;
    put(m, BSP + 0xa4, MATERIAL_COUNT);
    put(m, BSP + 0xa8, MATERIALS);
    for k in 0..MATERIAL_COUNT {
        let at = MATERIALS + 32 * k;
        put(
            m,
            at + 0xc,
            if r.chance(8) {
                NONE
            } else {
                shader_datum(r.below(SHADERS))
            },
        );
        put8(
            m,
            at + 0x1c,
            if r.chance(2) { 0xff } else { r.below(64) as u8 },
        );
    }
    put(m, MATERIAL_VIS, MATERIAL_MASKS);
    fill(m, r, MATERIAL_MASKS, 0x2000);
    put16(m, ACTIVE_BSP_INDEX, r.below(4) as u16);
    put(m, PART_PLANES, PLANES);
    put(m, PLANES + 0x20, 16);
    put(m, PLANES + 0x2c, 0x0001_e8c0);
    put(m, PLANES + 0x30, 0x0001_e8f0);
    put8(m, DOUBLE_LOD, r.below(2) as u8);
    // The shaders, their templates and level tables.
    for k in 0..SHADERS {
        let entry = TAG_TABLE + 16 * (SHADER_TAG + k);
        let data = SHADER_DATA + 0x100 * k;
        put(m, entry, u32::from_be_bytes(*b"shad"));
        put(m, entry + 4, shader_datum(k));
        put(m, entry + 8, data);
        put16(
            m,
            data + 0x3c,
            [0, 1, 2, 3, 4, 5, 5, 7][r.below(8) as usize],
        );
        put(m, data + 0x24, INSTANCES + 16 * k);
        put(
            m,
            INSTANCES + 16 * k,
            (0xe120 << 16) | (TEMPLATE_TAG + r.below(SHADERS)),
        );
        let entry = TAG_TABLE + 16 * (TEMPLATE_TAG + k);
        let template = TEMPLATE_DATA + 0x100 * k;
        let header = LEVEL_TABLES + 0x100 * k;
        put(m, entry, u32::from_be_bytes(*b"stem"));
        put(m, entry + 4, (0xe120 << 16) | (TEMPLATE_TAG + k));
        put(m, entry + 8, template);
        put(m, template + 0x5c, header);
        let levels = 1 + r.below(4);
        put(m, header, levels);
        put(m, header + 4, header + 0x10);
        let mut threshold = r.float(100.0, 4000.0);
        for row in 0..levels {
            let at = header + 0x10 + 10 * row;
            let mut layers = r.u32() & 0x0003_ffff;
            match r.below(8) {
                0 => layers = 0x8000,
                1 => layers = 0,
                2 => layers = 0x80 | (r.u32() & 0x10),
                _ => {}
            }
            if r.chance(5) {
                layers |= 1 << 21;
            }
            put16(m, at, r.u32() as u16);
            put(m, at + 2, layers);
            putf(m, at + 6, if r.chance(12) { f32::NAN } else { threshold });
            threshold *= r.float(0.1, 0.9);
        }
    }
    // An object's region table: no geometry.
    let entry = TAG_TABLE + 16 * OBJECT_TAG;
    put(m, entry, u32::from_be_bytes(*b"hlmt"));
    put(m, entry + 4, (0xe130 << 16) | OBJECT_TAG);
    put(m, entry + 8, OBJECT_TAGS);
    put(m, OBJECT_TAGS + 0x28, OBJECT_REGIONS);
    // The part geometries.
    for g in 0..GEOMETRY_COUNT {
        let at = GEOMETRIES + 0x100 * g;
        let parts = PARTS + 0x400 * g;
        put(m, at, if r.chance(6) { 0 } else { 1 + r.below(4) });
        put(m, at + 4, parts);
        for p in 0..5 {
            let part = parts + 0x48 * p;
            put16(m, part, r.below(7) as u16);
            put16(m, part + 4, r.below(MATERIAL_COUNT) as u16);
            put16(m, part + 0xa, r.below(64) as u16);
            put16(
                m,
                part + 0xc,
                if r.chance(8) {
                    0
                } else {
                    1 + r.below(5) as u16
                },
            );
            let size = match r.below(8) {
                0 => 0.0,
                1 => f32::NAN,
                2 => -1.0,
                _ => r.float(0.1, 4.0),
            };
            putf(m, part + 0x2c, size);
        }
    }
    fill(m, r, SUBPART_MASKS, 0x400);
    for k in 0..8 {
        put(m, OVERRIDES + 4 * k, shader_datum(r.below(SHADERS)));
    }
    // The two lists.
    for (header, items) in [
        (items::LISTS + 0x18, ITEMS_ONE),
        (items::LISTS + 0x30, ITEMS_TWO),
    ] {
        put(
            m,
            header,
            if r.chance(5) {
                r.below(2)
            } else {
                2 + r.below(12)
            },
        );
        put(m, header + 4, r.u32());
        put(m, header + 8, r.u32());
        put(m, header + 0xc, r.u32());
        put(m, header + 0x14, items);
        fill(m, r, items, 16 * 0x18);
    }
    // The first-person table.
    put(
        m,
        frame::FIRST_PERSON_COUNT,
        [0, 1, 3, 8, 9][r.below(5) as usize],
    );
    for s in 0..8 {
        put16(
            m,
            FIRST_PERSON_TABLE + 0x408 * s + 0x404,
            [0, r.below(32), 31, 32][r.below(4) as usize] as u16,
        );
    }
    // The light's records after the main view's.
    let main = r.below(0x300);
    let count = r.below(40);
    put(m, MAIN_RECORD_COUNT, main);
    put(m, RECORD_COUNT, main + count);
    for i in main..main + count {
        let at = SCENE_RECORDS + 0x20 * i;
        let which = r.below(10);
        let placement = which >= 6;
        let mut flags = (r.u32() & !0x1447)
            | u32::from(!r.chance(8))
            | if r.chance(2) { 2 } else { 0 }
            | if r.chance(2) { 4 } else { 0 }
            | if r.chance(3) { 0x40 } else { 0 }
            | if placement { 0x1000 } else { 0 };
        if which == 0 {
            flags |= 0x400;
        }
        put(m, at, flags);
        put(
            m,
            at + 4,
            if which == 1 {
                (0xe130 << 16) | OBJECT_TAG
            } else {
                NONE
            },
        );
        let selector = if r.chance(10) { r.below(31) } else { 0x1f };
        let region = r.below(16);
        let packed = if placement {
            (r.below(8) << 29) | (r.below(BSP_PLACEMENT_COUNT) << 18) | (selector << 4) | region
        } else {
            (r.below(8) << 29)
                | (0x7ff << 18)
                | (r.below(BSP_CLUSTERS) << 9)
                | (selector << 4)
                | region
        };
        put(m, at + 0xc, packed);
        put(
            m,
            at + 0x14,
            if r.chance(6) {
                0
            } else {
                SUBPART_MASKS + 0x10 * r.below(48)
            },
        );
        put8(
            m,
            at + 0x1a,
            if r.chance(8) { 0 } else { r.u32() as u8 | 1 },
        );
        put8(
            m,
            ATTACHMENT_BYTES + i,
            if r.chance(6) { r.below(8) as u8 } else { 0xff },
        );
    }
    (main, main + count)
}

/// `0x00040e90` lifted with the attachment walk stubbed. Returns the
/// transcript and EAX.
fn original_items(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    cpu: &mut Cpu,
    esp: u32,
) -> Result<(Vec<Callout>, u32), String> {
    put(m, esp, HARNESS_EXIT);
    cpu.gpr[4] = esp;
    cpu.gpr[0] = 0;
    cpu.eip = SCENE_ITEMS;
    let mut transcript = Vec::new();
    lifted.run(cpu, m, &[HARNESS_EXIT], |pc, cpu, m| {
        if pc != items::call::ATTACHMENTS {
            return false;
        }
        let sp = cpu.gpr[4];
        transcript.push(Callout {
            target: pc,
            site: word(m, sp),
            regs: [0, 3, 6, 7].iter().map(|&i| cpu.gpr[i]).collect(),
            args: Vec::new(),
        });
        cpu.gpr[4] = sp + 4;
        cpu.eip = word(m, sp);
        true
    })?;
    if cpu.gpr[4] != esp + 4 {
        return Err(format!("esp {:#x} after the call", cpu.gpr[4]));
    }
    Ok((transcript, cpu.gpr[0]))
}

/// The body, driven as the runtime drives it: a leaf runs lifted, the
/// attachment walk is the stub.
fn port_items(
    lifted: &mut Lifted<'_>,
    m: FlatMemory,
    mut cpu: Cpu,
    esp: u32,
    plan: items::Plan,
) -> (
    Vec<Callout>,
    guest_layout::entry::EntryResult,
    Option<LightItems>,
    FlatMemory,
) {
    use guest_layout::entry::{Cx, EntryFuture, Regs, Step, poll, resume};
    let mem = Arc::new(Mutex::new(Ram(m)));
    let cx = Cx::new(
        Arc::clone(&mem),
        Regs {
            esp,
            ..Regs::default()
        },
    );
    let out = Arc::new(Mutex::new(None));
    let body = cx.clone();
    let kept = Arc::clone(&out);
    let mut fut: EntryFuture = Box::pin(async move {
        let (ret, light_items) = items::run(&body, plan).await?;
        *kept.lock().unwrap() = Some(light_items);
        Ok::<_, guest_layout::entry::EntryError>(ret)
    });
    let mut transcript = Vec::new();
    let result = loop {
        match poll(&mut fut, &cx) {
            Step::Done(r) => break r,
            Step::Call(c) => {
                let mut g = mem.lock().unwrap();
                let m = &mut g.0;
                let sp = c.esp(esp);
                let set = |i: usize| match i {
                    0 => c.eax,
                    1 => c.ecx,
                    2 => c.edx,
                    3 => c.ebx,
                    6 => c.esi,
                    7 => c.edi,
                    _ => None,
                };
                for i in [0, 1, 2, 3, 6, 7] {
                    if let Some(v) = set(i) {
                        cpu.gpr[i] = v;
                    }
                }
                if ITEM_LEAVES.contains(&c.target) {
                    for (i, &a) in c.args.iter().enumerate() {
                        put(m, sp + 4 + 4 * i as u32, a);
                    }
                    put(m, sp, HARNESS_EXIT);
                    cpu.gpr[4] = sp;
                    cpu.eip = c.target;
                    lifted
                        .run(&mut cpu, m, &[HARNESS_EXIT], |_, _, _| false)
                        .unwrap_or_else(|e| panic!("leaf {:#x}: {e}", c.target));
                } else {
                    assert_eq!(c.target, items::call::ATTACHMENTS, "an unknown callee");
                    transcript.push(Callout {
                        target: c.target,
                        site: ATTACHMENTS_SITE,
                        regs: [0, 3, 6, 7]
                            .iter()
                            .map(|&i| set(i).unwrap_or(0xdead_beef))
                            .collect(),
                        args: c.args.clone(),
                    });
                }
                let regs = cpu.gpr;
                drop(g);
                resume(
                    &cx,
                    Regs {
                        eax: regs[0],
                        ecx: regs[1],
                        edx: regs[2],
                        ebx: regs[3],
                        esp,
                        ebp: regs[5],
                        esi: regs[6],
                        edi: regs[7],
                        xmm0: 0,
                    },
                );
            }
        }
    };
    drop(fut);
    drop(cx);
    let light_items = out.lock().unwrap().take();
    let m = Arc::try_unwrap(mem)
        .ok()
        .expect("the body kept no memory")
        .into_inner()
        .unwrap()
        .0;
    (transcript, result, light_items, m)
}

/// `light_gather::items` against `0x00040e90` in a light's kind, its
/// record walk `0x00044ac0` and the part calls (`0x000449e0`,
/// `0x00045ce0`, `0x000460d0`, `0x00041040`, `0x00040de0`, `0x00040e30`,
/// `0x0003b9d0`, the level query `0x0001bf50`, the material visibility
/// `0x00184000` and the plane lookup) lifted, the attachment walk
/// `0x00041c80` stubbed: equal transcripts, equal EAX and equal memory
/// outside the dead stack, over receivers, casters and both, kind 3, a
/// full list, first-person records into a slot, past the eighth light and
/// into a full slot, objects' records and overridden shaders (the
/// guest's part calls), and every skip of the part.
#[test]
fn item_lists_match_the_lifted_original() {
    if !with_image(items_cases) {
        eprintln!("SKIPPED item_lists_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn items_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..64u32 {
            let mut r = Rng(seed.wrapping_mul(0x5851_f42d_4c95_7f2d) ^ u64::from(n) ^ 0x0e90);
            let mut w = World::new(image, &mut r, 4);
            let kind = [1, 1, 2, 3][r.below(4) as usize];
            let (start, end) = items_world(&mut w, &mut r, kind);
            let label = format!("seed {seed} case {n} (kind {kind}, records {start:#x}..{end:#x})");
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 4;
            put(&mut w.mem, esp, HARNESS_EXIT);

            let plan = match items::plan(&Ram(w.mem.clone()), esp, 0) {
                Ok(Some(plan)) => plan,
                Ok(None) => panic!("{label}: a light's kind"),
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            let (port_calls, result, light_items, port) =
                port_items(&mut lifted, w.mem.clone(), c.clone(), esp, plan);

            let mut a = w.mem;
            let (calls, eax) = original_items(&mut lifted, &mut a, &mut c, esp)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(
                calls, port_calls,
                "{label}: the attachment walk (original, then port)"
            );
            assert_eq!(
                result.map(|ret| (ret.eax, ret.pop)),
                Ok((eax, 0)),
                "{label}: the return"
            );
            compare(&label, &a, &port, esp);
            tally.add("native");
            let light_items = light_items.expect("the items");
            for &(_, made) in &light_items.made {
                tally.add(match made {
                    Made::Rust => "a record in Rust".to_string(),
                    Made::Guest(why) => format!("a record by the guest ({why:?})"),
                });
            }
            for copied in &light_items.first_person {
                tally.add(match copied {
                    FirstPerson::Copied { slot: 7, .. } => "a first-person record in the last slot",
                    FirstPerson::Copied { .. } => "a first-person record copied",
                    FirstPerson::Full { .. } => "a full slot",
                });
            }
            if light_items.receivers.count > 0 {
                tally.add("receivers");
            }
            if light_items.casters.count > 0 {
                tally.add("casters");
            }
            let capacity = word(&a, items::LISTS + 0x18) as i32;
            if capacity >= 0 && light_items.receivers.count as i32 == capacity {
                tally.add("a full list");
            }
        }
    }
    eprintln!("item lists: {:#?}", tally.0);
    assert!(
        tally.get("native") >= 64,
        "too few calls answered: {:#?}",
        tally.0
    );
    for k in [
        "a record in Rust",
        "a record by the guest (Unsupported)",
        "a first-person record copied",
        "a first-person record in the last slot",
        "a full slot",
        "receivers",
        "casters",
        "a full list",
    ] {
        assert!(tally.get(k) > 0, "never {k}: {:#?}", tally.0);
    }
}

// ---------------------------------------------------------------------
// L5: one light's passes.

use engine::render::light_gather::passes::{self, LightPasses};

const PASS_TAGS: u32 = 0x0340_0000;
const PASS_DATA: u32 = 0x0341_0000;
const PASS_CONTEXT: u32 = 0x0342_0000;
const PASS_OVERRIDE: u32 = 0x0343_0000;
const PASS_SCRATCH: u32 = 0x0344_0000;
const PASS_LIGHT_MASK: u32 = 0x004c_1a68;
const PASS_CASTER_MASK: u32 = 0x004c_1a80;
const PASS_FLAGS: u32 = 0x004b_a014;
const PASS_COUNT: u32 = 0x004b_a134;
const PASS_VIEW_TARGET: u32 = 0x0048_58b8;
const PASS_COLOUR_B: u32 = 0x0048_5950;
const PASS_FALLOFF_4: u32 = 0x0048_5a40;

/// The passes' callees, all stubbed: target, the registers the original
/// hands it (`Cpu::gpr` indices), its stack arguments and the bytes it
/// pops.
const PASS_STUBS: [(u32, &[usize], u32, u32); 14] = [
    (passes::call::ITEMS, &[0], 0, 0),
    (passes::call::CONSTANTS, &[0, 1, 2, 6, 7], 4, 16),
    (passes::call::SHADOW_SETUP, &[1, 3, 6, 7], 1, 4),
    (passes::call::LAYER_BEGIN, &[6, 7], 0, 0),
    (passes::call::LAYER_DRAW, &[3, 6, 7], 0, 0),
    (passes::call::LAYER_END, &[3, 6, 7], 0, 0),
    (passes::call::TARGET, &[0, 6, 7], 2, 8),
    (passes::call::CLIP_MODE, &[0, 6, 7], 0, 0),
    (passes::call::STENCIL, &[3, 6, 7], 0, 0),
    (passes::call::RECEIVER_BLEND, &[6, 7], 0, 0),
    (passes::call::ADDITIONAL_CASTERS, &[6, 7], 0, 0),
    (passes::call::STENCIL_ENABLE, &[3, 6, 7], 1, 4),
    (passes::call::SCISSORS, &[3, 6, 7], 3, 12),
    (passes::call::DEPTH_RANGE, &[3, 6, 7], 2, 8),
];

fn pass_stub(target: u32) -> Option<(&'static [usize], u32, u32)> {
    PASS_STUBS
        .iter()
        .find(|s| s.0 == target)
        .map(|&(_, regs, args, pop)| (regs, args, pop))
}

/// The stub's effect, seeded by the call's place in the transcript: what
/// a real callee writes that the passes read after it (the lists' masks,
/// the constants' colour B and falloff, the flags, the draw byte), and
/// EAX and EDX. `shadow_at` is the shadow argument's address.
fn pass_effect(seed: u64, k: usize, c: &Callout, m: &mut FlatMemory, shadow_at: u32) -> (u32, u32) {
    let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(c.target));
    let float = |r: &mut Rng| match r.below(4) {
        0 => 0.0,
        1 => -r.float(0.0, 1.0),
        _ => r.float(0.0, 1.0),
    };
    match c.target {
        passes::call::ITEMS => {
            put(m, PASS_LIGHT_MASK, (r.u32() & 0x1f) << 10);
            put(m, PASS_CASTER_MASK, (r.u32() & 0x3) << 10);
        }
        passes::call::CONSTANTS => {
            for i in 0..3 {
                putf(m, PASS_COLOUR_B + 4 * i, float(&mut r));
            }
            putf(m, PASS_FALLOFF_4, float(&mut r));
        }
        passes::call::LAYER_BEGIN => return (r.u32() & !0xff | r.below(2), r.u32()),
        passes::call::SHADOW_SETUP | passes::call::LAYER_END if r.chance(4) => {
            // The flags are read again after the shadow layer.
            let f = word(m, PASS_FLAGS) ^ (r.u32() & 0x3);
            put(m, PASS_FLAGS, f);
        }
        passes::call::STENCIL if r.chance(4) => {
            // The mask and the draw byte are read again at each layer.
            put(m, PASS_LIGHT_MASK, (r.u32() & 0x1f) << 10);
            put8(m, shadow_at, r.below(2) as u8);
        }
        _ => put(m, PASS_SCRATCH + 4 * k as u32, r.u32()),
    }
    (r.u32(), r.u32())
}

/// A light's passes' inputs: the definition, the context, the override,
/// the additional casters' count, the lists, the flags and the view's
/// target, and the call's four stack words above `esp`. Returns EAX and
/// ECX at the entry.
fn pass_world(m: &mut FlatMemory, r: &mut Rng, esp: u32) -> (u32, u32) {
    let definition = (0xe00a << 16) | r.below(16);
    put(m, TAGS_POINTER, PASS_TAGS);
    put(m, PASS_TAGS + (definition & 0xffff) * 16 + 8, PASS_DATA);
    put8(m, PASS_DATA, if r.chance(2) { 0x20 } else { 0 });
    put16(m, PASS_DATA + 4, r.below(2) as u16);
    put16(m, PASS_DATA + 0x14, [2, 0, 1][r.below(3) as usize]);
    put16(m, PASS_CONTEXT, if r.chance(3) { 0 } else { 1 + r.below(3) as u16 });
    if r.chance(3) {
        put(m, OVERRIDE, 0);
    } else {
        put(m, OVERRIDE, PASS_OVERRIDE);
        put8(m, PASS_OVERRIDE + 5, r.below(2) as u8);
    }
    put(m, PASS_COUNT, [u32::MAX, 0, 3][r.below(3) as usize]);
    put(m, PASS_LIGHT_MASK, r.u32());
    put(m, PASS_CASTER_MASK, r.u32());
    put(m, PASS_FLAGS, r.u32());
    put(m, PASS_VIEW_TARGET, r.u32());
    for i in 0..3 {
        putf(m, PASS_COLOUR_B + 4 * i, r.float(-1.0, 1.0));
    }
    put(m, esp + 4, light_datum(r.below(8)));
    put(m, esp + 8, light_record(r.below(8)) + 0x84);
    put(m, esp + 0xc, (r.u32() & !0xff) | [0, 1, 2][r.below(3) as usize]);
    putf(m, esp + 0x10, r.float(0.0, 1.0));
    (definition, PASS_CONTEXT)
}

/// `0x00031170` lifted, every callee stubbed. Returns the transcript and
/// EAX.
fn original_passes(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    cpu: &mut Cpu,
    esp: u32,
    seed: u64,
) -> Result<(Vec<Callout>, u32), String> {
    put(m, esp, HARNESS_EXIT);
    cpu.gpr[4] = esp;
    cpu.eip = passes_entry();
    let shadow_at = esp + 0xc;
    let mut transcript = Vec::new();
    lifted.run(cpu, m, &[HARNESS_EXIT], |pc, cpu, m| {
        let Some((regs, args, pop)) = pass_stub(pc) else {
            return false;
        };
        let sp = cpu.gpr[4];
        let c = Callout {
            target: pc,
            site: word(m, sp),
            regs: regs.iter().map(|&i| cpu.gpr[i]).collect(),
            args: (0..args).map(|i| word(m, sp + 4 + 4 * i)).collect(),
        };
        let (eax, edx) = pass_effect(seed, transcript.len(), &c, m, shadow_at);
        cpu.gpr[0] = eax;
        cpu.gpr[2] = edx;
        cpu.gpr[4] = sp + 4 + pop;
        cpu.eip = c.site;
        transcript.push(c);
        true
    })?;
    if cpu.gpr[4] != esp + 4 + 0x10 {
        return Err(format!("esp {:#x} after the passes", cpu.gpr[4]));
    }
    Ok((transcript, cpu.gpr[0]))
}

fn passes_entry() -> u32 {
    guest_layout::points::RENDER_LIGHT_PASSES.base
}

/// The body, every guest call a stub, its return site the one its slot
/// names ([`passes::return_site`]).
fn port_passes(
    m: FlatMemory,
    esp: u32,
    plan: passes::Plan,
    seed: u64,
) -> (
    Vec<Callout>,
    guest_layout::entry::EntryResult,
    Option<LightPasses>,
    FlatMemory,
) {
    use guest_layout::entry::{Cx, EntryFuture, GuestCall, Regs, Step, poll, resume};
    let mem = Arc::new(Mutex::new(Ram(m)));
    let cx = Cx::new(
        Arc::clone(&mem),
        Regs {
            esp,
            ..Regs::default()
        },
    );
    let out = Arc::new(Mutex::new(None));
    let body = cx.clone();
    let kept = Arc::clone(&out);
    let mut fut: EntryFuture = Box::pin(async move {
        let (ret, light) = passes::run(&body, plan).await?;
        *kept.lock().unwrap() = Some(light);
        Ok::<_, guest_layout::entry::EntryError>(ret)
    });
    let shadow_at = esp + 0xc;
    let mut transcript = Vec::new();
    let result = loop {
        match poll(&mut fut, &cx) {
            Step::Done(r) => break r,
            Step::Call(c) => {
                let mut g = mem.lock().unwrap();
                let m = &mut g.0;
                let sp = c.esp(esp);
                let set = |c: &GuestCall, i: usize| match i {
                    0 => c.eax,
                    1 => c.ecx,
                    2 => c.edx,
                    3 => c.ebx,
                    6 => c.esi,
                    7 => c.edi,
                    _ => None,
                };
                let (declared, _, _) = pass_stub(c.target)
                    .unwrap_or_else(|| panic!("an unknown callee {:#x}", c.target));
                for (i, &a) in c.args.iter().enumerate() {
                    put(m, sp + 4 + 4 * i as u32, a);
                }
                put(m, sp, STUB_RETURN);
                let callout = Callout {
                    target: c.target,
                    site: passes::return_site(sp).unwrap_or(0),
                    regs: declared
                        .iter()
                        .map(|&i| set(&c, i).unwrap_or(0xdead_beef))
                        .collect(),
                    args: c.args.clone(),
                };
                let (eax, edx) = pass_effect(seed, transcript.len(), &callout, m, shadow_at);
                transcript.push(callout);
                drop(g);
                resume(
                    &cx,
                    Regs {
                        eax,
                        edx,
                        esp,
                        ..Regs::default()
                    },
                );
            }
        }
    };
    drop(fut);
    drop(cx);
    let light = out.lock().unwrap().take();
    let m = Arc::try_unwrap(mem)
        .ok()
        .expect("the body kept no memory")
        .into_inner()
        .unwrap()
        .0;
    (transcript, result, light, m)
}

/// Equal transcripts (target, return site, the registers each callee is
/// handed, its stack arguments), equal EAX and `ret 0x10`, and equal
/// memory outside the dead stack and the four saved registers, over
/// every flag combination the seeds reach: a shadow byte the casters keep
/// or drop, a sphere context, a dark or zeroed colour B, the falloff and
/// the definition's word `+0x14`, each layer's mask bit, a layer-10 begin
/// that answers either way, the override's byte and the additional
/// casters' count, and flags and masks a callee changes.
#[test]
fn passes_transcripts_match_the_lifted_original() {
    if !with_image(passes_cases) {
        eprintln!("SKIPPED passes_transcripts_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn passes_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..200u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n) ^ 0x1170);
            let mut m = image.clone();
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 0x20;
            let (eax, ecx) = pass_world(&mut m, &mut r, esp);
            c.gpr[0] = eax;
            c.gpr[1] = ecx;
            put(&mut m, esp, HARNESS_EXIT);
            let label = format!("seed {seed} case {n}");
            let case_seed = r.next();
            let plan = match passes::plan(&Ram(m.clone()), esp, eax, ecx, FPU) {
                Ok(plan) => plan,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            let (port_calls, result, light, port) = port_passes(m.clone(), esp, plan, case_seed);
            let mut a = m;
            let (calls, eax) = original_passes(&mut lifted, &mut a, &mut c, esp, case_seed)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            for (i, (x, y)) in calls.iter().zip(&port_calls).enumerate() {
                assert_eq!(x, y, "{label}: callout {i} (original, then port)");
            }
            assert_eq!(calls.len(), port_calls.len(), "{label}: the number of callouts");
            assert_eq!(
                result.map(|ret| (ret.eax, ret.pop)),
                Ok((eax, 0x10)),
                "{label}: the return"
            );
            // Below the entry ESP: the dead stack and the four saved
            // registers, the original's alone.
            if let Some(at) = first_difference(&a, &port, esp) {
                panic!(
                    "{label}: memory differs first at {at:#x} (E{:+#x}): {:#04x} (original) vs {:#04x} (port)",
                    (at as u32).wrapping_sub(esp) as i32,
                    a.bytes[at],
                    port.bytes[at]
                );
            }
            tally.add("native");
            let light = light.expect("what it did");
            tally.add(if light.shadow { "shadowed" } else { "unshadowed" });
            if light.additional_casters {
                tally.add("additional casters");
            }
            for layer in [10, 11, 12, 13, 14] {
                if light.drawn & 1 << layer != 0 {
                    tally.add(format!("layer {layer} drawn"));
                } else if light.reached & 1 << layer != 0 {
                    tally.add(format!("layer {layer} reached only"));
                }
            }
        }
    }
    eprintln!("passes: {:#?}", tally.0);
    assert!(tally.get("native") >= 400, "too few answered: {:#?}", tally.0);
    for need in [
        "shadowed",
        "unshadowed",
        "additional casters",
        "layer 10 drawn",
        "layer 10 reached only",
        "layer 11 drawn",
        "layer 11 reached only",
        "layer 12 drawn",
        "layer 12 reached only",
        "layer 13 drawn",
        "layer 14 drawn",
        "layer 14 reached only",
    ] {
        assert!(tally.get(need) > 0, "never {need}: {:#?}", tally.0);
    }
}

// ---------------------------------------------------------------------
// L6: the first-person lights and each slot's record replay.

use engine::render::light_gather::first_person::{self, FirstPersonLights, SlotRecords};

const FP_DEVICE: u32 = 0x0345_0000;
const FP_GEOMETRY: u32 = 0x0346_0000;
const FP_SCRATCH: u32 = 0x0347_0000;
const FP_DEVICE_POINTER: u32 = 0x0040_7488;
const FP_PUSH_BYTES: u32 = 0x0040_8648;
const FP_RANGE: u32 = 0x0048_5adc;
const FP_LAYER_WORDS: u32 = 0x0046_7130;
const FP_TARGETS: u32 = 0x0048_58b4;
const FP_LIGHT_ACTIVE: u32 = 0x0048_58bc;
const FP_STENCIL_STATE: u32 = 0x004b_843c;
const FP_PASSES: u32 = 0x004b_a018;
const FP_OBJECT_RECORD: u32 = (0xe130 << 16) | OBJECT_TAG;

/// A stub table: target, the registers the original hands it
/// (`Cpu::gpr` indices), its stack arguments and the bytes it pops.
type Stubs = &'static [(u32, &'static [usize], u32, u32)];

/// `0x000404b0`'s callees, all stubbed but the light volume (lifted in
/// both runs, as in the frame's differential).
const FP_STUBS: Stubs = &[
    (first_person::call::RECORDS, &[1, 2, 3, 6, 7], 1, 4),
    (first_person::call::CONSTANTS, &[0, 1, 2, 3, 6, 7], 4, 16),
    (first_person::call::TARGET, &[0, 3, 6, 7], 2, 8),
    (first_person::call::CLIP_MODE, &[0, 3, 6, 7], 0, 0),
    (first_person::call::RECEIVER_BLEND, &[3, 6, 7], 0, 0),
    (first_person::call::STENCIL, &[3, 6, 7], 0, 0),
    (first_person::call::LAYER_DRAW, &[3, 6, 7], 0, 0),
    (first_person::call::LAYER_END, &[3, 6, 7], 0, 0),
    (first_person::call::STENCIL_ENABLE, &[3, 6, 7], 1, 4),
    (first_person::call::SCISSORS, &[3, 6, 7], 3, 12),
    (first_person::call::PUSH_SPACE, &[0, 1, 3, 6, 7], 1, 4),
    (first_person::call::PUSH_STATE, &[0, 3, 6, 7], 0, 0),
];

/// `0x000402b0`'s per-part callees, when stubbed.
const RECORD_STUBS: Stubs = &[
    (items::call::RECORD_GEOMETRY, &[0, 1, 2, 3, 6, 7], 1, 4),
    (items::call::PART_MATERIAL, &[0, 1, 3, 6, 7], 2, 8),
    (items::call::PART_VISIBLE, &[0, 3, 6, 7], 1, 4),
    (items::call::PART_ITEMS, &[0, 1, 2, 3, 6, 7], 7, 0x1c),
];

fn stub_in(stubs: Stubs, target: u32) -> Option<(&'static [usize], u32, u32)> {
    stubs
        .iter()
        .find(|s| s.0 == target)
        .map(|&(_, regs, args, pop)| (regs, args, pop))
}

/// The lifted original from `entry` at `esp`, each callee in `stubs`
/// answered by `effect`; its return site is the word the call pushed when
/// `sites`, else 0 (the record replay's calls name none). Returns the
/// transcript, EAX and ESP after the return.
#[allow(clippy::too_many_arguments)]
fn original_stubbed(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    cpu: &mut Cpu,
    entry: u32,
    esp: u32,
    stubs: Stubs,
    sites: bool,
    mut effect: impl FnMut(usize, &Callout, &mut FlatMemory) -> (u32, u32),
) -> Result<(Vec<Callout>, u32, u32), String> {
    put(m, esp, HARNESS_EXIT);
    cpu.gpr[4] = esp;
    cpu.eip = entry;
    let mut transcript = Vec::new();
    lifted.run(cpu, m, &[HARNESS_EXIT], |pc, cpu, m| {
        let Some((regs, args, pop)) = stub_in(stubs, pc) else {
            return false;
        };
        let sp = cpu.gpr[4];
        let return_to = word(m, sp);
        let c = Callout {
            target: pc,
            site: if sites { return_to } else { 0 },
            regs: regs.iter().map(|&i| cpu.gpr[i]).collect(),
            args: (0..args).map(|i| word(m, sp + 4 + 4 * i)).collect(),
        };
        let (eax, edx) = effect(transcript.len(), &c, m);
        cpu.gpr[0] = eax;
        cpu.gpr[2] = edx;
        cpu.gpr[4] = sp + 4 + pop;
        cpu.eip = return_to;
        transcript.push(c);
        true
    })?;
    Ok((transcript, cpu.gpr[0], cpu.gpr[4]))
}

/// A body driven as the runtime drives it: a guest call to one of
/// `leaves` runs lifted, any other is the stub of `stubs` answered by
/// `effect`, its return site the one its slot names
/// ([`passes::return_site`]) when `sites`, else 0.
#[allow(clippy::too_many_arguments)]
fn port_stubbed(
    lifted: &mut Lifted<'_>,
    m: FlatMemory,
    mut cpu: Cpu,
    esp: u32,
    body: impl FnOnce(guest_layout::entry::Cx) -> guest_layout::entry::EntryFuture,
    leaves: &[u32],
    stubs: Stubs,
    sites: bool,
    mut effect: impl FnMut(usize, &Callout, &mut FlatMemory) -> (u32, u32),
) -> (Vec<Callout>, guest_layout::entry::EntryResult, FlatMemory) {
    use guest_layout::entry::{Cx, GuestCall, Regs, Step, poll, resume};
    let mem = Arc::new(Mutex::new(Ram(m)));
    let cx = Cx::new(
        Arc::clone(&mem),
        Regs {
            eax: cpu.gpr[0],
            ecx: cpu.gpr[1],
            edx: cpu.gpr[2],
            ebx: cpu.gpr[3],
            esp,
            ebp: cpu.gpr[5],
            esi: cpu.gpr[6],
            edi: cpu.gpr[7],
            xmm0: cpu.xmm[0],
        },
    );
    let mut fut = body(cx.clone());
    let mut transcript = Vec::new();
    let result = loop {
        match poll(&mut fut, &cx) {
            Step::Done(r) => break r,
            Step::Call(c) => {
                let mut g = mem.lock().unwrap();
                let m = &mut g.0;
                let sp = c.esp(esp);
                let set = |c: &GuestCall, i: usize| match i {
                    0 => c.eax,
                    1 => c.ecx,
                    2 => c.edx,
                    3 => c.ebx,
                    6 => c.esi,
                    7 => c.edi,
                    _ => None,
                };
                for (i, &a) in c.args.iter().enumerate() {
                    put(m, sp + 4 + 4 * i as u32, a);
                }
                let regs = if leaves.contains(&c.target) {
                    put(m, sp, HARNESS_EXIT);
                    for i in [0, 1, 2, 3, 6, 7] {
                        if let Some(v) = set(&c, i) {
                            cpu.gpr[i] = v;
                        }
                    }
                    cpu.gpr[4] = sp;
                    cpu.eip = c.target;
                    lifted
                        .run(&mut cpu, m, &[HARNESS_EXIT], |_, _, _| false)
                        .unwrap_or_else(|e| panic!("leaf {:#x}: {e}", c.target));
                    cpu.gpr
                } else {
                    let (declared, _, _) = stub_in(stubs, c.target)
                        .unwrap_or_else(|| panic!("an unknown callee {:#x}", c.target));
                    put(m, sp, STUB_RETURN);
                    let callout = Callout {
                        target: c.target,
                        site: if sites {
                            passes::return_site(sp).unwrap_or(0)
                        } else {
                            0
                        },
                        regs: declared
                            .iter()
                            .map(|&i| set(&c, i).unwrap_or(0xdead_beef))
                            .collect(),
                        args: c.args.clone(),
                    };
                    let (eax, edx) = effect(transcript.len(), &callout, m);
                    transcript.push(callout);
                    let mut gpr = cpu.gpr;
                    gpr[0] = eax;
                    gpr[2] = edx;
                    gpr
                };
                drop(g);
                resume(
                    &cx,
                    Regs {
                        eax: regs[0],
                        ecx: regs[1],
                        edx: regs[2],
                        ebx: regs[3],
                        esp,
                        ebp: regs[5],
                        esi: regs[6],
                        edi: regs[7],
                        xmm0: 0,
                    },
                );
            }
        }
    };
    drop(fut);
    drop(cx);
    let m = Arc::try_unwrap(mem)
        .ok()
        .expect("the body kept no memory")
        .into_inner()
        .unwrap()
        .0;
    (transcript, result, m)
}

fn assert_transcripts(label: &str, original: &[Callout], port: &[Callout]) {
    for (i, (x, y)) in original.iter().zip(port).enumerate() {
        assert_eq!(x, y, "{label}: callout {i} (original, then port)");
    }
    assert_eq!(original.len(), port.len(), "{label}: the number of callouts");
}

/// What a first-person light's callee writes that the loop reads after
/// it, seeded by the call's place: the light's list's mask (the record
/// replay, and again at a stencil call with the draw byte), colour B
/// (the constants, a NaN among them), the flags (a layer's end), the
/// table's count (at most eight), and the push buffer's pointers.
fn fp_effect(seed: u64, k: usize, c: &Callout, m: &mut FlatMemory, draws_at: u32) -> (u32, u32) {
    let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(c.target));
    match c.target {
        first_person::call::RECORDS => {
            put(m, PASS_LIGHT_MASK, (r.u32() & 0xf) << 11);
            if r.chance(8) {
                put(m, FIRST_PERSON_COUNT, r.below(9));
            }
        }
        first_person::call::CONSTANTS => {
            for i in 0..3 {
                let v = match r.below(6) {
                    0 => 0.0,
                    1 => -r.float(0.0, 1.0),
                    2 => f32::NAN,
                    3 => -0.0,
                    _ => r.float(0.0, 1.0),
                };
                putf(m, PASS_COLOUR_B + 4 * i, v);
            }
        }
        first_person::call::STENCIL if r.chance(4) => {
            put(m, PASS_LIGHT_MASK, (r.u32() & 0xf) << 11);
            put8(m, draws_at, r.below(2) as u8);
        }
        first_person::call::LAYER_END if r.chance(4) => {
            let f = word(m, PASS_FLAGS) ^ (r.u32() & 0x3);
            put(m, PASS_FLAGS, f);
        }
        first_person::call::PUSH_SPACE | first_person::call::PUSH_STATE => {
            return (r.u32(), r.u32());
        }
        _ => put(m, FP_SCRATCH + 4 * (k as u32 & 0xfff), r.u32()),
    }
    (r.u32(), r.u32())
}

/// The first-person table over the world's lights: its count (past eight
/// sometimes), every slot's light and count (none, one, some, 32, and
/// negative), the mode, the lists' mask, colour B, the flags, the layer
/// words and targets, and the device with a push buffer that has room
/// or not.
fn fp_world(w: &mut World, r: &mut Rng, esp: u32) {
    let m = &mut w.mem;
    put(
        m,
        FIRST_PERSON_COUNT,
        [0, 1, 1, 3, 8, 9, u32::MAX][r.below(7) as usize],
    );
    for s in 0..8 {
        let slot = FIRST_PERSON_TABLE + 0x408 * s;
        put(m, slot + 0x400, light_datum(r.below(w.lights)));
        put16(
            m,
            slot + 0x404,
            [0, 1, r.below(33), 32, 0xffff][r.below(5) as usize] as u16,
        );
    }
    put(m, esp + 4, [0, 1, 2, 3][r.below(4) as usize]);
    put(m, PASS_LIGHT_MASK, r.u32());
    for i in 0..3 {
        putf(m, PASS_COLOUR_B + 4 * i, r.float(-1.0, 1.0));
    }
    put(m, PASS_FLAGS, r.u32());
    put8(m, FP_PASSES, r.u32() as u8);
    for i in 0..3 {
        put(m, FP_LAYER_WORDS + 4 * i, r.u32());
    }
    put(m, FP_TARGETS, r.u32());
    put(m, PASS_VIEW_TARGET, r.u32());
    put8(m, FP_LIGHT_ACTIVE, r.u32() as u8);
    put(m, FP_STENCIL_STATE, r.u32());
    put(m, FP_RANGE, r.u32());
    put(m, FP_RANGE + 4, r.u32());
    put(m, FP_DEVICE_POINTER, FP_DEVICE);
    let end = 0x1000 + r.below(0x1000);
    put(
        m,
        FP_DEVICE,
        if r.chance(3) { end + r.below(16) } else { end - 1 - r.below(0x800) },
    );
    put(m, FP_DEVICE + 4, end);
    fill(m, r, FP_DEVICE + 0x1c10, 12);
    put(m, FP_PUSH_BYTES, r.below(0x400));
}

/// `light_gather::first_person::run` against `0x000404b0` lifted with
/// every callee stubbed but the light volume `0x000c17f0` (lifted in both
/// runs): equal transcripts (target, return site, the registers each
/// callee is handed, its stack arguments), equal EAX and `ret 4`, and
/// equal memory outside the dead stack, over no slot, one, eight and a
/// count a callee changes, empty and negative slots, each layer's mask
/// bit and a draw byte a callee changes, a kind-0 volume, colour B dark,
/// negative, NaN or bright, and a push buffer with room or without.
#[test]
fn first_person_lights_match_the_lifted_original() {
    if !with_image(first_person_cases) {
        eprintln!("SKIPPED first_person_lights_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn first_person_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..120u32 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(n) ^ 0x04b0);
            let mut w = World::new(image, &mut r, [4, 16, 40][(n % 3) as usize]);
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 4;
            fp_world(&mut w, &mut r, esp);
            put(&mut w.mem, esp, HARNESS_EXIT);
            let label = format!("seed {seed} case {n}");
            let case_seed = r.next();
            let draws_at = esp - first_person::slot::DRAWS;

            let plan = match first_person::plan(&Ram(w.mem.clone()), esp, FPU) {
                Ok(plan) => plan,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            let out: Arc<Mutex<Option<FirstPersonLights>>> = Arc::new(Mutex::new(None));
            let kept = Arc::clone(&out);
            let body = move |cx: guest_layout::entry::Cx| -> guest_layout::entry::EntryFuture {
                Box::pin(async move {
                    let (ret, lights) = first_person::run(&cx, plan).await?;
                    *kept.lock().unwrap() = Some(lights);
                    Ok::<_, guest_layout::entry::EntryError>(ret)
                })
            };
            let (port_calls, result, port) = port_stubbed(
                &mut lifted,
                w.mem.clone(),
                c.clone(),
                esp,
                body,
                &[LIGHT_VOLUME],
                FP_STUBS,
                true,
                |k, callout, m| fp_effect(case_seed, k, callout, m, draws_at),
            );
            let mut a = w.mem;
            // The original's depth range (the device's put and `+0x1c10`,
            // `0x003faae0`, `0x003f6190`) only fed D3D; the port makes
            // none of it. Its calls leave the put as it is and keep the
            // later callees' places; its words are put back.
            let put_at = FP_DEVICE as usize..FP_DEVICE as usize + 8;
            let range_at = (FP_DEVICE + 0x1c10) as usize..(FP_DEVICE + 0x1c1c) as usize;
            let (put_words, range_words) = (a.bytes[put_at.clone()].to_vec(), a.bytes[range_at.clone()].to_vec());
            let mut pushes = 0;
            let (raw, eax, after) = original_stubbed(
                &mut lifted,
                &mut a,
                &mut c,
                first_person_entry(),
                esp,
                FP_STUBS,
                true,
                |k, callout, m| {
                    if matches!(callout.target, first_person::call::PUSH_SPACE | first_person::call::PUSH_STATE) {
                        pushes += 1;
                        return (callout.regs[0], 0);
                    }
                    fp_effect(case_seed, k - pushes, callout, m, draws_at)
                },
            )
            .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(after, esp + 8, "{label}: ESP after the original");
            a.bytes[put_at].copy_from_slice(&put_words);
            a.bytes[range_at].copy_from_slice(&range_words);
            let pushed = raw.iter().any(|c| c.target == first_person::call::PUSH_SPACE);
            let port_esi = |site: u32| port_calls.iter().find(|c| c.site == site).map(|c| c.regs[2]);
            let calls: Vec<Callout> = raw
                .into_iter()
                .filter(|c| !matches!(c.target, first_person::call::PUSH_SPACE | first_person::call::PUSH_STATE))
                .map(|mut c| {
                    if c.site == first_person::site::VIEW_TARGET {
                        // The original's EAX is AL 1 over the put the push
                        // left, its ESI the device; the port's, 1 and the
                        // light's record.
                        c.regs[0] &= 0xff;
                        c.regs[2] = port_esi(c.site).unwrap_or(c.regs[2]);
                    }
                    c
                })
                .collect();
            assert_transcripts(&label, &calls, &port_calls);
            assert_eq!(
                result.map(|ret| (ret.eax, ret.pop)),
                Ok((eax, 4)),
                "{label}: the return"
            );
            compare(&label, &a, &port, esp);
            tally.add("native");
            let lights = out.lock().unwrap().take().expect("what it did");
            tally.add(format!("{} lights", lights.lights.len().min(2)));
            if lights.lights.len() == 8 {
                tally.add("8 lights");
            }
            for l in &lights.lights {
                tally.add(format!("volume {:?}", l.volume));
                tally.add(format!("flags {}", l.flags));
                for layer in [11, 12, 13, 14] {
                    if l.drawn & 1 << layer != 0 {
                        tally.add(format!("layer {layer} drawn"));
                    } else if l.reached & 1 << layer != 0 {
                        tally.add(format!("layer {layer} reached only"));
                    }
                }
            }
            if pushed {
                tally.add("push space");
            }
        }
    }
    eprintln!("first-person lights: {:#?}", tally.0);
    assert!(tally.get("native") >= 200, "too few answered: {:#?}", tally.0);
    for need in [
        "0 lights",
        "1 lights",
        "2 lights",
        "refused Unsupported",
        "flags 0",
        "flags 2",
        "flags 3",
        "layer 11 drawn",
        "layer 11 reached only",
        "layer 12 drawn",
        "layer 13 drawn",
        "layer 13 reached only",
        "layer 14 drawn",
        "layer 14 reached only",
        "push space",
    ] {
        assert!(tally.get(need) > 0, "never {need}: {:#?}", tally.0);
    }
}

fn first_person_entry() -> u32 {
    guest_layout::points::FIRST_PERSON_LIGHTS.base
}

fn first_person_records_entry() -> u32 {
    guest_layout::points::FIRST_PERSON_LIGHT_RECORDS.base
}

/// A stubbed per-part callee's answer: a geometry of up to four parts or
/// none, a part material written into the scratch (or no shader) and its
/// answer, a subpart answer.
fn record_effect(seed: u64, k: usize, c: &Callout, m: &mut FlatMemory) -> (u32, u32) {
    let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(c.target));
    match c.target {
        items::call::RECORD_GEOMETRY => {
            if r.chance(5) {
                return (0, r.u32());
            }
            let at = FP_GEOMETRY + 0x10 * (k as u32 & 0xff);
            put(m, at, r.below(5));
            return (at, r.u32());
        }
        items::call::PART_MATERIAL => {
            let scratch = c.regs[0];
            put(
                m,
                scratch,
                if r.chance(5) {
                    NONE
                } else {
                    shader_datum(r.below(SHADERS))
                },
            );
            return ((r.u32() & !0xff) | r.below(2), r.u32());
        }
        items::call::PART_VISIBLE => return ((r.u32() & !0xff) | r.below(2), r.u32()),
        _ => put(m, FP_SCRATCH + 0x4000 + 4 * (k as u32 & 0xfff), r.u32()),
    }
    (r.u32(), r.u32())
}

/// One slot's records over [`items_world`]: the world's light records
/// moved into slot `slot` (up to 32, or none, or a count that is not
/// above zero), the record tail where they go overwritten. With
/// `objects`, every record is an object's, so the guest's per-part calls
/// make each and they can be stubbed.
fn records_world(w: &mut World, r: &mut Rng, slot: u32, objects: bool) {
    let (start, end) = items_world(w, r, 1);
    let m = &mut w.mem;
    let base = FIRST_PERSON_TABLE + 0x408 * slot;
    let made = end - start;
    let count: u16 = match r.below(6) {
        0 => 0,
        1 => 0xfffe,
        2 => made.min(32) as u16,
        _ => r.below(made.min(32) + 1) as u16,
    };
    for t in 0..u32::from(count).min(32) {
        let from = SCENE_RECORDS + 0x20 * (start + t);
        for b in 0..0x20 {
            let v = m.bytes[(from + b) as usize];
            put8(m, base + 0x20 * t + b, v);
        }
        if objects {
            put(m, base + 0x20 * t + 4, FP_OBJECT_RECORD);
        }
    }
    put16(m, base + 0x404, count);
    fill(m, r, SCENE_RECORDS + 0x20 * start, 0x20 * made.max(1));
    put(m, RECORD_COUNT, r.u32());
}

/// `light_gather::first_person::run_records` against `0x000402b0`,
/// twice: with its per-part callees (`0x000449e0`, `0x00045ce0`,
/// `0x000460d0`, `0x00041040` and what they call) lifted in both runs
/// over the item lists' synthetic structure, where the port appends the
/// structure records in Rust; and with them stubbed over objects'
/// records, where the transcripts (target, the registers, the stack
/// arguments, among them the pixel dword with the frame's stale bytes and
/// the light's kind 1) must be equal. Equal EAX and `ret 4` and equal
/// memory outside the dead stack in both, over the three modes, an empty
/// slot, a slot of 32 and a count below zero.
#[test]
fn first_person_records_match_the_lifted_original() {
    if !with_image(first_person_records_cases) {
        eprintln!("SKIPPED first_person_records_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn first_person_records_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..96u32 {
            let mut r = Rng(seed.wrapping_mul(0x5851_f42d_4c95_7f2d) ^ u64::from(n) ^ 0x02b0);
            let mut w = World::new(image, &mut r, 4);
            let slot = r.below(8);
            let stubbed = n % 2 == 1;
            records_world(&mut w, &mut r, slot, stubbed);
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 8;
            let mode = [0, 1, 2, 3][r.below(4) as usize];
            put(&mut w.mem, esp + 4, mode);
            put(&mut w.mem, esp, HARNESS_EXIT);
            c.gpr[2] = slot;
            let eax = c.gpr[0];
            let label = format!(
                "seed {seed} case {n} (slot {slot}, mode {mode}, {})",
                if stubbed { "stubbed" } else { "lifted" }
            );
            let case_seed = r.next();

            let plan = match first_person::plan_records(&Ram(w.mem.clone()), esp, eax, slot, FPU) {
                Ok(plan) => plan,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            let out: Arc<Mutex<Option<SlotRecords>>> = Arc::new(Mutex::new(None));
            let kept = Arc::clone(&out);
            let body = move |cx: guest_layout::entry::Cx| -> guest_layout::entry::EntryFuture {
                Box::pin(async move {
                    let (ret, records) = first_person::run_records(&cx, plan).await?;
                    *kept.lock().unwrap() = Some(records);
                    Ok::<_, guest_layout::entry::EntryError>(ret)
                })
            };
            let (stubs, leaves): (Stubs, &[u32]) = if stubbed {
                (RECORD_STUBS, &[])
            } else {
                (&[], &ITEM_LEAVES)
            };
            let (port_calls, result, port) = port_stubbed(
                &mut lifted,
                w.mem.clone(),
                c.clone(),
                esp,
                body,
                leaves,
                stubs,
                false,
                |k, callout, m| record_effect(case_seed, k, callout, m),
            );
            let mut a = w.mem;
            let (calls, eax, after) = original_stubbed(
                &mut lifted,
                &mut a,
                &mut c,
                first_person_records_entry(),
                esp,
                stubs,
                false,
                |k, callout, m| record_effect(case_seed, k, callout, m),
            )
            .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(after, esp + 8, "{label}: ESP after the original");
            assert_transcripts(&label, &calls, &port_calls);
            assert_eq!(
                result.map(|ret| (ret.eax, ret.pop)),
                Ok((eax, 4)),
                "{label}: the return"
            );
            compare(&label, &a, &port, esp);
            tally.add("native");
            let records = out.lock().unwrap().take().expect("what it did");
            let count = word(&a, FIRST_PERSON_TABLE + 0x408 * slot + 0x404) as u16 as i16;
            tally.add(match count {
                i16::MIN..=0 => "an empty slot",
                32 => "a slot of 32",
                _ => "a slot",
            });
            tally.add(format!("mode {}", mode.min(2)));
            for &(_, made) in &records.made {
                tally.add(match made {
                    Made::Rust => "a record in Rust".to_string(),
                    Made::Guest(why) => format!("a record by the guest ({why:?})"),
                });
            }
            if stubbed && calls.iter().any(|c| c.target == items::call::PART_ITEMS) {
                tally.add("a part's items stubbed");
            }
            if !stubbed && word(&a, items::LISTS + 0x1c) as i32 > 0 {
                tally.add("receivers");
            }
        }
    }
    eprintln!("first-person records: {:#?}", tally.0);
    assert!(tally.get("native") >= 150, "too few answered: {:#?}", tally.0);
    for need in [
        "an empty slot",
        "a slot of 32",
        "a slot",
        "mode 0",
        "mode 1",
        "mode 2",
        "a record in Rust",
        "a record by the guest (Unsupported)",
        "a part's items stubbed",
        "receivers",
    ] {
        assert!(tally.get(need) > 0, "never {need}: {:#?}", tally.0);
    }
}

// Object shadows (L7).

use engine::render::light_gather::object_shadows::{self, rows};

const SHADOW_LIST: u32 = 0x0003_35e0;
const SHADOW_ROWS: u32 = 0x0003_3400;
const SHADOW_CASTER: u32 = 0x0003_2350;
const SHADOW_LIGHTMAP: u32 = 0x0002_26d0;
const SHADOW_PERMISSION: u32 = 0x0003_35c0;
const SHADOW_TARGET: u32 = 0x0001_ddd0;
const SHADOW_ELIGIBLE: u32 = 0x0003_d7f0;
const SHADOW_OWNER: u32 = 0x0003_e9c0;
const SHADOW_MODE: u32 = 0x0048_5a46;
const SHADOW_LIST_MODE: u32 = 0x0050_9413;
const SHADOW_OBJECTS: u32 = 0x0054_7f9c;
const SHADOW_LOG: u32 = 0x0348_0000;

const SHADOW_LIST_STUBS: Stubs = &[
    (SHADOW_LIGHTMAP, &[], 0, 0),
    (SHADOW_PERMISSION, &[], 0, 0),
    (SHADOW_TARGET, &[1, 3, 6, 7], 0, 0),
    (SHADOW_ROWS, &[0, 1, 3, 6, 7], 2, 8),
    (SHADOW_CASTER, &[0, 2, 3, 6, 7], 2, 8),
];

const SHADOW_ROW_STUBS: Stubs = &[
    (SHADOW_ELIGIBLE, &[0, 1, 2, 3, 6, 7], 3, 12),
    (SHADOW_OWNER, &[0, 1, 2, 3, 6, 7], 0, 0),
];

/// Run both bodies from the same image. `live` includes any scratch a
/// callee can observe after the final call (the outer loop's row buffer).
#[allow(clippy::too_many_arguments)]
fn compare_shadow_body(
    lifted: &mut Lifted<'_>,
    mut m: FlatMemory,
    mut c: Cpu,
    entry: u32,
    live: u32,
    pop: u16,
    stubs: Stubs,
    body: impl FnOnce(guest_layout::entry::Cx) -> guest_layout::entry::EntryFuture,
    effect: impl Fn(usize, &Callout, &mut FlatMemory) -> (u32, u32) + Copy,
    label: &str,
) -> Vec<Callout> {
    let esp = c.gpr[4];
    put(&mut m, esp, HARNESS_EXIT);
    let (port_calls, result, port) =
        port_stubbed(lifted, m.clone(), c.clone(), esp, body, &[], stubs, true, effect);
    let (calls, eax, after) = original_stubbed(lifted, &mut m, &mut c, entry, esp, stubs, true, effect)
        .unwrap_or_else(|e| panic!("{label}: {e}"));
    assert_transcripts(label, &calls, &port_calls);
    assert_eq!(after, esp + 4 + u32::from(pop), "{label}: ESP after return");
    assert_eq!(result.map(|ret| (ret.eax, ret.pop)), Ok((eax, pop)), "{label}: return");
    compare(label, &m, &port, live);
    calls
}

#[test]
fn object_shadow_list_matches_the_lifted_original() {
    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            // Fail each gate separately; then run empty, one, several and full lists.
            for (case, count) in [0, 0, 0, 1, 7, 32].into_iter().enumerate() {
                let mut r = Rng(seed ^ ((case as u64) << 32));
                let c = cpu(&mut r);
                let esp = c.gpr[4];
                let buf = esp - 0x280;
                let mut m = image.clone();
                put(&mut m, esp + 4, r.below(4));
                put(&mut m, SHADOW_OBJECTS, list_header(0));
                put8(&mut m, SHADOW_MODE, r.u32() as u8);
                put8(&mut m, SHADOW_LIST_MODE, r.u32() as u8);
                fill(&mut m, &mut r, buf, 0x280);
                let plan = object_shadows::plan(&Ram(m.clone()), esp).unwrap();
                let label = format!("shadow list seed {seed} case {case}");
                let calls = compare_shadow_body(
                    &mut lifted, m, c, SHADOW_LIST, buf, 4, SHADOW_LIST_STUBS,
                    move |cx| Box::pin(async move { object_shadows::run(&cx, plan).await }),
                    |k, call, m| {
                        let mut r = Rng(seed ^ (k as u64) << 32 ^ u64::from(call.site));
                        let log = SHADOW_LOG + 4 * k as u32;
                        let mode = m.bytes[SHADOW_MODE as usize];
                        let list_mode = m.bytes[SHADOW_LIST_MODE as usize];
                        put8(m, log, mode);
                        put8(m, log + 1, list_mode);
                        let eax = match call.target {
                            SHADOW_LIGHTMAP => (r.u32() & !0xff) | u32::from(case != 0),
                            SHADOW_PERMISSION => (r.u32() & !0xff) | u32::from(case != 1),
                            SHADOW_TARGET => {
                                // The list pointer and mode are fetched after target sizing.
                                put(m, SHADOW_OBJECTS, list_header(1));
                                put(m, esp + 4, r.u32());
                                r.u32()
                            }
                            SHADOW_ROWS => {
                                fill(m, &mut r, call.args[0], count * 0x14);
                                count
                            }
                            SHADOW_CASTER => {
                                // The next caster and fade must be loaded on its iteration.
                                if call.regs[4] > 1 {
                                    let next = call.regs[3] + 0x14;
                                    put(m, next, r.u32());
                                    put(m, next + 4, r.u32());
                                }
                                r.u32()
                            }
                            _ => unreachable!(),
                        };
                        (eax, r.u32())
                    },
                    &label,
                );
                assert_eq!(calls.iter().filter(|c| c.target == SHADOW_CASTER).count(), count as usize);
            }
        }
    }) {
        eprintln!("SKIPPED object_shadow_list_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

#[test]
fn object_shadow_rows_match_the_lifted_original() {
    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            for case in 0..20u32 {
                let mut r = Rng(seed ^ (u64::from(case) << 32));
                let mut w = World::new(image, &mut r, 4);
                let mut c = cpu(&mut r);
                let esp = c.gpr[4];
                let list = list_header(0);
                let mode = if case < 8 { 0 } else { case % 4 };
                let count = if case < 8 { 40 } else { [0, 24, 33, 11][(case % 4) as usize] };
                let handles: Vec<_> = (0..count).map(object_datum).collect();
                seed_list(&mut w.mem, &mut r, list, 48, &handles);
                let view = (word(&w.mem, VIEW_INDEX) as i32).clamp(0, 3) as u32;
                let hist = history::OBJECT_SHADOWS + view * history::STRIDE;
                let frame = word(&w.mem, FRAME);
                let m = &mut w.mem;
                fill(m, &mut r, hist, history::STRIDE);
                let past = if case < 8 { 0 } else { r.below(33) };
                put(m, hist + history::COUNT, past);
                for i in 0..past {
                    let a = hist + 0x24 * i;
                    put(m, a, r.u32() & 0xff00_0001);
                    put(m, a + 4, object_datum(r.below(OBJECT_SLOTS)));
                    put(m, a + 8, object_datum(r.below(OBJECT_SLOTS)));
                    for o in [0xc, 0x10, 0x14, 0x18] {
                        putf(m, a + o, r.float(0.0, 1.0));
                    }
                    put(m, a + 0x1c, frame.wrapping_sub(r.below(2)));
                    put(m, a + 0x20, frame.wrapping_sub(r.below(3)));
                }
                put(m, OVERRIDE, OVERRIDE_STRUCT);
                put8(m, OVERRIDE_STRUCT + 5, (case % 5 == 0) as u8);
                for i in 0..OBJECT_SLOTS {
                    for o in [0x30, 0x34, 0x38] {
                        putf(m, body(i) + o, r.float(-40.0, 40.0));
                    }
                    putf(m, body(i) + 0x3c, r.float(0.01, 12.0));
                }
                fill(m, &mut r, SCRATCH, 0x280);
                for i in 0..32 {
                    put8(m, SCRATCH + 0x14 * i + 3, (32 * case + i) as u8);
                }
                if case == 16 {
                    put16(m, list + 4, 0xffff);
                }
                c.gpr[3] = list;
                put(m, esp + 4, SCRATCH);
                put(m, esp + 8, mode);
                put(m, SHADOW_LOG, 0);
                let label = format!("shadow rows seed {seed} case {case}, mode {mode}");
                let plan = rows::plan(&Ram(m.clone()), esp, list, FPU)
                    .unwrap_or_else(|why| panic!("{label}: {why:?}"));
                let calls = compare_shadow_body(
                    &mut lifted, w.mem, c, SHADOW_ROWS, esp, 8, SHADOW_ROW_STUBS,
                    move |cx| Box::pin(async move { rows::run(&cx, plan).await.map(|(ret, _)| ret) }),
                    |k, call, m| {
                        let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(call.site));
                        let eax = if call.target == SHADOW_ELIGIBLE {
                            let i = call.args[0] & 0xffff;
                            putf(m, call.args[1], r.float(0.05, 1.0));
                            put8(m, call.args[2], r.u32() as u8);
                            putf(m, body(i) + 0x30, r.float(-50.0, 50.0));
                            if case == 4 && k == 0 {
                                // A current, fading history row appears during the walk,
                                // after preflight but before the guest enters the merge.
                                put(m, hist + history::COUNT, 1);
                                put(m, hist + 4, call.args[0]);
                                putf(m, hist + 0x14, 0.02);
                                putf(m, hist + 0x18, 0.03);
                                put(m, hist + 0x1c, frame);
                                put(m, hist + 0x20, frame.wrapping_sub(2));
                            }
                            if case >= 8 && k == 0 && count > 1 {
                                let objects = word(m, list + 0xc);
                                put(m, objects + 4, object_datum(OBJECT_SLOTS - 1));
                            }
                            (r.u32() & !0xff) | u32::from(case < 8 || !r.chance(4))
                        } else {
                            let n = word(m, SHADOW_LOG);
                            let a = SCRATCH + 0x14 * n;
                            // The owner's query sees the four scalar fields already published
                            // and the four flag bytes as they were on entry to this row.
                            m.bytes.copy_within(a as usize..(a + 0x14) as usize,
                                (SHADOW_LOG + 4 + 0x14 * n) as usize);
                            put(m, SHADOW_LOG, n + 1);
                            if case >= 8 {
                                put8(m, a + 3, r.u32() as u8);
                            }
                            (r.u32() & !0xff) | u32::from(r.chance(4))
                        };
                        (eax, r.u32())
                    },
                    &label,
                );
                if case < 8 {
                    // Still checks eligibility after filling the 32nd row, without
                    // asking about another owner or writing a 33rd candidate.
                    assert_eq!(calls.iter().filter(|c| c.target == SHADOW_ELIGIBLE).count(), 40);
                    assert_eq!(calls.iter().filter(|c| c.target == SHADOW_OWNER).count(), 32);
                }
            }
        }
    }) {
        eprintln!("SKIPPED object_shadow_rows_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

/// The mask caster's callees, including the layer hooks that read the named
/// return slot. The test supplies finite, synthetic coordinates to its basis
/// and radius calculations; all scalar coefficients are the image's own.
const SHADOW_CASTER_STUBS: Stubs = &[
    (SHADOW_ELIGIBLE, &[], 3, 12),
    (0x0003_d7c0, &[6, 7], 0, 0),    // chain visibility, also writes through EDI
    (0x0003_2ac0, &[3, 6, 7], 0, 0), // switch from mask to creature
    (0x0003_2ce0, &[0, 3, 6, 7], 1, 4), // draw creature, EAX object
    (0x0003_1f50, &[], 5, 20),        // extents
    (0x0003_0bf0, &[0], 0, 0),        // vector to normalize
    (0x000c_1930, &[0, 2], 0, 0),     // volume
    (0x000c_1a80, &[1, 2], 1, 4),     // pose
    (0x0003_20b0, &[6], 0, 0),        // transformed bounds
    (0x000c_1d90, &[], 4, 16),        // frusta
    (0x0013_20f0, &[0, 1, 2], 7, 28), // visibility
    (0x0016_5a20, &[7], 3, 12),       // insert caster
    (0x0002_cbc0, &[], 1, 4),         // records
    (0x0004_4720, &[], 0, 0),         // part visibility
    (0x0003_0ca0, &[0, 1], 0, 0),     // pass bit
    (passes::call::CONSTANTS, &[0, 1, 2], 4, 16),
    (passes::call::SHADOW_SETUP, &[1, 3], 1, 4),
    (passes::call::ITEMS, &[0], 0, 0),
    (passes::call::LAYER_BEGIN, &[7], 0, 0),
    (passes::call::LAYER_DRAW, &[], 0, 0),
    (passes::call::LAYER_END, &[], 0, 0),
    (0x0002_22a0, &[], 0, 0),         // filter between layers 10 and 11
    (passes::call::TARGET, &[0], 2, 8),
    (passes::call::CLIP_MODE, &[0], 0, 0),
    (passes::call::RECEIVER_BLEND, &[], 0, 0),
    (passes::call::STENCIL, &[3], 0, 0),
    (0x0004_a780, &[], 0, 0),         // restore
];

const SHADOW_LIGHTING: u32 = 0x0349_0000;
const SHADOW_AXES: u32 = 0x034a_0000;
/// The object lighting cache's datum array (bitmap at +0x100) and its
/// four 0x100-byte records.
const LIGHTING_RECORDS: u32 = 0x034b_0000;
const LIGHTING_DATA: u32 = 0x034c_0000;

/// An empty lighting cache, and a root that takes its lighting from it
/// (body +7 bit 0 clear). The caster's lookup `0x0003dc90` runs unstubbed
/// on both sides: the original's against the port's
/// (`engine::render::object_lighting`).
fn lighting_cache(m: &mut FlatMemory) {
    put(m, 0x0050_9434, LIGHTING_RECORDS);
    for (at, v) in [(0x20, 4), (0x24, 0x100), (0x34, 0), (0x38, 0), (0x3c, 0), (0x44, LIGHTING_DATA)] {
        put(m, LIGHTING_RECORDS + at, v);
    }
    put(m, LIGHTING_RECORDS + 0x48, LIGHTING_RECORDS + 0x100);
    put16(m, LIGHTING_RECORDS + 0x40, 0x8000);
    put(m, LIGHTING_RECORDS + 0x100, 0);
    put8(m, body(0) + 7, 0);
}

/// A root with no record: the lookup makes one (default light, three
/// permutation bytes, two change colours) and it is not lit yet.
fn new_lighting_record(m: &mut FlatMemory, r: &mut Rng) {
    lighting_cache(m);
    put(m, body(0) + 0xcc, NONE);
    put16(m, body(0) + 0x118, 30);
    put16(m, body(0) + 0x11a, 0x1c0);
    put16(m, body(0) + 0x124, 48);
    put16(m, body(0) + 0x126, 0x180);
    for i in 0..12 {
        putf(m, body(0) + 0x180 + 4 * i, r.float(0.0, 1.0));
    }
}

/// A root whose own record (record 2) is lit: the lookup gives its +0x54.
fn lit_lighting_record(m: &mut FlatMemory, r: &mut Rng) {
    lighting_cache(m);
    let record = LIGHTING_DATA + 0x200;
    put(m, body(0) + 0xcc, 0x8000_0002);
    put(m, record + 4, object_datum(0));
    put8(m, record + 3, 1);
    let sample = record + 0x54;
    putf(m, sample + 0x1c, r.float(0.9, 1.5));
    for i in 0..3 {
        putf(m, sample + 0xc + i * 4, r.float(-1.0, 1.0));
    }
}

fn shadow_caster_world(w: &mut World, r: &mut Rng, esp: u32, case: u32) {
    let m = &mut w.mem;
    fill(m, r, esp - 0xb50, 0xb50);
    put(m, esp + 4, object_datum(0));
    putf(m, esp + 8, r.float(0.75, 1.25));
    put(m, body(0), definition_datum(0));
    put16(m, definition(0) + 0x18, 2);
    put(m, body(0) + 0x14, NONE);
    put8(m, OBJECT_DATA + 2, 0);
    put8(m, OBJECT_DATA + 3, 0);
    put8(m, body(0) + 4, 0);
    put8(m, body(0) + 6, 0);
    put8(m, body(0) + 7, 1);
    put8(m, body(0) + 0xaa, (case % 2) as u8);
    put8(m, SHADOWS_ON, 1);
    put8(m, SHADOW_LIST_MODE, 1);
    put(m, 0x004d_e2f4, SHADOW_LIGHTING);
    let sample = SHADOW_LIGHTING + 0x1c;
    putf(m, sample + 0x1c, r.float(0.9, 1.5));
    putf(m, body(0) + 0x3c, r.float(0.1, 2.0));
    put16(m, body(0) + 0x2c, r.u32() as u16);
    for i in 0..3 {
        putf(m, body(0) + 0x30 + i * 4, r.float(-20.0, 20.0));
        putf(m, sample + 0xc + i * 4, r.float(-1.0, 1.0));
    }
    put(m, 0x0046_87ac, SHADOW_AXES);
    put(m, 0x0046_87b4, SHADOW_AXES + 12);
    for i in 0..6 {
        putf(m, SHADOW_AXES + 4 * i, r.float(-1.0, 1.0));
    }
    put(m, 0x0054_aa74, list_header(0));
    put16(m, list_header(0) + 4, 1);
    put(m, PASS_LIGHT_MASK, 0);
    put(m, PASS_CASTER_MASK, 0);
    put(m, PASS_VIEW_TARGET, r.u32());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskCase {
    NoClusters,
    NoCasters,
    NoReceivers,
    DeclineLayer,
    BothLayers,
    FilterClearsReceiver,
    StencilSkipsDraw,
    Ineligible,
    AttachedDuringEligibility,
    DisabledDuringEligibility,
    Hidden,
    NoSample,
    ZeroStrength,
    BelowThreshold,
    AtThreshold,
    AboveThreshold,
    OutsideBounds,
    LitRecord,
    EnterCreature,
    DeclineCreature,
    ContinueCreature,
    InactiveCreature,
}

#[test]
fn object_shadow_caster_matches_the_lifted_original() {
    use MaskCase::*;
    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            for case in [
                NoClusters, NoCasters, NoReceivers, DeclineLayer, BothLayers,
                FilterClearsReceiver, StencilSkipsDraw, Ineligible, AttachedDuringEligibility,
                DisabledDuringEligibility, Hidden, NoSample, ZeroStrength, BelowThreshold,
                AtThreshold, AboveThreshold, OutsideBounds, LitRecord, EnterCreature,
                DeclineCreature, ContinueCreature, InactiveCreature,
            ] {
                let mut r = Rng(seed ^ ((case as u64) << 32) ^ 0x2350);
                let mut w = World::new(image, &mut r, 4);
                let c = cpu(&mut r);
                let esp = c.gpr[4];
                shadow_caster_world(&mut w, &mut r, esp, case as u32);
                let creature = matches!(case, EnterCreature | DeclineCreature | ContinueCreature | InactiveCreature);
                if creature {
                    let mask = matches!(case, EnterCreature | DeclineCreature);
                    put8(&mut w.mem, SHADOW_LIST_MODE, u8::from(mask));
                    put8(&mut w.mem, SHADOW_LIST_MODE + 1, if case == InactiveCreature { 0 } else { 0xb3 });
                }
                let threshold_case = matches!(case, BelowThreshold | AtThreshold | AboveThreshold);
                if threshold_case {
                    putf(&mut w.mem, esp + 8, 1.0);
                }
                // The default lighting's strength, or the cache's records.
                let strength = SHADOW_LIGHTING + 0x1c + 0x1c;
                match case {
                    NoSample => new_lighting_record(&mut w.mem, &mut r),
                    LitRecord => lit_lighting_record(&mut w.mem, &mut r),
                    ZeroStrength => putf(&mut w.mem, strength, 0.0),
                    BelowThreshold | AtThreshold | AboveThreshold => {
                        let threshold = word(&w.mem, 0x0045_df88);
                        let at = match case {
                            BelowThreshold => threshold - 1,
                            AboveThreshold => threshold + 1,
                            _ => threshold,
                        };
                        put(&mut w.mem, strength, at);
                    }
                    _ => {}
                }
                let label = format!("shadow mask seed {seed} case {case:?}");
                let plan = object_shadows::caster::plan(&Ram(w.mem.clone()), esp, FPU)
                    .unwrap_or_else(|why| panic!("{label}: {why:?}"));
                let calls = compare_shadow_body(
                    &mut lifted, w.mem, c, SHADOW_CASTER, esp, 8, SHADOW_CASTER_STUBS,
                    move |cx| Box::pin(async move { object_shadows::caster::run(&cx, plan).await }),
                    |k, c, m| {
                        let mut r = Rng(seed ^ ((k as u64) << 32) ^ u64::from(c.site));
                        let mut eax = r.u32();
                        match c.target {
                            SHADOW_ELIGIBLE => {
                                putf(m, c.args[1], if threshold_case { 1.0 } else { r.float(0.8, 1.1) });
                                if case == AttachedDuringEligibility {
                                    put(m, body(0) + 0x14, object_datum(1));
                                } else if case == DisabledDuringEligibility {
                                    put8(m, SHADOWS_ON, 0);
                                }
                                let kind = if creature { r.u32() as u8 | 1 } else { 0 };
                                put8(m, c.args[2], kind);
                                eax = (eax & !0xff) | u32::from(case != Ineligible);
                            }
                            0x0003_d7c0 => {
                                put8(m, c.regs[1], r.u32() as u8);
                                eax = (eax & !0xff) | u32::from(case != Hidden);
                            }
                            0x0003_2ac0 => {
                                assert_eq!(m.bytes[SHADOW_LIST_MODE as usize], 1);
                                eax = (eax & !0xff) | u32::from(case == EnterCreature);
                            }
                            0x0003_2ce0 => {
                                let mode = SHADOW_LIST_MODE as usize;
                                let active = if case == EnterCreature { 1 } else { 0xb3 };
                                assert_eq!(&m.bytes[mode..mode + 2], &[0, active]);
                                // The low byte of the final answer is reloaded after the draw.
                                put8(m, esp - 0xb50 + 0x13, 0x91);
                            }
                            0x0003_1f50 => {
                                putf(m, c.args[3], r.float(1.0, 4.0));
                                putf(m, c.args[4], r.float(0.5, 6.0));
                            }
                            0x0003_0bf0 => {
                                let v = c.regs[0];
                                let log = SHADOW_LOG + if c.site == 0x0003_24b6 { 0x40 } else { 0x4c };
                                m.bytes.copy_within(v as usize..(v + 12) as usize, log as usize);
                                for i in 0..3 {
                                    putf(m, v + i * 4, r.float(-1.0, 1.0));
                                }
                            }
                            0x000c_1930 => fill(m, &mut r, c.regs[1], 0x7c),
                            0x000c_1a80 => {
                                putf(m, c.regs[1] + 0x58, r.float(0.5, 2.0));
                                putf(m, c.regs[1] + 0x5c, r.float(0.2, 3.0));
                            }
                            0x0003_20b0 => {
                                // Capture the stack matrix and all three half extents.
                                let bounds = c.regs[0] as usize;
                                m.bytes.copy_within(bounds..bounds + 0x40, SHADOW_LOG as usize);
                                eax = (eax & !0xff) | u32::from(case != OutsideBounds);
                            }
                            0x000c_1d90 => put16(m, c.args[3], 1 + r.below(5) as u16),
                            0x0004_4720 => put16(m, list_header(0) + 4, u16::from(case != NoClusters)),
                            passes::call::ITEMS => {
                                put(m, PASS_CASTER_MASK, u32::from(case != NoCasters) << 10);
                                put(m, PASS_LIGHT_MASK, u32::from(case != NoReceivers) << 11);
                            }
                            passes::call::LAYER_BEGIN => eax = (eax & !0xff) | u32::from(case != DeclineLayer),
                            0x0002_22a0 if case == FilterClearsReceiver => put(m, PASS_LIGHT_MASK, 0),
                            passes::call::STENCIL if case == StencilSkipsDraw => put8(m, esp - 0xb50 + 0x13, 0),
                            _ => {}
                        }
                        (eax, r.u32())
                    },
                    &label,
                );
                if creature {
                    let sites: &[u32] = match case {
                        EnterCreature => &[0x0003_2394, 0x0003_23d5, 0x0003_2a3c, 0x0003_2a5a],
                        DeclineCreature => &[0x0003_2394, 0x0003_23d5, 0x0003_2a3c],
                        ContinueCreature => &[0x0003_2394, 0x0003_23d5, 0x0003_2a98],
                        InactiveCreature => &[0x0003_2394, 0x0003_23d5],
                        _ => unreachable!(),
                    };
                    assert_eq!(calls.iter().map(|c| c.site).collect::<Vec<_>>(), sites, "{label}");
                    continue;
                }
                // The original and the body must stop at the same predicate even
                // when it changes a gate that was satisfied during preflight.
                let stopped_at = match case {
                    Ineligible | AttachedDuringEligibility | DisabledDuringEligibility => Some(SHADOW_ELIGIBLE),
                    // The lighting lookup is not a stub: these end after
                    // the visibility call.
                    Hidden | NoSample | ZeroStrength | BelowThreshold | AtThreshold => Some(0x0003_d7c0),
                    OutsideBounds => Some(0x0003_20b0),
                    _ => None,
                };
                if let Some(last) = stopped_at {
                    assert_eq!(calls.last().map(|c| c.target), Some(last), "{label}");
                    continue;
                }
                let visits = |target| calls.iter().filter(|c| c.target == target).count();
                assert_eq!(visits(passes::call::ITEMS), usize::from(case != NoClusters), "{label}");
                let draws = match case {
                    DeclineLayer | FilterClearsReceiver | StencilSkipsDraw => 1,
                    BothLayers | AboveThreshold | LitRecord => 2,
                    _ => 0,
                };
                assert_eq!(visits(0x0002_22a0), usize::from(draws != 0), "{label}");
                assert_eq!(visits(passes::call::LAYER_DRAW), draws, "{label}");
            }
        }
    }) {
        eprintln!("SKIPPED object_shadow_caster_matches_the_lifted_original: no XBE (set H2_XBE)");
    }
}

/// Planning uses the same root, sample and view as the mask differential.
/// A refusal returns control to the original with every byte intact,
/// including the entry's scratch frame. These inputs need no retail image.
#[test]
fn object_shadow_caster_refuses_before_writing() {
    use gather::Refusal::Unsupported;
    let image = FlatMemory::new(0, RAM);
    for seed in seeds() {
        let mut r = Rng(seed);
        let mut w = World::new(&image, &mut r, 4);
        let esp = cpu(&mut r).gpr[4];
        shadow_caster_world(&mut w, &mut r, esp, 0);
        assert!(object_shadows::caster::plan(&Ram(w.mem.clone()), esp, FPU).is_ok());
        // A root with no lighting record is planned: the body makes it.
        let mut m = w.mem.clone();
        put8(&mut m, body(0) + 7, 0);
        put(&mut m, body(0) + 0xcc, NONE);
        assert!(object_shadows::caster::plan(&Ram(m), esp, FPU).is_ok(), "seed {seed}, missing record");
        type Edit = fn(&mut FlatMemory, &mut Fpu);
        let cases: &[(&str, gather::Refusal, Edit)] = &[
            ("disabled", Unsupported, |m, _| put8(m, SHADOWS_ON, 0)),
            ("attached", Unsupported, |m, _| put(m, body(0) + 0x14, object_datum(1))),
            ("creature", Unsupported, |m, _| put8(m, body(0) + 0xaa, 12)),
            ("x87 precision", Unsupported, |_, fpu| fpu.x87_control = 0x037f),
            ("SSE rounding", Unsupported, |_, fpu| fpu.mxcsr |= 1 << 13),
        ];
        for &(case, why, edit) in cases {
            let mut m = w.mem.clone();
            let mut fpu = FPU;
            edit(&mut m, &mut fpu);
            let ram = Ram(m.clone());
            assert_eq!(object_shadows::caster::plan(&ram, esp, fpu).err(), Some(why), "seed {seed}, {case}");
            assert!(m.bytes == ram.0.bytes, "seed {seed}, {case}: refusal modified memory");
        }
    }
}

/// Includes the CRT inverse sine and both x87 tangent calculations in the
/// lifted tree. EDI points to seeded words so the offscreen early return
/// must leave the caller's entire output intact.
#[test]
fn screen_bounds_match_the_lifted_original() {
    use engine::function_eval::constant::NO_REPORT;
    use engine::light_constants::{BoundsQuery, SCREEN_BOUNDS, screen_bounds};

    let found = with_image(|image, mut lifted| {
        let centre = SCRATCH;
        let view = centre + 0x40;
        let out = view + 0x40;
        for seed in seeds() {
            let mut rng = Rng(seed);
            for n in 0..72 {
                let mut original = image.clone();
                let mut c = cpu(&mut rng);
                let shape = if n < 48 { n % 8 } else { 7 };
                let mode = n / 8 % 6;
                let (coords, radius): ([f32; 3], f32) = match shape {
                    0 => ([0.0, 0.0, 4.0], 1.0),    // behind the view
                    1 => ([0.0; 3], 4.0),           // eye inside the sphere
                    2 => ([6.0, -6.0, 0.25], 1.0), // angular clamps
                    3 => ([0.0, 0.0, -1.0], -1.0),  // asin(-1)
                    4 => ([-0.0, 1.0, -5.0], -0.0),
                    5 => ([f32::NAN, 0.0, -5.0], 0.75),
                    // The SSE depth square underflows before the x87 sqrt.
                    6 => ([0.0, 0.0, -f32::MIN_POSITIVE], -f32::MIN_POSITIVE),
                    _ => (
                        [rng.float(-4.0, 4.0), rng.float(-4.0, 4.0), rng.float(-9.0, -1.0)],
                        rng.float(0.1, 2.0),
                    ),
                };
                // Exercise both the domain-error refusal and the CRT mode
                // that suppresses reporting and returns the indefinite.
                let reporting = mode % 2 == 0;
                put(&mut original, NO_REPORT, u32::from(!reporting));
                for (i, v) in coords.into_iter().enumerate() {
                    putf(&mut original, centre + 4 * i as u32, v);
                }
                let random_view = shape == 7;
                let scale = if random_view { rng.float(0.25, 2.5) } else { 1.0 };
                putf(&mut original, view + 4, scale);
                for i in 0..12 {
                    let v = if random_view {
                        rng.float(-1.0, 1.0)
                    } else if [0, 4, 8].contains(&i) {
                        1.0
                    } else {
                        0.0
                    };
                    putf(&mut original, view + 8 + i * 4, v);
                }
                for i in 0..6 {
                    put(&mut original, out + i * 4, rng.u32());
                }
                let fpu = Fpu {
                    mxcsr: if shape == 6 {
                        0x1f80
                    } else {
                        [0x1f80, 0x3f80, 0x5f80, 0x7f80, 0x9f80, 0x1f80][mode]
                    },
                    x87_control: [0x027f, 0x037f, 0x007f][mode % 3],
                };
                let query = BoundsQuery { centre, view, word: radius.to_bits() };
                let mut port = Ram(original.clone());
                let result = screen_bounds(&port, query, fpu);
                if reporting && matches!(shape, 5 | 6) {
                    assert_eq!(result, Err(engine::light_constants::Error::Data));
                    continue;
                }
                let label = format!("screen bounds seed {seed} case {n}");
                let bounds = result.unwrap_or_else(|e| panic!("{label}: {e:?}"));
                if let Some(words) = bounds {
                    for (i, v) in words.into_iter().enumerate() {
                        put(&mut port.0, out + 4 * i as u32, v);
                    }
                }
                c.gpr[0] = centre;
                c.gpr[1] = view;
                c.gpr[7] = out;
                c.mxcsr = fpu.mxcsr;
                c.x87.control = fpu.x87_control;
                if n % 2 != 0 {
                    c.x87.status |= x87::abi::H2_FP_PE;
                }
                let esp = c.gpr[4];
                lifted
                    .call(&mut c, &mut original, SCREEN_BOUNDS, esp, &[query.word])
                    .unwrap_or_else(|e| panic!("{label}: {e}"));
                assert_eq!(c.gpr[0] as u8, u8::from(bounds.is_some()), "{label}: AL");
                assert_eq!(c.gpr[4], esp, "{label}: ret 4");
                compare(&label, &original, &port.0, esp);
            }
        }
    });
    if !found {
        eprintln!("SKIPPED screen_bounds_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

#[test]
fn empty_additional_casters_match_the_lifted_original() {
    use engine::render::light_gather::passes::{additional, call};

    let found = with_image(|image, mut lifted| {
        let handles = VIS_LISTS + 0x100;
        let flags = VIS_LISTS + 0x200;
        for seed in seeds() {
            let mut rng = Rng(seed);
            for count in [-1, 0, 1, 6, 16] {
                let mut m = image.clone();
                let mut c = cpu(&mut rng);
                let sp = c.gpr[4];
                let cursor = rng.u32();
                put(&mut m, 0x004b_a134, count as u32);
                put(&mut m, 0x004c_19ac, cursor);
                put(&mut m, 0x004c_0b80, LIGHT_VIS);
                put(&mut m, LIGHT_VIS + 0x14, VIS_LISTS);
                put(&mut m, VIS_LISTS, 8);
                put16(&mut m, VIS_LISTS + 4, 4);
                put(&mut m, VIS_LISTS + 8, flags);
                put(&mut m, VIS_LISTS + 0xc, handles);
                let matching = rng.u32();
                for i in 0..8 {
                    // Two live copies of the matching handle. Only its
                    // second row and the slots after the count are marked.
                    let handle = if i < 2 { matching } else { matching ^ i };
                    put(&mut m, handles + 4 * i, handle);
                    let bits = rng.u32() as u16 & !0x1000;
                    put16(&mut m, flags + 2 * i, bits | if i == 1 || i >= 4 { 0x1000 } else { 0 });
                }
                for i in 0..count.max(0) as u32 {
                    let handle = if i % 3 == 0 { matching } else { matching ^ (4 + i) };
                    put(&mut m, 0x004b_a074 + 12 * i, handle);
                }
                let mut port = Ram(m.clone());
                let plan = additional::empty(&port).unwrap();
                plan.write(&mut port).unwrap();
                lifted.call(&mut c, &mut m, call::ADDITIONAL_CASTERS, sp, &[]).unwrap();
                assert_eq!(c.gpr[0], plan.eax(), "seed {seed}, count {count}");
                assert_eq!(c.gpr[4], sp);
                compare(&format!("empty additional casters seed {seed} count {count}"), &m, &port.0, sp);
            }
        }
    });
    if !found {
        eprintln!("SKIPPED empty_additional_casters_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

// The game's layers, their shader reset and the final target/viewport wrapper
// run lifted. The already-admitted layer build returns zero, as the native
// whole-group hook does. Device calls are the HLE boundary; the device's own
// tail tests compare its push buffer and the next draw.
const TYPED_TAIL_STUBS: Stubs = &[
    (passes::call::ITEMS, &[], 0, 0),
    (passes::call::CONSTANTS, &[], 4, 16),
    (0x0003_bab0, &[], 4, 16),        // whole sorted group
    (0x003f_7b30, &[3], 1, 4),        // SetRenderTargetSameSize: EBX = depth
    (0x003f_7870, &[], 2, 8),         // SetRenderTarget
    (0x003f_8100, &[], 1, 4),         // SetViewport
    (0x003f_5cf0, &[1, 2], 0, 0),     // SetRenderState_Simple
    (0x003f_5b20, &[], 1, 4),         // SampleAlpha
    (passes::call::STENCIL_ENABLE, &[], 1, 4),
    (passes::call::SCISSORS, &[], 3, 12),
    (0x003f_6190, &[0], 0, 0),        // viewport fragment's output put
];

const TAIL_WINDOW: u32 = 0x0048_5648;
const TAIL_CACHE: u32 = 0x0048_5b48;
const TAIL_IMPLEMENTATION: u32 = 0x034c_0000;
const TAIL_STATE: u32 = 0x034d_0000;
const TAIL_RESTORE: u32 = 0x034e_0000;

fn typed_tail_world(m: &mut FlatMemory, r: &mut Rng, c: &mut Cpu) {
    let esp = c.gpr[4];
    (c.gpr[0], c.gpr[1]) = pass_world(m, r, esp);
    // Nonzero definition kind keeps the colour written by the constants.
    // A kind-zero context and a bright colour B reach layers 11, 14, 12, 13.
    put(m, PASS_DATA, 0);
    put16(m, PASS_DATA + 4, 1);
    put16(m, PASS_CONTEXT, 0);
    let above = word(m, 0x0045_dc7c) + 1;
    put(m, PASS_COLOUR_B, above);
    put(m, OVERRIDE, 0);
    put(m, esp, HARNESS_EXIT);
    put(m, esp + 0xc, r.u32() & !0xff);
    put(m, PASS_VIEW_TARGET, 0);
    let colour = 0x034b_0000;
    let depth = 0x034b_0100;
    for (a, v) in [(0x0050_935c, colour), (0x0050_9364, depth),
                   (0x0051_f3f4, colour), (0x0051_f3f8, depth)] {
        put(m, a, v);
    }
    put16(m, 0x0048_5602, 0);
    put8(m, 0x0048_5607, 0);
    for (i, v) in [5, 17, 85, 181].into_iter().enumerate() {
        put16(m, TAIL_WINDOW + i as u32 * 2, v);
    }
    // In this stubbed-device comparison the depth-range wrapper writes
    // already-current clip/flags/put; it still calls its fragment.
    put(m, FP_DEVICE_POINTER, FP_DEVICE);
    put(m, FP_DEVICE, 0x034b_1000);
    put(m, FP_DEVICE + 4, 0x034b_2000);
    put(m, FP_DEVICE + 0x1c18, r.u32() | 2);
    for i in [0, 4] {
        let value = r.u32();
        put(m, FP_RANGE + i, value);
        put(m, FP_DEVICE + 0x1c10 + i, value);
    }
    let unit = word(m, 0x0045_dbc0);
    put(m, 0x004b_8494, unit);
    fill(m, r, TAIL_CACHE, 0x1400);
    put(m, TAIL_CACHE + 0x24, TAIL_IMPLEMENTATION);
    put(m, TAIL_CACHE + 0x20, TAIL_STATE);
    put(m, TAIL_STATE + 0x14, TAIL_RESTORE);
    put16(m, TAIL_IMPLEMENTATION + 0x11c, 3); // zero count, nonzero first
    fill(m, r, FP_LAYER_WORDS, 12);
    put(m, 0x0046_7010, r.u32());
    put(m, FP_TARGETS, r.u32());
    put(m, FP_STENCIL_STATE, r.u32());
    put(m, 0x004b_8348, r.u32());
    put(m, 0x004b_8474, r.u32());
    put(m, 0x0040_75e4, r.u32());
}

/// Run the full root with the same device contract each time. Check every
/// viewport's six arguments while its pointer still addresses live stack.
fn lifted_typed_tail(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    c: &Cpu,
    mut observe: impl FnMut(&Callout, &FlatMemory),
) -> (Vec<Callout>, u32) {
    let esp = c.gpr[4];
    let (calls, eax, after) = original_stubbed(
        lifted, m, &mut c.clone(), passes_entry(), esp, TYPED_TAIL_STUBS, true,
        |_, call, m| {
            observe(call, m);
            match call.target {
                0x003f_8100 => {
                    let p = call.args[0];
                    let viewport: [u32; 6] = std::array::from_fn(|i| word(m, p + 4 * i as u32));
                    assert_eq!(viewport, [17, 5, 164, 80, 0, 1.0f32.to_bits()]);
                    (0, 0)
                }
                0x003f_6190 => (call.regs[0], 0),
                _ => (0, 0),
            }
        },
    ).expect("lifted diffuse/specular light tail");
    assert_eq!(after, esp + 0x14);
    (calls, eax)
}

#[test]
fn typed_light_tail_matches_the_lifted_layer_wrappers() {
    use engine::render::light_gather::typed_tail::plan_dynamic;

    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            let mut r = Rng(seed);
            let mut c = cpu(&mut r);
            let esp = c.gpr[4];
            let mut original = image.clone();
            typed_tail_world(&mut original, &mut r, &mut c);
            let mut typed = Ram(original.clone());
            let pass = passes::plan(&typed, esp, c.gpr[0], c.gpr[1], FPU).unwrap();
            let flags = engine::light_passes::flag::CONTEXT_ZERO | engine::light_passes::flag::SPECULAR;
            for draws in [0, 1 << 12 | 1 << 13, 1 << 12, 1 << 13] {
                put(&mut original, PASS_LIGHT_MASK, draws);
                put(&mut typed.0, PASS_LIGHT_MASK, draws);
                let tail = plan_dynamic(&typed, &pass, flags, draws).unwrap();
                tail.write(&mut typed).unwrap();
                let (calls, eax) = lifted_typed_tail(&mut lifted, &mut original, &c, |_, _| {});
                assert_eq!(eax, tail.eax);
                let count = |target| calls.iter().filter(|c| c.target == target).count();
                assert_eq!(count(0x0003_bab0), draws.count_ones() as usize);
                assert_eq!(count(0x003f_8100), draws.count_ones() as usize + 1);
                compare(&format!("typed tail seed {seed} mask {draws:#x}"), &original, &typed.0, esp);
            }

            // A nonempty previous restore stays on the original. Execute
            // the following light on both states: it must actually apply
            // the restored state and observe identical cache and memory.
            let slot = 0x60u32;
            let value = r.u32();
            for mem in [&mut original, &mut typed.0] {
                put(mem, PASS_LIGHT_MASK, 1 << 12);
                put(mem, TAIL_CACHE + 0x24, TAIL_IMPLEMENTATION);
                put(mem, TAIL_CACHE + 0x20, TAIL_STATE);
                put16(mem, TAIL_IMPLEMENTATION + 0x11c, 1 << 9);
                put8(mem, TAIL_RESTORE, slot as u8);
                put(mem, TAIL_RESTORE + 1, value);
                put(mem, 0x0040_7498 + 4 * slot, !value);
            }
            assert!(plan_dynamic(&typed, &pass, flags, 1 << 12).is_none());
            let (a, ra) = lifted_typed_tail(&mut lifted, &mut original, &c, |_, _| {});
            let (b, rb) = lifted_typed_tail(&mut lifted, &mut typed.0, &c, |_, _| {});
            assert_transcripts("the following light", &a, &b);
            assert_eq!(ra, rb);
            assert_eq!(word(&typed.0, 0x0040_7498 + 4 * slot), value);
            compare("the following light", &original, &typed.0, esp);
        }
    }) {
        eprintln!("SKIPPED typed_light_tail_matches_the_lifted_layer_wrappers: no XBE (set H2_XBE)");
    }
}


/// The mask can retain layer 13 even when the current light never reaches
/// it. The guest then leaves layer 12's draw bit in the argument byte.
#[test]
fn typed_tail_uses_the_last_reached_layer_for_the_returned_draw_byte() {
    use engine::light_passes::flag;
    use engine::render::light_gather::typed_tail::plan_dynamic;

    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            let mut rng = Rng(seed);
            for flags in [0, flag::CONTEXT_ZERO, flag::SPECULAR] {
                let mut c = cpu(&mut rng);
                let esp = c.gpr[4];
                let mut original = image.clone();
                typed_tail_world(&mut original, &mut rng, &mut c);
                put16(&mut original, PASS_CONTEXT, u16::from(flags & flag::CONTEXT_ZERO == 0));
                let threshold = word(&original, engine::light_passes::constant::SPECULAR);
                for i in 0..3 {
                    put(&mut original, PASS_COLOUR_B + 4 * i, threshold);
                }
                if flags & flag::SPECULAR != 0 {
                    putf(&mut original, PASS_COLOUR_B + 8, f32::INFINITY);
                }
                let upper = rng.u32() & !0xff;
                let mut typed = Ram(original.clone());
                let plan = passes::plan(&typed, esp, c.gpr[0], c.gpr[1], FPU).unwrap();
                for diffuse in [true, false] {
                    let draws = u32::from(diffuse) << 12;
                    for mem in [&mut original, &mut typed.0] {
                        put(mem, PASS_LIGHT_MASK, draws | (1 << 13));
                        put(mem, esp + 0xc, upper);
                    }
                    // Enter with real saved registers and let the guest build
                    // the constants call frame. Only the list producer is stubbed.
                    let mut native = c.clone();
                    native.eip = passes_entry();
                    let mut gathers = 0;
                    lifted.run(&mut native, &mut typed.0, &[passes::call::CONSTANTS], |pc, cpu, m| {
                        if pc != passes::call::ITEMS {
                            return false;
                        }
                        gathers += 1;
                        cpu.eip = word(m, cpu.gpr[4]);
                        cpu.gpr[4] += 4;
                        cpu.gpr[0] = 0;
                        cpu.gpr[2] = 0;
                        true
                    }).unwrap();
                    assert_eq!(gathers, 1);
                    let call = passes::ConstantsCall::read(&typed, native.gpr[4]).unwrap().unwrap();
                    assert!(call.matches(&typed, &plan, [native.gpr[0], native.gpr[1], native.gpr[2]]).unwrap());
                    let tail = plan_dynamic(&typed, &plan, flags, draws).unwrap();
                    tail.write(&mut typed).unwrap();
                    native.gpr[0] = tail.eax;
                    native.gpr[3] = 0;
                    native.gpr[4] = call.view_target_esp();
                    native.eip = passes::site::VIEW_TARGET;
                    lifted.run(&mut native, &mut typed.0, &[HARNESS_EXIT], |_, _, _| false).unwrap();
                    assert_eq!(native.gpr[4], esp + 0x14);
                    for reg in [3, 5, 6, 7] {
                        assert_eq!(native.gpr[reg], c.gpr[reg], "callee-save register {reg}");
                    }
                    let (calls, eax) = lifted_typed_tail(&mut lifted, &mut original, &c, |_, _| {});
                    assert_eq!(eax, native.gpr[0]);
                    assert_eq!(calls.iter().filter(|c| c.target == 0x0003_bab0).count(), usize::from(diffuse));
                    assert_eq!(word(&original, esp + 0xc), upper | u32::from(diffuse));
                    compare(&format!("seed {seed}, flags {flags}, diffuse {diffuse}"), &original, &typed.0, esp);
                }
            }
        }
    }) {
        eprintln!("SKIPPED typed_tail_uses_the_last_reached_layer_for_the_returned_draw_byte: no XBE (set H2_XBE)");
    }
}

/// An opacity group uses the view depth but switches to target 1's embedded
/// colour surface. Exercise both device bind leaves, with the game wrappers
/// lifted all the way through the final restoration. At the device boundary
/// the same shader reset and pass tail as diffuse survive, plus the cache
/// flag from the rebind. A following view light sees a cache hit.
#[test]
fn opacity_light_keeps_the_lifted_target_switches() {
    use engine::light_passes::flag;
    use engine::render::light_gather::typed_tail::plan_dynamic;
    use engine::shadow_constants::global::{TARGETS, TARGET_STRIDE};

    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            let mut rng = Rng(seed);
            for same_size in [true, false] {
                let mut c = cpu(&mut rng);
                let esp = c.gpr[4];
                let mut original = image.clone();
                typed_tail_world(&mut original, &mut rng, &mut c);
                let view = word(&original, 0x0050_935c);
                let depth = word(&original, 0x0050_9364);
                let descriptor = TARGETS + TARGET_STRIDE;
                let opacity = descriptor + 0x18;
                put(&mut original, descriptor + 0x88, 0x034b_0800);
                put8(&mut original, descriptor + 0x95, 0);
                put(&mut original, descriptor + 0x78, 1);
                for off in [0, 0xc, 0x10] {
                    let header = rng.u32();
                    put(&mut original, view + off, header);
                    let matches = same_size || off != 0x10;
                    put(&mut original, opacity + off, if matches { header } else { !header });
                }
                put8(&mut original, 0x0051_f3fc, 0x7b);
                let flags = flag::CONTEXT_ZERO | flag::SPECULAR;
                let draws = (1 << 14) | (1 << 12) | if same_size { 0 } else { 1 << 13 };
                put(&mut original, PASS_LIGHT_MASK, draws);
                let mut expected = Ram(original.clone());
                let pass = passes::plan(&expected, esp, c.gpr[0], c.gpr[1], FPU).unwrap();
                assert!(plan_dynamic(&expected, &pass, flags, draws).is_none());

                // The lifted 14 and 12 wrappers both take the ordinary
                // shader reset/end. Their target switches are observable
                // separately at the D3D boundary.
                let tail = plan_dynamic(&expected, &pass, flags, draws & !(1 << 14)).unwrap();
                tail.write(&mut expected).unwrap();
                put8(&mut expected.0, 0x0051_f3fc, 1);

                let mut binds = Vec::new();
                let mut viewports = Vec::new();
                let mut layers = Vec::new();
                let (_, eax) = lifted_typed_tail(&mut lifted, &mut original, &c, |call, m| {
                    match call.target {
                        0x003f_7b30 => {
                            assert!(same_size);
                            assert_eq!(call.site, 0x0001_cd68);
                            assert_eq!(call.regs, [depth]);
                            binds.push(call.args[0]);
                        }
                        0x003f_7870 => {
                            assert!(!same_size);
                            assert_eq!(call.site, 0x0001_cd71);
                            assert_eq!(call.args[1], depth);
                            binds.push(call.args[0]);
                        }
                        0x003f_8100 => {
                            assert_eq!(word(m, 0x0051_f3f8), depth);
                            viewports.push((word(m, 0x0051_f3f4), word(m, 0x0051_f3fc) as u8));
                        }
                        0x0003_bab0 => layers.push(word(m, FP_LAYER_WORDS)),
                        _ => {}
                    }
                });
                assert_eq!(binds, [opacity, view]);
                let mut expected_layers = vec![14, 12];
                if draws & (1 << 13) != 0 {
                    expected_layers.push(13);
                }
                assert_eq!(layers, expected_layers);
                let mut expected_viewports = vec![(view, 1); layers.len() + 1];
                expected_viewports[0] = (opacity, 0);
                assert_eq!(viewports, expected_viewports);
                assert_eq!(eax, tail.eax);
                compare("opacity target restoration", &original, &expected.0, esp);

                for m in [&mut original, &mut expected.0] {
                    put(m, PASS_LIGHT_MASK, 1 << 12);
                }
                let next = plan_dynamic(&expected, &pass, flags, 1 << 12).unwrap();
                next.write(&mut expected).unwrap();
                let (calls, eax) = lifted_typed_tail(&mut lifted, &mut original, &c, |_, _| {});
                assert!(calls.iter().all(|c| !matches!(c.target, 0x003f_7b30 | 0x003f_7870)));
                assert_eq!(eax, next.eax);
                compare("view light after opacity", &original, &expected.0, esp);
            }
        }
    }) {
        eprintln!("SKIPPED opacity_light_keeps_the_lifted_target_switches: no XBE (set H2_XBE)");
    }
}

// ---------------------------------------------------------------------
// L8: the first-person loop's last slot, typed inside the guest's body.

/// Compare the return predicted at the constants call with the real loop
/// exit. Stop at the final view-target return to isolate the memory this
/// exit changes from the writes made by the layer and device callees.
#[test]
fn first_person_final_slot_returns_as_the_lifted_loop_exit() {
    if !with_image(|image, mut lifted| {
        for seed in seeds() {
            let mut rng = Rng(seed);
            for count in [1, 2, 8] {
                let mut world = World::new(image, &mut rng, 8);
                let mut cpu = cpu(&mut rng);
                let esp = cpu.gpr[4];
                fp_world(&mut world, &mut rng, esp);
                let mut m = world.mem;
                put(&mut m, esp, HARNESS_EXIT);
                put(&mut m, FIRST_PERSON_COUNT, count);
                for i in 0..count {
                    put16(&mut m, FIRST_PERSON_TABLE + 0x408 * i + 0x404, 1);
                }
                let plan = first_person::plan(&Ram(m.clone()), esp, FPU).unwrap();
                let saved = cpu.gpr;
                let mut slot_index = 0;
                let mut final_slot = None;
                let mut tail = None;
                let mut calls = 0;
                cpu.eip = first_person_entry();
                lifted.run(&mut cpu, &mut m, &[HARNESS_EXIT], |pc, cpu, m| {
                    if pc == first_person::site::VIEW_TARGET && final_slot.is_some() {
                        tail = Some(m.clone());
                    }
                    let Some((regs, args, pop)) = stub_in(FP_STUBS, pc) else {
                        return false;
                    };
                    let sp = cpu.gpr[4];
                    let site = word(m, sp);
                    if pc == first_person::call::CONSTANTS {
                        let at = first_person::AtConstants {
                            esp: sp, eax: cpu.gpr[0], ecx: cpu.gpr[1], edx: cpu.gpr[2],
                            ebx: cpu.gpr[3], esi: cpu.gpr[6], edi: cpu.gpr[7],
                        };
                        let result = first_person::final_slot(&Ram(m.clone()), &plan, &at);
                        if slot_index + 1 == count {
                            final_slot = Some(result.unwrap());
                        } else {
                            assert_eq!(result.unwrap_err(), gather::Refusal::Unsupported);
                        }
                        slot_index += 1;
                    }
                    let call = Callout {
                        target: pc, site,
                        regs: regs.iter().map(|&i| cpu.gpr[i]).collect(),
                        args: (0..args).map(|i| word(m, sp + 4 + 4 * i)).collect(),
                    };
                    let (eax, edx) = fp_effect(seed, calls, &call, m, plan.draw_byte());
                    if pc == first_person::call::RECORDS {
                        put(m, FIRST_PERSON_COUNT, count);
                    }
                    calls += 1;
                    cpu.gpr[0] = eax;
                    cpu.gpr[2] = edx;
                    cpu.gpr[4] = sp + 4 + pop;
                    cpu.eip = site;
                    true
                }).unwrap();
                let label = format!("first-person exit seed {seed}, {count} slots");
                assert_eq!(slot_index, count, "{label}");
                let slot = final_slot.unwrap();
                assert_eq!(slot.ret.eax, cpu.gpr[0], "{label}: EAX");
                assert_eq!(slot.exit_esp, cpu.gpr[4], "{label}: ESP");
                for reg in [3, 5, 6, 7] {
                    assert_eq!(saved[reg], cpu.gpr[reg], "{label}: saved register {reg}");
                }
                let mut tail = tail.unwrap();
                put8(&mut tail, FP_PASSES, 0);
                put(&mut tail, PASS_FLAGS, 0);
                compare(&label, &m, &tail, esp);
            }
        }
    }) {
        eprintln!("SKIPPED first_person_final_slot_returns_as_the_lifted_loop_exit: no XBE (set H2_XBE)");
    }
}

// ---------------------------------------------------------------------
// The first-person draw's items: 0x0003fdf0 up to its layer draws.

use engine::render::light_gather::first_person_items as fp_items;

const FIRST_PERSON_DRAW: u32 = 0x0003_fdf0;
/// Where the record loop ends, both when it ran and when it had nothing.
const FIRST_PERSON_LOOP_END: u32 = 0x0003_ff9c;
/// The first-person effects, stubbed (`ret 4`).
const FIRST_PERSON_EFFECTS: u32 = 0x0017_4220;
const ITEMS_ZERO: u32 = 0x033d_0000;
const SORTED_ENTRIES: u32 = 0x033e_0000;
const OBJECT_SECTIONS: u32 = 4;

/// Objects' first-person records over [`items_world`]: a render model
/// with sections, materials and node matrices, the first person's record
/// list, list 0 and the sorted list, and the view the sorted depth is
/// taken along.
fn first_person_items_world(w: &mut World, r: &mut Rng) {
    let (start, end) = items_world(w, r, 1);
    let m = &mut w.mem;
    // The render model: its flags, sections and materials.
    put8(m, OBJECT_TAGS + 4, if r.chance(2) { 4 } else { 0 });
    put(m, OBJECT_TAGS + 0x64, MATERIALS);
    for s in (0..OBJECT_SECTIONS).chain([0xff]) {
        let at = OBJECT_REGIONS + 0x5c * s;
        put(
            m,
            at + 0x34,
            if r.chance(8) {
                0
            } else {
                GEOMETRIES + 0x100 * r.below(GEOMETRY_COUNT)
            },
        );
        put16(
            m,
            at + 0x2c,
            if r.chance(4) { 0xffff } else { r.below(8) as u16 },
        );
    }
    // The node matrices and the remap the model's flag bit 2 reads.
    for k in 0..0x200 {
        putf(m, fp_items::NODE_MATRICES + 4 * k, r.float(-8.0, 8.0));
    }
    for k in 0..16 {
        put16(m, fp_items::NODE_MATRICES + 4 + 4 * k, r.below(8) as u16);
    }
    for k in 0..SHADERS {
        put8(m, SHADER_DATA + 0x100 * k + 0x16, r.below(4) as u8);
    }
    // The first person's records: objects' records of the light's range.
    let count = if end > start { r.below(12) } else { 0 };
    put(m, fp_items::RECORD_COUNT, count);
    for n in 0..count {
        let index = if r.chance(8) {
            0xffff
        } else {
            start + r.below(end - start)
        };
        put16(m, fp_items::RECORDS + 2 * n, index as u16);
        if index == 0xffff {
            continue;
        }
        let at = SCENE_RECORDS + 0x20 * index;
        put(
            m,
            at + 4,
            if r.chance(12) {
                NONE
            } else {
                (0xe130 << 16) | OBJECT_TAG
            },
        );
        put(
            m,
            at + 8,
            if r.chance(16) { NONE } else { 4 * r.below(0x40) },
        );
        let section = if r.chance(8) { 0xff } else { r.below(OBJECT_SECTIONS) };
        let packed = word(m, at + 0xc) & !(0x1ff << 9);
        put(m, at + 0xc, packed | (section << 9));
        put8(m, at + 0x1b, r.u32() as u8);
        put8(m, PIXEL_BYTES + index, r.u32() as u8);
    }
    // List 0, the sorted list and the view.
    let header = items::LISTS;
    let capacity = if r.chance(5) { r.below(3) } else { 4 + r.below(24) };
    put(m, header, capacity);
    put(m, header + 4, r.below(capacity + 1));
    put(m, header + 8, r.u32());
    put(m, header + 0xc, r.u32());
    put(m, header + 0x10, r.u32());
    put(m, header + 0x14, ITEMS_ZERO);
    fill(m, r, ITEMS_ZERO, 32 * 0x18);
    put(
        m,
        fp_items::SORTED_COUNT,
        if r.chance(6) { 0x3fe + r.below(3) } else { r.below(16) },
    );
    put(m, fp_items::SORTED_ENTRIES, SORTED_ENTRIES);
    fill(m, r, SORTED_ENTRIES, 0x400 * fp_items::SORTED_ENTRY + 0x100);
    for k in 0..3 {
        putf(m, fp_items::VIEW_POSITION + 4 * k, r.float(-4.0, 4.0));
        putf(m, fp_items::VIEW_FORWARD + 4 * k, r.float(-1.0, 1.0));
    }
}

/// `0x0003fdf0` lifted from its entry to the end of its record loop, the
/// first-person effects stubbed.
fn original_first_person_items(
    lifted: &mut Lifted<'_>,
    m: &mut FlatMemory,
    cpu: &mut Cpu,
    esp: u32,
    mode: u32,
    call: u32,
) -> Result<(), String> {
    put(m, esp, HARNESS_EXIT);
    put(m, esp + 4, call);
    cpu.gpr[4] = esp;
    cpu.gpr[0] = mode;
    cpu.eip = FIRST_PERSON_DRAW;
    lifted
        .run(cpu, m, &[FIRST_PERSON_LOOP_END, HARNESS_EXIT], |pc, cpu, m| {
            if pc != FIRST_PERSON_EFFECTS {
                return false;
            }
            let sp = cpu.gpr[4];
            cpu.eip = word(m, sp);
            cpu.gpr[4] = sp + 8;
            true
        })
        .and_then(|pc| match pc {
            FIRST_PERSON_LOOP_END => Ok(()),
            _ => Err(format!("left at {pc:#x}")),
        })
}

#[test]
fn first_person_items_match_the_lifted_original() {
    if !with_image(first_person_items_cases) {
        eprintln!("SKIPPED first_person_items_match_the_lifted_original: no XBE (set H2_XBE)");
    }
}

fn first_person_items_cases(image: &FlatMemory, mut lifted: Lifted<'_>) {
    let mut tally = Tally::default();
    for seed in seeds() {
        for n in 0..96u32 {
            let mut r = Rng(seed.wrapping_mul(0x5851_f42d_4c95_7f2d) ^ u64::from(n) ^ 0xfdf0);
            let mut w = World::new(image, &mut r, 4);
            first_person_items_world(&mut w, &mut r);
            let mode = r.below(3);
            let call = r.below(2);
            let label = format!("seed {seed} case {n} (mode {mode}, call {call})");
            let mut c = cpu(&mut r);
            let esp = c.gpr[4] - 8;
            // The return address and the argument the original is entered
            // with, in both runs: the port never writes them.
            put(&mut w.mem, esp, HARNESS_EXIT);
            put(&mut w.mem, esp + 4, call);

            let mut port = Ram(w.mem.clone());
            let made = match fp_items::make(&port, mode) {
                Ok(made) => made,
                Err(why) => {
                    tally.add(format!("refused {why:?}"));
                    continue;
                }
            };
            made.writes.apply(&mut port).unwrap();

            let mut a = w.mem;
            original_first_person_items(&mut lifted, &mut a, &mut c, esp, mode, call)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            compare(&label, &a, &port.0, esp);
            assert_eq!(made.layers, word(&a, items::LISTS + 8), "{label}: list 0's layers");
            tally.add("native");
            for part in &made.made {
                if part.item.is_some() {
                    tally.add("an item");
                }
                if part.sorted.is_some() {
                    tally.add("a sorted entry");
                }
                if part.item.is_none() && part.layers & 0xffff_7fff != 0 {
                    tally.add("a full list");
                }
            }
        }
    }
    eprintln!("first-person items: {:#?}", tally.0);
    assert!(tally.get("native") >= 96, "too few calls answered: {:#?}", tally.0);
    for k in ["an item", "a sorted entry", "a full list"] {
        assert!(tally.get(k) > 0, "never {k}: {:#?}", tally.0);
    }
}
