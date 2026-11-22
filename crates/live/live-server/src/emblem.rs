//! The Live emblem URL, manifest lookup, and PNG compositor shared by native and wasm.

use std::collections::HashMap;
use std::io::Cursor;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use live_proto::game::Appearance;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const PREFIX: &str = "/art/emblem";
pub const MAX_MANIFEST_BYTES: usize = 4 << 20;
pub const MAX_MASK_BYTES: usize = 2 << 20;
const MAX_SIDE: u32 = 512;
const FG_COUNT: u8 = 64;
const BG_COUNT: u8 = 32;
const PALETTE_CAP: usize = 256;

// Bump when changing the pixels or encoding for the same inputs.
const FORMAT: &[u8] = b"live-emblem-png-v1";

const DEFAULT_PALETTE: [[u8; 3]; 18] = [
    [236, 236, 236],
    [110, 110, 110],
    [28, 28, 28],
    [190, 40, 34],
    [226, 106, 30],
    [240, 190, 40],
    [126, 150, 60],
    [40, 130, 60],
    [246, 150, 170],
    [130, 60, 150],
    [40, 150, 160],
    [110, 180, 230],
    [36, 70, 160],
    [130, 90, 200],
    [120, 80, 44],
    [200, 160, 110],
    [150, 30, 70],
    [70, 90, 70],
];

/// FNV-1a over UTF-16 code units, as in the client.
pub(crate) fn fnv(seed: &str, salt: u32) -> u32 {
    let mut h = 2166136261 ^ salt;
    for unit in seed.encode_utf16() {
        h = (h ^ u32::from(unit)).wrapping_mul(16777619);
    }
    h
}

fn choice(seed: &str, salt: u32) -> u32 {
    let mut h = fnv(seed, salt);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b3c6d);
    h ^ (h >> 12)
}

/// A relative URL usable before the browser has loaded the art manifest.
/// Missing appearance fields carry a seed-derived selector so each home can
/// apply the same palette and layer counts as the old client.
pub fn image_path(seed: &str, appearance: Option<Appearance>) -> String {
    let a = appearance.unwrap_or_default();
    let fields = [
        a.emblem_foreground.filter(|&v| v < FG_COUNT),
        a.emblem_background.filter(|&v| v < BG_COUNT),
        a.primary,
        a.secondary,
        a.tertiary,
        a.quaternary,
    ];
    let mut fields: Vec<_> = fields
        .into_iter()
        .enumerate()
        .map(|(i, value)| match value {
            Some(v) => v.to_string(),
            None => format!("s{:08x}", choice(seed, i as u32 + 1)),
        })
        .collect();
    if needs_seed(
        a.primary.is_none(),
        a.secondary.is_none(),
        a.tertiary.is_none(),
        a.quaternary.is_none(),
    ) {
        fields.push(format!("{:08x}{:08x}", choice(seed, 3), choice(seed, 5)));
    }
    format!("{PREFIX}/{}.png", fields.join("-"))
}

#[derive(Debug, Clone, Copy)]
enum Selector {
    Index(u8),
    Seed(u32),
    Rgb([u8; 3]),
}

impl Selector {
    fn parse(text: &str, colour: bool) -> Option<Self> {
        if let Some(hex) = text.strip_prefix('s') {
            return (hex.len() == 8 && lower_hex(hex))
                .then(|| u32::from_str_radix(hex, 16).ok())
                .flatten()
                .map(Self::Seed);
        }
        if let Some(hex) = text.strip_prefix('r') {
            return (colour && lower_hex(hex))
                .then(|| parse_rgb(hex))
                .flatten()
                .map(Self::Rgb);
        }
        if text.len() > 3
            || !text.bytes().all(|b| b.is_ascii_digit())
            || (text.len() > 1 && text.starts_with('0'))
        {
            return None;
        }
        text.parse().ok().map(Self::Index)
    }
}

fn parse_rgb(hex: &str) -> Option<[u8; 3]> {
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
    Some([n[1], n[2], n[3]])
}

