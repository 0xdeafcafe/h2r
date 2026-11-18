//! Bitmap tag image -> `gfx::TextureDesc` -> RGBA8.
//!
//! # Mapping rules
//!
//! Field meanings come from the bitmap image block (`tags::Image`, record
//! `BitmapImage`, stride 0x74; see `docs/formats/map-format.md`). Evidence labels:
//! **V** = checked against the disc by `h2 bitmap --all` (layout: the LOD size
//! equals the computed layout rounded up to 128 bytes for every image in
//! mainmenu, 00a_introduction and 01b_spacestation), **S** = source hypothesis
//! not independently confirmed. Per-format colour evidence is the typed
//! [`Evidence`] column of [`FORMAT_TABLE`].
//!
//! * **Format** ([`FORMAT_TABLE`]): Halo's `PixelFormat` id -> Xbox
//!   `D3DFormat`. Size checks cannot tell two same-width formats apart, so the
//!   pairing is only as strong as its [`Evidence`].
//! * **Swizzled / linear**: image `flags` bit 3 (`8`) = swizzled, bit 4 (`16`)
//!   = linear. Exactly one must be set for uncompressed formats (both or
//!   neither is rejected); a linear image selects the `LIN_*` twin. DXT images
//!   have neither bit. **V**.
//! * **Mip count**: `mip_count` is the number of levels *below* the base, so a
//!   texture has `mip_count + 1` levels; more than `1 + log2(max dim)` or any
//!   extra level on a linear image is rejected rather than clamped. **V**.
//! * **Type**: 0 = 2D, 1 = 3D (`depth` = slice count, must be >= 1), 2 = cube
//!   (must be square). Non-volume depth must be 0 or 1.
//! * **LODs**: the three `lod_pointers` each address a *complete* mip chain,
//!   padded to a multiple of 128 bytes, whose first level is mip 0, 1 and 2
//!   respectively; LOD *n* is absent (size 0) for small textures. We decode the
//!   first present LOD, shifting the descriptor's size and level count. **V**.
//! * **Pitch**: linear rows are padded to 64 bytes ([`LINEAR_PITCH_ALIGN`]). **V**.
//! * **DXT chain**: for 2D and cube DXT textures the stored chain ends at the
//!   first level that fits one 4x4 block ([`stored_dxt_levels`]). **V** for 2D
//!   and cube. **Unverified for DXT volumes** (none exist on the disc):
//!   [`texture_desc`] refuses them rather than guess.
//! * **Limits**: layouts above [`MAX_LAYOUT_BYTES`] or decoded output above
//!   [`MAX_DECODED_BYTES`] are rejected before allocating, and only the layout's
//!   bytes are read, never the tag's declared LOD size.

use crate::resource::ResourceFiles;
use crate::{Error, Result};
use gfx::texture::{DecodeOptions, TextureDesc, TextureError, TextureKind, decode_texture, max_levels};
use gfx::{D3DFormat, HostFormat};
use tags::{Image, PixelFormat, TextureKind as TagKind};

/// Largest texture layout (guest bytes) we will read.
pub const MAX_LAYOUT_BYTES: usize = 256 << 20;
/// Largest RGBA8 output we will produce for one image.
pub const MAX_DECODED_BYTES: u64 = 1 << 30;
/// Number of LOD pointers in an image record.
pub const LOD_COUNT: u32 = 3;

/// How strongly a format pairing is established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// Decoded real disc images viewed by a person: structure correct, channel
    /// order and alpha plausible (known-colour content: logo, sky, UI), plus
    /// the layout size check. Also covered by synthetic channel test vectors.
    Pixels,
    /// Layout size check on real images only (no known-colour content seen);
    /// channel order rests on the synthetic test vectors / `docs/graphics/graphics.md`.
    LayoutOnly,
    /// Hypothesis: neither the pairing nor colours are confirmed. Output is
    /// for inspection only.
    Hypothesis,
}

