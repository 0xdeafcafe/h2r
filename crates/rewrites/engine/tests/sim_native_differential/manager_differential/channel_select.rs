use super::*;

pub(super) fn cases(h: &mut Harness<'_>) {
    for seed in 0..3 {
        h.case(
            0x1ccae0,
            seed,
            |c, m| {
                c.esi = DATA;
                word(m, DATA + 0x68, if seed == 0 { u32::MAX } else { 17 });
                word(m, STACK + 4, 0xa511_0051 ^ seed);
                word(m, STACK + 8, 0xf023_bc31 + seed);
            },
            |target, _, c, _| match target {
                0x1cd070 => {
                    c.eax = 0xaabb_ccdd;
                    0x14
                }
                0x1cb410 => {
                    c.eax = 0xccff_6600 | u32::from(seed == 2);
                    0x18
                }
                0x1ccc50 => {
                    c.eax = 0x54ab_1fa0;
                    0
                }
                _ => panic!("unexpected animation start callee {target:x}"),
            },
        );
    }

    for seed in 0..4 {
        for entry in [0x1ccb40, 0x1ccb80] {
            h.case(
                entry,
                seed,
                |c, m| {
                    c.eax = DATA;
                    c.ecx = DATA;
                    word(m, DATA, if seed == 0 { u32::MAX } else { 0xff7a_0001 });
                    half(m, DATA + 4, 17);
                    half(m, DATA + 6, if seed == 1 { u16::MAX } else { 3 });
                    word(m, 0x4e3b44, DATA + 0x100);
                    word(m, DATA + 0x118, DATA + 0x200);
                    half(m, DATA + 0x314, if seed == 2 { 321 } else { 0xffd3 });
                    c.xmm[0][..4].copy_from_slice(&(-0.45f32).to_le_bytes());
                },
                |target, _, c, _| {
                    assert!(matches!(target, 0x1daea0 | 0x1c6560));
                    c.eax = DATA + 0x300;
                    // The conversion at the duration return preserves these lanes.
                    c.xmm[0] = [0xa7; 16];
                    4
                },
            );
        }
    }

    for entry in [0x1ccba0, 0x1ccc50] {
        h.case(
            entry,
            0x35,
            |c, m| {
                c.eax = DATA;
                for i in 0..24 {
                    word(m, DATA + i * 4, 0xbeef_a110 ^ (i << 9));
                }
            },
            |_, _, _, _| unreachable!("channel reset makes no calls"),
        );
    }

    for seed in 0..6 {
        h.case(
            0x1cccd0,
            seed,
            |c, m| {
                // Four names with distinct values. Cases exercise disabled,
                // absent, matched-none, and both outcomes from the resolver.
                c.ecx = if seed == 1 { 0xa200 } else { 0xa100 + seed % 4 };
                c.ebx = DATA + 0x180;
                for i in 0..4 {
                    word(m, 0x46fbf8 + i * 8, 0xa100 + i);
                    word(
                        m,
                        0x46fbfc + i * 8,
                        if seed == 2 { u32::MAX } else { 0x100 + i },
                    );
                }
                m.bytes[0x46fbf4] = u8::from(seed != 0);
                for i in 1..=4 {
                    word(m, STACK + 4 * i, 0x310 + 7 * i);
                }
                word(m, STACK + 20, if seed == 5 { c.ebx } else { DATA + 0x200 });
            },
            |target, _, c, m| {
                assert_eq!(target, 0x1cc3f0);
                let out = m.read_u32(c.esp + 24).unwrap();
                word(m, out, 0x80ff_0021 + seed);
                c.eax = 0xb06a_ff00 | u32::from(seed != 3);
                24
            },
        );
    }
}
