//! Synthetic channel pools, class tables and instance records for the
//! channel census, class ranking, pick, pressure scan and ceiling pick.
//! The callbacks and the distance leaf run in the fixture, so a pass sees
//! the frame they would observe.
use super::*;
use engine::points;
use guest_layout::sound::SoundInstance;

const NONE: u32 = u32::MAX;
const POOL: u32 = ARENA + 0x1000;
const RECORDS: u32 = ARENA + 0x2000;
const TAG_TABLE: u32 = ARENA + 0x3000;
const TAG_DATA: u32 = ARENA + 0x3400;
const GLOBAL_TAG: u32 = ARENA + 0x4000;
const BLOCK: u32 = ARENA + 0x4100;
const PALETTE: u32 = ARENA + 0x4200;
const ROWS: u32 = ARENA + 0x4300;
const CHANNELS: u32 = ARENA + 0x5000;
const SOUND_GLOBALS: u32 = ARENA + 0x5800;
const RESULTS: u32 = ARENA + 0x5900;
const LIST: u32 = ARENA + 0x5a00;
const CALL_TABLE: u32 = ARENA + 0x5b00;
const CALLBACK: u32 = ARENA + 0x5c00;
const CLASS_FLAGS: u32 = ARENA + 0x5d00;
const REASON: u32 = ARENA + 0x5e00;
const COUNTER: u32 = ARENA + 0x5f00;
const CHANNEL_SIZE: u32 = 0x24;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    type Setup = fn(&mut Guest, &mut Cpu, u32);
    type Reply = fn(&mut Guest, &mut Cpu, u32, u32) -> u32;
    let (setup, reply, count): (Setup, Reply, u32) = match entry {
        0x128500 => (census, census_reply, 11),
        0x128b90 => (compare, compare_reply, 10),
        0x128700 => (select, select_reply, 11),
        0x128920 => (pressure, pressure_reply, 7),
        0x128a60 => (capped, no_calls, 6),
        _ => return vec![],
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}

fn handle(index: u32) -> u32 {
    ((index + 0x40) << 16) | index
}
fn record(index: u32) -> u32 {
    RECORDS + index * SoundInstance::SIZE
}
fn channel(index: u32) -> u32 {
    CHANNELS + index * CHANNEL_SIZE
}
fn tag_handle(index: u32) -> u32 {
    0x7700_0000 | index
}

/// Tags 1 and 3 belong to class 1 and 3; tag 2 to class 2. Priorities rise
/// with the class except that classes 1 and 3 tie.
fn world(g: &mut Guest, seed: u32) {
    g.set_u32(points::SOUND_INSTANCES.base, POOL).unwrap();
    g.set_u32(POOL + 0x44, RECORDS).unwrap();
    g.set_u32(points::SOUND_CHANNELS.base, CHANNELS).unwrap();
    g.set_u32(points::SOUND_GLOBALS.base, SOUND_GLOBALS).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAG_TABLE).unwrap();
    g.set_u32(points::GLOBALS_TAG.base, GLOBAL_TAG).unwrap();
    for tag in 0..12 {
        g.set_u32(TAG_TABLE + 16 * tag + 8, TAG_DATA + 0x40 * tag).unwrap();
    }
    for (tag, class) in [(1u32, 1u8), (2, 2), (3, 3)] {
        g.set_u8(TAG_DATA + 0x40 * tag + 2, class).unwrap();
        g.set_u8(TAG_DATA + 0x40 * tag + 4, if tag == 3 { 4 } else { 3 })
            .unwrap();
    }
    // The class palette: a tag reached through the globals block.
    g.set_u32(TAG_TABLE + 16 * 10 + 8, PALETTE).unwrap();
    g.set_u32(PALETTE + 4, ROWS).unwrap();
    g.set_u32(BLOCK + 4, 10).unwrap();
    g.set_u32(4, 10).unwrap();
    g.set_u32(GLOBAL_TAG + 0xc0, u32::from(seed != 99)).unwrap();
    g.set_u32(GLOBAL_TAG + 0xc4, BLOCK).unwrap();
    for (class, priority) in [(0u32, 0u16), (1, 5), (2, 7), (3, 5)] {
        let row = ROWS + 0x5c * class;
        g.set_u16(row, 3).unwrap();
        g.set_u16(row + 2, 2).unwrap();
        g.set_u32(row + 4, 20).unwrap();
        g.set_u16(row + 0xc, priority).unwrap();
    }
    g.set_u32(SOUND_GLOBALS + 0x84, 100).unwrap();
    g.set_u16(SOUND_GLOBALS + 0x208, 5).unwrap();
    for index in 0..8 {
        let r = record(index);
        g.set_u32(r + 0xc, tag_handle(1)).unwrap();
        g.set_u32(r + 0x10, 0x2222_0001).unwrap();
        g.set_u32(r + 0x14, CALL_TABLE).unwrap();
        g.set_u32(r + 0x8c, 100).unwrap();
        g.set_u8(r + 0xa0, 2).unwrap();
        g.set_u8(r + 0xa1, 1).unwrap();
        g.set_u16(r + 0xac, u16::MAX).unwrap();
    }
    for index in 0..5 {
        g.set_u32(channel(index), NONE).unwrap();
        g.set_u8(channel(index) + 8, 3).unwrap();
    }
    g.set_u32(CALL_TABLE + 0x1c, CALLBACK).unwrap();
    g.set_u32(COUNTER, 0).unwrap();
    g.write_bytes(REASON, &[0x77; 16], 1).unwrap();
    g.write_bytes(RESULTS, &[0x5a; 0x60], 1).unwrap();
}

