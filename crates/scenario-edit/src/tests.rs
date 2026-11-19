//! Tests on an invented cache: every byte below is made up here. No layout
//! value, name or data comes from a game file.
use crate::Error;
use crate::edit::{Command, History, Op};
use crate::geometry::{self, Camera, Ray};
use crate::layout::{self, BlockKind};
use crate::save::{self, MapSource};
use crate::scenario::{Scenario, TagRef, Value};
use map::{FourCC, Map};
use std::io::Cursor;

use crate::fixture::*;

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

/// Another tag's bytes (the tag editor's edits) save with the scenario's
/// edits and change nothing else but the checksum; the scenario's own
/// payload is refused.
#[test]
fn other_tags_patches_save_with_the_scenario_s() {
    let src = fixture();
    let mut s = read(src.clone());
    // The weapon tag's payload (slot 2, at metadata +0x1100).
    let weap = META + 0x1100;
    s.set_tag_patches(vec![(weap + 4, vec![1, 2, 3])]).unwrap();
    s.set(
        BlockKind::Weapons,
        0,
        "position",
        Value::Vec3([7.0, 1.0, 2.0]),
    )
    .unwrap();
    let out = write(&src, &s);
    let d = diff(&src, &out);
    let at = META_FILE + 0x1100 + 4;
    assert_eq!(out[at..at + 3], [1, 2, 3]);
    let x = sfile(WEAPONS_AT + 8);
    for (lo, hi) in &d {
        let ok = (*lo >= at && *hi <= at + 3)
            || (*lo >= x && *hi <= x + 4)
            || *lo >= 0x2d0 && *hi <= 0x2d4;
        assert!(ok, "{lo:#x}..{hi:#x} changed");
    }
    open(out).validate_checksum().unwrap();
    assert!(matches!(
        s.set_tag_patches(vec![(META + SCNR as u32 + 0x10, vec![0])]),
        Err(Error::Refused(_))
    ));
    // Cleared: nothing is left of them.
    s.set_tag_patches(Vec::new()).unwrap();
    s.set(
        BlockKind::Weapons,
        0,
        "position",
        Value::Vec3([0.0, 1.0, 2.0]),
    )
    .unwrap();
    assert_eq!(write(&src, &s), src);
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
    let mut ed = crate::Session::new();
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
fn decompose_inverts_rotation_and_the_transform_matrix() {
    for ypr in [
        [0.0, 0.0, 0.0],
        [0.3, -0.7, 1.1],
        [-2.5, 0.4, -0.2],
        [3.0, 1.2, 2.9],
    ] {
        let t = crate::Transform {
            position: [1.0, -2.0, 3.5],
            rotation: Some(crate::RotationField::Ypr(ypr)),
        };
        let mut m = t.matrix();
        let (p, back) = geometry::decompose(&m);
        assert_eq!(p, [1.0, -2.0, 3.5]);
        for k in 0..3 {
            assert!((back[k] - ypr[k]).abs() < 1e-4, "{ypr:?} -> {back:?}");
        }
        // A gizmo may leave scale in the axes; it is ignored.
        for v in &mut m[..12] {
            *v *= 2.0;
        }
        let (_, scaled) = geometry::decompose(&m);
        for k in 0..3 {
            assert!((scaled[k] - ypr[k]).abs() < 1e-4);
        }
    }
    // Facing-only placements turn about z.
    let f = crate::Transform {
        position: [0.0; 3],
        rotation: Some(crate::RotationField::Facing(0.5)),
    };
    assert_eq!(f.ypr(), [0.5, 0.0, 0.0]);
    // The camera's view and projection make up its view-projection.
    let cam = Camera::default();
    let vp = geometry::mul(&cam.proj(1.5), &cam.view());
    assert_eq!(vp, cam.view_proj(1.5));
    assert_eq!(geometry::flat(&vp)[4..8], vp[1]);
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
        scale: 1.0,
        object: None,
    };
    vp.set_markers(&device, &crate::render::marker_vertices(&[marker], &[]));
    vp.render(&device, &queue, &cam, size);
    let drawn = pixel(&vp);
    assert_ne!(empty, drawn);
    // Orange weapons: red dominates blue.
    assert!(drawn[0] > drawn[2], "{drawn:?}");
    // The overlay is seen through the marker, blended over it: neither
    // the marker's colour nor the overlay's alone.
    let quad = |z| geometry::Tri {
        p: [[-5.0, -5.0, z], [5.0, -5.0, z], [0.0, 5.0, z]],
        n: [0.0, 0.0, 1.0],
    };
    vp.set_overlay(
        &device,
        &crate::render::overlay_vertices(&[quad(-2.0)], [0.0, 0.0, 1.0]),
    );
    vp.render(&device, &queue, &cam, size);
    let ghosted = pixel(&vp);
    assert_ne!(ghosted, drawn);
    assert!(ghosted[2] > drawn[2], "blue mixed in: {ghosted:?}");
    assert!(ghosted[0] > 0, "the marker still shows: {ghosted:?}");
    vp.set_overlay(&device, &[]);
    vp.render(&device, &queue, &cam, size);
    assert_eq!(pixel(&vp), drawn);
}

/// Transparent parts (glass, water, cloud decks) are neither framed nor
/// picked: only opaque parts become triangles.
#[test]
fn transparent_parts_are_left_out_of_the_opaque_world() {
    let part = |transparent, z: f32| geometry::WorldPart {
        transparent,
        positions: vec![[0.0, 0.0, z], [500.0, 0.0, z], [0.0, 500.0, z]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0; 2]; 3],
        tris: vec![[0, 1, 2]],
        ..Default::default()
    };
    let tris = geometry::opaque_triangles(&[part(true, -14.0), part(false, 2.0)]);
    assert_eq!(tris.len(), 1);
    assert_eq!(tris[0].p[0][2], 2.0);
    assert_eq!(tris[0].n, [0.0, 0.0, 1.0]);
}

