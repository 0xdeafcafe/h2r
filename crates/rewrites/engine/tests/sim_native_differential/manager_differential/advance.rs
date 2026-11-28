use super::*;

const MANAGER: u32 = DATA + 0x1000;
const OUT: u32 = DATA + 0x2000;
const MASK: u32 = DATA + 0x2100;
const CLIP: u32 = DATA + 0x3000;

fn setup_channels(m: &mut FlatMemory, seed: u32) {
    m.bytes[0x46fbf5] = u8::from(seed == 0 || seed == 1);
    m.bytes[0x46fbf4] = u8::from(seed != 3 && seed != 4);
    for (offset, index) in [(0, 1u32), (0x20, 2), (0x40, 3)] {
        word(
            m,
            MANAGER + offset,
            if seed == 4 || (seed == 1 && offset == 0x40) {
                u32::MAX
            } else {
                index
            },
        );
        word(m, MANAGER + offset + 4, 0x68000 + index);
    }
    if seed == 9 {
        half(m, MANAGER + 0x20 + 6, u16::MAX);
    }
    let weight = match seed {
        1 => 1.0f32,
        2 => 0.375,
        5 => 0.0,
        6 => f32::NAN,
        7 => 0.90,
        8 => 0.015625,
        _ => 0.625,
    };
    word(m, MANAGER + 0x80, weight.to_bits());
    word(m, 0x45dc78, 0.90f32.to_bits());
    word(m, 0x45dbdc, 0.015625f32.to_bits());
    word(m, 0x4687a4, OUT + 0x200);
    for (i, v) in [1.0f32, -2.5, 0.03125].into_iter().enumerate() {
        word(m, OUT + 0x200 + 4 * i as u32, v.to_bits());
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    for seed in 0..2 {
        h.case(
            0x1cd4e0,
            seed,
            |c, m| {
                c.esi = MANAGER;
                m.bytes[(MANAGER + 0x67) as usize] = if seed == 0 { 2 } else { 0 };
            },
            |target, site, c, _| {
                assert_eq!(target, 0x1d9320);
                c.eax = if site == 0x1cd4e9 { 0x1201 } else { 0x3440 };
                // The first timer helper can point ECX at a different record.
                c.ecx = OUT;
                0
            },
        );
    }
    for seed in 0..10 {
        h.case(
            0x1cd510,
            seed,
            |c, m| {
                setup_channels(m, seed);
                c.ecx = MANAGER;
                c.eax = if seed == 4 { STACK - 4 } else { OUT };
            },
            |_, _, _, _| unreachable!("selection is a leaf"),
        );

        h.case(
            0x1cd590,
            seed,
            |c, m| {
                setup_channels(m, seed);
                c.esi = 2;
                word(m, STACK + 4, MANAGER);
                word(m, STACK + 12, 0.8125f32.to_bits());
                word(m, STACK + 16, MASK);
                word(m, STACK + 20, OUT);
                word(m, MASK, 1);
            },
            |target, _, c, m| match target {
                0x1c6f60 => {
                    let dest = m.read_u32(c.esp + 16).unwrap();
                    word(m, dest, 0.33f32.to_bits());
                    c.ecx = 0x10123456;
                    16
                }
                0x1cdf00 => {
                    c.eax = MANAGER;
                    c.xmm[0] = [0x43; 16];
                    c.xmm[0][..4].copy_from_slice(&0.3125f32.to_le_bytes());
                    0
                }
                0x1ce110 => 4,
                _ => panic!("sample_channels unexpected callee {target:x}"),
            },
        );

        h.case(
            0x1cd6c0,
            seed,
            |c, m| {
                setup_channels(m, seed);
                c.ebx = MANAGER;
                c.esi = OUT;
                c.eax = if seed == 6 { 1 } else { u32::MAX };
                word(m, guest_layout::points::OBJECTS.base, DATA + 0x4000);
                word(m, DATA + 0x4044, DATA + 0x5000);
                word(m, DATA + 0x5014, DATA + 0x6000);
                m.bytes[(DATA + 0x60b3) as usize] = 1;
                word(m, STACK + 4, OUT + 0x40);
            },
            |target, site, c, m| match target {
                0x1cdf00 => {
                    c.eax = MANAGER;
                    c.xmm[0][..4].copy_from_slice(&0.625f32.to_le_bytes());
                    0
                }
                0x1c6d00 => {
                    let p = m.read_u32(c.esp + 4).unwrap();
                    let base = if site == 0x1cd7a5 { 1.75f32 } else { -2.375 };
                    for i in 0..3 {
                        word(m, c.edi + 4 * i, (base + i as f32).to_bits());
                    }
                    word(m, p, (base * 0.375).to_bits());
                    4
                }
                _ => panic!("channel_motion unexpected callee {target:x}"),
            },
        );
    }
    for seed in 0..5 {
        h.case(
            0x1cd8a0,
            seed,
            |c, m| {
                c.eax = u32::MAX;
                c.esi = MANAGER;
                word(m, MANAGER, 7);
                word(m, MANAGER + 4, 0x20003);
                m.bytes[0x46fbf4] = 1;
                m.bytes[(MANAGER + 0x6e) as usize] = u8::from(seed == 3);
                m.bytes[(CLIP + 0x11) as usize] = u8::from(seed != 4);
                half(m, CLIP + 0x14, 7);
                half(m, CLIP + 0x114, 13);
                word(m, MANAGER + 0x80, 0.375f32.to_bits());
                word(m, 0x45dcfc, 1.75f32.to_bits());
                word(m, STACK + 4, OUT);
            },
            |target, site, c, m| match target {
                0x1c6440 => {
                    c.eax = if c.ecx == MANAGER { CLIP } else { CLIP + 0x100 };
                    0
                }
                0xb6760 => {
                    c.eax = 0x34560001;
                    0
                }
                0x1c7380 => {
                    for i in 0..3 {
                        word(m, c.edi + 4 * i, (i as f32 * 0.375).to_bits());
                    }
                    0
                }
                0x20ab60 => {
                    c.eax = OUT;
                    0
                }
                0x24590 => {
                    let value = match site {
                        0x1cd966 => 0.375f32,
                        0x1cd980 => {
                            if seed == 0 {
                                0.125
                            } else {
                                2.375
                            }
                        }
                        0x1cd9b9 => 0.625,
                        _ => unreachable!(),
                    };
                    let mut stack = x87::abi::X87Abi {
                        control: c.x87.control,
                        status: c.x87.status,
                        tag: c.x87.tag,
                        st: c.x87.st,
                        ..Default::default()
                    };
                    let status = x87::abi::ld_f32(&mut stack, value.to_bits());
                    assert_eq!(status & x87::abi::status::MASK, x87::abi::status::OK);
                    c.x87.status = stack.status;
                    c.x87.tag = stack.tag;
                    c.x87.st = stack.st;
                    0
                }
                _ => panic!("blend_rate unexpected callee {target:x}"),
            },
        );
    }
}
