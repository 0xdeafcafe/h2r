//! `render.compare` (docs/graphics/framescene.md, section 9.2): two images
//! compared pixel by pixel, as a pure function over RGBA ([`compare`]) and
//! over PNG files ([`compare_files`], which `h2 agent frame compare` runs
//! with no game).
//!
//! The colour channels are compared; alpha is not (frames are written
//! opaque). A pixel differs when any channel is off by more than
//! [`CompareReply::NOISE`]; the bar is [`CompareReply::meets_bar`].

use std::fmt;
use std::path::Path;

use agent_proto::{CompareParams, CompareReply, RpcError, code};
use serde_json::json;

/// Straight RGBA8, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// `width` by `height` of one colour.
    pub fn filled(width: u32, height: u32, px: [u8; 4]) -> Rgba {
        Rgba {
            width,
            height,
            pixels: px.repeat(width as usize * height as usize),
        }
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * 4
    }

    /// Pixel `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = self.index(x, y);
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn set(&mut self, x: u32, y: u32, px: [u8; 4]) {
        let i = self.index(x, y);
        self.pixels[i..i + 4].copy_from_slice(&px);
    }
}

/// Why two images could not be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareError {
    /// The images are not the same size.
    SizeMismatch { a: [u32; 2], b: [u32; 2] },
    /// A file could not be read or decoded.
    Read { path: String, error: String },
    /// The difference image could not be written.
    Write { path: String, error: String },
}

impl fmt::Display for CompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompareError::SizeMismatch { a, b } => write!(
                f,
                "size mismatch: a is {}x{}, b is {}x{}",
                a[0], a[1], b[0], b[1]
            ),
            CompareError::Read { path, error } => write!(f, "reading {path}: {error}"),
            CompareError::Write { path, error } => write!(f, "writing {path}: {error}"),
        }
    }
}

impl std::error::Error for CompareError {}

impl CompareError {
    /// As the agent answers it: invalid params, with `data.error` naming
    /// the kind (`size_mismatch`, `read`, `write`).
    pub fn rpc(&self) -> RpcError {
        let mut e = RpcError::new(code::INVALID_PARAMS, self.to_string());
        e.data = Some(match self {
            CompareError::SizeMismatch { a, b } => {
                json!({"error": "size_mismatch", "a": a, "b": b})
            }
            CompareError::Read { path, .. } => json!({"error": "read", "path": path}),
            CompareError::Write { path, .. } => json!({"error": "write", "path": path}),
        });
        e
    }
}

/// Compares `a` with `b`; with `want_diff`, also the difference image:
/// pixels that differ in red (brighter the further off), pixels off by no
/// more than the noise in dark blue, the rest `a` at a quarter brightness.
pub fn compare(
    a: &Rgba,
    b: &Rgba,
    want_diff: bool,
) -> Result<(CompareReply, Option<Rgba>), CompareError> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(CompareError::SizeMismatch {
            a: [a.width, a.height],
            b: [b.width, b.height],
        });
    }
    let (w, h) = (a.width, a.height);
    let mut diff = want_diff.then(|| Rgba::filled(w, h, [0, 0, 0, 0xff]));
    let mut differing = 0u64;
    let mut max_channel = 0u8;
    let mut total = 0u64;
    let mut first = None;
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (a.get(x, y), b.get(x, y));
            let mut off = 0u8;
            for c in 0..3 {
                let d = pa[c].abs_diff(pb[c]);
                total += u64::from(d);
                off = off.max(d);
            }
            max_channel = max_channel.max(off);
            let differs = off > CompareReply::NOISE;
            if differs {
                differing += 1;
                first.get_or_insert([x, y]);
            }
            if let Some(d) = &mut diff {
                let px = if differs {
                    [64u8.saturating_add(off.saturating_mul(4)), 0, 0, 0xff]
                } else if off > 0 {
                    [0, 0, 96, 0xff]
                } else {
                    [pa[0] / 4, pa[1] / 4, pa[2] / 4, 0xff]
                };
                d.set(x, y, px);
            }
        }
    }
    let pixels = u64::from(w) * u64::from(h);
    let channels = pixels * 3;
    let mean = if channels == 0 {
        0.0
    } else {
        total as f64 / channels as f64
    };
    Ok((
        CompareReply {
            size: [w, h],
            differing,
            max_channel,
            mean,
            first,
            within_bar: CompareReply::meets_bar(max_channel, differing, pixels),
            diff: None,
        },
        diff,
    ))
}

/// Decodes a PNG of any colour type into RGBA8.
pub fn read_png(path: &Path) -> Result<Rgba, CompareError> {
    let err = |e: String| CompareError::Read {
        path: path.display().to_string(),
        error: e,
    };
    let bytes = std::fs::read(path).map_err(|e| err(e.to_string()))?;
    decode_png(&bytes).map_err(err)
}

