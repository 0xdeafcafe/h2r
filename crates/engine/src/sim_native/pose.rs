use super::*;

fn column(g: &impl Memory, p: u32) -> std::result::Result<Vec4, GuestFault> {
    // MOVSS from memory clears the otherwise unused second lane.
    let x = g.u32(p)?;
    let yz = g.u64(at(p, 4))?;
    Ok([x, 0, yz as u32, (yz >> 32) as u32])
}

fn store_column(g: &mut impl Memory, p: u32, v: Vec4) -> std::result::Result<(), GuestFault> {
    g.set_u32(p, v[0])?;
    g.set_u64(at(p, 4), u64::from(v[2]) | (u64::from(v[3]) << 32))
}

/// Compose two scaled affine transforms, including in-place products.
pub fn compose(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let (a, b) = (c.ecx, c.edx);
    c.eax = g.u32(at(c.esp, 4))?;
    let out = c.eax;
    let scale = g.u32(a)?;
    let rhs_scale = g.u32(b)?;
    let tx = g.u32(at(b, 40))?;
    let ty = g.u32(at(b, 44))?;
    let tz = g.u32(at(b, 48))?;
    let product = op(c, Mul, rhs_scale, scale);
    let basis = [
        column(g, at(a, 4))?,
        column(g, at(a, 16))?,
        column(g, at(a, 28))?,
    ];
    let x = vector(c, Mul, [tx; 4], basis[0]);
    let y = vector(c, Mul, [ty; 4], basis[1]);
    let z = vector(c, Mul, [tz; 4], basis[2]);
    let xy = vector(c, Add, x, y);
    let position = vector(c, Add, xy, z);
    let scaled = vector(c, Mul, position, [scale; 4]);
    let mut result = vector(c, Add, scaled, column(g, at(a, 40))?);
    for (i, v) in basis.into_iter().enumerate() {
        put_xmm(c, 4 + i, v);
    }
    put_xmm(c, 7, [scale; 4]);
    put_xmm(c, 3, scalar(product));

    // The last coefficient of the next column is latched before the
    // preceding store. Partial overlap can overwrite its other two.
    for offset in [28, 16, 4] {
        let z = g.u32(at(b, offset + 8))?;
        store_column(g, at(out, offset + 12), result)?;
        let y = g.u32(at(b, offset + 4))?;
        let x = g.u32(at(b, offset))?;
        let z = vector(c, Mul, [z; 4], basis[2]);
        let y = vector(c, Mul, [y; 4], basis[1]);
        let x = vector(c, Mul, [x; 4], basis[0]);
        let yz = vector(c, Add, y, z);
        result = vector(c, Add, x, yz);
        put_xmm(c, 1, yz);
        put_xmm(c, 2, z);
    }
    put_xmm(c, 0, result);
    store_column(g, at(out, 4), result)?;
    g.set_u32(out, product)?;
    ret(g, c, 4)
}

pub const POSE_INIT_FLAGS: u32 = 0x0055_ebb4;
pub const POSE_NORM_THRESHOLD: u32 = 0x0055_ebb0;
const SIGNS: u32 = 0x0045_3750;
use crate::function_eval::constant::{ONE, SMALL};

