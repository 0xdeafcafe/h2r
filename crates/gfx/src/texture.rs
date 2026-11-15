//! Xbox texture descriptors, memory layout, and the decode pipeline
//! (guest bytes -> host-ready levels).
//!
//! The header bit layout is Cxbx-Reloaded's `X_D3DPixelContainer` description
//! (`XbD3D8Types.h` @ `585c49a50af1255ab155099e06f24505f9c5a800`, the
//! `X_D3DFORMAT_*` and `X_D3DSIZE_*` masks):
//!
//! ```text
//! Format word                          Size word (linear/non-pow2 only)
//!  31..28 log2 depth   (P)              31..24 pitch/64 - 1
//!  27..24 log2 height  (V)              23..12 height - 1
//!  23..20 log2 width   (U)              11..0  width - 1
//!  19..16 mip levels
//!  15..8  D3DFORMAT
//!   7..4  dimensions (2 or 3)
//!   2     cube map
//! ```
//!
//! Memory layout rules (from xemu `pgraph/texture.c`, commit
//! `4bdad199102b22ed0cfa998e9af6b35d76238b3e`): swizzled and compressed mip
//! levels are packed back to back with no padding, each level halving
//! (minimum 1) in every dimension; a cube map is six faces, each face holding
//! the whole mip chain, with the face stride rounded up to 128 bytes. Linear
//! textures are a single `pitch x height` image.

use crate::dxt::{self, BcKind};
use crate::format::{D3DFormat, HostFormat, Layout, TexelFormat};
use crate::{convert, swizzle};

/// Largest meaningful mip count: `1 + log2(max(w, h, d))`.
pub fn max_levels(w: u32, h: u32, d: u32) -> u32 {
    1 + w.max(h).max(d).max(1).ilog2()
}

/// Required alignment of the per-face stride of cube maps.
pub const CUBEMAP_FACE_ALIGNMENT: usize = 128;

