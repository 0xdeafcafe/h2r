use super::*;

const MANAGER: u32 = DATA + 0x1800;
const CLIP: u32 = DATA + 0x1a00;
const OUT: u32 = DATA + 0x1c00;
const OBJECT_TABLE: u32 = DATA + 0x2000;
const POOL: u32 = DATA + 0x3000;
const OBJECT: u32 = DATA + 0x4000;
const CLOCK: u32 = DATA + 0x5000;

fn vec3(m: &mut FlatMemory, at: u32, values: [f32; 3]) {
    for (i, v) in values.into_iter().enumerate() {
        word(m, at + i as u32 * 4, v.to_bits());
    }
}

fn movement(h: &mut Harness<'_>) {
    for seed in 0..6 {
        h.case(
            0x1cdb00,
            seed,
            |c, m| {
                c.eax = MANAGER;
                word(m, STACK + 4, if seed == 0 { u32::MAX } else { 3 });
                word(m, STACK + 8, OUT);
                word(m, guest_layout::points::OBJECTS.base, OBJECT_TABLE);
                word(m, OBJECT_TABLE + 68, POOL);
                word(m, POOL + 3 * 12 + 8, OBJECT);
                m.bytes[(OBJECT + 179) as usize] = u8::from(seed == 1);
                m.bytes[(MANAGER + 110) as usize] = u8::from(seed == 2);
                m.bytes[0x46fbf4] = 1;
                word(m, MANAGER, 4);
                half(m, MANAGER + 6, 3);
                m.bytes[(CLIP + 17) as usize] = 5;
                half(m, CLIP + 20, 42);
                word(m, 0x510c54, CLOCK);
                half(m, CLOCK + 2, 30);
                let sample = if seed == 3 {
                    0.0
                } else {
                    seed as f32 * 0.25 + 0.125
                };
                vec3(m, OUT, [sample, sample * 0.5, sample * 0.125]);
                word(m, MANAGER + 128, 0x3eaa_1234);
            },
            |target, _, c, m| {
                match target {
                    0x1c6440 => c.eax = CLIP,
                    0xb6760 => c.eax = 0x9876_5401,
                    0x1c7380 => {
                        word(m, c.edi, 0x1234_5678);
                        word(m, c.edi + 4, 0xffff_4321);
                    }
                    0x20ab60 => vec3(m, c.eax, [0.375, -0.7, 0.0625]),
                    0x1cd8a0 => {
                        c.eax = 0xd234_1200 | (seed & 1);
                        return 4;
                    }
                    _ => panic!("unexpected motion call {target:#x}"),
                }
                0
            },
        );
    }
}

fn samples(h: &mut Harness<'_>) {
    for seed in 0..7 {
        h.case(
            0x1cdcd0,
            seed,
            |c, m| {
                c.edi = MANAGER;
                // Case six overlaps the second components of the output and
                // source frames to exercise the load-before-store order.
                word(m, STACK + 4, if seed == 6 { STACK - 32 } else { OUT });
                word(m, STACK + 8, OUT + 16);
                m.bytes[0x46fbf5] = u8::from(seed == 0);
                m.bytes[0x46fbf4] = u8::from(seed != 3 && seed != 4);
                for off in [0, 32, 64] {
                    word(m, MANAGER + off, if seed == 4 { u32::MAX } else { 3 });
                    half(m, MANAGER + off + 6, 2);
                }
                let weight = match seed {
                    1 => 1.25,
                    5 => 0.0,
                    _ => 0.375f32,
                };
                word(m, MANAGER + 128, weight.to_bits());
                word(m, MANAGER + 24, 0.25f32.to_bits());
                word(m, MANAGER + 28, (-0.75f32).to_bits());
            },
            |target, site, c, m| match target {
                0x1cdf00 => {
                    c.xmm[0] = u128::from(0.425f32.to_bits()).to_le_bytes();
                    0
                }
                0x1c6c00 => {
                    let v = if site == 0x1cddba { 0.75 } else { -0.125 };
                    let a = m.read_u32(c.esp + 4).unwrap();
                    vec3(m, a, [v, -v, v * 0.25]);
                    vec3(m, c.esi, [0.5 * v, 2.0 * v, 3.0 * v]);
                    c.eax = if seed == 6 { 0xabc4_99f1 } else { 0xabc4_99ff };
                    4
                }
                0x1c6c80 => {
                    vec3(m, c.ebx, [1.5, 0.125, -3.0]);
                    4
                }
                _ => panic!("unexpected sample call {target:#x}"),
            },
        );
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    movement(h);
    samples(h);
    for (i, weight) in [0.0f32, -1.0, 0.35, 3.0, f32::from_bits(0x7f80_0001), -0.0]
        .into_iter()
        .enumerate()
    {
        h.case(
            0x1cdf00,
            i as u32,
            |c, m| {
                c.eax = MANAGER;
                word(m, MANAGER + 128, weight.to_bits());
            },
            |target, _, _, _| panic!("leaf called {target:#x}"),
        );
    }
    for seed in 0..4 {
        h.case(
            0x1cdf50,
            seed,
            |c, m| {
                c.edi = MANAGER;
                for (i, off) in [0, 32].into_iter().enumerate() {
                    word(m, MANAGER + off, if seed == 3 { u32::MAX } else { 2 });
                    half(
                        m,
                        MANAGER + off + 6,
                        if i == 0 && seed == 1 { u16::MAX } else { 3 },
                    );
                    m.bytes[(MANAGER + off + 17) as usize] = 0x5a;
                    m.bytes[(MANAGER + off + 18) as usize] = u8::from(i == 0 || seed == 2);
                }
            },
            |target, site, _, m| {
                assert_eq!(target, 0x1c6500);
                if site == 0x1cdfbe && seed == 2 {
                    // A child reactivates the override, which reset must reread.
                    word(m, MANAGER + 64, 4);
                    half(m, MANAGER + 70, 5);
                    m.bytes[(MANAGER + 82) as usize] = 1;
                }
                0
            },
        );
    }
}