/// Glyphs keep a minimum size on screen: far away they grow, and so does
/// what picks them.
#[test]
fn far_glyphs_grow_and_stay_pickable() {
    let near = [0.0, 0.0, 10.0];
    let far = [0.0, 0.0, 400.0];
    assert_eq!(geometry::glyph_scale(near, [0.0; 3]), 1.0);
    assert_eq!(geometry::glyph_scale(far, [0.0; 3]), 10.0);
    let spawn = geometry::Marker {
        block: BlockKind::StartingLocations,
        index: 2,
        position: [0.0; 3],
        axes: geometry::rotation([0.0; 3]),
        scale: 1.0,
        object: None,
    };
    // 3 units to the side: a miss from close up, a hit from 400 up.
    let down = |z: f32| Ray {
        origin: [3.0, 0.0, z],
        dir: [0.0, 0.0, -1.0],
    };
    assert_eq!(geometry::pick(&[spawn], &down(10.0)), None);
    assert_eq!(
        geometry::pick(&[spawn], &down(400.0)),
        Some((BlockKind::StartingLocations, 2))
    );
    // Spawns and flags have their own glyphs, bigger than a diamond.
    for kind in [BlockKind::StartingLocations, BlockKind::NetgameFlags] {
        let m = geometry::Marker {
            block: kind,
            ..spawn
        };
        let top = geometry::marker_triangles(&m)
            .iter()
            .flatten()
            .map(|p| p[2])
            .fold(f32::MIN, f32::max);
        assert!(top > 1.0, "{kind:?} glyph is {top} tall");
    }
}

fn disc_iso() -> std::path::PathBuf {
    std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../disc/halo2.iso"))
        })
}

/// Ascension regression: its BSP has a 700-unit transparent cloud deck
/// (part kind 4, `transparent\sky_two_alpha_clouds`) at z = -14.5, below
/// the playable structure. Drawn opaque and framed, it hid the structure's
/// lower half and shrank the rest to a speck: only "the ground plane and
/// rocks" showed. The structure (six clusters) must be opaque world, the
/// deck must not, and the camera must frame the level, not the deck.
#[test]
#[ignore = "needs the owner's disc image"]
fn disc_ascension_frames_its_structure_not_its_cloud_deck() {
    let maps = MapSource::disc_maps(&disc_iso()).unwrap();
    let (_, source) = maps.iter().find(|(n, _)| n == "ascension").unwrap();
    let mut map = source.open().unwrap();
    let mut raw = geometry::RawFiles::new(vec![(map::ResourceFile::Local, source.open().unwrap())]);
    let (parts, failed) = geometry::world_parts(&mut map, &mut raw).unwrap();
    assert_eq!(failed, 0);
    let deck: Vec<_> = parts.iter().filter(|p| p.transparent).collect();
    assert!(!deck.is_empty());
    let span = |ps: &[&geometry::WorldPart]| {
        let mut b = geometry::Bounds::EMPTY;
        ps.iter().flat_map(|p| &p.positions).for_each(|&q| b.add(q));
        b.max[0] - b.min[0]
    };
    assert!(span(&deck) > 600.0, "the cloud deck spans {}", span(&deck));
    let opaque = geometry::opaque_triangles(&parts);
    // The structure: the opaque triangles inside its 50-unit footprint.
    let structure = opaque
        .iter()
        .filter(|t| t.p.iter().all(|p| p[0].abs() < 30.0 && p[1].abs() < 30.0))
        .count();
    assert!(structure > 8000, "{structure} structure triangles");
    assert!(
        !opaque
            .iter()
            .any(|t| t.p.iter().all(|p| (p[2] + 14.485).abs() < 0.01) && t.p[0][0].abs() > 300.0),
        "the cloud deck is in the opaque world"
    );

    let mut s = crate::Session::new();
    s.open_named(source.clone(), "ascension").unwrap();
    let cam = *s.camera();
    assert!(cam.distance < 150.0, "framed from {}", cam.distance);
    assert!(cam.target[0].abs() < 30.0 && cam.target[1].abs() < 30.0);
    let scene = s.scene().expect("a scene");
    assert!(
        scene.stats.textured * 2 > scene.stats.shaders,
        "{:?}",
        scene.stats
    );
    assert!(
        scene.stats.modelled * 2 > scene.stats.objects,
        "{:?}",
        scene.stats
    );
    assert!(
        scene
            .world
            .batches
            .iter()
            .any(|b| b.blend == crate::scene::Blend::Translucent)
    );
}

/// Framing drops the few placements far outside a level.
#[test]
fn trimmed_bounds_drop_outliers() {
    let mut points: Vec<geometry::Vec3> = (0..40)
        .map(|i| [i as f32 % 10.0, (i / 10) as f32, 0.0])
        .collect();
    points.push([500.0, -300.0, 80.0]);
    let b = crate::session::trimmed_bounds(&points, 0.04);
    assert_eq!(b.max, [9.0, 3.0, 0.0]);
    assert_eq!(b.min, [0.0, 0.0, 0.0]);
    // Few points: at least the middle one is kept.
    let one = crate::session::trimmed_bounds(&[[1.0, 2.0, 3.0]], 0.5);
    assert_eq!((one.min, one.max), ([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]));
}

