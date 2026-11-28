//! `build_matrices`: an object's node records turned into node matrices,
//! as the skeleton walk of `bd090` does it with the pose matrix `1421f0`
//! and the composition `142a60` (docs/engine/skeleton-walk.md; phase 1b
//! of docs/engine/sim-rewrite.md).
//!
//! The walk visits nodes 1 to `count - 1` in index order. A node whose bit
//! is set in the keep mask keeps its matrix. Any other node's record (the
//! 32-byte [`Record`] of 1a's sampler) becomes a matrix, which is written
//! over the node's slot and then composed in place with its parent's
//! matrix. The parent is read from the array as it stands at that moment,
//! so a parent later in the order gives its matrix from before the walk,
//! and a node that names itself composes with its own fresh pose.
//!
//! The arithmetic is the guest's, lane for lane, dead lanes included, in
//! its order, through [`Fp`]. The walk also returns the XMM registers the
//! last composition leaves, because the guest's caller can observe them.
//! An operand off the fast path anywhere declines the whole walk; nothing
//! the caller holds has changed then except `out`, which it discards.
use crate::kernel::{Record, Unsupported};
use sim_core::det::{self, Fp};
use sim_core::lanes::Vec4;
use sim_core::math::{NodeMatrix, Vec3};

/// The image constants the two leaves read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    /// `[0x45dbc0]`, the image's 1.0.
    pub one: u32,
    /// The pose matrix's norm cutoff `[0x55ebb0]`, or `[0x45dbdc]` when the
    /// cutoff is not yet initialised (`[0x55ebb4]` bit 0 clear): the first
    /// pose copies it there before comparing.
    pub threshold: u32,
    /// The sign mask at `0x453750` the cross terms are XORed with.
    pub signs: Vec4,
}

/// What the walk leaves besides the matrices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    /// The last node posed, or `None` when the mask kept every node.
    pub last: Option<usize>,
    /// XMM0 to XMM7 after the walk: the last composition's, or the input
    /// unchanged when no node was posed.
    pub xmm: [Vec4; 8],
}

/// A matrix column as the composition loads it: `movss` the first value
/// (lanes 1 to 3 clear), then `movhps` the other two into lanes 2 and 3.
fn column(v: Vec3) -> Vec4 {
    [v.0[0], 0, v.0[1], v.0[2]]
}

/// The lanes `store_column` writes back: 0, 2 and 3.
fn stored(v: Vec4) -> Vec3 {
    Vec3([v[0], v[2], v[3]])
}

/// `1421f0`: a node record to a matrix. `xmm1` is the register on entry;
/// its upper two lanes enter one packed add whose upper lanes are dead.
///
/// A rotation whose squared norm is at or below the cutoff gives the
/// identity basis. Scale and position are copied as bits.
pub fn pose(fp: &mut Fp, k: &Constants, local: &Record, xmm1: Vec4) -> NodeMatrix {
    let q: Vec4 = [local[0], local[1], local[2], local[3]];
    let square = fp.mul4(q, q);
    let sums = fp.add4(square, [square[2], square[3], xmm1[2], xmm1[3]]);
    let norm = fp.add(sums[0], sums[1]);
    // `comiss norm, cutoff`, then `jbe` to the identity.
    fp.check(norm);
    fp.check(k.threshold);
    let one = k.one;
    let (forward, left, up) = if det::compare(norm, k.threshold).is_le() {
        ([one, 0, 0], [0, one, 0], [0, 0, one])
    } else {
        // One Newton step on the reciprocal estimate, doubled.
        let r = det::rcp(norm);
        let t = fp.mul(norm, r);
        let t = fp.mul(t, r);
        let twice = fp.add(r, r);
        let inverse = fp.sub(twice, t);
        let factor = fp.add(inverse, inverse);
        let scaled = fp.mul4(q, [factor; 4]);
        let xx = fp.mul4([q[0]; 4], scaled);
        let ww = fp.mul4([q[3]; 4], scaled);
        let yy = fp.mul4([q[1]; 4], scaled);
        let zz = fp.mul(scaled[2], q[2]);
        let w_terms = [ww[2], ww[1], ww[2], ww[1]];
        let signed: Vec4 = std::array::from_fn(|i| w_terms[i] ^ k.signs[i]);
        let cross = fp.add4([xx[1], xx[2], xx[1], xx[2]], signed);
        let one_minus_z = fp.sub(one, zz);
        let fx = fp.sub(one_minus_z, yy[1]);
        let middle = fp.sub(one_minus_z, xx[0]);
        let one_minus_x = fp.sub(one, xx[0]);
        let last = fp.sub(one_minus_x, yy[1]);
        let off = fp.sub(yy[2], ww[0]);
        let other = fp.add(yy[2], ww[0]);
        (
            [fx, cross[2], cross[3]],
            [cross[0], middle, other],
            [cross[1], off, last],
        )
    };
    NodeMatrix {
        scale: local[7],
        forward: Vec3(forward),
        left: Vec3(left),
        up: Vec3(up),
        position: Vec3([local[4], local[5], local[6]]),
    }
}

