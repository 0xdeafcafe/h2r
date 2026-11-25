//! Immutable world-space geometry carried by a presented image.

use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use smallvec::SmallVec;

pub mod probe;

/// One group of world-space edges with a shared style and label anchor.
pub struct WorldLine<'a> {
    pub edges: &'a [[[f32; 3]; 2]],
    pub anchor: [f32; 3],
    pub color: [f32; 3],
    pub dashed: bool,
    pub label: Option<&'a str>,
    pub style: LineStyle,
}

/// A soft halo around a line's core.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// Extra half-width on each side, in pixels at 1080p.
    pub width: f32,
    /// The halo's peak opacity as a fraction of the line's own.
    pub alpha: f32,
}

/// Arrowheads along a line, lying in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chevrons {
    /// The way they point (world space; used flat in the XY plane).
    pub direction: [f32; 3],
    /// World units between arrowheads.
    pub spacing: f32,
    /// World units from an arrowhead's tip back to its wings.
    pub size: f32,
    /// World units a second the arrowheads travel along `direction`;
    /// zero keeps them still.
    pub speed: f32,
}

/// A filled strip on the floor along each edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ribbon {
    /// World units across.
    pub width: f32,
    /// Opacity as a fraction of the line's own.
    pub alpha: f32,
}

/// Brightness that breathes with the present time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// Seconds a cycle.
    pub period: f32,
    /// 0..1: how far the dark end drops below full brightness.
    pub depth: f32,
}

/// Opacity that falls away with the distance from the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    /// World units: full opacity up to here.
    pub start: f32,
    /// World units: invisible from here.
    pub end: f32,
}

/// How a [`WorldLine`] is drawn. The default is a thin, opaque line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineStyle {
    /// Core width in pixels at 1080p (scaled by the output height).
    pub width: f32,
    pub alpha: f32,
    pub glow: Option<Glow>,
    pub pulse: Option<Pulse>,
    pub fade: Option<Fade>,
    pub chevrons: Option<Chevrons>,
    pub ribbon: Option<Ribbon>,
    /// Draw the edges themselves (the default). Off leaves only what the
    /// style adds along them (chevrons, ribbon) and the label: arrowheads
    /// that flow along a run with no line under them.
    pub stroke: bool,
    /// Size of one font pixel of the label (1 is the 8x8 font as is).
    pub label_scale: f32,
    /// Centre the label on its anchor, with a drop shadow, and scale it
    /// by the output height over 1080p (at least 1). Pulse and fade apply
    /// to it too.
    pub label_centered: bool,
    /// The letters the label is drawn in.
    pub label_face: LabelFace,
}

/// Which letters a [`WorldLine`]'s label is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelFace {
    /// The built-in 8x8 debug font, `label_scale` font pixels a pixel.
    #[default]
    Debug,
    /// The disc's HUD font ([`hud_font`]), with its button glyphs. At
    /// `label_scale` 2 a font pixel is a HUD pixel (the HUD is 480 lines
    /// tall). While no HUD font is loaded the window draws the label in
    /// the 8x8 debug font; the source that asked for it says so.
    Hud,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            alpha: 1.0,
            glow: None,
            pulse: None,
            fade: None,
            chevrons: None,
            ribbon: None,
            stroke: true,
            label_scale: 1.0,
            label_centered: false,
            label_face: LabelFace::Debug,
        }
    }
}

/// One glyph of a [`LabelFont`]: its metrics as the disc font gives them
/// (`docs/formats/fonts.md`) and where its pixels are in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelGlyph {
    /// The UTF-16 code unit it draws.
    pub unit: u16,
    /// The pen moves by `x_offset + advance` (plus kerning).
    pub advance: i16,
    pub x_offset: i16,
    /// The bitmap's top row is `y_offset` above the baseline.
    pub y_offset: i16,
    pub width: u16,
    pub height: u16,
    /// The bitmap's top-left texel in the atlas.
    pub atlas: [u32; 2],
    /// Drawn in its own colours (a button picture), not the label's.
    pub coloured: bool,
}

/// One glyph's pixels, straight-alpha RGBA, `width * height * 4` bytes,
/// for [`LabelFont::build`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphPixels {
    pub unit: u16,
    pub advance: i16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
    pub coloured: bool,
}

