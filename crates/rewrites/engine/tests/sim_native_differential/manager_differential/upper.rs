//! Channel records, graph masks, root motion and event payloads.
use super::*;

const CHANNEL: u32 = DATA + 0x1000;
const TABLE: u32 = DATA + 0x2000;
const GRAPH: u32 = DATA + 0x3000;
const TRACK: u32 = DATA + 0x4000;
const OUTPUT: u32 = DATA + 0x5000;
const EXTERNAL_MASK: u32 = DATA + 0x6000;
const VIEW: u32 = DATA + 0x7000;
const STREAM: u32 = DATA + 0x7800;
const CLOCK: u32 = DATA + 0x8000;
const EVENT_LIST: u32 = DATA + 0x9000;
const CALLBACK: u32 = DATA + 0xf000;

/// The same channel is passed in different registers at the three pose
/// call sites. Its state distinguishes fast, preload and merged-mask paths.
fn channel_fixture(pc: u32, seed: u32, c: &mut CpuAbi, m: &mut FlatMemory) {
    c.eax = CHANNEL;
    c.ecx = CHANNEL;
    c.ebx = CHANNEL;
    c.esi = if matches!(pc, 0x1c6e30 | 0x1c7100 | 0x1c7300 | 0x1c7340) {
        CHANNEL
    } else {
        OUTPUT
    };
    c.edi = OUTPUT + 64;
    c.edx = if seed == 0 { 0 } else { EXTERNAL_MASK };
    if seed == 3 {
        c.eflags |= 0x400;
    }
    word(m, guest_layout::points::TAG_ENTRIES.base, TABLE);
    word(m, TABLE + 16 + 8, GRAPH);
    word(m, TABLE + 32 + 8, GRAPH + 0x100);
    word(m, 0x4687a4, OUTPUT + 128);
    word(m, 0x510c54, CLOCK);
    half(m, CLOCK + 2, 40);
    word(m, CHANNEL, 1);
    word(m, CHANNEL + 4, if seed == 0 { u32::MAX } else { 0x1_0000 });
    word(m, CHANNEL + 16, if seed == 2 { 1 } else { 0 });
    half(
        m,
        CHANNEL + 18,
        if seed == 0 {
            0
        } else if seed == 3 {
            0x401
        } else {
            0x801
        },
    );
    word(m, CHANNEL + 24, (0.7f32 + seed as f32 / 8.0).to_bits());
    word(m, CHANNEL + 28, (seed as f32 + 0.2).to_bits());
    word(m, GRAPH + 0x4c, 1);
    word(m, GRAPH + 0x50, VIEW);
    word(m, VIEW + 4, 2);
    word(m, TRACK + 16, if seed == 2 { 1 } else { 0 });
    half(m, TRACK + 20, 11);
    word(m, TRACK + 24, 0x41);
    word(m, TRACK + 0x4c, 3);
    word(m, TRACK + 0x50, EVENT_LIST);
    for (i, kind) in [3, 2, 0].into_iter().enumerate() {
        half(m, EVENT_LIST + i as u32 * 4, kind);
        half(m, EVENT_LIST + i as u32 * 4 + 2, (i * 7) as u16);
    }
    for i in 0..32 {
        word(m, GRAPH + 0x44 + i * 4, i ^ (seed << 8));
        word(m, EXTERNAL_MASK + i * 4, !i);
    }
    // Restore the subgraph header, which sits before the mask fields.
    word(m, GRAPH + 0x4c, 1);
    word(m, GRAPH + 0x50, VIEW);
    for i in 1..=6 {
        word(m, STACK + 4 * i, OUTPUT + i * 4);
    }
    // The caller's mask is the second parameter of base-pose and the
    // fourth parameter of blend-pose. Zero takes their direct paths.
    let mask = if seed == 0 { 0 } else { EXTERNAL_MASK };
    word(m, STACK + if pc == 0x1c7100 { 16 } else { 8 }, mask);
    if pc == 0x1c73a0 {
        word(m, STACK + 4, (seed as f32 + 0.75).to_bits());
        half(m, STACK + 8, 4);
        word(m, 0x47fb28 + 3 * 40, CALLBACK);
    }
}

