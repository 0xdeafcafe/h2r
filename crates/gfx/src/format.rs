//! The Xbox `D3DFORMAT` enumeration and the properties we derive from it.
//!
//! The Xbox values are **not** the PC Direct3D 8 values: they are the NV2A
//! texture colour-format codes (the field the GPU reads straight out of the
//! texture header), with the "linear" (`LIN_*`) variants occupying the
//! `0x10..` range and a handful of extras for depth and bump formats.
//!
//! Sources (described here, not copied; both projects are GPL):
//!
//! * Cxbx-Reloaded `src/core/hle/D3D8/XbD3D8Types.h`, `X_D3DFMT_*` and the
//!   `X_D3DFORMAT_*` header masks, commit
//!   `585c49a50af1255ab155099e06f24505f9c5a800`.
//! * xemu `hw/xbox/nv2a/nv2a_regs.h` (`NV097_SET_TEXTURE_FORMAT_COLOR_*`) and
//!   `hw/xbox/nv2a/pgraph/gl/constants.h` (per-format channel swizzles),
//!   commit `4bdad199102b22ed0cfa998e9af6b35d76238b3e`.
//!
//! Naming convention: a channel list such as `A8R8G8B8` reads from the most
//! significant channel to the least significant one of the little-endian
//! texel value. `A8R8G8B8` is therefore stored in memory as bytes `B, G, R, A`.

/// An Xbox `D3DFORMAT` value as found in bits 8..16 of a pixel container's
/// `Format` word.
///
/// Only formats that appear in texture/surface headers are listed. Aliases
/// (`DXT2 == DXT3`, `DXT4 == DXT5`, `V8U8 == G8B8`, ...) share one variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
#[allow(missing_docs)] // the names are the documentation: they are the XDK names
pub enum D3DFormat {
    // --- Swizzled colour formats -------------------------------------------
    L8 = 0x00,
    Al8 = 0x01,
    A1R5G5B5 = 0x02,
    X1R5G5B5 = 0x03,
    A4R4G4B4 = 0x04,
    R5G6B5 = 0x05,
    A8R8G8B8 = 0x06,
    /// Also `X8L8V8U8`.
    X8R8G8B8 = 0x07,
    /// 8-bit palettised; the palette is a separate resource.
    P8 = 0x0B,
    // --- Block compressed (stored as linear rows of 4x4 blocks) ------------
    Dxt1 = 0x0C,
    /// Also `DXT2` (premultiplied alpha is a sampler-side concern).
    Dxt3 = 0x0E,
    /// Also `DXT4`.
    Dxt5 = 0x0F,
    // --- Linear colour formats ----------------------------------------------
    LinA1R5G5B5 = 0x10,
    LinR5G6B5 = 0x11,
    LinA8R8G8B8 = 0x12,
    LinL8 = 0x13,
    LinR8B8 = 0x16,
    /// Also `LIN_V8U8`.
    LinG8B8 = 0x17,
    // --- More swizzled ---------------------------------------------------------
    A8 = 0x19,
    A8L8 = 0x1A,
    LinAl8 = 0x1B,
    LinX1R5G5B5 = 0x1C,
    LinA4R4G4B4 = 0x1D,
    LinX8R8G8B8 = 0x1E,
    LinA8 = 0x1F,
    LinA8L8 = 0x20,
    // --- Packed YUV (always linear) -----------------------------------------
    /// Bytes `Y0 U Y1 V`.
    Yuy2 = 0x24,
    /// Bytes `U Y0 V Y1`.
    Uyvy = 0x25,
    // --- More swizzled ---------------------------------------------------------
    /// Also `L6V5U5`.
    R6G5B5 = 0x27,
    /// Also `V8U8`.
    G8B8 = 0x28,
    R8B8 = 0x29,
    // --- Depth ---------------------------------------------------------------
    D24S8 = 0x2A,
    F24S8 = 0x2B,
    /// Also `D16_LOCKABLE`.
    D16 = 0x2C,
    F16 = 0x2D,
    LinD24S8 = 0x2E,
    LinF24S8 = 0x2F,
    LinD16 = 0x30,
    LinF16 = 0x31,
    // --- 16-bit single channel ----------------------------------------------
    L16 = 0x32,
    V16U16 = 0x33,
    LinL16 = 0x35,
    LinV16U16 = 0x36,
    LinR6G5B5 = 0x37,
    // --- Swizzled 4-channel permutations -------------------------------------
    R5G5B5A1 = 0x38,
    R4G4B4A4 = 0x39,
    /// Also `Q8W8V8U8`.
    A8B8G8R8 = 0x3A,
    B8G8R8A8 = 0x3B,
    R8G8B8A8 = 0x3C,
    // --- Linear 4-channel permutations ---------------------------------------
    LinR5G5B5A1 = 0x3D,
    LinR4G4B4A4 = 0x3E,
    LinA8B8G8R8 = 0x3F,
    LinB8G8R8A8 = 0x40,
    LinR8G8B8A8 = 0x41,
}

