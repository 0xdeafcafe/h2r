//! Synthetic sound instances with signed range, permutation and chunk offsets.
//! The individual roots call seeded tag, allocation and cache services.

use super::*;
use engine::points::{RANDOM_SEEDS, SOUND_GLOBALS, SOUND_INSTANCES, TAG_ENTRIES};

const ENTRIES: u32 = ARENA + 0x100;
const SOUND: u32 = ARENA + 0x400;
const SOURCE: u32 = ARENA + 0x800;
const POOL: u32 = ARENA + 0xc00;
const ROWS: u32 = ARENA + 0x1000;
const GESTALT: u32 = ARENA + 0x1800;
const RANGES: u32 = ARENA + 0x2000;
const PERMUTATIONS: u32 = ARENA + 0x2800;
const CHUNKS: u32 = ARENA + 0x3000;
const CACHE: u32 = ARENA + 0x3800;
const CACHE_ROWS: u32 = ARENA + 0x4000;
const CLASS: u32 = ARENA + 0x4800;
const TAG: u32 = 0x1234_0005;
const HANDLE: u32 = 0x9876_0002;
const INSTANCE: u32 = ROWS + 2 * 0xbc;

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    for (at, value) in [
        (TAG_ENTRIES.base, ENTRIES),
        (ENTRIES + (TAG & 0xffff) * 16 + 8, SOUND),
        (SOUND_INSTANCES.base, POOL),
        (POOL + 0x44, ROWS),
        (0x51ebd4, GESTALT),
        (GESTALT + 0x24, RANGES),
        (GESTALT + 0x2c, PERMUTATIONS),
        (GESTALT + 0x44, CHUNKS),
        (GESTALT + 12, ARENA + 0x5000),
        (0x502104, CACHE),
        (CACHE + 0x44, CACHE_ROWS),
        (SOUND_GLOBALS.base, ARENA + 0x5800),
        (ARENA + 0x5884, 0xffff_faf0),
        (RANDOM_SEEDS.base, ARENA + 0x6000),
        (ARENA + 0x6004, 0x6591_c412),
        (INSTANCE + 12, TAG),
        (CHUNKS + 12 * 5 + 8, 0xade0_0007),
    ] {
        g.set_u32(at, value).unwrap();
    }
    // Range 3-1, permutation 6-2 and chunk 8-3.
    for (at, word) in [
        (SOUND + 8, 3),
        (RANGES + 2 * 12 + 8, 6),
        (PERMUTATIONS + 4 * 16 + 12, 8),
        (INSTANCE + 0x9e, (-3i16) as u16),
    ] {
        g.set_u16(at, word).unwrap();
    }
    g.set_u8(INSTANCE + 0x9c, (-1i8) as u8).unwrap();
    g.set_u8(INSTANCE + 0x9d, (-2i8) as u8).unwrap();
    g.set_u8(INSTANCE + 4, if seed & 1 == 0 { 0x41 } else { 0x43 })
        .unwrap();
    g.set_u8(CACHE_ROWS + 7 * 16 + 4, seed as u8).unwrap();
    match c.eip {
        0x218f10 => {
            c.gpr[1] = 2;
            c.gpr[2] = SOUND;
            g.set_u8(SOUND + 12, if seed == 0 { 0xff } else { 1 })
                .unwrap();
            g.set_u32(GESTALT + 0x4c, ARENA + 0x6500).unwrap();
            g.set_u32(ARENA + 0x6500 + 28 + 4, ARENA + 0x6700).unwrap();
            g.set_u16(ARENA + 0x6720, if seed == 1 { 0xffff } else { 3 })
                .unwrap();
        }
        0x218f50 => {
            c.gpr[0] = if seed == 1 || seed == 7 { 1 } else { u32::MAX };
            c.gpr[1] = SOUND;
            c.xmm[1] = u128::from(
                match seed {
                    2 => -20f32,
                    3 => 40f32,
                    4 => f32::NAN,
                    _ => 5f32,
                }
                .to_bits(),
            );
            g.set_u16(SOUND + 8, 0).unwrap();
            g.set_u8(SOUND + 10, if seed == 0 { 0 } else { 3 }).unwrap();
            g.set_u32(GESTALT + 0x1c, ARENA + 0x6600).unwrap();
            for (i, low, high) in [(0u32, -9i16, -3i16), (1, 1, 12), (2, 12, 19)] {
                let row = RANGES + 12 * i;
                let bounds = ARENA + 0x6600 + 10 * i;
                g.set_u16(row + 2, i as u16).unwrap();
                g.set_u16(row + 10, if seed == 7 && i == 1 { 0 } else { 2 })
                    .unwrap();
                for (off, val) in [(2, low), (4, high), (6, low), (8, high)] {
                    g.set_u16(bounds + off, val as u16).unwrap();
                }
            }
        }
        0x126000 => {
            // Requests span rejection, a full pool, automatic and explicit
            // selections, a promotion rule, both immediate-play flags, a
            // propagation delay, and optional attachment and payload fields.
            let flags = match seed {
                3 => 0x400,
                4 => 0x3ff,
                _ => 0,
            };
            g.set_u32(SOURCE, flags).unwrap();
            g.set_u32(SOURCE + 0x14, (-0.125f32).to_bits()).unwrap();
            g.set_u16(SOURCE + 0x8c, 67).unwrap();
            g.set_u16(SOURCE + 0x9c, 1).unwrap();
            g.set_u16(SOURCE + 0x9e, 2).unwrap();
            g.set_u16(
                SOURCE + 12,
                match seed {
                    5 => 1 << 8,
                    6 => 1 << 11,
                    _ => 0,
                },
            )
            .unwrap();
            g.set_u8(SOUND, if seed == 4 { 8 } else { 0 }).unwrap();
            g.set_u8(CLASS + 10, if seed == 4 { 4 } else { 0 }).unwrap();
            g.set_u32(0x547f14, 2.5_f32.to_bits()).unwrap();
            g.set_u32(0x440c48, 0).unwrap();
            g.set_u32(0x44a0b4, 0.25_f32.to_bits()).unwrap();
            for (i, word) in [TAG, 3, SOURCE, if seed == 4 { 2 } else { u32::MAX }]
                .into_iter()
                .enumerate()
            {
                g.set_u32(STACK + 4 + i as u32 * 4, word).unwrap();
            }
        }
        0x1286b0 => {
            c.gpr[0] = HANDLE;
            for (i, word) in [TAG, 0xacdd_fffe, 0xdeb1_0032, 0x90cf_c41c]
                .into_iter()
                .enumerate()
            {
                g.set_u32(STACK + 4 + i as u32 * 4, word).unwrap();
            }
        }
        0x125e60 => {
            g.set_u32(STACK + 4, INSTANCE).unwrap();
        }
        0x1268e0 => {
            c.gpr[0] = INSTANCE;
        }
        0x126960 => {
            c.gpr[1] = INSTANCE;
        }
        other => panic!("unknown impulse root {other:#x}"),
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x125260 => {
            c.gpr[0] = if seed == 0 { -1.0f32 } else { 1.0 }.to_bits();
            0
        }
        0x1251e0 => {
            c.gpr[0] = 0.125f32.to_bits();
            8
        }
        0x2226b0 => 0,
        0x16b840 => {
            c.gpr[0] = if seed == 1 { u32::MAX } else { HANDLE };
            0
        }
        0x127e20 => {
            let distance: f32 = if seed == 2 { 0.01 } else { 16384.0 };
            c.xmm[0] = u128::from(distance.to_bits());
            0
        }
        0x1252f0 => {
            // The pitch draw is a game-thread call, with both the seed write
            // and the value on the x87 stack visible when the allocator resumes.
            g.set_u32(c.gpr[1], seed.wrapping_mul(109)).unwrap();
            let mut pitch = x87::abi::X87Abi {
                st: c.x87.st,
                control: c.x87.control,
                status: c.x87.status,
                tag: c.x87.tag,
                ..Default::default()
            };
            x87::abi::ld_f32(&mut pitch, 0.125f32.to_bits());
            c.x87.st = pitch.st;
            c.x87.status = pitch.status;
            c.x87.tag = pitch.tag;
            0
        }
        0x221810 => {
            c.gpr[0] = CLASS;
            4
        }
        0x219a30 => {
            c.gpr[0] = 2;
            0
        }
        0x2198f0 => {
            c.gpr[0] = ARENA + 0x6400;
            4
        }
        0x219f60 => 0,
        0x218f10 | 0x218f50 => {
            c.gpr[0] = 1;
            0
        }
        0x219070 => {
            c.gpr[0] = 3;
            4
        }
        0x1286b0 => 16,
        0x125e60 => 4,
        0x126960 => {
            c.gpr[0] = 0xfad0_bc29;
            0
        }
        0x218850 => {
            c.gpr[0] = seed;
            12
        }
        0x1268e0 => 0,
        other => panic!("unexpected impulse call {other:#x}"),
    }
}

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let count = match entry {
        0x218f10 => 3,
        0x218f50 => 8,
        0x126000 => 7,
        0x1286b0 => 1,
        0x125e60 => 4,
        0x1268e0 | 0x126960 => 2,
        _ => 0,
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}