fn no_calls(_: &mut Guest, _: &mut Cpu, target: u32, _: u32) -> u32 {
    panic!("unexpected call {target:#x}");
}

fn census(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    world(g, seed);
    cpu.gpr[6] = RESULTS;
    g.set_u32(STACK + 4, handle(1)).unwrap();
    g.set_u16(
        SOUND_GLOBALS + 0x208,
        if seed == 0 { 0 } else { 5 },
    )
    .unwrap();
    g.set_u32(channel(1), if seed == 1 { NONE } else { handle(2) }).unwrap();
    g.set_u32(channel(2), handle(1)).unwrap();
    g.set_u32(channel(3), if seed == 10 { handle(3) } else { NONE }).unwrap();
    g.set_u32(channel(4), handle(7)).unwrap();
    // Owner of the second record: same tag and a loop owner that may differ.
    g.set_u32(record(2) + 0x10, if matches!(seed, 3..=5) { 0x3333_0001 } else { 0x2222_0001 })
        .unwrap();
    g.set_u32(record(3) + 0x10, 0x4444_0001).unwrap();
    g.set_u32(record(3) + 0x8c, 7).unwrap();
    g.set_u32(record(7) + 0xc, tag_handle(2)).unwrap();
    match seed {
        5 => g.set_u32(CALL_TABLE + 0x1c, 0).unwrap(),
        6 => g.set_u8(channel(1) + 8, 9).unwrap(),
        7 => g.set_u32(record(2) + 0xc, tag_handle(2)).unwrap(),
        8 => g.set_u32(record(2) + 0x10, NONE).unwrap(),
        9 => g.set_u32(record(1) + 0x10, NONE).unwrap(),
        _ => {}
    }
    g.set_u32(record(2) + 0x8c, if seed == 2 { 100 } else { 90 }).unwrap();
}

fn census_reply(_: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    assert_eq!(target, CALLBACK);
    cpu.gpr[0] = if seed == 4 { 0x100 } else { 0x101 };
    16
}

fn compare(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    world(g, if seed == 9 { 99 } else { seed });
    cpu.gpr[0] = handle(2) | 0x0f00_0000;
    cpu.gpr[1] = handle(1) | 0x0e00_0000;
    g.set_u32(STACK + 4, 3.0f32.to_bits()).unwrap();
    let tags = match seed {
        0 => (2, 1),
        1 => (1, 2),
        2 | 3 | 9 => (1, 3),
        _ => (1, 1),
    };
    g.set_u32(record(1) + 0xc, tag_handle(tags.0)).unwrap();
    g.set_u32(record(2) + 0xc, tag_handle(tags.1)).unwrap();
    let layers = match seed {
        4 => (3u8, 1u8),
        5 => (1, 3),
        6 | 7 => (2, 2),
        8 => (0xff, 0),
        _ => (2, 2),
    };
    g.set_u8(record(1) + 0xa0, layers.0).unwrap();
    g.set_u8(record(2) + 0xa0, layers.1).unwrap();
    g.set_u8(record(2) + 0xa1, 0xfe).unwrap();
}

fn compare_reply(_: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    assert_eq!(target, 0x127e20);
    let distance = match seed {
        3 => 2.0f32,
        7 => 3.0,
        _ => 5.0,
    };
    cpu.xmm[0] = u128::from(distance.to_bits());
    0
}

fn select(g: &mut Guest, _: &mut Cpu, seed: u32) {
    world(g, seed);
    g.set_u32(STACK + 4, handle(1)).unwrap();
    g.set_u32(STACK + 8, REASON).unwrap();
    g.set_u16(record(1) + 0xac, if seed == 0 { 3 } else { u16::MAX }).unwrap();
    let looping = seed >= 5;
    g.set_u8(CLASS_FLAGS + 0x10 + 8, if looping { 2 } else { 0 }).unwrap();
    g.set_u8(CLASS_FLAGS + 0x30 + 8, 0).unwrap();
    g.set_u32(record(1) + 0x10, if seed == 10 { NONE } else { 0x2222_0001 }).unwrap();
    g.set_u16(SOUND_GLOBALS + 0x208, if seed == 5 { 0 } else { 3 }).unwrap();
    g.set_u32(channel(1), handle(2)).unwrap();
    g.set_u32(channel(2), handle(3)).unwrap();
    g.set_u8(channel(1) + 8, if seed == 6 { 9 } else { 3 }).unwrap();
    g.set_u32(record(2) + 0x10, 0x2222_0001).unwrap();
    g.set_u32(record(3) + 0x10, 0x5555_0001).unwrap();
    g.set_u8(record(2) + 0x1b, 0x5c).unwrap();
    g.set_u8(record(1) + 0x1b, 0xa3).unwrap();
    if seed == 9 {
        g.set_u32(record(2) + 0xc, tag_handle(3)).unwrap();
    }
}

