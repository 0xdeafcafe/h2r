//! Text from the game's glyph cache at a render scale above 1
//! (`crate::glyph`, docs/high-resolution.md "Text").
//!
//! A synthetic glyph cache (a 256x256 `LIN_A4R4G4B4` texture the guest's
//! glyph-cache pointer names) holds a few shapes rasterised here the way
//! the game's fonts are: white, with 3-bit coverage alpha. A screen-aligned
//! quad draws its top-left 64x64 texels 1:1 over the 64x64 back buffer,
//! as the game draws a string. At 2x, 3x and 4x the view's edges must
//! follow the true outline (no stair steps, about a host pixel wide); at
//! 1x, and in the reference at every scale, the image must be the one
//! drawn with smoothing off. GPU tests; they skip without an adapter.
//!
//! `H2_FONT_HD_PNG=<dir>` writes the 4x image before (smoothing off, the
//! guest's bilinear filter) and after as `font-hd-before.png` and
//! `font-hd-after.png`, composited over a dark blue.

mod common;

use common::*;
use d3d::scaled::ScaledBackend;
use d3d::{Backend, Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

/// The glyph cache's data (physical) and the bitmap that names it.
const GLYPHS: u32 = 0x0040_0000;
const BITMAP: u32 = MISC + 0x6000;
const SIDE: u32 = 256;
/// Texels drawn: the top-left `DRAWN x DRAWN`, onto the whole target.
const DRAWN: f32 = 64.0;

const TSS_MAGFILTER: u32 = 3;
const TSS_MINFILTER: u32 = 4;
const TEXF_POINT: u32 = 1;
const TEXF_LINEAR: u32 = 2;

// ---------------------------------------------------------------------
// The synthetic glyphs, in texel coordinates.

fn capsule(p: (f64, f64), a: (f64, f64), b: (f64, f64), r: f64) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (ex, ey) = (p.0 - a.0 - t * dx, p.1 - a.1 - t * dy);
    ex * ex + ey * ey <= r * r
}

/// The shallow stroke the stair-step check walks along.
const STROKE: ((f64, f64), (f64, f64), f64) = ((33.0, 7.0), (61.0, 20.0), 2.6);

fn inside(x: f64, y: f64) -> bool {
    // A ring.
    let (rx, ry) = (x - 16.0, y - 16.0);
    let d2 = rx * rx + ry * ry;
    if (7.5 * 7.5..=12.5 * 12.5).contains(&d2) {
        return true;
    }
    // A shallow, thick stroke.
    if capsule((x, y), STROKE.0, STROKE.1, STROKE.2) {
        return true;
    }
    // An "A": two legs and a bar.
    if capsule((x, y), (6.0, 60.0), (16.0, 36.0), 1.7)
        || capsule((x, y), (16.0, 36.0), (26.0, 60.0), 1.7)
        || (10.5..21.5).contains(&x) && (49.0..51.8).contains(&y)
    {
        return true;
    }
    // A thin, light stroke and a dot.
    capsule((x, y), (36.0, 60.0), (61.0, 34.0), 0.55) || {
        let (dx, dy) = (x - 42.0, y - 30.0);
        dx * dx + dy * dy <= 2.2 * 2.2
    }
}

/// Coverage of the square `[x0, x0 + side) x [y0, y0 + side)`, `n x n`
/// samples.
fn coverage(x0: f64, y0: f64, side: f64, n: u32) -> f64 {
    let mut hits = 0;
    for j in 0..n {
        for i in 0..n {
            let x = x0 + (i as f64 + 0.5) / n as f64 * side;
            let y = y0 + (j as f64 + 0.5) / n as f64 * side;
            hits += inside(x, y) as u32;
        }
    }
    hits as f64 / (n * n) as f64
}

