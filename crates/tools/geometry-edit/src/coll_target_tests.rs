//! A `coll` BSP opened as a mesh target, edited, saved through
//! `scenario_edit::save` and read back, on the invented object map of
//! `scenario_edit::fixture::fixture_with_object_collision` (its `coll`,
//! tag 6, is one BSP of a unit cube).
use super::*;
use crate::{GeometrySession, Patches, WriteClass};
use collision_build::{query, verify};
use map::RawPointer;
use mesh_edit::{MeshOp, SelectMode};
use scenario_edit::fixture::{self, META, META_FILE, OBJ_COLL};
use scenario_edit::{GeometryChanges, GeometryGrown, Scenario};
use std::io::Cursor;
use tags::geometry::RawSource;

/// A `coll` has no raw data.
struct NoRaw;
impl RawSource for NoRaw {
    fn read_raw(&mut self, _: RawPointer, _: u32) -> tags::Result<Vec<u8>> {
        Err(tags::Error::Invalid("a collision BSP has no raw data"))
    }
}

fn coll() -> TagId {
    fixture::id(6)
}

fn target() -> Target {
    Target::new(TargetKind::Coll, coll(), 0)
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn open(bytes: &[u8]) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes.to_vec())).unwrap()
}

/// The object map, with `materials` collision materials on its `coll`
/// (the invented tag has none), its metadata ending the file.
fn map_bytes(materials: u32) -> Vec<u8> {
    let mut b = fixture::fixture_with_object_collision();
    if materials > 0 {
        // Past the BSP's blocks and before the nodes at `+0x400`.
        let at = OBJ_COLL + 0x3e0;
        put(&mut b, META_FILE + OBJ_COLL + 0x14, materials);
        put(&mut b, META_FILE + OBJ_COLL + 0x18, META + at as u32);
        for i in 0..materials as usize {
            put(&mut b, META_FILE + at + 4 * i, 0x0500_0010 + i as u32);
        }
    }
    fixture::metadata_last(b)
}

fn session(bytes: &[u8]) -> GeometrySession {
    let mut s = GeometrySession::new();
    s.open(&mut open(bytes), &mut NoRaw, target()).unwrap();
    s
}

/// Every vertex moved up by `dz`.
fn lift_all(s: &mut GeometrySession, dz: f32) {
    let ops = [
        MeshOp::Mode {
            mode: SelectMode::Vert,
        },
        MeshOp::SelectAll,
        MeshOp::Transform {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, dz],
            ],
            pivot: None,
        },
    ];
    s.apply(target(), &ops, None).unwrap();
}

/// The map with `p` saved through the scenario's save.
fn saved(src: &[u8], p: &Patches) -> Vec<u8> {
    let mut map = open(src);
    let mut s = Scenario::read(&mut map).unwrap();
    let mut c = GeometryChanges::default();
    for w in &p.tags {
        assert_eq!(w.space, None, "a coll is in the ordinary metadata");
        c.patches.push((w.address, w.bytes.clone()));
    }
    c.grown = p
        .grown
        .iter()
        .map(|g| GeometryGrown {
            pair_space: g.pair_space,
            pair: g.pair,
            count: g.count,
            bytes: g.bytes.clone(),
            align: g.align,
        })
        .collect();
    s.set_geometry_changes(c).unwrap();
    let mut out = Cursor::new(Vec::new());
    scenario_edit::save::write_patched(&mut open(src), &s, &mut out).unwrap();
    out.into_inner()
}

#[test]
fn the_listing_names_the_bsp_with_its_counts() {
    let src = map_bytes(2);
    let cube = fixture::collision_cube();
    let all = crate::list(&mut open(&src));
    let c = all.iter().find(|t| t.target == target()).expect("listed");
    assert_eq!(
        (c.vertices, c.triangles, c.parts, c.local),
        (cube.vertices.len(), cube.surfaces.len(), 1, true)
    );
    assert_eq!(Target::parse(&target().id()), Some(target()));
    assert_eq!(target().id(), format!("coll_bsp:{:08x}:0", coll().0));
}

