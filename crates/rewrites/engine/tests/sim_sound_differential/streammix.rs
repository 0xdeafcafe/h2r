//! Stream submissions against the lifted driver using synthetic descriptors.
use super::*;

#[path = "streammix_routes.rs"]
mod stream_routes;

const DRIVER: u32 = ARENA;
const PARAMETERS: u32 = ARENA + 0x4000;
const CURRENT: u32 = ARENA + 0x4300;
const REQUEST: u32 = ARENA + 0x4600;
const HANDLE: u32 = 71;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let count = match entry {
        0x21fa80 => 5,
        0x21e230 => 3,
        0x2215d0 | 0x2216f0 => 9,
        _ => return Vec::new(),
    };
    let mut rows: Vec<_> = (0..count).map(|seed| Case { setup, reply, seed }).collect();
    if entry == 0x21fa80 {
        rows.extend(stream_routes::cases());
    }
    rows
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    match c.eip {
        0x21fa80 => {
            let index = 2;
            let stream = DRIVER + 0xc + index * 0x34;
            c.gpr[7] = PARAMETERS;
            g.set_u32(STACK + 4, index).unwrap();
            g.set_u32(STACK + 8, 19).unwrap();
            g.set_u32(STACK + 12, u32::from(seed == 0)).unwrap();
            g.set_u32(0x51ebe4, DRIVER).unwrap();
            g.set_u8(DRIVER, 1).unwrap();
            g.set_u8(stream + 3, 0x21).unwrap(); // initialized and plain
            g.set_u32(stream + 0x24, HANDLE).unwrap();
            let db = (-12.5f32).to_bits();
            g.set_u32(stream + 0xc, if seed == 4 { 0 } else { db })
                .unwrap();
            g.set_u32(PARAMETERS + 0xc, db).unwrap();
            g.set_u32(PARAMETERS + 0x30, 1).unwrap();
            g.set_u16(PARAMETERS + 0x24, u16::MAX).unwrap();
            g.set_u32(PARAMETERS + 4, seed % 3).unwrap();
            g.set_u32(PARAMETERS + 8, 0.3125f32.to_bits()).unwrap();
        }
        0x21e230 => {
            c.gpr[1] = 4 + seed;
            c.gpr[6] = REQUEST;
            g.set_u32(REQUEST + 0x40, seed).unwrap();
            g.set_u32(STACK + 4, (-3.5f32).to_bits()).unwrap();
        }
        _ => {
            c.gpr[0] = if seed == 0 { 0 } else { CURRENT };
            g.set_u32(STACK + 4, REQUEST).unwrap();
            g.set_u32(STACK + 8, HANDLE).unwrap();
            let count = if seed == 2 { 0 } else { 3 };
            g.set_u32(CURRENT, count).unwrap();
            g.set_u32(REQUEST + 0x40, if seed == 1 { count + 1 } else { count })
                .unwrap();
            g.set_u32(REQUEST + 0x44, REQUEST).unwrap();
            for i in 0..count {
                let bin = if seed >= 6 { i.min(1) } else { i };
                let old_gain = i + 13;
                let gain = match seed {
                    4 if i == 1 => 98,     // update one volume
                    6 | 7 if i == 2 => 97, // duplicate: row scratch conflicts or agrees
                    8 if i >= 1 => 97,     // duplicate changed twice
                    _ => old_gain,
                };
                g.set_u32(CURRENT + 4 + 8 * i, bin).unwrap();
                g.set_u32(CURRENT + 8 + 8 * i, old_gain).unwrap();
                g.set_u32(REQUEST + 8 * i, if seed == 5 && i == 1 { 2 } else { bin })
                    .unwrap();
                g.set_u32(REQUEST + 4 + 8 * i, gain).unwrap();
            }
            if seed >= 7 {
                // Comparison of a changed duplicate uses its row to look up
                // an earlier gain (22165a), which can be a stale stack word.
                g.set_u32(STACK - 0xec + 0x6c + 8, 97).unwrap();
            }
        }
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    let sp = c.gpr[4];
    match target {
        0x12aff0 => {
            c.xmm[0] = u128::from((0.125f32 + seed as f32 * 0.0625).to_bits());
            12
        }
        0x37b804 | 0x37b7ff => {
            assert_eq!(g.u32(sp + 4).unwrap(), HANDLE);
            c.gpr[0] = 0;
            8
        }
        0x2201f0 => 4,
        0x37b81d | 0x37c70a | 0x37b6c3 | 0x37c5e4 => {
            assert_eq!(g.u32(sp + 4).unwrap(), HANDLE);
            c.gpr[0] = seed + 17;
            8
        }
        _ => panic!("unexpected stream mix call {target:#x}"),
    }
}
