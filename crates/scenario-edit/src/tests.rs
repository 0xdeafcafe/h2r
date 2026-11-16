//! Tests on an invented cache: every byte below is made up here. No layout
//! value, name or data comes from a game file.
use crate::Error;
use crate::edit::{Command, History, Op};
use crate::geometry::{self, Camera, Ray};
use crate::layout::{self, BlockKind};
use crate::save::{self, MapSource};
use crate::scenario::{Scenario, TagRef, Value};
use map::{FourCC, Map, TagId};
use std::io::Cursor;

const INDEX: usize = 0x1000;
const META_FILE: usize = 0x1200;
const META: u32 = 0x9000_0000;
const SCNR: usize = 0x100; // payload offset inside metadata
const SCNR_SIZE: usize = 0x1000;
const FILE: usize = 0x4000;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn putf(b: &mut [u8], at: usize, v: f32) {
    put(b, at, v.to_bits());
}
fn id(slot: u32) -> TagId {
    TagId(0xe100_0000 | slot)
}
/// File offset of a scenario payload offset.
fn sfile(at: usize) -> usize {
    META_FILE + SCNR + at
}
fn block(b: &mut [u8], field: usize, count: u32, at: usize) {
    put(b, sfile(field), count);
    put(b, sfile(field) + 4, META + (SCNR + at) as u32);
}

const WEAPONS_AT: usize = 0x400;
const WEAPON_PALETTE_AT: usize = 0x4b0;
const STARTS_AT: usize = 0x500;
const FLAGS_AT: usize = 0x580;
const NETGAME_AT: usize = 0x600;
const SCENERY_AT: usize = 0x700;
const NAMES_AT: usize = 0x800;

/// A multiplayer cache with globals, a scenario, a weapon and a collection.
fn fixture() -> Vec<u8> {
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
fn fixture_at_end() -> Vec<u8> {
    let mut b = fixture();
    let end = META_FILE + 0x2000;
    b.truncate(end);
    put(&mut b, 8, end as u32);
    b
}

fn open(bytes: Vec<u8>) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes)).unwrap()
}
fn read(bytes: Vec<u8>) -> Scenario {
    Scenario::read(&mut open(bytes)).unwrap()
}
fn write(src: &[u8], s: &Scenario) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    save::write_patched(&mut open(src.to_vec()), s, &mut out).unwrap();
    out.into_inner()
}
/// Byte ranges where two equal-length buffers differ.
fn diff(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    assert_eq!(a.len(), b.len());
    let mut out = Vec::new();
    let mut k = 0;
    while k < a.len() {
        if a[k] == b[k] {
            k += 1;
            continue;
        }
        let s = k;
        while k < a.len() && a[k] != b[k] {
            k += 1;
        }
        out.push((s, k));
    }
    out
}
fn covers(d: &[(usize, usize)], lo: usize, hi: usize) -> bool {
    d.iter()
        .all(|&(s, e)| (s >= lo && e <= hi) || (s >= 0x2d0 && e <= 0x2d4))
}

#[test]
fn reads_typed_fields_and_palette() {
    let s = read(fixture());
    assert_eq!(s.tag, id(1));
    assert_eq!(s.block(BlockKind::Weapons).len(), 2);
    assert_eq!(s.block(BlockKind::Vehicles).len(), 0);
    assert_eq!(
        s.block(BlockKind::Weapons).palette,
        vec![TagRef {
            group: FourCC(*b"weap"),
            id: id(2)
        }]
    );
    assert_eq!(
        s.get(BlockKind::Weapons, 1, "position").unwrap(),
        Value::Vec3([10.0, 11.0, 12.0])
    );
    assert_eq!(
        s.get(BlockKind::Weapons, 0, "palette index").unwrap(),
        Value::Int(0)
    );
    assert_eq!(
        s.get(BlockKind::StartingLocations, 1, "facing").unwrap(),
        Value::Float(0.5)
    );
    assert_eq!(
        s.get(BlockKind::StartingLocations, 1, "team").unwrap(),
        Value::Int(1)
    );
    assert_eq!(
        s.get(BlockKind::NetgameFlags, 0, "type").unwrap(),
        Value::Int(2)
    );
    assert_eq!(
        s.get(BlockKind::NetgameFlags, 0, "team").unwrap(),
        Value::Int(1)
    );
    assert_eq!(
        s.get(BlockKind::NetgameEquipment, 0, "respawn time")
            .unwrap(),
        Value::Int(30)
    );
    assert_eq!(
        s.get(BlockKind::NetgameEquipment, 0, "item").unwrap(),
        Value::Tag(TagRef {
            group: FourCC(*b"itmc"),
            id: id(3)
        })
    );
    assert!(matches!(
        s.get(BlockKind::Weapons, 2, "position"),
        Err(Error::NoEntry(..))
    ));
    assert!(matches!(
        s.get(BlockKind::Weapons, 0, "nope"),
        Err(Error::NoField(_))
    ));
    // Labels come from the map's own tag names.
    assert_eq!(s.label(BlockKind::Weapons, 0), "0: blaster");
    assert_eq!(s.label(BlockKind::NetgameEquipment, 0), "0: kit");
    assert_eq!(s.label(BlockKind::Scenery, 0), "0");
    assert!(!s.is_modified());
}

