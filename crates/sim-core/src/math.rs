//! Bit-level math types. Values are kept as binary32 bit patterns so a
//! kernel's arithmetic always goes through [`crate::det::Fp`], and so NaN
//! payloads and signed zeros round-trip through guest memory unchanged.

/// A quaternion `(i, j, k, w)`, lane 0 first, as the guest stores it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Quat(pub [u32; 4]);

/// Three binary32 values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Vec3(pub [u32; 3]);

/// The guest's `real_matrix4x3`, 0x34 bytes (docs/graphics/native-renderer.md):
/// scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct NodeMatrix {
    pub scale: u32,
    pub forward: Vec3,
    pub left: Vec3,
    pub up: Vec3,
    pub position: Vec3,
}

const _: () = assert!(std::mem::size_of::<NodeMatrix>() == 0x34);

impl Quat {
    pub fn from_f32(v: [f32; 4]) -> Self {
        Self(v.map(f32::to_bits))
    }
    pub fn to_f32(self) -> [f32; 4] {
        self.0.map(f32::from_bits)
    }
}

impl Vec3 {
    pub fn from_f32(v: [f32; 3]) -> Self {
        Self(v.map(f32::to_bits))
    }
    pub fn to_f32(self) -> [f32; 3] {
        self.0.map(f32::from_bits)
    }
}

impl NodeMatrix {
    /// The record as the guest lays it out, little-endian.
    pub fn to_bytes(&self) -> [u8; 0x34] {
        let mut out = [0; 0x34];
        let words = std::iter::once(self.scale)
            .chain(self.forward.0)
            .chain(self.left.0)
            .chain(self.up.0)
            .chain(self.position.0);
        for (chunk, w) in out.chunks_exact_mut(4).zip(words) {
            chunk.copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8; 0x34]) -> Self {
        let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
        let v = |i: usize| Vec3([w(i), w(i + 1), w(i + 2)]);
        Self {
            scale: w(0),
            forward: v(1),
            left: v(4),
            up: v(7),
            position: v(10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_matrix_round_trips_in_guest_layout() {
        let m = NodeMatrix {
            scale: 1,
            forward: Vec3([2, 3, 4]),
            left: Vec3([5, 6, 7]),
            up: Vec3([8, 9, 10]),
            position: Vec3([11, 12, 13]),
        };
        let b = m.to_bytes();
        assert_eq!(&b[0x28..0x2c], &11u32.to_le_bytes());
        assert_eq!(&b[0x04..0x08], &2u32.to_le_bytes());
        assert_eq!(NodeMatrix::from_bytes(&b), m);
    }
}
