//! A sound's level for its scale against the engine's (docs/engine/sound-instances.md,
//! "Levels"). Each case writes one random sound tag, class, scale row, class
//! state and duck state as guest records, runs the lifted `0x1251e0` (with
//! `0x218d70`, `0x218de0`, `0x2195f0`, `0x2197f0`, `0x127010` and
//! `0x221810`) to its return, and asks `GuestLevels` the same question of
//! the same memory. Both must give the same bits.

use super::*;
use engine::points::{GLOBALS_TAG, SOUND_GLOBALS, TAG_ENTRIES};
use engine::sim_sound::levels::{CLASS_STATES, GESTALT, GuestLevels};
use sim_sound::instances::{FPU_CONTROL, Levels, SourceParams, TagIndex};

const CASES: u32 = 8192;
const TAG: u32 = 0x1234_0005;
const ENTRIES: u32 = ARENA + 0x100;
const SOUND: u32 = ARENA + 0x400;
const GAME_GLOBALS: u32 = ARENA + 0x800;
const CLASSES_TAG: u32 = ARENA + 0x900;
const CLASSES: u32 = ARENA + 0x1000;
const GESTALT_AT: u32 = ARENA + 0x2000;
const SCALES: u32 = ARENA + 0x2100;
const STATES: u32 = ARENA + 0x2200;
const SOUND_STATE: u32 = ARENA + 0x3000;
const CLASS_COUNT: u32 = 6;
const SCALE_ROWS: u32 = 4;
const DUCK_ROWS: u32 = 4;

struct Rand(u64);

impl Rand {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
    fn chance(&mut self, in_n: u32) -> bool {
        self.below(in_n) == 0
    }
    /// Uniform in `[lo, hi)`, or now and then an edge the branches turn on.
    fn float(&mut self, lo: f32, hi: f32, edges: &[f32]) -> u32 {
        if !edges.is_empty() && self.chance(4) {
            return edges[self.below(edges.len() as u32) as usize].to_bits();
        }
        let t = (self.next() & 0xffff) as f32 / 65536.0;
        (lo + (hi - lo) * t).to_bits()
    }
}

const DB_EDGES: &[f32] = &[-100.0, -64.0, -63.999, -0.0, 0.0, 0.001, 6.0, f32::NAN, f32::NEG_INFINITY];
const SCALE_EDGES: &[f32] = &[0.0, 1.0, 0.5, -0.25, 1.5, f32::NAN];
const TIME_EDGES: &[f32] = &[0.0, 0.0005, 0.001, 1.0, 2.5, -0.5, f32::NAN];

