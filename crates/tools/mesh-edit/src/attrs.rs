//! Face-corner attributes and vertex skin.

use crate::math::{self, Vec2, Vec3};

/// Which of a corner's attributes an edit has changed since import. The
/// writer re-encodes a dirty attribute and keeps the source's packed bytes
/// for a clean one.
pub mod dirty {
    pub const POSITION: u8 = 1;
    pub const UV: u8 = 2;
    pub const LIGHTMAP: u8 = 4;
    pub const NORMAL: u8 = 8;
    /// Binormal and tangent.
    pub const FRAME: u8 = 16;
    pub const LIGHT: u8 = 32;
    pub const SKIN: u8 = 64;
    pub const ALL: u8 = 0x7f;
}

/// What a vertex buffer stores apart from position, per face corner. A UV
/// seam or a hard edge is two corners of one vertex with different values,
/// as the source stores it.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Corner {
    /// Texture coordinate.
    pub uv: Vec2,
    /// Lightmap coordinate.
    pub lightmap: Vec2,
    /// Unit or zero.
    pub normal: Vec3,
    pub binormal: Vec3,
    pub tangent: Vec3,
    /// Incident light direction (Halo 2's per-vertex lighting term).
    pub light_dir: Vec3,
    /// The source vertex this corner came from, if imported.
    pub src: Option<u32>,
    /// `dirty::*` bits: what changed since import.
    pub dirty: u8,
}

impl Corner {
    /// The same attributes as a new corner: no source vertex, all dirty.
    pub fn fresh(&self) -> Corner {
        Corner { src: None, dirty: dirty::ALL, ..*self }
    }

    /// `self` at 0, `other` at 1. Directions are renormalized. The result
    /// is a new corner (no source, all dirty).
    pub fn lerp(&self, other: &Corner, t: f32) -> Corner {
        let dir = |a: Vec3, b: Vec3| {
            let v = math::lerp(a, b, t);
            let n = math::normalize(v);
            if n == [0.0; 3] { v } else { n }
        };
        Corner {
            uv: math::lerp2(self.uv, other.uv, t),
            lightmap: math::lerp2(self.lightmap, other.lightmap, t),
            normal: dir(self.normal, other.normal),
            binormal: dir(self.binormal, other.binormal),
            tangent: dir(self.tangent, other.tangent),
            light_dir: dir(self.light_dir, other.light_dir),
            src: None,
            dirty: dirty::ALL,
        }
    }

    /// The attribute values' exact bits (not `src` or `dirty`): two corners
    /// with equal keys export as one vertex.
    pub fn key(&self) -> [u32; 16] {
        let mut k = [0u32; 16];
        let values = [
            self.uv[0], self.uv[1], self.lightmap[0], self.lightmap[1],
            self.normal[0], self.normal[1], self.normal[2],
            self.binormal[0], self.binormal[1], self.binormal[2],
            self.tangent[0], self.tangent[1], self.tangent[2],
            self.light_dir[0], self.light_dir[1], self.light_dir[2],
        ];
        for (slot, value) in k.iter_mut().zip(values) {
            *slot = value.to_bits();
        }
        k
    }

    /// Whether every value is finite and the normal is unit or zero.
    pub(crate) fn is_sane(&self) -> bool {
        let finite = self.key().iter().all(|&bits| f32::from_bits(bits).is_finite());
        let len = math::length(self.normal);
        finite && (len == 0.0 || (len - 1.0).abs() < 1e-3)
    }
}

/// How many nodes a skinned vertex follows.
pub const MAX_INFLUENCES: usize = 4;

/// The nodes a vertex follows, as Halo 2 packs them: up to four
/// `(node, weight)` byte pairs whose weights sum to 255. Unused pairs have
/// weight 0.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Skin {
    pub nodes: [u8; MAX_INFLUENCES],
    pub weights: [u8; MAX_INFLUENCES],
}

impl Skin {
    /// Follows one node fully.
    pub fn rigid(node: u8) -> Skin {
        Skin { nodes: [node, 0, 0, 0], weights: [255, 0, 0, 0] }
    }

    pub fn total(&self) -> u32 {
        self.weights.iter().map(|&w| u32::from(w)).sum()
    }

    /// The weighted mix of skins: influences on one node add, the four
    /// strongest are kept, and the weights are renormalized to sum to 255
    /// (largest remainders round up). `None` if no weight is left.
    pub fn blend(parts: &[(Skin, f32)]) -> Option<Skin> {
        let mut acc: Vec<(u8, f32)> = Vec::new();
        for &(skin, w) in parts {
            for k in 0..MAX_INFLUENCES {
                let x = f32::from(skin.weights[k]) * w;
                if x <= 0.0 {
                    continue;
                }
                match acc.iter_mut().find(|(node, _)| *node == skin.nodes[k]) {
                    Some(entry) => entry.1 += x,
                    None => acc.push((skin.nodes[k], x)),
                }
            }
        }
        acc.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        acc.truncate(MAX_INFLUENCES);
        let total: f32 = acc.iter().map(|&(_, w)| w).sum();
        if total <= 0.0 {
            return None;
        }
        let scaled: Vec<f32> = acc.iter().map(|&(_, w)| w / total * 255.0).collect();
        let mut bytes: Vec<u32> = scaled.iter().map(|&s| s.floor() as u32).collect();
        let mut short = 255 - bytes.iter().sum::<u32>().min(255);
        let mut order: Vec<usize> = (0..bytes.len()).collect();
        order.sort_by(|&a, &b| (scaled[b] - scaled[b].floor()).total_cmp(&(scaled[a] - scaled[a].floor())));
        for &i in order.iter().cycle().take(order.len() * 255) {
            if short == 0 {
                break;
            }
            bytes[i] += 1;
            short -= 1;
        }
        let mut out = Skin::default();
        for (k, (&(node, _), &w)) in acc.iter().zip(&bytes).enumerate() {
            out.nodes[k] = node;
            out.weights[k] = w.min(255) as u8;
        }
        Some(out)
    }
}

/// Blends the skins that are present; `None` if none are.
pub(crate) fn blend_skins(parts: &[(Option<Skin>, f32)]) -> Option<Skin> {
    let present: Vec<(Skin, f32)> =
        parts.iter().filter_map(|&(skin, w)| skin.map(|s| (s, w))).collect();
    if present.is_empty() { None } else { Skin::blend(&present) }
}