/// Errors produced while describing or decoding textures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TextureError {
    /// The format field of the header is not an Xbox texture format.
    #[error("unknown Xbox D3DFORMAT 0x{0:02x}")]
    UnknownFormat(u32),
    /// Header fields contradict each other.
    #[error("invalid texture header: {0}")]
    InvalidHeader(&'static str),
    /// Guest data shorter than the layout requires.
    #[error("texture data too short: need {need} bytes, have {have}")]
    DataTooShort {
        /// Bytes required.
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// Swizzling failed (non power-of-two dimensions).
    #[error(transparent)]
    Swizzle(#[from] swizzle::SwizzleError),
    /// Texel conversion failed.
    #[error(transparent)]
    Convert(#[from] convert::ConvertError),
}

/// Texture dimensionality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureKind {
    /// Plain 2D texture.
    D2,
    /// Cube map: 6 square faces in order +X, -X, +Y, -Y, +Z, -Z.
    Cube,
    /// 3D volume texture.
    Volume,
}

/// Everything needed to locate and interpret a texture's guest memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    /// Pixel format.
    pub format: D3DFormat,
    /// 2D, cube or volume.
    pub kind: TextureKind,
    /// Base level width in texels.
    pub width: u32,
    /// Base level height in texels.
    pub height: u32,
    /// Base level depth (1 unless `kind == Volume`).
    pub depth: u32,
    /// Number of mip levels (always 1 for linear formats).
    pub levels: u32,
    /// Row pitch in bytes. Only meaningful for [`Layout::Linear`]; for other
    /// layouts it is the tight pitch of the base level.
    pub pitch: u32,
}

impl TextureDesc {
    /// A tightly packed descriptor built by hand (tests, the renderer's own
    /// textures). Mips default to 1 for linear formats.
    pub fn new(format: D3DFormat, kind: TextureKind, width: u32, height: u32, depth: u32, levels: u32) -> Self {
        let levels = if format.layout() == Layout::Linear { 1 } else { levels.max(1).min(max_levels(width, height, depth)) };
        let pitch = format.bytes_per_texel().map_or(0, |b| b * width);
        Self { format, kind, width, height, depth, levels, pitch }
    }

    /// Parses the `Format` and `Size` words of an `X_D3DPixelContainer`.
    pub fn from_header(format_word: u32, size_word: u32) -> Result<Self, TextureError> {
        let raw_fmt = (format_word >> 8) & 0xFF;
        let format = D3DFormat::from_raw(raw_fmt).ok_or(TextureError::UnknownFormat(raw_fmt))?;
        let dims = (format_word >> 4) & 0xF;
        let cube = format_word & 0x4 != 0;
        let levels = ((format_word >> 16) & 0xF).max(1);
        let kind = match (dims, cube) {
            (3, false) => TextureKind::Volume,
            (2, true) => TextureKind::Cube,
            (2, false) | (0, false) | (1, false) => TextureKind::D2,
            (3, true) => return Err(TextureError::InvalidHeader("cube map with 3 dimensions")),
            _ => return Err(TextureError::InvalidHeader("dimension field must be 2 or 3")),
        };
        let (width, height, depth, pitch);
        if size_word == 0 {
            // Swizzled / compressed: sizes are log2 in the format word.
            width = 1 << ((format_word >> 20) & 0xF);
            height = 1 << ((format_word >> 24) & 0xF);
            depth = if kind == TextureKind::Volume { 1 << ((format_word >> 28) & 0xF) } else { 1 };
            pitch = format.bytes_per_texel().map_or(0, |b| b * width);
        } else {
            width = (size_word & 0xFFF) + 1;
            height = ((size_word >> 12) & 0xFFF) + 1;
            depth = 1;
            pitch = (((size_word >> 24) & 0xFF) + 1) * 64;
            if kind != TextureKind::D2 {
                return Err(TextureError::InvalidHeader("linear textures cannot be cube/volume"));
            }
        }
        if kind == TextureKind::Cube && width != height {
            return Err(TextureError::InvalidHeader("cube map faces must be square"));
        }
        let levels = if format.layout() == Layout::Linear { 1 } else { levels.min(max_levels(width, height, depth)) };
        let desc = Self { format, kind, width, height, depth, levels, pitch };
        // Reject a pitch smaller than one row now rather than panicking later.
        desc.layout()?;
        Ok(desc)
    }

    /// Number of faces (6 for cube maps, else 1).
    pub fn faces(&self) -> u32 {
        if self.kind == TextureKind::Cube { 6 } else { 1 }
    }

    /// Computes where every level lives in guest memory.
    pub fn layout(&self) -> Result<TextureLayout, TextureError> {
        let mut levels = Vec::with_capacity(self.levels as usize);
        let (mut w, mut h, mut d) = (self.width, self.height, self.depth);
        let mut offset = 0usize;
        match self.format.layout() {
            Layout::Linear => {
                let row = self.format.bytes_per_texel().unwrap_or(0) as usize * w as usize;
                if (self.pitch as usize) < row {
                    return Err(TextureError::InvalidHeader("linear pitch smaller than one row"));
                }
                let size = self.pitch as usize * h as usize;
                levels.push(LevelLayout { level: 0, width: w, height: h, depth: 1, offset: 0, size });
                offset = size;
            }
            layout => {
                for level in 0..self.levels.clamp(1, max_levels(self.width, self.height, self.depth)) {
                    let size = if layout == Layout::Compressed {
                        let kind = BcKind::from_texel(self.format.texel()).expect("compressed");
                        dxt::level_bytes(kind, w, h) * d as usize
                    } else {
                        let bpt = self.format.bytes_per_texel().expect("uncompressed") as usize;
                        w as usize * h as usize * d as usize * bpt
                    };
                    levels.push(LevelLayout { level, width: w, height: h, depth: d, offset, size });
                    offset += size;
                    w = (w / 2).max(1);
                    h = (h / 2).max(1);
                    d = (d / 2).max(1);
                }
            }
        }
        let faces = self.faces();
        let face_stride = if faces > 1 {
            offset.next_multiple_of(CUBEMAP_FACE_ALIGNMENT)
        } else {
            offset
        };
        // The last face is not padded.
        let total_size = face_stride * (faces as usize - 1) + offset;
        Ok(TextureLayout { levels, face_stride, faces, total_size })
    }
}

/// Location of one mip level inside a face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelLayout {
    /// Mip index (0 = base).
    pub level: u32,
    /// Level width in texels.
    pub width: u32,
    /// Level height in texels.
    pub height: u32,
    /// Level depth (volumes).
    pub depth: u32,
    /// Byte offset from the start of the face.
    pub offset: usize,
    /// Bytes occupied in guest memory.
    pub size: usize,
}

