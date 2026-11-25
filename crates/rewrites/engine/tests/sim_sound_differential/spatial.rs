//! Channel descriptors over synthetic listeners and instances. Both outcomes
//! of the instance callback exercise the descriptor's private working copy.
use super::*;
use engine::points;

const ROWS: u32 = ARENA + 0x1000;
const POOL: u32 = ARENA + 0x2000;
const SOURCES: u32 = ARENA + 0x3000;
const GLOBALS: u32 = ARENA + 0x4000;
const OUTPUT: u32 = ARENA + 0x5000;
const GAIN: u32 = ARENA + 0x5100;
const CALLBACK: u32 = ARENA + 0x5400;
const TARGET: u32 = ARENA + 0x5500;
const TAGS: u32 = ARENA + 0x5600;
const TAG: u32 = ARENA + 0x5900;
const CLASS: u32 = ARENA + 0x5c00;
const DIRECTION: u32 = ARENA + 0x6000;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry == points::SOUND_CHANNEL_SPATIAL.base {
        (0..14).map(|seed| Case { setup, reply, seed }).collect()
    } else if matches!(entry, 0x12aff0 | 0x12ac20 | 0x12ad30) {
        (0..12).map(|seed| Case { setup: attenuation_setup, reply: attenuation_reply, seed }).collect()
    } else {
        Vec::new()
    }
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    let channel = ROWS + 0x24;
    let source = SOURCES + 0xbc;
    for (at, word) in [
        (points::SOUND_CHANNELS.base, ROWS),
        (points::SOUND_INSTANCES.base, POOL),
        (points::SOUND_GLOBALS.base, GLOBALS),
        (POOL + 0x44, SOURCES),
        (channel, 0xabcd0001),
        (channel + 0x14, if seed % 4 == 0 { 0 } else { 171 }),
        (GLOBALS + 0x84, if seed == 6 { 5500 } else { 325 }),
        (source + 0xc, 0x43210001),
        (source + 0x10, 0x1234),
        (source + 0x14, if seed == 0 { 0 } else { CALLBACK }),
        (CALLBACK + 0xc, TARGET),
        (points::TAG_ENTRIES.base, TAGS),
        (TAGS + 0x18, TAG),
        (CLASS + 0x14, 0.6f32.to_bits()),
        (GAIN, 1.5f32.to_bits()),
        (STACK + 4, GAIN),
        (STACK + 8, OUTPUT),
    ] {
        g.set_u32(at, word).unwrap();
    }
    c.gpr[0] = 0x1234_0001;
    let kind: u8 = [0, 0, 1, 1, 1, 2, 2, 2, 7, 15, 1, 2, 1, 2][seed as usize];
    g.set_u8(source + 0x1b, 0xc0 | kind).unwrap();
    g.set_u8(source + 0x19, if seed % 3 == 0 { 8 } else { 1 }).unwrap();
    g.set_u8(source + 4, if seed == 12 { 32 } else { 0 }).unwrap();
    g.set_u8(source + 6, if seed % 2 == 0 { 0 } else { 2 }).unwrap();
    g.set_u8(source + 0xa1, 2).unwrap();
    g.set_u8(channel + 10, if seed < 4 { 0 } else { 1 + (seed % 2) as u8 }).unwrap();
    g.set_u8(TAG + 2, 0xf3).unwrap();
    let listener = GLOBALS + 0x88 + 2 * 0x48;
    for i in 0..9 {
        g.set_u32(listener + 12 + i * 4, ((i as f32 - 2.25) * 0.2).to_bits()).unwrap();
    }
    for i in 0..3 {
        g.set_u32(source + 0x24 + i * 4, ((i as f32 - 2.0) * 0.75).to_bits()).unwrap();
        g.set_u32(source + 0x34 + i * 4, (i as f32 + 0.25).to_bits()).unwrap();
        g.set_u32(DIRECTION + i * 4, ((i as f32 - 1.0) * 1.25).to_bits()).unwrap();
    }
    for i in 0..14 {
        g.set_u32(OUTPUT + i * 4, (i as f32 * 0.08).to_bits()).unwrap();
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x12abe0 => {
            for i in 0..3 {
                g.set_u32(c.gpr[0] + 4 * i, (i as f32 + 2.5).to_bits()).unwrap();
            }
            0
        }
        0x142700 | 0x1427f0 => {
            for i in 0..3 {
                g.set_u32(c.gpr[2] + 4 * i, (i as f32 - 0.625).to_bits()).unwrap();
            }
            0
        }
        0x125060 => {
            c.gpr[0] = u32::from(seed % 3 != 0);
            0
        }
        0x188ea0 => {
            c.gpr[0] = DIRECTION;
            0
        }
        0x12ad30 => {
            c.xmm[0] = u128::from(0.375f32.to_bits());
            8
        }
        0x2197f0 => {
            c.gpr[0] = (-3.75f32).to_bits();
            0
        }
        0x12a6d0 => {
            c.gpr[0] = (if g.u32(c.gpr[4] + 8).unwrap() >> 31 == 0 { 0.75f32 } else { -1.25 }).to_bits();
            8
        }
        0x2195f0 => {
            let mut x = x87::abi::X87Abi {
                st: c.x87.st, tag: c.x87.tag, status: c.x87.status, control: c.x87.control,
                ..Default::default()
            };
            x87::abi::ld_f32(&mut x, 0.625f32.to_bits());
            c.x87.st = x.st;
            c.x87.status = x.status;
            c.x87.tag = x.tag;
            4
        }
        0x221810 => {
            c.gpr[0] = CLASS;
            4
        }
        TARGET => {
            let copy = g.u32(c.gpr[4] + 16).unwrap();
            for i in 0..14 {
                g.set_u32(copy + i * 4, 0x5500 | i).unwrap();
            }
            c.gpr[0] = seed % 2;
            16
        }
        _ => panic!("unexpected spatial child {target:#x}"),
    }
}

