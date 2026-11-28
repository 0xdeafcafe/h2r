//! The start gate against the lifted original (docs/engine/sound-instances.md,
//! "The start layer"). Each case writes one random sound tag, its class and
//! gestalt rows, four listeners and local users, a BSP with a fine or a
//! coarse cluster table, a seed and a request record, runs the lifted
//! `0x126c30` (with every callee lifted: `0x126bd0`, `0x125010`,
//! `0x127d00`, `0x127e20`, `0x221da0`, `0x249d60`, `0x249ed0`, `0x12ac20`,
//! `0x124fc0`, `0x12aff0`, `0x221810`, `0x1251e0` and its levels) to its
//! return, and runs `gate::start_gate` on a second copy of the same memory.
//! The answer, the listener and reason words and the seed must agree.

use super::*;
use engine::points::{GLOBALS_TAG, PLAYER_GLOBALS, RANDOM_SEEDS, SOUND_GLOBALS, TAG_ENTRIES};
use engine::sim_sound::gate::{self, BSP, CLASS_SWITCH};
use engine::sim_sound::levels::{CLASS_STATES, GESTALT};
use sim_sound::instances::FPU_CONTROL;

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
const DEFAULTS: u32 = ARENA + 0x2400;
const ROWS: u32 = ARENA + 0x2600;
const SOUND_STATE: u32 = ARENA + 0x3000;
const USERS: u32 = ARENA + 0x3400;
const RNG: u32 = ARENA + 0x3500;
const BSP_AT: u32 = ARENA + 0x3600;
const FINE: u32 = ARENA + 0x3a00;
const FINE_TABLE: u32 = ARENA + 0x3b00;
const COARSE_TABLE: u32 = ARENA + 0x3c00;
const RECORD: u32 = ARENA + 0x3d00;
const OUT_LISTENER: u32 = ARENA + 0x3f00;
const OUT_REASON: u32 = ARENA + 0x3f04;
const UNSET: u32 = 0xdead_beef;
const CLASS_COUNT: u32 = 4;
const DEFAULT_ROWS: u32 = 3;
const SCALE_ROWS: u32 = 3;
const CLUSTERS: u32 = 6;

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
    fn float(&mut self, lo: f32, hi: f32, edges: &[f32]) -> u32 {
        if !edges.is_empty() && self.chance(5) {
            return edges[self.below(edges.len() as u32) as usize].to_bits();
        }
        let t = (self.next() & 0xffff) as f32 / 65536.0;
        (lo + (hi - lo) * t).to_bits()
    }
}

const DB: &[f32] = &[-100.0, -64.0, 0.0, -0.0, 6.0, f32::NAN];
const DISTANCE: &[f32] = &[0.0, 0.001, 0.0005, 1.0, 1.00001, f32::NAN];
const POSITION: &[f32] = &[0.0, -0.0, 5.0, f32::NAN];