/// One row of the Halo pixel format -> Xbox `D3DFormat` table.
#[derive(Clone, Copy, Debug)]
pub struct FormatRow {
    /// Halo 2 bitmap pixel format.
    pub halo: PixelFormat,
    /// Xbox format for swizzled (or block-compressed) storage.
    pub swizzled: D3DFormat,
    /// Xbox format for linear storage, if the hardware has one.
    pub linear: Option<D3DFormat>,
    /// How well the pairing is established.
    pub evidence: Evidence,
}

macro_rules! row {
    ($h:ident => $s:ident, $l:ident, $e:ident) => {
        FormatRow {
            halo: PixelFormat::$h,
            swizzled: D3DFormat::$s,
            linear: Some(D3DFormat::$l),
            evidence: Evidence::$e,
        }
    };
    ($h:ident => $s:ident, $e:ident) => {
        FormatRow { halo: PixelFormat::$h, swizzled: D3DFormat::$s, linear: None, evidence: Evidence::$e }
    };
}

/// Supported formats, named after the pixel layout each decodes to (ids as
/// in `tags::PixelFormat`; each row's evidence says whether its pixels
/// were checked or only its layout); Xbox formats and channel semantics:
/// `docs/graphics/graphics.md`.
pub const FORMAT_TABLE: &[FormatRow] = &[
    row!(A8 => A8, LinA8, LayoutOnly),
    row!(Y8 => L8, LinL8, Pixels),
    row!(Ay8 => Al8, LinAl8, LayoutOnly),
    row!(A8Y8 => A8L8, LinA8L8, Pixels),
    row!(Rgb565 => R5G6B5, LinR5G6B5, LayoutOnly),
    row!(Argb1555 => A1R5G5B5, LinA1R5G5B5, Pixels),
    row!(Argb4444 => A4R4G4B4, LinA4R4G4B4, Pixels),
    row!(Xrgb8888 => X8R8G8B8, LinX8R8G8B8, Pixels),
    row!(Argb8888 => A8R8G8B8, LinA8R8G8B8, Pixels),
    row!(Dxt1 => Dxt1, Pixels),
    row!(Dxt3 => Dxt3, Pixels),
    row!(Dxt5 => Dxt5, Pixels),
    // HYPOTHESES: which Xbox 2-channel format V8U8 and G8B8 correspond to is
    // not established (a size check cannot separate them), and the data is
    // signed, so the RGBA output is not a colour image.
    row!(V8U8 => G8B8, LinG8B8, Hypothesis),
    row!(G8B8 => R8B8, LinR8B8, Hypothesis),
    // P8Bump / P8 are 8-bit swizzled images whose bytes are bump heights /
    // palette indices; the tag carries no palette. We show the raw index as
    // luminance purely to inspect it and check its layout (**V**); the values
    // are not colours.
    row!(P8Bump => L8, Hypothesis),
    row!(P8 => L8, Hypothesis),
];

