//! Xbox packed resources (`XPR0`) holding one texture: what an XBE's
//! `$$XTIMAGE` and `$$XSIMAGE` sections hold, and what `TitleImage.xbx` and
//! `SaveImage.xbx` are on the hard disk (docs/runtime/storage.md). The
//! original dashboard drew a game's title image and its saves' save image
//! from these.
//!
//! Layout (XDK `XPR_HEADER`, then `D3DTexture` records):
//!
//! ```text
//! +0  'XPR0'
//! +4  u32 total size (header + data)
//! +8  u32 header size: where the data starts
//! +12 resource: u32 Common, u32 Data (offset from the data start),
//!     u32 Lock, u32 Format, u32 Size   (gfx::TextureDesc::from_header)
//!     ... more resources, ended by 0xFFFFFFFF
//! ```
//!
//! Only the first resource is read, and only when it is a texture (the
//! `Common` word's type bits, 0x00070000, are 4). Title and save images are
//! a single 2D texture: 128x128 and 64x64 DXT1 on the retail disc.

use crate::bitmap::{decode_bytes, invalid};
use crate::{Level, Result};
use gfx::texture::{TextureDesc, TextureKind};

pub const MAGIC: &[u8; 4] = b"XPR0";
/// `D3DCOMMON_TYPE_MASK` and its texture value.
const TYPE_MASK: u32 = 0x0007_0000;
const TYPE_TEXTURE: u32 = 0x0004_0000;
/// Largest side we accept: title images are 128, save images 64.
const MAX_SIDE: u32 = 1024;

/// The first texture of an `XPR0` resource and its pixel bytes.
pub fn parse(bytes: &[u8]) -> Result<(TextureDesc, &[u8])> {
    let word = |o: usize| -> Result<u32> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| invalid(format!("XPR truncated at {o:#x}")))
    };
    if bytes.get(..4) != Some(MAGIC) {
        return Err(invalid("not an XPR0 resource"));
    }
    let total = (word(4)? as usize).min(bytes.len());
    let header = word(8)? as usize;
    if header < 32 || header > total {
        return Err(invalid(format!("XPR header size {header:#x} out of range")));
    }
    let common = word(12)?;
    if common == u32::MAX || common & TYPE_MASK != TYPE_TEXTURE {
        return Err(invalid("XPR's first resource is not a texture"));
    }
    let desc = TextureDesc::from_header(word(24)?, word(28)?)?;
    if desc.kind != TextureKind::D2 || desc.width > MAX_SIDE || desc.height > MAX_SIDE {
        return Err(invalid("XPR texture is not a small 2D image"));
    }
    let start = header
        .checked_add(word(16)? as usize)
        .filter(|s| *s <= total)
        .ok_or_else(|| invalid("XPR texture data out of range"))?;
    Ok((desc, &bytes[start..total]))
}

/// Decodes the first texture of an `XPR0` resource to RGBA8 (its base level).
pub fn decode(bytes: &[u8]) -> Result<Level> {
    let (desc, data) = parse(bytes)?;
    decode_bytes(&desc, data)?
        .into_iter()
        .find(|l| l.level == 0 && l.face == 0)
        .ok_or_else(|| invalid("XPR texture has no base level"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An XPR0 with one `width`x`height` DXT1 texture of `blocks` (8 bytes
    /// each, repeated to fill it), data at 0x800 as the disc's are.
    fn xpr(log_w: u32, log_h: u32, block: [u8; 8]) -> Vec<u8> {
        let (w, h) = (1u32 << log_w, 1u32 << log_h);
        let header = 0x800usize;
        let data = (w.div_ceil(4) * h.div_ceil(4) * 8) as usize;
        let mut out = vec![0u8; header + data];
        out[..4].copy_from_slice(MAGIC);
        out[4..8].copy_from_slice(&((header + data) as u32).to_le_bytes());
        out[8..12].copy_from_slice(&(header as u32).to_le_bytes());
        let format = (log_h << 24) | (log_w << 20) | (1 << 16) | (0x0c << 8) | (2 << 4) | 1;
        for (i, v) in [0x0004_0001u32, 0, 0, format, 0, u32::MAX]
            .iter()
            .enumerate()
        {
            out[12 + i * 4..16 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        for c in out[header..].chunks_mut(8) {
            c.copy_from_slice(&block);
        }
        out
    }

    #[test]
    fn decodes_a_dxt1_title_image() {
        // Colour 0 pure red (565 0xF800), every texel index 0.
        let l = decode(&xpr(7, 7, [0x00, 0xF8, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!((l.width, l.height), (128, 128));
        assert_eq!(l.rgba.len(), 128 * 128 * 4);
        assert_eq!(&l.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&l.rgba[l.rgba.len() - 4..], &[255, 0, 0, 255]);
    }

    #[test]
    fn decodes_a_64_pixel_save_image() {
        // Colour 0 pure blue (565 0x001F).
        let l = decode(&xpr(6, 6, [0x1F, 0x00, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!((l.width, l.height), (64, 64));
        assert_eq!(&l.rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn refuses_what_is_not_a_texture_xpr() {
        assert!(decode(b"XPR1").is_err());
        assert!(decode(&[]).is_err());
        let mut x = xpr(6, 6, [0; 8]);
        x[12..16].copy_from_slice(&0x0001_0001u32.to_le_bytes()); // a vertex buffer
        assert!(decode(&x).is_err());
        let mut x = xpr(6, 6, [0; 8]);
        x[8..12].copy_from_slice(&0x10_0000u32.to_le_bytes()); // header past the end
        assert!(decode(&x).is_err());
        let mut x = xpr(6, 6, [0; 8]);
        x.truncate(0x800 + 16); // data cut short
        assert!(decode(&x).is_err());
    }
}
