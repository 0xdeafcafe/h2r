//! Transition search and channel installation with mutable graph callbacks.
use super::*;

const MANAGER: u32 = DATA + 0x1000;
const TABLE: u32 = DATA + 0x2000;
const GRAPH: u32 = DATA + 0x3000;
const CLIP: u32 = DATA + 0x4000;
const EVENTS: u32 = DATA + 0x5000;
const OUTPUT: u32 = DATA + 0x6000;

fn machine(m: &mut FlatMemory, seed: u32) {
    word(m, guest_layout::points::TAG_ENTRIES.base, TABLE);
    word(m, TABLE + 3 * 16 + 8, GRAPH);
    for off in [0, 0x20, 0x40] {
        word(m, MANAGER + off, 0x8000_0003);
        word(m, MANAGER + off + 4, 0x0002_ffff);
        m.bytes[(MANAGER + off + 0x12) as usize] = 1;
    }
    word(m, MANAGER + 0x68, 0xfa11_0003);
    for i in 0..4 {
        word(m, MANAGER + 0x70 + i * 4, 0xd100 + seed * 16 + i);
    }
    word(m, MANAGER + 0x1c, 3.25f32.to_bits());
    word(m, MANAGER + 0x80, 0.75f32.to_bits());
    half(m, CLIP + 0x14, 12);
    m.bytes[(CLIP + 0x18) as usize] = 0;
    word(m, CLIP + 0x4c, 4);
    word(m, CLIP + 0x50, EVENTS);
    for i in 0..4 {
        half(m, EVENTS + i * 4, 5 + i as u16);
        half(m, EVENTS + i * 4 + 2, 4);
    }

    // Synthetic thresholds and remapping table exercise both clamping
    // branches without embedding private image operands.
    word(m, 0x45dbdc, 0.0625f32.to_bits());
    word(m, 0x45dbd8, 0.001f32.to_bits());
    word(m, 0x45dc78, 0.875f32.to_bits());
    word(m, 0x45dc74, 0.75f32.to_bits());
    word(m, 0x45dd24, 0.125f32.to_bits());
    word(m, 0x45dbc0, 1.0f32.to_bits());
    m.bytes[0x46fbf4] = 1;
    m.bytes[0x46fbf5] = 1;
    for i in 0..4 {
        word(
            m,
            0x46fbf8 + i * 8,
            if i == seed % 4 { 0xd103 + seed * 16 } else { i },
        );
        word(m, 0x46fbfc + i * 8, 0x40 + i);
    }
}

fn find(h: &mut Harness<'_>) {
    for seed in 0..4 {
        h.case(
            0x1ccda0,
            seed,
            |c, m| {
                machine(m, seed);
                c.ebx = MANAGER;
                word(m, STACK + 4, OUTPUT);
                word(m, STACK + 8, 0x9000 + seed);
                word(m, STACK + 12, 0x180 + seed);
                word(m, STACK + 16, OUTPUT + 4);
                if seed == 0 {
                    m.bytes[0x46fbf5] = 0;
                }
                if seed == 1 {
                    half(m, MANAGER + 6, u16::MAX);
                }
            },
            |target, site, c, m| match (target, site) {
                (0x1dc8c0, _) => {
                    let dest = m.read_u32(c.esp + 8).unwrap();
                    word(
                        m,
                        dest,
                        if site == 0x1cce8d {
                            0x0020_0001
                        } else {
                            0x0040_0002 + c.ebp
                        },
                    );
                    48
                }
                (0x1daea0, _) => {
                    c.eax = CLIP;
                    4
                }
                (0x1dade0, _) => {
                    c.eax = if seed == 3 { 0 } else { 3 };
                    0
                }
                _ => panic!("unexpected transition search call {target:x} at {site:x}"),
            },
        );
    }
}

fn install(h: &mut Harness<'_>) {
    for seed in 0..4 {
        h.case(
            0x1cd070,
            seed,
            |c, m| {
                machine(m, seed);
                c.ecx = MANAGER;
                c.eax = 0x3230 + seed;
                for (i, v) in [
                    0x8100 + seed,
                    0x8200,
                    0x8300,
                    [0x30, 0x450, 0x204, 4][seed as usize],
                    0x4100,
                ]
                .into_iter()
                .enumerate()
                {
                    word(m, STACK + 4 + i as u32 * 4, v);
                }
            },
            |target, site, c, m| match (target, site) {
                (0x1daea0, _) => {
                    c.eax = CLIP;
                    4
                }
                (0x1ccba0, _) => 0,
                (0x1cc3f0, _) => {
                    let dest = m.read_u32(c.esp + 24).unwrap();
                    word(m, dest, 0x1440_1000 + seed);
                    c.eax = u32::from(seed != 3);
                    24
                }
                (0x1ccda0, _) => {
                    let dest = m.read_u32(c.esp + 4).unwrap();
                    let flag = m.read_u32(c.esp + 16).unwrap();
                    word(m, dest, 0x24ff_000a);
                    m.bytes[flag as usize] = 1;
                    c.eax = dest;
                    16
                }
                (0xe6800, _) => {
                    c.eax = 0;
                    0
                }
                (0x1c6440, _) => {
                    c.eax = CLIP;
                    0
                }
                (0x1cccd0, _) => {
                    let out = m.read_u32(c.esp + 20).unwrap();
                    word(m, out, 0x38);
                    word(m, OUTPUT, 0x00a1_01b0);
                    c.eax = OUTPUT;
                    20
                }
                (0x1cb410, _) => {
                    c.eax = 1;
                    24
                }
                (0x1c7300, _) => {
                    c.eax = 1;
                    0
                }
                (0x1cb3b0, _) => {
                    word(m, OUTPUT, 0x50);
                    c.eax = OUTPUT;
                    4
                }
                (0x1c6470, _) => {
                    c.eax = 1;
                    20
                }
                (0x1c6560, _) => 4,
                (0x1ce010, _) => 0,
                (0xb6760, _) => {
                    c.eax = 0;
                    0
                }
                (0x1ce180, _) => 12,
                _ => panic!("unexpected transition install call {target:x} at {site:x}"),
            },
        );
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    find(h);
    install(h);
}
