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

#[test]
fn a_count_change_is_refused_and_named() {
    let (_, mut s) = opened();
    s.apply(
        cluster(),
        &[
            MeshOp::Mode {
                mode: SelectMode::Face,
            },
            MeshOp::SelectAll,
            MeshOp::ExtrudeFaces {
                offset: [0.0, 0.0, 1.0],
            },
        ],
        None,
    )
    .unwrap();
    assert_eq!(s.class(cluster()), Some(WriteClass::Grown));
    let e = s.changes().unwrap_err().to_string();
    assert!(e.contains(&cluster().id()) && e.contains("grown"), "{e}");
    assert_eq!(s.preview_positions(cluster()), None);
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