fn select_reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    let sp = cpu.gpr[4];
    match target {
        0x221810 => {
            let class = g.u32(sp + 4).unwrap() as u16 as i16 as i32 as u32;
            cpu.gpr[0] = CLASS_FLAGS + 0x10 * class;
            4
        }
        0x128500 => {
            let block = cpu.gpr[6];
            let (flag, count2, limit2, count1, limit1) = match seed {
                1 => (1u8, 0u16, 3u16, 0u16, 2u16),
                2 => (0, 5, 3, 0, 2),
                3 => (0, 1, 3, 4, 2),
                _ => (0, 0, 3, 0, 2),
            };
            g.set_u8(block + 0x24, flag).unwrap();
            g.set_u16(block + 0x26, count2).unwrap();
            g.set_u16(block + 0x48, limit2).unwrap();
            g.set_u16(block, count1).unwrap();
            g.set_u16(block + 0x22, limit1).unwrap();
            4
        }
        0x128a60 => {
            cpu.gpr[0] = 7;
            4
        }
        0x128920 => {
            cpu.gpr[0] = match seed {
                8 => 0xffff,
                6 | 9 => 2,
                _ => 3,
            };
            8
        }
        0x127320 => 4,
        0x12a5d0 => 0,
        _ => panic!("unexpected channel pick call {target:#x}"),
    }
}

fn pressure(g: &mut Guest, _: &mut Cpu, seed: u32) {
    world(g, seed);
    g.set_u32(STACK + 4, handle(1)).unwrap();
    g.set_u32(STACK + 8, REASON).unwrap();
    g.set_u16(
        SOUND_GLOBALS + 0x208,
        match seed {
            0 => 0,
            6 => 1,
            _ => 3,
        },
    )
    .unwrap();
    let owners = match seed {
        1 => [NONE, NONE, NONE],
        _ => [handle(2), handle(3), handle(4)],
    };
    for (index, owner) in owners.into_iter().enumerate() {
        g.set_u32(channel(index as u32), owner).unwrap();
    }
    match seed {
        1 => g.set_u8(channel(0) + 8, 9).unwrap(),
        2..=4 => g.set_u8(channel(2) + 8, 9).unwrap(),
        6 => g.set_u8(channel(0) + 8, 9).unwrap(),
        _ => {}
    }
}

fn pressure_reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x127e20 => {
            let n = g.u32(COUNTER + 4).unwrap();
            g.set_u32(COUNTER + 4, n + 1).unwrap();
            cpu.xmm[0] = u128::from((4.0f32 + n as f32).to_bits());
            0
        }
        0x128b90 => {
            let n = g.u32(COUNTER).unwrap();
            g.set_u32(COUNTER, n + 1).unwrap();
            let sequence: &[u32] = match seed {
                2 => &[1, 0],
                3 => &[1, 1, 1],
                4 => &[1, 1, 0],
                _ => &[0],
            };
            cpu.gpr[0] = sequence.get(n as usize).copied().unwrap_or(0) | 0x0100_0000 * (seed & 1);
            4
        }
        _ => panic!("unexpected pressure call {target:#x}"),
    }
}

fn capped(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    world(g, if seed == 4 { 99 } else { seed });
    let count = match seed {
        0 => 0,
        5 => 0x8001,
        _ => 3,
    };
    cpu.gpr[0] = handle(1) | 0x0b00_0000;
    cpu.gpr[1] = 0xabcd_0000 | count;
    g.set_u32(STACK + 4, LIST).unwrap();
    for (slot, channel_index) in [0u16, 1, 2, 3].into_iter().enumerate() {
        g.set_u16(LIST + 2 * slot as u32, channel_index).unwrap();
    }
    for index in 0..4 {
        g.set_u32(channel(index), handle(2 + index)).unwrap();
    }
    let starts = match seed {
        2 => [90u32, 95, 99, 99],
        3 => [70, 70, 90, 90],
        _ => [90, 70, 60, 50],
    };
    let layers = if seed == 2 { [5u8, 3, 9, 9] } else { [2, 2, 2, 2] };
    g.set_u8(record(1) + 0xa0, 5).unwrap();
    for index in 0..4 {
        let owner = record(2 + index);
        g.set_u32(owner + 0x8c, starts[index as usize]).unwrap();
        g.set_u8(owner + 0xa0, layers[index as usize]).unwrap();
    }
}