/// Expand a quaternion, translation and uniform scale to an affine matrix.
pub fn pose_matrix(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = c.esp;
    if g.u8(POSE_INIT_FLAGS)? & 1 == 0 {
        let flags = g.u32(POSE_INIT_FLAGS)?;
        let threshold = g.u32(SMALL)?;
        g.set_u32(POSE_INIT_FLAGS, flags | 1)?;
        g.set_u32(POSE_NORM_THRESHOLD, threshold)?;
    }
    let p = g.u32(at(c.ebp, 12))?;
    let out = g.u32(at(c.ebp, 8))?;
    c.eax = p;
    c.ecx = out;
    let xy = g.u64(p)?;
    let zw = g.u64(at(p, 8))?;
    let q = [xy as u32, (xy >> 32) as u32, zw as u32, (zw >> 32) as u32];
    put_xmm(c, 7, q);
    let square = vector(c, Mul, q, q);
    let [_, _, hi2, hi3] = xmm(c, 1);
    let sums = vector(c, Add, square, [square[2], square[3], hi2, hi3]);
    let norm = op(c, Add, sums[0], sums[1]);
    put_xmm(c, 1, [sums[1]; 4]);
    if compare(c, norm, g.u32(POSE_NORM_THRESHOLD)?) != 0 {
        let one = g.u32(ONE)?;
        for offset in [4, 20] {
            g.set_u64(at(out, offset), u64::from(one))?;
            g.set_u64(at(out, offset + 8), 0)?;
        }
        g.set_u32(at(out, 36), one)?;
    } else {
        let r = simd::rcp_approx(norm);
        let t = op(c, Mul, norm, r);
        let t = op(c, Mul, t, r);
        let twice = op(c, Add, r, r);
        let inverse = op(c, Sub, twice, t);
        let factor = op(c, Add, inverse, inverse);
        let scaled = vector(c, Mul, q, [factor; 4]);
        let xx = vector(c, Mul, [q[0]; 4], scaled);
        let ww = vector(c, Mul, [q[3]; 4], scaled);
        let yy = vector(c, Mul, [q[1]; 4], scaled);
        let zz = op(c, Mul, scaled[2], q[2]);
        let signs = unpack(g.u128(SIGNS)?);
        let w_terms = [ww[2], ww[1], ww[2], ww[1]];
        let signed = std::array::from_fn(|i| w_terms[i] ^ signs[i]);
        let cross = vector(c, Add, [xx[1], xx[2], xx[1], xx[2]], signed);
        g.set_u64(at(out, 8), (pack(cross) >> 64) as u64)?;
        g.set_u32(at(out, 16), cross[0])?;
        g.set_u64(at(out, 24), pack(cross) as u64)?;
        let one_minus_z = op(c, Sub, g.u32(ONE)?, zz);
        g.set_u32(at(out, 4), op(c, Sub, one_minus_z, yy[1]))?;
        let middle = op(c, Sub, one_minus_z, xx[0]);
        g.set_u32(at(out, 20), middle)?;
        let one_minus_x = op(c, Sub, g.u32(ONE)?, xx[0]);
        let last = op(c, Sub, one_minus_x, yy[1]);
        g.set_u32(at(out, 36), last)?;
        let off = op(c, Sub, yy[2], ww[0]);
        g.set_u32(at(out, 32), off)?;
        let other = op(c, Add, yy[2], ww[0]);
        g.set_u32(at(out, 24), other)?;
        put_xmm(c, 1, [middle, signed[1], signed[2], signed[3]]);
        let [_, _, hi2, hi3] = xmm(c, 2);
        put_xmm(c, 2, [other, yy[3], hi2, hi3]);
        put_xmm(c, 3, scalar(last));
        put_xmm(c, 4, xx);
        put_xmm(c, 5, ww);
        put_xmm(c, 6, [yy[1]; 4]);
        put_xmm(c, 7, [off, scaled[3], scaled[2], scaled[3]]);
    }
    g.set_u32(out, g.u32(at(p, 28))?)?;
    g.set_u64(at(out, 40), g.u64(at(p, 16))?)?;
    let z = g.u32(at(p, 24))?;
    put_xmm(c, 0, scalar(z));
    g.set_u32(at(out, 48), z)?;
    c.ebp = pop(c, g)?;
    ret(g, c, 8)
}

pub const ROTATION_BLOCK: u32 = 0x0050_4480;
pub const ROTATION_TRACK: u32 = 0x0050_44b4;
pub const ROTATION_OUT: u32 = 0x0050_44c0;
pub const ROTATION_WEIGHT: u32 = 0x0050_446c;
pub const FRAME_A: u32 = 0x0050_4464;
pub const FRAME_B: u32 = 0x0050_4468;

/// Blend-screen codec. The reciprocal square root is the Pentium III
/// approximation, with a distinct sum order for each lane.
pub fn rotation_codec(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(16);
    let block = g.u32(ROTATION_BLOCK)?;
    let stride = g.u32(at(block, 20))?;
    let track = at(at(block, 32), stride.wrapping_mul(g.u32(ROTATION_TRACK)?));
    let out = g.u32(ROTATION_OUT)?;
    let weight = g.u32(ROTATION_WEIGHT)?;
    let b = g.u32(FRAME_B)?;
    g.set_u32(c.esp, out)?;
    let a = g.u32(FRAME_A)?;
    c.ecx = at(track, b << 4);
    c.eax = at(track, a << 4);
    g.set_u32(at(c.esp, 4), weight)?;
    g.set_u32(at(c.esp, 8), c.ecx)?;
    g.set_u32(at(c.esp, 12), c.eax)?;
    let read = |p| {
        let mut bytes = [0; 16];
        g.read_bytes(p, &mut bytes, 16)?;
        Ok::<_, GuestFault>(unpack(u128::from_le_bytes(bytes)))
    };
    let a = read(c.eax)?;
    let b = read(c.ecx)?;
    let weight = [weight; 4];
    let delta = vector(c, Sub, b, a);
    let scaled = vector(c, Mul, delta, weight);
    let blend = vector(c, Add, scaled, a);
    let squares = vector(c, Mul, blend, blend);
    let swapped = [squares[2], squares[3], squares[0], squares[1]];
    let pairs = vector(c, Add, squares, swapped);
    let transposed = [pairs[1], pairs[0], pairs[1], pairs[0]];
    let sum = vector(c, Add, pairs, transposed);
    let inv = sum.map(simd::rsqrt_approx);
    c.edx = g.u32(c.esp)?;
    let rotation = vector(c, Mul, blend, inv);
    g.write_bytes(c.edx, &pack(rotation).to_le_bytes(), 16)?;
    for (i, v) in [
        (0, inv),
        (1, a),
        (2, rotation),
        (3, weight),
        (6, transposed),
        (7, swapped),
    ] {
        put_xmm(c, i, v);
    }
    let frame = c.esp;
    add_flags(c, frame, 16);
    c.esp = at(frame, 16);
    ret(g, c, 0)
}
