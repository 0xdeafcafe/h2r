//! The cached support simplex, transformed from shape B to shape A.
//! A vertex's fourth input word is its feature ID. It never scales a
//! column, but the fourth lane of every column participates in the result.
use sim_core::det::{self, Fp};
use sim_core::lanes::Vec4;

pub const MAX_VERTICES: usize = 4;

#[derive(Debug, Default)]
pub struct TransformedSimplex {
    pub vertices: [Vec4; MAX_VERTICES],
    /// The final multiply and add intermediates are live at the next shape call.
    pub scaled_y: Vec4,
    pub translated_z: Vec4,
}

/// Computes the complete simplex before the adapter scatters any vertices.
/// The soft-float arm handles exceptional operands and directed rounding.
pub fn transform(
    columns: [Vec4; 4],
    vertices: &[Vec4],
    mxcsr: &mut u32,
) -> Option<TransformedSimplex> {
    if vertices.len() > MAX_VERTICES || *mxcsr & simd::MXCSR_DEFAULT != simd::MXCSR_DEFAULT {
        return None;
    }
    if det::fast_env(*mxcsr) && det::host_fast_env() {
        if let Some(result) = transform_fast(columns, vertices) {
            return Some(result);
        }
    }

    let [cx, cy, cz, translation] = columns.map(pack);
    let mut result = TransformedSimplex::default();
    for (dest, v) in result.vertices.iter_mut().zip(vertices) {
        let x = simd::mulps(mxcsr, cx, pack([v[0]; 4])).ok()?;
        let y = simd::mulps(mxcsr, cy, pack([v[1]; 4])).ok()?;
        let z = simd::mulps(mxcsr, cz, pack([v[2]; 4])).ok()?;
        let zt = simd::addps(mxcsr, z, translation).ok()?;
        let xy = simd::addps(mxcsr, x, y).ok()?;
        *dest = unpack(simd::addps(mxcsr, xy, zt).ok()?);
        result.scaled_y = unpack(y);
        result.translated_z = unpack(zt);
    }
    Some(result)
}

fn pack(v: Vec4) -> u128 {
    v.into_iter().rev().fold(0, |p, w| p << 32 | u128::from(w))
}

fn unpack(v: u128) -> Vec4 {
    std::array::from_fn(|i| (v >> (32 * i)) as u32)
}

fn transform_fast(columns: [Vec4; 4], vertices: &[Vec4]) -> Option<TransformedSimplex> {
    let mut result = TransformedSimplex::default();
    let mut fp = Fp::new();
    for (dest, v) in result.vertices.iter_mut().zip(vertices) {
        let x = fp.mul4(columns[0], [v[0]; 4]);
        let y = fp.mul4(columns[1], [v[1]; 4]);
        let z = fp.mul4(columns[2], [v[2]; 4]);
        let zt = fp.add4(z, columns[3]);
        let xy = fp.add4(x, y);
        *dest = fp.add4(xy, zt);
        result.scaled_y = y;
        result.translated_z = zt;
    }
    fp.exact().then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplex_matches_individual_sse_instructions() {
        let columns = [
            [0.4, -2.0, 1.0, -0.0], [-0.5, 0.0, 0.125, 0.75],
            [0.3, 1.125, -0.5, 0.25], [4.0, -5.0, 0.5, 0.0],
        ].map(|c| c.map(f32::to_bits));
        let mut seed = 3u32;
        for count in 1..=4 {
            let vertices = std::array::from_fn::<_, 4, _>(|_| {
                let mut v = std::array::from_fn(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed as i32 as f32 / (1u32 << 28) as f32).to_bits()
                });
                v[3] = seed; // opaque ID, including non-finite float patterns
                v
            });
            let initial = simd::MXCSR_DEFAULT | simd::PE;
            let mut status = initial;
            let result = transform(columns, &vertices[..count], &mut status).unwrap();
            let mut mxcsr = initial;
            for (i, v) in vertices[..count].iter().enumerate() {
                let x = simd::mulps(&mut mxcsr, pack(columns[0]), pack([v[0];4])).unwrap();
                let y = simd::mulps(&mut mxcsr, pack(columns[1]), pack([v[1];4])).unwrap();
                let z = simd::mulps(&mut mxcsr, pack(columns[2]), pack([v[2];4])).unwrap();
                let zt = simd::addps(&mut mxcsr, z, pack(columns[3])).unwrap();
                let xy = simd::addps(&mut mxcsr, x, y).unwrap();
                let actual = simd::addps(&mut mxcsr, xy, zt).unwrap();
                assert_eq!(pack(result.vertices[i]), actual);
                if i + 1 == count {
                    assert_eq!(pack(result.scaled_y), y);
                    assert_eq!(pack(result.translated_z), zt);
                }
            }
            assert_eq!(mxcsr, status);
        }
    }

    #[test]
    fn exceptional_operands_preserve_payload_and_record_sticky_exceptions() {
        let mut mxcsr = simd::MXCSR_DEFAULT | (1 << 13);
        let columns = [[1f32.to_bits(); 4]; 4];
        let nan = 0x7f80_1245;
        let result = transform(columns, &[[nan, 1, 0, 0]], &mut mxcsr).unwrap();
        assert_eq!(result.vertices[0], [nan | 0x0040_0000; 4]);
        assert_ne!(mxcsr & simd::IE, 0);
        assert_ne!(mxcsr & simd::DE, 0);
        assert_eq!(mxcsr >> 13, 1);
    }
}
