//! Synthetic records for the frame walk, game-sound schedule, obstruction
//! query, gain curves and submix descriptors.
use super::*;
use engine::points::announcer_queue as queue;

const GLOBALS: u32 = 0x4e6380;
const BSP: u32 = 0x4e0348;
const TIME: u32 = 0x510c54;
const POOL: u32 = 0x4ed28c;
const MIXER: u32 = 0x51ebe4;
const STATE: u32 = ARENA;
const CLOCK: u32 = ARENA + 0x1000;
const SOURCE: u32 = ARENA + 0x2000;
const WORLD: u32 = ARENA + 0x3000;
const POOL_HEADER: u32 = ARENA + 0x4000;
const ROWS: u32 = ARENA + 0x5000;
const BITS: u32 = ARENA + 0x6000;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let n = match entry {
        0x125b40 | 0x18c060 => 5,
        0x23f000 | 0x23f120 | 0x23f140 => 8,
        0x221da0 | 0x221a70 => 9,
        0x2195f0 | 0x21e270 | 0x21e2d0 => FLOATS.len() as u32,
        0x2211e0 => 3,
        _ => 0,
    };
    (0..n).map(|seed| Case { setup, reply, seed }).collect()
}

const FLOATS: &[f32] = &[
    -256.0,
    -55.25,
    -0.0,
    0.0,
    0.6,
    9.5,
    3800.0,
    88000.0,
    f32::INFINITY,
    f32::from_bits(0x7fc0_0123),
];

fn setup(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    match cpu.eip {
        0x125b40 | 0x18c060 => frame(g, cpu.eip, seed),
        0x23f000 | 0x23f120 | 0x23f140 => announcer(g, cpu, seed),
        0x221da0 | 0x221a70 => obstruction(g, cpu, seed),
        0x2195f0 | 0x21e270 | 0x21e2d0 => {
            g.set_u32(STACK + 4, FLOATS[seed as usize].to_bits())
                .unwrap();
        }
        0x2211e0 => {
            let index = seed + 1;
            cpu.gpr[0] = index;
            g.set_u32(MIXER, STATE).unwrap();
            for (i, offset) in [0x2abc, 0x2ac0, 0x2ac4].into_iter().enumerate() {
                let gain = match seed {
                    0 => 0.0,
                    1 => 0.125 * (i + 1) as f32,
                    _ => 1.75,
                };
                g.set_u32(STATE + offset, gain.to_bits()).unwrap();
            }
            let row = STATE + 0x1a18 + index * 0x3c;
            let obstruction: f32 = [1.0, 0.0, 0.625][seed as usize];
            let occlusion: f32 = [1.0, 0.375, 0.125][seed as usize];
            g.set_u32(row + 0x30, obstruction.to_bits()).unwrap();
            g.set_u32(row + 0x2c, occlusion.to_bits()).unwrap();
            g.set_u32(row + 0x34, 47).unwrap();
            g.set_u32(STACK + 4, 0.25f32.to_bits()).unwrap();
        }
        _ => unreachable!(),
    }
}

