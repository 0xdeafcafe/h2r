//! The session on the invented textured map of `scenario_edit::fixture`:
//! one BSP (`id(4)`) with one cluster, the 4 by 4 floor quad, whose raw
//! block is local at `TEX_BLOCK_FILE`.
use super::*;
use map::{Map, RawPointer};
use mesh_edit::{MeshOp, SelectHow, SelectMode};
use scenario_edit::fixture::{self, META_FILE, TEX_BLOCK_FILE, TEX_SBSP};
use std::io::Cursor;

/// Raw data of the map itself.
struct Local(Vec<u8>);
impl RawSource for Local {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        if p.file != ResourceFile::Local {
            return Err(tags::Error::MissingResource(p.file));
        }
        let at = p.offset as usize;
        self.0
            .get(at..at + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::Invalid("raw read past the file"))
    }
}

fn cluster() -> Target {
    Target::new(TargetKind::Cluster, fixture::id(4), 0)
}

fn opened() -> (Vec<u8>, GeometrySession) {
    let bytes = fixture::fixture_with_textured_bsp();
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let mut s = GeometrySession::new();
    let info = s
        .open(&mut map, &mut Local(bytes.clone()), cluster())
        .unwrap();
    assert_eq!((info.verts, info.faces), (4, 2));
    assert!(info.local);
    (bytes, s)
}

fn lift(s: &mut GeometrySession, at: [f32; 3], dz: f32) -> Vec<MeshOp> {
    let mesh = s.mesh(cluster()).unwrap();
    let v = mesh.vert_ids().find(|&v| mesh.position(v) == at).unwrap();
    let ops = vec![
        MeshOp::Select {
            ids: vec![v.0],
            how: SelectHow::Set,
        },
        MeshOp::Transform {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, dz],
            ],
            pivot: None,
        },
    ];
    s.apply(cluster(), &ops, None).unwrap();
    ops
}

/// The map with `p` written, as `scenario_edit::save` writes it.
fn saved(source: &[u8], p: &Patches) -> Vec<u8> {
    let mut map = Map::from_reader(Cursor::new(source.to_vec())).unwrap();
    let mut edit = map.edit();
    for w in &p.tags {
        match w.space {
            None => edit.patch_metadata(w.address, &w.bytes).unwrap(),
            Some(space) => edit.patch_bsp(space, w.address, &w.bytes).unwrap(),
        }
    }
    for w in &p.raw {
        edit.patch_raw(w.block, w.block_size, w.offset, &w.bytes)
            .unwrap();
    }
    for a in &p.appended {
        let word = edit.append_raw(&a.bytes, a.align).unwrap().encode();
        let writes = std::iter::once(RegionWrite {
            space: a.pointer_space,
            address: a.pointer_address,
            bytes: word.to_le_bytes().to_vec(),
            was: a.old.encode().to_le_bytes().to_vec(),
        })
        .chain(a.writes.iter().cloned());
        for w in writes {
            match w.space {
                None => edit.patch_metadata(w.address, &w.bytes).unwrap(),
                Some(space) => edit.patch_bsp(space, w.address, &w.bytes).unwrap(),
            }
        }
    }
    let mut out = Cursor::new(Vec::new());
    edit.write_to(&mut out).unwrap();
    out.into_inner()
}

#[test]
fn listing_names_the_cluster_with_its_counts() {
    let bytes = fixture::fixture_with_textured_bsp();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let all = crate::list(&mut map);
    let c = all.iter().find(|t| t.target == cluster()).unwrap();
    assert_eq!((c.vertices, c.triangles, c.parts, c.local), (4, 2, 1, true));
}

#[test]
fn an_unedited_target_writes_nothing() {
    let (_, mut s) = opened();
    s.apply(cluster(), &[MeshOp::SelectAll], None).unwrap();
    assert_eq!(s.changes().unwrap(), Patches::default());
    assert_eq!(s.check(None)[0].class, WriteClass::Unchanged);
    assert!(!s.has_changes());
}