/// The Xbox format for a Halo format and storage flag.
///
/// Float formats have no Xbox texture equivalent and are reported as
/// [`Error::UnsupportedFormat`].
pub fn d3d_format(format: PixelFormat, linear: bool) -> Result<D3DFormat> {
    let row = FORMAT_TABLE
        .iter()
        .find(|r| r.halo == format)
        .ok_or_else(|| Error::UnsupportedFormat(format!("{format:?}")))?;
    if linear {
        row.linear.ok_or_else(|| Error::UnsupportedFormat(format!("{format:?} (linear)")))
    } else {
        Ok(row.swizzled)
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

/// Builds the descriptor for the chain starting at mip `lod` (0..=2).
///
/// Sizes are shifted down by `lod` and the level count reduced to match.
/// Contradictory fields are rejected here instead of being silently fixed up
/// by `TextureDesc::new`.
pub fn texture_desc(image: &Image, lod: u32) -> Result<TextureDesc> {
    if lod >= LOD_COUNT {
        return Err(invalid(format!("LOD {lod} out of range")));
    }
    let (swz, lin) = (image.is_swizzled(), image.is_linear());
    let compressed = matches!(image.format, PixelFormat::Dxt1 | PixelFormat::Dxt3 | PixelFormat::Dxt5);
    if swz && lin {
        return Err(invalid("both swizzled and linear flags set"));
    }
    if !compressed && !swz && !lin {
        return Err(invalid("uncompressed image is neither swizzled nor linear"));
    }
    let format = d3d_format(image.format, lin)?;
    let kind = match image.kind {
        TagKind::D2 => TextureKind::D2,
        TagKind::D3 => TextureKind::Volume,
        TagKind::Cube => TextureKind::Cube,
        TagKind::Unknown(n) => return Err(invalid(format!("texture type {n}"))),
    };
    let [w, h, d] = image.dimensions.map(u32::from);
    if w == 0 || h == 0 {
        return Err(invalid("zero dimension"));
    }
    match kind {
        TextureKind::Volume if d == 0 => return Err(invalid("volume with zero depth")),
        TextureKind::D2 | TextureKind::Cube if d > 1 => return Err(invalid("depth > 1 on a non-volume")),
        TextureKind::Cube if w != h => return Err(invalid("cube map is not square")),
        _ => {}
    }
    if compressed && kind == TextureKind::Volume {
        return Err(Error::UnsupportedFormat("DXT volume (chain layout unverified)".into()));
    }
    let depth = if kind == TextureKind::Volume { d } else { 1 };
    let total = u32::from(image.mip_count) + 1;
    if total > max_levels(w, h, depth) {
        return Err(invalid(format!("{total} levels exceed the maximum for {w}x{h}x{depth}")));
    }
    if lin && total != 1 {
        return Err(invalid("linear image with mip levels"));
    }
    if lod >= total {
        return Err(invalid(format!("LOD {lod} beyond {total} levels")));
    }
    let shift = |v: u32| (v >> lod).max(1);
    let (w, h, depth) = (shift(w), shift(h), shift(depth));
    let mut levels = total - lod;
    if compressed {
        levels = levels.min(stored_dxt_levels(w, h));
    }
    let mut desc = TextureDesc::new(format, kind, w, h, depth, levels);
    if desc.levels != levels {
        return Err(invalid("gfx adjusted the level count"));
    }
    if lin {
        let row = format
            .bytes_per_texel()
            .and_then(|b| b.checked_mul(w))
            .ok_or_else(|| invalid("linear row size overflows"))?;
        desc.pitch = row.next_multiple_of(LINEAR_PITCH_ALIGN);
    }
    Ok(desc)
}

/// Linear images are stored with each row padded to a multiple of 64 bytes,
/// the Xbox D3D pitch granule (also the unit of the `Size` word's pitch field
/// in `gfx::texture`). **V**: with it the layout rounded up to 128 bytes equals
/// the LOD size for every linear image checked.
pub const LINEAR_PITCH_ALIGN: u32 = 64;

/// Levels of a block-compressed 2D or cube chain that the tag data actually
/// stores: the chain stops at the first level that fits in a single 4x4 block
/// (the 2x2 and 1x1 levels that D3D would pad to a block each are absent).
/// **V** for 2D and cube textures: without this cap, textures such as 1024x8
/// DXT3 are 16 bytes longer than the LOD data; with it the layout rounded up to
/// 128 bytes equals the LOD size on every 2D/cube DXT image checked.
pub fn stored_dxt_levels(mut w: u32, mut h: u32) -> u32 {
    let mut n = 1;
    while w > 4 || h > 4 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        n += 1;
    }
    n
}

/// One decoded mip level of one face, row-major RGBA8 (`depth` slices stacked).
#[derive(Clone, Debug)]
pub struct Level {
    /// Cube face (0 otherwise).
    pub face: u32,
    /// Mip index relative to the decoded chain (0 = largest decoded).
    pub level: u32,
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Depth slices.
    pub depth: u32,
    /// `width * height * depth * 4` bytes, `R,G,B,A`.
    pub rgba: Vec<u8>,
}

/// A decoded image.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    /// The descriptor decoded with.
    pub desc: TextureDesc,
    /// Which LOD pointer supplied the data (its first level is mip `lod`).
    pub lod: u32,
    /// Levels, face-major then by mip.
    pub levels: Vec<Level>,
    /// Bytes in the LOD resource, as declared by the tag.
    pub data_len: usize,
    /// Bytes the descriptor's layout needs (never more than `data_len`).
    pub layout_len: usize,
}