/// A glyph placed by [`LabelFont::layout`]: its top-left in font pixels
/// from the top-left of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacedGlyph {
    pub x: i32,
    pub y: i32,
    pub glyph: LabelGlyph,
}

/// A line of text laid out in a [`LabelFont`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextLayout {
    pub glyphs: Vec<PlacedGlyph>,
    /// Font pixels from the line's left to its last pen position or
    /// bitmap edge, whichever is further.
    pub width: i32,
    /// Characters the font has no glyph for (not drawn).
    pub missing: usize,
}

/// A disc font's glyphs packed into one RGBA atlas, for labels drawn as
/// textured quads. Built from the user's own font file at run time;
/// nothing of it is in the repository.
#[derive(Debug, PartialEq, Eq)]
pub struct LabelFont {
    /// Distinct for every font built in the process: the window uploads
    /// the atlas again when it changes.
    pub generation: u64,
    /// The font file's name, for notes.
    pub name: String,
    pub ascent: i16,
    pub descent: i16,
    /// Atlas width and height in texels.
    pub size: [u32; 2],
    /// `size[0] * size[1] * 4` bytes, straight alpha.
    pub rgba: Vec<u8>,
    /// Sorted by unit.
    glyphs: Vec<LabelGlyph>,
    /// (first, second, amount), sorted.
    kerning: Vec<(u16, u16, i16)>,
}

/// The atlas is this wide; rows are added as needed.
pub const ATLAS_WIDTH: u32 = 512;
/// Empty texels around each glyph, so filtering never reads a neighbour.
const ATLAS_GAP: u32 = 1;

static GENERATION: AtomicU64 = AtomicU64::new(1);

impl LabelFont {
    /// Packs `glyphs` into shelves of an [`ATLAS_WIDTH`] atlas, tallest
    /// first. A glyph wider than the atlas, or whose pixels are not
    /// `width * height * 4` bytes, is left out (its unit then reads as
    /// missing). A later glyph of the same unit is dropped.
    pub fn build(
        name: &str,
        ascent: i16,
        descent: i16,
        mut glyphs: Vec<GlyphPixels>,
        kerning: Vec<(u16, u16, i16)>,
    ) -> LabelFont {
        let fits = |g: &GlyphPixels| {
            u32::from(g.width) + 2 * ATLAS_GAP <= ATLAS_WIDTH
                && g.rgba.len() == usize::from(g.width) * usize::from(g.height) * 4
        };
        glyphs.retain(fits);
        let mut seen = std::collections::HashSet::new();
        glyphs.retain(|g| seen.insert(g.unit));
        let mut order: Vec<usize> = (0..glyphs.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse((glyphs[i].height, glyphs[i].width)));
        let mut placed = vec![[0u32; 2]; glyphs.len()];
        let (mut x, mut y, mut row) = (ATLAS_GAP, ATLAS_GAP, 0u32);
        for &i in &order {
            let (w, h) = (u32::from(glyphs[i].width), u32::from(glyphs[i].height));
            if x + w + ATLAS_GAP > ATLAS_WIDTH {
                x = ATLAS_GAP;
                y += row + ATLAS_GAP;
                row = 0;
            }
            placed[i] = [x, y];
            x += w + ATLAS_GAP;
            row = row.max(h);
        }
        let height = (y + row + ATLAS_GAP).max(1);
        let mut rgba = vec![0u8; (ATLAS_WIDTH * height * 4) as usize];
        for (g, &[gx, gy]) in glyphs.iter().zip(&placed) {
            let w = usize::from(g.width) * 4;
            for row in 0..usize::from(g.height) {
                let at = ((gy as usize + row) * ATLAS_WIDTH as usize + gx as usize) * 4;
                rgba[at..at + w].copy_from_slice(&g.rgba[row * w..(row + 1) * w]);
            }
        }
        let mut atlas: Vec<LabelGlyph> = glyphs
            .iter()
            .zip(&placed)
            .map(|(g, &atlas)| LabelGlyph {
                unit: g.unit,
                advance: g.advance,
                x_offset: g.x_offset,
                y_offset: g.y_offset,
                width: g.width,
                height: g.height,
                atlas,
                coloured: g.coloured,
            })
            .collect();
        atlas.sort_by_key(|g| g.unit);
        let mut kerning = kerning;
        kerning.sort();
        kerning.dedup_by_key(|k| (k.0, k.1));
        LabelFont {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            ascent,
            descent,
            size: [ATLAS_WIDTH, height],
            rgba,
            glyphs: atlas,
            kerning,
        }
    }

