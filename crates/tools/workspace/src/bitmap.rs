//! The `bitmap` builder: a PNG as a `bitm` (docs/editor/studio-redesign.md,
//! "Contents").
//!
//! The tag is one linear `ARGB8888` image with no mips, written by
//! [`tags::rgba_bitmap::install`] on a template with the header and image
//! settings of an interface picture (the menu's level pictures). It is
//! sized for interface and HUD art; a texture with mips or another format
//! is not this builder's.
use std::io::Cursor;

use map::FourCC;
use map::build::Block;
use tags::rgba_bitmap::{FORMAT_ARGB8888, HEADER_LEN, IMAGE_LEN, Template, install};

use crate::build::{Cx, Failure};
use crate::builders::Job;

const BITM: FourCC = FourCC(*b"bitm");

/// A PNG as 8-bit RGBA.
pub struct Picture {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

/// Decode `png`: any bit depth and colour type, to RGBA8.
pub fn decode(png: &[u8]) -> Result<Picture, String> {
    let mut decoder = png::Decoder::new(Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("not a PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("the PNG is too large")?];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("the PNG is damaged: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 {
        return Err("the PNG is empty".into());
    }
    let (width, height) = (
        u16::try_from(w).map_err(|_| format!("{w} pixels wide: a bitmap holds up to 65535"))?,
        u16::try_from(h).map_err(|_| format!("{h} pixels high: a bitmap holds up to 65535"))?,
    );
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("an indexed PNG was not expanded".into()),
    };
    if rgba.len() != w * h * 4 {
        return Err("the PNG decoded to the wrong size".into());
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

/// The settings every bitmap this builder writes starts from.
fn template() -> Template {
    let mut image = Block::zeroed(IMAGE_LEN);
    image
        .put_u32(0, BITM.as_u32())
        .put_u16(0xc, FORMAT_ARGB8888)
        .put_u16(0xe, 0x290);
    image.bytes[0x16] = 11;
    image.bytes[0x17] = 16;
    let mut header = Block::zeroed(HEADER_LEN);
    header.put_u16(0, 4).put_u16(2, 4).put_u16(4, 1);
    Template {
        header: header.bytes,
        image: image.bytes,
    }
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    if job.item.tag.as_deref().is_none_or(str::is_empty) {
        return Err("a bitmap item needs `tag`, the path of the bitmap it adds".into());
    }
    decode(job.source)?;
    Ok(())
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let path = job
        .item
        .tag
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or("a bitmap item needs `tag`, the path of the bitmap it adds")?;
    let key = Cx::key(BITM, path);
    cx.check_free(&key)?;
    let pic = decode(job.source)?;
    let id = install(
        cx.set,
        &template(),
        &key.path,
        pic.width,
        pic.height,
        &pic.rgba,
    )
    .map_err(|e| format!("writing bitmap {key}: {e}"))?;
    cx.note_added(&key, id);
    Ok(())
}
