//! Channel selection and three-channel advance, with opaque graph services.
use super::*;

const TAGS: u32 = DATA + 0x1000;
const MANAGER: u32 = DATA + 0x2000;
const CHANNEL: u32 = DATA + 0x3000;
const GRAPH: u32 = DATA + 0x4000;
const ELEMENT: u32 = DATA + 0x5000;
const RESULT: u32 = DATA + 0x6000;

fn setup(m: &mut FlatMemory) {
    word(m, guest_layout::points::TAG_ENTRIES.base, TAGS);
    word(m, TAGS + 16 + 8, GRAPH);
    word(m, MANAGER + 0x68, 1);
    word(m, MANAGER + 0x70, 0x1200);
    word(m, MANAGER + 0x74, 0x1201);
    word(m, MANAGER + 0x78, 0x1202);
    word(m, CHANNEL, 1);
    word(m, CHANNEL + 4, 0x0002_0003);
    word(m, RESULT, 0x0002_0003);
    half(m, ELEMENT + 20, 13);
}

fn answer(target: u32, site: u32, c: &mut CpuAbi, m: &mut FlatMemory) -> u32 {
    match target {
        0x1cd070 => 20,
        0x1db170 | 0x1dbc80 => {
            if site == 0x1cb577 {
                let local = m.read_u32(c.esp + 8).unwrap();
                word(m, local, 0x0002_0003);
            }
            c.eax = RESULT;
            32
        }
        0x1dd560 => {
            c.eax = RESULT - 4;
            12
        }
        0x1dd0b0 => {
            let local = m.read_u32(c.esp + 4).unwrap();
            word(m, local, 0x0002_0003);
            c.eax = local;
            8
        }
        0x1dd630 => {
            c.eax = RESULT;
            16
        }
        0x1cb3b0 => {
            c.eax = RESULT;
            4
        }
        0x1cb410 => {
            c.eax = 1;
            24
        }
        0x1c6470 => {
            c.eax = 1;
            20
        }
        0x1c6560 => 4,
        0x1c68c0 => {
            c.eax = 0x1234_5678;
            12
        }
        0x1d9430 => {
            c.xmm[0] = u128::from(0.25f32.to_bits()).to_le_bytes();
            0
        }
        0x1d9240 => 4,
        0x1daea0 => {
            c.eax = ELEMENT;
            4
        }
        0x1cb5f0 => 16,
        0x1cb680 => 0,
        0x1c66a0 => 20,
        _ => panic!("unexpected channel call {target:#x} at {site:#x}"),
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    h.case(0x1cb0a0, 1, |c, _| c.eax = CHANNEL, answer);
    for seed in 0..3 {
        h.case(
            0x1cb0d0,
            seed,
            |c, m| {
                setup(m);
                c.esi = MANAGER;
                c.edi = 1;
                c.edx = if seed == 0 { u32::MAX } else { 1 };
                half(m, GRAPH + 12, 37);
                word(m, GRAPH + 0x78, if seed == 2 { 33 } else { 37 });
                word(m, STACK + 4, seed);
            },
            answer,
        );
    }
    for seed in 0..7 {
        h.case(
            0x1cb230,
            seed,
            |c, m| {
                setup(m);
                c.esi = CHANNEL;
                word(m, STACK + 4, MANAGER);
                m.bytes[(CHANNEL + 14) as usize] = seed as u8;
                if seed != 0 {
                    word(m, RESULT, 0x0004_0005);
                }
            },
            answer,
        );
    }
    for seed in 0..2 {
        h.case(
            0x1cb3b0,
            seed,
            |c, m| {
                setup(m);
                c.edi = MANAGER;
                c.esi = CHANNEL;
                word(m, STACK + 4, if seed == 0 { 0x0004_0021 } else { u32::MAX });
            },
            answer,
        );
        h.case(
            0x1cb410,
            seed,
            |c, m| {
                setup(m);
                c.eax = MANAGER;
                c.ebx = if seed == 0 { 0x1c } else { 0 };
                for (i, arg) in [CHANNEL, 0x0002_0020, 12, 13, 14, 15]
                    .into_iter()
                    .enumerate()
                {
                    word(m, STACK + 4 + i as u32 * 4, arg);
                }
            },
            answer,
        );
        h.case(
            0x1cb490,
            seed,
            |c, m| {
                setup(m);
                c.eax = MANAGER;
                if seed == 0 {
                    word(m, MANAGER + 0x68, u32::MAX);
                }
                word(m, STACK + 4, CHANNEL);
                word(m, STACK + 8, 0x0002_0020);
            },
            answer,
        );
        for entry in [0x1cb4c0, 0x1cb520] {
            h.case(
                entry,
                seed,
                |c, m| {
                    setup(m);
                    c.esi = MANAGER;
                    c.ecx = u32::MAX;
                    c.edx = u32::MAX;
                    if seed == 1 {
                        word(m, MANAGER + 0x68, u32::MAX);
                    }
                    word(m, STACK + 4, CHANNEL);
                    word(m, STACK + 8, 0x18);
                },
                answer,
            );
        }
        h.case(
            0x1cb5b0,
            seed,
            |c, m| {
                setup(m);
                c.ecx = MANAGER;
                c.edx = CHANNEL;
                m.bytes[(MANAGER + 0x6c) as usize] = seed as u8;
            },
            answer,
        );
        h.case(
            0x1cb5f0,
            seed,
            |c, m| {
                setup(m);
                c.eax = if seed == 0 { 32 } else { 3 };
                m.bytes[(MANAGER + 0x65) as usize] = 1;
                word(m, STACK + 4, MANAGER);
                word(m, STACK + 8, 0.5f32.to_bits());
                // Partial overlap in both DF directions.
                let src = DATA + 0x7000;
                word(m, STACK + 12, src + if seed == 0 { 4 } else { 92 });
                word(m, STACK + 16, src + if seed == 0 { 0 } else { 96 });
                if seed != 0 {
                    c.eflags |= 0x400;
                }
                for n in 0..=(8 * c.eax) {
                    word(m, src + 4 * n, n * 61 + 5);
                }
            },
            answer,
        );
    }
    for seed in 0..3 {
        h.case(
            0x1cb680,
            seed,
            |c, m| {
                setup(m);
                c.eax = MANAGER;
                m.bytes[0x46fbf4] = u8::from(seed != 0);
                word(
                    m,
                    MANAGER + 0x80,
                    [0.0f32, 0.5, 1.0][seed as usize].to_bits(),
                );
                half(m, MANAGER + 0x12, 0x352);
                half(m, MANAGER + 0x32, 0x177);
            },
            answer,
        );
        h.case(
            0x1cb710,
            seed,
            |c, m| {
                setup(m);
                c.esi = MANAGER;
                m.bytes[0x46fbf4] = u8::from(seed != 2);
                for channel in [0, 32, 64] {
                    word(m, MANAGER + channel, if seed == 2 { u32::MAX } else { 1 });
                    word(m, MANAGER + channel + 4, 0x0002_0004);
                }
                word(m, MANAGER + 28, 4.5f32.to_bits());
                m.bytes[(MANAGER + 0x51) as usize] = 10;
                m.bytes[(ELEMENT + 24) as usize] = if seed == 1 { 2 } else { 0 };
                for (i, v) in [0, 1, 2, DATA + 0x7500, DATA + 0x7600]
                    .into_iter()
                    .enumerate()
                {
                    word(m, STACK + 4 + 4 * i as u32, v);
                }
            },
            answer,
        );
    }
}