/// `142a60`: `a ∘ b`, the parent `a` applied to the child `b`, and the
/// XMM registers it leaves. Every value of both is read before the first
/// store, so the result does not depend on whether the output slot is
/// `a`, `b` or neither.
pub fn compose(fp: &mut Fp, a: &NodeMatrix, b: &NodeMatrix) -> (NodeMatrix, [Vec4; 8]) {
    let product = fp.mul(b.scale, a.scale);
    let basis = [column(a.forward), column(a.left), column(a.up)];
    let [tx, ty, tz] = b.position.0;
    let x = fp.mul4([tx; 4], basis[0]);
    let y = fp.mul4([ty; 4], basis[1]);
    let z = fp.mul4([tz; 4], basis[2]);
    let xy = fp.add4(x, y);
    let position = fp.add4(xy, z);
    let scaled = fp.mul4(position, [a.scale; 4]);
    let mut result = fp.add4(scaled, column(a.position));
    // The child's up, left and forward columns, in the guest's order; each
    // pass stores the previous result (position, up, left).
    let mut done = [Vec3::default(); 3];
    let (mut yz, mut zv) = ([0; 4], [0; 4]);
    for (slot, v) in [b.up, b.left, b.forward].into_iter().enumerate() {
        done[slot] = stored(result);
        let z = fp.mul4([v.0[2]; 4], basis[2]);
        let y = fp.mul4([v.0[1]; 4], basis[1]);
        let x = fp.mul4([v.0[0]; 4], basis[0]);
        yz = fp.add4(y, z);
        result = fp.add4(x, yz);
        zv = z;
    }
    let [position, up, left] = done;
    let m = NodeMatrix {
        scale: product,
        forward: stored(result),
        left,
        up,
        position,
    };
    let xmm = [
        result,
        yz,
        zv,
        [product, 0, 0, 0],
        basis[0],
        basis[1],
        basis[2],
        [a.scale; 4],
    ];
    (m, xmm)
}

/// The skeleton walk over `out`, whose length is the node count. `out[0]`
/// is the root, already built; `locals`, `parents` and `keep` cover at
/// least `out.len()` nodes (`keep` one bit per node, LSB first). `xmm` is
/// the register file on entry.
///
/// Declines when a parent index is outside `out`, when an input is short,
/// or when any operand of any operation is off the fast path.
pub fn build_matrices(
    k: &Constants,
    locals: &[Record],
    parents: &[i16],
    keep: &[u32],
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<Walk, Unsupported> {
    build_with(Fp::new(), k, locals, parents, keep, xmm, out).map(|(walk, _)| walk)
}

/// [`build_matrices`] under the guest's `mxcsr`, as `bd090`'s SSE code
/// runs whatever the MXCSR holds ([`Fp::guest`]): the walk and the MXCSR
/// after it, with every flag an operation raised. Declines on the same
/// input faults, and on an operation whose exception the MXCSR unmasks.
pub fn build_matrices_guest(
    mxcsr: u32,
    k: &Constants,
    locals: &[Record],
    parents: &[i16],
    keep: &[u32],
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<(Walk, u32), Unsupported> {
    let (walk, fp) = build_with(Fp::guest(mxcsr), k, locals, parents, keep, xmm, out)?;
    Ok((walk, fp.mxcsr().unwrap_or(mxcsr)))
}

fn build_with(
    mut fp: Fp,
    k: &Constants,
    locals: &[Record],
    parents: &[i16],
    keep: &[u32],
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<(Walk, Fp), Unsupported> {
    let count = out.len();
    if locals.len() < count || parents.len() < count || keep.len() * 32 < count {
        return Err(Unsupported);
    }
    let mut walk = Walk { last: None, xmm };
    for i in 1..count {
        if keep[i >> 5] >> (i & 31) & 1 != 0 {
            continue;
        }
        let posed = pose(&mut fp, k, &locals[i], walk.xmm[1]);
        out[i] = posed;
        let parent = usize::try_from(parents[i])
            .ok()
            .filter(|&p| p < count)
            .ok_or(Unsupported)?;
        let (m, regs) = compose(&mut fp, &out[parent], &posed);
        out[i] = m;
        walk = Walk { last: Some(i), xmm: regs };
    }
    if fp.exact() { Ok((walk, fp)) } else { Err(Unsupported) }
}
