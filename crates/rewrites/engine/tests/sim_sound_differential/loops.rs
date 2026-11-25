//! Single-track rings with a salted head, one overlapping fade, missing
//! matches, and a callback that drops the next instance during a gain ramp.

use super::*;
use engine::points::{LOOPING_SOUNDS, SOUND_INSTANCES};

const LOOP_POOL: u32 = ARENA + 0x1000;
const VOICE_POOL: u32 = ARENA + 0x1100;
const LOOPS: u32 = ARENA + 0x2000;
const VOICES: u32 = ARENA + 0x4000;
const OWNER: u32 = 0xb128_0001;
const HEAD: u32 = 0x1bad_0002;
const SECOND: u32 = 0x3abe_0003;
const ADDED: u32 = 0x7cab_0004;
const NONE: u32 = u32::MAX;
const TAG: u32 = 0x8765_0006;

fn row(datum: u32) -> u32 {
    VOICES + (datum & 0xffff) * 0xbc
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    let head = if seed == 0 { NONE } else { HEAD };
    g.set_u32(LOOPING_SOUNDS.base, LOOP_POOL).unwrap();
    g.set_u32(SOUND_INSTANCES.base, VOICE_POOL).unwrap();
    g.set_u32(LOOP_POOL + 0x44, LOOPS).unwrap();
    g.set_u32(VOICE_POOL + 0x44, VOICES).unwrap();
    // Track 1 of loop 1.
    g.set_u32(LOOPS + 0xd4 + 0x88 + 0x14, head).unwrap();
    for (voice, next, prev, tag, pitch) in [
        (HEAD, SECOND, SECOND, TAG, 9u8),
        (SECOND, HEAD, HEAD, TAG, 0xfe),
    ] {
        let addr = row(voice);
        g.set_u32(addr + 0xc, tag).unwrap();
        g.set_u32(addr + 0x6c, next).unwrap();
        g.set_u32(addr + 0x70, prev).unwrap();
        g.set_u8(addr + 2, 2).unwrap();
        g.set_u8(addr + 0x9c, pitch).unwrap();
    }
    match c.eip {
        0x21b870 => {
            c.gpr[0] = OWNER;
            c.gpr[1] = 0xabc0_0001;
            c.gpr[3] = if seed == 4 { TAG + 1 } else { TAG };
            let pitch = match seed {
                1 => NONE,
                2 => (-2i32) as u32,
                _ => 17,
            };
            g.set_u32(STACK + 4, pitch).unwrap();
            if seed == 5 {
                g.set_u32(row(SECOND) + 0x6c, NONE).unwrap();
            }
        }
        0x21b910 => {
            c.gpr[0] = TAG;
            c.gpr[6] = 1;
            c.gpr[7] = 0xfedc;
            g.set_u32(STACK + 4, OWNER).unwrap();
            g.set_u32(STACK + 8, 0xbc34).unwrap();
            g.set_u32(STACK + 12, ARENA + 0x84).unwrap();
        }
        0x21bc80 => {
            c.gpr[0] = OWNER;
            c.gpr[1] = 0xabc0_0001;
            c.gpr[6] = ADDED;
            if seed == 1 {
                g.set_u32(row(HEAD) + 0x6c, HEAD).unwrap();
                g.set_u32(row(HEAD) + 0x70, HEAD).unwrap();
            }
        }
        0x21bd00 => {
            let entries = ARENA + 0x7000;
            let globals = ARENA + 0x7100;
            let refs = ARENA + 0x7300;
            let classes = ARENA + 0x7400;
            let rows = ARENA + 0x7600;
            let sound = ARENA + 0x8000;
            g.set_u32(engine::points::TAG_ENTRIES.base, entries)
                .unwrap();
            g.set_u32(engine::points::GLOBALS_TAG.base, globals)
                .unwrap();
            g.set_u32(entries + (TAG & 0xffff) * 16 + 8, sound).unwrap();
            g.set_u8(sound + 2, 2).unwrap();
            g.set_u32(globals + 0xc0, 1).unwrap();
            g.set_u32(globals + 0xc4, refs).unwrap();
            g.set_u32(refs + 4, 7).unwrap();
            g.set_u32(entries + 7 * 16 + 8, classes).unwrap();
            g.set_u32(classes + 4, rows).unwrap();
            g.set_u16(rows + 2 * 0x5c + 0xa, if seed == 1 { 0 } else { 1 << 10 })
                .unwrap();
            g.set_u32(STACK + 4, 0.375_f32.to_bits()).unwrap();
            g.set_u32(STACK + 8, head).unwrap();
        }
        0x21be20 => {
            c.gpr[3] = head;
            g.set_u32(STACK + 4, 0x9675).unwrap();
            g.set_u32(STACK + 8, 0.125_f32.to_bits()).unwrap();
            if seed >= 2 {
                g.set_u8(row(HEAD) + 4, 0x84).unwrap();
            }
            if seed == 3 {
                g.set_u8(row(HEAD) + 2, 1).unwrap();
            }
        }
        entry => panic!("unexpected loop entry {entry:x}"),
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x21b870 => {
            c.gpr[0] = if seed == 1 { HEAD } else { NONE };
            4
        }
        0x21b940 => {
            c.gpr[0] = ADDED;
            24
        }
        0x12a810 => {
            if seed == 4 {
                // The runtime must refetch the link after sampling gain.
                g.set_u32(row(HEAD) + 0x6c, HEAD).unwrap();
            }
            c.gpr[0] = (-12.25_f32).to_bits();
            c.gpr[1] = 0xfedc1234;
            c.gpr[2] = 0xfcdb;
            0
        }
        entry => panic!("unexpected loop callout {entry:x}"),
    }
}

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry == 0x21b0e0 {
        let mut cases: Vec<_> = (0..10)
            .map(|seed| Case {
                setup: walk_setup,
                reply: walk_reply,
                seed,
            })
            .collect();
        cases.extend(loop_tracks::cases());
        return cases;
    }
    let count = match entry {
        0x21b870 => 6,
        0x21b910 => 2,
        0x21bc80 => 3,
        0x21be20 | 0x21bd00 => 5,
        _ => 0,
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}

