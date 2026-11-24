//! Guest-visible changes at the promotion timer, positional mix-in and
//! cluster-distance boundaries.
use super::*;

const DATA: u32 = ARENA + 0x1000;
const RULES: u32 = ARENA + 0x3000;
const TIMERS: u32 = ARENA + 0x4000;
const SOURCE: u32 = ARENA + 0x5000;
const LISTENER: u32 = ARENA + 0x6000;
const WEIGHTS: u32 = ARENA + 0x7000;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let count = match entry {
        0x126ec0 | 0x249d60 => 7,
        0x220e00 | 0x221e30 => 5,
        _ => 0,
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    match c.eip {
        0x126ec0 => {
            let tag = 3;
            c.gpr[1] = 0x1367_0000 | tag;
            let table = ARENA + 0x800;
            g.set_u32(0x4e3b44, table).unwrap();
            g.set_u32(table + 16 * tag + 8, SOURCE).unwrap();
            g.set_u8(SOURCE + 12, if seed == 0 { u8::MAX } else { 0 })
                .unwrap();
            g.set_u32(0x51ebd4, LISTENER).unwrap();
            g.set_u32(LISTENER + 0x4c, DATA).unwrap();
            g.set_u32(0x4e6380, ARENA).unwrap();
            g.set_u32(ARENA + 0x84, 17).unwrap();
            g.set_u32(STACK + 4, WEIGHTS).unwrap();
            g.set_u32(DATA, 2).unwrap();
            g.set_u32(DATA + 4, RULES).unwrap();
            g.set_u32(DATA + 8, 2).unwrap();
            g.set_u32(DATA + 12, TIMERS).unwrap();
            g.set_u32(DATA + 16, if seed == 3 { 1 } else { 0 }).unwrap();
            g.set_u32(DATA + 20, 13).unwrap();
            g.set_u32(DATA + 24, if seed % 2 == 1 { 18 } else { 17 })
                .unwrap();
            for rule in 0..2 {
                let r = RULES + 16 * rule;
                g.set_u16(r + 2, u16::from(seed >= 3)).unwrap();
                g.set_u32(r + 4, 0.75f32.to_bits()).unwrap();
                g.set_u32(r + 8, 5).unwrap();
                g.set_u32(r + 12, 2).unwrap();
                g.set_u32(TIMERS + 4 * rule, seed * 2).unwrap();
            }
        }
        0x249d60 => {
            let (a, b) = if seed % 2 == 0 { (1, 4) } else { (4, 1) };
            c.gpr[0] = a;
            c.gpr[1] = if seed == 0 { a } else { b };
            c.gpr[2] = ARENA;
            g.set_u32(ARENA + 0x224, u32::from(seed != 1)).unwrap();
            g.set_u32(ARENA + 0x228, DATA).unwrap();
            g.set_u32(ARENA + 0x9c, 5).unwrap();
            g.set_u32(DATA + 4, 2.25f32.to_bits()).unwrap();
            g.set_u32(DATA + 8, 26.75f32.to_bits()).unwrap();
            g.set_u32(DATA + 0x28, TIMERS).unwrap();
            for i in 0..10 {
                g.set_u8(TIMERS + i, [0, 1, 0, 255, 1, 253, 41][seed as usize])
                    .unwrap();
            }
        }
        0x220e00 => {
            c.gpr[7] = SOURCE;
            g.set_u32(0x51ebe4, ARENA).unwrap();
            let index = 2;
            g.set_u32(STACK + 4, index).unwrap();
            let record = ARENA + 0x1a18 + 0x3c * index;
            g.set_u8(ARENA, 1).unwrap();
            g.set_u8(record + 4, if seed == 0 { 0 } else { 2 }).unwrap();
            g.set_u32(record + 0x34, 784).unwrap();
            g.set_u8(SOURCE, u8::from(seed % 2 == 0)).unwrap();
            for (i, v) in [1.0f32, -17.25, 9.5, 0.375, 0.125, 0.5, 0.1, 0.05]
                .into_iter()
                .enumerate()
            {
                g.set_u32(SOURCE + 4 + 4 * i as u32, v.to_bits()).unwrap();
                if seed >= 2 && i < 3 {
                    g.set_u32(record + 8 + 4 * i as u32, v.to_bits()).unwrap();
                }
            }
        }
        0x221e30 => {
            c.gpr[7] = SOURCE;
            g.set_u32(STACK + 4, WEIGHTS).unwrap();
            g.set_u32(STACK + 8, LISTENER).unwrap();
            g.set_u32(SOURCE + 0x40, if seed == 0 { 35 } else { 17 })
                .unwrap();
            g.set_u32(LISTENER, 35).unwrap();
            g.set_u16(LISTENER + 4, 2).unwrap();
            g.set_u32(SOURCE + 0x3c, if seed == 1 { 2 } else { 3 })
                .unwrap();
            g.set_u8(SOURCE + 0x44, u8::from(seed == 2)).unwrap();
            g.set_u8(SOURCE + 0x47, u8::from(seed == 1 || seed == 2))
                .unwrap();
            g.set_u32(WEIGHTS + 12, 0.625f32.to_bits()).unwrap();
            g.set_u32(0x4e0348, ARENA).unwrap();
            g.set_u32(ARENA + 0xa0, DATA).unwrap();
            g.set_u32(0x4ed280, DATA + 0x600).unwrap();
            g.set_u16(0x4686c4, 0).unwrap();
            g.set_u32(0x4e0340, DATA + 0x800).unwrap();
            for i in 0..3 {
                g.set_u32(SOURCE + 0x24 + 4 * i, ((i + 1) as f32).to_bits())
                    .unwrap();
                g.set_u32(LISTENER + 0x30 + 4 * i, (-(i as f32)).to_bits())
                    .unwrap();
            }
        }
        _ => unreachable!(),
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x249ed0 => {
            c.gpr[0] = 0xc5;
            0
        }
        0x37c668 => {
            // Source data is reloaded after the DirectSound setter returns.
            g.set_u32(SOURCE + 4, 2.375f32.to_bits()).unwrap();
            c.gpr[0] = 1;
            20
        }
        0x2211e0 => {
            c.gpr[0] = 4;
            4
        }
        0x1de630 => {
            // Front and back ray results: distance along each segment.
            g.set_u32(c.gpr[0], 0.375f32.to_bits()).unwrap();
            c.gpr[0] = u32::from(seed == 4);
            16
        }
        0x222150 => {
            g.set_u32(c.gpr[3], 0.375f32.to_bits()).unwrap();
            g.set_u32(c.gpr[3] + 4, 0.125f32.to_bits()).unwrap();
            12
        }
        _ => panic!("unhandled sound callout {target:#x}, seed {seed}"),
    }
}