fn frame(g: &mut Guest, entry: u32, seed: u32) {
    g.set_u32(GLOBALS, STATE).unwrap();
    g.set_u32(TIME, CLOCK).unwrap();
    g.set_u32(CLOCK + 8, 11).unwrap();
    g.set_u32(0x50210c, CLOCK + 0x80).unwrap();
    g.set_u32(CLOCK + 0xb8, if seed == 1 { i32::MAX as u32 } else { 7 })
        .unwrap();
    for gate in 0..3 {
        g.set_u8(STATE + 0x78 + gate, u8::from(gate != seed || seed >= 3))
            .unwrap();
    }
    g.set_u8(STATE + 0x7b, u8::from(seed == 4)).unwrap();
    g.set_u8(STATE + 0x7e, u8::from(seed >= 3)).unwrap();
    g.set_u16(STATE + 0x208, 2).unwrap();
    if entry == 0x125b40 {
        g.set_u32(0x4e8c20, SOURCE).unwrap();
        for (seat, value) in [48, u32::MAX, 49, u32::MAX].into_iter().enumerate() {
            g.set_u32(SOURCE + 12 + 4 * seat as u32, value).unwrap();
        }
    } else {
        g.set_u32(0x510c50, if seed == 4 { CLOCK + 0x100 } else { 0 })
            .unwrap();
        g.set_u8(CLOCK + 0x105, 1).unwrap();
        g.set_u32(0x4ed288, CLOCK + 0x300).unwrap();
        g.set_u32(CLOCK + 0x324, if seed % 2 == 0 { 13 } else { 7 })
            .unwrap();
        g.set_u32(POOL, POOL_HEADER).unwrap();
        g.set_u32(POOL_HEADER + 0x24, 24).unwrap();
        g.set_u32(POOL_HEADER + 0x38, if seed == 0 { 0 } else { 36 })
            .unwrap();
        g.set_u32(POOL_HEADER + 0x44, ROWS).unwrap();
        g.set_u32(POOL_HEADER + 0x48, BITS).unwrap();
        for (slot, kind, time) in [(0, 1, 4), (7, 4, 11), (31, 4, 12), (33, 2, u32::MAX)] {
            let row = ROWS + 24 * slot;
            let bits = BITS + (slot >> 5) * 4;
            g.set_u32(bits, g.u32(bits).unwrap() | (1 << (slot % 32)))
                .unwrap();
            g.set_u16(row, 7 + slot as u16).unwrap();
            g.set_u8(row + 3, kind).unwrap();
            g.set_u8(row + 4, u8::from(seed == 2)).unwrap();
            g.set_u32(row + 0x10, time).unwrap();
        }
        g.set_u32(STACK + 4, 0.375f32.to_bits()).unwrap();
    }
}

fn announcer(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    g.set_u32(queue::COUNT, [0, 3, 3, 1, 4, 5, 2, 1][seed as usize])
        .unwrap();
    for slot in 0..5 {
        let base = queue::QUEUE + 12 * slot;
        g.set_u32(base, 125 + slot).unwrap();
        g.set_u32(base + 4, 321 + slot).unwrap();
        g.set_u32(base + 8, 935 + slot).unwrap();
    }
    g.set_u8(queue::HEAD_PLAYING, u8::from(seed == 3 || seed == 6))
        .unwrap();
    g.set_u16(
        queue::HEAD_DELAY,
        [0, 1, 0, u16::MAX, u16::MAX, u16::MAX, 0, 0][seed as usize],
    )
    .unwrap();
    if seed == 7 {
        g.set_u32(queue::QUEUE, u32::MAX).unwrap();
    }
    cpu.gpr[1] = 942;
    g.set_u32(STACK + 4, 0x1234_abcd).unwrap();
    g.set_u32(STACK + 8, if seed % 2 == 0 { 321 } else { 847 })
        .unwrap();
}

fn obstruction(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    let index = 1;
    let listener = STATE + 0x88 + 0x48 * index;
    g.set_u32(GLOBALS, STATE).unwrap();
    g.set_u32(BSP, WORLD).unwrap();
    g.set_u32(listener, 15).unwrap();
    g.set_u16(listener + 4, if seed == 1 { u16::MAX } else { 3 })
        .unwrap();
    if cpu.eip == 0x221da0 {
        cpu.gpr[1] = if seed == 0 { u32::MAX } else { index };
        cpu.gpr[2] = SOURCE;
        g.set_u8(SOURCE, u8::from(seed == 2)).unwrap();
        g.set_u8(SOURCE + 3, if seed == 3 { 0x32 } else { 0x31 })
            .unwrap();
        g.set_u32(SOURCE + 0x28, if seed == 4 { 15 } else { 6 })
            .unwrap();
        g.set_u16(
            SOURCE + 0x2c,
            match seed {
                5 => 3,
                8 => u16::MAX,
                _ => 9,
            },
        )
        .unwrap();
        g.set_u32(STACK + 4, 0.5f32.to_bits()).unwrap();
    } else {
        g.set_u32(STACK + 4, index).unwrap();
        g.set_u32(STACK + 8, SOURCE).unwrap();
        g.set_u32(SOURCE, if seed == 0 { 0 } else { 3 }).unwrap();
        for (slot, seat) in [1, 0, 1].into_iter().enumerate() {
            g.set_u32(SOURCE + 4 + 0x48 * slot as u32, seat).unwrap();
        }
        g.set_u32(WORLD + 0x9c, 5).unwrap();
        g.set_u32(WORLD + 0x224, u32::from(seed >= 3)).unwrap();
        g.set_u32(WORLD + 0x228, WORLD + 0x400).unwrap();
        g.set_u32(WORLD + 0x400, 1).unwrap();
        g.set_u32(WORLD + 0x430, WORLD + 0x800).unwrap();
        g.set_u8(WORLD + 0x800, if seed == 4 { u8::MAX } else { 2 })
            .unwrap();
    }
}

