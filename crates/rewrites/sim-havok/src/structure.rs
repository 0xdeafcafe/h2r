//! The polygon that a structure child builds from a BSP surface's edge ring.
//! Keep vertex order and all four SSE lanes, including the feature ID in w.
use simd::{Xm, addss, comiss, mulss, ucomiss};

pub type Vertex = [u32; 4];

#[derive(Clone, Copy)]
pub struct Transform {
    pub scale: u32,
    pub basis: [[u32; 3]; 3],
    pub translation: [u32; 3],
}

fn mul(mxcsr: &mut u32, left: u32, right: u32) -> Result<u32, Xm> {
    mulss(mxcsr, left.into(), right).map(|v| v as u32)
}

fn add(mxcsr: &mut u32, left: u32, right: u32) -> Result<u32, Xm> {
    addss(mxcsr, left.into(), right).map(|v| v as u32)
}

impl Transform {
    fn point(&self, mut point: [u32; 3], one: u32, mxcsr: &mut u32) -> Result<[u32; 3], Xm> {
        // Skip scaling only on an ordered equal comparison.
        if ucomiss(mxcsr, self.scale, one)? != 0x40 {
            for p in &mut point {
                *p = mul(mxcsr, self.scale, *p)?;
            }
        }
        let mut out = [0; 3];
        for (i, coord) in out.iter_mut().enumerate() {
            let z = mul(mxcsr, self.basis[2][i], point[2])?;
            let y = mul(mxcsr, self.basis[1][i], point[1])?;
            let zy = add(mxcsr, z, y)?;
            let x = mul(mxcsr, self.basis[0][i], point[0])?;
            let zyx = add(mxcsr, zy, x)?;
            *coord = add(mxcsr, zyx, self.translation[i])?;
        }
        Ok(out)
    }
}

/// Source ring in the surface's winding, with the selected endpoint first.
pub fn polygon(
    ring: impl Iterator<Item = [u32; 3]>,
    transform: Option<&Transform>,
    one: u32,
    feature_bits: u32,
    mxcsr: &mut u32,
) -> Result<Vec<Vertex>, Xm> {
    ring.enumerate().map(|(index, p)| {
        let [x, y, z] = match transform {
            Some(t) => t.point(p, one, mxcsr)?,
            None => p,
        };
        Ok([x, y, z, feature_bits | index as u32])
    }).collect()
}

/// The fourth product is discarded by the reduction, but it still raises
/// exceptions. Strictly greater keeps the earliest vertex on ties.
pub fn support(
    vertices: &[Vertex],
    direction: Vertex,
    mut best: u32,
    mxcsr: &mut u32,
) -> Result<usize, Xm> {
    let mut selected = 0;
    for (i, vertex) in vertices.iter().enumerate() {
        let mut product = [0; 4];
        for axis in 0..4 {
            product[axis] = mul(mxcsr, vertex[axis], direction[axis])?;
        }
        let yx = add(mxcsr, product[1], product[0])?;
        let dot = add(mxcsr, product[2], yx)?;
        if comiss(mxcsr, dot, best)? == 0 {
            best = dot;
            selected = i;
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_uses_guest_axis_order() {
        let large = 1e20f32;
        let transform = Transform {
            scale: 1f32.to_bits(),
            basis: [[1., 1., 1.], [-large, -large, -large], [large; 3]].map(|v| v.map(f32::to_bits)),
            translation: [(-0.5f32).to_bits(); 3],
        };
        let mut env = simd::MXCSR_DEFAULT;
        let result = polygon([[1f32.to_bits(); 3]; 2].into_iter(), Some(&transform),
            1f32.to_bits(), 0x100, &mut env).unwrap();
        assert_eq!(result[0], [0.5f32.to_bits(), 0.5f32.to_bits(), 0.5f32.to_bits(), 0x100]);
        assert_eq!(result[1][3], 0x101);
    }

    #[test]
    fn support_preserves_first_tie_and_unused_lane_exceptions() {
        let vertices = [[1., 0., 0., 0.], [2., 2., 0., 3.], [2., -2., 0., 5.]]
            .map(|v| v.map(f32::to_bits));
        let mut env = simd::MXCSR_DEFAULT;
        let direction = [1., 0., 0., f32::INFINITY].map(f32::to_bits);
        assert_eq!(support(&vertices, direction, (-10f32).to_bits(), &mut env).unwrap(), 1);
        assert_ne!(env & simd::IE, 0);
    }
}
