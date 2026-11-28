//! A dev raster as a `bitm`: a mip chain boxed down from it, every level
//! DXT5 with opaque alpha, written as one image through the Halo 1 bitmap
//! converter's tag writer (`h1_assets::bitmaps::convert::convert_with`), as
//! the pogo stick's atlas and the stage's textures are
//! (`content_kit::texture`). The chain is kept apart from the tag so a process
//! encodes it once ([`crate::dev::encoded`]).

use anyhow::{Result, ensure};
use h1_assets::bitmaps::convert::convert_with;
use h1_assets::bitmaps::decode::texture_desc;
use h1_assets::bitmaps::mask::{INTERPOLATED_ALPHA, encode_dxt5};
use h1_assets::models::Converted;
use h1map::bitmap::{
    Bitmap, FLAG_COMPRESSED, FLAG_POWER_OF_TWO, IMAGE_LEN, Image, ImageKind, PixelFormat,
};
use content_kit::texture::{Rgba, halve, mip_count};

/// The bitmap header's usage word for an ordinary colour map.
const USAGE_DEFAULT: u16 = 1;

/// One encoded image: its size and its whole DXT5 chain, base level first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub width: u32,
    pub height: u32,
    pub chain: Vec<u8>,
}

fn header(width: u32, height: u32) -> Image {
    Image {
        width: width as u16,
        height: height as u16,
        depth: 1,
        kind: ImageKind::D2,
        format: PixelFormat::Dxt5,
        flags: FLAG_POWER_OF_TWO | FLAG_COMPRESSED,
        registration: [(width / 2) as i16, (height / 2) as i16],
        mip_count: mip_count(width, height),
        pixel_offset: 0,
        pixel_size: 0,
        raw: [0; IMAGE_LEN],
    }
}

/// The length of the chain of a `width` by `height` image.
pub fn chain_len(width: u32, height: u32) -> Result<usize> {
    Ok(texture_desc(&header(width, height))?.layout()?.total_size)
}

/// Encodes `image`, whose sides are powers of two from 4 to 2048.
pub fn encode(image: &dev_texture::Image) -> Result<Encoded> {
    let (w, h) = (image.width, image.height);
    ensure!(
        w.is_power_of_two() && h.is_power_of_two() && (4..=2048).contains(&w) && (4..=2048).contains(&h),
        "a raster of {w}x{h}: both sides are powers of two from 4 to 2048"
    );
    let layout = texture_desc(&header(w, h))?.layout()?;
    let mut chain = vec![0u8; layout.total_size];
    let mut level = Rgba {
        width: w,
        height: h,
        pixels: image.rgba.clone(),
    };
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
    Ok(Encoded {
        width: w,
        height: h,
        chain,
    })
}

/// The `bitm` tag of an encoded image.
pub fn bitmap(encoded: &Encoded) -> Result<Converted> {
    let (w, h) = (encoded.width, encoded.height);
    let mut image = header(w, h);
    image.pixel_size = encoded.chain.len() as u32;
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
    let mut chain = Some(encoded.chain.clone());
    let (converted, _) = convert_with(&header, |_| Ok(chain.take().unwrap_or_default()))?;
    Ok(converted)
}
