//! Render models as GPU-ready arrays (stage 2,
//! docs/graphics/native-renderer.md, "Stage 2: objects and skinning"): one
//! vertex and index buffer per model, in the model's rest space, every
//! vertex carrying up to four model-node indices and weights; one draw per
//! opaque part; the nodes' inverse rest frames. Built once per model tag;
//! pure data (`crate::model_load` fills it from a map).

use bytemuck::{Pod, Zeroable};

use crate::world::Aabb;

/// The game's node matrix (`real_matrix4x3`, `0x34` bytes): `+0` scale,
/// `+4` forward, `+0x10` left, `+0x1c` up, `+0x28` position. A local point
/// `p` maps to `scale · (p.x·forward + p.y·left + p.z·up) + position`
/// (Code: the bounding-sphere update `0x000bdf90`). The same layout holds a
/// mode node's inverse rest frame at node `+0x28`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct NodeMatrix {
    pub scale: f32,
    pub forward: [f32; 3],
    pub left: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
}

/// Size of a node matrix in guest memory and in the GPU's node buffers.
pub const NODE_MATRIX_SIZE: usize = 0x34;
const _: () = assert!(std::mem::size_of::<NodeMatrix>() == NODE_MATRIX_SIZE);

impl NodeMatrix {
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        forward: [1.0, 0.0, 0.0],
        left: [0.0, 1.0, 0.0],
        up: [0.0, 0.0, 1.0],
        position: [0.0; 3],
    };

    /// From 13 words in the guest's order.
    pub fn from_words(w: &[f32; 13]) -> Self {
        Self {
            scale: w[0],
            forward: [w[1], w[2], w[3]],
            left: [w[4], w[5], w[6]],
            up: [w[7], w[8], w[9]],
            position: [w[10], w[11], w[12]],
        }
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.position;
        for k in 0..3 {
            out[k] += self.scale * (p[0] * self.forward[k] + p[1] * self.left[k] + p[2] * self.up[k]);
        }
        out
    }

    /// A direction (no scale, no translation).
    pub fn vector(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|k| v[0] * self.forward[k] + v[1] * self.left[k] + v[2] * self.up[k])
    }

    /// `self ∘ inner`: first `inner`, then `self`.
    pub fn compose(&self, inner: &Self) -> Self {
        Self {
            scale: self.scale * inner.scale,
            forward: self.vector(inner.forward),
            left: self.vector(inner.left),
            up: self.vector(inner.up),
            position: self.point(inner.position),
        }
    }

    pub fn is_finite(&self) -> bool {
        bytemuck::cast_ref::<Self, [f32; 13]>(self).iter().all(|f| f.is_finite())
    }
}

/// One model vertex (52 bytes), in the model's rest space.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
    /// Model node indices (after the section's node map).
    pub nodes: [u8; 4],
    pub weights: [f32; 4],
}

/// One opaque part of a section: its model-local material and triangles.
#[derive(Clone, Debug, Default)]
pub struct ModelPartInput {
    pub material: u16,
    pub transparent: bool,
    pub triangles: Vec<[u16; 3]>,
}

/// A section's drawable input.
#[derive(Clone, Debug, Default)]
pub struct ModelSectionInput {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    /// Section-local node indices per vertex; empty for a section without
    /// vertex nodes (bound whole to its first mapped node).
    pub node_indices: Vec<[u8; 4]>,
    pub node_weights: Vec<[f32; 4]>,
    /// Section-local node -> model node (empty: identity).
    pub node_map: Vec<u8>,
    pub parts: Vec<ModelPartInput>,
}

/// A render model's input: its nodes' inverse rest frames (model node
/// order, the object's node order) and the sections to draw (one
/// permutation per region, one LOD).
#[derive(Clone, Debug, Default)]
pub struct ModelInput {
    pub inverse_binds: Vec<NodeMatrix>,
    pub sections: Vec<ModelSectionInput>,
}

/// One draw: a part's index range and its model-local material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelDraw {
    pub material: u16,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelStats {
    pub sections: u32,
    pub transparent_parts: u32,
    /// Triangles with an index past the vertices or a repeated corner.
    pub dropped_triangles: u32,
    /// Vertex node indices past the node map or the model's nodes (bound
    /// to node 0 instead).
    pub bad_nodes: u32,
}

