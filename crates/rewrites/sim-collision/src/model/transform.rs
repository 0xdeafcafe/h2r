//! Use 141590 for the inverse of an instance or an object node. When
//! 1eff40 applies it, output X sums Y, X, Z; the other rows sum Z, Y, X.
use crate::{Bail, GuestRead, instance::PlacementMatrix};
use sim_core::det::Fp;

pub(super) fn matrix(g: &impl GuestRead, at: u32) -> Result<PlacementMatrix, Bail> {
    let mut bytes = [0; 52];
    g.read(at, &mut bytes)?;
    Ok(PlacementMatrix::from_words(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
    })))
}

pub(super) fn local_ray(
    matrix: PlacementMatrix,
    one: u32,
    start: [u32; 3],
    delta: [u32; 3],
) -> Result<([u32; 3], [u32; 3]), Bail> {
    let mut fp = Fp::new();
    let ray = local_ray_with(&mut fp, matrix, one, start, delta);
    fp.exact().then_some(ray).ok_or(Bail::Float)
}

/// [`local_ray`] in the caller's `fp` ([`Fp::guest`] runs any operand).
pub(super) fn local_ray_with(
    fp: &mut Fp,
    matrix: PlacementMatrix,
    one: u32,
    start: [u32; 3],
    delta: [u32; 3],
) -> ([u32; 3], [u32; 3]) {
    let inv = matrix.inverse_with(fp, one).words();
    let mut local = |mut v: [u32; 3], point: bool| {
        // `ucomiss scale, one; lahf; test ah, 44h; jnp`: only an ordered
        // equal skips the scale.
        if fp.ucomiss(inv[0], one) != Some(std::cmp::Ordering::Equal) {
            v = v.map(|n| fp.mul(n, inv[0]));
        }
        std::array::from_fn(|i| {
            let x = fp.mul(inv[1 + i], v[0]);
            let y = fp.mul(inv[4 + i], v[1]);
            let z = fp.mul(inv[7 + i], v[2]);
            let n = if i == 0 {
                let yx = fp.add(y, x);
                fp.add(yx, z)
            } else {
                let zy = fp.add(z, y);
                fp.add(zy, x)
            };
            if point { fp.add(n, inv[10 + i]) } else { n }
        })
    };
    (local(start, true), local(delta, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_axis_keeps_its_distinct_rounding_order() {
        let mut words = [1.0f32.to_bits(); 13];
        words[10..].fill(0);
        let matrix = PlacementMatrix::from_words(words);
        // Adding one to -2^26 loses it; the cancelling pair recovers it
        // only on X, where the two large terms are added first.
        let v = [67108864., -67108864., 1.0].map(f32::to_bits);
        let (start, delta) = local_ray(matrix, words[0], v, v).unwrap();
        assert_eq!(start, [words[0], 0, 0]);
        assert_eq!(delta, start);
    }
}