const START_SOUND: u32 = 0x21d110;
const AUX_POOL: u32 = ARENA + 0x6800;
const AUX_ROWS: u32 = ARENA + 0x6900;
const PLAYBACK: u32 = ARENA + 0x6b00;
const PROMOTION: u32 = ARENA + 0x6c00;

fn chain_setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    c.eip = 0x126000;
    setup(g, c, 2);
    c.eip = START_SOUND;
    c.gpr[0] = SOURCE;
    g.set_u32(STACK + 4, TAG).unwrap();
    for (at, value) in [
        (0x51ebe0, AUX_POOL),
        (AUX_POOL + 0x44, AUX_ROWS),
        (ARENA + 0x6a08, 9),
        (GESTALT + 4, PLAYBACK),
        (PLAYBACK + 0x10, 1.5f32.to_bits()),
        (PLAYBACK + 0x14, 0.75f32.to_bits()),
        (GESTALT + 0x1c, ARENA + 0x6e00),
        (GESTALT + 0x4c, PROMOTION),
        (PROMOTION, 1),
        (PROMOTION + 4, ARENA + 0x6d00),
        (PROMOTION + 8, 1),
        (PROMOTION + 12, ARENA + 0x6d80),
        (PROMOTION + 16, 0),
        (PROMOTION + 20, 3),
        (PROMOTION + 24, if seed == 3 { 20 } else { 0 }),
        (ARENA + 0x5884, 10),
        (0x4e034c, ARENA + 0x7000),
        (ARENA + 0x70c0, 1),
        (ARENA + 0x70c4, ARENA + 0x7100),
        (ARENA + 0x7104, 1),
        (ENTRIES + 16 + 8, ARENA + 0x7200),
        (ARENA + 0x7204, CLASS),
    ] {
        g.set_u32(at, value).unwrap();
    }
    g.set_u8(SOUND + 12, if seed == 0 { u8::MAX } else { 0 })
        .unwrap();
    g.set_u8(SOUND + 10, 1).unwrap();
    for (at, value) in [
        (PLAYBACK + 0x18, -1i16),
        (PLAYBACK + 0x1a, 3),
        (RANGES + 36 + 10, 1),
        (ARENA + 0x6e06, -1200),
        (ARENA + 0x6e08, 1200),
    ] {
        g.set_u16(at, value as u16).unwrap();
    }
    for i in 0..3 {
        g.set_u8(ARENA + 0x5878 + i, 1).unwrap();
    }
}