fn reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        // Frame service and listener callbacks.
        0x21ec00 | 0x12a0b0 | 0x21f5a0 | 0x1269f0 | 0x221900 | 0x219cc0 | 0x1290d0 | 0x128020
        | 0x1282d0 | 0x129380 | 0x1293f0 | 0x21ee80 | 0x13d230 | 0x18cb10 | 0x18bf90 | 0x1272c0
        | 0x21d390 | 0x16ba40 => 0,
        0x219d90 | 0x21f360 | 0x127400 | 0x21b0e0 | 0x18bb80 | 0x225b60 => 4,
        0x129100 | 0x221a70 | 0x221e30 => 8,
        0x3314b0 => {
            cpu.gpr[0] = 57;
            0
        }
        0x14dde0 => {
            cpu.gpr[0] = if seed == 4 { 0 } else { u32::MAX };
            0
        }
        0x18a7b0 => {
            // A sound update can retire a later source before the scan resumes.
            if seed == 3 {
                g.set_u32(BITS + 4, 0).unwrap();
            }
            4
        }
        0xbadc0 | 0x18d1c0 => {
            cpu.gpr[0] = 0;
            4
        }
        0xb9d20 => {
            cpu.gpr[0] = 0;
            0
        }
        0x189710 => {
            cpu.gpr[0] = if seed == 6 { u32::MAX } else { 538 };
            4
        }
        // Obstruction: cluster mask in ECX, sided sets in EAX and stack arg 1.
        0x249d60 => {
            cpu.xmm[0] = u128::from(if seed == 6 { 0.0f32 } else { 90.0 }.to_bits());
            0
        }
        0x249d10 => {
            g.set_u32(cpu.gpr[1], u32::from(seed >= 4)).unwrap();
            4
        }
        0x18cac0 => {
            cpu.xmm[0] = u128::from(0.375f32.to_bits());
            0
        }
        0x20670 => {
            let masks = g.u32(cpu.gpr[4] + 4).unwrap();
            g.write_bytes(masks, &[0; 128], 1).unwrap();
            16
        }
        0x249c90 => {
            let first = cpu.gpr[0];
            let second = g.u32(cpu.gpr[4] + 8).unwrap();
            g.set_u32(first, if seed % 2 == 0 { 0b01101 } else { 0b10001 })
                .unwrap();
            g.set_u32(second, 0b11010).unwrap();
            8
        }
        // The room/filter setters may re-enter the engine and replace the buffer.
        0x12aff0 => {
            cpu.xmm[0] = u128::from(0.375f32.to_bits());
            12
        }
        0x37c6e5 => {
            g.set_u32(STATE + 0x1a18 + (seed + 1) * 0x3c + 0x34, 98)
                .unwrap();
            cpu.gpr[0] = 17;
            12
        }
        0x21e270 | 0x21e2d0 => {
            cpu.gpr[0] = 541;
            4
        }
        0x37b68b => {
            cpu.gpr[0] = 29;
            8
        }
        _ => panic!("unhandled sound callout {target:#x} seed {seed}"),
    }
}