#[test]
fn a_moved_vertex_saves_only_its_bytes_the_bounds_and_the_checksum() {
    let (source, mut s) = opened();
    lift(&mut s, [4.0, 4.0, 0.0], 1.0);
    let check = &s.check(Some(cluster()))[0];
    assert_eq!(
        (check.class, check.moved, check.refused.clone()),
        (WriteClass::InPlace, 1, None)
    );
    assert!(check.lightmap_stale > 0);
    let p = s.changes().unwrap();
    let out = saved(&source, &p);
    assert_eq!(out.len(), source.len());
    let block_len = p.raw[0].block_size as usize;
    let block = TEX_BLOCK_FILE..TEX_BLOCK_FILE + block_len;
    let cluster_bounds = META_FILE + TEX_SBSP + 0x180 + 0x54;
    let bounds = cluster_bounds..cluster_bounds + 24;
    for i in (0..out.len()).filter(|&i| out[i] != source[i]) {
        assert!(
            block.contains(&i) || bounds.contains(&i) || (0x2d0..0x2d4).contains(&i),
            "byte {i:#x} changed"
        );
    }
    // It reads back to the edited position, and the cluster's bounds hold it.
    let mut map = Map::from_reader(Cursor::new(out.clone())).unwrap();
    let render = StructureRender::read(&mut map, fixture::id(4)).unwrap();
    assert_eq!(render.clusters[0].bounds, [0.0, 4.0, 0.0, 4.0, 0.0, 1.0]);
    let section = render.clusters[0]
        .geometry
        .load(&mut Local(out), None)
        .unwrap()
        .unwrap();
    let v = section.vertices(None).unwrap();
    assert_eq!(v.positions[3], [4.0, 4.0, 1.0]);
    assert_eq!(
        v.positions[..3],
        [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]]
    );
}

/// Each patch names its section and carries the map's bytes it replaces:
/// what a live write sends as its owner and `was`.
#[test]
fn each_patch_names_its_owner_and_the_bytes_it_replaces() {
    let (source, mut s) = opened();
    lift(&mut s, [4.0, 4.0, 0.0], 1.0);
    let p = s.changes().unwrap();
    assert!(!p.raw.is_empty() && !p.tags.is_empty());
    for w in &p.raw {
        assert_eq!(w.owner, cluster());
        assert_eq!(w.was.len(), w.bytes.len());
        let at = w.block.offset as usize + w.offset as usize;
        assert_eq!(w.was, source[at..at + w.was.len()]);
        assert_ne!(w.was, w.bytes);
    }
    let mut map = Map::from_reader(Cursor::new(source.clone())).unwrap();
    for w in &p.tags {
        assert_eq!(w.was.len(), w.bytes.len());
        let space = w.space.or(map.metadata_space()).unwrap();
        let at = space.translate(w.address, w.was.len() as u32).unwrap();
        let held = map.read_file_range(at, w.was.len() as u64).unwrap();
        assert_eq!(w.was, held);
    }
}

#[test]
fn undo_and_redo_restore_the_mesh_the_log_and_the_changes() {
    let (_, mut s) = opened();
    let ops = lift(&mut s, [4.0, 4.0, 0.0], 1.0);
    let edited = s.changes().unwrap();
    assert_eq!(s.ops(cluster()), ops.as_slice());
    assert!(s.undo());
    assert_eq!(s.changes().unwrap(), Patches::default());
    assert!(s.ops(cluster()).is_empty());
    assert!(s.redo());
    assert_eq!(s.changes().unwrap(), edited);
    // A drag's steps merge into one.
    let key = 7;
    for dz in [0.25, 0.25] {
        s.apply(
            cluster(),
            &[MeshOp::Transform {
                matrix: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, dz],
                ],
                pivot: None,
            }],
            Some(key),
        )
        .unwrap();
    }
    s.seal();
    assert!(s.undo());
    assert_eq!(s.changes().unwrap(), edited);
}

#[test]
fn replaying_the_op_log_gives_the_current_mesh() {
    let (_, mut s) = opened();
    lift(&mut s, [4.0, 0.0, 0.0], -0.5);
    lift(&mut s, [0.0, 4.0, 0.0], 2.0);
    let mut replay = Mesh::clone(&s.find(cluster()).unwrap().original);
    replay.apply_all(s.ops(cluster())).unwrap();
    assert_eq!(replay.export(), s.mesh(cluster()).unwrap().export());
}

fn extrude() -> Vec<MeshOp> {
    vec![
        MeshOp::Mode {
            mode: SelectMode::Face,
        },
        MeshOp::SelectAll,
        MeshOp::ExtrudeFaces {
            offset: [0.0, 0.0, 1.0],
        },
    ]
}

fn opened_on(bytes: &[u8]) -> GeometrySession {
    let mut map = Map::from_reader(Cursor::new(bytes.to_vec())).unwrap();
    let mut s = GeometrySession::new();
    s.open(&mut map, &mut Local(bytes.to_vec()), cluster())
        .unwrap();
    s
}

