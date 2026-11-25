//! A style's atlas as a Halo 2 `bitm`: the PNG decoded, a mip chain boxed
//! down from it, every level DXT5 (alpha opaque), written as one image
//! through the Halo 1 bitmap converter's tag writer
//! (`h1_assets::bitmaps::convert::convert_with`), which lays an image out
//! as retail's are: power of two, compressed, cached, LOD 0 one raw
//! resource holding the whole chain.

use anyhow::{Context, Result, bail, ensure};
use h1_assets::bitmaps::convert::convert_with;
use h1_assets::bitmaps::decode::texture_desc;
use h1_assets::bitmaps::mask::{INTERPOLATED_ALPHA, encode_dxt5};
use h1_assets::models::Converted;
use h1map::bitmap::{
    Bitmap, FLAG_COMPRESSED, FLAG_POWER_OF_TWO, IMAGE_LEN, Image, ImageKind, PixelFormat,
};

/// The bitmap header's usage word for an ordinary colour map.
const USAGE_DEFAULT: u16 = 1;
/// The smallest side a mip level is boxed down to: one DXT block.
const SMALLEST_SIDE: u32 = 4;

/// The first bytes of a Git LFS pointer file.
const LFS_POINTER: &[u8] = b"version https://git-lfs";

/// Decoded RGBA rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// A PNG file's pixels as RGBA. A Git LFS pointer (a checkout made without
/// LFS) is named as one.
pub fn decode(png_bytes: &[u8]) -> Result<Rgba> {
    if png_bytes.starts_with(LFS_POINTER) {
        bail!("the atlas is a Git LFS pointer: run `git lfs pull` and build again");
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().context("the atlas PNG")?;
    let (width, height) = (reader.info().width, reader.info().height);
    let size = reader
        .output_buffer_size()
        .context("the atlas PNG is too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).context("the atlas PNG")?;
    buf.truncate(info.buffer_size());
    let pixels: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => bail!("the atlas PNG is {other:?}, want RGB or RGBA"),
    };
    ensure!(
        pixels.len() == width as usize * height as usize * 4,
        "the atlas PNG decoded to {} bytes for {width}x{height}",
        pixels.len()
    );
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

/// The next mip level: each texel the mean of the four above it.
pub fn halve(src: &Rgba) -> Rgba {
    let (w, h) = ((src.width / 2).max(1), (src.height / 2).max(1));
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    let at = |x: u32, y: u32, c: usize| {
        let (x, y) = (x.min(src.width - 1), y.min(src.height - 1));
        u32::from(src.pixels[((y * src.width + x) * 4) as usize + c])
    };
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let sum = at(2 * x, 2 * y, c)
                    + at(2 * x + 1, 2 * y, c)
                    + at(2 * x, 2 * y + 1, c)
                    + at(2 * x + 1, 2 * y + 1, c);
                pixels.push(((sum + 2) / 4) as u8);
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        pixels,
    }
}

/// How many mip levels below the base an image of this size gets: halved
/// until its shorter side is one DXT block.
pub fn mip_count(width: u32, height: u32) -> u16 {
    let mut n = 0;
    let mut side = width.min(height);
    while side > SMALLEST_SIDE {
        side /= 2;
        n += 1;
    }
    n
}

/// The `bitm` tag holding `atlas`, sides powers of two.
pub fn bitmap(atlas: &Rgba) -> Result<Converted> {
    let (w, h) = (atlas.width, atlas.height);
    ensure!(
        w.is_power_of_two() && h.is_power_of_two() && w >= 4 && h >= 4 && w <= 2048 && h <= 2048,
        "an atlas of {w}x{h}: both sides are powers of two from 4 to 2048"
    );
    let mut image = Image {
        width: w as u16,
        height: h as u16,
        depth: 1,
        kind: ImageKind::D2,
        format: PixelFormat::Dxt5,
        flags: FLAG_POWER_OF_TWO | FLAG_COMPRESSED,
        registration: [(w / 2) as i16, (h / 2) as i16],
        mip_count: mip_count(w, h),
        pixel_offset: 0,
        pixel_size: 0,
        raw: [0; IMAGE_LEN],
    };
    let layout = texture_desc(&image)?.layout()?;
    let mut chain = vec![0u8; layout.total_size];
    let mut level = atlas.clone();
    for l in &layout.levels {
        while level.width > l.width || level.height > l.height {
            level = halve(&level);
        }
        ensure!(
            (level.width, level.height) == (l.width, l.height),
            "mip level {} is {}x{}, the chain has {}x{}",
            l.level,
            l.width,
            l.height,
            level.width,
            level.height
        );
        let opaque = vec![255u8; (level.width * level.height) as usize];
        let blocks = encode_dxt5(level.width, level.height, &level.pixels, &opaque);
        let n = blocks.len().min(l.size);
        chain[l.offset..l.offset + n].copy_from_slice(&blocks[..n]);
    }
    image.pixel_size = chain.len() as u32;
    let header = Bitmap {
        kind: 0,
        encoding: INTERPOLATED_ALPHA,
        usage: USAGE_DEFAULT,
        flags: 0,
        detail_fade: 0.0,
        sharpen: 0.0,
        bump_height: 0.0,
        sprite_budget: [0, 0],
        color_plate: [w as u16, h as u16],
        blur: 0.0,
        alpha_bias: 0.0,
        mipmap_count: 0,
        sprite_usage: 0,
        sprite_spacing: 0,
        sequences: Vec::new(),
        images: vec![image],
    };
    let mut chain = Some(chain);
    let (converted, _) = convert_with(&header, |_| Ok(chain.take().unwrap_or_default()))?;
    Ok(converted)
}