impl DecodedImage {
    /// Whether the LOD size is exactly the layout size rounded up to 128
    /// bytes, as observed for well-formed images. A `false` here means the
    /// mapping is probably wrong even though decoding succeeded.
    pub fn size_matches(&self) -> bool {
        self.layout_len.next_multiple_of(128) == self.data_len
    }
}

/// Decodes guest bytes for `desc` to RGBA8 levels. Checks the layout and output
/// size caps before allocating anything.
pub fn decode_bytes(desc: &TextureDesc, data: &[u8]) -> Result<Vec<Level>> {
    let layout = desc.layout()?;
    if layout.total_size > MAX_LAYOUT_BYTES {
        return Err(invalid(format!("layout of {} bytes exceeds cap", layout.total_size)));
    }
    let texels: u64 = layout.levels.iter().map(|l| u64::from(l.width) * u64::from(l.height) * u64::from(l.depth)).sum();
    if texels * u64::from(layout.faces) * 4 > MAX_DECODED_BYTES {
        return Err(invalid("decoded size exceeds cap"));
    }
    let decoded = decode_texture(desc, data, None, DecodeOptions { bc_supported: false, ..Default::default() })?;
    let swap = decoded.host == HostFormat::Bgra8Unorm;
    Ok(decoded
        .levels
        .into_iter()
        .map(|l| {
            let mut rgba = l.data;
            if swap {
                for p in rgba.as_chunks_mut::<4>().0.iter_mut() {
                    p.swap(0, 2);
                }
            }
            Level { face: l.face, level: l.level, width: l.width, height: l.height, depth: l.depth, rgba }
        })
        .collect())
}

/// Reads the highest-resolution LOD present and decodes it on the CPU.
///
/// Only the layout's bytes are read (the tag's declared size is checked
/// against the layout and the file, not allocated).
pub fn decode_image(image: &Image, files: &mut ResourceFiles) -> Result<DecodedImage> {
    let (lod, range) = image
        .lods
        .iter()
        .enumerate()
        .find_map(|(i, r)| r.map(|r| (i as u32, r)))
        .ok_or(Error::NoData)?;
    let desc = texture_desc(image, lod)?;
    let layout_len = desc.layout()?.total_size;
    if layout_len > MAX_LAYOUT_BYTES {
        return Err(invalid(format!("layout of {layout_len} bytes exceeds cap")));
    }
    let declared = range.size as usize;
    if declared < layout_len {
        return Err(TextureError::DataTooShort { need: layout_len, have: declared }.into());
    }
    let file_len = files.length(range.pointer.file).ok_or(Error::MissingResource(range.pointer.file))?;
    if u64::from(range.pointer.offset) + u64::from(range.size) > file_len {
        return Err(Error::Map(map::Error::Bounds {
            what: "bitmap LOD",
            offset: range.pointer.offset.into(),
            size: range.size.into(),
            limit: file_len,
        }));
    }
    let data = files.read(range.pointer.file, range.pointer.offset, layout_len as u32)?;
    let levels = decode_bytes(&desc, &data)?;
    Ok(DecodedImage { desc, lod, levels, data_len: declared, layout_len })
}

#[cfg(test)]
mod tests {
    use super::*;

    use gfx::format::Layout;
    use crate::resource::{Region, ResourceFiles};
    use map::{RawPointer, ResourceFile};
    use tags::ResourceRange;
    use tags::records::BitmapImage;
    use tags::wire::{BinaryRecord, Flags16};

    const SWZ: u16 = 0x8;
    const LIN: u16 = 0x10;
    const DXT: u16 = 0x2;