#[test]
fn a_count_change_on_a_map_that_cannot_take_raw_data_is_refused_and_named() {
    // The textured map's raw data lies past its metadata.
    let (_, mut s) = opened();
    s.apply(cluster(), &extrude(), None).unwrap();
    assert_eq!(s.class(cluster()), Some(WriteClass::Grown));
    let e = s.changes().unwrap_err().to_string();
    assert!(e.contains(&cluster().id()) && e.contains("grown"), "{e}");
    assert!(s.check(None)[0].refused.is_some());
    assert_eq!(s.preview_positions(cluster()), None);
}

#[test]
fn a_grown_cluster_saves_and_reads_back_to_the_exported_mesh() {
    let source = fixture::fixture_with_textured_bsp_last();
    let mut s = opened_on(&source);
    s.apply(cluster(), &extrude(), None).unwrap();
    let check = &s.check(Some(cluster()))[0];
    assert_eq!((check.class, check.refused.clone()), (WriteClass::Grown, None));
    let export = s.mesh(cluster()).unwrap().export();
    let p = s.changes().unwrap();
    assert_eq!(p.appended.len(), 1);
    let a = &p.appended[0];
    assert_eq!(a.target, cluster());
    assert_eq!(a.old.offset as usize, TEX_BLOCK_FILE);
    let out = saved(&source, &p);

    // The saved map validates, its metadata still ends the file, and a
    // scenario block can still grow.
    let mut map = Map::from_reader(Cursor::new(out.clone())).unwrap();
    map.validate_checksum().unwrap();
    assert_eq!(map.raw_append_refusal(), None);
    let mut edit = map.edit();
    edit.append_metadata(&[0; 0x54], 16).unwrap();
    edit.write_to(&mut Cursor::new(Vec::new())).unwrap();

    // It reads back to the export: counts, positions, triangles by part.
    let render = StructureRender::read(&mut map, fixture::id(4)).unwrap();
    let g = &render.clusters[0].geometry;
    let block = g.block.block.unwrap();
    assert_ne!(block.offset as usize, TEX_BLOCK_FILE, "the old block is unreferenced");
    assert_eq!(block.offset, source.len() as u32 - 0x2200);
    assert_eq!(usize::from(g.info.total_vertex_count), export.positions.len());
    assert_eq!(usize::from(g.info.total_triangle_count), export.triangles.len());
    let section = g.load(&mut Local(out.clone()), Some(1)).unwrap().unwrap();
    let v = section.vertices(None).unwrap();
    assert_eq!(v.positions, export.positions);
    let triangles: Vec<[u32; 3]> = section
        .parts
        .iter()
        .flat_map(|p| section.part_triangles(p))
        .map(|t| t.map(u32::from))
        .collect();
    assert_eq!(triangles, export.triangles);
    assert!(section.parts.iter().all(|p| p.is_triangle_list()));
    assert_eq!(section.subparts.len(), section.parts.len());
    // The cluster's bounds hold the extruded top.
    assert_eq!(render.clusters[0].bounds[5], 1.0);
    // Nothing else names the old block.
    for c in &render.clusters {
        assert_ne!(c.geometry.block.block.map(|b| b.offset), Some(TEX_BLOCK_FILE as u32));
    }
}

#[test]
fn an_unedited_grown_target_writes_nothing_and_undo_drops_the_append() {
    let source = fixture::fixture_with_textured_bsp_last();
    let mut s = opened_on(&source);
    s.apply(cluster(), &extrude(), None).unwrap();
    assert_eq!(s.changes().unwrap().appended.len(), 1);
    assert!(s.undo());
    assert_eq!(s.changes().unwrap(), Patches::default());
}

