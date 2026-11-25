//! Admit, update and reject loop requests, and find a salted datum across
//! holes in the looping sound allocation bitmap.
use super::*;
use engine::native_sound_segments::{LOOP_LOOKUP, LOOP_REQUEST};
use engine::points::{LOOPING_SOUNDS, SOUND_GLOBALS, TAG_ENTRIES};

const GLOBALS: u32 = ARENA + 0x200;
const ENTRIES: u32 = ARENA + 0x1000;
const TAG: u32 = 0x4213_0002;
const TAG_DATA: u32 = ARENA + 0x2000;
const SOURCE: u32 = ARENA + 0x3000;
const POOL: u32 = ARENA + 0x4000;
const ROWS: u32 = ARENA + 0x5000;
const BITS: u32 = ARENA + 0x9000;
const OWNER: u32 = 0xb4de_0010;

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    for (at, value) in [
        (SOUND_GLOBALS.base, GLOBALS),
        (TAG_ENTRIES.base, ENTRIES),
        (ENTRIES + (TAG & 0xffff) * 16 + 8, TAG_DATA),
        (LOOPING_SOUNDS.base, POOL),
        (POOL + 0x24, 0xd4),
        (POOL + 0x38, 39),
        (POOL + 0x44, ROWS),
        (POOL + 0x48, BITS),
        (BITS, 1 << 2),
        (BITS + 4, (1 << 4) | (1 << 8)),
    ] {
        g.set_u32(at, value).unwrap();
    }
    for slot in [2, 36] {
        let row = ROWS + slot * 0xd4;
        g.set_u16(row, 0xf391).unwrap();
        g.set_u32(row + 4, TAG).unwrap();
        g.set_u32(row + 8, if slot == 36 { OWNER } else { OWNER.wrapping_add(1) }).unwrap();
    }
    if c.eip == LOOP_LOOKUP.base {
        g.set_u32(STACK + 4, if seed == 3 { TAG + 1 } else { TAG }).unwrap();
        g.set_u32(STACK + 8, if seed == 2 { OWNER + 1 } else { OWNER }).unwrap();
        return;
    }
    assert_eq!(c.eip, LOOP_REQUEST.base);
    for offset in 0..3 {
        g.set_u8(GLOBALS + 0x78 + offset, u8::from(seed != offset + 1)).unwrap();
    }
    for offset in 0..68 {
        g.set_u8(SOURCE + offset, (offset as u8).wrapping_mul(13)).unwrap();
    }
    if seed == 8 {
        g.set_u8(SOURCE, 2).unwrap();
    }
    g.set_u8(TAG_DATA, if seed == 7 { 0x80 } else { 0 }).unwrap();
    for (index, value) in [
        if seed == 0 { u32::MAX } else { TAG },
        0xfa32_8357,
        OWNER,
        0x2345_9143,
        SOURCE,
        if matches!(seed, 3 | 4) { 2 } else { 0 },
        if seed == 6 { 0x11 } else { 1 },
        0xf5ad_4502,
    ].into_iter().enumerate() {
        g.set_u32(STACK + 4 + index as u32 * 4, value).unwrap();
    }
}

fn reply(_: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x16bc00 => {
            c.gpr[0] = if seed == 0 { u32::MAX } else { 2 };
            0
        }
        0x21cf80 => {
            c.gpr[0] = if matches!(seed, 4 | 5 | 7) { u32::MAX } else { 0xf391_0002 };
            8
        }
        0x21aeb0 => {
            c.gpr[0] = if seed == 5 { u32::MAX } else { 0xf391_0024 };
            12
        }
        0x21d060 => 0,
        _ => panic!("unexpected loop request call {target:#x}"),
    }
}

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let seeds = if entry == LOOP_LOOKUP.base { 4 } else if entry == LOOP_REQUEST.base { 9 } else { 0 };
    (0..seeds).map(|seed| Case { setup, reply, seed }).collect()
}
