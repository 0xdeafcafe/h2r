//! Link only triangles sharing an indexed chart edge. Packed neighbours
//! in the page may be on entirely different walls.
use super::{Triangle, Vertex, VertexRun};
use anyhow::{Context, Result, ensure};
use map::{Map, TagId};
use scenario_edit::geometry::{add, cross, dot, sub};
use std::{
    collections::{BTreeSet, HashMap},
    io::{Read, Seek},
};
use tags::{
    collision::{Bsp, Child},
    geometry::TagView,
};

pub(super) struct ClusterLookup {
    tree: Bsp,
    clusters: Vec<i16>,
}

impl ClusterLookup {
    pub(super) fn read<R: Read + Seek>(map: &mut Map<R>, bsp: TagId) -> Result<Self> {
        let address = map
            .tag(bsp)?
            .location
            .context("BSP has no payload")?
            .address;
        let bytes = map.read_tag_bytes(bsp)?;
        let view = TagView::new(&bytes, address);
        let root = view.at(0, 0x34)?;
        let collisions = view.block(root, 0x14, Bsp::STRUCTURE_SIZE)?;
        ensure!(
            collisions.len() == 1,
            "expected one structure collision BSP for the sky lookup"
        );
        let base = u32::from_le_bytes(root[0x18..0x1c].try_into()?);
        let tree = Bsp::decode_structure(
            &bytes,
            address,
            base.checked_sub(address)
                .context("invalid collision BSP pointer")? as usize,
        )?;
        // Structure leaves parallel the collision leaves. The engine's
        // location query indexes this 8-byte table with its leaf number.
        let clusters: Vec<_> = view
            .block(root, 0x2c, 8)?
            .into_iter()
            .map(|r| i16::from_le_bytes([r[0], r[1]]))
            .collect();
        ensure!(
            clusters.len() == tree.leaves.len(),
            "collision/structure leaf count mismatch"
        );
        Ok(Self { tree, clusters })
    }

    pub(super) fn cluster(&self, p: [f32; 3]) -> Option<usize> {
        let mut next = Child::Node(0);
        for _ in 0..=self.tree.nodes.len() {
            match next {
                Child::None => return None,
                Child::Leaf(i) => return usize::try_from(*self.clusters.get(i as usize)?).ok(),
                Child::Node(i) => {
                    let n = self.tree.nodes.get(i as usize)?;
                    let [x, y, z, d] = self.tree.planes.get(usize::from(n.plane))?.0;
                    next = if (z * p[2] + y * p[1]) + x * p[0] >= d {
                        n.front
                    } else {
                        n.back
                    };
                }
            }
        }
        None
    }
}

fn root(parents: &mut [usize], mut i: usize) -> usize {
    while parents[i] != i {
        parents[i] = parents[parents[i]];
        i = parents[i];
    }
    i
}

fn edges([a, b, c]: [u16; 3]) -> [[usize; 2]; 3] {
    [[a, b], [b, c], [c, a]].map(|[i, j]| [usize::from(i.min(j)), usize::from(i.max(j))])
}

pub(super) fn append_triangles(
    out: &mut Vec<Triangle>,
    positions: &[[f32; 3]],
    uv: &[[f32; 2]],
    faces: &[[u16; 3]],
    next_chart: &mut u32,
) -> Result<()> {
    let mut parents: Vec<_> = (0..faces.len()).collect();
    let mut first = HashMap::new();
    for (i, &face) in faces.iter().enumerate() {
        ensure!(
            face.iter()
                .all(|&j| usize::from(j) < uv.len() && usize::from(j) < positions.len()),
            "lit triangle indexes beyond its UV or position stream"
        );
        for edge in edges(face) {
            if let Some(&other) = first.get(&edge) {
                let left = root(&mut parents, i);
                let right = root(&mut parents, other);
                parents[left] = right;
            } else {
                first.insert(edge, i);
            }
        }
    }
    let mut charts = HashMap::new();
    for (i, &face) in faces.iter().enumerate() {
        let component = root(&mut parents, i);
        let chart = *charts.entry(component).or_insert_with(|| {
            let chart = *next_chart;
            *next_chart += 1;
            chart
        });
        out.push(Triangle {
            position: face.map(|j| positions[usize::from(j)]),
            uv: face.map(|j| uv[usize::from(j)]),
            chart,
        });
    }
    Ok(())
}