    fn image(format: PixelFormat, kind: TagKind, dims: [u16; 3], mips: u16, flags: u16) -> Image {
        let mut wire = BitmapImage::decode(&[0u8; BitmapImage::SIZE]).unwrap();
        wire.flags = Flags16(flags);
        Image { format, kind, dimensions: dims, mip_count: mips, lods: [None; 3], low_detail: None, wire }
    }

    fn temp(bytes: &[u8]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("assets-bm-{}-{}.bin", std::process::id(), N.fetch_add(1, Relaxed)));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn dxt_chain_stops_at_one_block() {
        assert_eq!(stored_dxt_levels(512, 512), 8); // 512..4
        assert_eq!(stored_dxt_levels(1024, 8), 9); // 1024x8 .. 4x1
        assert_eq!(stored_dxt_levels(4, 4), 1);
        assert_eq!(stored_dxt_levels(2, 2), 1);
        let img = image(PixelFormat::Dxt5, TagKind::D2, [1024, 8, 1], 10, DXT);
        let d = texture_desc(&img, 0).unwrap();
        assert_eq!(d.levels, 9);
        // 8192+2048+1024+512+256+128+64+32+16 bytes
        assert_eq!(d.layout().unwrap().total_size, 12272);
        // cubes use the same rule
        let c = image(PixelFormat::Dxt1, TagKind::Cube, [64, 64, 1], 6, DXT);
        assert_eq!(texture_desc(&c, 0).unwrap().levels, 5);
    }