/// Lockout regression: a few of its placements sit far outside the level,
/// so framing every placement left the level small in the view. Trimmed
/// framing must come in at least a quarter closer than the full bounds
/// would.
#[test]
#[ignore = "needs the owner's disc image"]
fn disc_lockout_frames_its_level_not_its_outliers() {
    let maps = MapSource::disc_maps(&disc_iso()).unwrap();
    let (_, source) = maps.iter().find(|(n, _)| n == "lockout").unwrap();
    let mut s = crate::Session::new();
    s.build_scenes = false;
    s.open_named(source.clone(), "lockout").unwrap();
    let mut all = geometry::Bounds::EMPTY;
    s.markers().iter().for_each(|m| all.add(m.position));
    let full = all.radius() * geometry::FRAME_DISTANCE;
    let cam = *s.camera();
    assert!(
        cam.distance < full * 0.75,
        "framed from {} (all placements: {full})",
        cam.distance
    );
}

/// The BSP is lit by its lightmap pages: most of Zanzibar's solid world
/// draws through one.
#[test]
#[ignore = "needs the owner's disc image"]
fn disc_zanzibar_world_is_lightmapped() {
    let maps = MapSource::disc_maps(&disc_iso()).unwrap();
    let (_, source) = maps.iter().find(|(n, _)| n == "zanzibar").unwrap();
    let mut s = crate::Session::new();
    s.open_named(source.clone(), "zanzibar").unwrap();
    let scene = s.scene().unwrap();
    assert!(scene.stats.lightmap_pages > 50, "{:?}", scene.stats);
    let (lit, all) = scene
        .world
        .batches
        .iter()
        .filter(|b| b.blend != crate::scene::Blend::Translucent)
        .fold((0, 0), |(l, a), b| {
            let n = b.indices.len() / 3;
            (l + if b.lightmap.is_some() { n } else { 0 }, a + n)
        });
    assert!(lit * 10 > all * 9, "{lit} of {all} solid triangles lit");
    // Pages are palette colours, opaque, and not all one value.
    let page = &scene.textures[scene.world.batches.iter().find_map(|b| b.lightmap).unwrap()];
    assert!(page.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    assert!(
        page.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] != page.rgba[..3])
    );
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

/// New string IDs save after the map's own, with the patches that use them.
#[test]
fn new_string_ids_save_with_the_patches_that_use_them() {
    let src = fixture();
    let n = open(src.clone()).strings().len();
    let mut s = read(src.clone());
    let id = (5u32 << 24) | n as u32;
    let weap = META + 0x1100;
    s.set_tag_strings(vec!["fresh".into()]);
    s.set_tag_patches(vec![(weap + 4, id.to_le_bytes().to_vec())])
        .unwrap();
    assert_eq!(s.changes().strings, ["fresh"]);
    let out = write(&src, &s);
    let mut m = open(out.clone());
    m.validate_checksum().unwrap();
    assert_eq!(m.strings().len(), n + 1);
    assert_eq!(m.strings()[n].as_deref(), Some("fresh"));
    let at = META_FILE + 0x1100 + 4;
    assert_eq!(out[at..at + 4], id.to_le_bytes());
    // A string the map has, or a wrong index, is refused at save.
    let have = open(src.clone()).strings().iter().flatten().next().cloned();
    if let Some(have) = have {
        s.set_tag_strings(vec![have]);
        let mut o = Cursor::new(Vec::new());
        assert!(save::write_patched(&mut open(src.clone()), &s, &mut o).is_err());
    }
}

/// Another tag's grown block (the tag editor's) is appended like the
/// scenario's, and only its pair changes in the source bytes.
#[test]
fn another_tags_grown_block_is_appended_and_repointed() {
    let src = fixture_at_end();
    let mut s = read(src.clone());
    // A pair in the weapon tag's payload (slot 2, at metadata +0x1100).
    let pair = META + 0x1100 + 0x10;
    let bytes: Vec<u8> = (0..0x30).collect();
    let g = crate::TagGrown {
        root: pair,
        count: 3,
        bytes: bytes.clone(),
    };
    // Its pair may not also be patched, and the scenario's bytes are its own.
    s.set_tag_patches(vec![(pair + 4, vec![1])]).unwrap();
    assert!(s.set_tag_grown(vec![g.clone()]).is_err());
    s.set_tag_patches(vec![(pair + 0x20, vec![9])]).unwrap();
    let scnr = crate::TagGrown {
        root: META + SCNR as u32 + 0x10,
        ..g.clone()
    };
    assert!(matches!(
        s.set_tag_grown(vec![scnr]),
        Err(Error::Refused(_))
    ));
    s.set_tag_grown(vec![g]).unwrap();
    assert_eq!(s.changes().tag_grown.len(), 1);
    let out = write(&src, &s);
    assert_eq!(out.len(), src.len() + 0x1000);
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let root = META_FILE + 0x1100 + 0x10;
    let patched = META_FILE + 0x1100 + 0x30;
    for (lo, hi) in diff(&src, &out[..src.len()]) {
        let ok = [
            (8, 12),
            (0x18, 0x20),
            (0x2d0, 0x2d4),
            (root, root + 8),
            (patched, patched + 1),
        ]
        .iter()
        .any(|&(a, b)| lo >= a && hi <= b);
        assert!(ok, "{lo:#x}..{hi:#x}");
    }
    assert_eq!(word(&out, root), 3);
    assert_eq!(word(&out, root + 4), META + 0x2000);
    assert_eq!(&out[src.len()..src.len() + 0x30], &bytes[..]);
    open(out).validate_checksum().unwrap();
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
    // A block between tags, in bytes none owns, reads: a grown block that
    // tags added by a later rebuild follow.
    let mut b = fixture_at_end();
    let flags = Scenario::read(&mut open(b.clone()))
        .unwrap()
        .block(BlockKind::NetgameFlags)
        .clone();
    let len = flags.len() * flags.entry(0).unwrap().len();
    assert!(0x1110 + len <= 0x1200, "{len:#x}");
    b.copy_within(sfile(FLAGS_AT)..sfile(FLAGS_AT) + len, META_FILE + 0x1110);
    put(&mut b, sfile(0x11c), META + 0x1110);
    let s = Scenario::read(&mut open(b)).unwrap();
    assert_eq!(s.block(BlockKind::NetgameFlags).entry(0), flags.entry(0));
}