/// How texels of a format are laid out in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Morton-order ("swizzled"): power-of-two dimensions, mip chains, cube
    /// maps and volumes allowed. The GPU's preferred layout.
    Swizzled,
    /// Row-major with a pitch; one level, no cube/volume.
    Linear,
    /// 4x4 block compressed. Blocks are stored in row-major block order (no
    /// Morton), mip chains/cube maps allowed.
    Compressed,
}

/// The channel interpretation of a texel, independent of [`Layout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum TexelFormat {
    L8,
    Al8,
    A8,
    A8L8,
    A1R5G5B5,
    X1R5G5B5,
    A4R4G4B4,
    R5G6B5,
    R6G5B5,
    R5G5B5A1,
    R4G4B4A4,
    A8R8G8B8,
    X8R8G8B8,
    A8B8G8R8,
    B8G8R8A8,
    R8G8B8A8,
    G8B8,
    R8B8,
    L16,
    P8,
    Yuy2,
    Uyvy,
    Dxt1,
    Dxt3,
    Dxt5,
    /// Depth/stencil and signed-bump formats: valid in headers but not
    /// uploadable as colour textures by this crate.
    Unsupported(D3DFormat),
}

impl D3DFormat {
    /// Decodes the raw 8-bit format field. Returns `None` for values that are
    /// not Xbox texture formats (including the PC-only ones).
    pub fn from_raw(v: u32) -> Option<Self> {
        use D3DFormat::*;
        Some(match v {
            0x00 => L8,
            0x01 => Al8,
            0x02 => A1R5G5B5,
            0x03 => X1R5G5B5,
            0x04 => A4R4G4B4,
            0x05 => R5G6B5,
            0x06 => A8R8G8B8,
            0x07 => X8R8G8B8,
            0x0B => P8,
            0x0C => Dxt1,
            0x0E => Dxt3,
            0x0F => Dxt5,
            0x10 => LinA1R5G5B5,
            0x11 => LinR5G6B5,
            0x12 => LinA8R8G8B8,
            0x13 => LinL8,
            0x16 => LinR8B8,
            0x17 => LinG8B8,
            0x19 => A8,
            0x1A => A8L8,
            0x1B => LinAl8,
            0x1C => LinX1R5G5B5,
            0x1D => LinA4R4G4B4,
            0x1E => LinX8R8G8B8,
            0x1F => LinA8,
            0x20 => LinA8L8,
            0x24 => Yuy2,
            0x25 => Uyvy,
            0x27 => R6G5B5,
            0x28 => G8B8,
            0x29 => R8B8,
            0x2A => D24S8,
            0x2B => F24S8,
            0x2C => D16,
            0x2D => F16,
            0x2E => LinD24S8,
            0x2F => LinF24S8,
            0x30 => LinD16,
            0x31 => LinF16,
            0x32 => L16,
            0x33 => V16U16,
            0x35 => LinL16,
            0x36 => LinV16U16,
            0x37 => LinR6G5B5,
            0x38 => R5G5B5A1,
            0x39 => R4G4B4A4,
            0x3A => A8B8G8R8,
            0x3B => B8G8R8A8,
            0x3C => R8G8B8A8,
            0x3D => LinR5G5B5A1,
            0x3E => LinR4G4B4A4,
            0x3F => LinA8B8G8R8,
            0x40 => LinB8G8R8A8,
            0x41 => LinR8G8B8A8,
            _ => return None,
        })
    }

    /// The raw Xbox `D3DFORMAT` value.
    pub fn raw(self) -> u32 {
        self as u32
    }

    /// Memory layout class.
    pub fn layout(self) -> Layout {
        use D3DFormat::*;
        match self {
            Dxt1 | Dxt3 | Dxt5 => Layout::Compressed,
            LinA1R5G5B5 | LinR5G6B5 | LinA8R8G8B8 | LinL8 | LinR8B8 | LinG8B8 | LinAl8
            | LinX1R5G5B5 | LinA4R4G4B4 | LinX8R8G8B8 | LinA8 | LinA8L8 | Yuy2 | Uyvy
            | LinD24S8 | LinF24S8 | LinD16 | LinF16 | LinL16 | LinV16U16 | LinR6G5B5
            | LinR5G5B5A1 | LinR4G4B4A4 | LinA8B8G8R8 | LinB8G8R8A8 | LinR8G8B8A8 => {
                Layout::Linear
            }
            _ => Layout::Swizzled,
        }
    }

