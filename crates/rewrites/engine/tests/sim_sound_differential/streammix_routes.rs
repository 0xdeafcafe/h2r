//! Drive the routing decisions inside the full stream update. Each panner
//! gets a different vtable, so the transcript also checks driver reloads
//! after a callback replaces the panner.
use super::*;

const DRIVER: u32 = ARENA;
const OTHER_DRIVER: u32 = ARENA + 0x4000;
const PARAMS: u32 = ARENA + 0x8000;
const PANNER: u32 = ARENA + 0x8100;
const OTHER_PANNER: u32 = ARENA + 0x8200;
const METHODS: u32 = ARENA + 0x8300;
const OTHER_METHODS: u32 = ARENA + 0x8400;
const PANNER_CALL: u32 = ARENA + 0x8500;

pub(super) fn cases() -> Vec<Case> {
    (0..12).map(|seed| Case { setup, reply, seed }).collect()
}

fn submix(seed: u32) -> i16 {
    match seed {
        0 => -1,                   // plain stream
        1 | 6 | 9 | 11 => 1,       // static submix
        _ => 2,                    // dynamic submix
    }
}

fn setup(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    cpu.gpr[7] = PARAMS;
    let stream = 3;
    let flags = match seed {
        4 | 5 => 1,                 // stop the secondary buffer if playing
        6.. => 0x20,                // four independent speaker levels
        _ => 0,
    };
    for (driver, panner, methods, method_bias) in [
        (DRIVER, PANNER, METHODS, 0),
        (OTHER_DRIVER, OTHER_PANNER, OTHER_METHODS, 0x40),
    ] {
        g.set_u16(driver + 8, 2).unwrap();
        g.set_u8(driver + 1, 3).unwrap();
        g.set_u32(driver + 0x2ab4, panner).unwrap();
        g.set_u32(panner, methods).unwrap();
        for offset in [0xc, 0x10, 0x14, 0x18] {
            g.set_u32(methods + offset, PANNER_CALL + method_bias + offset).unwrap();
        }
    }
    let dynamic = DRIVER + 0x1a18 + 2 * 0x3c;
    g.set_u32(dynamic, 6).unwrap();
    g.set_u32(dynamic + 0x34, 0x110).unwrap();
    g.set_u32(dynamic + 0x38, 0x220).unwrap();
    let stream_record = DRIVER + 0xc + stream * 0x34;
    g.set_u32(stream_record + 0x24, 0x330).unwrap();
    g.set_u8(stream_record + 1, 0x8).unwrap();
    g.set_u32(0x51ebe4, DRIVER).unwrap();
    g.set_u32(STACK + 4, stream).unwrap();
    g.set_u32(STACK + 8, ARENA + 0x8800).unwrap();
    g.set_u32(STACK + 12, 1).unwrap();
    g.set_u32(PARAMS + 0xc, (-2.25f32).to_bits()).unwrap();
    g.set_u32(PARAMS + 0x14, (-5.5f32).to_bits()).unwrap();
    g.set_u32(PARAMS + 0x18, (-12.5f32).to_bits()).unwrap();
    g.set_u32(PARAMS + 0x28, (-1.25f32).to_bits()).unwrap();
    g.set_u16(PARAMS + 0x24, submix(seed) as u16).unwrap();
    g.set_u32(PARAMS + 0x30, flags).unwrap();
    let levels: [f32; 4] = match seed {
        8 => [f32::NEG_INFINITY, -120., -1000., -65.],
        9 => [-5., -20., f32::from_bits(0x7fc0_1234), -31.],
        10 => [10., f32::INFINITY, -3., 0.],
        11 => [0.; 4],
        _ => [-3., -7.25, -19., -85.],
    };
    for (index, level) in levels.into_iter().enumerate() {
        g.set_u32(PARAMS + 0x38 + 4 * index as u32, level.to_bits()).unwrap();
    }
}

fn reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    let sp = cpu.gpr[4];
    let ret = g.u32(sp).unwrap();
    let dynamic = DRIVER + 0x1a18 + 2 * 0x3c;
    if (PANNER_CALL..PANNER_CALL + 0x60).contains(&target) {
        if seed >= 6 {
            g.set_u32(0x51ebe4, OTHER_DRIVER).unwrap();
        }
        return 16;
    }
    match target {
        0x12aff0 => {
            // The clamp returns a normalized gain in XMM0.
            cpu.xmm[0] = u128::from(0.8125f32.to_bits());
            12
        }
        0x21e230 => {
            let sends = cpu.gpr[6];
            let count = g.u32(sends + 0x40).unwrap();
            g.set_u32(sends + count * 8, cpu.gpr[1]).unwrap();
            g.set_u32(sends + count * 8 + 4, 25 + count).unwrap();
            g.set_u32(sends + 0x40, count + 1).unwrap();
            4
        }
        0x37b83f | 0x37b7e3 => {
            let desc = g.u32(sp + 8).unwrap();
            g.set_u32(desc, 1).unwrap();
            g.set_u32(desc + 4, 7).unwrap();
            g.set_u32(desc + 8, (-250i32) as u32).unwrap();
            if ret == 0x21ff56 {
                g.set_u32(dynamic + 0x38, 0x221).unwrap();
            } else if ret == 0x21ff67 {
                g.set_u32(dynamic + 0x34, 0x111).unwrap();
            }
            8
        }
        0x37b75b => {
            let status = g.u32(sp + 8).unwrap();
            let playing = matches!(seed, 3 | 4);
            g.set_u32(status, u32::from(playing)).unwrap();
            g.set_u32(dynamic + 0x38, 0x222).unwrap();
            8
        }
        0x37b6df | 0x37b71b => 16,
        0x2215d0 | 0x2216f0 | 0x37b66f | 0x37c70a | 0x37b7ff => 8,
        0x2201f0 => 4,
        _ => panic!("unexpected stream routing callback {target:#x} at {ret:#x}"),
    }
}
