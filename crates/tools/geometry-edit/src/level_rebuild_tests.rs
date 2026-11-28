//! A sculpt stroke on a structure cluster rebuilds the level's collision,
//! per-leaf block, MOPP and portals, and the save writes them: on the
//! invented textured map with a level
//! (`scenario_edit::fixture::fixture_with_textured_level`: the 4 by 4 floor
//! quad, a collision room over it and one portal across it at x = 2).
use super::*;
use crate::{GeometrySession, Patches, Target, TargetKind};
use collision_build::rebuild::on_triangle;
use collision_build::{query, verify};
use map::writer::MapEditor;
use map::{RawPointer, ResourceFile};
use mesh_edit::{Brush, MeshOp, Stroke};
use scenario_edit::fixture;
use std::io::Cursor;
use tags::geometry::RawSource;

const HEIGHT: f32 = fixture::TEX_LEVEL_HEIGHT;
const PORTAL_X: f32 = fixture::TEX_PORTAL_X;

struct Local(Vec<u8>);
impl RawSource for Local {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        if p.file != ResourceFile::Local {
            return Err(tags::Error::MissingResource(p.file));
        }
        let at = p.offset as usize;
        self.0.get(at..at + size as usize).map(<[u8]>::to_vec).ok_or(tags::Error::Invalid("raw read past the file"))
    }
}

fn structure() -> TagId {
    fixture::id(4)
}

fn cluster() -> Target {
    Target::new(TargetKind::Cluster, structure(), 0)
}

fn portal_points() -> Vec<[f32; 3]> {
    fixture::textured_level_portal()
}

fn map_bytes() -> Vec<u8> {
    fixture::fixture_with_textured_level()
}

fn open(bytes: &[u8]) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes.to_vec())).unwrap()
}

fn session(bytes: &[u8]) -> GeometrySession {
    let mut s = GeometrySession::new();
    s.open(&mut open(bytes), &mut Local(bytes.to_vec()), cluster()).unwrap();
    s
}

fn bump() -> MeshOp {
    MeshOp::Sculpt(Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Raise, [2.0, 2.0, 0.0], 1.5, 0.5) })
}

/// The map with `p` written as `scenario_edit::save` writes it: patches,
/// raw patches, appended raw blocks, then grown metadata blocks.
fn saved(source: &[u8], p: &Patches) -> Vec<u8> {
    fn patch<R: std::io::Read + std::io::Seek>(
        edit: &mut MapEditor<'_, R>,
        space: Option<map::AddressSpace>,
        address: u32,
        bytes: &[u8],
    ) {
        match space {
            None => edit.patch_metadata(address, bytes).unwrap(),
            Some(space) => edit.patch_bsp(space, address, bytes).unwrap(),
        }
    }
    let mut map = open(source);
    let mut edit = map.edit();
    for w in &p.tags {
        patch(&mut edit, w.space, w.address, &w.bytes);
    }
    for w in &p.raw {
        edit.patch_raw(w.block, w.block_size, w.offset, &w.bytes).unwrap();
    }
    for a in &p.appended {
        let word = edit.append_raw(&a.bytes, a.align).unwrap().encode();
        patch(&mut edit, a.pointer_space, a.pointer_address, &word.to_le_bytes());
        for w in &a.writes {
            patch(&mut edit, w.space, w.address, &w.bytes);
        }
    }
    for g in &p.grown {
        let at = edit.append_metadata(&g.bytes, g.align).unwrap();
        let mut pair = g.count.to_le_bytes().to_vec();
        pair.extend_from_slice(&at.to_le_bytes());
        patch(&mut edit, g.pair_space, g.pair, &pair);
    }
    let mut out = Cursor::new(Vec::new());
    edit.write_to(&mut out).unwrap();
    out.into_inner()
}

/// The session's cluster as triangles.
fn triangles(s: &GeometrySession) -> Vec<Tri> {
    let e = s.mesh(cluster()).unwrap().export();
    e.triangles
        .iter()
        .zip(&e.materials)
        .map(|(t, &material)| Tri { points: t.map(|v| e.positions[v as usize]), material })
        .collect()
}

