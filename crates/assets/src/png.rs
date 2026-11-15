//! PNG output for decoded RGBA8 images.

use std::io::{self, Write};

/// Writes `width x height` RGBA8 pixels as a PNG. Returns `InvalidInput`
/// (never panics) if either dimension is zero, the size overflows, or the
/// buffer length does not equal `width * height * 4`.
pub fn write_rgba(out: impl Write, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    write(out, width, height, rgba, ::png::Compression::Balanced)
}

/// [`write_rgba`] with fast compression: several times quicker to encode,
/// somewhat larger files. Equally deterministic.
pub fn write_rgba_fast(out: impl Write, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    write(out, width, height, rgba, ::png::Compression::Fast)
}

fn write(
    out: impl Write,
    width: u32,
    height: u32,
    rgba: &[u8],
    compression: ::png::Compression,
) -> io::Result<()> {
    let bad = |m: &'static str| io::Error::new(io::ErrorKind::InvalidInput, m);
    if width == 0 || height == 0 {
        return Err(bad("zero-sized image"));
    }
    let need = (width as u64)
        .checked_mul(height as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| bad("image size overflows"))?;
    if need != rgba.len() as u64 {
        return Err(bad("pixel buffer length does not match dimensions"));
    }
    let mut enc = ::png::Encoder::new(out, width, height);
    enc.set_color(::png::ColorType::Rgba);
    enc.set_depth(::png::BitDepth::Eight);
    enc.set_compression(compression);
    let mut w = enc.write_header().map_err(io::Error::other)?;
    w.write_image_data(rgba).map_err(io::Error::other)?;
    w.finish().map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::write_rgba;

    /// Decodes with the `png` crate, which verifies chunk CRCs and the zlib
    /// Adler-32 while reading, and returns (width, height, pixels).
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let dec = ::png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, ::png::ColorType::Rgba);
        assert_eq!(info.bit_depth, ::png::BitDepth::Eight);
        buf.truncate(info.buffer_size());
        r.finish().unwrap();
        (info.width, info.height, buf)
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect()
    }

    #[test]
    fn round_trips_pixels() {
        let px = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut v = Vec::new();
        write_rgba(&mut v, 2, 1, &px).unwrap();
        assert_eq!(decode(&v), (2, 1, px.to_vec()));
        let mut fast = Vec::new();
        super::write_rgba_fast(&mut fast, 2, 1, &px).unwrap();
        assert_eq!(decode(&fast), (2, 1, px.to_vec()));
    }

    #[test]
    fn round_trips_a_large_image() {
        let (w, h) = (2048u32, 1024u32);
        let px = pattern(w as usize * h as usize * 4);
        let mut v = Vec::new();
        write_rgba(&mut v, w, h, &px).unwrap();
        let (dw, dh, back) = decode(&v);
        assert_eq!((dw, dh), (w, h));
        assert!(back == px);
    }

    #[test]
    fn a_corrupted_stream_is_detected_by_the_checks() {
        let mut v = Vec::new();
        write_rgba(&mut v, 4, 4, &pattern(64)).unwrap();
        let n = v.len();
        v[n - 20] ^= 0xff; // inside IDAT data / CRC
        let dec = ::png::Decoder::new(std::io::Cursor::new(&v[..]));
        let failed = dec.read_info().and_then(|mut r| {
            let mut b = vec![0; r.output_buffer_size().unwrap()];
            r.next_frame(&mut b).map(|_| ())
        });
        assert!(failed.is_err());
    }

    #[test]
    fn bad_buffers_return_errors_instead_of_panicking() {
        let mut v = Vec::new();
        assert!(write_rgba(&mut v, 2, 2, &[0; 4]).is_err());
        assert!(write_rgba(&mut v, 0, 2, &[]).is_err());
        assert!(write_rgba(&mut v, u32::MAX, u32::MAX, &[0; 4]).is_err());
    }
}