fn stub(target: u32, _site: u32, c: &mut CpuAbi, m: &mut FlatMemory) -> u32 {
    match target {
        0x1c6c80 => {
            for (i, v) in [2.5f32, -1.25, 0.75].into_iter().enumerate() {
                word(m, c.ebx + i as u32 * 4, v.to_bits());
            }
            4
        }
        0x1c6dc0 => {
            for (i, v) in [-0.25f32, 1.5, 0.5].into_iter().enumerate() {
                word(m, c.esi + i as u32 * 4, v.to_bits());
            }
            word(m, c.ebx, 4.0f32.to_bits());
            4
        }
        0x1daea0 | 0x1daf30 => {
            c.eax = TRACK;
            4
        }
        0x1dd9d0 | 0x1daff0 => {
            c.eax = VIEW;
            4
        }
        0x1ddb40 => {
            word(m, c.edi, STREAM);
            word(m, c.edi + 4, VIEW);
            half(m, VIEW + 6, 16);
            word(m, STREAM + 16, 3);
            8
        }
        0x20ad40 | 0x27a380 => 0,
        0x20aa70 => 4,
        0x279d40 => {
            c.eax = EXTERNAL_MASK;
            0
        }
        0x279d80 => 32,
        0x279e40 => 36,
        0x27a060 => 28,
        CALLBACK => {
            let node = m.read_u32(0x5044c0).unwrap();
            for i in 0..3 {
                word(m, node + 16 + i * 4, (i as f32 - 1.25).to_bits());
            }
            0
        }
        _ => panic!("unexpected channel call {target:#x}"),
    }
}

fn events(h: &mut Harness<'_>) {
    for seed in 0..4 {
        h.case(
            0x1c7450,
            seed,
            |c, m| {
                c.ebx = if seed == 0 { 0 } else { CALLBACK };
                word(m, STACK + 4, GRAPH);
                word(m, STACK + 8, TRACK);
                word(m, STACK + 12, if seed == 0 { 0x380 } else { 0x3000 });
                let (a, b): (f32, f32) = if seed == 3 { (6.0, 0.0) } else { (0.0, 6.0) };
                word(m, STACK + 16, a.to_bits());
                word(m, STACK + 20, b.to_bits());
                word(m, STACK + 24, OUTPUT);
                for (block, stride, graph_refs) in [(0x4c, 4, 0), (0x54, 8, 0x18), (0x5c, 4, 0x20)]
                {
                    let list = EVENT_LIST + block * 32;
                    word(m, TRACK + block, 4);
                    word(m, TRACK + block + 4, list);
                    if graph_refs != 0 {
                        let refs = OUTPUT + block * 32;
                        word(m, GRAPH + graph_refs, refs);
                        word(m, refs + 4, 1);
                        half(m, refs + 8, if seed == 2 { 0x16 } else { 0 });
                    }
                    for (i, frame) in [0u16, 2, 6, 10].into_iter().enumerate() {
                        let entry = list + stride * i as u32;
                        half(m, entry, if graph_refs == 0 { i as u16 } else { 0 });
                        half(m, entry + 2, frame);
                        if stride == 8 {
                            word(m, entry + 4, 0x22 + i as u32);
                        }
                    }
                }
            },
            |target, site, c, m| {
                assert_eq!(target, CALLBACK);
                // The next iteration reloads the interval from the caller,
                // and reads subsequent event records after the callback.
                c.xmm[1] = [0x33; 16];
                c.xmm[2] = [0x66; 16];
                if seed == 3 && site == 0x1c751f {
                    half(m, EVENT_LIST + 0x4c * 32 + 6, 8);
                }
                12
            },
        );
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    for pc in [
        0x1c6bb0, 0x1c6c00, 0x1c6c80, 0x1c6d00, 0x1c6dc0, 0x1c6e30, 0x1c6ea0, 0x1c6ee0, 0x1c6f60,
        0x1c7100, 0x1c7200, 0x1c7300, 0x1c7340, 0x1c7380, 0x1c73a0,
    ] {
        for seed in 0..4 {
            h.case(pc, seed, |c, m| channel_fixture(pc, seed, c, m), stub);
        }
    }
    // Resolve can replace the channel speed. The resumed sampler decides
    // interpolation from the live channel, including clock and NaN cases.
    for (seed, speed) in [(-0.0f32).to_bits(), 1.0f32.to_bits(), 0x7f80_0011]
        .into_iter().enumerate()
    {
        h.case(
            0x1c6f60,
            seed as u32,
            |c, m| channel_fixture(0x1c6f60, 1, c, m),
            |target, site, c, m| {
                if site == 0x1c6f8c {
                    word(m, CHANNEL + 24, speed);
                }
                stub(target, site, c, m)
            },
        );
    }
    events(h);
}