    pub fn glyph(&self, unit: u16) -> Option<&LabelGlyph> {
        self.glyphs
            .binary_search_by_key(&unit, |g| g.unit)
            .ok()
            .map(|i| &self.glyphs[i])
    }

    /// Whether every character of `text` has a glyph.
    pub fn covers(&self, text: &str) -> bool {
        self.layout(text).missing == 0
    }

    /// Ascent plus descent, in font pixels.
    pub fn line_height(&self) -> i32 {
        i32::from(self.ascent) + i32::from(self.descent)
    }

    fn kern(&self, first: u16, second: u16) -> i16 {
        self.kerning
            .binary_search_by_key(&(first, second), |k| (k.0, k.1))
            .map_or(0, |i| self.kerning[i].2)
    }

    /// One line of `text` as the engine lays it out (`0x13ef40`): each
    /// glyph at `pen + kerning + x_offset`, its top `y_offset` above the
    /// baseline (`ascent` down), and the pen then moved by `kerning +
    /// x_offset + advance`. Control characters are skipped; a character
    /// the font lacks is counted in `missing` and not drawn.
    pub fn layout(&self, text: &str) -> TextLayout {
        let mut out = TextLayout::default();
        let (mut pen, mut prev) = (0i32, 0u16);
        for c in text.chars() {
            let Ok(unit) = u16::try_from(u32::from(c)) else {
                out.missing += 1;
                continue;
            };
            if unit < 0x20 {
                continue;
            }
            let Some(&glyph) = self.glyph(unit) else {
                out.missing += 1;
                continue;
            };
            let x = pen + i32::from(self.kern(prev, unit)) + i32::from(glyph.x_offset);
            let y = i32::from(self.ascent) - i32::from(glyph.y_offset);
            pen = x + i32::from(glyph.advance);
            prev = unit;
            out.width = out.width.max(pen).max(x + i32::from(glyph.width));
            if glyph.width > 0 && glyph.height > 0 {
                out.glyphs.push(PlacedGlyph { x, y, glyph });
            }
        }
        out
    }
}

static HUD_FONT: Mutex<Option<Arc<LabelFont>>> = Mutex::new(None);

/// Publishes the disc's HUD font for [`LabelFace::Hud`] labels (`None`
/// takes it away).
pub fn set_hud_font(font: Option<Arc<LabelFont>>) {
    *HUD_FONT.lock().unwrap_or_else(|e| e.into_inner()) = font;
}

/// The HUD font [`set_hud_font`] published, if any.
pub fn hud_font() -> Option<Arc<LabelFont>> {
    HUD_FONT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A mark drawn in a view's HUD space rather than the world: points in
/// guest pixels of the 640 by 480 image, with the host's HUD fit already
/// applied by the source. The window scales them with the image as it
/// scales the guest's own HUD. A source that draws over the motion
/// tracker uses these (docs/game/superbounce.md, "The bounce view").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HudMark<'a> {
    /// A line through `points`, `width` guest pixels wide (scaled by the
    /// image's height); `closed` joins the last point to the first.
    Path {
        points: &'a [[f32; 2]],
        closed: bool,
        width: f32,
        dashed: bool,
        color: [f32; 3],
        alpha: f32,
    },
    /// A filled convex polygon: a fan from its first point.
    Fill {
        points: &'a [[f32; 2]],
        color: [f32; 3],
        alpha: f32,
    },
    /// Text in the HUD font ([`hud_font`]; the 8x8 font without one),
    /// `pixel` guest pixels a font pixel, its top left at `at` or centred
    /// on it.
    Text {
        at: [f32; 2],
        text: &'a str,
        color: [f32; 3],
        alpha: f32,
        pixel: f32,
        centered: bool,
    },
}

/// How long a sample can appear when its image is shown again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageValidity {
    Image,
    Lease { expires: Instant, generation: u64 },
}

