//! Compare the complete start transaction at every call boundary, both with
//! and without an auxiliary record. Different failures before and after the
//! reservation exercise the direct fallback and the rollback path.

use super::{ARENA, Case, Cpu, Guest, STACK};
use engine::points;

const REQUEST: u32 = ARENA + 0x100;
const TAG: u32 = 0xb27d_0003;
const GLOBALS: u32 = ARENA + 0x300;
const POOL: u32 = ARENA + 0x700;
const ROWS: u32 = ARENA + 0x1000;
const DESCRIPTOR: u32 = ARENA + 0x1800;
const EFFECT: u32 = ARENA + 0x2000;
const AUX_HANDLE: u32 = 0xa8cd_0002;
const VOICE: u32 = 0xc975_0010;

fn setup(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    cpu.gpr[0] = REQUEST;
    for (at, value) in [
        (STACK + 4, TAG),
        (points::SOUND_GLOBALS.base, GLOBALS),
        (0x51ebe0, POOL),
        (POOL + 0x44, ROWS),
        (REQUEST, (seed & 1) << 8),
        (REQUEST + 0x98, 0x8842_abcd),
        (DESCRIPTOR + 8, u32::from(seed != 1)),
    ] {
        g.set_u32(at, value).unwrap();
    }
    for i in 0..3 {
        g.set_u8(GLOBALS + 0x78 + i, u8::from(seed != 10 + i))
            .unwrap();
    }
    g.set_u16(REQUEST + 4, 0x8596).unwrap();
    g.set_u8(EFFECT + 8, 0x81).unwrap();
}

fn reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    let sp = cpu.gpr[4];
    match target {
        0x21dd30 => {
            let output = g.u32(sp + 8).unwrap();
            g.set_u32(output, DESCRIPTOR).unwrap();
            cpu.gpr[0] = u32::from(!matches!(seed, 0 | 5 | 7));
            8
        }
        0x16b840 => {
            cpu.gpr[0] = if seed == 2 { u32::MAX } else { AUX_HANDLE };
            0
        }
        0x18d090 => {
            cpu.gpr[0] = if seed == 3 { 0 } else { EFFECT };
            0
        }
        0x21dc60 => 0,
        0x126c30 => {
            assert_eq!(g.u32(sp + 4).unwrap(), TAG);
            let permutation = g.u32(sp + 8).unwrap();
            g.set_u32(permutation, 11).unwrap();
            cpu.gpr[0] = u32::from(!matches!(seed, 3 | 5));
            12
        }
        0x126ec0 => {
            let rule = g.u32(sp + 4).unwrap();
            g.set_u32(rule, 3).unwrap();
            cpu.gpr[0] = u32::from(matches!(seed, 6 | 7));
            4
        }
        0x126000 => {
            for (i, expected) in [TAG, 11, REQUEST, 3].into_iter().enumerate() {
                assert_eq!(g.u32(sp + 4 + i as u32 * 4).unwrap(), expected);
            }
            cpu.gpr[0] = if seed == 8 { u32::MAX } else { VOICE };
            16
        }
        0x21d630 => {
            assert_eq!(g.u32(sp + 4).unwrap(), AUX_HANDLE);
            let row = ROWS + 2 * 20;
            assert_eq!(g.u8(row + 3).unwrap(), 1);
            assert_eq!(g.u32(row + 12).unwrap(), VOICE);
            8
        }
        0x21dcf0 => {
            assert_eq!(cpu.gpr[3], AUX_HANDLE);
            0
        }
        _ => panic!("unexpected scheduled start call {target:#x}"),
    }
}

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry != 0x21d110 {
        return Vec::new();
    }
    (0..13).map(|seed| Case { setup, reply, seed }).collect()
}