    #[test]
    fn dxt_volumes_are_refused_as_unverified() {
        let img = image(PixelFormat::Dxt1, TagKind::D3, [8, 8, 8], 0, DXT);
        assert!(matches!(texture_desc(&img, 0), Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn linear_rows_pad_to_64_bytes() {
        let img = image(PixelFormat::Argb8888, TagKind::D2, [21, 50, 1], 0, LIN | 0x280);
        let d = texture_desc(&img, 0).unwrap();
        assert_eq!((d.format, d.pitch), (D3DFormat::LinA8R8G8B8, 128));
        assert_eq!(d.layout().unwrap().total_size, 6400);
    }

    #[test]
    fn lod_shifts_size_and_levels() {
        let img = image(PixelFormat::Xrgb8888, TagKind::D2, [64, 32, 1], 6, SWZ);
        let d = texture_desc(&img, 1).unwrap();
        assert_eq!((d.width, d.height, d.levels), (32, 16, 6));
        assert!(texture_desc(&img, 7).is_err());
    }

    #[test]
    fn out_of_range_lods_return_errors_without_panicking() {
        let img = image(PixelFormat::Xrgb8888, TagKind::D2, [64, 32, 1], 6, SWZ);
        for lod in [3, 31, 32, 33, 64, u32::MAX] {
            assert!(texture_desc(&img, lod).is_err(), "lod {lod}");
        }
    }

    #[test]
    fn contradictory_descriptors_are_rejected_not_fixed_up() {
        let bad = |img: Image| assert!(matches!(texture_desc(&img, 0), Err(Error::Invalid(_))));
        // zero depth on a volume (gfx would clamp it to 1)
        bad(image(PixelFormat::Xrgb8888, TagKind::D3, [4, 4, 0], 0, SWZ));
        // depth on a 2D image
        bad(image(PixelFormat::Xrgb8888, TagKind::D2, [4, 4, 3], 0, SWZ));
        // zero size
        bad(image(PixelFormat::Xrgb8888, TagKind::D2, [0, 4, 1], 0, SWZ));
        // more mips than the size allows (gfx would clamp)
        bad(image(PixelFormat::Xrgb8888, TagKind::D2, [4, 4, 1], 5, SWZ));
        bad(image(PixelFormat::Dxt1, TagKind::D2, [16, 16, 1], 9, DXT));
        // mips on a linear image (gfx would force one level)
        bad(image(PixelFormat::Argb8888, TagKind::D2, [8, 8, 1], 3, LIN));
        // both / neither storage flags
        bad(image(PixelFormat::Argb8888, TagKind::D2, [8, 8, 1], 0, SWZ | LIN));
        bad(image(PixelFormat::Argb8888, TagKind::D2, [8, 8, 1], 0, 0));
        // non-square cube
        bad(image(PixelFormat::Xrgb8888, TagKind::Cube, [8, 4, 1], 0, SWZ));
        // a valid one still passes
        assert!(texture_desc(&image(PixelFormat::Xrgb8888, TagKind::D3, [4, 4, 4], 2, SWZ), 0).is_ok());
    }

    #[test]
    fn decodes_a_linear_image_from_a_resource_file() {
        // 2x2 linear A8R8G8B8, pitch 64, bytes B,G,R,A per texel.
        let mut data = vec![0u8; 128];
        data[0..4].copy_from_slice(&[10, 20, 30, 255]);
        data[4..8].copy_from_slice(&[1, 2, 3, 4]);
        data[64..68].copy_from_slice(&[5, 6, 7, 8]);
        let path = temp(&[vec![0u8; 16], data].concat());
        let mut files = ResourceFiles::new();
        files.set(ResourceFile::Shared, Region { path: path.clone(), base: 16, length: 128 }).unwrap();
        let mut img = image(PixelFormat::Argb8888, TagKind::D2, [2, 2, 1], 0, LIN);
        img.lods[0] = Some(ResourceRange {
            pointer: RawPointer { file: ResourceFile::Shared, offset: 0 },
            size: 128,
        });
        let d = decode_image(&img, &mut files).unwrap();
        assert!(d.size_matches());
        let px = &d.levels[0].rgba;
        assert_eq!(&px[0..4], &[30, 20, 10, 255]); // R,G,B,A
        assert_eq!(&px[4..8], &[3, 2, 1, 4]);
        assert_eq!(&px[8..12], &[7, 6, 5, 8]);
        // a missing file is reported, not panicked on
        img.lods[0].as_mut().unwrap().pointer.file = ResourceFile::MainMenu;
        assert!(matches!(decode_image(&img, &mut files), Err(Error::MissingResource(_))));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_huge_declared_lod_size_is_not_allocated() {
        let path = temp(&[0u8; 256]);
        let mut files = ResourceFiles::new();
        files.set(ResourceFile::Local, Region { path: path.clone(), base: 0, length: 256 }).unwrap();
        let mut img = image(PixelFormat::Argb8888, TagKind::D2, [2, 2, 1], 0, LIN);
        let at = |size| Some(ResourceRange { pointer: RawPointer { file: ResourceFile::Local, offset: 0 }, size });
        // declared size far beyond the file: rejected by bounds, no allocation
        img.lods[0] = at(u32::MAX);
        assert!(matches!(decode_image(&img, &mut files), Err(Error::Map(_))));
        // declared size smaller than the layout: rejected
        img.lods[0] = at(64);
        assert!(matches!(decode_image(&img, &mut files), Err(Error::Texture(_))));
        // a huge layout (65535x65535 linear) is rejected by the cap, not read
        let mut big = image(PixelFormat::Argb8888, TagKind::D2, [65535, 65535, 1], 0, LIN);
        big.lods[0] = at(u32::MAX);
        assert!(matches!(decode_image(&big, &mut files), Err(Error::Invalid(_))));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn format_table_has_no_duplicate_halo_formats() {
        for (i, a) in FORMAT_TABLE.iter().enumerate() {
            assert!(FORMAT_TABLE[i + 1..].iter().all(|b| b.halo != a.halo));
        }
    }

    #[test]
    fn linear_selects_the_lin_twin() {
        assert_eq!(d3d_format(PixelFormat::Argb8888, false).unwrap(), D3DFormat::A8R8G8B8);
        assert_eq!(d3d_format(PixelFormat::Argb8888, true).unwrap(), D3DFormat::LinA8R8G8B8);
        assert_eq!(d3d_format(PixelFormat::Dxt5, false).unwrap(), D3DFormat::Dxt5);
        assert!(d3d_format(PixelFormat::Dxt5, true).is_err());
        assert!(d3d_format(PixelFormat::P8, true).is_err());
        assert!(d3d_format(PixelFormat::P8Bump, true).is_err());
        assert!(d3d_format(PixelFormat::ArgbF32, false).is_err());
    }

    #[test]
    fn twins_have_the_right_layout() {
        for r in FORMAT_TABLE {
            if let Some(l) = r.linear {
                assert_eq!(l.layout(), Layout::Linear, "{:?}", r.halo);
                assert_eq!(l.texel(), r.swizzled.texel(), "{:?}", r.halo);
            }
        }
    }

    #[test]
    fn unconfirmed_pairings_are_labelled() {
        let ev = |f| FORMAT_TABLE.iter().find(|r| r.halo == f).unwrap().evidence;
        assert_eq!(ev(PixelFormat::V8U8), Evidence::Hypothesis);
        assert_eq!(ev(PixelFormat::G8B8), Evidence::Hypothesis);
        assert_eq!(ev(PixelFormat::P8), Evidence::Hypothesis);
        assert_eq!(ev(PixelFormat::Argb8888), Evidence::Pixels);
    }

    /// One 4x4-block solid-red DXT colour block: c0 = 0xF800 (R5G6B5 red), c1 = 0.
    const RED_BLOCK: [u8; 8] = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];

    /// Known texel bytes -> expected RGBA for each decodable table row, run
    /// through the same path real images take (swizzled and linear).
    #[test]
    fn every_format_decodes_a_known_texel_to_the_expected_channels() {
        let red = [255, 0, 0, 255];
        let cases: Vec<(PixelFormat, Vec<u8>, [u8; 4])> = vec![
            (PixelFormat::A8, vec![0x80], [255, 255, 255, 0x80]),
            (PixelFormat::Y8, vec![0x80], [0x80, 0x80, 0x80, 255]),
            (PixelFormat::Ay8, vec![0x40], [0x40; 4]),
            (PixelFormat::A8Y8, vec![0x10, 0x20], [0x10, 0x10, 0x10, 0x20]), // byte0 = Y, byte1 = A
            (PixelFormat::Rgb565, vec![0x00, 0xF8], red),
            (PixelFormat::Argb1555, vec![0xE0, 0x83], [0, 255, 0, 255]),
            (PixelFormat::Argb4444, vec![0x0F, 0xF0], [0, 0, 255, 255]),
            (PixelFormat::Xrgb8888, vec![1, 2, 3, 0], [3, 2, 1, 255]), // bytes B,G,R,X
            (PixelFormat::Argb8888, vec![1, 2, 3, 4], [3, 2, 1, 4]),   // bytes B,G,R,A
        ];
        for (fmt, texel, want) in cases {
            for (flag, tag) in [(SWZ, "swizzled"), (LIN, "linear")] {
                let img = image(fmt, TagKind::D2, [1, 1, 1], 0, flag);
                let desc = texture_desc(&img, 0).unwrap();
                let mut data = vec![0u8; desc.layout().unwrap().total_size.max(64)];
                data[..texel.len()].copy_from_slice(&texel);
                let levels = decode_bytes(&desc, &data).unwrap();
                assert_eq!(&levels[0].rgba[..4], &want, "{fmt:?} {tag}");
            }
        }
        // DXT: one block per 4x4 image.
        let dxt1 = RED_BLOCK.to_vec();
        let dxt3 = [vec![0x88u8; 8], RED_BLOCK.to_vec()].concat();
        let dxt5 = [vec![0x80, 0x80, 0, 0, 0, 0, 0, 0], RED_BLOCK.to_vec()].concat();
        for (fmt, data, a) in [(PixelFormat::Dxt1, dxt1, 255), (PixelFormat::Dxt3, dxt3, 0x88), (PixelFormat::Dxt5, dxt5, 0x80)] {
            let img = image(fmt, TagKind::D2, [4, 4, 1], 0, DXT);
            let desc = texture_desc(&img, 0).unwrap();
            let levels = decode_bytes(&desc, &data).unwrap();
            for px in levels[0].rgba.as_chunks::<4>().0.iter() {
                assert_eq!(*px, [255, 0, 0, a], "{fmt:?}");
            }
        }
    }
}
