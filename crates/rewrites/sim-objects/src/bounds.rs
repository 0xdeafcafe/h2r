//! The bounds arithmetic of `bdef0` (docs/engine/object-update.md, "Slice 4
//! bodies"; design docs/engine/sim-rewrite-objects.md sections 2 and 7):
//! the two bounding spheres from the root node matrix and the definition,
//! and whether the spatial partition must relink. A pure kernel: no guest
//! memory, no globals. The guest body (`engine::sim_objects::bounds`)
//! runs the same arithmetic on the guest's `xmm` registers; this is the
//! form a speculated pass (`sim_core::Spec`, slice 12) computes.
//!
//! Every operation is retail's scalar SSE in retail's order through
//! [`Fp`]: exact when every operand is on the fast path, and otherwise
//! [`Fp::exact`] is false and the caller computes the bounds in the guest
//! body instead.

use crate::defs::Def;
use sim_core::det::Fp;

/// A node matrix as the guest stores it (`34` bytes): scale, forward,
/// left, up, translation, each as binary32 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeMatrix {
    pub scale: u32,
    pub forward: [u32; 3],
    pub left: [u32; 3],
    pub up: [u32; 3],
    pub translation: [u32; 3],
}

impl NodeMatrix {
    /// From the 13 dwords at the matrix's address.
    pub fn from_words(w: [u32; 13]) -> NodeMatrix {
        NodeMatrix {
            scale: w[0],
            forward: [w[1], w[2], w[3]],
            left: [w[4], w[5], w[6]],
            up: [w[7], w[8], w[9]],
            translation: [w[10], w[11], w[12]],
        }
    }
}

/// The spheres `bdef0` stores: body `+30` (centre) and `+3c` (radius),
/// `+50` and `+5c`, and the one the partition registers (`+40`, `+4c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub centre: [u32; 3],
    pub radius: u32,
    pub second_centre: [u32; 3],
    pub second_radius: u32,
    /// Definition flags bit 12: the partition uses the second sphere.
    pub partition_second: bool,
}

impl Bounds {
    /// The sphere the partition registers.
    pub fn partition(&self) -> ([u32; 3], u32) {
        if self.partition_second {
            (self.second_centre, self.second_radius)
        } else {
            (self.centre, self.radius)
        }
    }
}

/// `x` through the matrix rows: `(m_z·z + m_y·y) + m_x·x + t` per
/// component, the order `bdef0` and `b9dd0` use.
fn point(fp: &mut Fp, m: &NodeMatrix, v: [u32; 3]) -> [u32; 3] {
    let mut out = [0; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let z = fp.mul(m.up[i], v[2]);
        let y = fp.mul(m.left[i], v[1]);
        let zy = fp.add(z, y);
        let x = fp.mul(m.forward[i], v[0]);
        let row = fp.add(zy, x);
        *o = fp.add(row, m.translation[i]);
    }
    out
}

/// The spheres of an object whose definition is `def`, root node matrix
/// `root` and body scale `scale` (`+a0`), as `bdef0` computes them when
/// no component supplies the sphere. `None` when an operand left the
/// fast path: the caller then runs the guest body.
pub fn spheres(def: &Def, root: &NodeMatrix, scale: u32) -> Option<Bounds> {
    let mut fp = Fp::new();
    let mut offset = def.bounding_offset.map(f32::to_bits);
    // `ucomiss scale, 1.0`: unordered or not equal scales the offset.
    fp.check(root.scale);
    if f32::from_bits(root.scale) != 1.0 {
        offset = offset.map(|x| fp.mul(root.scale, x));
    }
    let centre = point(&mut fp, root, offset);
    let radius = fp.mul(scale, def.bounding_radius.to_bits());
    let dynamic = def.dynamic_scale.to_bits();
    fp.check(dynamic);
    let (second_centre, second_radius) = if def.dynamic_scale > 0.0 {
        let d = def.dynamic_offset.map(f32::to_bits);
        let c = [0, 1, 2].map(|i| fp.add(centre[i], d[i]));
        (c, fp.mul(scale, dynamic))
    } else {
        (centre, radius)
    };
    let bounds = Bounds {
        centre,
        radius,
        second_centre,
        second_radius,
        partition_second: def.flags & 0x1000 != 0,
    };
    fp.exact().then_some(bounds)
}

/// Whether an object already in the partition must relink: its centre
/// moved by more than `threshold` (squared distance, `comiss`; `ja`) or
/// its radius changed (`ucomiss`). `None` off the fast path.
pub fn relink(registered: ([u32; 3], u32), new: ([u32; 3], u32), threshold: u32) -> Option<bool> {
    let mut fp = Fp::new();
    let d = [0, 1, 2].map(|i| fp.sub(new.0[i], registered.0[i]));
    let zz = fp.mul(d[2], d[2]);
    let xx = fp.mul(d[0], d[0]);
    let yy = fp.mul(d[1], d[1]);
    let zx = fp.add(zz, xx);
    let squared = fp.add(zx, yy);
    fp.check(threshold);
    fp.check(registered.1);
    fp.check(new.1);
    let moved = f32::from_bits(squared) > f32::from_bits(threshold);
    let changed = f32::from_bits(registered.1) != f32::from_bits(new.1);
    fp.exact().then_some(moved || changed)
}

#[cfg(test)]
mod tests;
