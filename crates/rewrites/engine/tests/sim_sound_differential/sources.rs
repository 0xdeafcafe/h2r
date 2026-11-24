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