/// The cache's texels: white, alpha the 3-bit coverage widened to 4 bits
/// as the game's decoder does (`(a << 1) | (a & 1)`, docs/fonts.md).
fn glyph_texels() -> Vec<u8> {
    let mut out = Vec::with_capacity((SIDE * SIDE * 2) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let c = coverage(x as f64, y as f64, 1.0, 16);
            let a3 = (c * 7.0).round() as u16;
            let a4 = (a3 << 1) | (a3 & 1);
            out.extend_from_slice(&((a4 << 12) | 0x0FFF).to_le_bytes());
        }
    }
    out
}

// ---------------------------------------------------------------------
// The guest side.

/// The glyph cache at `TEX_HEADER`, and the bitmap at `BITMAP` naming it
/// (when `named`) through the global `crate::glyph` reads.
fn glyph_cache(m: &mut SparseMemory, named: bool) {
    let bytes = glyph_texels();
    m.map(0x8000_0000 | GLYPHS, bytes.len() as u32);
    m.write(0x8000_0000 | GLYPHS, &bytes).unwrap();
    let format = (2 << 4) | (0x1D << 8) | (1 << 16); // LIN_A4R4G4B4, 1 level
    let size = (SIDE - 1) | ((SIDE - 1) << 12) | ((SIDE * 2 / 64 - 1) << 24);
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, GLYPHS, 0, format, size]
        .iter()
        .enumerate()
    {
        m.write_u32(TEX_HEADER + 4 * i as u32, *w).unwrap();
    }
    let global = d3d::glyph::GLYPH_CACHE_BITMAP;
    m.map(global & !0xFFF, 0x1000);
    m.write_u32(global, BITMAP).unwrap();
    let texture = if named {
        TEX_HEADER
    } else {
        TEX_HEADER + 0x100
    };
    m.write_u32(BITMAP + d3d::glyph::BITMAP_TEXTURE, texture)
        .unwrap();
}

