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
    let count = match entry {
        0x21b870 => 6,
        0x21b910 => 2,
        0x21bc80 => 3,
        0x21be20 => 5,
        _ => 0,
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}
