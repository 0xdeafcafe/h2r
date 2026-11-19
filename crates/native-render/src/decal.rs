//! Shared dynamic decal types. Admission and resources come from the ordered
//! `d3d::DrawAdmission` snapshot; this module has no guest-memory decoder.

/// Bytes in one decal vertex: float3 position and packed D3DCOLOR.
pub const VERTEX_SIZE: usize = 16;

/// A dynamic decal pass mode. Modes 0..5 are W-buffered; mode 6 is not.
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

    /// Whether this mode's wrapper selects W depth.
    pub fn w_buffered(self) -> bool {
        self != Self::Opaque
    }
}

/// One vertex as the resolved declaration supplies it (`v0` and `v3`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Bytes in guest order (B, G, R, A for a D3DCOLOR).
    pub color: [u8; 4],
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
    fn only_mode_six_is_not_w_buffered() {
        for mode in [
            Mode::Alpha0,
            Mode::Alpha1,
            Mode::Multiply,
            Mode::Darken,
            Mode::Maximum,
            Mode::Alpha5,
        ] {
            assert!(mode.w_buffered());
        }
        assert!(!Mode::Opaque.w_buffered());
    }
}
