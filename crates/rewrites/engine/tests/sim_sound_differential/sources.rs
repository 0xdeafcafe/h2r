//! Synthetic channels and listeners. Callees can alter the count or
//! replace a channel while the frame is suspended.
use super::*;
use engine::points;
use guest_layout::sound::{LoopingSound, SoundChannel, SoundInstance, SoundListener};

const GLOBALS: u32 = ARENA;
const CHANNELS: u32 = ARENA + 0x1000;
const STREAMS: u32 = ARENA + 0x2000;
const SUBMIXES: u32 = ARENA + 0x2400;
const INSTANCE_POOL: u32 = ARENA + 0x3000;
const RECORDS: u32 = ARENA + 0x3800;
const CALLBACKS: u32 = ARENA + 0x4000;
const CALLBACK: u32 = ARENA + 0x4200;
const PLAYER_MAPPING: u32 = ARENA + 0x4800;
const SOURCE: u32 = ARENA + 0x5000;
const DISTANCE_OUT: u32 = ARENA + 0x5800;
const LOOP_POOL: u32 = ARENA + 0x6000;
const LOOPS: u32 = ARENA + 0x6200;
const COMPANION_POOL: u32 = ARENA + 0x6500;
const COMPANIONS: u32 = ARENA + 0x6800;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let (setup, reply, count): (
        fn(&mut Guest, &mut Cpu, u32),
        fn(&mut Guest, &mut Cpu, u32, u32) -> u32,
        u32,
    ) = match entry {
        0x1290d0 | 0x129380 | 0x1293f0 => (channels, channel_reply, 4),
        0x21b070 => (loop_release, loop_reply, 5),
        0x127320 | 0x127390 => (completion, complete_reply, 6),
        0x128020 | 0x1282d0 => (instances, instance_reply, 15),
        0x127d00 => (listeners, listener_reply, 12),
        0x127e20 => (distances, listener_reply, 8),
        _ => return vec![],
    };
    (0..count).map(|seed| Case { setup, reply, seed }).collect()
}

fn channels(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    g.set_u32(points::SOUND_GLOBALS.base, GLOBALS).unwrap();
    g.set_u32(points::SOUND_CHANNELS.base, CHANNELS).unwrap();
    g.set_u32(points::SOUND_INSTANCES.base, INSTANCE_POOL)
        .unwrap();
    g.set_u32(INSTANCE_POOL + 0x44, RECORDS).unwrap();
    g.set_u32(GLOBALS + 0x84, 2).unwrap();
    g.set_u16(GLOBALS + 0x208, [0, 5, 5, u16::MAX][seed as usize])
        .unwrap();
    g.set_u32(GLOBALS + 0x1e4, 3.0f32.to_bits()).unwrap();
    g.set_u32(0x44a0b4, (-4.0f32).to_bits()).unwrap();
    g.set_u32(CALLBACKS + 8, CALLBACK).unwrap();
    cpu.gpr[3] = SUBMIXES;
    cpu.gpr[7] = STREAMS;
    for index in 0..5 {
        let channel = CHANNELS + index * SoundChannel::SIZE;
        let source = RECORDS + index * SoundInstance::SIZE;
        g.set_u32(
            channel,
            if index == 1 {
                u32::MAX
            } else {
                0xabcd_0000 | index
            },
        )
        .unwrap();
        g.set_u8(channel + 9, u8::from(index == 3)).unwrap();
        g.set_u16(channel + 0xc, (4 - index) as u16).unwrap();
        g.set_u16(channel + 0xe, if index == 0 { 3 } else { u16::MAX })
            .unwrap();
        g.set_u32(STREAMS + 4 * index, 0x1111 + index).unwrap();
        g.set_u32(SUBMIXES + 4 * index, 0x2222 + index).unwrap();
        g.set_u8(source + 2, if index == 0 { 0 } else { 3 })
            .unwrap();
        g.set_u8(source + 4, 0x80).unwrap();
        g.set_u32(source + 0xb4, if index == 0 { 7 } else { 0 })
            .unwrap();
        g.set_u32(source + 0xb8, if index == 4 { 3 } else { 1 })
            .unwrap();
        g.set_u32(
            source + 0x20,
            if seed == 1 { (-40.0f32).to_bits() } else { 0 },
        )
        .unwrap();
        g.set_u32(source + 0x14, CALLBACKS).unwrap();
        g.set_u8(source + 0x9c, 0xfb).unwrap();
    }
}