/// Full memory map of a texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureLayout {
    /// Per-level locations (identical for every face).
    pub levels: Vec<LevelLayout>,
    /// Bytes between the starts of consecutive faces.
    pub face_stride: usize,
    /// Face count.
    pub faces: u32,
    /// Minimum guest bytes the texture needs (the last face is not padded).
    pub total_size: usize,
}

/// Options controlling [`decode_texture`].
#[derive(Clone, Copy, Debug)]
pub struct DecodeOptions {
    /// Pass DXT blocks through as BC1/2/3. When `false` (or when the base
    /// size is not a multiple of 4) they are decoded to RGBA8 on the CPU.
    pub bc_supported: bool,
    /// `D3DTSS_COLORSIGN` channels (R, G, B, A of the sampled texel) to
    /// store biased: their decoded byte is XORed with `0x80`, so the
    /// signed byte `s` is stored as the unsigned `s + 128`. The host's
    /// UNORM filtering is then linear in `s`, and the pixel shader turns the
    /// filtered value back into a signed one (see
    /// `shaders::psh::PixelOptions::color_sign`). Any signed channel forces
    /// the CPU path for DXT data.
    pub signed: [bool; 4],
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self { bc_supported: true, signed: [false; 4] }
    }
}

impl DecodeOptions {
    /// Sets [`Self::signed`] from a `D3DTSS_COLORSIGN` value
    /// (`D3DTSIGN_ASIGNED` is bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED`
    /// 31).
    pub fn with_color_sign(self, colorsign: u32) -> Self {
        let bit = |n: u32| colorsign >> n & 1 != 0;
        Self { signed: [bit(29), bit(30), bit(31), bit(28)], ..self }
    }
}

/// One decoded mip level of one face, ready to upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLevel {
    /// Cube face index (0 for non-cube textures).
    pub face: u32,
    /// Mip index.
    pub level: u32,
    /// Logical width in texels.
    pub width: u32,
    /// Logical height in texels.
    pub height: u32,
    /// Depth slices.
    pub depth: u32,
    /// Row-major data in the owning texture's [`HostFormat`], slices
    /// concatenated. Rows are tight; for BC formats a "row" is a row of
    /// blocks.
    pub data: Vec<u8>,
}

/// A fully decoded texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTexture {
    /// Host format of every level.
    pub host: HostFormat,
    /// The descriptor it was decoded from.
    pub desc: TextureDesc,
    /// Levels ordered face-major, then by mip.
    pub levels: Vec<DecodedLevel>,
}

impl DecodedTexture {
    /// Bytes per row of blocks/texels for a decoded level.
    pub fn bytes_per_row(&self, l: &DecodedLevel) -> u32 {
        if self.host.is_compressed() {
            l.width.div_ceil(4) * self.host.bytes_per_unit()
        } else {
            l.width * self.host.bytes_per_unit()
        }
    }
}