fn lower_hex(hex: &str) -> bool {
    hex.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn needs_seed(primary: bool, secondary: bool, background: bool, background2: bool) -> bool {
    (!primary && (secondary || background || background2)) || (!background && background2)
}

#[derive(Debug)]
pub struct Spec {
    fields: [Selector; 6],
    seed: [u32; 2],
}

impl Spec {
    pub fn parse(path: &str) -> Option<Self> {
        let file = path
            .strip_prefix(PREFIX)?
            .strip_prefix('/')?
            .strip_suffix(".png")?;
        if file.len() > 6 * 9 + 5 + 17 {
            return None;
        }
        let mut parts = file.split('-');
        let mut fields = [Selector::Index(0); 6];
        for (i, field) in fields.iter_mut().enumerate() {
            *field = Selector::parse(parts.next()?, i >= 2)?;
            if matches!(
                (i, *field),
                (0, Selector::Index(FG_COUNT..)) | (1, Selector::Index(BG_COUNT..))
            ) {
                return None;
            }
        }
        let seeded = |i: usize| matches!(fields[i], Selector::Seed(_));
        let seed = if needs_seed(seeded(2), seeded(3), seeded(4), seeded(5)) {
            let hex = parts.next()?;
            if hex.len() != 16 || !lower_hex(hex) {
                return None;
            }
            [
                u32::from_str_radix(&hex[..8], 16).ok()?,
                u32::from_str_radix(&hex[8..], 16).ok()?,
            ]
        } else {
            [0, 0]
        };
        parts.next().is_none().then_some(Self { fields, seed })
    }
}

#[derive(Deserialize)]
pub struct ArtManifest {
    version: u32,
    images: HashMap<String, Image>,
    #[serde(default)]
    aliases: HashMap<String, String>,
    #[serde(default)]
    palette: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Image {
    file: String,
}

/// Files under a manifest's art directory and the resolved colours, ordered
/// foreground a/b, then background a/b.
pub struct Layers<'a> {
    pub fg: &'a str,
    pub bg: &'a str,
    colours: [[u8; 3]; 4],
}

impl Layers<'_> {
    pub fn colours(&self) -> [[u8; 3]; 4] {
        self.colours
    }

    pub fn revision(&self, bg: &[u8], fg: &[u8]) -> String {
        revision(&[bg, fg, self.colours.as_flattened()])
    }
}

impl ArtManifest {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return None;
        }
        let manifest: Self = serde_json::from_slice(bytes).ok()?;
        (manifest.version == 1).then_some(manifest)
    }

    fn file(&self, key: &str) -> Option<&str> {
        let image = self
            .aliases
            .get(key)
            .and_then(|target| self.images.get(target))
            .or_else(|| self.images.get(key))?;
        let file = image.file.as_str();
        // Extracted mask paths are relative PNGs. Use the same restriction
        // on disk and over HTTP, including against URL escapes.
        (file.len() <= 256
            && file.ends_with(".png")
            && file.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }))
        .then_some(file)
    }

    pub fn layers(&self, spec: &Spec) -> Option<Layers<'_>> {
        let layer = |prefix: &str, sel, max| {
            let index = match sel {
                Selector::Index(index) => index,
                Selector::Seed(n) => {
                    let count = (0..max)
                        .take_while(|i| self.file(&format!("{prefix}{i}")).is_some())
                        .count();
                    if count == 0 {
                        return None;
                    }
                    (n as usize % count) as u8
                }
                Selector::Rgb(_) => return None,
            };
            self.file(&format!("{prefix}{index}"))
        };
        let [fg, bg, primary, secondary, background, background2] = spec.fields;
        let fg = layer("emblem/fg/", fg, FG_COUNT)?;
        let bg = layer("emblem/bg/", bg, BG_COUNT)?;
        let palette: Vec<_> = self
            .palette
            .iter()
            .filter_map(|v| {
                let text = v.as_str()?.trim();
                parse_rgb(text.strip_prefix('#').unwrap_or(text))
            })
            .take(PALETTE_CAP)
            .collect();
        let palette = if palette.len() >= 2 {
            palette.as_slice()
        } else {
            &DEFAULT_PALETTE
        };
        let n = palette.len();
        // Seeded colours always step away from the original seeded primary
        // and background, even when telemetry overrides either of them.
        let [p, s, b, b2] = [primary, secondary, background, background2];
        let hash = |selector, fallback| match selector {
            Selector::Seed(h) => h as usize,
            _ => fallback as usize,
        };
        let p_seed = hash(p, spec.seed[0]) % n;
        let b_seed = (p_seed + 1 + hash(b, spec.seed[1]) % (n - 1)) % n;
        let colour = |sel, index| match sel {
            Selector::Index(i) => palette[i as usize % n],
            Selector::Rgb(rgb) => rgb,
            Selector::Seed(_) => palette[index],
        };
        let step = |sel, from| match sel {
            Selector::Seed(h) => (from + 1 + h as usize % (n - 1)) % n,
            _ => 0,
        };
        let colours = [
            colour(p, p_seed),
            colour(s, step(s, p_seed)),
            colour(b, b_seed),
            colour(b2, step(b2, b_seed)),
        ];
        Some(Layers { fg, bg, colours })
    }
}