/// [`read_png`] over bytes.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("image too large")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{:?} after decoding, expected 8 bits", info.bit_depth));
    }
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 0xff])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 0xff]).collect(),
        other => return Err(format!("{other:?} after decoding, expected RGBA")),
    };
    Ok(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Writes RGBA8 as a PNG.
pub fn write_png(path: &Path, img: &Rgba) -> Result<(), CompareError> {
    let err = |e: String| CompareError::Write {
        path: path.display().to_string(),
        error: e,
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
    }
    let f = std::fs::File::create(path).map_err(|e| err(e.to_string()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&img.pixels))
        .map_err(|e| err(e.to_string()))
}

/// `render.compare` over files: reads both, compares them, and writes the
/// difference image at `diff` when asked.
pub fn compare_files(p: &CompareParams) -> Result<CompareReply, CompareError> {
    let a = read_png(Path::new(&p.a))?;
    let b = read_png(Path::new(&p.b))?;
    let (mut reply, image) = compare(&a, &b, p.diff.is_some())?;
    if let (Some(path), Some(image)) = (&p.diff, image) {
        write_png(Path::new(path), &image)?;
        reply.diff = Some(path.clone());
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREY: [u8; 4] = [100, 100, 100, 0xff];

    fn pair(w: u32, h: u32) -> (Rgba, Rgba) {
        (Rgba::filled(w, h, GREY), Rgba::filled(w, h, GREY))
    }

    #[test]
    fn equal_images_differ_nowhere() {
        let (a, b) = pair(8, 4);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!(r.size, [8, 4]);
        assert_eq!((r.differing, r.max_channel, r.mean), (0, 0, 0.0));
        assert_eq!(r.first, None);
        assert!(r.within_bar);
    }

    #[test]
    fn one_pixel_off_is_found_where_it_is() {
        let (a, mut b) = pair(8, 4);
        b.set(5, 2, [100, 140, 100, 0xff]);
        let (r, diff) = compare(&a, &b, true).unwrap();
        assert_eq!(r.differing, 1);
        assert_eq!(r.max_channel, 40);
        assert_eq!(r.first, Some([5, 2]));
        assert!((r.mean - 40.0 / (8.0 * 4.0 * 3.0)).abs() < 1e-12);
        assert!(!r.within_bar, "40 is over 8");
        let diff = diff.unwrap();
        assert_eq!(diff.get(5, 2)[0], 64 + 160);
        assert_eq!(diff.get(0, 0), [25, 25, 25, 0xff], "a, dimmed");
    }

    #[test]
    fn off_by_two_does_not_count_as_differing() {
        let (a, mut b) = pair(4, 4);
        b.set(1, 1, [102, 98, 100, 0xff]);
        let (r, diff) = compare(&a, &b, true).unwrap();
        assert_eq!((r.differing, r.max_channel), (0, 2));
        assert_eq!(r.first, None);
        assert!(r.mean > 0.0);
        assert_eq!(diff.unwrap().get(1, 1), [0, 0, 96, 0xff], "noise, in blue");
        b.set(1, 1, [103, 100, 100, 0xff]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!((r.differing, r.max_channel), (1, 3));
    }

    #[test]
    fn alpha_is_not_compared() {
        let (a, mut b) = pair(2, 2);
        b.set(0, 0, [100, 100, 100, 0]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!((r.differing, r.max_channel), (0, 0));
    }

    #[test]
    fn a_size_mismatch_is_a_named_error() {
        let a = Rgba::filled(4, 4, GREY);
        let b = Rgba::filled(4, 5, GREY);
        let e = compare(&a, &b, false).unwrap_err();
        assert_eq!(
            e,
            CompareError::SizeMismatch {
                a: [4, 4],
                b: [4, 5]
            }
        );
        let rpc = e.rpc();
        assert_eq!(rpc.code, code::INVALID_PARAMS);
        assert_eq!(rpc.data.unwrap()["error"], "size_mismatch");
    }

    #[test]
    fn the_bar_max_channel_8_passes_and_9_fails() {
        // 100 x 100: one differing pixel is exactly 0.01%.
        let (a, mut b) = pair(100, 100);
        b.set(0, 0, [108, 100, 100, 0xff]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!((r.differing, r.max_channel), (1, 8));
        assert!(r.within_bar);
        b.set(0, 0, [109, 100, 100, 0xff]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!(r.max_channel, 9);
        assert!(!r.within_bar);
    }

    #[test]
    fn the_bar_differing_at_exactly_a_hundredth_of_a_percent_passes_and_over_fails() {
        let (a, mut b) = pair(100, 100);
        b.set(3, 3, [104, 100, 100, 0xff]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!(r.differing, 1, "1 in 10000");
        assert!(r.within_bar);
        b.set(4, 3, [104, 100, 100, 0xff]);
        let (r, _) = compare(&a, &b, false).unwrap();
        assert_eq!(r.differing, 2, "2 in 10000");
        assert!(!r.within_bar);
    }

    #[test]
    fn files_round_trip_and_the_difference_image_is_written() {
        let dir = std::env::temp_dir().join(format!(
            "agent-compare-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let (a, mut b) = pair(6, 3);
        b.set(2, 1, [0, 0, 0, 0xff]);
        let (pa, pb, pd) = (dir.join("a.png"), dir.join("b.png"), dir.join("d/diff.png"));
        write_png(&pa, &a).unwrap();
        write_png(&pb, &b).unwrap();
        assert_eq!(read_png(&pa).unwrap(), a);
        let p = CompareParams {
            a: pa.display().to_string(),
            b: pb.display().to_string(),
            diff: Some(pd.display().to_string()),
        };
        let r = compare_files(&p).unwrap();
        assert_eq!((r.differing, r.max_channel, r.first), (1, 100, Some([2, 1])));
        assert_eq!(r.diff.as_deref(), Some(p.diff.as_deref().unwrap()));
        let d = read_png(&pd).unwrap();
        assert_eq!((d.width, d.height), (6, 3));
        let missing = CompareParams {
            a: dir.join("nope.png").display().to_string(),
            ..p
        };
        assert!(matches!(
            compare_files(&missing),
            Err(CompareError::Read { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