#[test]
fn a_bucket_lit_cluster_keeps_its_counts() {
    let mut source = fixture::fixture_with_textured_bsp_last();
    // A flagged bucket and one cluster reference with an offset, in the
    // lightmap's spare room (its palette's tail), in the moved metadata.
    let meta = source.len() - 0x2000;
    let file = |at: usize| meta + at;
    let addr = |at: usize| fixture::META + at as u32;
    let (bucket, reference, offsets) = (
        fixture::TEX_LTMP + 0x500,
        fixture::TEX_LTMP + 0x540,
        fixture::TEX_LTMP + 0x550,
    );
    let group = fixture::TEX_LTMP + 0x110;
    source[file(bucket)..file(bucket) + 0x38].fill(0);
    source[file(bucket)] = 1;
    // No raw block of its own.
    fixture::put(&mut source, file(bucket) + 0xc, u32::MAX);
    fixture::put(&mut source, file(group) + 0x40, 1);
    fixture::put(&mut source, file(group) + 0x44, addr(bucket));
    source[file(reference)..file(reference) + 0x10].fill(0);
    fixture::put(&mut source, file(reference) + 4, 1);
    fixture::put(&mut source, file(reference) + 8, addr(offsets));
    fixture::put(&mut source, file(group) + 0x60, 1);
    fixture::put(&mut source, file(group) + 0x64, addr(reference));
    let mut s = opened_on(&source);
    s.apply(cluster(), &extrude(), None).unwrap();
    let why = s.check(None)[0].refused.clone().unwrap();
    assert!(why.contains("lightmap bucket"), "{why}");
    let e = s.changes().unwrap_err().to_string();
    assert!(e.contains(&cluster().id()), "{e}");
    // A moved vertex is still in place.
    assert!(s.undo());
    lift(&mut s, [4.0, 4.0, 0.0], 1.0);
    assert!(s.changes().unwrap().appended.is_empty());
}

#[test]
fn a_failed_op_list_changes_nothing() {
    let (_, mut s) = opened();
    let ops = [
        MeshOp::SelectAll,
        MeshOp::SelectNone,
        MeshOp::Transform {
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 1.0],
            ],
            pivot: None,
        },
    ];
    let e = s.apply(cluster(), &ops, None).unwrap_err();
    assert!(matches!(e, Error::Op(2, _)), "{e}");
    assert!(!s.can_undo());
    assert!(s.ops(cluster()).is_empty());
    assert_eq!(s.mesh(cluster()).unwrap().selection().len(), 0);
}

#[test]
fn the_preview_redraws_the_cluster_with_the_moved_vertex() {
    let (_, mut s) = opened();
    let before = s.previews();
    assert_eq!(before.len(), 1);
    assert_eq!(
        (before[0].bsp, before[0].drawn.clone()),
        (fixture::id(4), vec![Drawn::Cluster(0)])
    );
    lift(&mut s, [4.0, 4.0, 0.0], 1.0);
    let after = s.previews();
    assert_eq!(after[0].positions.len(), 4);
    assert!(after[0].positions.contains(&[4.0, 4.0, 1.0]));
    assert!(!after[0].positions.contains(&[4.0, 4.0, 0.0]));
    assert!(s.undo());
    assert_eq!(s.previews(), before);
}

#[test]
fn a_sculpt_stroke_on_the_cluster_is_one_step_and_its_collision_compiles() {
    let (_, mut s) = opened();
    let stroke = MeshOp::Sculpt(mesh_edit::Stroke {
        detail: Some(0.5),
        ..mesh_edit::Stroke::dab(mesh_edit::Brush::Raise, [2.0, 2.0, 0.0], 1.5, 0.5)
    });
    s.apply(cluster(), std::slice::from_ref(&stroke), None).unwrap();
    let mesh = s.mesh(cluster()).unwrap();
    assert!(mesh.vert_count() > 4, "refined where the brush is");
    let top = mesh.vert_ids().map(|v| mesh.position(v)[2]).fold(f32::MIN, f32::max);
    assert!(top > 0.0 && top <= 0.5);
    assert_eq!(s.ops(cluster()), &[stroke.clone()]);
    // A count change: the cluster is grown, not patched in place.
    assert_eq!(s.check(Some(cluster()))[0].class, WriteClass::Grown);
    // A cut through the floor: the hole compiles too.
    let cut = MeshOp::Sculpt(mesh_edit::Stroke {
        detail: Some(0.5),
        ..mesh_edit::Stroke::dab(mesh_edit::Brush::Cut, [1.0, 1.0, 0.0], 0.6, 1.0)
    });
    s.apply(cluster(), std::slice::from_ref(&cut), None).unwrap();
    assert_eq!(s.ops(cluster()).len(), 2);
    // Each stroke is one undo step.
    assert!(s.undo());
    assert_eq!(s.ops(cluster()), &[stroke]);
    assert!(s.undo());
    assert_eq!(s.mesh(cluster()).unwrap().vert_count(), 4);
    assert_eq!(s.check(Some(cluster()))[0].class, WriteClass::Unchanged);
}