/// Includes the version of the encoding, and either the immutable bucket
/// directory or the actual inputs on disk. Callers supply the input boundaries.
pub fn revision(parts: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(FORMAT);
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, thiserror::Error)]
#[error("invalid emblem revision query")]
pub struct InvalidRevisionQuery;

pub fn query_revision(query: Option<&str>) -> Result<Option<&str>, InvalidRevisionQuery> {
    let Some(query) = query else {
        return Ok(None);
    };
    let version = query
        .strip_prefix("v=")
        .filter(|v| v.len() == 32 && lower_hex(v));
    version.map(Some).ok_or(InvalidRevisionQuery)
}

pub fn redirect(path: &str, version: &str, cache_control: &str) -> Response {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            (header::LOCATION, format!("{path}?v={version}")),
            (header::CACHE_CONTROL, cache_control.into()),
        ],
    )
        .into_response()
}

pub fn error(status: StatusCode) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

pub fn png_response(png: Vec<u8>, cache_control: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, cache_control),
        ],
        png,
    )
        .into_response()
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("unsupported emblem mask")]
    Mask,
    #[error(transparent)]
    Decode(#[from] png::DecodingError),
    #[error(transparent)]
    Encode(#[from] png::EncodingError),
}

struct Mask {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    stride: usize,
}

fn decode(bytes: &[u8]) -> Result<Mask, ImageError> {
    if bytes.len() > MAX_MASK_BYTES {
        return Err(ImageError::Mask);
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: 8 << 20 });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let info = decoder.read_header_info()?;
    if !(1..=MAX_SIDE).contains(&info.width) || !(1..=MAX_SIDE).contains(&info.height) {
        return Err(ImageError::Mask);
    }
    let mut reader = decoder.read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or(ImageError::Mask)?];
    let info = reader.next_frame(&mut pixels)?;
    let stride = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return Err(ImageError::Mask),
    };
    reader.finish()?;
    Ok(Mask {
        width: info.width as usize,
        height: info.height as usize,
        pixels,
        stride,
    })
}

impl Mask {
    fn channels(&self, x: usize, y: usize) -> [u8; 2] {
        let i = (y * self.width + x) * self.stride;
        [self.pixels[i], self.pixels[i + 2]]
    }

    // The browser drew the foreground to the background's dimensions.
    fn sample(&self, x: usize, y: usize, w: usize, h: usize) -> [u8; 2] {
        if (self.width, self.height) == (w, h) {
            return self.channels(x, y);
        }
        let source = |at, size, dest| {
            ((at as f64 + 0.5) * size as f64 / dest as f64 - 0.5).clamp(0.0, (size - 1) as f64)
        };
        let sx = source(x, self.width, w);
        let sy = source(y, self.height, h);
        let (x0, y0) = (sx as usize, sy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (dx, dy) = (sx.fract(), sy.fract());
        let [tl, tr, bl, br] = [
            self.channels(x0, y0),
            self.channels(x1, y0),
            self.channels(x0, y1),
            self.channels(x1, y1),
        ];
        std::array::from_fn(|i| {
            let top = f64::from(tl[i]) * (1.0 - dx) + f64::from(tr[i]) * dx;
            let bottom = f64::from(bl[i]) * (1.0 - dx) + f64::from(br[i]) * dx;
            (top * (1.0 - dy) + bottom * dy).round() as u8
        })
    }
}

pub fn compose_png(bg: &[u8], fg: &[u8], colours: [[u8; 3]; 4]) -> Result<Vec<u8>, ImageError> {
    let bg = decode(bg)?;
    let fg = decode(fg)?;
    let [fa, fb, ba, bb] = colours;
    let mut rgba = Vec::with_capacity(bg.width * bg.height * 4);
    for y in 0..bg.height {
        for x in 0..bg.width {
            let [br, bbm] = bg.channels(x, y).map(|b| f64::from(b) / 255.0);
            let [fr, fbm] = fg
                .sample(x, y, bg.width, bg.height)
                .map(|b| f64::from(b) / 255.0);
            let keep = 1.0 - fr.max(fbm);
            for c in 0..3 {
                let under = (f64::from(ba[c]) * br + f64::from(bb[c]) * bbm).min(255.0);
                rgba.push(
                    (f64::from(fa[c]) * fr + f64::from(fb[c]) * fbm + under * keep).round() as u8,
                );
            }
            rgba.push(255);
        }
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, bg.width as u32, bg.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgba)?;
    writer.finish()?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Golden {
        manifest: ArtManifest,
        selections: Vec<Selection>,
        pixels: Vec<Pixels>,
    }

    #[derive(Deserialize)]
    struct Selection {
        path: String,
        fg: u8,
        bg: u8,
        colours: [[u8; 3]; 4],
    }

    #[derive(Deserialize)]
    struct Pixels {
        width: u32,
        height: u32,
        bg: Vec<u8>,
        fg: Vec<u8>,
        colours: [[u8; 3]; 4],
        expected: Vec<u8>,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("../../../../web/test/fixtures/emblem.json")).unwrap()
    }

    fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
        png
    }

    #[test]
    fn emblem_pixels_match_the_typescript_compositor() {
        for Pixels {
            width,
            height,
            bg,
            fg,
            colours,
            expected,
        } in golden().pixels
        {
            let bg = encode(width, height, &bg);
            let fg = encode(width, height, &fg);
            let png = compose_png(&bg, &fg, colours).unwrap();
            assert_eq!(png, compose_png(&bg, &fg, colours).unwrap());
            let got = decode(&png).unwrap();
            assert_eq!(
                (got.width as u32, got.height as u32, got.stride),
                (width, height, 4)
            );
            assert_eq!(got.pixels, expected);
        }
    }

    #[test]
    fn choices_match_the_typescript_palette_and_seed() {
        let Golden {
            manifest,
            selections,
            ..
        } = golden();
        for Selection {
            path,
            fg,
            bg,
            colours,
        } in selections
        {
            let spec = Spec::parse(&path).unwrap();
            let layers = manifest.layers(&spec).unwrap();
            assert_eq!(layers.fg, format!("emblem/fg/{fg}.png"), "{path}");
            assert_eq!(layers.bg, format!("emblem/bg/{bg}.png"), "{path}");
            assert_eq!(layers.colours(), colours, "{path}");
        }
    }

    #[test]
    fn scales_foreground_to_background_size_and_bounds_masks() {
        let bg = encode(3, 1, &[0; 12]);
        let fg = encode(2, 1, &[0, 0, 0, 0, 255, 0, 0, 0]);
        let colours = [[255, 0, 0], [0; 3], [0; 3], [0; 3]];
        let png = compose_png(&bg, &fg, colours).unwrap();
        assert_eq!(
            decode(&png).unwrap().pixels,
            [0, 0, 0, 255, 128, 0, 0, 255, 255, 0, 0, 255]
        );

        let too_wide = encode(MAX_SIDE + 1, 1, &vec![0; (MAX_SIDE as usize + 1) * 4]);
        assert!(matches!(
            compose_png(&too_wide, &fg, colours),
            Err(ImageError::Mask)
        ));
        assert!(compose_png(b"not a PNG", &fg, colours).is_err());
    }

    #[test]
    fn rejects_malformed_or_noncanonical_specs() {
        for file in [
            "64-1-2-3-4-5.png",
            "0-32-2-3-4-5.png",
            "0-1-256-3-4-5.png",
            "0-1-02-3-4-5.png",
            "0-1-r00000G-3-4-5.png",
            "0-1-s0000ABCD-3-4-5.png",
            "0-1-2-3-4-5-6.png",
            "0-1-2-3-4-s12345678.png",
            "0-1-2-3-4-5.jpg",
            "../0-1-2-3-4-5.png",
            "0-1-2-3-4.png",
        ] {
            assert!(Spec::parse(&format!("{PREFIX}/{file}")).is_none(), "{file}");
        }
        assert!(query_revision(Some("v=00000000000000000000000000000000")).is_ok());
        assert!(query_revision(Some("v=abcd")).is_err());
        assert!(query_revision(Some("other=1")).is_err());
    }

    #[test]
    fn checks_manifest_paths_and_resolves_aliases() {
        let manifest = ArtManifest::parse(
            br##"{
            "version": 1, "images": {
                "red": { "file": "layers/red.png" },
                "emblem/bg/0": { "file": "layers/blue.png" },
                "emblem/bg/1": { "file": "../bad.png" },
                "emblem/bg/2": { "file": "a/%2fexample.png" }
            },
            "aliases": { "emblem/fg/0": "red" }
        }"##,
        )
        .unwrap();
        let spec = |file: &str| Spec::parse(&format!("{PREFIX}/{file}")).unwrap();
        let layers = manifest.layers(&spec("0-0-0-1-rffaaff-3.png")).unwrap();
        assert_eq!(layers.fg, "layers/red.png");
        assert_eq!(layers.bg, "layers/blue.png");
        assert_eq!(
            layers.colours(),
            [
                DEFAULT_PALETTE[0],
                DEFAULT_PALETTE[1],
                [255, 170, 255],
                DEFAULT_PALETTE[3]
            ]
        );
        assert!(manifest.layers(&spec("0-1-0-1-2-3.png")).is_none());
        assert!(manifest.layers(&spec("0-2-0-1-2-3.png")).is_none());
    }
}