    /// The channel interpretation, with swizzled/linear twins unified.
    pub fn texel(self) -> TexelFormat {
        use D3DFormat as F;
        use TexelFormat as T;
        match self {
            F::L8 | F::LinL8 => T::L8,
            F::Al8 | F::LinAl8 => T::Al8,
            F::A8 | F::LinA8 => T::A8,
            F::A8L8 | F::LinA8L8 => T::A8L8,
            F::A1R5G5B5 | F::LinA1R5G5B5 => T::A1R5G5B5,
            F::X1R5G5B5 | F::LinX1R5G5B5 => T::X1R5G5B5,
            F::A4R4G4B4 | F::LinA4R4G4B4 => T::A4R4G4B4,
            F::R5G6B5 | F::LinR5G6B5 => T::R5G6B5,
            F::R6G5B5 | F::LinR6G5B5 => T::R6G5B5,
            F::R5G5B5A1 | F::LinR5G5B5A1 => T::R5G5B5A1,
            F::R4G4B4A4 | F::LinR4G4B4A4 => T::R4G4B4A4,
            F::A8R8G8B8 | F::LinA8R8G8B8 => T::A8R8G8B8,
            F::X8R8G8B8 | F::LinX8R8G8B8 => T::X8R8G8B8,
            F::A8B8G8R8 | F::LinA8B8G8R8 => T::A8B8G8R8,
            F::B8G8R8A8 | F::LinB8G8R8A8 => T::B8G8R8A8,
            F::R8G8B8A8 | F::LinR8G8B8A8 => T::R8G8B8A8,
            F::G8B8 | F::LinG8B8 => T::G8B8,
            F::R8B8 | F::LinR8B8 => T::R8B8,
            F::L16 | F::LinL16 => T::L16,
            F::P8 => T::P8,
            F::Yuy2 => T::Yuy2,
            F::Uyvy => T::Uyvy,
            F::Dxt1 => T::Dxt1,
            F::Dxt3 => T::Dxt3,
            F::Dxt5 => T::Dxt5,
            F::D24S8 | F::F24S8 | F::D16 | F::F16 | F::LinD24S8 | F::LinF24S8 | F::LinD16
            | F::LinF16 | F::V16U16 | F::LinV16U16 => T::Unsupported(self),
        }
    }

    /// Whether the format is a depth/stencil surface format.
    pub fn is_depth(self) -> bool {
        use D3DFormat::*;
        matches!(self, D24S8 | F24S8 | D16 | F16 | LinD24S8 | LinF24S8 | LinD16 | LinF16)
    }

    /// Bytes per texel for uncompressed formats, `None` for block formats.
    pub fn bytes_per_texel(self) -> Option<u32> {
        self.texel().bytes_per_texel()
    }

    /// Bytes per 4x4 block for compressed formats.
    pub fn block_bytes(self) -> Option<u32> {
        match self.texel() {
            TexelFormat::Dxt1 => Some(8),
            TexelFormat::Dxt3 | TexelFormat::Dxt5 => Some(16),
            _ => None,
        }
    }
}

impl TexelFormat {
    /// Bytes per texel for uncompressed formats (`None` for DXT).
    pub fn bytes_per_texel(self) -> Option<u32> {
        use TexelFormat::*;
        match self {
            L8 | Al8 | A8 | P8 => Some(1),
            A8L8 | A1R5G5B5 | X1R5G5B5 | A4R4G4B4 | R5G6B5 | R6G5B5 | R5G5B5A1 | R4G4B4A4
            | G8B8 | R8B8 | L16 | Yuy2 | Uyvy => Some(2),
            A8R8G8B8 | X8R8G8B8 | A8B8G8R8 | B8G8R8A8 | R8G8B8A8 => Some(4),
            Dxt1 | Dxt3 | Dxt5 => None,
            Unsupported(f) => match f {
                D3DFormat::D16
                | D3DFormat::F16
                | D3DFormat::LinD16
                | D3DFormat::LinF16 => Some(2),
                _ => Some(4),
            },
        }
    }
}

/// The format a decoded texture is handed to the GPU in.
///
/// This is deliberately a small closed set: everything that has no exact
/// wgpu equivalent is expanded to [`HostFormat::Rgba8Unorm`] on the CPU, with
/// the Xbox channel semantics (e.g. `L8` samples as `(l, l, l, 1)`) baked in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostFormat {
    /// 8-bit RGBA, bytes `R, G, B, A`.
    Rgba8Unorm,
    /// 8-bit BGRA, bytes `B, G, R, A`. Xbox `A8R8G8B8` passes through as this.
    Bgra8Unorm,
    /// BC1 (DXT1). Needs `Features::TEXTURE_COMPRESSION_BC`.
    Bc1RgbaUnorm,
    /// BC2 (DXT3).
    Bc2RgbaUnorm,
    /// BC3 (DXT5).
    Bc3RgbaUnorm,
}

