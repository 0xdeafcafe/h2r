use super::*;

const CHANNEL: u32 = DATA + 0x1000;
const CLIP: u32 = DATA + 0x2000;
const TABLE: u32 = DATA + 0x3000;
const GRAPH: u32 = DATA + 0x4000;
const CLOCK: u32 = DATA + 0x5000;
const EVENTS: u32 = DATA + 0x6000;
const REPLACEMENT: u32 = DATA + 0x7000;

fn setup(c: &mut CpuAbi, m: &mut FlatMemory, seed: u32) {
    let mut rng = Rng(u64::from(seed) + 0x1860);
    for offset in (0..32).step_by(4) {
        word(m, CHANNEL + offset, rng.u32());
    }
    word(m, guest_layout::points::TAG_ENTRIES.base, TABLE);
    word(m, TABLE + 16 + 8, GRAPH);
    word(m, CHANNEL, 0x3456_0001);
    word(m, CHANNEL + 4, 0x0013_0022);
    half(m, CHANNEL + 0x10, 0);
    half(m, CHANNEL + 0x12, 1);
    word(m, CHANNEL + 0x18, 1.0f32.to_bits());
    word(m, CHANNEL + 0x1c, 3.0f32.to_bits());
    half(m, CLIP + 0x14, 8);
    half(m, CLIP + 0x2e, 1);
    word(m, CLIP + 0x4c, 0);
    word(m, CLIP + 0x50, EVENTS);
    word(m, 0x510c54, CLOCK);
    word(m, CLOCK + 4, 0.05f32.to_bits());
    word(m, REPLACEMENT, 0x0021_0023);
    c.esi = CHANNEL;
    for offset in (4..=20).step_by(4) {
        word(m, STACK + offset, rng.u32());
    }
}

fn answer(target: u32, site: u32, c: &mut CpuAbi, m: &mut FlatMemory, seed: u32) -> u32 {
    match target {
        0x1daea0 | 0x1c6440 => {
            c.eax = CLIP;
            if target == 0x1c6440 { 0 } else { 4 }
        }
        0x1c6bb0 => {
            c.eax = GRAPH;
            0
        }
        0x1c7450 => {
            c.eax = 0x3377_0011 ^ seed;
            if seed == 5 && site == 0x1c6754 {
                half(m, CHANNEL + 4, 3);
            }
            if seed == 6 && site == 0x1c67a1 {
                half(m, CHANNEL + 6, 4);
            }
            if seed == 7 && site == 0x1c67a1 {
                // The second event callback can change the loop target.
                half(m, CLIP + 0x2e, (-3i16) as u16);
            }
            24
        }
        0x1cb3b0 => {
            c.eax = REPLACEMENT;
            4
        }
        0x1c6560 => 4,
        0x1c69b0 => 0,
        0x1c66a0 => 20,
        _ => panic!("unexpected call {target:#x} at {site:#x}"),
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    for entry in [0x1c6440, 0x1c6470, 0x1c6500, 0x1c6560, 0x1c6620, 0x1c68c0, 0x1c6920] {
        for seed in 0..4 {
            h.case(entry, seed, |c, m| {
                setup(c, m, seed);
                c.eax = CHANNEL;
                c.ecx = if entry == 0x1c6470 { 1 } else { CHANNEL };
                let arg = [-0.375f32, 0.0, 0.5, 11.0][seed as usize];
                word(m, STACK + 4, arg.to_bits());
                if entry == 0x1c6470 {
                    word(m, STACK + 4, if seed == 3 { u32::MAX } else { 0x0050_0007 });
                    c.edx = 0xffef_8123;
                    if seed == 2 { c.ecx = u32::MAX; }
                } else if entry == 0x1c68c0 {
                    let delay_and_flags = [2, 0, 1 << 8, 0][seed as usize];
                    half(m, CHANNEL + 0x10, delay_and_flags);
                    if seed == 3 { half(m, CHANNEL + 0x12, 0); }
                } else if seed == 1 {
                    // For the unbound path the original reads the low
                    // page's halfword on the way to computing the frame.
                    half(m, CHANNEL + 6, u16::MAX);
                    half(m, 0x14, 0);
                    half(m, CHANNEL + 0x12, 0);
                }
            }, |target, site, c, m| answer(target, site, c, m, seed));
        }
    }

    for seed in 0..8 {
        h.case(0x1c66a0, seed, |c, m| {
            setup(c, m, seed);
            word(m, STACK + 4, CHANNEL);
            let target = [-0.5f32, 3.7, 9.4, 9.4, 9.4, 9.4, 9.4, 9.4][seed as usize];
            word(m, STACK + 8, target.to_bits());
            let flags = match seed {
                3 | 7 => 3,    // Loop at the clip's loop frame.
                4 => 0x2b, // Select a new clip when the loop frame is zero.
                _ => 1,
            };
            half(m, CHANNEL + 0x12, flags);
            if seed == 4 { half(m, CLIP + 0x2e, 0); }
        }, |target, site, c, m| answer(target, site, c, m, seed));
    }

    h.case(0x1c69b0, 10, |c, m| {
        setup(c, m, 10);
        half(m, CHANNEL + 6, u16::MAX);
    }, |_, _, _, _| panic!("unbound event mask made a call"));

    // Exercise the four-at-a-time scan, its signed remainder and both
    // interval endpoints. Event times and kind bits need not be sorted.
    for (seed, (count, speed)) in [(-1, 1.0f32), (0, 0.0), (1, -1.0), (4, 1.0),
        (5, -1.0), (8, 1.0), (9, 0.0), (5, f32::NAN)].into_iter().enumerate() {
        h.case(0x1c69b0, seed as u32, |c, m| {
            setup(c, m, seed as u32);
            word(m, CHANNEL + 0x18, speed.to_bits());
            word(m, CLIP + 0x4c, count as u32);
            for (i, frame) in [3i16, -1, 8, 0, 1, 2, 4, 9, 6].into_iter().enumerate() {
                half(m, EVENTS + i as u32 * 4, ((i * 7) % 35) as u16);
                half(m, EVENTS + i as u32 * 4 + 2, frame as u16);
            }
        }, |target, site, c, m| answer(target, site, c, m, seed as u32));
    }
}
