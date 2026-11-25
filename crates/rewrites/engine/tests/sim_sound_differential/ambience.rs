//! BSP ambience selection with local, adjacent, blocked and out-of-range
//! palette entries. The sort stub retains palette order in both arms.
use super::*;
use engine::points;

const LIST: u32 = ARENA + 0x1000;
const PORTAL_LIST: u32 = ARENA + 0x2000;
const PORTALS: u32 = ARENA + 0x2100;
const CLUSTERS: u32 = ARENA + 0x2400;
const PALETTES: u32 = ARENA + 0x2800;
const OUT: u32 = ARENA + 0x4000;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry == points::SOUND_AMBIENCE_FARTHER.base {
        return (0..4)
            .map(|seed| Case {
                setup: compare_distances,
                reply,
                seed,
            })
            .collect();
    }
    if entry == points::SOUND_CLUSTER_AMBIENCE.base {
        return (0..9).map(|seed| Case { setup, reply, seed }).collect();
    }
    Vec::new()
}

fn compare_distances(g: &mut Guest, _: &mut Cpu, seed: u32) {
    let other = [
        1f32.to_bits(),
        0f32.to_bits(),
        (-1f32).to_bits(),
        f32::NAN.to_bits(),
    ][seed as usize];
    for (address, value) in [
        (STACK + 4, 1),
        (STACK + 8, 0),
        (STACK + 12, LIST),
        (LIST, 0),
        (LIST + 4, other),
    ] {
        g.set_u32(address, value).unwrap();
    }
}

fn setup(g: &mut Guest, _: &mut Cpu, seed: u32) {
    let palette_count = match seed {
        0 => 0,
        1 => 1,
        _ => 3,
    };
    let portal_count = u32::from(seed >= 3);
    let gain = if seed >= 4 { OUT + 0x100 } else { 0 };
    let count = if seed % 2 == 1 { 0 } else { OUT + 0x400 };
    let args = [
        ARENA,
        0,
        ARENA + 0x80,
        6f32.to_bits(),
        OUT,
        gain,
        OUT + 0x200,
        OUT + 0x300,
        count,
    ];
    for (i, arg) in args.into_iter().enumerate() {
        g.set_u32(STACK + 4 + i as u32 * 4, arg).unwrap();
    }
    for (address, value) in [
        (ARENA + 0xa0, CLUSTERS),
        (ARENA + 0x60, PORTALS),
        (ARENA + 0x14c, LIST),
        (ARENA + 0xd4, palette_count),
        (ARENA + 0xd8, PALETTES),
        (LIST + 4, portal_count),
        (LIST + 8, PORTAL_LIST),
        (PALETTES + 0x44, 4.0f32.to_bits()),
        (PALETTES + 0x64 + 0x44, 8f32.to_bits()),
    ] {
        g.set_u32(address, value).unwrap();
    }
    g.set_u16(CLUSTERS + 0x72, if seed == 0 { u16::MAX } else { 0 })
        .unwrap();
    g.set_u16(CLUSTERS + 0x78, 0).unwrap();
    g.set_u16(CLUSTERS + 0xb0 + 0x72, if seed == 8 { u16::MAX } else { 1 })
        .unwrap();
    g.set_u16(PORTAL_LIST, 0x8000).unwrap();
    g.set_u16(PORTALS + 2, 1).unwrap();
    g.set_u8(PORTALS + 0x18, if seed == 6 { 8 } else { 0 })
        .unwrap();
}

fn reply(_: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x249d60 => c.xmm[0] = u128::from(if seed == 7 { 16f32 } else { 1f32 }.to_bits()),
        0x14b240 => c.xmm[0] = u128::from(1.5f32.to_bits()),
        0x13de30 => return 12,
        0x2197f0 => c.gpr[0] = c.xmm[0] as u32,
        _ => panic!("unexpected ambience call to {target:#x}"),
    }
    0
}
