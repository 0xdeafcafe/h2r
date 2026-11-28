//! Shared dynamic decal types. Resources come from typed scene values;
//! this module has no guest-memory decoder.

/// Bytes in one guest decal vertex: float3 position and a packed 11:11:10
/// signed normalised vector (`NORMPACKED3`, semantic 3).
pub const VERTEX_SIZE: usize = 16;

/// The guest's dynamic decal pass mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Mode {
    Alpha0 = 0,
    Alpha1 = 1,
    Multiply = 2,
    Darken = 3,
    Maximum = 4,
    Alpha5 = 5,
    Opaque = 6,
}

impl Mode {
    pub const COUNT: usize = 7;

    pub fn from_guest(v: i16) -> Option<Self> {
        Some(match v {
            0 => Self::Alpha0,
            1 => Self::Alpha1,
            2 => Self::Multiply,
            3 => Self::Darken,
            4 => Self::Maximum,
            5 => Self::Alpha5,
            6 => Self::Opaque,
            _ => return None,
        })
    }

    pub fn index(self) -> i16 {
        self as i16
    }
}

/// One vertex as the native pass uploads it. The guest's second field is
/// `NORMPACKED3` ([`decode_packed`]); the wgpu attribute reads it as
/// `Float32x3`, which supplies the fetch's fixed `w = 1` itself.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Packed field decoded to floats: 11:11:10 signed normalised `(x, y, z)`.
    pub diffuse: [f32; 3],
}

/// The packed field as the NV2A fetches a `NORMPACKED3` attribute: 11:11:10 signed
/// normalised in one dword (`gfx::repack` and the D3D translation decode
/// the same dword the same way).
pub fn decode_packed(packed: [u8; 4]) -> [f32; 3] {
    let w = u32::from_le_bytes(packed);
    let signed = |v: u32, bits: u32| {
        let shift = 32 - bits;
        (((v << shift) as i32) >> shift) as f32 / ((1i32 << (bits - 1)) - 1) as f32
    };
    [
        signed(w & 0x7ff, 11).max(-1.0),
        signed((w >> 11) & 0x7ff, 11).max(-1.0),
        signed(w >> 22, 10).max(-1.0),
    ]
}

/// Triangle indices for a quad list, as the D3D path expands type 8.
pub fn quad_indices(quads: u32) -> Vec<u32> {
    (0..quads)
        .flat_map(|q| {
            let b = q * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_list_expansion_matches_d3d() {
        assert_eq!(quad_indices(2), [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn packed_decode_is_11_11_10_signed_normalised() {
        assert_eq!(decode_packed(0u32.to_le_bytes()), [0.0, 0.0, 0.0]);
        // The largest positive pattern of each field is exactly 1.
        let max: u32 = 0x3ff | (0x3ff << 11) | (0x1ff << 22);
        assert_eq!(decode_packed(max.to_le_bytes()), [1.0, 1.0, 1.0]);
        // All bits set: each field's two's complement is -1 LSB.
        let [x, y, z] = decode_packed(u32::MAX.to_le_bytes());
        assert_eq!([x, y], [-1.0 / 1023.0; 2]);
        assert_eq!(z, -1.0 / 511.0);
        // The most negative pattern of each field clamps to -1.
        let min: u32 = 0x400 | (0x400 << 11) | (0x200 << 22);
        assert_eq!(decode_packed(min.to_le_bytes()), [-1.0, -1.0, -1.0]);
        // Half in each field, and one field independent of the others.
        let half: u32 = 0x200 | (0x200 << 11) | (0x100 << 22);
        let [x, y, z] = decode_packed(half.to_le_bytes());
        assert_eq!([x, y], [512.0 / 1023.0; 2]);
        assert_eq!(z, 256.0 / 511.0);
        let [x, y, z] = decode_packed(1u32.to_le_bytes());
        assert_eq!([x, y, z], [1.0 / 1023.0, 0.0, 0.0]);
    }
}