#[test]
fn panel_delete_and_duplicate() {
    let dir = std::env::temp_dir().join(format!("scenario-edit-panel-ar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.map");
    std::fs::write(&path, fixture_at_end()).unwrap();
    let mut ed = crate::Session::new();
    ed.open(MapSource::file(&path)).unwrap();
    // Full blocks refuse to grow until growth is allowed.
    assert_eq!(
        ed.add_refusal(BlockKind::Weapons),
        Some(crate::session::GROWTH_REFUSAL)
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

// ---- Map packages -------------------------------------------------------

mod packages {
    use super::*;
    use crate::Session as ScenarioEditor;
    use crate::package::{self, PackageForm};
    use content::patch::Format;
    use content::{Library, Manifest, MapId, Sha256};
    use std::path::{Path, PathBuf};

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir()
                .join(format!("scenario-edit-pkg-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An editor over `bytes` saved as `<tmp>/lockout.map`, packages in
    /// `<tmp>/data/maps`.
    fn editor(t: &Temp, bytes: &[u8]) -> (ScenarioEditor, PathBuf) {
        let path = t.0.join("lockout.map");
        std::fs::write(&path, bytes).unwrap();
        let mut ed = ScenarioEditor::new();
        ed.set_data_root(t.0.join("data"));
        ed.open(MapSource::file(&path)).unwrap();
        assert_eq!(ed.map_name(), Some("lockout"));
        (ed, path)
    }

    fn form(dir: &str) -> PackageForm {
        PackageForm {
            name: "Lockout Moved".into(),
            author: "tester".into(),
            description: "a flag moved".into(),
            image: None,
            dir: dir.into(),
            activate: false,
        }
    }

    fn move_flag(ed: &mut ScenarioEditor, x: f32) {
        ed.apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            Value::Vec3([x, 3.0, 3.0]),
        ))
        .unwrap();
    }

    /// The bytes the runtime would serve for `lockout`.
    fn served(maps: &Path, base: &[u8]) -> Vec<u8> {
        let mut lib = Library::open(maps);
        assert!(lib.take_notices().iter().all(|n| !n.contains("warning")));
        let r = lib.resolve("lockout", &mut &base[..]).unwrap().unwrap();
        r.read_all(&mut &base[..]).unwrap()
    }

    fn current(ed: &ScenarioEditor, base: &MapSource) -> Vec<u8> {
        write(&base.read_base().unwrap(), ed.scenario().unwrap())
    }

    #[test]
    fn saved_package_resolves_to_the_edited_map() {
        let t = Temp::new("save");
        let src = fixture();
        let (mut ed, path) = editor(&t, &src);
        move_flag(&mut ed, 9.0);
        let edited = current(&ed, &MapSource::file(&path));
        // The writer leaves the checksum the resolver would write.
        let mut repaired = edited.clone();
        content::repair_checksum(&mut repaired);
        assert_eq!(repaired, edited);
        let saved = ed.save_package(&form("lockout-moved")).unwrap();
        assert!(!ed.is_dirty());
        assert_eq!(saved.format, Format::Ppf3);
        assert_eq!(ed.last_package().unwrap().1, saved.id);
        assert_eq!(ed.package_dir(), Some("lockout-moved"));
        let maps = ed.maps_dir();
        assert_eq!(served(&maps, &src), edited);
        // The manifest on disk.
        let text = std::fs::read_to_string(maps.join("lockout-moved/manifest.toml")).unwrap();
        let m = Manifest::from_toml(&text).unwrap();
        assert_eq!(m, saved.manifest);
        assert_eq!(m.package.name, "Lockout Moved");
        assert_eq!(m.package.author.as_deref(), Some("tester"));
        assert_eq!(m.package.description.as_deref(), Some("a flag moved"));
        assert_eq!(m.package.image, None);
        assert_eq!(m.base.map, "lockout");
        assert_eq!(m.base.sha256, Some(Sha256::of(&src)));
        assert!(!m.base.derived);
        assert_eq!(m.patches.len(), 1);
        assert!(m.packs.is_empty());
        let layer = &m.patches[0];
        let patch = std::fs::read(maps.join("lockout-moved").join(&layer.file)).unwrap();
        assert_eq!(Sha256::of(&patch), layer.sha256);
        assert_eq!(
            layer.file,
            format!("lockout-{}.ppf", &layer.sha256.to_string()[..8])
        );
        // No structure BSPs in the fixture: the hash of no bytes.
        assert_eq!(m.result.as_ref().unwrap().geometry, Some(Sha256::of(b"")));
        assert_eq!(saved.id, MapId::of(&m));
        // Only the edit and the checksum are in the patch: small.
        assert!(saved.patch_len < 60 + 2 * (9 + 12), "{}", saved.patch_len);
        // The same edit saved elsewhere has the same MapId; the source map
        // is untouched.
        let again = ed.save_package(&form("lockout-copy")).unwrap();
        assert_eq!(again.id, saved.id);
        assert_eq!(std::fs::read(&path).unwrap(), src);
        // Another directory that exists is not replaced.
        assert!(ed.save_package(&form("lockout-moved")).is_err());
    }

    #[test]
    fn reopen_edit_and_save_again_keeps_one_layer() {
        let t = Temp::new("reopen");
        let src = fixture();
        let (mut ed, path) = editor(&t, &src);
        move_flag(&mut ed, 9.0);
        let first = ed.save_package(&form("lockout-moved")).unwrap();
        // Open the plain map again, then the package over it.
        ed.open(MapSource::file(&path)).unwrap();
        assert_eq!(ed.packages().len(), 1);
        let id = ed.open_package("lockout-moved").unwrap();
        assert_eq!(id, first.id);
        assert!(!ed.is_dirty() && !ed.can_undo());
        let s = ed.scenario().unwrap();
        assert_eq!(
            s.get(BlockKind::NetgameFlags, 0, "position").unwrap(),
            Value::Vec3([9.0, 3.0, 3.0])
        );
        // The form starts from the package.
        let f = ed.form_defaults();
        assert_eq!(
            (f.name.as_str(), f.dir.as_str()),
            ("Lockout Moved", "lockout-moved")
        );
        // A second edit; opening another package over it is refused.
        ed.apply(Command::set(
            BlockKind::StartingLocations,
            1,
            "facing",
            Value::Float(2.5),
        ))
        .unwrap();
        assert!(matches!(
            ed.open_package("lockout-moved"),
            Err(Error::Refused(_))
        ));
        let mut base_ed = ScenarioEditor::new();
        base_ed.open(MapSource::file(&path)).unwrap();
        let second = ed.save_package(&f).unwrap();
        assert_ne!(second.id, first.id);
        let dir = ed.maps_dir().join("lockout-moved");
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n != "manifest.toml")
            .collect();
        assert_eq!(files, [second.manifest.patches[0].file.clone()]);
        // base -> current, both edits.
        let got = served(&ed.maps_dir(), &src);
        let mut want = read(src.clone());
        want.set(
            BlockKind::NetgameFlags,
            0,
            "position",
            Value::Vec3([9.0, 3.0, 3.0]),
        )
        .unwrap();
        want.set(BlockKind::StartingLocations, 1, "facing", Value::Float(2.5))
            .unwrap();
        assert_eq!(got, write(&src, &want));
    }

    #[test]
    fn a_grown_block_saves_as_bps_and_reopens() {
        let t = Temp::new("grow");
        let src = fixture_at_end();
        let (mut ed, _) = editor(&t, &src);
        ed.set_allow_growth(true);
        let n = ed
            .scenario()
            .unwrap()
            .block(BlockKind::NetgameEquipment)
            .len();
        ed.add_entry(BlockKind::NetgameEquipment, Some(0)).unwrap();
        let mut f = form("lockout-more");
        f.activate = true;
        let saved = ed.save_package(&f).unwrap();
        assert_eq!(saved.format, Format::Bps);
        assert!(saved.manifest.patches[0].file.ends_with(".bps"));
        let maps = ed.maps_dir();
        let got = served(&maps, &src);
        assert_eq!(got.len(), src.len() + 0x1000);
        let mut m = Map::from_reader(Cursor::new(got.clone())).unwrap();
        m.validate_checksum().unwrap();
        assert_eq!(
            Scenario::read(&mut m)
                .unwrap()
                .block(BlockKind::NetgameEquipment)
                .len(),
            n + 1
        );
        // Reopen, edit the grown entry, save: still one BPS layer from the
        // base, and the result has both.
        ed.open_package("lockout-more").unwrap();
        ed.apply(Command::set(
            BlockKind::NetgameEquipment,
            n,
            "respawn time",
            Value::Int(7),
        ))
        .unwrap();
        let again = ed.save_package(&f).unwrap();
        assert_eq!(again.format, Format::Bps);
        let got2 = served(&maps, &src);
        assert_eq!(got2.len(), got.len());
        let r = Scenario::read(&mut Map::from_reader(Cursor::new(got2)).unwrap()).unwrap();
        assert_eq!(
            r.get(BlockKind::NetgameEquipment, n, "respawn time")
                .unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn images_are_validated() {
        let t = Temp::new("image");
        let (mut ed, _) = editor(&t, &fixture());
        move_flag(&mut ed, 1.0);
        let img = t.0.join("shot.png");
        let mut f = form("lockout-img");
        f.image = Some(img.clone());
        std::fs::write(&img, b"GIF89a....").unwrap();
        assert!(
            ed.save_package(&f)
                .unwrap_err()
                .to_string()
                .contains("PNG or JPEG")
        );
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(512 * 1024 + 1, 0);
        std::fs::write(&img, &big).unwrap();
        assert!(
            ed.save_package(&f)
                .unwrap_err()
                .to_string()
                .contains("over")
        );
        f.image = Some(t.0.join("missing.png"));
        assert!(ed.save_package(&f).is_err());
        assert!(!ed.maps_dir().join("lockout-img").exists());
        assert!(ed.is_dirty(), "a refused save leaves the edits unsaved");
        let jpeg = t.0.join("shot.jpeg");
        std::fs::write(&jpeg, [0xff, 0xd8, 0xff, 0xe0, 1, 2]).unwrap();
        f.image = Some(jpeg);
        let saved = ed.save_package(&f).unwrap();
        assert_eq!(saved.manifest.package.image.as_deref(), Some("preview.jpg"));
        assert_eq!(
            std::fs::read(saved.dir.join("preview.jpg")).unwrap(),
            [0xff, 0xd8, 0xff, 0xe0, 1, 2]
        );
        // The image does not change the MapId.
        f.image = None;
        f.dir = "lockout-noimg".into();
        assert_eq!(ed.save_package(&f).unwrap().id, saved.id);
    }

    #[test]
    fn activation_picks_the_package() {
        let t = Temp::new("active");
        let src = fixture();
        let (mut ed, _) = editor(&t, &src);
        move_flag(&mut ed, 1.0);
        let mut a = form("lockout-a");
        a.activate = true;
        let first = ed.save_package(&a).unwrap();
        move_flag(&mut ed, 2.0);
        // Sorts last, so it would win by name, but is not active.
        let second = ed.save_package(&form("lockout-z")).unwrap();
        let maps = ed.maps_dir();
        let text = std::fs::read_to_string(maps.join(content::ACTIVE_FILE)).unwrap();
        assert_eq!(text.trim(), "lockout = \"lockout-a\"");
        let mut lib = Library::open(&maps);
        let r = lib.resolve("lockout", &mut &src[..]).unwrap().unwrap();
        assert_eq!(r.id, first.id);
        content::store::set_active(&maps, "lockout", None).unwrap();
        let mut lib = Library::open(&maps);
        assert_eq!(
            lib.resolve("lockout", &mut &src[..]).unwrap().unwrap().id,
            second.id
        );
    }

    #[test]
    fn form_checks_and_default_dirs() {
        assert_eq!(
            PackageForm::default_dir("lockout", "Snipers Only!"),
            "lockout-snipers-only"
        );
        assert_eq!(PackageForm::default_dir("lockout", "  "), "lockout-edit");
        assert!(PackageForm::default_dir("lockout", &"x".repeat(100)).len() <= 64);
        let t = Temp::new("form");
        // The fixture's checksum is not filled in; a real map's is.
        let mut src = fixture();
        content::repair_checksum(&mut src);
        let (mut ed, _) = editor(&t, &src);
        let mut f = form("lockout-x");
        f.name = " ".into();
        assert!(ed.save_package(&f).is_err());
        for dir in ["", "cache", "a b", "../x", ".x"] {
            let mut f = form(dir);
            f.name = "n".into();
            assert!(ed.save_package(&f).is_err(), "{dir:?}");
        }
        // An unedited map saves as an empty patch, identical when resolved.
        let saved = ed.save_package(&form("lockout-same")).unwrap();
        assert_eq!(saved.patch_len, 60);
        assert_eq!(served(&ed.maps_dir(), &src), src);
        assert!(package::packages_for(&ed.maps_dir(), "LOCKOUT").len() == 1);
        assert!(package::packages_for(&ed.maps_dir(), "midship").is_empty());
    }

    /// On the owner's disc: move a flag on a multiplayer map, save it as a
    /// package under `$TMPDIR`, resolve it as the runtime would over the
    /// disc's map, re-read the scenario, then reopen the package and save
    /// a second edit. Only the disc is read.
    #[test]
    #[ignore = "needs the owner's disc image"]
    fn disc_package_round_trip() {
        let iso = std::env::var_os("H2_DISC")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../disc/halo2.iso"))
            });
        let t = Temp::new("disc");
        let maps = MapSource::disc_maps(&iso).unwrap();
        let (name, source) = maps
            .iter()
            .find(|(_, s)| {
                s.open()
                    .is_ok_and(|m| m.header().cache_type == map::CacheType::Multiplayer)
            })
            .cloned()
            .unwrap();
        let mut ed = ScenarioEditor::new();
        ed.set_data_root(t.0.join("data"));
        ed.open_named(source.clone(), &name).unwrap();
        let s = ed.scenario().unwrap();
        assert!(!s.block(BlockKind::NetgameFlags).is_empty(), "{name}");
        ed.apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            Value::Vec3([1.0, 2.0, 3.0]),
        ))
        .unwrap();
        let edited = package::edited_bytes(&source, ed.scenario().unwrap()).unwrap();
        let mut f = form(&PackageForm::default_dir(
            &name.to_ascii_lowercase(),
            "disc test",
        ));
        f.activate = true;
        let saved = ed.save_package(&f).unwrap();
        assert_eq!(saved.format, Format::Ppf3);
        let base = source.read_base().unwrap();
        let mut lib = Library::open(ed.maps_dir());
        let r = lib.resolve(&name, &mut &base[..]).unwrap().unwrap();
        assert_eq!(r.id, saved.id);
        let got = r.read_all(&mut &base[..]).unwrap();
        assert!(
            got == edited,
            "{name}: resolved bytes differ from the edited map"
        );
        let mut m = Map::from_reader(Cursor::new(got)).unwrap();
        m.validate_checksum().unwrap();
        let geometry = package::geometry_hash(&mut source.open().unwrap()).unwrap();
        assert_eq!(
            saved.manifest.result.as_ref().unwrap().geometry,
            Some(geometry)
        );
        let r = Scenario::read(&mut m).unwrap();
        assert_eq!(
            r.get(BlockKind::NetgameFlags, 0, "position").unwrap(),
            Value::Vec3([1.0, 2.0, 3.0])
        );
        ed.open_package(&f.dir).unwrap();
        ed.apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "facing",
            Value::Float(0.5),
        ))
        .unwrap();
        let second = ed.save_package(&f).unwrap();
        let mut lib = Library::open(ed.maps_dir());
        let r = lib.resolve(&name, &mut &base[..]).unwrap().unwrap();
        assert_eq!(r.id, second.id);
        let mut m = Map::from_reader(Cursor::new(r.read_all(&mut &base[..]).unwrap())).unwrap();
        let r = Scenario::read(&mut m).unwrap();
        assert_eq!(
            r.get(BlockKind::NetgameFlags, 0, "facing").unwrap(),
            Value::Float(0.5)
        );
        eprintln!(
            "{name}: package {} ({} bytes of PPF3), geometry unchanged",
            second.dir.display(),
            second.patch_len
        );
    }
}