#[test]
fn every_block_definition_is_consistent() {
    for b in layout::BLOCKS {
        for f in b.fields {
            assert!(f.offset + f.ty.size() <= b.stride, "{} {}", b.label, f.name);
        }
        // Fields never overlap.
        let mut spans: Vec<_> = b
            .fields
            .iter()
            .map(|f| (f.offset, f.offset + f.ty.size()))
            .collect();
        spans.sort();
        assert!(spans.windows(2).all(|w| w[0].1 <= w[1].0), "{}", b.label);
    }
    // Object placement strides match the engine type table already in tags.
    for t in tags::object::OBJECT_TYPES.iter() {
        let Some((placement, palette, stride)) = t.placement else {
            continue;
        };
        if let Some(b) = layout::BLOCKS
            .iter()
            .find(|b| b.offset == u32::from(placement))
        {
            assert_eq!(b.stride, u32::from(stride), "{}", b.label);
            assert_eq!(b.palette.as_ref().unwrap().offset, u32::from(palette));
        }
    }
}

#[test]
fn unedited_save_is_byte_identical() {
    let src = fixture();
    let s = read(src.clone());
    assert!(s.changes().is_empty());
    assert_eq!(write(&src, &s), src);
}

#[test]
fn moving_a_weapon_patches_only_its_position_and_the_checksum() {
    let src = fixture();
    let mut s = read(src.clone());
    let old = s
        .set(
            BlockKind::Weapons,
            1,
            "position",
            Value::Vec3([10.0, 11.5, 12.0]),
        )
        .unwrap();
    assert_eq!(old, Value::Vec3([10.0, 11.0, 12.0]));
    // Only the y float changed, so one 4-byte patch (or less) is queued.
    let p = s.changes().patches;
    assert_eq!(p.len(), 1);
    let pos = META + (SCNR + WEAPONS_AT) as u32 + 0x54 + 8;
    assert!(p[0].0 >= pos + 4 && p[0].0 + p[0].1.len() as u32 <= pos + 8);
    let out = write(&src, &s);
    let d = diff(&src, &out);
    let y = sfile(WEAPONS_AT + 0x54 + 0xc);
    assert!(covers(&d, y, y + 4), "{d:x?}");
    // The checksum is repaired, and the output reads back with the edit.
    let mut m = open(out.clone());
    m.validate_checksum().unwrap();
    let again = Scenario::read(&mut m).unwrap();
    assert_eq!(
        again.get(BlockKind::Weapons, 1, "position").unwrap(),
        Value::Vec3([10.0, 11.5, 12.0])
    );
    assert_eq!(
        again.get(BlockKind::Weapons, 0, "position").unwrap(),
        Value::Vec3([0.0, 1.0, 2.0])
    );
    // Unknown bytes of the edited element are untouched.
    let e = sfile(WEAPONS_AT + 0x54);
    assert_eq!(out[e + 0x24..e + 0x54], src[e + 0x24..e + 0x54]);
}

#[test]
fn every_editable_field_writes_only_its_own_bytes() {
    let src = fixture();
    let base = read(src.clone());
    for b in &base.blocks {
        for f in b.def.fields.iter().filter(|f| f.editable) {
            for index in 0..b.len() {
                let mut s = base.clone();
                let new = match s.get(b.def.kind, index, f.name).unwrap() {
                    Value::Int(_) if f.role == crate::Role::PaletteIndex => Value::Int(-1),
                    Value::Int(v) => Value::Int(if v == 1 { 0 } else { 1 }),
                    Value::Float(v) => Value::Float(v + 0.25),
                    Value::Vec3(v) => Value::Vec3(v.map(|x| x + 0.25)),
                    Value::Tag(_) => Value::Tag(TagRef {
                        group: FourCC(*b"weap"),
                        id: id(2),
                    }),
                };
                s.set(b.def.kind, index, f.name, new).unwrap();
                let out = write(&src, &s);
                let at = META_FILE
                    + (b.address - META) as usize
                    + index * b.def.stride as usize
                    + f.offset as usize;
                let d = diff(&src, &out);
                assert!(
                    !d.is_empty() || s.changes().is_empty(),
                    "{} {}",
                    b.def.label,
                    f.name
                );
                assert!(
                    covers(&d, at, at + f.ty.size() as usize),
                    "{} {} {d:x?}",
                    b.def.label,
                    f.name
                );
                assert_eq!(read(out).get(b.def.kind, index, f.name).unwrap(), new);
            }
        }
    }
}

#[test]
fn validation_refuses_bad_values() {
    let mut s = read(fixture());
    use BlockKind::*;
    let bad = [
        (Weapons, "palette index", Value::Int(1)),
        (Weapons, "palette index", Value::Int(-2)),
        (Weapons, "position", Value::Vec3([f32::NAN, 0.0, 0.0])),
        (Weapons, "position", Value::Float(1.0)),
        (Weapons, "scale", Value::Float(-1.0)),
        (Weapons, "name index", Value::Int(0)),
        (NetgameFlags, "team", Value::Int(0x10000)),
        (NetgameEquipment, "type", Value::Int(256)),
        (
            NetgameEquipment,
            "item",
            Value::Tag(TagRef {
                group: FourCC(*b"weap"),
                id: id(9),
            }),
        ),
        (
            NetgameEquipment,
            "item",
            Value::Tag(TagRef {
                group: FourCC(*b"itmc"),
                id: id(2),
            }),
        ),
        (
            NetgameEquipment,
            "item",
            Value::Tag(TagRef {
                group: FourCC(*b"scnr"),
                id: id(1),
            }),
        ),
        (StartingEquipment, "collection 1", Value::Tag(TagRef::NONE)),
    ];
    for (k, f, v) in bad {
        assert!(s.set(k, 0, f, v).is_err(), "{k:?} {f} {v:?}");
    }
    assert!(s.changes().is_empty());
    s.set(Weapons, 0, "palette index", Value::Int(-1)).unwrap();
    s.set(NetgameEquipment, 0, "item", Value::Tag(TagRef::NONE))
        .unwrap();
}

