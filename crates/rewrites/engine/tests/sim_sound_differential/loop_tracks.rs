//! Track update cases for the full loop-pool walk. Synthetic pitch bands
//! overlap, become inaudible, acquire replacement instances or fail admission.

use super::*;

const TRACK_TAG: u32 = ARENA + 0x11000;
const SOUND_TAG: u32 = ARENA + 0x12000;
const BANK: u32 = ARENA + 0x13000;
const CLASSES: u32 = ARENA + 0x14000;
const SCALES: u32 = ARENA + 0x15000;
const RANGES: u32 = ARENA + 0x16000;
const BOUNDS: u32 = ARENA + 0x17000;
const TRACK_RNG: u32 = ARENA + 0x18000;
const RNG_POOL: u32 = ARENA + 0x19000;
const SOUND_DATUM: u32 = 0x1c20_0011;

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    let (owner, lsnd) = setup_test_loop(g, c);
    g.set_u32(lsnd + 0x1c, 1).unwrap();
    g.set_u32(lsnd + 0x20, TRACK_TAG).unwrap();
    g.set_u32(TRACK_TAG + 8, 2.5f32.to_bits()).unwrap();
    g.set_u32(LOOP_TAGS + (SOUND_DATUM & 0xffff) * 16 + 8, SOUND_TAG)
        .unwrap();
    g.set_u32(owner + 0x88, HEAD).unwrap();
    g.set_u32(owner + 0x98, 0.25f32.to_bits()).unwrap();
    g.set_u32(owner + 0x10, 0.75f32.to_bits()).unwrap();
    g.set_u32(SOUND + 0x1e0, 0.125f32.to_bits()).unwrap();
    for (datum, next, pitch) in [(HEAD, SECOND, 0u8), (SECOND, HEAD, 2)] {
        let voice = row(datum);
        g.set_u8(voice + 2, 2).unwrap();
        g.set_u32(voice + 0xc, SOUND_DATUM).unwrap();
        g.set_u32(voice + 0x10, 0xa1d0_0000).unwrap();
        g.set_u32(voice + 0x6c, next).unwrap();
        g.set_u32(voice + 0x90, 30.0f32.to_bits()).unwrap();
        g.set_u8(voice + 0x9c, pitch).unwrap();
    }
    g.set_u32(0x51ebd4, BANK).unwrap();
    g.set_u32(BANK + 4, CLASSES).unwrap();
    g.set_u32(BANK + 0xc, SCALES).unwrap();
    g.set_u32(BANK + 0x1c, BOUNDS).unwrap();
    g.set_u32(BANK + 0x24, RANGES).unwrap();
    g.set_u32(
        CLASSES + 0xc,
        if seed & 1 == 0 { 0 } else { 1.0f32.to_bits() },
    )
    .unwrap();
    g.set_u32(0x51ebdc, TRACK_RNG).unwrap();
    g.set_u32(TRACK_RNG + 4, NONE).unwrap();
    g.set_u32(TRACK_RNG + 8, 0x4193b58d).unwrap();
    if seed & 1 != 0 {
        // Per-loop RNG slot pinned to an owner retains its current draw.
        g.set_u32(0x51ebd8, RNG_POOL).unwrap();
        g.set_u32(RNG_POOL + 0x44, TRACK_RNG - 0x1c).unwrap();
        g.set_u32(owner + 0x54, 0x6780_0001).unwrap();
        g.set_u32(TRACK_RNG + 4, 0xadd0).unwrap();
    }

    g.set_u8(
        SOUND_TAG + 1,
        if (37..=41).contains(&seed) { 1 << 5 } else { 0 },
    )
    .unwrap();
    g.set_u8(SOUND_TAG + 0xa, 3).unwrap();
    for (i, lo, hi) in [(0, 5, 20), (1, 25, 33), (2, 30, 64)] {
        g.set_u16(RANGES + i * 12 + 2, i as u16).unwrap();
        g.set_u16(BOUNDS + i * 10 + 2, lo).unwrap();
        g.set_u16(BOUNDS + i * 10 + 4, hi).unwrap();
    }
    match seed {
        32 => g.set_u8(owner + 0x52, 2).unwrap(), // stopped track
        33 => g.set_u32(STACK + 4, 0).unwrap(),   // fraction only
        40 => g.set_u8(SOUND_TAG + 0xa, 0).unwrap(), // empty pitch range table
        41 => g.set_u32(owner + 0x88, NONE).unwrap(), // empty instance ring
        _ => {}
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x218f50 => {
            c.gpr[0] = if seed == 34 { 0 } else { 1 };
            0
        }
        0x21b870 => {
            c.gpr[0] = if seed == 35 { SECOND } else { NONE };
            4
        }
        0x21b940 => {
            c.gpr[0] = if seed == 39 || seed == 42 {
                NONE
            } else {
                ADDED
            };
            24
        }
        0x219f80 => {
            c.gpr[0] = (-7.5f32).to_bits();
            4
        }
        0x126df0 | 0x127320 => 4,
        _ => walk_reply(g, c, target, seed),
    }
}

pub(super) fn cases() -> Vec<Case> {
    (32..43).map(|seed| Case { setup, reply, seed }).collect()
}