fn chain_reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    let sp = c.gpr[4];
    match target {
        0x21dd30 => {
            let out = g.u32(sp + 8).unwrap();
            g.set_u32(out, ARENA + 0x6a00).unwrap();
            c.gpr[0] = u32::from(seed != 0);
            8
        }
        0x16b840 => {
            c.gpr[0] = if c.gpr[2] == AUX_POOL {
                0xb104_0001
            } else if seed == 2 {
                u32::MAX
            } else {
                HANDLE
            };
            0
        }
        0x18d090 => {
            c.gpr[0] = 0;
            0
        }
        0x21dc60 | 0x21dcf0 => 0,
        0x126c30 => {
            assert_eq!(g.u32(sp + 4).unwrap(), TAG);
            g.set_u32(g.u32(sp + 8).unwrap(), 3).unwrap();
            c.gpr[0] = u32::from(seed != 4);
            12
        }
        0x21d630 => 8,
        0x218850 => {
            // Registration can re-enter the guest through DirectSound. Pin the
            // chunk only after the initial request succeeds.
            c.gpr[0] = 1;
            12
        }
        _ => reply(g, c, target, seed),
    }
}

fn chain_answer(code: &mut lifted::Original, case: &Case, c: &mut Cpu, g: &mut Guest) {
    if matches!(c.eip, 0x125260 | 0x1252f0 | 0x219070 | 0x221810) {
        let site = g.u32(c.gpr[4]).unwrap();
        for _ in 0..20000 {
            if let Flow::Return { target } = lifted::code::step(code, c, &mut Memory(g)).unwrap()
                && target == site
            {
                return;
            }
        }
        panic!("guest sound helper did not return to {site:#x}");
    }
    answer(code, case, c, g);
}

