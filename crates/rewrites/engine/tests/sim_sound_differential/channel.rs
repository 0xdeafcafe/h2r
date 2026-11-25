//! Channel update transcripts over synthetic selected pitch ranges, stream
//! state and overlapping loop companions. Setter replies may change a record.
use super::*;

const CHANNEL: u32 = ARENA;
const INSTANCE: u32 = ARENA + 0x800;
const TABLE: u32 = ARENA + 0x1800;
const TAG: u32 = ARENA + 0x1a00;
const GESTALT: u32 = ARENA + 0x2000;
const PLAYBACK: u32 = ARENA + 0x2400;
const RANGE: u32 = ARENA + 0x2600;
const PERM: u32 = ARENA + 0x2800;
const PITCH: u32 = ARENA + 0x2a00;
const DRIVER: u32 = ARENA + 0x3000;
const COMPANION: u32 = ARENA + 0x4000;
const LISTENER: u32 = ARENA + 0x4400;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry != engine::points::SOUND_CHANNEL_UPDATE.base { return Vec::new(); }
    (0..10).map(|seed| Case { setup, reply, seed }).collect()
}

fn setup(g: &mut Guest, _: &mut Cpu, seed: u32) {
    let pool = ARENA + 0x500;
    let companion_pool = ARENA + 0x3a00;
    for (address, value) in [
        (engine::points::SOUND_CHANNELS.base, CHANNEL),
        (engine::points::SOUND_INSTANCES.base, pool),
        (pool + 0x44, INSTANCE),
        (engine::points::TAG_ENTRIES.base, TABLE),
        (TABLE + 8, TAG),
        (0x51ebd4, GESTALT),
        (GESTALT + 0xc, PLAYBACK),
        (GESTALT + 0x24, RANGE),
        (GESTALT + 0x2c, PERM),
        (GESTALT + 0x1c, PITCH),
        (engine::points::SOUND_DRIVER.base, DRIVER),
        (0x51ebd8, companion_pool),
        (companion_pool + 0x44, COMPANION),
        (INSTANCE + 0x1c, 0.375f32.to_bits()),
        (INSTANCE + 0x20, (-7.5f32).to_bits()),
        (INSTANCE + 0x90, 1.25f32.to_bits()),
        (INSTANCE + 0x94, 3.625f32.to_bits()),
        (INSTANCE + 0xa8, if seed % 3 == 0 { u32::MAX } else { 0 }),
        (COMPANION + 4, if seed == 5 { u32::MAX } else { 17 }),
        (COMPANION + 0xc, 0.375f32.to_bits()),
        (LISTENER + 0x14, 0.625f32.to_bits()),
        (LISTENER + 0x48, 2.0f32.to_bits()),
        (CHANNEL + 0x1c, if seed == 4 { 0 } else { PERM }),
        (STACK + 4, 0),
        (STACK + 8, (-0.125f32).to_bits()),
        (STACK + 12, LISTENER),
    ] { g.set_u32(address, value).unwrap(); }
    g.set_u16(PLAYBACK + 8, (-37_i16) as u16).unwrap();
    g.set_u16(PLAYBACK + 10, 113).unwrap();
    g.set_u16(PITCH, 27).unwrap();
    g.set_u8(PERM + 4, (-3_i8) as u8).unwrap();
    let flags = match seed {
        0 | 1 => 0x40,
        2 => 0x200 | 8,
        3 | 4 => 0x200,
        5 => 8,
        _ => 0,
    };
    g.set_u16(INSTANCE + 4, flags).unwrap();
    g.set_u8(INSTANCE + 6, if seed >= 5 { 2 } else { 0 }).unwrap();
    g.set_u16(PERM + 0xe, if seed % 2 == 0 { 3 } else { 1 }).unwrap();
    g.set_u8(COMPANION + 3, 0x80).unwrap();
}

fn reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    // Poison volatile registers at every return. Each side must reload what
    // it needs, including the companions reread after chunk submission.
    cpu.gpr[0] = 0xeeeeeedd;
    cpu.gpr[1] = 0xaaaa2222;
    cpu.gpr[2] = 0xdddd3333;
    match target {
        0x1251e0 => {
            cpu.gpr[0] = [-80.0f32, -6.25, 9.0, f32::NAN][seed as usize % 4].to_bits();
            8
        }
        0x12a1b0 | 0x21f8a0 | 0x21f720 => 4,
        0x21fa80 => 12,
        0x221810 => { cpu.gpr[0] = LISTENER; 4 }
        0x12a9d0 => {
            // Nonempty ext80 stack: the caller stores and pops this value.
            cpu.x87.st[0] = x87::F80::from_f32_bits(0.875f32.to_bits()).to_bytes();
            cpu.x87.status &= !(7 << 11);
            cpu.x87.tag &= !3;
            0
        }
        0x125e60 => { cpu.gpr[0] = seed & 1; 4 }
        0x129f20 => {
            if seed == 7 { g.set_u32(INSTANCE + 0xa8, u32::MAX).unwrap(); }
            8
        }
        0x1286b0 => 16,
        _ => panic!("unexpected channel call {target:#x}"),
    }
}
