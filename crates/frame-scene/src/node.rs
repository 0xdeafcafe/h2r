//! The game's node matrix, as objects carry their pose.

use bytemuck::{Pod, Zeroable};
use serde::Serialize;

/// The game's node matrix (`real_matrix4x3`, `0x34` bytes): `+0` scale,
/// `+4` forward, `+0x10` left, `+0x1c` up, `+0x28` position. A local point
/// `p` maps to `scale · (p.x·forward + p.y·left + p.z·up) + position`
/// (Code: the bounding-sphere update `0x000bdf90`). The same layout holds a
/// mode node's inverse rest frame at node `+0x28`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable, Serialize)]
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