#[test]
fn the_fixture_reads_as_a_room_with_a_portal() {
    let src = map_bytes();
    let level = read(&mut open(&src), structure()).unwrap();
    assert_eq!(level.portals.len(), 1);
    assert_eq!(level.portals[0].points, portal_points());
    assert!(level.pathfinding.is_none());
    assert!(verify::check(&level.level.level.bsp, 1).is_empty());
}

#[test]
fn a_stroke_on_the_cluster_rebuilds_collision_and_portals_that_still_close() {
    let src = map_bytes();
    let mut s = session(&src);
    s.apply(cluster(), &[bump()], None).unwrap();
    let change = s.level_change(structure()).expect("the stroke rebuilt the level").clone();
    assert!(change.rebuilt.removed >= 1 && change.rebuilt.added >= 1, "the floor was replaced");
    let after = triangles(&s);
    let out = saved(&src, &s.changes().unwrap());

    // The collision read back: valid, the bump solid, the room above open.
    let back = read(&mut open(&out), structure()).unwrap();
    let bsp = &back.level.level.bsp;
    assert!(verify::check(bsp, 1).is_empty(), "{:#?}", verify::check(bsp, 1));
    assert_eq!(back.level.level.leaves.len(), bsp.leaves.len() * collision_build::level::LEAF_SIZE);
    assert_eq!(query::locate(bsp, [2.0, 2.0, 0.3]), None, "under the bump's top");
    assert!(query::locate(bsp, [2.0, 2.0, 0.8]).is_some(), "over it");
    assert!(query::locate(bsp, [3.8, 3.8, 0.1]).is_some(), "the flat floor's room");
    // Every open leaf is in cluster 0, and keeps the file's other bytes.
    for e in back.level.level.leaves.chunks_exact(collision_build::level::LEAF_SIZE) {
        assert_eq!(&e[..6], &[0, 0, 0xa0, 0xa1, 0xa2, 0xa3]);
    }
    // Every new floor surface lies on the sculpted render surface.
    let floor = |p: [f32; 3]| after.iter().any(|t| on_triangle(p, t.points));
    for s in 0..bsp.surfaces.len() {
        let ring: Vec<[f32; 3]> = bsp.surface_ring(s).iter().map(|&v| bsp.vertices[v].point).collect();
        if ring.iter().all(|p| p[2] < 0.75 && p[0] > 0.01 && p[0] < 3.99 && p[1] > 0.01 && p[1] < 3.99) {
            assert!(ring.iter().all(|&p| floor(p)), "surface {s} off the floor: {ring:?}");
        }
    }

    // The portal read back: on its plane in the rebuilt BSP, and its
    // bottom edge on the new floor all along, so it still closes.
    let portal = &back.portals[0];
    assert_eq!(bsp.planes[portal.plane as usize].0, [1.0, 0.0, 0.0, PORTAL_X]);
    assert!(portal.points.len() > 4, "the bottom edge follows the bump: {:?}", portal.points);
    assert!(portal.points.iter().all(|p| (p[0] - PORTAL_X).abs() < 1e-4));
    let top = portal.points.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    assert_eq!(top, HEIGHT);
    let ground: Vec<[f32; 3]> = portal.points.iter().copied().filter(|p| p[2] < HEIGHT - 0.1).collect();
    let peak = ground.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    assert!(peak > 0.3 && peak <= 0.5 + 1e-4, "it rises over the bump: {peak}");
    for pair in ground.windows(2) {
        for k in 0..=4 {
            let p = mix(pair[0], pair[1], k as f32 / 4.0);
            assert!(floor(p), "a gap under the portal at {p:?}");
        }
    }
    let n = portal.points.len() as f32;
    let centre = portal.points.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    let stored = [8, 12, 16].map(|o| f32::from_bits(u32_at(&portal.bytes, o)));
    for k in 0..3 {
        assert!((stored[k] - centre[k]).abs() < 1e-4, "the centre follows: {stored:?} {centre:?}");
    }
}

