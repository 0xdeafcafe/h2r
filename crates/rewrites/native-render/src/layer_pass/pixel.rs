//! The animated pixel-constant writer at `0x000176a0`.

use tags::material::{engine_scalar, ResolvedImplementation, BYTE_SCALE, CHANNEL_MASKS};

/// Merge the animation after the instance and engine colour writes. The
/// fragment selector and offset have already been resolved in the material.
pub(super) fn apply(
    imp: &mut ResolvedImplementation,
    register: (bool, u8),
    b2: u8,
    scalar: f32,
    rgb: [f32; 3],
) -> Option<()> {
    let (mask, packed) = lanes(b2, scalar, rgb);
    imp.write_pixel_constant(register, mask, packed).ok()
}

fn lanes(b2: u8, scalar: f32, rgb: [f32; 3]) -> (u32, u32) {
    let mode = usize::from(b2 & 7);
    // 0x00131e50 multiplies in x87 precision and converts with FISTP.
    let byte = |v: f32| (f64::from(v.clamp(0.0, 1.0)) * f64::from(BYTE_SCALE)).round_ties_even() as u32;
    let packed = match mode {
        0..=3 => {
            let transform = usize::from((b2 >> 3) & 7);
            let v = byte(engine_scalar(scalar, transform));
            v << [16, 8, 0, 24][mode]
        }
        4 => (byte(rgb[0]) << 16) | (byte(rgb[1]) << 8) | byte(rgb[2]),
        // 0x1b6fe writes scalar modes beyond A past the temporary RGBA
        // vector. Its four zeroed channels are what 0x1773d reads back.
        _ => 0,
    };
    (CHANNEL_MASKS[mode], packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scalar_changes_just_its_channel_after_the_transform() {
        let original = 0x6d31_5ed4;
        let merged = |b2, value| {
            let (mask, packed) = lanes(b2, value, [0.0; 3]);
            (original & !mask) | (packed & mask)
        };
        assert_eq!(merged(0, 1.5), 0x6dff_5ed4);
        assert_eq!(merged(1 | (1 << 3), 0.75), 0x6d31_40d4);
        assert_eq!(merged(2 | (3 << 3), 0.5), 0x6d31_5eff);
        assert_eq!(merged(3, -0.2), 0x0031_5ed4);
        assert_eq!(merged(3, 0.5), 0x8031_5ed4);
        assert_eq!(merged(5, 1.0), 0x6d00_5ed4);
    }

    #[test]
    fn rgb_uses_the_palette_without_the_scalar_transform() {
        assert_eq!(
            lanes(4 | (1 << 3), 0.75, [0.25, 0.5, 0.75]),
            (0x00ff_ffff, 0x0040_80bf)
        );
    }
}
