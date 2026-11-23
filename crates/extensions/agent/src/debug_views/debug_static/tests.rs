//! Synthetic scenario and BSP records. All coordinates are invented.
use super::*;
use tags::collision::{Edge as BspEdge, Surface, Vertex};

const BASE: u32 = 0x1000;

fn pair(bytes: &mut [u8], field: usize, count: u32, offset: usize) {
    bytes[field..field + 4].copy_from_slice(&count.to_le_bytes());
    bytes[field + 4..field + 8].copy_from_slice(&(BASE + offset as u32).to_le_bytes());
}

#[test]
fn trigger_and_kill_blocks_select_their_oriented_boxes() {
    let mut bytes = vec![0; 0x390];
    pair(&mut bytes, 0x108, 3, 0x280);
    pair(&mut bytes, 0x230, 3, 0x380);
    for (i, origin) in [[2.0, 3.0, 0.0], [8.0, 2.0, 5.0], [9.0, 9.0, 9.0]]
        .iter()
        .enumerate()
    {
        let at = 0x280 + i * 0x44;
        // A third volume follows an object. Its local frame cannot be
        // projected into the static overlay.
        if i != 2 {
            bytes[at + 4..at + 6].copy_from_slice(&(-1i16).to_le_bytes());
        }
        for (field, point) in [
            (0xc, [0.0f32, 1.0, 0.0]),
            (0x18, [0.0, 0.0, 1.0]),
            (0x24, *origin),
            (0x30, [2.0, 3.0, 4.0]),
        ] {
            for (n, value) in point.into_iter().enumerate() {
                let k = at + field + 4 * n;
                bytes[k..k + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    // Repeated kill reference, and a missing reference.
    for (dst, value) in bytes[0x380..0x386].chunks_exact_mut(2).zip([1u16, 1, 9]) {
        dst.copy_from_slice(&value.to_le_bytes());
    }
    let mut out = StaticViews::default();
    out.volumes(&TagView::new(&bytes, BASE));
    assert_eq!(out.triggers.len(), 24);
    assert_eq!(out.kill_volumes.len(), 12);
    assert_eq!(out.kill_volumes, out.triggers[12..24]);
    assert!(
        out.kill_volumes
            .contains(&[[8.0, 2.0, 5.0], [8.0, 4.0, 5.0]])
    );
    assert!(
        out.kill_volumes
            .contains(&[[8.0, 2.0, 5.0], [5.0, 2.0, 5.0]])
    );
}

#[test]
fn damaged_kill_reference_block_leaves_triggers_available() {
    let mut bytes = vec![0; 0x238];
    pair(&mut bytes, 0x230, 1, 0x300);
    let mut out = StaticViews::default();
    out.volumes(&TagView::new(&bytes, BASE));
    assert!(out.refusal(View::KillVolumes).is_some());
    assert!(out.refusal(View::TriggerVolumes).is_none());
}

#[test]
fn collision_edges_with_either_invisible_neighbor_are_barriers() {
    let points = [[1.0, 2.0, 3.0], [4.0, 2.0, 3.0], [1.0, 5.0, 3.0]];
    let bsp = Bsp {
        surfaces: [0, INVISIBLE]
            .map(|flags| Surface {
                flags,
                first_edge: 0,
                material: 0,
                plane: 0,
                breakable_surface: 255,
            })
            .to_vec(),
        vertices: points.map(|point| Vertex { point, edge: 0 }).to_vec(),
        edges: [(0, 1, 0, u16::MAX), (1, 2, 0, 1), (2, 0, 1, u16::MAX)]
            .map(
                |(start_vertex, end_vertex, left_surface, right_surface)| BspEdge {
                    start_vertex,
                    end_vertex,
                    left_surface,
                    right_surface,
                    forward_edge: 0,
                    reverse_edge: 0,
                },
            )
            .to_vec(),
        ..Default::default()
    };
    let mut bytes = vec![0; 0x40];
    pair(&mut bytes, 0x14, 1, 0x40);
    bytes.extend(bsp.encode_blocks(BASE + 0x40));
    let mut out = StaticViews::default();
    out.structure_collision(&TagView::new(&bytes, BASE))
        .unwrap();
    assert_eq!(
        out.collision,
        [
            [points[0], points[1]],
            [points[1], points[2]],
            [points[2], points[0]]
        ]
    );
    assert_eq!(
        out.barriers,
        [[points[1], points[2]], [points[2], points[0]]]
    );
}

#[test]
fn instance_collision_rotates_scales_and_translates_a_local_edge() {
    let bsp = Bsp {
        surfaces: vec![Surface {
            flags: INVISIBLE,
            first_edge: 0,
            material: 0,
            plane: 0,
            breakable_surface: 255,
        }],
        vertices: [[0.0, 1.0, 2.0], [3.0, 1.0, 2.0]]
            .map(|point| Vertex { point, edge: 0 })
            .to_vec(),
        edges: vec![BspEdge {
            start_vertex: 0,
            end_vertex: 1,
            left_surface: 0,
            right_surface: u16::MAX,
            forward_edge: 0,
            reverse_edge: 0,
        }],
        ..Default::default()
    };
    let mut bytes = vec![0; 0x400];
    pair(&mut bytes, 0x138, 1, 0x200);
    pair(&mut bytes, 0x140, 1, 0x300);
    let matrix = [
        2.0f32, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
    ];
    for (i, v) in matrix.into_iter().enumerate() {
        bytes[0x300 + 4 * i..0x304 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    // Keep the inline descriptors in the definition. Put their children
    // after the placements and relocate their addresses accordingly.
    let mut blocks = bsp.encode_blocks(BASE + 0x400 - Bsp::STRUCTURE_SIZE as u32);
    let children = blocks.split_off(Bsp::STRUCTURE_SIZE);
    bytes[0x270..0x270 + Bsp::STRUCTURE_SIZE].copy_from_slice(&blocks);
    bytes.extend(children);
    let mut views = StaticViews::default();
    views
        .instance_collision(&TagView::new(&bytes, BASE))
        .unwrap();
    assert_eq!(views.barriers, [[[8.0, 20.0, 34.0], [8.0, 26.0, 34.0]]]);
    assert_eq!(views.collision, views.barriers);
}

#[test]
fn placed_mesh_keeps_shared_edges_once() {
    let mesh = mesh_edges(
        &[
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 1.0],
            [2.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        [[0, 1, 2], [0, 2, 3]],
    );
    let mut edges = Vec::new();
    let mut work = MAX_PLACED_LINES;
    place(&mut edges, &mesh, MAX_LINES, &mut work, &|[x, y, z]| {
        [10.0 - y, 20.0 + x, z]
    })
    .unwrap();
    assert_eq!(edges.len(), 5);
    assert!(edges.contains(&[[10.0, 20.0, 1.0], [10.0, 22.0, 1.0]]));
    assert!(edges.contains(&[[9.0, 22.0, 1.0], [9.0, 20.0, 1.0]]));
}

#[test]
fn full_wireframe_cache_skips_remaining_transforms() {
    let local = [[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]; 2];
    let mut edges = vec![[[0.0; 3]; 2]; MAX_LINES - 1];
    let mut work = 1;
    place(&mut edges, &local, MAX_LINES, &mut work, &|p| p).unwrap();
    assert_eq!(edges.len(), MAX_LINES);
    assert_eq!(work, 0);
    place(&mut edges, &local, MAX_LINES, &mut work, &|_| {
        panic!("cache is full")
    })
    .unwrap();
}

#[test]
fn volume_axes_are_orthonormalized_and_parallel_axes_are_rejected() {
    let edges = box_edges([1.0; 3], [0.0, 2.0, 0.0], [0.0, 2.0, 3.0], [2.0, 3.0, 4.0]).unwrap();
    assert!(edges.contains(&[[1.0; 3], [1.0, 3.0, 1.0]]));
    assert!(edges.contains(&[[1.0; 3], [-2.0, 1.0, 1.0]]));
    assert!(edges.contains(&[[1.0; 3], [1.0, 1.0, 5.0]]));
    assert!(box_edges([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0; 3]).is_none());
}

#[test]
fn truncated_and_oversized_descriptors_refuse_before_allocating() {
    let empty = TagView::new(&[], BASE);
    assert!(rows(&empty, 0x108, 0x44).is_err());
    let mut bytes = vec![0; 0x18];
    pair(&mut bytes, 0x10, u32::MAX, 0);
    assert!(rows(&TagView::new(&bytes, BASE), 0x10, 0x44).is_err());
    let mut bytes = vec![0; 0x1c];
    pair(&mut bytes, 0x14, 1, 0);
    bytes[0x18..0x1c].copy_from_slice(&(BASE - 1).to_le_bytes());
    assert!(
        StaticViews::default()
            .structure_collision(&TagView::new(&bytes, BASE))
            .is_err()
    );
}

#[test]
fn rejected_transforms_still_consume_placement_budget() {
    let points = [[[1.0; 3]; 2]; 2];
    let mut out = Vec::new();
    let mut work = 2;
    place(&mut out, &points, 100, &mut work, &|_| [f32::INFINITY; 3]).unwrap();
    assert!(out.is_empty());
    assert_eq!(work, 0);
    assert!(place(&mut out, &points, 100, &mut work, &|p| p).is_err());
}

#[test]
fn aliased_definitions_share_one_decode_budget() {
    let mut bytes = vec![0; 0x100];
    // A single vertex in the embedded BSP.
    pair(&mut bytes, 0x38, 1, 0x80);
    let view = TagView::new(&bytes, BASE);
    let mut bytes_left = 16;
    CollisionMesh::read(&view, 0, &mut bytes_left).unwrap();
    assert!(CollisionMesh::read(&view, 0, &mut bytes_left).is_err());
}

/// A live collision commit (`edit.collision.commit`) replaces the
/// structure's level collision lines and keeps its instances' lines.
#[test]
fn a_committed_level_replaces_only_the_structure_lines() {
    let level = |x: f32| Bsp {
        surfaces: vec![Surface { plane: 0, first_edge: 0, flags: 0, breakable_surface: 0xff, material: 0 }],
        edges: vec![BspEdge {
            start_vertex: 0,
            end_vertex: 1,
            forward_edge: 0,
            reverse_edge: 0,
            left_surface: 0,
            right_surface: u16::MAX,
        }],
        vertices: vec![
            Vertex { point: [x, 0.0, 0.0], edge: 0 },
            Vertex { point: [x, 1.0, 0.0], edge: 0 },
        ],
        ..Bsp::default()
    };
    let instance: Edge = [[9.0, 9.0, 9.0], [9.0, 9.0, 8.0]];
    let mut views = StaticViews::default();
    views.replace_structure(&level(1.0));
    views.collision.push(instance);
    assert_eq!(views.collision, vec![[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], instance]);
    let mut grown = level(2.0);
    grown.vertices.push(Vertex { point: [2.0, 0.0, 1.0], edge: 1 });
    grown.edges.push(BspEdge { start_vertex: 1, end_vertex: 2, ..grown.edges[0] });
    views.replace_structure(&grown);
    assert_eq!(
        views.collision,
        vec![[[2.0, 0.0, 0.0], [2.0, 1.0, 0.0]], [[2.0, 1.0, 0.0], [2.0, 0.0, 1.0]], instance]
    );
    assert!(views.barriers.is_empty());
}