fn channel_reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        // Change the count in a child. A snapshot at the head of the walk would visit stale slots.
        0x12a450 | 0x129790 => {
            if seed == 2 {
                g.set_u16(GLOBALS + 0x208, 1).unwrap();
            }
            if target == 0x129790 { 4 } else { 0 }
        }
        0x2197f0 => {
            cpu.gpr[0] = 2.0f32.to_bits();
            0
        }
        0x21f360 => 4,
        0x12a810 => {
            cpu.gpr[0] = (-0.75f32).to_bits();
            0
        }
        0x127320 => 4,
        0x129aa0 => {
            // Write gains and a channel parameter through the guest's output pointers.
            let gain = g.u32(cpu.gpr[4] + 4).unwrap();
            let params = g.u32(cpu.gpr[4] + 8).unwrap();
            g.set_u32(gain, 0.25f32.to_bits()).unwrap();
            g.set_u32(params + 8, 0x31415926).unwrap();
            8
        }
        0x128ca0 | 0x21c400 => 12,
        CALLBACK => 24,
        _ => panic!("unexpected channel call {target:#x}"),
    }
}

fn loop_release(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    cpu.gpr[7] = 0xa100_0001;
    g.set_u32(points::LOOPING_SOUNDS.base, LOOP_POOL).unwrap();
    g.set_u32(LOOP_POOL + 0x44, LOOPS).unwrap();
    g.set_u32(0x51ebd8, COMPANION_POOL).unwrap();
    g.set_u32(COMPANION_POOL + 0x44, COMPANIONS).unwrap();
    g.set_u32(
        LOOPS + LoopingSound::SIZE + 0x54,
        if seed == 0 { u32::MAX } else { 0xbbcd_0002 },
    )
    .unwrap();
    g.set_u8(
        COMPANIONS + 0x1c * 2 + 2,
        [0, 2, 0x81, 0x80, 1][seed as usize],
    )
    .unwrap();
}

fn loop_reply(_: &mut Guest, _: &mut Cpu, target: u32, _: u32) -> u32 {
    assert_eq!(target, 0x16ba40);
    0
}

fn listeners(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    let sp = cpu.gpr[4];
    g.set_u32(sp + 4, SOURCE).unwrap();
    g.set_u32(
        sp + 8,
        if seed == 3 {
            0.25f32.to_bits()
        } else {
            24f32.to_bits()
        },
    )
    .unwrap();
    g.set_u32(sp + 12, if seed == 0 { 0 } else { DISTANCE_OUT })
        .unwrap();
    g.set_u32(points::PLAYER_GLOBALS.base, PLAYER_MAPPING)
        .unwrap();
    g.set_u32(points::SOUND_GLOBALS.base, GLOBALS).unwrap();
    g.set_u8(SOURCE + 3, if seed < 2 { 0 } else { 1 }).unwrap();
    g.set_u16(
        SOURCE,
        match seed {
            4 | 5 => 0x100,
            6 | 7 => 0x800,
            _ => 0,
        },
    )
    .unwrap();
    for user in 0..4 {
        let active = seed >= 2 && seed != 8 && user != 1;
        g.set_u8(GLOBALS + 0x8e + user * SoundListener::SIZE, active.into())
            .unwrap();
        g.set_u32(
            PLAYER_MAPPING + 12 + 4 * user,
            if user == 2 && seed != 0 {
                0xcafe_0001
            } else {
                u32::MAX
            },
        )
        .unwrap();
    }
}

fn distances(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    let user = 2;
    let listener = GLOBALS + 0x88 + user * SoundListener::SIZE;
    g.set_u32(points::SOUND_GLOBALS.base, GLOBALS).unwrap();
    cpu.gpr[1] = SOURCE;
    cpu.gpr[2] = user;
    let kind = match seed {
        0 => 0,
        1 => 2,
        2 => 0x0f,
        _ => 1,
    };
    g.set_u8(SOURCE + 3, kind).unwrap();
    g.set_u8(SOURCE + 2, u8::from(seed >= 4)).unwrap();
    for (i, value) in [2.0f32, -3.0, 0.5].into_iter().enumerate() {
        g.set_u32(SOURCE + 12 + 4 * i as u32, value.to_bits())
            .unwrap();
    }
    g.set_u32(listener + 0x30, 3.0f32.to_bits()).unwrap();
    g.set_u32(listener + 0x34, 5.0f32.to_bits()).unwrap();
    let z: f32 = match seed {
        4 => -4.0,
        5 => 3.0,
        6 => 0.5,
        7 => f32::NAN,
        _ => 1.0,
    };
    g.set_u32(listener + 0x38, z.to_bits()).unwrap();
    g.set_u32(SOURCE + 0x30, 1.25f32.to_bits()).unwrap();
}

