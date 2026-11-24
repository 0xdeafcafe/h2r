//! Synthetic sound seeds, cache chunks and stream slots. The callback in
//! the promotion case replaces the queued chunk while the old one releases.

use super::*;
use engine::{datum::layout, points};

const POOL: u32 = ARENA + 0x1000;
const DATA: u32 = ARENA + 0x2000;
const BITS: u32 = ARENA + 0x3000;
const MIXER: u32 = ARENA + 0x4000;
const MANAGER: u32 = ARENA + 0x5000;
const CHANNEL: u32 = ARENA + 0x6000;
const RESOURCES: u32 = ARENA + 0x7000;
const TAGS: u32 = ARENA + 0x8000;
const INFO: u32 = ARENA + 0x9000;
const CURRENT: u32 = ARENA + 0xa000;
const QUEUED: u32 = ARENA + 0xb000;
const REPLACEMENT: u32 = ARENA + 0xc000;
const STREAM_STATE: u32 = MIXER + 0xc + 2 * 0x34 + 1;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let n = match entry {
        0x219cc0 => 3,
        0x12a450 => 12,
        _ => 0,
    };
    (0..n).map(|seed| Case { setup, reply, seed }).collect()
}

fn setup(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    if cpu.eip == 0x219cc0 {
        g.set_u32(0x51ebd8, POOL).unwrap();
        g.set_u32(0x51ebdc, DATA).unwrap();
        g.set_u32(POOL + layout::DATA, DATA).unwrap();
        g.set_u32(POOL + layout::ELEMENT_SIZE, 0x1c).unwrap();
        g.set_u32(POOL + layout::HIGH_WATER, 36).unwrap();
        g.set_u32(POOL + layout::BITMAP, BITS).unwrap();
        g.set_u32(BITS, 1 << 3).unwrap();
        g.set_u32(BITS + 4, (1 << 3) | (1 << 18)).unwrap();
        for i in 0..54 {
            g.set_u16(DATA + i * 0x1c, (i + 13) as u16).unwrap();
            g.set_u32(DATA + i * 0x1c + 8, i * 141).unwrap();
        }
        return;
    }

    cpu.gpr[0] = 0xcafe_0001;
    g.set_u32(points::SOUND_CHANNELS.base, CHANNEL - 0x24).unwrap();
    g.set_u32(points::SOUND_DRIVER.base, MIXER).unwrap();
    g.set_u32(points::SOUND_GLOBALS.base, MANAGER).unwrap();
    g.set_u32(points::SOUND_INSTANCES.base, POOL).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(POOL + layout::DATA, DATA).unwrap();
    g.set_u32(DATA + 0xbc + 0xc, 0xc156_0002).unwrap();
    g.set_u32(TAGS + 2 * 16 + 8, INFO).unwrap();
    g.set_u8(INFO + 3, 0xf1).unwrap();
    g.set_u32(CHANNEL, 0xa32b_0001).unwrap();
    g.set_u8(CHANNEL + 9, u8::from(seed == 1)).unwrap();
    g.set_u16(CHANNEL + 0xc, if seed == 0 { u16::MAX } else { 2 }).unwrap();
    g.set_u32(CHANNEL + 0x10, 1.75f32.to_bits()).unwrap();
    g.set_u16(CHANNEL + 0x18, 0).unwrap();
    g.set_u16(CHANNEL + 0x1a, 1).unwrap();
    g.set_u32(CHANNEL + 0x1c, if seed == 2 { 0 } else { CURRENT }).unwrap();
    g.set_u32(CHANNEL + 0x20, if seed < 7 { 0 } else { QUEUED }).unwrap();
    g.set_u16(CURRENT + 0xc, 3).unwrap();
    g.set_u16(QUEUED + 0xc, 7).unwrap();
    g.set_u16(REPLACEMENT + 0xc, 13).unwrap();
    g.set_u32(0x51ebd4, RESOURCES).unwrap();
    g.set_u32(RESOURCES + layout::DATA, RESOURCES + 0x100).unwrap();
    g.set_u32(MANAGER + 0x1e0, 0.5f32.to_bits()).unwrap();
    let phase = match seed {
        3 => 0,
        4 => 2,
        5 | 11 => 3,
        6 => 6,
        _ => 1,
    };
    g.set_u8(STREAM_STATE, phase | 0x98).unwrap();
}

fn reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x16bc00 => {
            cpu.gpr[0] = [u32::MAX, 3, 35][seed as usize];
            0
        }
        0x125ef0 => {
            if seed == 9 {
                g.set_u32(CHANNEL + 0x20, REPLACEMENT).unwrap();
            }
            cpu.gpr[0] = 0x5634_0013;
            0
        }
        0x219290 => {
            cpu.gpr[0] = if seed == 8 { 4 } else { u32::MAX };
            0
        }
        0x218850 => {
            cpu.gpr[0] = if seed == 9 { 0 } else { 2 };
            if seed == 10 {
                g.set_u8(STREAM_STATE, 0x80).unwrap();
            }
            12
        }
        0x21f650 => {
            cpu.xmm[0] = u128::from(3.25f32.to_bits());
            4
        }
        _ => panic!("unexpected stream call {target:#x}"),
    }
}