#[test]
fn the_walk_finds_every_block_where_the_tag_holds_it() {
    let src = map_bytes(2);
    let s = CollSource::read(&mut open(&src), coll()).unwrap();
    let cube = fixture::collision_cube();
    assert_eq!(s.sites.len(), 1);
    let site = &s.sites[0];
    assert_eq!(site.slot, None);
    assert_eq!(site.blocks[7].count as usize, cube.vertices.len());
    assert_eq!(site.blocks[5].count as usize, cube.surfaces.len());
    // The bytes read are the blocks the codec encodes.
    let staged = collision_build::coll::stage_blocks(&s.collision.bsps().next().unwrap().clone());
    for (k, b) in staged.iter().enumerate() {
        assert_eq!(b.bytes, site.block_bytes[k], "{}", b.name);
    }
    assert!(matches!(
        CollSource::read(&mut open(&src), fixture::id(1)),
        Err(Error::NoTarget(_))
    ));
}

#[test]
fn an_unedited_bsp_writes_nothing() {
    let src = map_bytes(2);
    let mut s = session(&src);
    let cube = fixture::collision_cube();
    assert_eq!(s.mesh(target()).unwrap().face_count(), cube.surfaces.len());
    s.apply(target(), &[MeshOp::SelectAll], None).unwrap();
    assert_eq!(s.changes().unwrap(), Patches::default());
    assert_eq!(s.check(None)[0].class, WriteClass::Unchanged);
    assert!(!s.has_changes());
}

#[test]
fn a_moved_bsp_is_compiled_saved_and_reads_back() {
    // Each of the cube's six faces has its own material.
    let src = map_bytes(6);
    let mut s = session(&src);
    lift_all(&mut s, 1.0);
    let check = &s.check(Some(target()))[0];
    assert_eq!((check.class, check.refused.clone()), (WriteClass::GrownBlocks, None));
    assert_eq!(check.class.name(), "grown_blocks");
    assert!(s.has_changes());
    let p = s.changes().unwrap();
    assert!(!p.is_empty() && p.raw.is_empty() && p.appended.is_empty());
    let out = saved(&src, &p);
    let back = Collision::read(&mut open(&out), coll()).unwrap();
    assert!(back.validate().is_empty(), "{:#?}", back.validate());
    let bsp = back.bsps().next().unwrap();
    // The leading word is kept; the compiler's own acceptance holds.
    let was = Collision::read(&mut open(&src), coll()).unwrap();
    assert_eq!(bsp.unknown_00, was.bsps().next().unwrap().unknown_00);
    assert!(verify::check(bsp, back.materials.len()).is_empty());
    // The unit cube about the origin is now over z = 0.5 to 1.5.
    let z = |f: fn(f32, f32) -> f32, init| bsp.vertices.iter().fold(init, |a, v| f(a, v.point[2]));
    assert!((z(f32::min, f32::MAX) - 0.5).abs() < 1e-4);
    assert!((z(f32::max, f32::MIN) - 1.5).abs() < 1e-4);
    assert_eq!(query::locate(bsp, [0.0, 0.0, 1.0]), None, "inside the moved cube");
    assert!(query::locate(bsp, [0.0, 0.0, 0.0]).is_some(), "where the cube was");
    // Opened again, the saved BSP is the edited mesh.
    let mut again = session(&out);
    assert_eq!(again.check(None)[0].class, WriteClass::Unchanged);
    assert!(again.mesh(target()).unwrap().face_count() > 0);
    again.close(target());
}

#[test]
fn undoing_the_edit_writes_nothing_again() {
    let src = map_bytes(2);
    let mut s = session(&src);
    lift_all(&mut s, 1.0);
    assert!(s.has_changes());
    assert!(s.undo());
    assert!(!s.has_changes());
    assert_eq!(s.changes().unwrap(), Patches::default());
}

#[test]
fn a_material_the_tag_lacks_is_refused_and_named() {
    // The invented tag as it is has no collision materials.
    let src = map_bytes(0);
    let mut s = session(&src);
    lift_all(&mut s, 1.0);
    let check = &s.check(Some(target()))[0];
    assert_eq!(check.class, WriteClass::GrownBlocks);
    assert!(check.refused.is_some());
    let e = s.changes().unwrap_err().to_string();
    assert!(e.contains(&target().id()), "{e}");
}

#[test]
fn only_a_model_section_has_levels_of_detail() {
    let src = map_bytes(2);
    let mut s = session(&src);
    let e = s.set_use_for_lower_lods(target(), true).unwrap_err();
    assert!(matches!(e, Error::Refused { .. }), "{e}");
    let section = Target::new(TargetKind::ModeSection, fixture::id(5), 0);
    let e = s.set_use_for_lower_lods(section, true).unwrap_err();
    assert!(matches!(e, Error::NotOpen(_)), "{e}");
    assert!(!s.uses_for_lower_lods(section));
    assert!(!s.can_undo());
}