fn listener_reply(_: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x127e20 => {
            let scores = if seed == 11 {
                [f32::NAN, 0.0, 2.0, 2.0]
            } else {
                [9.0, 7.0, 4.0, 4.0]
            };
            cpu.xmm[0] = u128::from(scores[cpu.gpr[2] as usize].to_bits());
            0
        }
        0x221da0 => {
            cpu.gpr[0] = u32::from(matches!(seed, 4 | 6 | 9));
            4
        }
        _ => panic!("unexpected listener call {target:#x}"),
    }
}

const TAGS: u32 = ARENA + 0x10000;
const SOUND_TAG: u32 = TAGS + 0x1000;
const GESTALT: u32 = TAGS + 0x2000;
const PLAYBACK: u32 = TAGS + 0x3000;
const MARKERS: u32 = TAGS + 0x4000;
const CLASS: u32 = TAGS + 0x5000;
const PITCH_RANGES: u32 = TAGS + 0x6000;
const OCCUPIED: u32 = TAGS + 0x7000;
const DRIVER: u32 = TAGS + 0x8000;
const SOURCE_POOL: u32 = TAGS + 0x9000;
const START_CALLBACK: u32 = TAGS + 0xc000;

fn instances(g: &mut Guest, cpu: &mut Cpu, seed: u32) {
    g.set_u32(points::SOUND_GLOBALS.base, GLOBALS).unwrap();
    g.set_u32(points::SOUND_INSTANCES.base, INSTANCE_POOL)
        .unwrap();
    g.set_u32(points::SOUND_CHANNELS.base, CHANNELS).unwrap();
    g.set_u32(points::SOUND_DRIVER.base, DRIVER).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(0x51ebd4, GESTALT).unwrap();
    g.set_u32(GESTALT + 4, PLAYBACK).unwrap();
    g.set_u32(GESTALT + 0x24, PITCH_RANGES).unwrap();
    g.set_u32(GESTALT + 0x4c, MARKERS).unwrap();
    g.set_u32(TAGS + 8, SOUND_TAG).unwrap();
    g.set_u8(
        SOUND_TAG + 12,
        if matches!(seed, 11..=13) { 0 } else { 0xff },
    )
    .unwrap();
    g.set_u8(SOUND_TAG + 1, u8::from(seed == 4) << 3).unwrap();
    g.set_u16(SOUND_TAG + 6, 1).unwrap();
    g.set_u32(PLAYBACK + 0x38 + 4, 25f32.to_bits()).unwrap();
    g.set_u32(MARKERS + 0x10, 2).unwrap();
    g.set_u32(MARKERS + 0x14, 1).unwrap();
    g.set_u32(MARKERS + 0x18, 2).unwrap();
    g.set_u32(GLOBALS + 0x84, 10).unwrap();
    g.set_u32(INSTANCE_POOL + 0x24, SoundInstance::SIZE)
        .unwrap();
    g.set_u32(INSTANCE_POOL + 0x38, 34).unwrap();
    g.set_u32(INSTANCE_POOL + 0x44, SOURCE_POOL).unwrap();
    g.set_u32(INSTANCE_POOL + 0x48, OCCUPIED).unwrap();
    g.set_u32(OCCUPIED, 1 << 1).unwrap();
    g.set_u32(OCCUPIED + 4, 2).unwrap(); // Dormant slots between generations.
    g.set_u8(CLASS + 0xa, u8::from(seed == 7) << 5).unwrap();
    g.set_u16(CLASS + 0xe, u16::from(seed == 8)).unwrap();
    for i in [1, 33] {
        let rec = SOURCE_POOL + i * SoundInstance::SIZE;
        g.set_u16(rec, i as u16 + 5).unwrap();
        let flags = match seed {
            4 | 5 => 4,
            7 => 0x10,
            8 | 9 => 0x240,
            _ => 0,
        };
        g.set_u16(rec + 4, flags).unwrap();
        g.set_u32(rec + 0xc, 0xf00d_0000).unwrap();
        g.set_u32(rec + 0x10, 0xcdef_0099).unwrap();
        g.set_u32(rec + 0x14, CALLBACKS).unwrap();
        g.set_u8(rec + 0x18, u8::from(seed % 3 == 2) << 3).unwrap();
        g.set_u32(rec + 0x50, 19f32.to_bits()).unwrap();
        g.set_u32(rec + 0x8c, if seed == 10 { 11 } else { 1 })
            .unwrap();
        g.set_u8(
            rec + 0xa0,
            match seed {
                12 => 1,
                13 => 3,
                _ => 0xff,
            },
        )
        .unwrap();
        g.set_u16(
            rec + 0xac,
            if matches!(seed, 8..=10) { 0 } else { u16::MAX },
        )
        .unwrap();
    }
    g.set_u8(CHANNELS + 9, u8::from(seed == 10)).unwrap();
    g.set_u16(CHANNELS + 0xc, 2).unwrap();
    g.set_u8(DRIVER + 2 * 0x34 + 13, u8::from(seed == 9))
        .unwrap();
    g.set_u32(
        CHANNELS,
        match seed {
            0..=2 => u32::MAX,
            3 => (6 << 16) | 1,
            _ => 0xcafe_0007,
        },
    )
    .unwrap();
    g.set_u32(CALLBACKS + 4, START_CALLBACK).unwrap();
    // Assignment visits class fallback for no-channel, no-source seeds.
    if cpu.eip == 0x1282d0 && seed == 14 {
        g.set_u16(SOURCE_POOL + SoundInstance::SIZE + 4, 0).unwrap();
    }
}

