//! A model's marker groups: the scan `1d8f00` and the transform fill
//! `1d8f50` of `b8d30` (docs/engine/markers.md; phase 1c of
//! docs/engine/sim-rewrite.md).
//!
//! A model tag holds marker groups in name order, each a name, an
//! instance count and an instance array; [`find_group`] is the linear
//! scan for the first group of a name. Each instance names the node it
//! rides (a byte, with `0xff` for none), carries a name byte the node
//! array must match, a translation, a rotation quaternion and a trailing
//! float. [`select`] walks the instances in the guest's order, and
//! [`model_markers`] turns the selected ones into the guest's `0x70`-byte
//! records: the node, the local transform (the basis leaf `141e10` on the
//! quaternion, the scale 1, the instance's translation), the world
//! transform (the local one composed onto the node's matrix through
//! `142a60`, the same [`compose`](crate::skeleton::compose) the skeleton
//! walk uses), and the trailing float. A mirrored object negates the left
//! column of the world transform, as `0 - x` rather than a sign flip, so
//! a zero column stays zero.
//!
//! No guest memory is touched; the engine adapter
//! (`engine::sim_anim::markers`) gathers the bytes and writes the
//! records. The arithmetic is the guest's, lane for lane, through
//! [`Fp`]; an operand off the fast path declines the whole call.
use crate::kernel::Unsupported;
use crate::skeleton::compose;
use sim_core::det::{self, Fp};
use sim_core::math::{NodeMatrix, Vec3};

/// One instance, `0x24` bytes: the node byte `+0`, the name byte `+1`, the
/// output node `+2`, the translation `+4`, the quaternion `+0x10` and the
/// trailing float `+0x20`.
pub const INSTANCE_BYTES: usize = 0x24;
/// One output record: the node `+0`, the local matrix `+4`, the world
/// matrix `+0x38` and the trailing float `+0x6c`.
pub const MARKER_BYTES: usize = 0x70;

/// The image constants the leaves read: 1.0 `[0x45dbc0]`, the norm cutoff
/// `[0x45dbdc]` and the dividend `[0x45dc0c]` the basis leaf divides by
/// the squared norm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    pub one: u32,
    pub small: u32,
    pub two: u32,
}

/// `1d8f00`: the first group whose name is `name`. The sentinel `-1` a
/// caller passes for "no group" never matches a name.
pub fn find_group(names: &[u32], name: u32) -> Option<usize> {
    names.iter().position(|&n| n == name)
}

/// `141e10`: a quaternion to the nine basis floats, the forward, left and
/// up columns. A squared norm at or below the cutoff gives a scale factor
/// of zero, so the products vanish and the basis comes out the identity.
pub fn basis(fp: &mut Fp, k: &Constants, q: &[u32; 4]) -> [u32; 9] {
    let [q0, q1, q2, q3] = *q;
    // The squared norm, one square and one add at a time, as the guest
    // accumulates it: ((q0² + q1²) + q2²) + q3².
    let (a, b) = (fp.mul(q0, q0), fp.mul(q1, q1));
    let ab = fp.add(a, b);
    let c = fp.mul(q2, q2);
    let abc = fp.add(ab, c);
    let d = fp.mul(q3, q3);
    let norm = fp.add(abc, d);
    // `comiss norm, cutoff`, then `jbe` to a zero factor.
    fp.check(norm);
    fp.check(k.small);
    let s = if det::compare(norm, k.small).is_le() {
        0
    } else {
        fp.div(k.two, norm)
    };
    // Every product below is one `mulss` of a factor already scaled: the
    // guest scales q0, q1 and q2 by s and multiplies each quaternion
    // component into the scaled factors, so no product squares before
    // scaling.
    let q0s = fp.mul(q0, s);
    let q1s = fp.mul(q1, s);
    let q2s = fp.mul(s, q2);
    let q0q0 = fp.mul(q0, q0s);
    let q0q1 = fp.mul(q0, q1s);
    let q0q2 = fp.mul(q0, q2s);
    let q0q3 = fp.mul(q0s, q3);
    let q1q1 = fp.mul(q1s, q1);
    let q1q2 = fp.mul(q1, q2s);
    let q1q3 = fp.mul(q1s, q3);
    let q2q2 = fp.mul(q2s, q2);
    let q2q3 = fp.mul(q2s, q3);
    let d0 = fp.add(q2q2, q1q1);
    let d1 = fp.add(q2q2, q0q0);
    let d2 = fp.add(q1q1, q0q0);
    [
        fp.sub(k.one, d0),
        fp.add(q0q1, q2q3),
        fp.sub(q0q2, q1q3),
        fp.sub(q0q1, q2q3),
        fp.sub(k.one, d1),
        fp.add(q1q2, q0q3),
        fp.add(q0q2, q1q3),
        fp.sub(q1q2, q0q3),
        fp.sub(k.one, d2),
    ]
}

