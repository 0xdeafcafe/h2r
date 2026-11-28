use super::*;

const MANAGER: u32 = DATA + 0x1200;
const GRAPH: u32 = DATA + 0x2200;
const TABLE: u32 = DATA + 0x3200;
const OUTPUT: u32 = DATA + 0x4200;
const NODES: u32 = DATA + 0x5200;

fn setup(c: &mut CpuAbi, m: &mut FlatMemory, seed: u32) {
    c.eax = MANAGER;
    c.ecx = MANAGER;
    c.esi = MANAGER;
    c.ebx = OUTPUT;
    c.edx = OUTPUT + 32;
    c.edi = OUTPUT + 64;
    let handle = 0x4521_0000 | seed;
    word(m, guest_layout::points::TAG_ENTRIES.base, TABLE);
    word(m, TABLE + 8 + 16 * seed, GRAPH);
    word(m, MANAGER + 0x68, handle);
    for i in 0..3 {
        word(m, MANAGER + 0x70 + 4 * i, 0x2233_1000 + seed * 17 + i);
        word(m, NODES + 32 * i, 0x3132_3300 + i);
    }
    word(m, GRAPH + 12, 3);
    word(m, GRAPH + 16, NODES);
    word(m, GRAPH + 24, OUTPUT + 128);
    for (i, value) in [MANAGER, OUTPUT, 2, 3, OUTPUT + 180, OUTPUT + 184]
        .into_iter()
        .enumerate()
    {
        word(m, STACK + 4 + i as u32 * 4, value);
    }
}

fn wrapper_arguments(h: &mut Harness<'_>) {
    for (entry, target, arguments) in [
        (0x1cbce0, 0x1dcfa0, 0),
        (0x1cbd10, 0x1dd0b0, 8),
        (0x1cbd50, 0x1dd1c0, 16),
        (0x1cbd80, 0x1dd290, 24),
        (0x1cbdb0, 0x1dd4c0, 12),
    ] {
        for seed in 0..2 {
            h.case(
                entry,
                seed,
                |c, m| {
                    setup(c, m, seed);
                    if entry == 0x1cbce0 && seed == 1 {
                        // The first out-field overlaps the next source name.
                        c.ecx = MANAGER + 0x74 - 12;
                    }
                },
                |callee, _, c, _| {
                    assert_eq!(target, callee);
                    c.eax = 0x11ac_0000 | seed;
                    c.ecx = 0x1430_0000 | seed;
                    c.edx = 0x7712_0000 | seed;
                    arguments
                },
            );
        }
    }
}

fn leaf_queries(h: &mut Harness<'_>) {
    for entry in [0x1cbd30, 0x1cbde0, 0x1cbe20, 0x1cbe90] {
        for seed in 0..3 {
            h.case(
                entry,
                seed,
                |c, m| {
                    setup(c, m, seed);
                    match entry {
                        0x1cbde0 => {
                            c.esi = if seed == 0 { 0x3132_3301 } else { 0x7575 };
                            if seed == 2 {
                                word(m, GRAPH + 12, u32::MAX);
                            }
                        }
                        0x1cbe20 => c.eax = if seed == 2 { u32::MAX } else { seed },
                        0x1cbe90 if seed == 2 => word(m, TABLE + 8 + 16 * seed, 0),
                        _ => word(m, GRAPH + 12, 0x8a76),
                    }
                },
                |t, _, _, _| panic!("unexpected leaf call {t:x}"),
            );
        }
    }
}