#[test]
fn undo_redo_and_drag_merging() {
    let src = fixture();
    let mut s = read(src.clone());
    let mut h = History::default();
    let pos = |s: &Scenario| s.get(BlockKind::Weapons, 0, "position").unwrap();
    // A drag: many sets with one merge key are one undo step.
    for k in 1..=5 {
        h.apply(
            &mut s,
            Command::set(
                BlockKind::Weapons,
                0,
                "position",
                Value::Vec3([k as f32, 1.0, 2.0]),
            )
            .merging(7),
        )
        .unwrap();
    }
    h.seal();
    h.apply(
        &mut s,
        Command::set(BlockKind::NetgameFlags, 0, "team", Value::Int(0)),
    )
    .unwrap();
    assert_eq!(pos(&s), Value::Vec3([5.0, 1.0, 2.0]));
    h.undo(&mut s).unwrap();
    assert_eq!(
        s.get(BlockKind::NetgameFlags, 0, "team").unwrap(),
        Value::Int(1)
    );
    h.undo(&mut s).unwrap();
    assert_eq!(pos(&s), Value::Vec3([0.0, 1.0, 2.0]));
    assert!(!h.can_undo());
    assert!(s.changes().is_empty());
    assert_eq!(write(&src, &s), src);
    h.redo(&mut s).unwrap();
    assert_eq!(pos(&s), Value::Vec3([5.0, 1.0, 2.0]));
    // A new command clears the redo stack.
    h.apply(
        &mut s,
        Command::set(BlockKind::Weapons, 1, "scale", Value::Float(2.0)),
    )
    .unwrap();
    assert!(!h.can_redo());
    // A failing command leaves the scenario as it was.
    let before = s.clone().changes();
    let cmd = Command {
        label: "two".into(),
        ops: vec![
            Op::Set(crate::SetField {
                block: BlockKind::Weapons,
                index: 0,
                field: "scale",
                value: Value::Float(3.0),
            }),
            Op::Set(crate::SetField {
                block: BlockKind::Weapons,
                index: 0,
                field: "scale",
                value: Value::Float(-3.0),
            }),
        ],
        merge: None,
    };
    assert!(h.apply(&mut s, cmd).is_err());
    assert_eq!(s.changes(), before);
}

#[test]
fn undo_restores_source_values_the_validator_would_refuse() {
    let mut src = fixture();
    // A source palette index past the palette (as a damaged map might have).
    let e = sfile(WEAPONS_AT);
    src[e..e + 2].copy_from_slice(&9i16.to_le_bytes());
    let mut s = read(src.clone());
    let mut h = History::default();
    h.apply(
        &mut s,
        Command::set(BlockKind::Weapons, 0, "palette index", Value::Int(0)),
    )
    .unwrap();
    h.undo(&mut s).unwrap();
    assert_eq!(
        s.get(BlockKind::Weapons, 0, "palette index").unwrap(),
        Value::Int(9)
    );
    assert!(s.changes().is_empty());
}