/// One instance the guest emits: where it sits and the node it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The instance's offset in the instance array.
    pub at: usize,
    /// The output node: the instance's third byte, as the `u16` the guest
    /// stores (and composes with, sign-extended; without a remap table it
    /// is the byte, so never negative).
    pub node: u16,
}

/// The instances `1d8f50` emits, in its order: the name filter first,
/// then the capacity check that ends the walk (a full output stops it
/// rather than skipping on). `nodes` is the 256-byte window around the
/// object's node array, byte `128 + i8` for an instance naming node
/// `i8`; `None`, like a null node array, skips the filter, and so does
/// an instance naming no node (`0xff`).
pub fn select(instances: &[u8], nodes: Option<&[u8; 256]>, capacity: i32) -> Vec<Selection> {
    let mut out = Vec::new();
    for (n, instance) in instances.chunks_exact(INSTANCE_BYTES).enumerate() {
        if let Some(nodes) = nodes
            && instance[0] != 0xff
            && nodes[(128 + i32::from(instance[0] as i8)) as usize] != instance[1]
        {
            continue;
        }
        if out.len() as i32 >= capacity {
            break;
        }
        out.push(Selection {
            at: n * INSTANCE_BYTES,
            node: u16::from(instance[2]),
        });
    }
    out
}

/// One output marker, the guest's `0x70`-byte record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    pub node: u16,
    pub local: NodeMatrix,
    pub world: NodeMatrix,
    /// The instance's trailing float, copied uninterpreted.
    pub extra: u32,
}

impl Marker {
    pub fn to_bytes(&self) -> [u8; MARKER_BYTES] {
        let mut out = [0u8; MARKER_BYTES];
        out[..2].copy_from_slice(&self.node.to_le_bytes());
        out[4..0x38].copy_from_slice(&self.local.to_bytes());
        out[0x38..0x6c].copy_from_slice(&self.world.to_bytes());
        out[0x6c..].copy_from_slice(&self.extra.to_le_bytes());
        out
    }
}

/// The marker records for a selection of instances. Declines when any
/// operand leaves the fast path or a selection's node is outside
/// `matrices`.
pub fn model_markers(
    k: &Constants,
    instances: &[u8],
    sel: &[Selection],
    matrices: &[NodeMatrix],
    mirror: bool,
) -> Result<Vec<Marker>, Unsupported> {
    markers_with(Fp::new(), k, instances, sel, matrices, mirror).map(|(out, _)| out)
}

/// [`model_markers`] under the guest's `mxcsr`, as `b8d30`'s SSE code runs
/// whatever the MXCSR holds ([`Fp::guest`]): the records and the MXCSR
/// afterwards. Declines only on an unmasked exception or a selection's
/// node outside `matrices`.
pub fn model_markers_guest(
    mxcsr: u32,
    k: &Constants,
    instances: &[u8],
    sel: &[Selection],
    matrices: &[NodeMatrix],
    mirror: bool,
) -> Result<(Vec<Marker>, u32), Unsupported> {
    markers_with(Fp::guest(mxcsr), k, instances, sel, matrices, mirror)
        .map(|(out, m)| (out, m.unwrap_or(mxcsr)))
}

fn markers_with(
    mut fp: Fp,
    k: &Constants,
    instances: &[u8],
    sel: &[Selection],
    matrices: &[NodeMatrix],
    mirror: bool,
) -> Result<(Vec<Marker>, Option<u32>), Unsupported> {
    let mut out = Vec::with_capacity(sel.len());
    for s in sel {
        let instance = &instances[s.at..s.at + INSTANCE_BYTES];
        let word = |n: usize| u32::from_le_bytes(instance[n..n + 4].try_into().unwrap());
        let q = [word(0x10), word(0x14), word(0x18), word(0x1c)];
        let b = basis(&mut fp, k, &q);
        let local = NodeMatrix {
            scale: k.one,
            forward: Vec3([b[0], b[1], b[2]]),
            left: Vec3([b[3], b[4], b[5]]),
            up: Vec3([b[6], b[7], b[8]]),
            position: Vec3([word(4), word(8), word(0xc)]),
        };
        let matrix = matrices.get(usize::from(s.node)).ok_or(Unsupported)?;
        let (mut world, _) = compose(&mut fp, matrix, &local);
        if mirror {
            world.left = Vec3([
                fp.sub(0, world.left.0[0]),
                fp.sub(0, world.left.0[1]),
                fp.sub(0, world.left.0[2]),
            ]);
        }
        out.push(Marker {
            node: s.node,
            local,
            world,
            extra: word(0x20),
        });
    }
    if fp.exact() {
        Ok((out, fp.mxcsr()))
    } else {
        Err(Unsupported)
    }
}