/// A built model.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    /// Sorted by material.
    pub draws: Vec<ModelDraw>,
    pub inverse_binds: Vec<NodeMatrix>,
    /// Rest-space bounds.
    pub bounds: Option<Aabb>,
    pub stats: ModelStats,
}

impl Model {
    pub fn build(input: &ModelInput) -> Self {
        let mut m = Model {
            inverse_binds: input.inverse_binds.clone(),
            ..Default::default()
        };
        let node_count = input.inverse_binds.len().max(1);
        let mut bounds = Aabb::EMPTY;
        for s in &input.sections {
            m.stats.sections += 1;
            let n = s.positions.len();
            let map = |local: u8, bad: &mut u32| -> u8 {
                let model = if s.node_map.is_empty() {
                    Some(usize::from(local))
                } else {
                    s.node_map.get(usize::from(local)).map(|&g| usize::from(g))
                };
                match model {
                    Some(g) if g < node_count => g as u8,
                    _ => {
                        *bad += 1;
                        0
                    }
                }
            };
            let skinned = s.node_indices.len() == n && s.node_weights.len() == n;
            let base = m.vertices.len() as u32;
            for i in 0..n {
                let (nodes, weights) = if skinned {
                    let w = s.node_weights[i];
                    let mut nodes = [0u8; 4];
                    for k in 0..4 {
                        // A zero weight's index is padding: do not count it.
                        nodes[k] = if w[k] > 0.0 { map(s.node_indices[i][k], &mut m.stats.bad_nodes) } else { 0 };
                    }
                    (nodes, w)
                } else {
                    ([map(0, &mut m.stats.bad_nodes), 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
                };
                bounds.add(s.positions[i]);
                m.vertices.push(ModelVertex {
                    position: s.positions[i],
                    uv: s.uvs.get(i).copied().unwrap_or([0.0; 2]),
                    normal: s.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]),
                    nodes,
                    weights,
                });
            }
            for part in &s.parts {
                if part.transparent {
                    m.stats.transparent_parts += 1;
                    continue;
                }
                let first_index = m.indices.len() as u32;
                for t in &part.triangles {
                    let bad = t.iter().any(|&i| usize::from(i) >= n) || t[0] == t[1] || t[1] == t[2] || t[0] == t[2];
                    if bad {
                        m.stats.dropped_triangles += 1;
                        continue;
                    }
                    m.indices.extend(t.iter().map(|&i| base + u32::from(i)));
                }
                let index_count = m.indices.len() as u32 - first_index;
                if index_count > 0 {
                    m.draws.push(ModelDraw {
                        material: part.material,
                        first_index,
                        index_count,
                    });
                }
            }
        }
        m.draws.sort_by_key(|d| (d.material, d.first_index));
        m.bounds = (!bounds.is_empty()).then_some(bounds);
        m
    }

    /// The model-local materials its draws use, ascending.
    pub fn materials(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.draws.iter().map(|d| d.material).collect();
        v.dedup();
        v
    }

