//! The game's glyph cache, so text can be redrawn sharp at a render scale
//! above 1 (`docs/fonts.md`, "Glyph cache texture").
//!
//! Halo 2 draws all text from one texture: a 256x256 `LIN_A4R4G4B4`
//! texture of one level that it fills with decoded glyphs as strings need
//! them (`0x1f5c0`, shelf packing with a one-texel clear border) and
//! samples with screen-aligned quads. Its bitmap is made once at
//! `0x1eff0` (format 9, linear) and kept at [`GLYPH_CACHE_BITMAP`]; the
//! bitmap's D3D texture is the pointer at [`BITMAP_TEXTURE`] in it.
//!
//! A stage is the glyph cache when its texture has that shape **and** is
//! the texture that pointer chain names now. The shape alone is cheap and
//! rules out nearly every other texture before any guest read; the chain
//! makes a misdetection need another texture at the very same header.

use crate::GuestMemory;
use crate::backend::TextureInfo;
use gfx::D3DFormat;
use gfx::texture::TextureKind;

/// The global holding the glyph cache's bitmap (written at `0x1f02e`).
pub const GLYPH_CACHE_BITMAP: u32 = 0x004B_62AC;
/// Offset of the D3D texture pointer in a bitmap (`0x1d030`, `0x1cfc2`).
pub const BITMAP_TEXTURE: u32 = 0x50;
/// Glyph cache size in texels (both axes; `0x1eff0` passes 256 and 256).
pub const SIZE: u32 = 256;

/// Whether the texture `info` (at `info.header`) is the glyph cache.
pub fn is_glyph_cache(mem: &dyn GuestMemory, info: &TextureInfo) -> bool {
    if !has_glyph_cache_shape(info) {
        return false;
    }
    let Ok(bitmap) = mem.read_u32(GLYPH_CACHE_BITMAP) else {
        return false;
    };
    if bitmap == 0 {
        return false;
    }
    matches!(mem.read_u32(bitmap.wrapping_add(BITMAP_TEXTURE)), Ok(t) if t == info.header)
}

/// The glyph cache's shape: 2D, `LIN_A4R4G4B4`, 256x256, one level.
pub fn has_glyph_cache_shape(info: &TextureInfo) -> bool {
    info.kind == TextureKind::D2
        && info.format == D3DFormat::LinA4R4G4B4
        && info.width == SIZE
        && info.height == SIZE
        && info.levels == 1
}

/// Glyph smoothing is on unless `H2_SMOOTH_TEXT` is `0`, `off` or `false`.
pub fn smoothing_from_env() -> bool {
    smoothing_from(std::env::var("H2_SMOOTH_TEXT").ok().as_deref())
}

/// [`smoothing_from_env`] for a given value.
pub fn smoothing_from(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim),
        Some("0") | Some("off") | Some("false")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SparseMemory;

    fn info(header: u32) -> TextureInfo {
        TextureInfo {
            header,
            palette: 0,
            format: D3DFormat::LinA4R4G4B4,
            kind: TextureKind::D2,
            width: 256,
            height: 256,
            depth: 1,
            levels: 1,
        }
    }

    fn memory(bitmap: u32, texture: u32) -> SparseMemory {
        let mut m = SparseMemory::new();
        m.map(GLYPH_CACHE_BITMAP & !0xFFF, 0x1000);
        m.map(0x0060_0000, 0x1000);
        m.write_u32(GLYPH_CACHE_BITMAP, bitmap).unwrap();
        m.write_u32(0x0060_0000 + BITMAP_TEXTURE, texture).unwrap();
        m
    }

    #[test]
    fn the_texture_the_bitmap_names_is_the_glyph_cache() {
        let m = memory(0x0060_0000, 0x0070_0000);
        assert!(is_glyph_cache(&m, &info(0x0070_0000)));
        assert!(!is_glyph_cache(&m, &info(0x0070_0100)), "another header");
    }

    #[test]
    fn the_shape_must_match_too() {
        let m = memory(0x0060_0000, 0x0070_0000);
        let mut i = info(0x0070_0000);
        i.format = D3DFormat::A4R4G4B4;
        assert!(!is_glyph_cache(&m, &i), "swizzled");
        let mut i = info(0x0070_0000);
        i.width = 128;
        assert!(!is_glyph_cache(&m, &i), "size");
        let mut i = info(0x0070_0000);
        i.levels = 2;
        assert!(!is_glyph_cache(&m, &i), "mips");
    }

    #[test]
    fn no_bitmap_or_unmapped_memory_is_not_the_glyph_cache() {
        let m = memory(0, 0x0070_0000);
        assert!(!is_glyph_cache(&m, &info(0x0070_0000)));
        assert!(!is_glyph_cache(&SparseMemory::new(), &info(0x0070_0000)));
    }

    #[test]
    fn the_switch() {
        assert!(smoothing_from(None));
        assert!(smoothing_from(Some("1")));
        for off in ["0", "off", "false", " 0 "] {
            assert!(!smoothing_from(Some(off)), "{off}");
        }
    }
}