/// Geometry captured on the render owner and consumed on the window thread.
pub trait ImageMarks: Debug + Send + Sync {
    /// Returning `None` suppresses a stale or empty sample. The default
    /// keeps the sample for the lifetime of its image. A lease can check
    /// its generation before consulting the clock.
    fn validity(&self, _now: &dyn Fn() -> Instant) -> Option<ImageValidity> {
        Some(ImageValidity::Image)
    }

    fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>));

    fn visit(
        &self,
        now: &dyn Fn() -> Instant,
        visitor: &mut dyn FnMut(WorldLine<'_>),
    ) -> Option<ImageValidity> {
        let validity = self.validity(now)?;
        self.lines(visitor);
        Some(validity)
    }

    /// Marks in the view's HUD space ([`HudMark`]), drawn after the world
    /// lines. Most sources have none.
    fn hud(&self, _now: &dyn Fn() -> Instant, _visitor: &mut dyn FnMut(HudMark<'_>)) {}
}

#[derive(Debug, Default)]
pub struct ImageSamples {
    pub editor: Option<Arc<dyn ImageMarks>>,
    pub sources: SmallVec<[Arc<dyn ImageMarks>; 2]>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShownMarks {
    pub expires: Option<Instant>,
    pub editor_generation: Option<u64>,
}

impl ImageSamples {
    pub fn has_visible_sample(&self, now: &dyn Fn() -> Instant) -> bool {
        self.sources
            .iter()
            .chain(self.editor.iter())
            .any(|sample| sample.validity(now).is_some())
    }

    /// Draws all producers through the same visitor. Only the editor
    /// sample contributes to the editor RPC's reported generation.
    pub fn visit(
        &self,
        now: &dyn Fn() -> Instant,
        visitor: &mut dyn FnMut(WorldLine<'_>),
    ) -> ShownMarks {
        let mut shown = ShownMarks::default();
        for sample in &self.sources {
            if let Some(ImageValidity::Lease { expires, .. }) = sample.visit(now, visitor) {
                shown.expires = Some(shown.expires.map_or(expires, |old| old.min(expires)));
            }
        }
        if let Some(ImageValidity::Lease {
            expires,
            generation,
        }) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.visit(now, visitor))
        {
            shown.expires = Some(shown.expires.map_or(expires, |old| old.min(expires)));
            shown.editor_generation = Some(generation);
        }
        shown
    }

    /// Draws every visible source's HUD-space marks ([`ImageMarks::hud`]).
    pub fn visit_hud(&self, now: &dyn Fn() -> Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
        for sample in &self.sources {
            if sample.validity(now).is_some() {
                sample.hud(now, visitor);
            }
        }
    }
}

/// A block of the screen the HUD's text lies in, as fractions of the
/// frame (0 to 1, y down): `[left, top, right, bottom]`.
pub type Block = [f32; 4];
/// The HUD's top-left block (notifications, the loss line) and
/// bottom-left block (the superbounce readout): a world label that
/// projects into one is left out, as it would run under HUD text.
pub const HUD_BLOCKS: [Block; 2] = [[0.0, 0.0, 0.42, 0.22], [0.0, 0.70, 0.42, 1.0]];

/// Whether a label centred at `screen` (fractions of the frame, y down)
/// would lie under the HUD's text ([`HUD_BLOCKS`]). The window's world-line
/// pass, which projects the anchors, asks it for each label; a label
/// off the frame is not under anything.
pub fn label_under_hud(screen: [f32; 2]) -> bool {
    screen.iter().all(|v| v.is_finite())
        && HUD_BLOCKS.iter().any(|b| {
            screen[0] >= b[0] && screen[0] <= b[2] && screen[1] >= b[1] && screen[1] <= b[3]
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(unit: u16, width: u16, height: u16, advance: i16) -> GlyphPixels {
        GlyphPixels {
            unit,
            advance,
            x_offset: 0,
            y_offset: 10,
            width,
            height,
            rgba: vec![255; usize::from(width) * usize::from(height) * 4],
            coloured: false,
        }
    }

    fn font() -> LabelFont {
        LabelFont::build(
            "test",
            10,
            3,
            vec![
                glyph(u16::from(b'A'), 6, 10, 7),
                glyph(u16::from(b'V'), 6, 10, 7),
                glyph(u16::from(b' '), 0, 0, 4),
                GlyphPixels {
                    y_offset: 10,
                    coloured: true,
                    ..glyph(0xe102, 16, 18, 16)
                },
            ],
            vec![(u16::from(b'A'), u16::from(b'V'), -2)],
        )
    }

    #[test]
    fn glyphs_lay_out_along_the_pen_with_kerning() {
        let f = font();
        let l = f.layout("AV A");
        let xs: Vec<i32> = l.glyphs.iter().map(|g| g.x).collect();
        // A at 0; V kerned 2 back after A's 7; the space draws nothing
        // but moves the pen 4; A again.
        assert_eq!(xs, [0, 5, 16]);
        assert_eq!(l.width, 23);
        assert_eq!(l.missing, 0);
        assert!(l.glyphs.iter().all(|g| g.y == 0));
    }

    #[test]
    fn a_button_glyph_keeps_its_colours_and_a_missing_letter_is_counted() {
        let f = font();
        let l = f.layout("A\u{e102}");
        assert_eq!(l.glyphs.len(), 2);
        assert!(l.glyphs[1].glyph.coloured && !l.glyphs[0].glyph.coloured);
        assert_eq!(l.glyphs[1].x, 7);
        assert_eq!(f.layout("AxA").missing, 1);
        assert!(!f.covers("Ax") && f.covers("A A"));
        // Control characters are skipped, not missing.
        assert_eq!(f.layout("A\nA").missing, 0);
    }

    #[test]
    fn the_atlas_keeps_every_glyph_apart_and_inside() {
        let mut many: Vec<GlyphPixels> = (0..200u16)
            .map(|i| glyph(0x100 + i, 9 + i % 7, 12 + i % 5, 9))
            .collect();
        many.push(glyph(1, 600, 4, 1)); // wider than the atlas: left out
        let f = LabelFont::build("many", 10, 3, many, Vec::new());
        assert!(f.glyph(1).is_none());
        let boxes: Vec<[u32; 4]> = (0..200u16)
            .map(|i| {
                let g = f.glyph(0x100 + i).unwrap();
                let [x, y] = g.atlas;
                [x, y, x + u32::from(g.width), y + u32::from(g.height)]
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            assert!(a[2] <= f.size[0] && a[3] <= f.size[1]);
            for b in &boxes[i + 1..] {
                let apart = a[2] + ATLAS_GAP <= b[0]
                    || b[2] + ATLAS_GAP <= a[0]
                    || a[3] + ATLAS_GAP <= b[1]
                    || b[3] + ATLAS_GAP <= a[1];
                assert!(apart, "{a:?} touches {b:?}");
            }
        }
        assert_eq!(f.rgba.len() as u32, f.size[0] * f.size[1] * 4);
    }

    #[test]
    fn hud_marks_come_from_visible_sources_only() {
        #[derive(Debug)]
        struct Tracker(bool);
        impl ImageMarks for Tracker {
            fn validity(&self, _now: &dyn Fn() -> Instant) -> Option<ImageValidity> {
                self.0.then_some(ImageValidity::Image)
            }
            fn lines(&self, _visitor: &mut dyn FnMut(WorldLine<'_>)) {}
            fn hud(&self, _now: &dyn Fn() -> Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
                visitor(HudMark::Fill {
                    points: &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                    color: [1.0; 3],
                    alpha: 1.0,
                });
            }
        }
        let mut samples = ImageSamples::default();
        samples.sources.push(Arc::new(Tracker(true)));
        samples.sources.push(Arc::new(Tracker(false)));
        let mut n = 0;
        samples.visit_hud(&Instant::now, &mut |_| n += 1);
        assert_eq!(n, 1);
        // A source without HUD marks draws none.
        #[derive(Debug)]
        struct Plain;
        impl ImageMarks for Plain {
            fn lines(&self, _visitor: &mut dyn FnMut(WorldLine<'_>)) {}
        }
        let mut plain = ImageSamples::default();
        plain.sources.push(Arc::new(Plain));
        plain.visit_hud(&Instant::now, &mut |_| n += 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn each_font_built_has_its_own_generation() {
        assert_ne!(font().generation, font().generation);
    }
}