/// One frame: a transparent clear, then the text quad.
fn frame<B: Backend>(backend: B, smoothing: bool, filter: u32, named: bool) -> Device<B> {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_glyph_smoothing(smoothing);
    set_states(&mut d, &mut m);
    glyph_cache(&mut m, named);
    d.clear(&mut m, 0, 0, 0xF0, 0x0000_0000, 1.0, 0).unwrap();
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    d.set_texture_stage_state(&mut m, 0, TSS_MAGFILTER, filter)
        .unwrap();
    d.set_texture_stage_state(&mut m, 0, TSS_MINFILTER, filter)
        .unwrap();
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        // D3D puts pixel centres on integers: the half-pixel offset
        // maps each texel onto one pixel, as the game's text does.
        for f in [W as f32 * u - 0.5, H as f32 * v - 0.5, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for f in [DRAWN * u, DRAWN * v] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    d.note_write(0x8000_0000 | VB_DATA, vb.len() as u32);
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let skipped: Vec<_> = d
        .take_events()
        .into_iter()
        .filter(|e| matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
        .collect();
    assert!(skipped.is_empty(), "{skipped:?}");
    d.flush_render_targets(&mut m).unwrap();
    d
}

fn context() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

fn share(ctx: &GfxContext) -> GfxContext {
    GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

fn scaled(ctx: &GfxContext, scale: u32) -> Scaled {
    ScaledBackend::new(
        GfxBackend::new(share(ctx)),
        GfxBackend::new(share(ctx)),
        scale,
    )
    .unwrap()
}

/// RGBA bytes of a colour surface and its width.
fn pixels(b: &mut GfxBackend, s: gfx::surface::SurfaceId) -> (Vec<u8>, u32) {
    let (px, _) = b.read_color(s).unwrap();
    let w = b.store.get(s).unwrap().desc().width;
    (px, w)
}

/// The plain (unscaled) backend's back buffer.
fn plain_image(ctx: &GfxContext, smoothing: bool, filter: u32) -> Vec<u8> {
    let mut d = frame(GfxBackend::new(share(ctx)), smoothing, filter, true);
    let bb = d.back_buffer().unwrap();
    pixels(&mut d.backend, bb).0
}

/// The view's and the reference's back buffers at `scale`.
fn scaled_images(
    ctx: &GfxContext,
    scale: u32,
    smoothing: bool,
    filter: u32,
    named: bool,
) -> ((Vec<u8>, u32), Vec<u8>) {
    let mut d = frame(scaled(ctx, scale), smoothing, filter, named);
    let bb = d.back_buffer().unwrap();
    let reference = pixels(d.backend.reference_mut(), bb.reference).0;
    let view = pixels(
        d.backend.view_mut(),
        bb.view.expect("the back buffer scales"),
    );
    (view, reference)
}

fn alpha(px: &[u8], w: u32, x: u32, y: u32) -> f64 {
    px[((y * w + x) * 4 + 3) as usize] as f64 / 255.0
}

// ---------------------------------------------------------------------
// Measures.

/// Where a view's host pixel samples the texture, in texels, for a pixel
/// centre at `c` host pixels (pixel `x` has its centre at `x + 0.5`). The
/// view moves positions by `(2s - 1) / (4s)` guest pixels (docs/
/// high-resolution.md, "Half-pixel shift"), so a 1:1 quad's image sits
/// that far up and left of where a perfect upscale would put it; the
/// truth below moves the same way, so the measures see only the filter.
fn texel_at(c: f64, scale: u32) -> f64 {
    let s = scale as f64;
    c / s + (2.0 * s - 1.0) / (4.0 * s)
}

/// What the image says about its edges against the true outline.
#[derive(Debug)]
struct Edges {
    /// Sum of |alpha - coverage| over the image outside the thin stroke,
    /// in texels (a host pixel is `1 / s^2` of one): the area drawn wrong.
    area_error: f64,
    /// Along the shallow stroke's lower edge, per host column: the largest
    /// distance (host pixels, vertical) between where alpha crosses one
    /// half and the true edge. A staircase shows up here.
    stair: f64,
    /// Along the same edge: the mean number of rows per column with alpha
    /// strictly between 0.1 and 0.9 (the edge's width; the true edge of
    /// this slope is about 1.1 rows).
    width: f64,
}

fn edges(px: &[u8], w: u32, scale: u32) -> Edges {
    let s = scale as f64;
    let mut sum = 0.0;
    for y in 0..w {
        for x in 0..w {
            let a = alpha(px, w, x, y);
            let c = coverage(
                texel_at(x as f64, scale),
                texel_at(y as f64, scale),
                1.0 / s,
                8,
            );
            // The thin, light stroke (bottom right) is left out: its
            // coverage at the guest's size never reached 1, so no filter
            // can know how wide it was.
            let thin = x >= w / 2 && y >= w / 2;
            if !thin {
                sum += (a - c).abs();
            }
        }
    }
    let ((ax, ay), (bx, by), r) = STROKE;
    let slope = (by - ay) / (bx - ax);
    // The lower edge: the axis moved down by r / cos.
    let offset = r * (1.0 + slope * slope).sqrt();
    let mut stair: f64 = 0.0;
    let mut width = 0.0;
    let mut columns = 0;
    let (x0, x1) = ((ax + 5.0) * s, (bx - 5.0) * s);
    for x in x0 as u32..x1 as u32 {
        let xt = texel_at(x as f64 + 0.5, scale);
        let axis = ay + (xt - ax) * slope;
        // Back to host rows (row y's centre is y + 0.5).
        let truth = (axis + offset - texel_at(0.0, scale)) * s;
        let mut prev = alpha(px, w, x, (axis * s) as u32);
        let mut crossing = None;
        let mut soft = 0;
        for y in (axis * s) as u32 + 1..((axis + 2.0 * offset) * s) as u32 {
            let a = alpha(px, w, x, y);
            if a > 0.1 && a < 0.9 {
                soft += 1;
            }
            if crossing.is_none() && prev >= 0.5 && a < 0.5 {
                // Pixel centres are at +0.5.
                crossing = Some(y as f64 - 0.5 + (prev - 0.5) / (prev - a));
            }
            prev = a;
        }
        let c = crossing.expect("the stroke's edge crosses one half");
        stair = stair.max((c - truth).abs());
        width += soft as f64;
        columns += 1;
    }
    Edges {
        area_error: sum / (s * s),
        stair,
        width: width / columns as f64,
    }
}

/// The image composited over a dark blue, as a PNG.
fn write_png(path: &std::path::Path, px: &[u8], w: u32) {
    let bg = [16.0, 32.0, 64.0];
    let mut rgb = Vec::with_capacity(px.len() / 4 * 3);
    for p in px.as_chunks::<4>().0 {
        let a = p[3] as f64 / 255.0;
        for (c, b) in p[..3].iter().zip(bg) {
            rgb.push((*c as f64 * a + b * (1.0 - a)).round() as u8);
        }
    }
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, (px.len() / 4) as u32 / w);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}

// ---------------------------------------------------------------------

#[test]
fn one_x_and_the_reference_are_unchanged() {
    let Some(ctx) = context() else { return };
    for filter in [TEXF_POINT, TEXF_LINEAR] {
        let off = plain_image(&ctx, false, filter);
        assert!(
            off.as_chunks::<4>().0.iter().any(|p| p[3] == 255),
            "the text drew"
        );
        assert_eq!(plain_image(&ctx, true, filter), off, "1x, filter {filter}");
        let (_, reference) = scaled_images(&ctx, 4, true, filter, true);
        assert_eq!(reference, off, "the 4x reference, filter {filter}");
        // The wrapper at 1x: both its images are the plain one.
        let ((view, _), reference) = scaled_images(&ctx, 1, true, filter, true);
        assert_eq!(view, off, "the 1x view, filter {filter}");
        assert_eq!(reference, off, "the 1x reference, filter {filter}");
    }
}

#[test]
fn text_edges_are_smooth_at_two_three_and_four_times() {
    let Some(ctx) = context() else { return };
    for scale in [2, 3, 4] {
        let ((after, w), _) = scaled_images(&ctx, scale, true, TEXF_LINEAR, true);
        let ((linear, _), _) = scaled_images(&ctx, scale, false, TEXF_LINEAR, true);
        let ((point, _), _) = scaled_images(&ctx, scale, false, TEXF_POINT, true);
        let ((after_point, _), _) = scaled_images(&ctx, scale, true, TEXF_POINT, true);
        assert_eq!(
            after_point, after,
            "{scale}x: the guest's filter no longer matters"
        );
        let (a, l, p) = (
            edges(&after, w, scale),
            edges(&linear, w, scale),
            edges(&point, w, scale),
        );
        eprintln!("{scale}x after  {a:?}\n{scale}x linear {l:?}\n{scale}x point  {p:?}");
        let s = scale as f64;
        // Point sampling is a staircase of up to half a texel.
        assert!(p.stair > 0.3 * s, "{scale}x: the point-sampled edge steps");
        // Bilinear is soft: its edge spans about a texel.
        assert!(l.width > 0.75 * s, "{scale}x: the bilinear edge is soft");
        // After: on the outline to within half a host pixel, about a host
        // pixel wide, and closer to the true coverage than either.
        assert!(a.stair < 0.5, "{scale}x: steps of {} pixels", a.stair);
        assert!(a.width < 2.0, "{scale}x: edges {} pixels wide", a.width);
        assert!(
            a.area_error < 0.6 * l.area_error.min(p.area_error),
            "{scale}x: {a:?} against {l:?}, {p:?}"
        );
        if scale == 4
            && let Some(dir) = std::env::var_os("H2_FONT_HD_PNG")
        {
            let dir = std::path::PathBuf::from(dir);
            write_png(&dir.join("font-hd-before.png"), &linear, w);
            write_png(&dir.join("font-hd-after.png"), &after, w);
        }
    }
}

#[test]
fn a_texture_the_glyph_pointer_does_not_name_is_left_alone() {
    let Some(ctx) = context() else { return };
    let ((off, _), _) = scaled_images(&ctx, 4, false, TEXF_LINEAR, true);
    let ((unnamed, _), _) = scaled_images(&ctx, 4, true, TEXF_LINEAR, false);
    assert_eq!(unnamed, off, "same shape, but not the glyph cache");
}