fn guest(image: &Image, case: u32) -> (Guest, Cpu, SourceParams) {
    let mut r = Rand(0x51e0_c0de_0000_0000 ^ (u64::from(case) << 24 | u64::from(case)));
    for _ in 0..4 {
        r.next();
    }
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    let len = image.ram.bytes.len().min(RAM);
    g.write_bytes(0, &image.ram.bytes[..len], 1).unwrap();
    g.write_bytes(ARENA, &vec![0; 0x4000], 1).unwrap();
    g.write_bytes(STACK_BOTTOM, &[0xa5; 65536], 1).unwrap();
    let set = |g: &mut Guest, at: u32, v: u32| g.set_u32(at, v).unwrap();
    set(&mut g, TAG_ENTRIES.base, ENTRIES);
    set(&mut g, ENTRIES + (TAG & 0xffff) * 16 + 8, SOUND);
    set(&mut g, ENTRIES + 16 + 8, CLASSES_TAG);
    set(&mut g, CLASSES_TAG + 4, CLASSES);
    set(&mut g, GLOBALS_TAG.base, GAME_GLOBALS);
    set(&mut g, GAME_GLOBALS + 0xc0, 1);
    set(&mut g, GAME_GLOBALS + 0xc4, GAME_GLOBALS + 0x100);
    set(&mut g, GAME_GLOBALS + 0x104, 1);
    set(&mut g, GESTALT, GESTALT_AT);
    set(&mut g, GESTALT_AT + 0xc, SCALES);
    set(&mut g, CLASS_STATES, STATES);
    set(&mut g, SOUND_GLOBALS.base, SOUND_STATE);
    let class = r.below(CLASS_COUNT);
    g.set_u8(SOUND + 2, class as u8).unwrap();
    g.set_u8(SOUND + 0xb, r.below(SCALE_ROWS) as u8).unwrap();
    for c in 0..CLASS_COUNT {
        let row = CLASSES + c * 0x5c;
        let lo = r.float(-80.0, 10.0, DB_EDGES);
        set(&mut g, row + 0x20, lo);
        let hi = if r.chance(3) { lo } else { r.float(-80.0, 10.0, DB_EDGES) };
        set(&mut g, row + 0x24, hi);
        for d in 0..DUCK_ROWS {
            let at = row + 0x28 + d * 16;
            set(&mut g, at, r.float(-20.0, 2.0, &[0.0, -0.0, f32::NAN]));
            for o in [4, 8, 0xc] {
                set(&mut g, at + o, r.float(0.0, 3.0, TIME_EDGES));
            }
        }
        set(&mut g, STATES + c * 16 + 4, r.float(-12.0, 3.0, &[0.0, -0.0, f32::NAN]));
        g.set_u8(STATES + c * 16 + 0xc, if r.chance(4) { 1 } else { r.next() as u8 & !1 }).unwrap();
    }
    for s in 0..SCALE_ROWS {
        set(&mut g, SCALES + s * 20, r.float(-30.0, 6.0, DB_EDGES));
        set(&mut g, SCALES + s * 20 + 4, r.float(-30.0, 6.0, DB_EDGES));
    }
    let index = |r: &mut Rand| if r.chance(3) { u32::MAX } else { r.below(DUCK_ROWS) };
    let current = index(&mut r);
    let previous = if r.chance(2) { current } else { index(&mut r) };
    set(&mut g, SOUND_STATE + 0x1f8, current);
    set(&mut g, SOUND_STATE + 0x1fc, r.float(0.0, 8.0, TIME_EDGES));
    set(&mut g, SOUND_STATE + 0x200, previous);
    set(&mut g, SOUND_STATE + 0x204, r.float(0.0, 8.0, TIME_EDGES));
    let mut source = SourceParams::default();
    for w in &mut source.0 {
        *w = r.next();
    }
    source.0[1] = r.float(0.0, 1.0, SCALE_EDGES);
    source.0[2] = r.float(-10.0, 10.0, &[0.0, -0.0, f32::NAN]);
    set(&mut g, STACK, EXIT);
    set(&mut g, STACK + 4, source.0[2]);
    set(&mut g, STACK + 8, source.0[1]);
    let mut cpu = Cpu {
        eip: 0x1251e0,
        gpr: [0x19, 0x321, 0x54, 0x765, STACK, 0x98, SOUND, 0xdc],
        eflags: 0x893,
        ..Cpu::default()
    };
    cpu.x87.control = FPU_CONTROL;
    (g, cpu, source)
}

fn run_lifted(code: &mut lifted::Original, cpu: &mut Cpu, g: &mut Guest, context: &str) -> u32 {
    for _ in 0..20_000 {
        if let Flow::Return { target } = lifted::code::step(code, cpu, &mut Memory(g)).unwrap()
            && target == EXIT
        {
            assert_eq!(cpu.gpr[4], STACK + 12, "{context}: ret 8");
            return cpu.gpr[0];
        }
    }
    panic!("{context}: lifted 0x1251e0 did not return");
}

#[test]
fn guest_levels_give_the_lifted_level_bits() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    let mut finite = 0u32;
    for case in 0..CASES {
        let context = format!("level case {case}");
        let (mut g, mut cpu, source) = guest(image, case);
        let levels = GuestLevels::new(&g);
        let ours = levels.level(TagIndex(TAG), &source).to_bits();
        assert_eq!(levels.faults.get(), 0, "{context}: fault");
        let lifted = run_lifted(&mut code, &mut cpu, &mut g, &context);
        assert_eq!(ours, lifted, "{context}: {} vs lifted {}", f32::from_bits(ours), f32::from_bits(lifted));
        finite += u32::from(f32::from_bits(lifted).is_finite());
    }
    // The edges make some levels NaN; most cases still exercise the math.
    assert!(finite > CASES / 2, "{finite} of {CASES} finite");
}