    /// Where vertex `v` lands for the object's node matrices `nodes` (the
    /// vertex shader's arithmetic, on the CPU: for tests and checks).
    pub fn skin(&self, v: &ModelVertex, nodes: &[NodeMatrix]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for k in 0..4 {
            let w = v.weights[k];
            if w <= 0.0 {
                continue;
            }
            let i = usize::from(v.nodes[k]);
            let (Some(n), Some(inv)) = (nodes.get(i), self.inverse_binds.get(i)) else {
                continue;
            };
            let p = n.point(inv.point(v.position));
            for c in 0..3 {
                out[c] += w * p[c];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_z(position: [f32; 3]) -> NodeMatrix {
        // A quarter turn about z: x goes to y.
        NodeMatrix {
            scale: 1.0,
            forward: [0.0, 1.0, 0.0],
            left: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            position,
        }
    }

    #[test]
    fn the_layout_is_the_games_and_compose_applies_inner_first() {
        let m = NodeMatrix::from_words(&[2.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0]);
        assert_eq!(m, NodeMatrix { scale: 2.0, ..turn_z([10.0, 20.0, 30.0]) });
        assert_eq!(m.point([1.0, 0.0, 0.0]), [10.0, 22.0, 30.0]);
        let inner = NodeMatrix {
            position: [1.0, 0.0, 0.0],
            ..NodeMatrix::IDENTITY
        };
        let c = m.compose(&inner);
        let p = [0.5, 0.25, 1.0];
        let want = m.point(inner.point(p));
        let got = c.point(p);
        for k in 0..3 {
            assert!((want[k] - got[k]).abs() < 1e-5, "{want:?} {got:?}");
        }
        assert_eq!(std::mem::size_of::<NodeMatrix>(), 0x34);
    }

    /// Two nodes: the root at the origin and an arm whose rest frame is at
    /// x = 1. A vertex at x = 2 half on each.
    fn arm() -> ModelInput {
        ModelInput {
            inverse_binds: vec![
                NodeMatrix::IDENTITY,
                NodeMatrix {
                    position: [-1.0, 0.0, 0.0],
                    ..NodeMatrix::IDENTITY
                },
            ],
            sections: vec![ModelSectionInput {
                positions: vec![[0.0; 3], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                uvs: vec![[0.0; 2]; 3],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                // Local node 0 is model node 1 and local 1 is model node 0.
                node_indices: vec![[1, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]],
                node_weights: vec![[1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
                node_map: vec![1, 0],
                parts: vec![
                    ModelPartInput {
                        material: 3,
                        transparent: false,
                        triangles: vec![[0, 1, 2], [0, 0, 1]],
                    },
                    ModelPartInput {
                        material: 1,
                        transparent: true,
                        triangles: vec![[0, 1, 2]],
                    },
                ],
            }],
        }
    }

    #[test]
    fn sections_map_their_nodes_and_parts_become_draws() {
        let m = Model::build(&arm());
        assert_eq!(m.vertices.len(), 3);
        assert_eq!(m.vertices[1].nodes, [1, 0, 0, 0], "local 0 -> model 1, local 1 -> model 0");
        assert_eq!(m.vertices[0].nodes, [0, 0, 0, 0], "local 1 -> model 0");
        assert_eq!(m.draws, vec![ModelDraw { material: 3, first_index: 0, index_count: 3 }]);
        assert_eq!((m.stats.transparent_parts, m.stats.dropped_triangles), (1, 1));
        assert_eq!(m.materials(), vec![3]);
    }

    #[test]
    fn at_rest_the_model_is_where_it_was_built_and_a_turned_node_moves_its_share() {
        let m = Model::build(&arm());
        // Rest pose: each node's world matrix is its rest frame (the inverse's inverse).
        let rest = [
            NodeMatrix::IDENTITY,
            NodeMatrix {
                position: [1.0, 0.0, 0.0],
                ..NodeMatrix::IDENTITY
            },
        ];
        for v in &m.vertices {
            assert_eq!(m.skin(v, &rest), v.position);
        }
        // Turn the arm a quarter about z at its joint: the vertex at x = 2 is
        // half (2, 0, 0) and half (1, 1, 0).
        let posed = [rest[0], turn_z([1.0, 0.0, 0.0])];
        assert_eq!(m.skin(&m.vertices[1], &posed), [1.5, 0.5, 0.0]);
        // The vertex wholly on the arm (model node 1 via the map) turns with it.
        // (0, 1, 0) is (-1, 1, 0) from the joint; turned, (-1, -1, 0) from it.
        assert_eq!(m.skin(&m.vertices[2], &posed), [0.0, -1.0, 0.0]);
    }

    #[test]
    fn rigid_sections_bind_to_their_first_mapped_node_and_bad_indices_to_the_root() {
        let mut input = arm();
        let s = &mut input.sections[0];
        s.node_indices.clear();
        s.node_weights.clear();
        let m = Model::build(&input);
        assert!(m.vertices.iter().all(|v| v.nodes == [1, 0, 0, 0] && v.weights == [1.0, 0.0, 0.0, 0.0]));
        let mut input = arm();
        input.sections[0].node_indices[0] = [7, 0, 0, 0];
        let m = Model::build(&input);
        assert_eq!(m.vertices[0].nodes, [0, 0, 0, 0]);
        assert_eq!(m.stats.bad_nodes, 1);
    }
}