#[test]
fn undoing_the_stroke_drops_the_rebuilt_level() {
    let src = map_bytes();
    let mut s = session(&src);
    s.apply(cluster(), &[bump()], None).unwrap();
    assert!(s.level_change(structure()).is_some());
    assert!(s.undo());
    assert!(s.level_change(structure()).is_none());
    assert_eq!(s.changes().unwrap(), Patches::default());
    assert!(s.redo());
    assert!(s.level_change(structure()).is_some());
}

#[test]
fn a_cut_under_the_portal_is_refused_and_changes_nothing() {
    let src = map_bytes();
    let mut s = session(&src);
    let cut = MeshOp::Sculpt(Stroke { detail: Some(0.5), ..Stroke::dab(Brush::Cut, [PORTAL_X, 2.0, 0.0], 0.6, 1.0) });
    let e = s.apply(cluster(), &[cut], None).unwrap_err();
    let Error::Refused { reason, .. } = &e else { panic!("{e}") };
    assert!(reason.starts_with("portal 0: the stroke leaves a gap under the portal edge"), "{reason}");
    assert!(s.ops(cluster()).is_empty());
    assert_eq!(s.mesh(cluster()).unwrap().vert_count(), 4);
    assert!(s.level_change(structure()).is_none());
}

#[test]
fn a_later_move_keeps_the_sculpted_level_in_step() {
    let src = map_bytes();
    let mut s = session(&src);
    s.apply(cluster(), &[bump()], None).unwrap();
    let first = s.level_change(structure()).unwrap().clone();
    let mesh = s.mesh(cluster()).unwrap();
    let corner = mesh.vert_ids().find(|&v| mesh.position(v) == [4.0, 4.0, 0.0]).unwrap();
    let lift = [
        MeshOp::Select { ids: vec![corner.0], how: mesh_edit::SelectHow::Set },
        MeshOp::Transform {
            matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.25]],
            pivot: None,
        },
    ];
    s.apply(cluster(), &lift, None).unwrap();
    assert_ne!(s.level_change(structure()).unwrap(), &first, "the move rebuilt the level again");
}

#[test]
fn a_portal_edge_over_a_hole_is_refused_by_name() {
    let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| Tri { points: [a, b, c], material: 0 };
    let floor = vec![
        tri([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 4.0, 0.0]),
        tri([0.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 4.0, 0.0]),
    ];
    // The floor with a strip `1 < y < 3` taken out across the portal.
    let strip = |y0: f32, y1: f32| {
        vec![
            tri([0.0, y0, 0.0], [4.0, y0, 0.0], [4.0, y1, 0.0]),
            tri([0.0, y0, 0.0], [4.0, y1, 0.0], [0.0, y1, 0.0]),
        ]
    };
    let holed: Vec<Tri> = strip(0.0, 1.0).into_iter().chain(strip(3.0, 4.0)).collect();
    let pts = portal_points();
    let e = follow(&pts, [1.0, 0.0, 0.0], PORTAL_X, &floor, (&floor, &holed), &holed).unwrap_err();
    assert!(e.starts_with("the stroke leaves a gap under the portal edge"), "{e}");
    // The whole floor raised by a quarter: the edge rides up with it.
    let raised: Vec<Tri> = floor.iter().map(|t| Tri { points: t.points.map(|p| [p[0], p[1], 0.25]), ..*t }).collect();
    let moved = follow(&pts, [1.0, 0.0, 0.0], PORTAL_X, &floor, (&floor, &raised), &raised).unwrap().unwrap();
    assert_eq!(moved.first(), Some(&[PORTAL_X, 0.0, 0.25]));
    assert!(moved.contains(&[PORTAL_X, 4.0, 0.25]));
    assert!(moved.iter().filter(|p| p[2] == HEIGHT).count() == 2);
    // An edge off the changed surface is left alone.
    let elsewhere = strip(5.0, 6.0);
    assert_eq!(follow(&pts, [1.0, 0.0, 0.0], PORTAL_X, &floor, (&elsewhere, &elsewhere), &floor), Ok(None));
}