// ---------- The sky ----------

#[test]
fn skies_are_read_from_the_scenario_root() {
    let address = 0x9000_0000u32;
    let mut b = vec![0u8; 0x40];
    put(&mut b, 0x8, 2);
    put(&mut b, 0xc, address + 0x20);
    for (k, id) in [0xe001_0002u32, 0xffff_ffff].into_iter().enumerate() {
        put(&mut b, 0x20 + 8 * k, FourCC(*b"sky ").as_u32());
        put(&mut b, 0x24 + 8 * k, id);
    }
    assert_eq!(
        crate::scene::skies_in(&b, address),
        [map::TagId(0xe001_0002), map::TagId::NULL]
    );
    // A block running off the payload, or a root too short, names none.
    put(&mut b, 0x8, 9);
    assert!(crate::scene::skies_in(&b, address).is_empty());
    assert!(crate::scene::skies_in(&b[..0xc], address).is_empty());
    // The invented map has no sky.
    let mut m = open(fixture());
    assert!(crate::scene::scenario_skies(&mut m).is_empty());
}

#[test]
fn the_sky_turns_with_the_camera_and_never_comes_nearer() {
    let a = Camera {
        target: [10.0, -4.0, 3.0],
        distance: 25.0,
        yaw: 0.7,
        pitch: 0.2,
        ..Camera::default()
    };
    let b = Camera {
        target: [-300.0, 80.0, 40.0],
        distance: 2.0,
        ..a
    };
    // Moving the eye leaves the sky where it was.
    assert_eq!(a.sky_view_proj(1.5), b.sky_view_proj(1.5));
    // What is straight ahead is in the middle of the view, however far.
    let f = a.forward();
    let m = a.sky_view_proj(1.5);
    let mut last = 0.0;
    for d in [1.0f32, 100.0, 1e5] {
        let c = geometry::transform(&m, [f[0] * d, f[1] * d, f[2] * d, 1.0]);
        let (x, y, z) = (c[0] / c[3], c[1] / c[3], c[2] / c[3]);
        assert!(x.abs() < 1e-4 && y.abs() < 1e-4, "{x} {y}");
        assert!(z > last && z < 1.0, "depth {z} after {last}");
        last = z;
    }
    // Turning moves it.
    let turned = Camera { yaw: 1.2, ..a };
    assert_ne!(turned.sky_view_proj(1.5), m);
}