fn attenuation_setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    if c.eip == 0x12aff0 {
        let (a, b, sample): (f32, f32, f32) = match seed {
            0 | 1 => (1.25, 1.25, 1.125),
            2 | 3 => (1.25, 1.25, 1.5),
            4 => (1.25, 1.25, 1.25),
            5 => (7.0, 3.5, 8.0),
            6 => (7.0, 3.5, 3.0),
            7 => (7.0, 3.5, 5.25),
            8 => (0.5, 1.5, -2.0),
            9 => (0.5, 1.5, 2.0),
            10 => (0.5, 1.5, f32::NAN),
            _ => (f32::NAN, 2.0, 1.0),
        };
        c.gpr[1] = 0xf00d0000 | (seed % 2);
        for (i, v) in [a, b, sample].into_iter().enumerate() {
            g.set_u32(STACK + 4 + i as u32 * 4, v.to_bits()).unwrap();
        }
        return;
    }
    let sample = [0.0f32, 1.5, 6.5, 12.0][(seed % 4) as usize];
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 0x18, TAG).unwrap();
    g.set_u32(0x51ebd4, GLOBALS).unwrap();
    g.set_u32(GLOBALS + 4, ROWS).unwrap();
    let near = if seed % 4 == 2 { 0.0f32 } else { 2.75f32 };
    for (at, value) in [(SOURCES + 0x34, near), (SOURCES + 0x38, 8.0),
                        (ROWS + 0x38, near), (ROWS + 0x3c, 9.5),
                        (CLASS + 0x18, near), (CLASS + 0x1c, 13.75)] {
        g.set_u32(at, value.to_bits()).unwrap();
    }
    g.set_u16(TAG + 6, 1).unwrap();
    g.set_u8(TAG + 2, 0xfb).unwrap();
    let source_flags = match seed % 4 { 0 => 0, 1 => 4, 2 => 8, _ => 12 };
    g.set_u16(SOURCES, source_flags | if seed % 2 != 0 { 0x210 } else { 0 }).unwrap();
    let class_flags = match seed % 3 { 0 => 0, 1 => 4, _ => 12 };
    g.set_u8(TAG + 1, class_flags | if seed == 6 { 0x10 } else { 0 }).unwrap();
    if c.eip == 0x12ac20 {
        c.gpr[0] = 0xacdc0001;
        c.gpr[7] = SOURCES;
        g.set_u32(STACK + 4, sample.to_bits()).unwrap();
    } else {
        c.gpr[0] = OUTPUT;
        c.gpr[1] = SOURCES;
        g.set_u32(STACK + 4, 0xacdc0001).unwrap();
        g.set_u32(STACK + 8, DIRECTION).unwrap();
        for (i, v) in [sample, sample * 0.75, sample * -0.5].into_iter().enumerate() {
            g.set_u32(OUTPUT + i as u32 * 4, v.to_bits()).unwrap();
        }
    }
}

fn attenuation_reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x124fc0 => {
            c.xmm[0] = u128::from(2.25f32.to_bits());
            0
        }
        0x12aff0 => {
            c.xmm[0] = u128::from(0.625f32.to_bits());
            12
        }
        0x12ac20 => {
            let gain = if seed == 4 { 0.0f32 } else { 0.375 };
            c.xmm[0] = u128::from(gain.to_bits());
            4
        }
        0x30bf0 => {
            c.xmm[0] = u128::from((if seed == 7 { 0.0f32 } else { 1.75 }).to_bits());
            0
        }
        0x1251a0 => {
            c.gpr[0] = (-9.25f32).to_bits();
            0
        }
        0x11cf50 | 0x125120 | 0x1250a0 => {
            c.xmm[0] = u128::from((seed as f32 + 0.125).to_bits());
            0
        }
        0x221810 | 0x2195f0 => reply(g, c, target, seed),
        _ => panic!("unexpected distance/cone child {target:#x}"),
    }
}