const LOOP_TAGS: u32 = ARENA + 0x9000;
const LSND: u32 = ARENA + 0xa000;
const SOUND: u32 = ARENA + 0xb000;
const MASK: u32 = ARENA + 0xc000;
const DETAILS: u32 = ARENA + 0xd000;
const SEEDS: u32 = ARENA + 0xe000;

fn setup_test_loop(g: &mut Guest, _c: &mut Cpu) -> (u32, u32) {
    g.set_u32(LOOPING_SOUNDS.base, LOOP_POOL).unwrap();
    g.set_u32(LOOP_POOL + 0x24, 0xd4).unwrap();
    g.set_u32(LOOP_POOL + 0x38, 1).unwrap();
    g.set_u32(LOOP_POOL + 0x44, LOOPS).unwrap();
    g.set_u32(LOOP_POOL + 0x48, MASK).unwrap();
    g.set_u32(MASK, 1).unwrap();
    g.set_u16(LOOPS, 0xa1d0).unwrap();
    g.set_u32(LOOPS + 4, TAG).unwrap();
    g.set_u32(LOOPS + 0x54, NONE).unwrap();
    g.set_u32(LOOPS + 0x88, NONE).unwrap();
    g.set_u32(SOUND_INSTANCES.base, VOICE_POOL).unwrap();
    g.set_u32(VOICE_POOL + 0x44, VOICES).unwrap();
    g.set_u32(engine::points::TAG_ENTRIES.base, LOOP_TAGS)
        .unwrap();
    g.set_u32(LOOP_TAGS + (TAG & 0xffff) * 16 + 8, LSND)
        .unwrap();
    g.set_u32(engine::points::SOUND_GLOBALS.base, SOUND)
        .unwrap();
    g.set_u32(engine::points::RANDOM_SEEDS.base, SEEDS).unwrap();
    g.set_u32(SEEDS + 4, 0xd9547b31).unwrap();
    g.set_u32(STACK + 4, 1).unwrap();
    (LOOPS, LSND)
}

fn walk_setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    setup_test_loop(g, c);
    g.set_u32(LSND + 0x24, 2).unwrap();
    g.set_u32(LSND + 0x28, DETAILS).unwrap();
    g.set_u32(SOUND + 0x84, 40).unwrap();
    for (i, opts) in [0, 2u32].into_iter().enumerate() {
        let detail = DETAILS + (i as u32) * 0x34;
        g.set_u32(detail + 8, TAG).unwrap();
        g.set_u32(detail + 12, 0.0625f32.to_bits()).unwrap();
        g.set_u32(detail + 16, 0.875f32.to_bits()).unwrap();
        g.set_u32(detail + 24, opts).unwrap();
    }
    match seed {
        1 => {
            // Replace the occupied row during the release callback.
            g.set_u8(LOOPS + 0x53, 3).unwrap();
        }
        2 => g.set_u8(LOOPS + 0x52, 2).unwrap(),   // stopped
        3 => g.set_u8(LOOPS + 0x50, 4).unwrap(),   // no detail sounds
        4 => g.set_u32(LOOPS + 0x58, 40).unwrap(), // equality holds the timer
        8 => g.set_u8(LOOPS + 0x50, 1).unwrap(),   // alternate details
        9 => {
            g.set_u32(LOOP_POOL + 0x38, 40).unwrap();
            g.set_u32(MASK + 4, 1 << 5).unwrap();
            let second = LOOPS + 37 * 0xd4;
            g.set_u16(second, 0x547f).unwrap();
            g.set_u32(second + 4, TAG).unwrap();
            g.set_u8(second + 0x52, 2).unwrap();
        }
        _ => {}
    }
}

fn walk_reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x16bc00 => {
            c.gpr[0] = if seed == 0 { NONE } else { 0 };
            0
        }
        0x21b070 => {
            g.set_u32(LOOP_POOL + 0x38, 0).unwrap();
            0
        }
        0x21cc60 => {
            c.gpr[0] = u32::from(seed != 5);
            12
        }
        0x125f70 => {
            c.gpr[0] = if seed == 6 || seed == 7 { NONE } else { ADDED };
            // An accepted request can be queued while the allocator returns
            // no instance. The call writes its result through EDI.
            g.set_u32(c.gpr[7], u32::from(seed == 7)).unwrap();
            4
        }
        addr => panic!("unhandled loop walk call {addr:x}"),
    }
}

#[path = "loop_tracks.rs"]
mod loop_tracks;
