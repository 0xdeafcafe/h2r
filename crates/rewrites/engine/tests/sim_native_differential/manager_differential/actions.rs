use super::*;

const MANAGER: u32 = DATA + 0x2000;
const DEST: u32 = DATA + 0x3000;
const TAG_TABLE: u32 = DATA + 0x4000;
const GRAPH: u32 = DATA + 0x5000;
const RECORD: u32 = DATA + 0x6000;

fn state(c: &mut CpuAbi, m: &mut FlatMemory, seed: u32) {
    c.eax = MANAGER;
    c.esi = DEST;
    word(m, guest_layout::points::TAG_ENTRIES.base, TAG_TABLE);
    word(m, MANAGER + 104, 0xace00003);
    word(m, TAG_TABLE + 3 * 16 + 8, GRAPH);
    for i in 0..4 {
        word(m, MANAGER + 112 + 4 * i, 0xab300000 + seed * 16 + i);
    }
}

fn motion(h: &mut Harness<'_>) {
    for seed in 0..3 {
        for entry in [0x1cbf50, 0x1cbfd0] {
            h.case(
                entry,
                seed,
                |c, m| {
                    c.ebx = if seed == 2 { STACK - 80 } else { DEST };
                    // The previous/next records, their order and destination
                    // overlap all matter when REP follows the guest DF.
                    word(m, 0x4687d8, RECORD + 32);
                    for i in 0..16 {
                        word(m, RECORD + i * 4, ((i as f32 - 4.0) / 16.0).to_bits());
                    }
                    if seed == 1 {
                        c.eflags |= 1 << 10;
                    }
                    let frame = [0.0f32, 3.5, -4.0][seed as usize];
                    for (i, val) in [MANAGER, 0xc012, frame.to_bits(), 0.25f32.to_bits(), 1]
                        .into_iter()
                        .enumerate()
                    {
                        word(m, STACK + 4 + 4 * i as u32, val);
                    }
                },
                |target, site, c, m| match target {
                    0x1cbec0 => {
                        let out = m.read_u32(c.esp + 12).unwrap();
                        for i in 0..3 {
                            let bas = if site == 0x1cc02f { -1.25 } else { 0.75 };
                            word(m, out + 16 + 4 * i, ((i as f32 + bas) * 0.25).to_bits());
                        }
                        12
                    }
                    0x141e10 => 0,
                    _ => panic!("unexpected root-motion child {target:x}"),
                },
            );
        }
    }
}

// Stub the graph query's output through its real second argument, then
// return it in EAX. Cases select each of the three fallbacks in order.
fn lookups(h: &mut Harness<'_>) {
    for entry in [0x1cc0a0, 0x1cc150, 0x1cc1a0, 0x1cc260] {
        for seed in 0..4 {
            h.case(
                entry,
                seed,
                |c, m| {
                    state(c, m, seed);
                    if entry == 0x1cc0a0 {
                        c.esi = MANAGER;
                    }
                    c.edx = seed;
                    word(m, STACK + 4, 0x93f50000 + seed);
                },
                |target, site, c, m| {
                    assert_eq!(target, 0x1dbc80);
                    let out = m.read_u32(c.esp + 8).unwrap();
                    let miss = match site {
                        0x1cc0d9 => seed >= 1,
                        0x1cc103 => seed >= 2,
                        0x1cc136 => seed == 3,
                        _ => false,
                    };
                    let value = if miss {
                        u32::MAX
                    } else {
                        (0x1234 + seed) << 16 | 1
                    };
                    word(m, out, value);
                    c.eax = out;
                    c.ecx = 0x98760000 ^ site;
                    c.edx = seed.wrapping_sub(site);
                    32
                },
            );
        }
    }
}

fn names(h: &mut Harness<'_>) {
    for seed in 0..4 {
        h.case(
            0x1cc2f0,
            seed,
            |c, m| {
                state(c, m, seed);
                // Select the caller's current name, the image's placeholder,
                // or explicit names. Read the identifiers from the image.
                let inherit = m.read_u32(0x1cc2f1).unwrap();
                let placeholder = m.read_u32(0x1cc318).unwrap();
                let name = [inherit, placeholder, 0x52ab, inherit][seed as usize];
                c.eax = name;
                c.ecx = name;
                c.edx = name;
                word(m, STACK + 4, MANAGER);
                word(m, STACK + 8, name);
                if seed == 3 {
                    for i in 0..4 {
                        word(m, MANAGER + 112 + 4 * i, u32::MAX);
                    }
                }
            },
            |target, _, c, m| {
                assert_eq!(target, 0x1dd560);
                c.eax = if seed == 2 { 0 } else { RECORD };
                word(m, RECORD + 4, if seed == 3 { u32::MAX } else { 0x9712 });
                12
            },
        );
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    motion(h);
    lookups(h);
    names(h);
    h.case(
        0x1cc2b0,
        1,
        |c, m| {
            c.edx = 0x9802;
            word(m, STACK + 4, MANAGER);
            word(m, STACK + 8, 0x321);
        },
        |target, _, _, _| {
            assert_eq!(target, 0x1d9470);
            8
        },
    );
    h.case(
        0x1cc2d0,
        2,
        |c, m| state(c, m, 2),
        |_, _, _, _| unreachable!(),
    );
}