/// A box of `half` round the origin, in one batch with texture 0.
fn sky_box(half: f32) -> crate::scene::Mesh {
    sky_box_at(half, [0.5, 0.5])
}

/// [`sky_box`] with every vertex at texture coordinates `uv`.
fn sky_box_at(half: f32, uv: [f32; 2]) -> crate::scene::Mesh {
    sky_box_look(half, uv, &crate::scene::Look::plain(Some(0)))
}

/// [`sky_box_at`] drawn with `look`.
fn sky_box_look(half: f32, uv: [f32; 2], look: &crate::scene::Look) -> crate::scene::Mesh {
    let mut part = geometry::WorldPart::default();
    for z in [-half, half] {
        for y in [-half, half] {
            for x in [-half, half] {
                part.positions.push([x, y, z]);
                part.normals.push([0.0, 0.0, 1.0]);
                part.uvs.push(uv);
            }
        }
    }
    // Two triangles per face, any winding: the sky is not culled.
    for [a, b, c, d] in [
        [0, 1, 3, 2],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 3, 7, 5],
    ] {
        part.tris.push([a, b, c]);
        part.tris.push([a, c, d]);
    }
    let mut mesh = crate::scene::Mesh::default();
    mesh.push_look(look, None, crate::scene::Blend::Opaque, &part);
    mesh
}

