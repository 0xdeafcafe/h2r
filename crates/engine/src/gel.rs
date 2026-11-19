//! Numerical inputs of the shader-gel quad (`0x00015f30..0x00015fc1`).
//! The literals are read from the image by the caller. Arithmetic follows
//! scalar SSE under the guest MXCSR; an unmasked exception refuses it.

pub const NEAR: u32 = 0x0048_5ad4;
pub const FAR: u32 = 0x0048_5ad8;
pub const ONE: u32 = 0x0045_dbc0;
pub const DEPTH_SCALE: u32 = 0x0045_dd40;

pub fn depth(near: u32, far: u32, one: u32, scale: u32, mut mxcsr: u32) -> Option<[u32; 2]> {
    let range = simd::subss(&mut mxcsr, far as u128, near).ok()? as u32;
    let quotient = simd::divss(&mut mxcsr, far as u128, range).ok()? as u32;
    let offset = simd::mulss(&mut mxcsr, near as u128, quotient).ok()? as u32;
    let z = simd::subss(&mut mxcsr, quotient as u128, offset).ok()? as u32;
    let z = simd::mulss(&mut mxcsr, z as u128, scale).ok()? as u32;
    let rhw = simd::divss(&mut mxcsr, one as u128, far).ok()? as u32;
    let rhw = simd::mulss(&mut mxcsr, rhw as u128, scale).ok()? as u32;
    let mut clamp = |x: u32| {
        // COMISS 0,x; JBE keeps x (including unordered). Then
        // COMISS x,scale; JBE keeps x (also unordered).
        let below_zero = simd::comiss(&mut mxcsr, 0, x).ok()? & 0x41 == 0;
        if below_zero {
            return Some(0);
        }
        let above_scale = simd::comiss(&mut mxcsr, x, scale).ok()? & 0x41 == 0;
        Some(if above_scale { scale } else { x })
    };
    Some([clamp(z)?, clamp(rhw)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_depth_follows_the_sse_operation_order() {
        let d = depth(
            0.25f32.to_bits(),
            4f32.to_bits(),
            1f32.to_bits(),
            1f32.to_bits(),
            0x1f80,
        )
        .unwrap();
        let q = 4f32 / (4f32 - 0.25f32);
        assert_eq!(d, [(q - 0.25f32 * q).to_bits(), 0.25f32.to_bits()]);
    }

    #[test]
    fn clamp_and_unmasked_divide_by_zero() {
        let bits = |x: f32| x.to_bits();
        assert_eq!(
            depth(bits(2.0), bits(4.0), bits(1.0), bits(1.0), 0x1f80).unwrap()[0],
            0
        );
        assert_eq!(
            depth(bits(0.0), bits(0.25), bits(1.0), bits(1.0), 0x1f80).unwrap()[1],
            bits(1.0)
        );
        assert!(depth(bits(1.0), bits(1.0), bits(1.0), bits(1.0), 0x1d80).is_none());
    }
}