#[test]
fn fused_game_sound_start_preserves_rng_draws_and_cache_request_order() {
    let Some(image) = Image::get() else { return };
    let selected = [
        START_SOUND,
        0x126ec0,
        0x126000,
        0x218f10,
        0x218f50,
        0x1286b0,
        0x126960,
        0x125e60,
        0x1268e0,
    ];
    let mut code = image.original();
    for seed in 0..5 {
        let table = Table::new(
            selected
                .iter()
                .map(|entry| engine::sim_sound::TABLE.at(*entry).unwrap().0),
        )
        .unwrap();
        if seed == 1 || seed == 2 {
            // An installed before-hook guards the allocation boundary. Calls
            // then leave the fused driver and resume on the same guest frame.
            assert!(table.guard(0x126000));
        }
        let mut guard_calls = 0;
        let scenario = Case {
            setup: chain_setup,
            reply: chain_reply,
            seed,
        };
        let (mut ref_ram, mut reference) = start(image, START_SOUND, &scenario);
        let (mut native_ram, mut native) = start(image, START_SOUND, &scenario);
        let mut draws = Vec::new();
        let mut requests = Vec::new();
        for tick in 0..120 {
            let context = format!(
                "fused impulse {seed}, transfer {tick} from {:#x}",
                native.eip
            );
            let lifted = loop {
                let transfer = segment(&mut code, &mut reference, &mut ref_ram);
                if table.at(transfer.target_pc).is_none() || table.guarded(transfer.target_pc) {
                    break transfer;
                }
            };
            let mut cpu = from_interp(&native);
            let actual = sim_objects::drive(&mut cpu, &mut native_ram, &table, native.eip).unwrap();
            assert_eq!(actual, lifted, "{context}");
            native = to_interp(&cpu);
            native.eip = actual.target_pc;
            assert_eq!(lifted::cpu::diff(&native, &reference), vec![], "{context}");
            compare_ram(&ref_ram, &native_ram, reference.gpr[4], &context);
            if actual.kind == abi::RETURN {
                assert_eq!(native.eip, EXIT);
                break;
            }
            if table.guarded(native.eip) {
                guard_calls += 1;
                for (g, sp) in [
                    (&mut ref_ram, reference.gpr[4]),
                    (&mut native_ram, native.gpr[4]),
                ] {
                    assert_eq!(g.u32(sp + 4).unwrap(), TAG);
                    assert_eq!(g.u32(sp + 8).unwrap(), 3);
                    assert_eq!(g.u32(sp + 12).unwrap(), SOURCE);
                    assert_eq!(g.u32(sp + 16).unwrap(), 0);
                    g.set_u32(sp + 8, 7).unwrap();
                }
                continue;
            }
            if matches!(native.eip, 0x125260 | 0x1252f0) {
                draws.push(native.eip);
            }
            if native.eip == 0x218850 {
                requests.push(native_ram.u32(native.gpr[4] + 12).unwrap());
            }
            chain_answer(&mut code, &scenario, &mut reference, &mut ref_ram);
            chain_answer(&mut code, &scenario, &mut native, &mut native_ram);
        }
        assert_eq!(native.eip, EXIT);
        assert_eq!(guard_calls, u32::from(seed == 1 || seed == 2));
        match seed {
            0 | 1 => {
                assert_eq!(
                    native_ram.u8(INSTANCE + 0xa1).unwrap(),
                    if seed == 1 { 7 } else { 3 }
                );
                assert_eq!(native.gpr[0], HANDLE);
                assert_eq!(draws, [0x125260, 0x1252f0]);
                assert_eq!(requests, [2, 4]);
            }
            2 => {
                assert_eq!(native.gpr[0], u32::MAX);
                assert_eq!(draws, [0x125260]);
                assert!(requests.is_empty());
            }
            _ => {
                assert_eq!(native.gpr[0], u32::MAX);
                assert!(draws.is_empty() && requests.is_empty());
            }
        }
    }
}