fn actions(h: &mut Harness<'_>) {
    for entry in [
        0x1cb920, 0x1cb970, 0x1cb9d0, 0x1cba80, 0x1cbad0, 0x1cbcb0, 0x1cbe50,
    ] {
        for seed in 0..3 {
            h.case(
                entry,
                seed,
                |c, m| {
                    setup(c, m, seed);
                    match entry {
                        0x1cb920 | 0x1cb9d0 if seed == 2 => word(m, MANAGER + 0x68, u32::MAX),
                        0x1cb970 => c.esi = OUTPUT,
                        0x1cba80 => word(m, STACK + 8, if seed == 0 { u32::MAX } else { 0x30001 }),
                        0x1cbad0 => {
                            if seed == 1 {
                                c.ecx = u32::MAX;
                                word(m, STACK + 12, u32::MAX);
                            }
                        }
                        0x1cbe50 => word(
                            m,
                            STACK + 8,
                            [0xffff_1000, 0x1234_ffff, 0x30001][seed as usize],
                        ),
                        _ => {}
                    }
                },
                |callee, _, c, m| {
                    let result = 0xc041_0001 + seed;
                    match callee {
                        0x1dd560 => {
                            c.eax = if seed == 0 { GRAPH + 0x200 } else { 0 };
                            12
                        }
                        0x1ce180 => {
                            c.eax = 0x71ef_f200;
                            12
                        }
                        0x1cc3f0 => {
                            // The final argument points to the first hidden return candidate.
                            let dst = m.read_u32(c.esp + 24).unwrap();
                            word(m, dst, result);
                            c.eax = u32::from(seed != 1);
                            24
                        }
                        0x1cb520 => {
                            let temp = m.read_u32(c.esp + 4).unwrap();
                            word(m, temp + 4, result);
                            c.eax = u32::from(seed != 1);
                            8
                        }
                        0x1cbad0 => {
                            let dst = m.read_u32(c.esp + 8).unwrap();
                            word(m, dst, if seed == 0 { u32::MAX } else { result });
                            c.eax = dst;
                            12
                        }
                        0x1daea0 => {
                            c.eax = GRAPH + 60;
                            4
                        }
                        0x1cb9d0 => {
                            word(m, c.ebx, if seed == 1 { u32::MAX } else { result });
                            c.eax = c.ebx;
                            4
                        }
                        0x1cb970 => {
                            word(m, c.esi, result);
                            c.eax = c.esi;
                            8
                        }
                        0x1cbb50 => {
                            c.eax = u32::from(seed != 1);
                            24
                        }
                        0x1ccc50 => {
                            c.eax = 0xcb00_1200;
                            0
                        }
                        0x1daff0 => {
                            c.eax = OUTPUT + 13;
                            4
                        }
                        _ => panic!("unexpected action query call {callee:x}"),
                    }
                },
            );
        }
    }
}

fn transitions(h: &mut Harness<'_>) {
    for seed in 0..3 {
        let mut attempt = 0;
        h.case(
            0x1cbb50,
            seed,
            |c, m| {
                setup(c, m, seed);
                // Two entries cycle in each synthetic fallback table.
                for i in [2u32, 3] {
                    half(m, 0x46fc18 + 2 * i, (5 - i) as u16);
                    half(m, 0x4454a0 + 2 * i, (5 - i) as u16);
                }
            },
            |target, _, c, m| match target {
                0x1dc790 => {
                    let out = m.read_u32(c.esp + 8).unwrap();
                    let success = seed == 0 || (seed == 1 && attempt == 2);
                    word(
                        m,
                        out,
                        if success {
                            0x1000_0000 | seed
                        } else {
                            u32::MAX
                        },
                    );
                    c.eax = out;
                    attempt += 1;
                    36
                }
                0x1cb410 => {
                    c.eax = 0xbc00_3201;
                    24
                }
                _ => panic!("unexpected transition call {target:x}"),
            },
        );
    }
}

fn sampling(h: &mut Harness<'_>) {
    for seed in 0..3 {
        h.case(
            0x1cbec0,
            seed,
            |c, m| {
                setup(c, m, seed);
                word(m, STACK + 4, 0x40002);
                word(m, STACK + 8, if seed < 2 { 0 } else { 1.25f32.to_bits() });
                word(m, STACK + 12, OUTPUT);
                // Distinguishable constants with a positive origin threshold.
                word(m, 0x45dcfc, 3.0f32.to_bits());
                word(m, 0x45dbdc, 0.25f32.to_bits());
            },
            |target, _, c, _| match target {
                0x1dd8f0 => {
                    c.eax = u32::from(seed == 0);
                    8
                }
                0x1cbe50 => {
                    c.eax = OUTPUT + seed * 12;
                    8
                }
                0x279d80 => {
                    c.eax = 0xcafe;
                    32
                }
                _ => panic!("unexpected sampler call {target:x}"),
            },
        );
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    wrapper_arguments(h);
    leaf_queries(h);
    actions(h);
    transitions(h);
    sampling(h);
}