/// A 64x64 viewport's centre pixel after [`crate::render::Viewport::render`].
fn center_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    vp: &crate::render::Viewport,
    size: [u32; 2],
) -> [u8; 4] {
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
}

/// A texture its shader clamps is drawn clamped: a sky box whose
/// coordinates all lie past the picture's bottom edge (`v` 1.25) takes
/// the bottom row (blue), where repeating would take the top (red).
#[test]
fn a_clamped_sky_texture_is_not_repeated() {
    use crate::scene::{Image, Scene, SceneSky};
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let size = [64u32, 64];
    let (red, blue) = ([230u8, 10, 10, 255], [10u8, 10, 230, 255]);
    let picture = Image {
        width: 2,
        height: 2,
        rgba: [red, red, blue, blue].concat(),
    };
    let mut scene = Scene {
        textures: vec![picture],
        lifts: vec![1.0],
        sky: Some(SceneSky {
            name: "test_sky".into(),
            mesh: sky_box_at(5.0, [0.5, 1.25]),
        }),
        ..Scene::default()
    };
    let cam = Camera {
        distance: 40.0,
        ..Camera::default()
    };
    let mut vp = crate::render::Viewport::new(&device);
    vp.set_scene(&device, &queue, &scene);
    vp.render(&device, &queue, &cam, size);
    let px = center_pixel(&device, &queue, &vp, size);
    assert!(px[0] > 180 && px[2] < 60, "repeated: {px:?}");
    scene.clamped.insert(0);
    vp.set_scene(&device, &queue, &scene);
    vp.render(&device, &queue, &cam, size);
    let px = center_pixel(&device, &queue, &vp, size);
    assert!(px[2] > 180 && px[0] < 60, "clamped: {px:?}");
}