fn guest(image: &Image, case: u32) -> Guest {
    let mut r = Rand(0x126c_30c0_de00_0000 ^ (u64::from(case) << 24 | u64::from(case)));
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
    let byte = |g: &mut Guest, at: u32, v: u32| g.set_u8(at, v as u8).unwrap();
    set(&mut g, TAG_ENTRIES.base, ENTRIES);
    set(&mut g, ENTRIES + (TAG & 0xffff) * 16 + 8, SOUND);
    set(&mut g, ENTRIES + 16 + 8, CLASSES_TAG);
    set(&mut g, CLASSES_TAG + 4, CLASSES);
    set(&mut g, GLOBALS_TAG.base, GAME_GLOBALS);
    set(&mut g, GAME_GLOBALS + 0xc0, 1);
    set(&mut g, GAME_GLOBALS + 0xc4, GAME_GLOBALS + 0x100);
    set(&mut g, GAME_GLOBALS + 0x104, 1);
    set(&mut g, GESTALT, GESTALT_AT);
    set(&mut g, GESTALT_AT + 4, DEFAULTS);
    set(&mut g, GESTALT_AT + 0xc, SCALES);
    set(&mut g, GESTALT_AT + 0x24, ROWS);
    set(&mut g, CLASS_STATES, STATES);
    set(&mut g, SOUND_GLOBALS.base, SOUND_STATE);
    set(&mut g, PLAYER_GLOBALS.base, USERS);
    set(&mut g, RANDOM_SEEDS.base, RNG);
    set(&mut g, BSP, BSP_AT);
    set(&mut g, CLASS_SWITCH, u32::from(r.chance(2)));
    set(&mut g, RNG + 4, r.next() ^ (r.next() << 16));

    // The tag: flags (distance bits 2, 3), class, the class gate bytes,
    // its default row, its gestalt row, `+0xa` and its scale row.
    byte(&mut g, SOUND + 1, r.next() & 0xc);
    byte(&mut g, SOUND + 2, r.below(CLASS_COUNT));
    byte(&mut g, SOUND + 4, [0, 1, 2, 2][r.below(4) as usize]);
    byte(&mut g, SOUND + 5, u32::from(!r.chance(4)));
    g.set_u16(SOUND + 6, r.below(DEFAULT_ROWS) as u16).unwrap();
    g.set_u16(SOUND + 8, r.below(4) as u16).unwrap();
    byte(&mut g, SOUND + 0xa, if r.chance(8) { 0 } else { r.next() | 1 });
    byte(&mut g, SOUND + 0xb, r.below(SCALE_ROWS));
    for i in 0..4 {
        g.set_u16(ROWS + i * 12 + 0xa, if r.chance(4) { 0 } else { r.next() as u16 | 1 }).unwrap();
    }
    for d in 0..DEFAULT_ROWS {
        let row = DEFAULTS + d * 0x38;
        set(&mut g, row, r.float(0.0, 8.0, DISTANCE));
        set(&mut g, row + 4, r.float(1.0, 40.0, DISTANCE));
        set(&mut g, row + 8, r.float(-0.2, 0.6, &[0.0, 1.0, f32::NAN]));
        set(&mut g, row + 0x10, r.float(-70.0, 6.0, DB));
    }
    for s in 0..SCALE_ROWS {
        let row = SCALES + s * 20;
        set(&mut g, row, r.float(-30.0, 6.0, DB));
        set(&mut g, row + 4, r.float(-30.0, 6.0, DB));
        set(&mut g, row + 0xc, r.float(0.0, 1.0, &[0.0, 1.0]));
        set(&mut g, row + 0x10, r.float(0.0, 1.0, &[0.0, 1.0]));
    }
    for c in 0..CLASS_COUNT {
        let row = CLASSES + c * 0x5c;
        set(&mut g, row + 0x18, r.float(0.0, 8.0, DISTANCE));
        set(&mut g, row + 0x1c, r.float(1.0, 40.0, DISTANCE));
        set(&mut g, row + 0x20, r.float(-40.0, 6.0, DB));
        set(&mut g, row + 0x24, r.float(-40.0, 6.0, DB));
        set(&mut g, STATES + c * 16 + 4, r.float(-12.0, 3.0, &[0.0]));
        byte(&mut g, STATES + c * 16 + 0xc, 1);
    }

    // Sound on, four listeners, four local users.
    for o in 0..3 {
        byte(&mut g, SOUND_STATE + 0x78 + o, u32::from(!r.chance(24)));
    }
    for n in 0..4 {
        let row = SOUND_STATE + 0x88 + n * 0x48;
        set(&mut g, row, r.below(3));
        g.set_u16(row + 4, if r.chance(6) { u16::MAX } else { r.below(CLUSTERS) as u16 }).unwrap();
        byte(&mut g, row + 6, u32::from(!r.chance(3)));
        for o in [0x30, 0x34, 0x38] {
            set(&mut g, row + o, r.float(-20.0, 20.0, POSITION));
        }
        set(&mut g, USERS + 0xc + 4 * n, if r.chance(2) { u32::MAX } else { r.next() & 0xffff });
    }

    // The BSP: a fine table (`+0x224`) or the coarse one, bytes with 0 and
    // 0xff among them.
    set(&mut g, BSP_AT + 0x9c, CLUSTERS);
    set(&mut g, BSP_AT + 0x224, u32::from(r.chance(2)));
    set(&mut g, BSP_AT + 0x228, FINE);
    set(&mut g, BSP_AT + 0xe8, COARSE_TABLE);
    set(&mut g, FINE + 0x28, FINE_TABLE);
    set(&mut g, FINE + 4, r.float(0.0, 2.0, &[]));
    set(&mut g, FINE + 8, r.float(2.0, 10.0, &[]));
    for i in 0..CLUSTERS * CLUSTERS {
        let v = match r.below(6) {
            0 => 0,
            1 => 0xff,
            _ => r.next(),
        };
        byte(&mut g, FINE_TABLE + i, v);
        byte(&mut g, COARSE_TABLE + i, r.next());
    }

    // The request: the source at `+0xc`.
    let source = RECORD + 0xc;
    byte(&mut g, source, r.next() & 0x0d);
    byte(&mut g, source + 1, r.next() & 0x09);
    byte(&mut g, source + 2, u32::from(r.chance(3)));
    byte(&mut g, source + 3, [0, 1, 1, 2, 0xf][r.below(5) as usize]);
    set(&mut g, source + 4, r.float(0.0, 1.0, &[0.0, 1.0, 2.0, f32::NAN]));
    set(&mut g, source + 8, r.float(-10.0, 6.0, &[0.0]));
    for o in [0xc, 0x10, 0x14, 0x30] {
        set(&mut g, source + o, r.float(-20.0, 20.0, POSITION));
    }
    set(&mut g, source + 0x28, r.below(3));
    g.set_u16(source + 0x2c, if r.chance(6) { u16::MAX } else { r.below(CLUSTERS) as u16 }).unwrap();
    set(&mut g, source + 0x34, r.float(0.0, 8.0, DISTANCE));
    set(&mut g, source + 0x38, r.float(1.0, 40.0, DISTANCE));
    set(&mut g, OUT_LISTENER, UNSET);
    set(&mut g, OUT_REASON, UNSET);
    g
}

