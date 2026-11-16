//! An invented multiplayer cache: every byte below is made up here. No
//! layout value, name or data comes from a game file. Used by this crate's
//! tests and by the editor's screenshot generator (docs/editor-guide.md).
use map::{FourCC, TagId};

pub const INDEX: usize = 0x1000;
pub const META_FILE: usize = 0x1200;
pub const META: u32 = 0x9000_0000;
pub const SCNR: usize = 0x100; // payload offset inside metadata
pub const SCNR_SIZE: usize = 0x1000;
pub const FILE: usize = 0x4000;

pub fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn putf(b: &mut [u8], at: usize, v: f32) {
    put(b, at, v.to_bits());
}
pub fn id(slot: u32) -> TagId {
    TagId(0xe100_0000 | slot)
}
/// File offset of a scenario payload offset.
pub fn sfile(at: usize) -> usize {
    META_FILE + SCNR + at
}
pub fn block(b: &mut [u8], field: usize, count: u32, at: usize) {
    put(b, sfile(field), count);
    put(b, sfile(field) + 4, META + (SCNR + at) as u32);
}

pub const WEAPONS_AT: usize = 0x400;
pub const WEAPON_PALETTE_AT: usize = 0x4b0;
pub const STARTS_AT: usize = 0x500;
pub const FLAGS_AT: usize = 0x580;
pub const NETGAME_AT: usize = 0x600;
pub const SCENERY_AT: usize = 0x700;
pub const NAMES_AT: usize = 0x800;

/// A multiplayer cache with globals, a scenario, a weapon and a collection.
pub fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; FILE];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    for (o, v) in [
        (4, 8),
        (8, FILE as u32),
        (0x10, INDEX as u32),
        (0x14, 0x200),
        (0x18, 0x2000),
        (0x1c, 0x2000),
    ] {
        put(&mut b, o, v);
    }
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    put(&mut b, 0x140, 1);
    b[0x198..0x19f].copy_from_slice(b"fixture");
    // Tag names.
    let names = b"globals\0levels\\test\\fixture\0objects\\test\\blaster\0objects\\test\\kit\0";
    b[0x980..0x980 + names.len()].copy_from_slice(names);
    put(&mut b, 0x2c0, 4);
    put(&mut b, 0x2c4, 0x980);
    put(&mut b, 0x2c8, names.len() as u32);
    put(&mut b, 0x2cc, 0x900);
    let mut at = 0;
    for (i, n) in names.split(|&c| c == 0).take(4).enumerate() {
        put(&mut b, 0x900 + i * 4, at);
        at += n.len() as u32 + 1;
    }
    // Groups, tags.
    let classes = [*b"matg", *b"scnr", *b"weap", *b"itmc"];
    let groups = INDEX + 32;
    let tags = groups + classes.len() * 12;
    let payloads = [
        (0usize, 0x10usize),
        (SCNR, SCNR_SIZE),
        (0x1100, 0x10),
        (0x1200, 0x10),
    ];
    for (i, c) in classes.iter().enumerate() {
        put(&mut b, groups + i * 12, FourCC(*c).as_u32());
        put(&mut b, groups + i * 12 + 4, u32::MAX);
        put(&mut b, groups + i * 12 + 8, u32::MAX);
        let t = tags + i * 16;
        put(&mut b, t, FourCC(*c).as_u32());
        put(&mut b, t + 4, id(i as u32).0);
        put(&mut b, t + 8, META + payloads[i].0 as u32);
        put(&mut b, t + 12, payloads[i].1 as u32);
    }
    for (o, v) in [
        (0, 0x8000_0020),
        (4, classes.len() as u32),
        (8, 0x8000_0020 + 12 * classes.len() as u32),
        (12, id(1).0),
        (16, id(0).0),
        (24, classes.len() as u32),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(&mut b, INDEX + o, v);
    }
    // Unknown bytes everywhere in the block area, so preservation is tested.
    for (k, x) in b[sfile(0x400)..sfile(0x800)].iter_mut().enumerate() {
        *x = 0xa0 ^ (k as u8);
    }
    // Weapons: two placements, one palette entry naming the weapon tag.
    block(&mut b, 0x90, 2, WEAPONS_AT);
    block(&mut b, 0x98, 1, WEAPON_PALETTE_AT);
    for i in 0..2 {
        let e = sfile(WEAPONS_AT + i * 0x54);
        b[e..e + 2].copy_from_slice(&0i16.to_le_bytes());
        for k in 0..3 {
            putf(&mut b, e + 8 + 4 * k, (i * 10 + k) as f32);
            putf(&mut b, e + 0x14 + 4 * k, 0.0);
        }
        putf(&mut b, e + 0x20, 1.0);
    }
    put(&mut b, sfile(WEAPON_PALETTE_AT), FourCC(*b"weap").as_u32());
    put(&mut b, sfile(WEAPON_PALETTE_AT) + 4, id(2).0);
    // Two starting locations, one flag, one netgame item, one scenery.
    block(&mut b, 0x100, 2, STARTS_AT);
    for i in 0..2 {
        let e = sfile(STARTS_AT + i * 0x34);
        for k in 0..3 {
            putf(&mut b, e + 4 * k, -(i as f32) - k as f32);
        }
        putf(&mut b, e + 0xc, 0.5);
        b[e + 0x10..e + 0x12].copy_from_slice(&1i16.to_le_bytes());
    }
    block(&mut b, 0x118, 1, FLAGS_AT);
    let e = sfile(FLAGS_AT);
    for k in 0..4 {
        putf(&mut b, e + 4 * k, 3.0);
    }
    b[e + 0x10..e + 0x14].copy_from_slice(&[2, 0, 1, 0]);
    block(&mut b, 0x120, 1, NETGAME_AT);
    let e = sfile(NETGAME_AT);
    b[e + 0xe..e + 0x10].copy_from_slice(&30i16.to_le_bytes());
    b[e + 0x14] = 0;
    for k in 0..6 {
        putf(&mut b, e + 0x40 + 4 * k, 1.0);
    }
    put(&mut b, e + 0x58, FourCC(*b"itmc").as_u32());
    put(&mut b, e + 0x5c, id(3).0);
    block(&mut b, 0x50, 1, SCENERY_AT);
    let e = sfile(SCENERY_AT);
    b[e..e + 2].copy_from_slice(&(-1i16).to_le_bytes());
    for k in 0..3 {
        putf(&mut b, e + 8 + 4 * k, 5.0);
        putf(&mut b, e + 0x14 + 4 * k, 0.0);
    }
    putf(&mut b, e + 0x20, 0.0);
    // Object names: (type, placement) = weapon 1, scenery 0, weapon 0.
    block(&mut b, 0x48, 3, NAMES_AT);
    for (i, (ty, p)) in [(2i16, 1i16), (6, 0), (2, 0)].into_iter().enumerate() {
        let e = sfile(NAMES_AT + i * 0x24);
        b[e..e + 4].copy_from_slice(b"nm\0\0");
        b[e + 0x20..e + 0x22].copy_from_slice(&ty.to_le_bytes());
        b[e + 0x22..e + 0x24].copy_from_slice(&p.to_le_bytes());
    }
    b
}

/// The fixture with its metadata region ending the file, as in retail maps,
/// so a block can grow.
pub fn fixture_at_end() -> Vec<u8> {
    let mut b = fixture();
    let end = META_FILE + 0x2000;
    b.truncate(end);
    put(&mut b, 8, end as u32);
    b
}