/// A batch's detail map is multiplied into its base map doubled, at its
/// own scale: on a mid-grey base, a white detail texel brightens to near
/// white and a black one darkens to black. The sky box's coordinates
/// (`u` 0.25) sit on the centre of the detail map's left (white) texel;
/// at scale 3 (`u` 0.75) on its right (black) one.
#[test]
fn a_detail_map_multiplies_the_base_map_at_its_scale() {
    use crate::scene::{Image, Layer, Look, MAX_DETAILS, Scene, SceneSky};
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let size = [64u32, 64];
    let grey = Image {
        width: 1,
        height: 1,
        rgba: vec![128, 128, 128, 255],
    };
    let white_black = Image {
        width: 2,
        height: 1,
        rgba: [[255u8, 255, 255, 255], [0, 0, 0, 255]].concat(),
    };
    let cam = Camera {
        distance: 40.0,
        ..Camera::default()
    };
    let draw = |details: [Option<Layer>; MAX_DETAILS]| {
        let look = Look {
            details,
            ..Look::plain(Some(0))
        };
        let scene = Scene {
            textures: vec![grey.clone(), white_black.clone()],
            lifts: vec![1.0; 2],
            sky: Some(SceneSky {
                name: "test_sky".into(),
                mesh: sky_box_look(5.0, [0.25, 0.5], &look),
            }),
            ..Scene::default()
        };
        let mut vp = crate::render::Viewport::new(&device);
        vp.set_scene(&device, &queue, &scene);
        vp.render(&device, &queue, &cam, size);
        center_pixel(&device, &queue, &vp, size)
    };
    let layer = |s: f32| {
        Some(Layer {
            texture: 1,
            scale: [s, s],
        })
    };
    let plain = draw([None; MAX_DETAILS]);
    assert!((100..=156).contains(&plain[0]), "base alone: {plain:?}");
    let white = draw([layer(1.0), None]);
    assert!(white[0] > 230, "times white x2: {white:?}");
    let black = draw([layer(3.0), None]);
    assert!(black[0] < 20, "times black at scale 3: {black:?}");
}

#[test]
fn the_sky_model_is_drawn_behind_the_level_wherever_the_eye_is() {
    use crate::scene::{Image, Scene, SceneSky};
    let instance = wgpu::Instance::default();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let size = [64u32, 64];
    let center = |vp: &crate::render::Viewport| center_pixel(&device, &queue, vp, size);
    let red = Image {
        width: 1,
        height: 1,
        rgba: vec![230, 10, 10, 255],
    };
    // A small sky box (5 units): drawn at infinity, so a level 20 units
    // away still covers it, and the eye can be anywhere.
    let mut scene = Scene {
        textures: vec![red],
        lifts: vec![1.0],
        sky: Some(SceneSky {
            name: "test_sky".into(),
            mesh: sky_box(5.0),
        }),
        ..Scene::default()
    };
    let mut vp = crate::render::Viewport::new(&device);
    for target in [[0.0, 0.0, 0.0], [5000.0, -300.0, 900.0]] {
        let cam = Camera {
            target,
            distance: 40.0,
            ..Camera::default()
        };
        vp.set_scene(&device, &queue, &scene);
        assert!(vp.has_sky());
        vp.render(&device, &queue, &cam, size);
        let px = center(&vp);
        assert!(px[0] > 180 && px[1] < 60 && px[2] < 60, "the sky: {px:?}");
    }
    // A wall of the level in front of the camera hides the sky.
    let cam = Camera {
        target: [0.0; 3],
        distance: 40.0,
        ..Camera::default()
    };
    let mut wall = geometry::WorldPart::default();
    let (c, f) = (cam.target, cam.forward());
    let r = geometry::normalize(geometry::cross(f, [0.0, 0.0, 1.0]));
    let u = geometry::cross(r, f);
    for (dx, dy) in [(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0)] {
        wall.positions.push(geometry::add(
            c,
            geometry::add(geometry::scale(r, dx), geometry::scale(u, dy)),
        ));
        wall.normals.push(geometry::scale(f, -1.0));
        wall.uvs.push([0.0; 2]);
    }
    wall.tris = vec![[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]];
    scene
        .world
        .push(None, None, crate::scene::Blend::Opaque, &wall);
    vp.set_scene(&device, &queue, &scene);
    vp.render(&device, &queue, &cam, size);
    let px = center(&vp);
    assert!(
        !(px[0] > 180 && px[1] < 60),
        "the level is in front: {px:?}"
    );
    // Without a sky model, the gradient.
    scene.sky = None;
    vp.set_scene(&device, &queue, &scene);
    assert!(!vp.has_sky());
}

/// A map's shared caches come from beside it, or from `shared` when the
/// source names one (a Map Pack map leans on the Halo 2 disc's).
#[test]
fn shared_caches_come_from_the_named_place() {
    let root = std::env::temp_dir().join(format!("h2-shared-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pack = root.join("pack");
    let base = root.join("base");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::create_dir_all(&base).unwrap();
    let map = pack.join("backwash.map");
    std::fs::write(&map, b"x").unwrap();
    std::fs::write(base.join("shared.map"), b"x").unwrap();
    std::fs::write(base.join("mainmenu.map"), b"x").unwrap();
    // Beside the map there is nothing; the base folder has two.
    assert!(geometry::shared_sources(&MapSource::file(&map)).is_empty());
    let with = MapSource::file(&map).with_shared(Some(base.clone()));
    let found = geometry::shared_sources(&with);
    let mut paths: Vec<_> = found.iter().map(|(_, s)| s.path.clone()).collect();
    paths.sort();
    assert_eq!(paths, [base.join("mainmenu.map"), base.join("shared.map")]);
    // Dropping the in-memory bytes keeps where the caches are.
    assert_eq!(with.base().shared, Some(base));
    let _ = std::fs::remove_dir_all(&root);
}

/// A package resolved over its base keeps the base's path and region, so
/// its bytes key the scene cache: opening it must not reuse the base's art
/// (a modded map's own scenery, shaders and sky), while the same bytes
/// again may.
#[test]
fn a_package_in_memory_does_not_reuse_its_bases_scene() {
    use crate::session::scene_key;
    let base = MapSource::region("disc.iso", Some((0x1000, 0x2000)));
    let with = |b: &[u8]| MapSource {
        bytes: Some(std::sync::Arc::from(b.to_vec())),
        ..base.clone()
    };
    let disc = scene_key(&base, "invented", "");
    let edit = scene_key(&with(b"one map"), "invented", "");
    assert_ne!(disc, edit);
    assert_ne!(edit, scene_key(&with(b"another"), "invented", ""));
    assert_eq!(edit, scene_key(&with(b"one map"), "invented", ""));
    assert_eq!(disc, scene_key(&base.clone(), "invented", ""));
}
