//! The four listener seats with missing users, free cameras, unit cameras,
//! mounted cameras and either side of an acoustic region plane.
use super::*;
use engine::points;

const OPTIONS: u32 = ARENA;
const SOUND: u32 = ARENA + 0x2000;
const USERS: u32 = ARENA + 0x2400;
const PLAYERS: u32 = ARENA + 0x2800;
const RECORDS: u32 = ARENA + 0x3000;
const OBJECTS: u32 = ARENA + 0x4000;
const HEADERS: u32 = ARENA + 0x4100;
const UNIT: u32 = ARENA + 0x4400;
const VEHICLE: u32 = ARENA + 0x4800;
const TAGS: u32 = ARENA + 0x4c00;
const DEFINITION: u32 = ARENA + 0x5000;
const SEAT: u32 = ARENA + 0x5400;
const STRUCTURE: u32 = ARENA + 0x5800;
const CLUSTER: u32 = ARENA + 0x5c00;
const REGION: u32 = ARENA + 0x6000;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry != points::SOUND_LISTENERS_UPDATE.base {
        return Vec::new();
    }
    (0..12).map(|seed| Case { setup, reply, seed }).collect()
}

fn coords(g: &mut Guest, address: u32, values: &[f32]) {
    for (index, &v) in values.iter().enumerate() {
        g.set_u32(address + 4 * index as u32, v.to_bits()).unwrap();
    }
}

fn setup(g: &mut Guest, _: &mut Cpu, seed: u32) {
    for (address, value) in [
        (
            points::GAME_GLOBALS.base,
            if seed == 0 { 0 } else { OPTIONS },
        ),
        (points::SOUND_GLOBALS.base, SOUND),
        (points::PLAYER_GLOBALS.base, USERS),
        (points::PLAYERS.base, PLAYERS),
        (points::OBJECTS.base, OBJECTS),
        (points::TAG_ENTRIES.base, TAGS),
        (PLAYERS + 0x44, RECORDS),
        (OBJECTS + 0x44, HEADERS),
        (HEADERS + 8, UNIT),
        (HEADERS + 12 + 8, VEHICLE),
        (UNIT + 0x14, if seed == 5 { u32::MAX } else { 1 }),
        (VEHICLE, 0),
        (TAGS + 8, DEFINITION),
        (DEFINITION + 0x1cc, SEAT),
        (0x4e0348, STRUCTURE),
        (STRUCTURE + 0xa0, CLUSTER),
        (STRUCTURE + 0x68, REGION),
        (SOUND + 0x1b0, if seed % 2 == 0 { 0 } else { u32::MAX }),
        (SOUND + 0x1cc, if seed % 2 == 1 { 0 } else { u32::MAX }),
        (STACK + 4, u32::from(seed % 3 != 0)),
    ] {
        g.set_u32(address, value).unwrap();
    }
    g.set_u8(OPTIONS + 0x1120, u8::from(seed > 1)).unwrap();
    g.set_u16(UNIT + 0x1fc, if seed == 6 { u16::MAX } else { 0 })
        .unwrap();
    g.set_u16(0x4686c4, 0).unwrap();
    g.set_u8(
        CLUSTER + 0x70,
        match seed {
            5 => 0xff,
            6 => 0,
            _ => 0x80,
        },
    )
    .unwrap();
    g.set_u16(REGION + 2, if seed == 8 { u16::MAX } else { 0 })
        .unwrap();
    coords(g, SEAT + 0x40, &[0.625]);
    coords(
        g,
        REGION + 4,
        &[1.75, 0.25, 0.875, if seed < 10 { 0.0 } else { 64. }],
    );
    for i in 0..4u32 {
        let mapped = seed >= 3 && (i == 0 || (seed % 2 == 1 && i == 3));
        g.set_u32(USERS + 0xc + i * 4, if mapped { i } else { u32::MAX })
            .unwrap();
        g.set_u32(
            RECORDS + i * 0x21c + 0x2c,
            if seed == 3 || i == 3 { u32::MAX } else { 0 },
        )
        .unwrap();
        let camera = 0x4e9c8c + i * 0x358;
        coords(g, camera, &[2.5 + i as f32, -0.75, 4.25]);
        g.set_u32(camera + 0xc, 0x11a0 + i).unwrap();
        g.set_u32(camera + 0x10, if seed < 5 { u32::MAX } else { 0 })
            .unwrap();
        coords(g, camera + 0x14, &[3., 0.625, 1.125]);
        coords(g, camera + 0x20, &[0.5, 0.625, -0.25]);
        coords(g, camera + 0x2c, &[-0.75, 0.25, 0.375]);
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x155760 => c.gpr[0] = if seed == 4 { 3 } else { 1 },
        0xcaf60 => coords(g, c.gpr[6], &[4.25, 8., -0.25]),
        0x11d6a0 => coords(g, c.gpr[0], &[-0.25, 0.5, 0.75, 0.125]),
        0x11d950 => (),
        0xba1d0 => {
            coords(g, c.gpr[1], &[1., -6., 0.875]);
            return 4;
        }
        0x11bed0 => {
            g.set_u32(c.gpr[6], 0x1234).unwrap();
            g.set_u32(c.gpr[6] + 4, if seed % 2 == 1 { u32::MAX } else { 0 })
                .unwrap();
        }
        0x220790 => return 4,
        _ => panic!("unexpected listener call to {target:#x}"),
    }
    0
}