fn run_lifted(code: &mut lifted::Original, g: &mut Guest, context: &str) -> u32 {
    let set = |g: &mut Guest, at: u32, v: u32| g.set_u32(at, v).unwrap();
    set(g, STACK, EXIT);
    set(g, STACK + 4, TAG);
    set(g, STACK + 8, OUT_LISTENER);
    set(g, STACK + 12, OUT_REASON);
    let mut cpu = Cpu {
        eip: 0x126c30,
        gpr: [RECORD, 0x321, 0x54, 0x765, STACK, 0x98, 0xba, 0xdc],
        eflags: 0x893,
        ..Cpu::default()
    };
    cpu.x87.control = FPU_CONTROL;
    for _ in 0..50_000 {
        if let Flow::Return { target } = lifted::code::step(code, &mut cpu, &mut Memory(g)).unwrap()
            && target == EXIT
        {
            assert_eq!(cpu.gpr[4], STACK + 16, "{context}: ret 0xc");
            return cpu.gpr[0] & 0xff;
        }
    }
    panic!("{context}: lifted 0x126c30 did not return");
}

#[test]
fn the_gate_gives_the_lifted_answer_listener_reason_and_seed() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    let mut seen = [0u32; 6];
    for case in 0..CASES {
        let context = format!("gate case {case}");
        let mut ours = guest(image, case);
        let gate = gate::start_gate(&mut ours, RECORD, TAG).unwrap_or_else(|e| panic!("{context}: {e:?}"));
        let mut lifted = guest(image, case);
        let open = run_lifted(&mut code, &mut lifted, &context);
        assert_eq!(u32::from(gate.refused.is_none()), open, "{context}: {gate:?}");
        assert_eq!(gate.listener.unwrap_or(UNSET), lifted.u32(OUT_LISTENER).unwrap(), "{context}: listener");
        assert_eq!(gate.refused.map_or(0, |r| r as u32), lifted.u32(OUT_REASON).unwrap(), "{context}: reason");
        assert_eq!(ours.u32(RNG + 4).unwrap(), lifted.u32(RNG + 4).unwrap(), "{context}: seed");
        seen[gate.refused.map_or(0, |r| r as usize)] += 1;
    }
    // Every answer: open (0), skipped (1), inaudible (4) and off (5).
    for reason in [0, 1, 4, 5] {
        assert!(seen[reason] > CASES / 50, "reason {reason}: {seen:?}");
    }
}