#[test]
fn malformed_blocks_are_refused() {
    // A block running past the scenario payload.
    let mut b = fixture();
    put(&mut b, sfile(0x90), 100);
    assert!(Scenario::read(&mut open(b)).is_err());
    // A block pointer before the payload.
    let mut b = fixture();
    put(&mut b, sfile(0x94), META);
    assert!(Scenario::read(&mut open(b)).is_err());
    // Two placement blocks sharing bytes.
    let mut b = fixture();
    block(&mut b, 0x118, 1, STARTS_AT);
    assert!(matches!(
        Scenario::read(&mut open(b)),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn saving_against_another_map_is_refused() {
    let s = read(fixture());
    let mut other = fixture();
    // Same layout, different scenario size.
    put(
        &mut other,
        INDEX + 32 + 4 * 12 + 16 + 12,
        SCNR_SIZE as u32 - 0x10,
    );
    let mut out = Cursor::new(Vec::new());
    assert!(save::write_patched(&mut open(other), &s, &mut out).is_err());
}

#[test]
fn save_as_writes_a_new_file_and_never_the_source() {
    let dir = std::env::temp_dir().join(format!("scenario-edit-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src_path = dir.join("source.map");
    let src = fixture();
    std::fs::write(&src_path, &src).unwrap();
    let source = MapSource::file(&src_path);
    let mut s = Scenario::read(&mut source.open().unwrap()).unwrap();
    s.set(
        BlockKind::StartingLocations,
        0,
        "facing",
        Value::Float(1.25),
    )
    .unwrap();
    assert!(save::save_as(&source, &s, &src_path).is_err());
    assert_eq!(std::fs::read(&src_path).unwrap(), src);
    let out = dir.join("edited.map");
    let n = save::save_as(&source, &s, &out).unwrap();
    assert_eq!(n as usize, src.len());
    let mut m = Map::open(&out).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(
        Scenario::read(&mut m)
            .unwrap()
            .get(BlockKind::StartingLocations, 0, "facing")
            .unwrap(),
        Value::Float(1.25)
    );
    assert!(!dir.join("edited.map.partial").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn panel_state_without_a_window() {
    let dir = std::env::temp_dir().join(format!("scenario-edit-panel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src_path = dir.join("source.map");
    std::fs::write(&src_path, fixture()).unwrap();
    let mut ed = crate::ScenarioEditor::new();
    assert!(!ed.is_dirty());
    ed.open(MapSource::file(&src_path)).unwrap();
    assert!(!ed.is_dirty() && !ed.can_undo());
    ed.apply(Command::set(
        BlockKind::NetgameFlags,
        0,
        "position",
        Value::Vec3([4.0, 3.0, 3.0]),
    ))
    .unwrap();
    assert!(ed.is_dirty());
    assert!(ed.undo());
    assert!(!ed.is_dirty());
    assert!(ed.redo());
    ed.save_as(dir.join("out.map")).unwrap();
    assert!(!ed.is_dirty());
    assert!(ed.undo());
    assert!(ed.is_dirty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rotation_matches_the_placement_convention() {
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
    let [f, l, u] = geometry::rotation([0.0; 3]);
    assert!(close(f, [1.0, 0.0, 0.0]) && close(l, [0.0, 1.0, 0.0]) && close(u, [0.0, 0.0, 1.0]));
    // Yaw turns forward toward +y; pitch tips it up; roll lifts left.
    let h = std::f32::consts::FRAC_PI_2;
    assert!(close(geometry::rotation([h, 0.0, 0.0])[0], [0.0, 1.0, 0.0]));
    assert!(close(geometry::rotation([0.0, h, 0.0])[0], [0.0, 0.0, 1.0]));
    assert!(close(geometry::rotation([0.0, 0.0, h])[1], [0.0, 0.0, 1.0]));
    // Always orthonormal and right-handed.
    let [f, l, u] = geometry::rotation([0.3, -0.7, 1.1]);
    for (a, b) in [(f, l), (l, u), (f, u)] {
        assert!(geometry::dot(a, b).abs() < 1e-5);
    }
    assert!(close(geometry::cross(f, l), u));
}

#[test]
fn camera_rays_projection_picking_and_planes() {
    let cam = Camera {
        target: [10.0, 0.0, 0.0],
        ..Camera::default()
    };
    let ray = cam.ray(0.5, 0.5, 1.5);
    assert!(geometry::dot(ray.dir, cam.forward()) > 0.9999);
    let p = cam.project([10.0, 0.0, 0.0], 1.5).unwrap();
    assert!((p[0] - 0.5).abs() < 1e-4 && (p[1] - 0.5).abs() < 1e-4);
    // A point up and to the right projects up and right, and its ray finds it.
    let q = geometry::add([10.0, 0.0, 0.0], [0.0, 0.0, 2.0]);
    let pq = cam.project(q, 1.5).unwrap();
    assert!(pq[1] < 0.5);
    let r = cam.ray(pq[0], pq[1], 1.5);
    assert!(r.sphere(q, 0.01).is_some());

    let s = read(fixture());
    let markers = geometry::markers(&s);
    // Weapons 2, starts 2, flag 1, netgame 1, scenery 1.
    assert_eq!(markers.len(), 7);
    let w1 = markers
        .iter()
        .find(|m| m.block == BlockKind::Weapons && m.index == 1)
        .unwrap();
    let down = Ray {
        origin: geometry::add(w1.position, [0.0, 0.0, 50.0]),
        dir: [0.0, 0.0, -1.0],
    };
    assert_eq!(
        geometry::pick(&markers, &down),
        Some((BlockKind::Weapons, 1))
    );
    let miss = Ray {
        origin: [100.0, 100.0, 100.0],
        dir: [0.0, 0.0, 1.0],
    };
    assert_eq!(geometry::pick(&markers, &miss), None);
    let hit = down.plane([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]).unwrap();
    assert!((hit[2] - 1.0).abs() < 1e-5 && (hit[0] - 10.0).abs() < 1e-5);
    let tri = geometry::Tri {
        p: [[0.0, 0.0, 0.0], [40.0, 0.0, 0.0], [0.0, 40.0, 0.0]],
        n: [0.0, 0.0, 1.0],
    };
    let floor = geometry::surface(&[tri], &down).unwrap();
    assert!(floor[2].abs() < 1e-5);
    assert_eq!(geometry::marker_triangles(&markers[0]).len(), 8);
}

/// Renders a marker offscreen and reads the centre pixel back. Skipped when
/// the machine has no wgpu adapter.
#[test]
fn viewport_renders_markers_offscreen() {
    use egui_wgpu::wgpu;
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut vp = crate::render::Viewport::new(&device);
    let cam = Camera {
        target: [0.0; 3],
        distance: 3.0,
        ..Camera::default()
    };
    let size = [64u32, 64];
    let pixel = |vp: &crate::render::Viewport| -> [u8; 4] {
        let tex = vp.color_texture().unwrap();
        let row = 256u32;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size[1]),
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap();
        let at = (32 * row + 32 * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    };
    vp.render(&device, &queue, &cam, size);
    let empty = pixel(&vp);
    let marker = geometry::Marker {
        block: BlockKind::Weapons,
        index: 0,
        position: [0.0; 3],
        axes: geometry::rotation([0.0; 3]),
    };
    vp.set_markers(&device, &crate::render::marker_vertices(&[marker], None));
    vp.render(&device, &queue, &cam, size);
    let drawn = pixel(&vp);
    assert_ne!(empty, drawn);
    // Orange weapons: red dominates blue.
    assert!(drawn[0] > drawn[2], "{drawn:?}");
}

/// Reads every map on the owner's disc (`H2_DISC`, else `disc/halo2.iso`),
/// read-only, and checks the field claims marked V in `layout`.
#[test]
#[ignore = "needs the owner's disc image"]
fn disc_scenarios_read_and_fields_are_plausible() {
    let iso = std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../disc/halo2.iso"))
        });
    let maps = MapSource::disc_maps(&iso).unwrap();
    let mut checked = 0;
    let (mut positions, mut outside) = (0usize, 0usize);
    for (name, source) in maps {
        let mut m = source.open().unwrap();
        if !matches!(
            m.header().cache_type,
            map::CacheType::Campaign | map::CacheType::Multiplayer
        ) {
            continue;
        }
        let s = Scenario::read(&mut m).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut bounds = geometry::Bounds::EMPTY;
        let mut files = vec![(map::ResourceFile::Local, source.open().unwrap())];
        for (f, shared) in geometry::shared_sources(&source) {
            files.push((f, shared.open().unwrap()));
        }
        let mut raw = geometry::RawFiles::new(files);
        let (tris, _) = geometry::world_triangles(&mut m, &mut raw).unwrap();
        tris.iter()
            .for_each(|t| t.p.iter().for_each(|&p| bounds.add(p)));
        for b in &s.blocks {
            for i in 0..b.len() {
                for f in b.def.fields {
                    let v = s.get(b.def.kind, i, f.name).unwrap();
                    let ctx = format!("{name} {} {i} {}", b.def.label, f.name);
                    match (f.role, v) {
                        (crate::Role::PaletteIndex, Value::Int(p)) => {
                            assert!(
                                p == -1 || (0..b.palette.len() as i64).contains(&p),
                                "{ctx}: {p}"
                            );
                        }
                        (crate::Role::Position, Value::Vec3(p)) => {
                            assert!(p.iter().all(|x| x.is_finite()), "{ctx}");
                            if !bounds.is_empty() {
                                let g = 10.0;
                                let inside = (0..3)
                                    .all(|k| p[k] > bounds.min[k] - g && p[k] < bounds.max[k] + g);
                                positions += 1;
                                if !inside {
                                    outside += 1;
                                    eprintln!("outside the BSP bounds: {ctx}");
                                }
                            }
                        }
                        (crate::Role::Rotation, Value::Vec3(r)) => {
                            assert!(
                                r.iter().all(|x| x.is_finite() && x.abs() <= 7.0),
                                "{ctx}: {r:?}"
                            );
                        }
                        (crate::Role::Facing, Value::Float(r)) => {
                            assert!(r.is_finite() && r.abs() <= 7.0, "{ctx}: {r}");
                        }
                        (crate::Role::Scale, Value::Float(x)) => {
                            assert!(x.is_finite() && x >= 0.0, "{ctx}: {x}")
                        }
                        (_, Value::Int(x)) if f.name == "team" => {
                            assert!((-1..16).contains(&x), "{ctx}: {x}")
                        }
                        (_, Value::Int(x))
                            if b.def.kind == BlockKind::NetgameFlags && f.name == "type" =>
                        {
                            assert!((0..32).contains(&x), "{ctx}: {x}")
                        }
                        (_, Value::Tag(r)) if !r.is_none() => {
                            let t = s.tags.get(&r.id);
                            assert!(t.is_some_and(|t| t.group == r.group), "{ctx}: dangling");
                            if b.def.kind == BlockKind::NetgameEquipment {
                                assert!(
                                    layout::ITEM_GROUPS.contains(&r.group.0),
                                    "{ctx}: {}",
                                    r.group
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            for r in &b.palette {
                if !r.is_none() {
                    let groups = &b.def.palette.as_ref().unwrap().groups;
                    assert!(
                        groups.contains(&r.group.0),
                        "{name} {} palette {}",
                        b.def.label,
                        r.group
                    );
                }
            }
        }
        // No edits: the patch set is empty, so a save would be a pass-through.
        assert!(s.changes().is_empty());
        checked += 1;
    }
    assert!(checked > 0);
    eprintln!(
        "{checked} scenarios checked; {outside} of {positions} positions outside the BSP bounds (+10)"
    );
    // Some placements legitimately sit outside the drawn BSP (sky scenery,
    // objects beyond the playable area); nearly all must be inside.
    assert!(
        outside * 100 <= positions,
        "{outside} of {positions} outside"
    );
}

fn names(s: &Scenario) -> Vec<(i16, i16)> {
    (0..s.names.len())
        .map(|n| s.names.target(n).unwrap())
        .collect()
}

#[test]
fn remove_swaps_the_last_entry_in_and_repoints_names() {
    let src = fixture();
    let mut s = read(src.clone());
    let w = |s: &Scenario, i| s.block(BlockKind::Weapons).entry(i).unwrap().to_vec();
    let (w0, w1) = (w(&s, 0), w(&s, 1));
    let removed = s.remove(BlockKind::Weapons, 0).unwrap();
    assert_eq!(removed.entry, w0);
    assert_eq!(s.block(BlockKind::Weapons).len(), 1);
    assert_eq!(w(&s, 0), w1);
    // The name of weapon 1 follows it to slot 0; weapon 0's names nothing;
    // the scenery name (another type) is untouched.
    assert_eq!(names(&s), vec![(2, 0), (6, 0), (2, -1)]);

    let out = write(&src, &s);
    let d = diff(&src, &out);
    let slot0 = sfile(WEAPONS_AT);
    let count = sfile(0x90);
    let n0 = sfile(NAMES_AT);
    for &(lo, hi) in &d {
        let inside = |a: usize, b: usize| lo >= a && hi <= b;
        assert!(
            inside(slot0, slot0 + 0x54)
                || inside(count, count + 4)
                || inside(n0, n0 + 3 * 0x24)
                || inside(0x2d0, 0x2d4),
            "{lo:#x}..{hi:#x}"
        );
    }
    // The old tail slot is left as dead data.
    let tail = sfile(WEAPONS_AT + 0x54);
    assert_eq!(out[tail..tail + 0x54], src[tail..tail + 0x54]);
    let mut m = open(out.clone());
    m.validate_checksum().unwrap();
    let again = Scenario::read(&mut m).unwrap();
    assert_eq!(again.block(BlockKind::Weapons).len(), 1);
    assert_eq!(again.block(BlockKind::Weapons).entry(0).unwrap(), &w1[..]);
    assert_eq!(names(&again), vec![(2, 0), (6, 0), (2, -1)]);

    // Undo restores the count, both slots and the names exactly.
    s.unremove(&removed).unwrap();
    assert_eq!((w(&s, 0), w(&s, 1)), (w0, w1));
    assert_eq!(names(&s), vec![(2, 1), (6, 0), (2, 0)]);
    assert!(s.changes().is_empty());
}

#[test]
fn removing_the_last_entry_and_every_block_kind() {
    let src = fixture();
    let mut s = read(src.clone());
    let r = s.remove(BlockKind::Weapons, 1).unwrap();
    assert_eq!(s.block(BlockKind::Weapons).len(), 1);
    // Weapon 1's name now names nothing; weapon 0's is unchanged.
    assert_eq!(names(&s), vec![(2, -1), (6, 0), (2, 0)]);
    // Only the count and the name change: slot 0 is untouched.
    let c = s.changes();
    assert_eq!(c.patches.len(), 2, "{c:x?}");
    s.unremove(&r).unwrap();
    assert!(s.changes().is_empty());
    for b in &read(src.clone()).blocks {
        let kind = b.def.kind;
        assert!(s.remove_refusal(kind).is_none(), "{kind:?}");
        if b.is_empty() {
            assert!(matches!(s.remove(kind, 0), Err(Error::NoEntry(..))));
            continue;
        }
        let before = s.clone();
        let r = s.remove(kind, 0).unwrap();
        assert_eq!(s.block(kind).len(), b.len() - 1);
        let out = write(&src, &s);
        assert_eq!(read(out).block(kind).len(), b.len() - 1, "{kind:?}");
        s.unremove(&r).unwrap();
        assert_eq!(s.changes(), before.changes());
    }
}

#[test]
fn remove_and_add_through_history_undo_exactly() {
    let src = fixture();
    let mut s = read(src.clone());
    let mut h = History::default();
    let w1 = s.block(BlockKind::Weapons).entry(1).unwrap().to_vec();
    h.apply(&mut s, Command::remove(BlockKind::Weapons, 0))
        .unwrap();
    // Adding back in place needs no growth: same count, slot 1 rewritten.
    let e = s
        .new_entry(BlockKind::Weapons, Some(0), Some([7.0, 8.0, 9.0]))
        .unwrap();
    h.apply(&mut s, Command::add(BlockKind::Weapons, e.clone()))
        .unwrap();
    assert_eq!(s.block(BlockKind::Weapons).len(), 2);
    let c = s.changes();
    assert!(c.grown.is_empty());
    assert!(
        !c.patches
            .iter()
            .any(|(a, _)| *a == META + (SCNR + 0x90) as u32)
    );
    assert_eq!(
        s.get(BlockKind::Weapons, 1, "position").unwrap(),
        Value::Vec3([7.0, 8.0, 9.0])
    );
    // A copy keeps every other byte of its source entry.
    assert_eq!(e[..8], w1[..8]);
    assert_eq!(e[0x14..], w1[0x14..]);
    let out = write(&src, &s);
    assert_eq!(read(out).block(BlockKind::Weapons).len(), 2);
    assert_eq!(h.undo(&mut s).unwrap().as_deref(), Some("add Weapons"));
    assert_eq!(s.block(BlockKind::Weapons).len(), 1);
    h.undo(&mut s).unwrap();
    assert!(s.changes().is_empty());
    assert_eq!(names(&s), vec![(2, 1), (6, 0), (2, 0)]);
    h.redo(&mut s).unwrap();
    h.redo(&mut s).unwrap();
    assert_eq!(s.changes(), c);
    // A command that fails part-way is rolled back.
    let before = s.clone().changes();
    let bad = Command {
        label: "bad".into(),
        ops: vec![
            Op::Remove {
                block: BlockKind::Weapons,
                index: 0,
            },
            Op::Remove {
                block: BlockKind::Weapons,
                index: 5,
            },
        ],
        merge: None,
    };
    assert!(h.apply(&mut s, bad).is_err());
    assert_eq!(s.changes(), before);
    assert_eq!(s.block(BlockKind::Weapons).len(), 2);
}

#[test]
fn new_entries_and_add_limits() {
    let mut s = read(fixture());
    // Zeroed, with the palette's first item and no name.
    let e = s
        .new_entry(BlockKind::Weapons, None, Some([1.0, 2.0, 3.0]))
        .unwrap();
    assert_eq!(e.len(), 0x54);
    assert_eq!(&e[..4], &[0, 0, 0xff, 0xff]);
    assert_eq!(f32::from_le_bytes(e[0xc..0x10].try_into().unwrap()), 2.0);
    assert!(e[0x14..].iter().all(|&b| b == 0));
    // No palette: -1.
    let e = s.new_entry(BlockKind::Vehicles, None, None).unwrap();
    assert_eq!(&e[..2], &[0xff, 0xff]);
    // Blocks without a palette or name start zeroed.
    let e = s.new_entry(BlockKind::NetgameFlags, None, None).unwrap();
    assert!(e.iter().all(|&b| b == 0));
    assert!(s.new_entry(BlockKind::Weapons, Some(9), None).is_err());
    assert!(
        s.new_entry(BlockKind::Weapons, None, Some([f32::NAN, 0.0, 0.0]))
            .is_err()
    );
    // Wrong length.
    assert!(s.add(BlockKind::Weapons, vec![0; 4]).is_err());
    // The engine's entry limit: 100 netgame equipment entries.
    let e = s
        .new_entry(BlockKind::NetgameEquipment, Some(0), None)
        .unwrap();
    for _ in 1..100 {
        s.add(BlockKind::NetgameEquipment, e.clone()).unwrap();
    }
    assert_eq!(s.block(BlockKind::NetgameEquipment).len(), 100);
    assert!(s.add_refusal(BlockKind::NetgameEquipment).is_some());
    assert!(matches!(
        s.add(BlockKind::NetgameEquipment, e),
        Err(Error::Refused(_))
    ));
    // unadd only takes back the entry that is last.
    assert!(s.unadd(BlockKind::Weapons, &[0; 0x54]).is_err());
}

#[test]
fn a_grown_block_is_appended_and_nothing_else_moves() {
    let src = fixture_at_end();
    let mut s = read(src.clone());
    let e = s
        .new_entry(BlockKind::NetgameEquipment, Some(0), Some([4.0, 5.0, 6.0]))
        .unwrap();
    s.add(BlockKind::NetgameEquipment, e.clone()).unwrap();
    let c = s.changes();
    assert_eq!(c.grown.len(), 1);
    assert_eq!(c.grown[0].count, 2);
    let out = write(&src, &s);
    // The region, the total and the file grow by one page.
    assert_eq!(out.len(), src.len() + 0x1000);
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    assert_eq!(word(&out, 8), word(&src, 8) + 0x1000);
    assert_eq!(word(&out, 0x18), word(&src, 0x18) + 0x1000);
    assert_eq!(word(&out, 0x1c), word(&src, 0x1c) + 0x1000);
    // Every source byte is unchanged except those header fields, the
    // checksum and the block's (count, address) pair.
    let root = sfile(0x120);
    for (lo, hi) in diff(&src, &out[..src.len()]) {
        let ok = [(8, 12), (0x18, 0x20), (0x2d0, 0x2d4), (root, root + 8)]
            .iter()
            .any(|&(a, b)| lo >= a && hi <= b);
        assert!(ok, "{lo:#x}..{hi:#x}");
    }
    // The new copy starts right after the old region's end.
    let end = META + 0x2000;
    assert_eq!(word(&out, root), 2);
    assert_eq!(word(&out, root + 4), end);
    let old = &src[sfile(NETGAME_AT)..sfile(NETGAME_AT) + 0x90];
    assert_eq!(&out[src.len()..src.len() + 0x90], old);
    assert_eq!(&out[src.len() + 0x90..src.len() + 0x120], &e[..]);
    assert!(out[src.len() + 0x120..].iter().all(|&b| b == 0));
    let mut m = open(out.clone());
    m.validate_checksum().unwrap();
    let mut again = Scenario::read(&mut m).unwrap();
    let b = again.block(BlockKind::NetgameEquipment);
    assert_eq!((b.len(), b.address), (2, end));
    assert_eq!(
        again
            .get(BlockKind::NetgameEquipment, 1, "position")
            .unwrap(),
        Value::Vec3([4.0, 5.0, 6.0])
    );
    // The re-read map edits its grown block in place.
    again
        .set(
            BlockKind::NetgameEquipment,
            1,
            "respawn time",
            Value::Int(5),
        )
        .unwrap();
    let out2 = write(&out, &again);
    assert_eq!(out2.len(), out.len());
    let d = diff(&out, &out2);
    let field = src.len() + 0x90 + 0xe;
    assert!(covers(&d, field, field + 2), "{d:x?}");
    // Undoing the add needs no growth again.
    s.unadd(BlockKind::NetgameEquipment, &e).unwrap();
    assert_eq!(write(&src, &s), src);
}

#[test]
fn growth_is_refused_when_the_metadata_does_not_end_the_file() {
    let src = fixture();
    let mut s = read(src.clone());
    let e = s.new_entry(BlockKind::NetgameFlags, Some(0), None).unwrap();
    s.add(BlockKind::NetgameFlags, e).unwrap();
    let mut out = Cursor::new(Vec::new());
    assert!(save::write_patched(&mut open(src), &s, &mut out).is_err());
    // A block inside another tag is refused on read.
    let mut b = fixture_at_end();
    put(&mut b, sfile(0x11c), META + 0x1100);
    assert!(Scenario::read(&mut open(b)).is_err());
}

#[test]
fn panel_delete_and_duplicate() {
    let dir = std::env::temp_dir().join(format!("scenario-edit-panel-ar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.map");
    std::fs::write(&path, fixture_at_end()).unwrap();
    let mut ed = crate::ScenarioEditor::new();
    ed.open(MapSource::file(&path)).unwrap();
    // Full blocks refuse to grow until growth is allowed.
    assert_eq!(
        ed.add_refusal(BlockKind::Weapons),
        Some(crate::panel::GROWTH_REFUSAL)
    );
    assert!(ed.add_entry(BlockKind::Weapons, Some(0)).is_err());
    ed.remove_entry(BlockKind::Weapons, 0).unwrap();
    assert_eq!(ed.selected(), Some((BlockKind::Weapons, 0)));
    assert!(ed.is_dirty());
    // Room in place again: a duplicate fits without growth.
    assert_eq!(ed.add_refusal(BlockKind::Weapons), None);
    assert_eq!(ed.add_entry(BlockKind::Weapons, Some(0)).unwrap(), 1);
    assert_eq!(ed.selected(), Some((BlockKind::Weapons, 1)));
    ed.set_allow_growth(true);
    assert_eq!(ed.add_entry(BlockKind::Weapons, None).unwrap(), 2);
    assert!(ed.undo());
    assert!(ed.undo());
    assert!(ed.undo());
    assert!(!ed.is_dirty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// On the owner's disc: add a netgame equipment entry to a multiplayer map
/// (the block grows: appended to the metadata) and remove a weapon (in
/// place), save to `$TMPDIR`, and re-read the result with the `map`, `tags`
/// and scenario readers. Only the documented bytes may differ.
#[test]
#[ignore = "needs the owner's disc image"]
fn disc_add_and_remove_round_trip() {
    let iso = std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../disc/halo2.iso"))
        });
    let maps = MapSource::disc_maps(&iso).unwrap();
    let (mut checked, mut refused) = (0, 0);
    for (name, source) in maps {
        let mut m = source.open().unwrap();
        if m.header().cache_type != map::CacheType::Multiplayer {
            continue;
        }
        let src_len = m.storage_length() as usize;
        let mut s = Scenario::read(&mut m).unwrap();
        let kind = BlockKind::NetgameEquipment;
        let n = s.block(kind).len();
        let at = [1.0, 2.0, 3.0];
        let e = s.new_entry(kind, Some(0), Some(at)).unwrap();
        s.add(kind, e.clone()).unwrap();
        let weapons = s.block(BlockKind::Weapons).len();
        if weapons > 0 {
            s.remove(BlockKind::Weapons, 0).unwrap();
        }
        let changes = s.changes();
        assert_eq!(changes.grown.len(), 1, "{name}");
        let mut out = Cursor::new(Vec::new());
        let growth = changes.grown[0].bytes.len().div_ceil(0x1000) as u32 * 0x1000;
        let limit = map::writer::max_metadata_size(m.header().cache_type).unwrap();
        if m.header().meta_size + growth > limit {
            // The largest retail map of its type: growth is refused.
            assert!(save::write_patched(&mut m, &s, &mut out).is_err(), "{name}");
            eprintln!("{name}: growth refused at the bound, as expected");
            refused += 1;
            continue;
        }
        save::write_patched(&mut m, &s, &mut out).unwrap();
        let out = out.into_inner();
        assert_eq!(out.len() % 0x1000, src_len % 0x1000, "{name}");
        assert!(out.len() > src_len);
        // Every source byte is identical except the header sizes, the
        // checksum and the scenario patches.
        let src = m.read_file_range(0, src_len as u64).unwrap();
        let mut patched: Vec<(usize, usize)> = changes
            .patches
            .iter()
            .map(|(a, b)| {
                let f = m.file_offset(*a, b.len() as u32).unwrap() as usize;
                (f, f + b.len())
            })
            .collect();
        let root = m.file_offset(changes.grown[0].root, 8).unwrap() as usize;
        patched.extend([(8, 12), (0x18, 0x20), (0x2d0, 0x2d4), (root, root + 8)]);
        for (lo, hi) in diff(&src, &out[..src_len]) {
            assert!(
                patched.iter().any(|&(a, b)| lo >= a && hi <= b),
                "{name}: {lo:#x}..{hi:#x}"
            );
        }
        let mut again = Map::from_reader(Cursor::new(out)).unwrap();
        again.validate_checksum().unwrap();
        assert_eq!(
            again.header().meta_size - m.header().meta_size,
            again.header().tag_data_size - m.header().tag_data_size
        );
        let scnr = again.scenario().unwrap().id;
        tags::DecodedTag::read(&mut again, scnr).unwrap_or_else(|e| panic!("{name}: {e}"));
        let r = Scenario::read(&mut again).unwrap();
        assert_eq!(r.block(kind).len(), n + 1, "{name}");
        assert_eq!(r.block(kind).entry(n).unwrap(), &e[..]);
        assert_eq!(r.get(kind, n, "position").unwrap(), Value::Vec3(at));
        assert_eq!(r.block(BlockKind::Weapons).len(), weapons.saturating_sub(1));
        assert!(r.changes().is_empty());
        eprintln!(
            "{name}: +1 netgame equipment ({n} -> {}), -1 weapon, {} -> {} bytes",
            n + 1,
            src_len,
            again.storage_length()
        );
        checked += 1;
    }
    eprintln!("{checked} maps grown, {refused} refused at the bound");
    assert!(checked > 0);
}