fn instance_reply(g: &mut Guest, cpu: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x16bc00 => {
            cpu.gpr[0] = if seed == 0 { u32::MAX } else { 1 };
            0
        }
        0x127fc0 => {
            cpu.gpr[0] = u32::from(seed != 1);
            0
        }
        0x127320 => 4,
        0x221810 => {
            cpu.gpr[0] = CLASS;
            4
        }
        0x127d00 => {
            cpu.gpr[0] = if seed % 2 == 0 { 1 } else { u32::MAX };
            12
        }
        0x12a810 => {
            cpu.gpr[0] = (-3.0f32).to_bits();
            0
        }
        0x126df0 => 4,
        0x125e60 => {
            cpu.gpr[0] = u32::from(seed < 7);
            4
        }
        0x128700 => {
            let reason = g.u32(cpu.gpr[4] + 8).unwrap();
            g.set_u32(reason, 3).unwrap();
            cpu.gpr[0] = if seed == 1 { u32::MAX } else { 0 };
            8
        }
        0x12a5d0 => 0,
        START_CALLBACK => {
            // Deleting a later occupied slot must suppress its start this tick.
            if seed == 4 {
                g.set_u32(OCCUPIED + 4, 0).unwrap();
            }
            16
        }
        0x2193d0 => {
            cpu.gpr[0] = if seed == 14 { 1 } else { u32::MAX };
            8
        }
        0x219310 => 12,
        _ => panic!("unexpected instance walk call {target:#x}"),
    }
}

fn completion(g: &mut Guest, c: &mut Cpu, seed: u32) {
    let sound = 0xabcd_0002;
    c.gpr[0] = sound;
    c.gpr[7] = sound;
    g.set_u32(STACK + 4, 9).unwrap();
    g.set_u32(points::SOUND_INSTANCES.base, INSTANCE_POOL)
        .unwrap();
    g.set_u32(INSTANCE_POOL + 0x44, RECORDS).unwrap();
    let r = RECORDS + 2 * SoundInstance::SIZE;
    g.set_u32(r + 0x10, 0xfeed_0012).unwrap();
    g.set_u32(r + 0x14, if seed == 0 { 0 } else { CALLBACKS })
        .unwrap();
    g.set_u32(CALLBACKS + 0x10, if seed < 2 { 0 } else { CALLBACK })
        .unwrap();
    g.set_u16(r + 0xac, if seed % 2 == 0 { u16::MAX } else { 3 })
        .unwrap();
    g.set_u32(r + 0xa8, if seed == 0 { u32::MAX } else { 0xface_0001 })
        .unwrap();
    g.set_u32(0x51ebd8, COMPANION_POOL).unwrap();
    g.set_u32(COMPANION_POOL + 0x44, COMPANIONS).unwrap();
    g.set_u8(r + 3, if seed == 1 { 1 } else { 0xff }).unwrap();
    g.set_u32(COMPANIONS + 0x1c + 4, if seed == 2 { u32::MAX } else { 0 })
        .unwrap();
    g.set_u8(
        COMPANIONS + 0x1c + 3,
        [2, 3, 4, 0, 0x81, 0x80][seed as usize],
    )
    .unwrap();
}

fn complete_reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x12a5d0 | 0x126960 | 0x127390 | 0x219c80 | 0x16ba40 => 0,
        CALLBACK => {
            // A callback may detach the source's companion before free.
            if seed == 4 {
                g.set_u32(RECORDS + 2 * SoundInstance::SIZE + 0xa8, u32::MAX)
                    .unwrap();
            }
            c.gpr[1] = seed;
            12
        }
        _ => panic!("unexpected sound completion callback {target:#x}"),
    }
}
