//! Synthetic sound instances with signed range, permutation and chunk offsets.
//! The individual roots call seeded tag, allocation and cache services.

use super::*;
use engine::points::{RANDOM_SEEDS, SOUND_GLOBALS, SOUND_INSTANCES, TAG_ENTRIES};

const ENTRIES: u32 = ARENA + 0x100;
const SOUND: u32 = ARENA + 0x400;
const SOURCE: u32 = ARENA + 0x800;
const POOL: u32 = ARENA + 0xc00;
const ROWS: u32 = ARENA + 0x1000;
const GESTALT: u32 = ARENA + 0x1800;
const RANGES: u32 = ARENA + 0x2000;
const PERMUTATIONS: u32 = ARENA + 0x2800;
const CHUNKS: u32 = ARENA + 0x3000;
const CACHE: u32 = ARENA + 0x3800;
const CACHE_ROWS: u32 = ARENA + 0x4000;
const CLASS: u32 = ARENA + 0x4800;
const TAG: u32 = 0x1234_0005;
const HANDLE: u32 = 0x9876_0002;
const INSTANCE: u32 = ROWS + 2 * 0xbc;

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    for (at, value) in [
        (TAG_ENTRIES.base, ENTRIES),
        (ENTRIES + (TAG & 0xffff) * 16 + 8, SOUND),
        (SOUND_INSTANCES.base, POOL),
        (POOL + 0x44, ROWS),
        (0x51ebd4, GESTALT),
        (GESTALT + 0x24, RANGES),
        (GESTALT + 0x2c, PERMUTATIONS),
        (GESTALT + 0x44, CHUNKS),
        (GESTALT + 12, ARENA + 0x5000),
        (0x502104, CACHE),
        (CACHE + 0x44, CACHE_ROWS),
        (SOUND_GLOBALS.base, ARENA + 0x5800),
        (ARENA + 0x5884, 0xffff_faf0),
        (RANDOM_SEEDS.base, ARENA + 0x6000),
        (ARENA + 0x6004, 0x6591_c412),
        (INSTANCE + 12, TAG),
        (CHUNKS + 12 * 5 + 8, 0xade0_0007),
    ] {
        g.set_u32(at, value).unwrap();
    }
    // Range 3-1, permutation 6-2 and chunk 8-3.
    for (at, word) in [
        (SOUND + 8, 3),
        (RANGES + 2 * 12 + 8, 6),
        (PERMUTATIONS + 4 * 16 + 12, 8),
        (INSTANCE + 0x9e, (-3i16) as u16),
    ] {
        g.set_u16(at, word).unwrap();
    }
    g.set_u8(INSTANCE + 0x9c, (-1i8) as u8).unwrap();
    g.set_u8(INSTANCE + 0x9d, (-2i8) as u8).unwrap();
    g.set_u8(INSTANCE + 4, if seed & 1 == 0 { 0x41 } else { 0x43 })
        .unwrap();
    g.set_u8(CACHE_ROWS + 7 * 16 + 4, seed as u8).unwrap();
    match c.eip {
        0x126000 => {
            // Requests span rejection, a full pool, automatic and explicit
            // selections, a promotion rule, both immediate-play flags, a
            // propagation delay, and optional attachment and payload fields.
            let flags = match seed {
                3 => 0x400,
                4 => 0x3ff,
                _ => 0,
            };
            g.set_u32(SOURCE, flags).unwrap();
            g.set_u32(SOURCE + 0x14, (-0.125f32).to_bits()).unwrap();
            g.set_u16(SOURCE + 0x8c, 67).unwrap();
            g.set_u16(SOURCE + 0x9c, 1).unwrap();
            g.set_u16(SOURCE + 0x9e, 2).unwrap();
            g.set_u16(
                SOURCE + 12,
                match seed {
                    5 => 1 << 8,
                    6 => 1 << 11,
                    _ => 0,
                },
            )
            .unwrap();
            g.set_u8(SOUND, if seed == 4 { 8 } else { 0 }).unwrap();
            g.set_u8(CLASS + 10, if seed == 4 { 4 } else { 0 }).unwrap();
            g.set_u32(0x547f14, 2.5_f32.to_bits()).unwrap();
            g.set_u32(0x440c48, 0).unwrap();
            g.set_u32(0x44a0b4, 0.25_f32.to_bits()).unwrap();
            for (i, word) in [TAG, 3, SOURCE, if seed == 4 { 2 } else { u32::MAX }]
                .into_iter()
                .enumerate()
            {
                g.set_u32(STACK + 4 + i as u32 * 4, word).unwrap();
            }
        }
        0x1286b0 => {
            c.gpr[0] = HANDLE;
            for (i, word) in [TAG, 0xacdd_fffe, 0xdeb1_0032, 0x90cf_c41c]
                .into_iter()
                .enumerate()
            {
                g.set_u32(STACK + 4 + i as u32 * 4, word).unwrap();
            }
        }
        0x125e60 => {
            g.set_u32(STACK + 4, INSTANCE).unwrap();
        }
        0x1268e0 => {
            c.gpr[0] = INSTANCE;
        }
        0x126960 => {
            c.gpr[1] = INSTANCE;
        }
        other => panic!("unknown impulse root {other:#x}"),
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x125260 => {
            c.gpr[0] = if seed == 0 { -1.0f32 } else { 1.0 }.to_bits();
            0
        }
        0x1251e0 => {
            c.gpr[0] = 0.125f32.to_bits();
            8
        }
        0x2226b0 => 0,
        0x16b840 => {
            c.gpr[0] = if seed == 1 { u32::MAX } else { HANDLE };
            0
        }
        0x127e20 => {
            let distance: f32 = if seed == 2 { 0.01 } else { 16384.0 };
            c.xmm[0] = u128::from(distance.to_bits());
            0
        }
        0x1252f0 => {
            // The pitch draw is a game-thread call, with both the seed write
            // and the value on the x87 stack visible when the allocator resumes.
            g.set_u32(c.gpr[1], seed.wrapping_mul(109)).unwrap();
            let mut pitch = x87::abi::X87Abi {
                st: c.x87.st,
                control: c.x87.control,
                status: c.x87.status,
                tag: c.x87.tag,
                ..Default::default()
            };
            x87::abi::ld_f32(&mut pitch, 0.125f32.to_bits());
            c.x87.st = pitch.st;
            c.x87.status = pitch.status;
            c.x87.tag = pitch.tag;
            0
        }
        0x221810 => {
            c.gpr[0] = CLASS;
            4
        }
        0x219a30 => {
            c.gpr[0] = 2;
            0
        }
        0x2198f0 => {
            c.gpr[0] = ARENA + 0x6400;
            4
        }
        0x219f60 => 0,
        0x218f10 | 0x218f50 => {
            c.gpr[0] = 1;
            0
        }
        0x219070 => {
            c.gpr[0] = 3;
            4
        }
        0x1286b0 => 16,
        0x125e60 => 4,
        0x126960 => {
            c.gpr[0] = 0xfad0_bc29;
            0
        }
        0x218850 => {
            c.gpr[0] = seed;
            12
        }
        0x1268e0 => 0,
        other => panic!("unexpected impulse call {other:#x}"),
    }
}

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let count = match entry {
        0x126000 => 7,
        0x1286b0 => 1,
        0x125e60 => 4,
        0x1268e0 | 0x126960 => 2,
        _ => 0,
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}