/// Decodes guest texture memory into host-ready levels.
///
/// `data` is the texture's bytes starting at its base address. `palette`
/// supplies the `A8R8G8B8` entries for `P8` textures.
pub fn decode_texture(
    desc: &TextureDesc,
    data: &[u8],
    palette: Option<&[u32]>,
    opts: DecodeOptions,
) -> Result<DecodedTexture, TextureError> {
    let layout = desc.layout()?;
    if data.len() < layout.total_size {
        return Err(TextureError::DataTooShort { need: layout.total_size, have: data.len() });
    }
    let texel = desc.format.texel();
    let bc = BcKind::from_texel(texel);
    // wgpu requires BC textures to have block-aligned base dimensions; tiny
    // DXT textures (1x1, 2x2) therefore take the CPU path.
    // Volumes would need TEXTURE_COMPRESSION_BC_SLICED_3D, which is not
    // requested, so they always take the CPU path.
    let any_signed = opts.signed.contains(&true);
    let bc_ok = opts.bc_supported && !any_signed && desc.kind != TextureKind::Volume && desc.width.is_multiple_of(4) && desc.height.is_multiple_of(4);
    let host = match (texel, bc) {
        (_, Some(kind)) if bc_ok => match kind {
            BcKind::Bc1 => HostFormat::Bc1RgbaUnorm,
            BcKind::Bc2 => HostFormat::Bc2RgbaUnorm,
            BcKind::Bc3 => HostFormat::Bc3RgbaUnorm,
        },
        (TexelFormat::A8R8G8B8 | TexelFormat::X8R8G8B8, _) => HostFormat::Bgra8Unorm,
        (TexelFormat::Unsupported(f), _) => return Err(convert::ConvertError::Unsupported(TexelFormat::Unsupported(f)).into()),
        _ => HostFormat::Rgba8Unorm,
    };

    let mut out = Vec::new();
    for face in 0..layout.faces {
        let face_base = face as usize * layout.face_stride;
        for lv in &layout.levels {
            let raw = &data[face_base + lv.offset..face_base + lv.offset + lv.size];
            let mut level_data = decode_level(desc, lv, raw, bc, host, palette)?;
            if any_signed {
                bias_signed(&mut level_data, host, opts.signed);
            }
            out.push(DecodedLevel {
                face,
                level: lv.level,
                width: lv.width,
                height: lv.height,
                depth: lv.depth,
                data: level_data,
            });
        }
    }
    Ok(DecodedTexture { host, desc: desc.clone(), levels: out })
}

/// Stores the signed channels of RGBA8/BGRA8 texels as `s + 128`
/// ([`DecodeOptions::signed`]).
fn bias_signed(data: &mut [u8], host: HostFormat, signed: [bool; 4]) {
    // Byte position of R, G, B, A within a host texel.
    let at = match host {
        HostFormat::Bgra8Unorm => [2, 1, 0, 3],
        _ => [0, 1, 2, 3],
    };
    let mut flip = [0u8; 4];
    for (c, &s) in signed.iter().enumerate() {
        if s {
            flip[at[c]] = 0x80;
        }
    }
    for px in data.as_chunks_mut::<4>().0 {
        for (b, f) in px.iter_mut().zip(flip) {
            *b ^= f;
        }
    }
}

