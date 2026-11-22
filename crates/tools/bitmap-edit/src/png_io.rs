//! PNG in and out: 8-bit RGBA on the way out; any 8 or 16-bit PNG
//! (grey, grey and alpha, RGB, RGBA, palette) as 8-bit RGBA on the way in.

use crate::canvas::{MAX_SIZE, check_size};
use crate::error::Error;

fn png_err(e: impl std::fmt::Display) -> Error {
    Error::Png(e.to_string())
}

/// `rgba` (`width * height * 4` bytes) as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, Error> {
    check_size(width, height)?;
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(Error::Rect(format!(
            "{width}x{height} needs {} bytes, got {}",
            width as usize * height as usize * 4,
            rgba.len()
        )));
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(png_err)?;
        w.write_image_data(rgba).map_err(png_err)?;
        w.finish().map_err(png_err)?;
    }
    Ok(out)
}

/// A PNG as `(width, height, RGBA8)`.
pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), Error> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info().map_err(png_err)?;
    let (w, h) = (r.info().width, r.info().height);
    if w == 0 || h == 0 || w > MAX_SIZE || h > MAX_SIZE {
        return Err(Error::Size(format!(
            "{w}x{h} is not an image size the canvas takes (1 to {MAX_SIZE})"
        )));
    }
    let size = r
        .output_buffer_size()
        .ok_or_else(|| Error::Png("the image is too large".to_owned()))?;
    let mut buf = vec![0; size];
    let info = r.next_frame(&mut buf).map_err(png_err)?;
    buf.truncate(info.buffer_size());
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err(Error::Png("an indexed PNG was not expanded".to_owned()));
        }
    };
    if rgba.len() != w as usize * h as usize * 4 {
        return Err(Error::Png(format!(
            "decoded {} bytes for {w}x{h}",
            rgba.len()
        )));
    }
    Ok((w, h, rgba))
}