pub(super) fn vertex_run(
    label: String,
    positions: &[[f32; 3]],
    faces: &[[u16; 3]],
    retail: &[[u8; 3]],
    selected: &dyn Fn([f32; 3]) -> bool,
) -> Result<VertexRun> {
    ensure!(
        positions.len() == retail.len(),
        "colour/position length mismatch for {label}"
    );
    let mut normals = vec![[0.; 3]; positions.len()];
    let mut adjacent = BTreeSet::new();
    for &face in faces {
        let [a, b, c] = face.map(usize::from);
        ensure!(
            a < positions.len() && b < positions.len() && c < positions.len(),
            "lit triangle outside colour run for {label}"
        );
        let face_normal = cross(
            sub(positions[b], positions[a]),
            sub(positions[c], positions[a]),
        );
        for i in [a, b, c] {
            normals[i] = add(normals[i], face_normal);
        }
        adjacent.extend(edges(face));
    }
    let vertices: Vec<_> = positions
        .iter()
        .zip(normals)
        .zip(retail)
        .map(|((&position, n), &retail)| {
            let len = dot(n, n).sqrt();
            let normal = if len.is_finite() && len > 0.0 && selected(position) {
                n.map(|v| v / len)
            } else {
                [0.; 3]
            };
            Vertex {
                position,
                normal,
                retail,
            }
        })
        .collect();
    adjacent.retain(|&[i, j]| vertices[i].normal != [0.; 3] && vertices[j].normal != [0.; 3]);
    Ok(VertexRun {
        label,
        vertices,
        edges: adjacent.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_samples_use_the_leaf_cluster_in_mixed_sky_bsps() {
        use tags::collision::{Node3d, Plane};
        let lookup = ClusterLookup {
            tree: Bsp {
                nodes: vec![Node3d {
                    plane: 0,
                    back: Child::Leaf(0),
                    front: Child::Leaf(1),
                }],
                planes: vec![Plane([1., 0., 0., 0.])],
                ..Default::default()
            },
            clusters: vec![-1, 2],
        };
        assert_eq!(lookup.cluster([0., 1., 0.]), Some(2));
        assert_eq!(lookup.cluster([-0.1, 1., 0.]), None);
    }

    #[test]
    fn coincident_chart_borders_do_not_join_separate_indexed_surfaces() {
        let uv = [
            [0., 0.],
            [1., 0.],
            [0., 1.],
            [1., 1.],
            [0., 0.],
            [1., 0.],
            [0., 1.],
        ];
        let positions = uv.map(|[x, y]| [x, y, 0.]);
        let mut out = Vec::new();
        append_triangles(
            &mut out,
            &positions,
            &uv,
            &[[0, 1, 2], [1, 3, 2], [4, 5, 6]],
            &mut 7,
        )
        .unwrap();
        assert_eq!(out[0].chart, 7);
        assert_eq!(out[0].chart, out[1].chart);
        assert_ne!(out[0].chart, out[2].chart);
    }

    #[test]
    fn vertex_run_preserves_indices_and_geometry_for_the_selected_sky() {
        let p = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
        let rgb = [[10, 11, 12], [20, 21, 22], [30, 31, 32]];
        let run = vertex_run("fixture".into(), &p, &[[0, 1, 2]], &rgb, &|p| p[0] == 0.).unwrap();
        assert_eq!(run.vertices[1].retail, rgb[1]);
        assert_eq!(run.vertices[1].normal, [0.; 3]);
        assert_eq!(run.vertices[2].normal, [0., 0., 1.]);
        assert_eq!(run.edges, [[0, 2]]);
    }
}