fn decode_level(
    desc: &TextureDesc,
    lv: &LevelLayout,
    raw: &[u8],
    bc: Option<BcKind>,
    host: HostFormat,
    palette: Option<&[u32]>,
) -> Result<Vec<u8>, TextureError> {
    let texel = desc.format.texel();
    if let Some(kind) = bc {
        if host.is_compressed() {
            return Ok(raw.to_vec());
        }
        // CPU fallback: decode each depth slice.
        let slice = dxt::level_bytes(kind, lv.width, lv.height);
        let mut out = Vec::with_capacity(lv.width as usize * lv.height as usize * lv.depth as usize * 4);
        for z in 0..lv.depth as usize {
            let rgba = dxt::decode_level(kind, &raw[z * slice..], lv.width, lv.height)
                .map_err(|need| TextureError::DataTooShort { need, have: raw.len() })?;
            out.extend(rgba);
        }
        return Ok(out);
    }

    let bpt = desc.format.bytes_per_texel().expect("uncompressed") as usize;
    let texels = lv.width as usize * lv.height as usize * lv.depth as usize;
    let tight: std::borrow::Cow<[u8]> = match desc.format.layout() {
        Layout::Swizzled => swizzle::unswizzle(raw, lv.width, lv.height, lv.depth, bpt)?.into(),
        Layout::Linear => {
            let row = lv.width as usize * bpt;
            let pitch = desc.pitch as usize;
            if pitch == row {
                raw[..row * lv.height as usize].into()
            } else {
                let mut v = Vec::with_capacity(row * lv.height as usize);
                for y in 0..lv.height as usize {
                    v.extend_from_slice(&raw[y * pitch..y * pitch + row]);
                }
                v.into()
            }
        }
        Layout::Compressed => unreachable!("handled via bc"),
    };
    Ok(match host {
        HostFormat::Bgra8Unorm => convert::to_bgra8(texel, &tight, texels)?,
        _ => convert::to_rgba8(texel, &tight, texels, palette)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(fmt: u32, dims: u32, cube: bool, levels: u32, u: u32, v: u32, p: u32) -> u32 {
        (p << 28) | (v << 24) | (u << 20) | (levels << 16) | (fmt << 8) | (dims << 4) | ((cube as u32) << 2)
    }

    #[test]
    fn parses_swizzled_header() {
        let d = TextureDesc::from_header(header(0x06, 2, false, 5, 4, 3, 0), 0).unwrap();
        assert_eq!(d.format, D3DFormat::A8R8G8B8);
        assert_eq!((d.width, d.height, d.depth, d.levels), (16, 8, 1, 5));
        assert_eq!(d.kind, TextureKind::D2);
    }

    #[test]
    fn parses_linear_header_with_size_word() {
        // 100x50, pitch 448 (= 7*64): size = (pitch/64-1)<<24 | (h-1)<<12 | (w-1)
        let size = (6 << 24) | (49 << 12) | 99;
        let d = TextureDesc::from_header(header(0x12, 2, false, 1, 0, 0, 0), size).unwrap();
        assert_eq!((d.width, d.height, d.pitch), (100, 50, 448));
        assert_eq!(d.format.layout(), Layout::Linear);
    }

    #[test]
    fn parses_cube_and_volume() {
        let c = TextureDesc::from_header(header(0x0C, 2, true, 1, 3, 3, 0), 0).unwrap();
        assert_eq!(c.kind, TextureKind::Cube);
        assert_eq!(c.faces(), 6);
        let v = TextureDesc::from_header(header(0x06, 3, false, 1, 2, 2, 3), 0).unwrap();
        assert_eq!(v.kind, TextureKind::Volume);
        assert_eq!((v.width, v.height, v.depth), (4, 4, 8));
    }

    #[test]
    fn rejects_bad_headers() {
        assert_eq!(
            TextureDesc::from_header(header(0x0A, 2, false, 1, 0, 0, 0), 0),
            Err(TextureError::UnknownFormat(0x0A))
        );
        assert!(TextureDesc::from_header(header(0x06, 2, true, 1, 3, 2, 0), 0).is_err());
        assert!(TextureDesc::from_header(header(0x06, 3, true, 1, 3, 3, 0), 0).is_err());
    }

    #[test]
    fn mip_chain_layout_swizzled() {
        let d = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::D2, 8, 4, 1, 4);
        let l = d.layout().unwrap();
        let sizes: Vec<_> = l.levels.iter().map(|l| (l.width, l.height, l.offset, l.size)).collect();
        assert_eq!(sizes, vec![(8, 4, 0, 128), (4, 2, 128, 32), (2, 1, 160, 8), (1, 1, 168, 4)]);
        assert_eq!(l.total_size, 172);
    }

    #[test]
    fn mip_chain_layout_dxt() {
        let d = TextureDesc::new(D3DFormat::Dxt1, TextureKind::D2, 8, 8, 1, 4);
        let l = d.layout().unwrap();
        let sizes: Vec<_> = l.levels.iter().map(|l| l.size).collect();
        // 8x8 = 4 blocks, 4x4 = 1, 2x2 = 1 (padded), 1x1 = 1 (padded).
        assert_eq!(sizes, vec![32, 8, 8, 8]);
    }

    #[test]
    fn cube_face_stride_is_128_aligned() {
        let d = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::Cube, 4, 4, 1, 3);
        let l = d.layout().unwrap();
        // 64 + 16 + 4 = 84 -> 128.
        assert_eq!(l.face_stride, 128);
        assert_eq!(l.total_size, 128 * 5 + 84);
    }

    #[test]
    fn volume_depth_halves_per_level() {
        let d = TextureDesc::new(D3DFormat::L8, TextureKind::Volume, 4, 4, 4, 3);
        let l = d.layout().unwrap();
        let sz: Vec<_> = l.levels.iter().map(|l| (l.depth, l.size)).collect();
        assert_eq!(sz, vec![(4, 64), (2, 8), (1, 1)]);
    }

    #[test]
    fn decode_swizzled_a8r8g8b8_roundtrip() {
        let (w, h) = (8u32, 4u32);
        let mut linear = Vec::new();
        for i in 0..(w * h) {
            linear.extend_from_slice(&[i as u8, (i * 2) as u8, (i * 3) as u8, 255 - i as u8]);
        }
        let sw = swizzle::swizzle(&linear, w, h, 1, 4).unwrap();
        let d = TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::D2, w, h, 1, 1);
        let t = decode_texture(&d, &sw, None, DecodeOptions::default()).unwrap();
        assert_eq!(t.host, HostFormat::Bgra8Unorm);
        assert_eq!(t.levels.len(), 1);
        assert_eq!(t.levels[0].data, linear);
    }

    #[test]
    fn decode_linear_strips_pitch_padding() {
        // 3x2 R5G6B5 with a 64-byte pitch.
        let mut data = vec![0u8; 128];
        for (y, px) in [[0xF800u16, 0x07E0, 0x001F], [0xFFFF, 0, 0xF800]].iter().enumerate() {
            for (x, v) in px.iter().enumerate() {
                data[y * 64 + x * 2..][..2].copy_from_slice(&v.to_le_bytes());
            }
        }
        let mut d = TextureDesc::new(D3DFormat::LinR5G6B5, TextureKind::D2, 3, 2, 1, 1);
        d.pitch = 64;
        let t = decode_texture(&d, &data, None, DecodeOptions::default()).unwrap();
        assert_eq!(t.host, HostFormat::Rgba8Unorm);
        let px: Vec<&[u8]> = t.levels[0].data.chunks(4).collect();
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[1], [0, 255, 0, 255]);
        assert_eq!(px[2], [0, 0, 255, 255]);
        assert_eq!(px[3], [255, 255, 255, 255]);
        assert_eq!(px[4], [0, 0, 0, 255]);
    }

    #[test]
    fn dxt_passthrough_vs_cpu_fallback() {
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&0xF800u16.to_le_bytes());
        block[2..4].copy_from_slice(&0xF800u16.to_le_bytes());
        let d = TextureDesc::new(D3DFormat::Dxt1, TextureKind::D2, 4, 4, 1, 1);
        let t = decode_texture(&d, &block, None, DecodeOptions { bc_supported: true, ..Default::default() }).unwrap();
        assert_eq!(t.host, HostFormat::Bc1RgbaUnorm);
        assert_eq!(t.levels[0].data, block);
        assert_eq!(t.bytes_per_row(&t.levels[0]), 8);
        let t = decode_texture(&d, &block, None, DecodeOptions { bc_supported: false, ..Default::default() }).unwrap();
        assert_eq!(t.host, HostFormat::Rgba8Unorm);
        assert_eq!(t.levels[0].data.len(), 64);
        assert_eq!(&t.levels[0].data[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn signed_channels_are_stored_biased() {
        // A8R8G8B8 0x80_7F_01_FF (A, R, G, B): R and G signed.
        let d = TextureDesc::new(D3DFormat::LinA8R8G8B8, TextureKind::D2, 1, 1, 1, 1);
        let opts = DecodeOptions { signed: [true, true, false, false], ..Default::default() };
        let t = decode_texture(&d, &[0xFF, 0x01, 0x7F, 0x80], None, opts).unwrap();
        assert_eq!(t.host, HostFormat::Bgra8Unorm);
        assert_eq!(t.levels[0].data, [0xFF, 0x81, 0xFF, 0x80]);
        // RGBA hosts flip the same logical channels; alpha here.
        let d = TextureDesc::new(D3DFormat::A8, TextureKind::D2, 1, 1, 1, 1);
        let opts = DecodeOptions { signed: [false, false, false, true], ..Default::default() };
        let t = decode_texture(&d, &[0x10], None, opts).unwrap();
        assert_eq!(t.levels[0].data, [255, 255, 255, 0x90]);
        // Signed DXT is decoded on the CPU so its texels can be biased.
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&0xF800u16.to_le_bytes());
        block[2..4].copy_from_slice(&0xF800u16.to_le_bytes());
        let d = TextureDesc::new(D3DFormat::Dxt1, TextureKind::D2, 4, 4, 1, 1);
        let opts = DecodeOptions { signed: [true, false, false, false], ..Default::default() };
        let t = decode_texture(&d, &block, None, opts).unwrap();
        assert_eq!(t.host, HostFormat::Rgba8Unorm);
        assert_eq!(&t.levels[0].data[..4], &[0x7F, 0, 0, 255]);
    }

    #[test]
    fn decode_cube_collects_all_faces_and_levels() {
        let d = TextureDesc::new(D3DFormat::L8, TextureKind::Cube, 2, 2, 1, 2);
        let l = d.layout().unwrap();
        let mut data = vec![0u8; l.total_size];
        for f in 0..6 {
            data[f * l.face_stride] = f as u8 + 1; // first texel of each face's base level
        }
        let t = decode_texture(&d, &data, None, DecodeOptions::default()).unwrap();
        assert_eq!(t.levels.len(), 12);
        for f in 0..6u32 {
            let lvl0 = t.levels.iter().find(|l| l.face == f && l.level == 0).unwrap();
            assert_eq!(lvl0.data[0], f as u8 + 1);
        }
    }

    #[test]
    fn decode_volume_unswizzles_in_3d() {
        let linear: Vec<u8> = (0..64).collect();
        let sw = swizzle::swizzle(&linear, 4, 4, 4, 1).unwrap();
        let d = TextureDesc::new(D3DFormat::L8, TextureKind::Volume, 4, 4, 4, 1);
        let t = decode_texture(&d, &sw, None, DecodeOptions::default()).unwrap();
        // L8 expands to RGBA; red channel carries the original byte.
        let red: Vec<u8> = t.levels[0].data.chunks(4).map(|p| p[0]).collect();
        assert_eq!(red, linear);
    }

    #[test]
    fn palette_and_short_data() {
        let d = TextureDesc::new(D3DFormat::P8, TextureKind::D2, 2, 2, 1, 1);
        let pal: Vec<u32> = (0..256).map(|i| 0xFF000000 | i).collect();
        let data = swizzle::swizzle(&[0, 1, 2, 3], 2, 2, 1, 1).unwrap();
        let t = decode_texture(&d, &data, Some(&pal), DecodeOptions::default()).unwrap();
        assert_eq!(&t.levels[0].data[4..8], &[0, 0, 1, 255]);
        assert!(matches!(
            decode_texture(&d, &[0u8; 2], Some(&pal), DecodeOptions::default()),
            Err(TextureError::DataTooShort { need: 4, have: 2 })
        ));
    }

    #[test]
    fn depth_formats_are_rejected_as_textures() {
        let d = TextureDesc::new(D3DFormat::D24S8, TextureKind::D2, 2, 2, 1, 1);
        assert!(decode_texture(&d, &[0u8; 64], None, DecodeOptions::default()).is_err());
    }

    #[test]
    fn linear_pitch_smaller_than_row_is_rejected() {
        // LIN_A8R8G8B8 (0x12), size word 99 -> 100x1, pitch 64 < 400.
        let err = TextureDesc::from_header(0x12 << 8 | 0x20, 99).unwrap_err();
        assert!(matches!(err, TextureError::InvalidHeader(_)), "{err:?}");
    }

    #[test]
    fn mip_count_is_clamped_to_the_chain_length() {
        // 4x4 with a header claiming 15 levels.
        let d = TextureDesc::from_header(header(0x06, 2, false, 15, 2, 2, 0), 0).unwrap();
        assert_eq!(d.levels, 3);
        assert_eq!(d.layout().unwrap().levels.len(), 3);
        assert_eq!(TextureDesc::new(D3DFormat::A8R8G8B8, TextureKind::D2, 4, 4, 1, 15).levels, 3);
    }

    #[test]
    fn dxt_volume_takes_the_cpu_path() {
        let d = TextureDesc::new(D3DFormat::Dxt1, TextureKind::Volume, 4, 4, 2, 1);
        let data = vec![0u8; d.layout().unwrap().total_size];
        let t = decode_texture(&d, &data, None, DecodeOptions { bc_supported: true, ..Default::default() }).unwrap();
        assert!(!t.host.is_compressed());
        assert_eq!(t.levels[0].data.len(), 4 * 4 * 2 * 4);
    }
}