impl HostFormat {
    /// The matching wgpu format.
    pub fn to_wgpu(self) -> wgpu::TextureFormat {
        match self {
            HostFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            HostFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            HostFormat::Bc1RgbaUnorm => wgpu::TextureFormat::Bc1RgbaUnorm,
            HostFormat::Bc2RgbaUnorm => wgpu::TextureFormat::Bc2RgbaUnorm,
            HostFormat::Bc3RgbaUnorm => wgpu::TextureFormat::Bc3RgbaUnorm,
        }
    }

    /// Whether this is a 4x4 block-compressed format.
    pub fn is_compressed(self) -> bool {
        matches!(self, HostFormat::Bc1RgbaUnorm | HostFormat::Bc2RgbaUnorm | HostFormat::Bc3RgbaUnorm)
    }

    /// Bytes per texel (uncompressed) or per 4x4 block (compressed).
    pub fn bytes_per_unit(self) -> u32 {
        match self {
            HostFormat::Rgba8Unorm | HostFormat::Bgra8Unorm => 4,
            HostFormat::Bc1RgbaUnorm => 8,
            HostFormat::Bc2RgbaUnorm | HostFormat::Bc3RgbaUnorm => 16,
        }
    }
}

/// Depth buffer formats: the wgpu depth format for an Xbox depth `D3DFORMAT`.
///
/// `F16`/`F24S8` are floating-point-depth variants (an NV2A "w-buffer"-ish
/// format); wgpu has no 24-bit float depth, so they map to the nearest
/// fixed/float formats and the difference is documented in `docs/graphics/graphics.md`.
pub fn depth_format_to_wgpu(f: D3DFormat) -> Option<wgpu::TextureFormat> {
    use D3DFormat::*;
    match f {
        D16 | LinD16 | F16 | LinF16 => Some(wgpu::TextureFormat::Depth16Unorm),
        D24S8 | LinD24S8 | F24S8 | LinF24S8 => Some(wgpu::TextureFormat::Depth24PlusStencil8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values_match_cxbx_table() {
        // Spot-check values from XbD3D8Types.h (commit 585c49a).
        assert_eq!(D3DFormat::A8R8G8B8.raw(), 0x06);
        assert_eq!(D3DFormat::Dxt1.raw(), 0x0C);
        assert_eq!(D3DFormat::Dxt3.raw(), 0x0E);
        assert_eq!(D3DFormat::Dxt5.raw(), 0x0F);
        assert_eq!(D3DFormat::LinA8R8G8B8.raw(), 0x12);
        assert_eq!(D3DFormat::A8.raw(), 0x19);
        assert_eq!(D3DFormat::LinR8G8B8A8.raw(), 0x41);
    }

    #[test]
    fn from_raw_roundtrips_every_variant() {
        let mut seen = 0;
        for v in 0..=0xFFu32 {
            if let Some(f) = D3DFormat::from_raw(v) {
                assert_eq!(f.raw(), v);
                seen += 1;
            }
        }
        assert_eq!(seen, 54);
        assert!(D3DFormat::from_raw(0x0A).is_none());
        assert!(D3DFormat::from_raw(0x64).is_none()); // VERTEXDATA is not a texture format
    }

    #[test]
    fn layouts() {
        assert_eq!(D3DFormat::A8R8G8B8.layout(), Layout::Swizzled);
        assert_eq!(D3DFormat::LinA8R8G8B8.layout(), Layout::Linear);
        assert_eq!(D3DFormat::Dxt5.layout(), Layout::Compressed);
        assert_eq!(D3DFormat::Yuy2.layout(), Layout::Linear);
    }

    #[test]
    fn sizes() {
        assert_eq!(D3DFormat::R5G6B5.bytes_per_texel(), Some(2));
        assert_eq!(D3DFormat::LinX8R8G8B8.bytes_per_texel(), Some(4));
        assert_eq!(D3DFormat::L8.bytes_per_texel(), Some(1));
        assert_eq!(D3DFormat::Dxt1.bytes_per_texel(), None);
        assert_eq!(D3DFormat::Dxt1.block_bytes(), Some(8));
        assert_eq!(D3DFormat::Dxt5.block_bytes(), Some(16));
    }

    #[test]
    fn linear_twins_share_texel_format() {
        assert_eq!(D3DFormat::A1R5G5B5.texel(), D3DFormat::LinA1R5G5B5.texel());
        assert_eq!(D3DFormat::G8B8.texel(), D3DFormat::LinG8B8.texel());
    }

    #[test]
    fn depth_formats() {
        assert!(D3DFormat::D24S8.is_depth());
        assert_eq!(
            depth_format_to_wgpu(D3DFormat::D16),
            Some(wgpu::TextureFormat::Depth16Unorm)
        );
        assert_eq!(depth_format_to_wgpu(D3DFormat::A8R8G8B8), None);
    }
}
