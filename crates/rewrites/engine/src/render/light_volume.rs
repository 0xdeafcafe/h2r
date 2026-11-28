//! The posed light volume built by `0x000c17f0` in its caller's scratch
//! storage. Read it after a successful call. The placement may have
//! overridden the definition's shape before the pose and scale were applied.

use super::{f32_at, u16_at, vec3_at};
use guest_layout::memory::{GuestMemory, MemFault};

/// The builders at `0x000c1930` and `0x000c1980` initialize 31 words.
pub const RECORD_SIZE: usize = 0x7c;

/// Shape-specific fields used by the visibility test and the projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Extent {
    Sphere {
        /// The larger of the two scaled definition or placement radii.
        radius: f32,
    },
    Projected {
        /// The effective nonzero shape code. A frustum has code 2.
        kind: u16,
        /// Direction from the light to the far centre per unit of axial depth.
        axis: [f32; 3],
        /// Cutoff distance after applying the light's scale.
        far_distance: f32,
        /// Horizontal and vertical extents evaluated at the cutoff.
        far_extent: [f32; 2],
        far_centre: [f32; 3],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightVolume {
    /// The projection's origin after the frustum's apex offset, where used.
    pub origin: [f32; 3],
    pub extent: Extent,
}

impl LightVolume {
    pub fn parse(bytes: &[u8; RECORD_SIZE]) -> Self {
        let kind = u16_at(bytes, 0);
        let extent = if kind == 0 {
            Extent::Sphere {
                radius: f32_at(bytes, 0x44),
            }
        } else {
            Extent::Projected {
                kind,
                axis: vec3_at(bytes, 0x3c),
                far_distance: f32_at(bytes, 0x64),
                far_extent: [f32_at(bytes, 0x68), f32_at(bytes, 0x6c)],
                far_centre: vec3_at(bytes, 0x70),
            }
        };
        Self {
            origin: vec3_at(bytes, 0x2c),
            extent,
        }
    }

    pub fn read(memory: &dyn GuestMemory, address: u32) -> Result<Self, MemFault> {
        let mut bytes = [0; RECORD_SIZE];
        memory.read(address, &mut bytes)?;
        Ok(Self::parse(&bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floats(bytes: &mut [u8], offset: usize, values: &[f32]) {
        for (field, value) in bytes[offset..].chunks_exact_mut(4).zip(values) {
            field.copy_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn sphere_uses_the_scaled_radius_in_the_overlapping_shape_fields() {
        let mut bytes = [0; RECORD_SIZE];
        floats(&mut bytes, 0x2c, &[7.0, 2.0, 1.0]);
        floats(&mut bytes, 0x3c, &[3.0, 9.0, 9.0]);
        floats(&mut bytes, 0x64, &[17.0, 18.0, 19.0]);
        assert_eq!(
            LightVolume::parse(&bytes),
            LightVolume {
                origin: [7.0, 2.0, 1.0],
                extent: Extent::Sphere { radius: 9.0 }
            }
        );
    }

    #[test]
    fn projected_shape_has_a_far_plane_and_preserves_its_kind() {
        for kind in [1u16, 2, 3] {
            let mut bytes = [0; RECORD_SIZE];
            bytes[..2].copy_from_slice(&kind.to_le_bytes());
            floats(&mut bytes, 0x2c, &[3.0, 1.0, 2.0]);
            floats(&mut bytes, 0x3c, &[0.0, 1.0, 0.0]);
            floats(&mut bytes, 0x64, &[12.0, 3.0, 4.0, 3.0, 13.0, 2.0]);
            assert_eq!(
                LightVolume::parse(&bytes),
                LightVolume {
                    origin: [3.0, 1.0, 2.0],
                    extent: Extent::Projected {
                        kind,
                        axis: [0.0, 1.0, 0.0],
                        far_distance: 12.0,
                        far_extent: [3.0, 4.0],
                        far_centre: [3.0, 13.0, 2.0],
                    },
                }
            );
        }
    }
}
