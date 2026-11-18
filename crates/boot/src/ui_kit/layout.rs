//! Layout checks over a [`View`] (docs/game/ui-builder.md "The checks").
//!
//! Each runs on what a [`Source`] gives: text widths and line heights,
//! and, for the checks on art, the images' own pixels. The synthetic
//! source has no images, so [`check`] says which checks it could not run
//! ([`Report::skipped`]); the disc-backed test runs them all.
//!
//! - **fit**: every text fits its rectangle.
//! - **overlap**: no two texts' ink overlaps, and no text that belongs to
//!   no row (description, legend, title) sits on a row's art.
//! - **frame provenance**: frame art borrowed from a retail screen with a
//!   list sits with that list as that screen places it (anchor and skin).
//! - **covered** (art): frame art from a screen without a list is covered
//!   by row art or text over at most [`Rules::max_covered`] pixels.
//! - **off screen** (art): no art's opaque pixels fall past the screen
//!   edge.
//! - **safe area**: texts and row art stay inside the title-safe area.
//! - **reserved**: nothing overlaps an area the screen beneath keeps.
//! - **contrast**: every text against what is drawn under it.
//! - **legend**: a screen that takes more than up, down, A and B says
//!   which buttons do what.
//! - **focus**: the focused element is drawn, on screen, brighter than the
//!   rest.

use std::fmt;

use tags::ui::Rect;

use crate::ui_kit::input::Input;
use crate::ui_kit::preview::{Canvas, Source, overflows, render_view_where};
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::view::{Art, Justify, Kind, Layer, Metrics, View, Widget};

/// What a view is held to.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    /// The title-safe area: the middle 85% of 640 by 480 (retail headers
    /// start at its top-left corner, `x -270, y 204`), down to the bottom of
    /// the retail footer rectangle (`y -211`), where the game's own legends
    /// sit.
    pub safe: Rect,
    /// Areas the screen beneath keeps (its logo, its header), by name.
    pub reserved: Vec<(String, Rect)>,
    /// The least contrast ratio (WCAG's, `(L1 + 0.05) / (L2 + 0.05)`)
    /// between a text and what is under it. The game's own unfocused rows,
    /// as the preview draws them from the screenshot's measure, come out
    /// at 2.9 to 3.1 on the real bars; rows dimmed to 55% of the skin
    /// colour (the first preview's guess, nearly invisible) at about 1.5.
    /// 2.5 passes the first and catches the second.
    pub min_contrast: f32,
    /// Frame art pixels another widget may cover.
    pub max_covered: usize,
    /// The screen takes inputs past up, down, A and B.
    pub needs_legend: bool,
}

pub const SAFE: Rect = Rect {
    top: 204,
    left: -272,
    bottom: -211,
    right: 272,
};

impl Default for Rules {
    fn default() -> Self {
        Rules {
            safe: SAFE,
            reserved: Vec::new(),
            min_contrast: 2.5,
            max_covered: 24,
            needs_legend: false,
        }
    }
}

impl Rules {
    /// The rules for a kit screen: a legend when it takes more than the
    /// list's own inputs.
    pub fn for_spec(s: &ScreenSpec) -> Rules {
        Rules {
            needs_legend: s
                .inputs
                .iter()
                .any(|i| !matches!(i, Input::Up | Input::Down | Input::A | Input::B)),
            ..Rules::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Issue {
    Fit {
        id: String,
    },
    TextOverlap {
        a: String,
        b: String,
    },
    OnOtherArt {
        text: String,
        art: String,
    },
    Covered {
        art: String,
        by: String,
        pixels: usize,
    },
    Misplaced {
        art: String,
        screen_id: i16,
    },
    OffScreen {
        art: String,
        pixels: usize,
    },
    Unsafe {
        id: String,
    },
    Reserved {
        id: String,
        area: String,
    },
    Contrast {
        id: String,
        ratio: f32,
    },
    NoLegend,
    FocusHidden {
        id: String,
    },
    FocusNotBrighter {
        id: String,
    },
}

impl Issue {
    /// The widgets it names, for a tool to mark (the editor's UI Screens).
    pub fn ids(&self) -> Vec<&str> {
        match self {
            Issue::Fit { id }
            | Issue::Unsafe { id }
            | Issue::Reserved { id, .. }
            | Issue::Contrast { id, .. }
            | Issue::FocusHidden { id }
            | Issue::FocusNotBrighter { id } => vec![id],
            Issue::TextOverlap { a, b } => vec![a, b],
            Issue::OnOtherArt { text, art } => vec![text, art],
            Issue::Covered { art, by, .. } => vec![art, by],
            Issue::Misplaced { art, .. } | Issue::OffScreen { art, .. } => vec![art],
            Issue::NoLegend => Vec::new(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::Fit { id } => write!(f, "{id}: the text overflows its rectangle"),
            Issue::TextOverlap { a, b } => write!(f, "{a} and {b}: the texts overlap"),
            Issue::OnOtherArt { text, art } => {
                write!(f, "{text} sits on {art}, art it doesn't belong to")
            }
            Issue::Covered { art, by, pixels } => {
                write!(f, "{art}: {pixels} of its pixels are covered by {by}")
            }
            Issue::Misplaced { art, screen_id } => write!(
                f,
                "{art}: frame art of screen {screen_id}, whose list is elsewhere or on another skin: it frames nothing, and rows cut into it"
            ),
            Issue::OffScreen { art, pixels } => {
                write!(f, "{art}: {pixels} of its pixels are past the screen edge")
            }
            Issue::Unsafe { id } => write!(f, "{id}: outside the title-safe area"),
            Issue::Reserved { id, area } => write!(f, "{id}: over {area}"),
            Issue::Contrast { id, ratio } => {
                write!(f, "{id}: contrast {ratio:.2} against what is under it")
            }
            Issue::NoLegend => write!(
                f,
                "the screen takes buttons past up, down, A and B, and has no legend"
            ),
            Issue::FocusHidden { id } => {
                write!(f, "{id}: the focused element is not drawn on screen")
            }
            Issue::FocusNotBrighter { id } => {
                write!(f, "{id}: the focused row is no brighter than the others")
            }
        }
    }
}

/// What [`check`] found, and the checks it could not run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub skipped: Vec<&'static str>,
}

/// `r`'s area, Y up.
fn area(r: Rect) -> i32 {
    (r.right - r.left).max(0) as i32 * (r.top - r.bottom).max(0) as i32
}

fn intersect(a: Rect, b: Rect) -> Rect {
    Rect {
        top: a.top.min(b.top),
        left: a.left.max(b.left),
        bottom: a.bottom.max(b.bottom),
        right: a.right.min(b.right),
    }
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.top <= outer.top
        && inner.bottom >= outer.bottom
        && inner.left >= outer.left
        && inner.right <= outer.right
}

/// Where a text widget's ink is: each line's width at its justification,
/// lines one line height apart from the top.
pub fn ink(w: &Widget, m: &dyn Metrics) -> Option<Rect> {
    let Kind::Text {
        text,
        font,
        justify,
        ..
    } = &w.kind
    else {
        return None;
    };
    let r = w.rect();
    let lh = m.line_height(*font);
    let mut out: Option<Rect> = None;
    for (n, line) in text.split('\n').enumerate() {
        let width = m.width(*font, line);
        if width == 0 {
            continue;
        }
        let left = i32::from(r.left)
            + match justify {
                Justify::Left => 0,
                Justify::Right => r.width() - width,
                Justify::Centre => (r.width() - width) / 2,
            };
        let top = i32::from(r.top) - n as i32 * lh;
        let l = Rect {
            top: top as i16,
            left: left as i16,
            bottom: (top - lh) as i16,
            right: (left + width) as i16,
        };
        out = Some(match out {
            None => l,
            Some(o) => Rect {
                top: o.top.max(l.top),
                left: o.left.min(l.left),
                bottom: o.bottom.min(l.bottom),
                right: o.right.max(l.right),
            },
        });
    }
    out
}

/// The row a widget belongs to (`row.3.label` is row 3's).
fn row_of(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("row.")?;
    Some(rest.split('.').next().unwrap_or(rest))
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        let v = v / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// The pixels of `r` on a 640 by 480 canvas, clipped.
fn pixels(r: Rect) -> impl Iterator<Item = (u32, u32)> {
    let x0 = (320 + i32::from(r.left)).clamp(0, 640) as u32;
    let x1 = (320 + i32::from(r.right)).clamp(0, 640) as u32;
    let y0 = (240 - i32::from(r.top)).clamp(0, 480) as u32;
    let y1 = (240 - i32::from(r.bottom)).clamp(0, 480) as u32;
    (y0..y1).flat_map(move |y| (x0..x1).map(move |x| (x, y)))
}

/// An art widget's visible pixels on the canvas, and how many fall off it.
type Mask = (Vec<(u32, u32)>, usize);

/// Where art `w` is opaque (alpha at least half, its own alpha applied),
/// as canvas pixels, and how many opaque pixels fall off the canvas. `None`
/// when the source has no image for it.
fn opaque(w: &Widget, src: &dyn Source) -> Option<Mask> {
    let Kind::Art { art, .. } = &w.kind else {
        return None;
    };
    let img = src.art(art)?;
    let (x0, y0) = (320 + i32::from(w.rect[1]), 240 - i32::from(w.rect[0]));
    let mut on = Vec::new();
    let mut off = 0;
    for row in 0..img.height {
        for col in 0..img.width {
            let a = u32::from(img.rgba[((row * img.width + col) * 4 + 3) as usize])
                * u32::from(w.alpha)
                / 255;
            if a < 24 {
                continue;
            }
            let (x, y) = (x0 + col as i32, y0 + row as i32);
            if (0..640).contains(&x) && (0..480).contains(&y) {
                on.push((x as u32, y as u32));
            } else {
                off += 1;
            }
        }
    }
    Some((on, off))
}

/// Runs every check on `v` drawn from `src`.
pub fn check(v: &View, src: &dyn Source, rules: &Rules) -> Report {
    let mut rep = Report::default();
    let shown: Vec<&Widget> = v.widgets.iter().filter(|w| w.alpha > 0).collect();
    let texts: Vec<(&Widget, Rect)> = shown
        .iter()
        .filter_map(|w| ink(w, src).map(|r| (*w, r)))
        .collect();
    let arts: Vec<&Widget> = shown
        .iter()
        .copied()
        .filter(|w| matches!(w.kind, Kind::Art { .. }))
        .collect();

    for w in &shown {
        if overflows(w, src) {
            rep.issues.push(Issue::Fit { id: w.id.clone() });
        }
    }
    for (i, (a, ra)) in texts.iter().enumerate() {
        for (b, rb) in &texts[i + 1..] {
            if area(intersect(*ra, *rb)) > 0 {
                rep.issues.push(Issue::TextOverlap {
                    a: a.id.clone(),
                    b: b.id.clone(),
                });
            }
        }
        for art in arts.iter().filter(|w| w.layer == Layer::Under) {
            // A row's frame reaches into its neighbours by design: only texts
            // that belong to no row (description, legend, title) are held off
            // row art.
            if row_of(&a.id).is_none()
                && row_of(&art.id).is_some()
                && area(intersect(*ra, art.rect())) > 0
            {
                rep.issues.push(Issue::OnOtherArt {
                    text: a.id.clone(),
                    art: art.id.clone(),
                });
            }
        }
    }

    // Frame art borrowed from a retail screen with a list belongs with that
    // list, where that screen puts it: retail's own compositions overlap
    // by design (a bracket under the first rows' caps), and a bracket kept
    // for a list moved or reskinned frames nothing and is cut by the rows.
    // Frame art from a screen without a list is held to its pixels.
    let mut by_pixels: Vec<&str> = Vec::new();
    for f in arts.iter().filter(|w| w.layer == Layer::Frame) {
        let Kind::Art {
            art: Art::Screen {
                screen_id, pane, ..
            },
            ..
        } = f.kind
        else {
            by_pixels.push(&f.id);
            continue;
        };
        match src.list_of(screen_id, pane) {
            None => {
                if !rep.skipped.contains(&"frame provenance") {
                    rep.skipped.push("frame provenance");
                }
            }
            Some(Some(l)) => {
                if v.list != Some(l) {
                    rep.issues.push(Issue::Misplaced {
                        art: f.id.clone(),
                        screen_id,
                    });
                }
            }
            Some(None) => by_pixels.push(&f.id),
        }
    }

    // Art against art and the screen edge: the images' own pixels.
    let masks: Vec<(&Widget, Option<Mask>)> =
        arts.iter().map(|w| (*w, opaque(w, src))).collect();
    if masks.iter().any(|(_, m)| m.is_none()) {
        rep.skipped.push("covered");
        rep.skipped.push("off screen");
    } else {
        let mut grid = vec![false; 640 * 480];
        for (frame, m) in masks
            .iter()
            .filter(|(w, _)| w.layer == Layer::Frame && by_pixels.contains(&w.id.as_str()))
        {
            let Some((on, _)) = m else { continue };
            grid.iter_mut().for_each(|g| *g = false);
            for &(x, y) in on {
                grid[(y * 640 + x) as usize] = true;
            }
            for (other, om) in masks.iter().filter(|(w, _)| w.layer != Layer::Frame) {
                let Some((oon, _)) = om else { continue };
                let n = oon
                    .iter()
                    .filter(|&&(x, y)| grid[(y * 640 + x) as usize])
                    .count();
                if n > rules.max_covered {
                    rep.issues.push(Issue::Covered {
                        art: frame.id.clone(),
                        by: other.id.clone(),
                        pixels: n,
                    });
                }
            }
            for (t, r) in &texts {
                let n = pixels(*r)
                    .filter(|&(x, y)| grid[(y * 640 + x) as usize])
                    .count();
                if n > rules.max_covered {
                    rep.issues.push(Issue::Covered {
                        art: frame.id.clone(),
                        by: t.id.clone(),
                        pixels: n,
                    });
                }
            }
        }
        for (w, m) in &masks {
            if let Some((_, off)) = m
                && *off > 0
            {
                rep.issues.push(Issue::OffScreen {
                    art: w.id.clone(),
                    pixels: *off,
                });
            }
        }
    }

    for (w, r) in &texts {
        if !inside(*r, rules.safe) {
            rep.issues.push(Issue::Unsafe { id: w.id.clone() });
        }
    }
    for w in arts.iter().filter(|w| w.layer == Layer::Under) {
        if !inside(w.rect(), rules.safe) {
            rep.issues.push(Issue::Unsafe { id: w.id.clone() });
        }
    }
    for (name, area_r) in &rules.reserved {
        for w in &shown {
            let r = ink(w, src).unwrap_or(w.rect());
            if area(intersect(r, *area_r)) > 0 {
                rep.issues.push(Issue::Reserved {
                    id: w.id.clone(),
                    area: name.clone(),
                });
            }
        }
    }

    // Contrast: each text against the view drawn without its texts.
    let backdrop: Canvas = render_view_where(v, src, &|w| w.text().is_none(), false);
    for (w, r) in &texts {
        let Kind::Text { colour, .. } = &w.kind else {
            continue;
        };
        let (mut sum, mut n) = ([0f32; 3], 0f32);
        for (x, y) in pixels(*r) {
            let i = ((y * 640 + x) * 4) as usize;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += f32::from(backdrop.rgba[i + c]);
            }
            n += 1.0;
        }
        if n == 0.0 {
            continue;
        }
        let bg = sum.map(|s| s / n);
        let a = f32::from(w.alpha) / 255.0;
        let fg = [0, 1, 2].map(|c| f32::from(colour[c]) * a + bg[c] * (1.0 - a));
        let (l1, l2) = (luminance(fg), luminance(bg));
        let ratio = (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05);
        if ratio < rules.min_contrast {
            rep.issues.push(Issue::Contrast {
                id: w.id.clone(),
                ratio,
            });
        }
    }

    if rules.needs_legend
        && !texts.iter().any(|(w, _)| {
            w.text()
                .is_some_and(|t| t.chars().any(|c| ('\u{e100}'..='\u{e1ff}').contains(&c)))
        })
    {
        rep.issues.push(Issue::NoLegend);
    }

    if let Some(focus) = &v.focus {
        let mine: Vec<&&Widget> = shown
            .iter()
            .filter(|w| w.id.starts_with(&format!("{focus}.")) || w.id == *focus)
            .collect();
        let on_screen =
            |r: Rect| r.top <= 240 && r.bottom >= -240 && r.left >= -320 && r.right <= 320;
        if !mine
            .iter()
            .any(|w| w.text().is_some() && on_screen(w.rect()))
        {
            rep.issues.push(Issue::FocusHidden { id: focus.clone() });
        } else {
            let alpha_of = |pred: &dyn Fn(&Widget) -> bool| {
                shown
                    .iter()
                    .filter(|w| w.text().is_some() && row_of(&w.id).is_some() && pred(w))
                    .map(|w| w.alpha)
                    .max()
            };
            let focused = alpha_of(&|w| row_of(&w.id) == row_of(focus));
            let others = alpha_of(&|w| row_of(&w.id) != row_of(focus));
            if let (Some(f), Some(o)) = (focused, others)
                && f <= o
            {
                rep.issues
                    .push(Issue::FocusNotBrighter { id: focus.clone() });
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::preview::SyntheticSource;
    use crate::ui_kit::spec::{ListPlace, RowSpec};
    use crate::ui_kit::view::{Geometry, view_of};

    fn spec(rows: usize) -> ScreenSpec {
        let mut s = ScreenSpec::new("t", "Title");
        for i in 0..rows {
            s.rows
                .push(RowSpec::value(&format!("Row {i}"), &["On", "Off"], 0));
        }
        s
    }

    fn checked(s: &ScreenSpec) -> Report {
        let v = view_of(s, &Geometry::fallback(s.list), &SyntheticSource, "t");
        check(&v, &SyntheticSource, &Rules::for_spec(s))
    }

    #[test]
    fn a_plain_list_passes_and_says_what_it_could_not_check() {
        let r = checked(&spec(3));
        assert_eq!(r.issues, vec![]);
        assert_eq!(r.skipped, vec!["covered", "off screen"]);
    }

    /// The first skulls page's frame: the Controls bracket kept under a
    /// list moved to the level select's place on another skin.
    #[test]
    fn a_frame_kept_for_a_moved_list_is_misplaced() {
        let mut s = spec(3);
        s.list = ListPlace::Moved {
            skin: 0,
            anchor: [-215, 130],
            bitmaps_left_of: 0,
            visible: 0,
        };
        let r = checked(&s);
        assert!(
            r.issues
                .iter()
                .any(|i| matches!(i, Issue::Misplaced { art, screen_id: 39 } if art == "frame.0")),
            "{:?}",
            r.issues
        );
        // With the level select's own bracket and none of the Controls art,
        // it passes.
        s.list = ListPlace::Moved {
            skin: 0,
            anchor: [-215, 130],
            bitmaps_left_of: i16::MIN,
            visible: 0,
        };
        s.art = crate::ui_kit::screens::skulls_art()[..1].to_vec();
        assert_eq!(checked(&s).issues, vec![]);
    }

    #[test]
    fn a_long_label_overflows() {
        let mut s = spec(1);
        s.rows[0].label = "x".repeat(50);
        assert!(checked(&s).issues.contains(&Issue::Fit {
            id: "row.0.label".into()
        }));
    }

    #[test]
    fn a_description_over_the_rows_is_found() {
        let mut s = spec(3);
        // The account menu's block: right half, from y 110 down, where rows
        // 1 to 3 are.
        s.description = Some(crate::ui_kit::spec::DescriptionBlock {
            rect: [110, -40, -130, 300],
            font: 1,
            colour: [0.4, 0.6, 0.8],
        });
        s.rows[1].description = Some("Some help".into());
        s.focus = 1;
        let r = checked(&s);
        assert!(
            r.issues
                .iter()
                .any(|i| matches!(i, Issue::OnOtherArt { text, .. } if text == "description")),
            "{:?}",
            r.issues
        );
    }

    #[test]
    fn dim_text_on_its_bar_has_too_little_contrast() {
        let s = spec(3);
        let g = Geometry::fallback(ListPlace::Controls);
        let mut v = view_of(&s, &g, &SyntheticSource, "t");
        for w in v.widgets.iter_mut().filter(|w| w.id == "row.1.label") {
            w.alpha = 30;
        }
        let r = check(&v, &SyntheticSource, &Rules::for_spec(&s));
        assert!(
            r.issues
                .iter()
                .any(|i| matches!(i, Issue::Contrast { id, .. } if id == "row.1.label"))
        );
    }

    #[test]
    fn left_and_right_need_a_legend() {
        let mut s = spec(2);
        s.inputs = vec![Input::Up, Input::Down, Input::Left, Input::Right];
        assert!(checked(&s).issues.contains(&Issue::NoLegend));
        s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        assert!(!checked(&s).issues.contains(&Issue::NoLegend));
    }

    #[test]
    fn text_past_the_safe_area_and_a_dull_focus_are_found() {
        let s = spec(2);
        let g = Geometry::fallback(ListPlace::Controls);
        let mut v = view_of(&s, &g, &SyntheticSource, "t");
        for w in v.widgets.iter_mut() {
            if w.id == "title" {
                w.rect = [235, -310, 211, 0];
            }
            if w.text().is_some() && w.id.starts_with("row.") {
                w.alpha = 255;
            }
        }
        let r = check(&v, &SyntheticSource, &Rules::for_spec(&s));
        assert!(r.issues.contains(&Issue::Unsafe { id: "title".into() }));
        assert!(
            r.issues
                .contains(&Issue::FocusNotBrighter { id: "row.0".into() })
        );
    }

    /// A source whose every art is a solid square of its declared size,
    /// from a screen without a list.
    struct Solid;
    impl Metrics for Solid {
        fn width(&self, f: i16, t: &str) -> i32 {
            crate::ui_kit::view::Synthetic.width(f, t)
        }
        fn line_height(&self, f: i16) -> i32 {
            crate::ui_kit::view::Synthetic.line_height(f)
        }
    }
    impl Source for Solid {
        fn art(&self, _a: &Art) -> Option<crate::ui_kit::preview::Image> {
            Some(crate::ui_kit::preview::Image {
                width: 20,
                height: 20,
                rgba: [255u8; 4].repeat(400),
            })
        }
        fn backing(&self, _c: &mut Canvas, _b: crate::ui_kit::view::Backing) {}
        fn text(&self, _: &mut Canvas, _: i16, _: &str, _: Rect, _: Justify, _: [u8; 3], _: u8) {}
        fn list_of(&self, _: i16, _: u16) -> Option<Option<crate::ui_kit::view::ListAt>> {
            Some(None)
        }
    }

    fn square(id: &str, layer: Layer, x: i16, y: i16) -> Widget {
        Widget {
            id: id.into(),
            rect: [y, x, y - 20, x + 20],
            layer,
            alpha: 255,
            kind: Kind::Art {
                art: Art::Screen {
                    screen_id: 1,
                    pane: 0,
                    index: 0,
                    image: 0,
                },
                size: [20, 20],
            },
        }
    }

    #[test]
    fn art_past_the_edge_and_art_covered_by_a_row_are_found() {
        let v = View {
            screen: "t".into(),
            state: "s".into(),
            backing: crate::ui_kit::view::Backing::None,
            widgets: vec![
                square("frame.0", Layer::Frame, 0, 0),
                square("row.0.art.0", Layer::Under, 10, 0),
                square("frame.1", Layer::Frame, 310, 100),
            ],
            focus: None,
            list: None,
        };
        let r = check(&v, &Solid, &Rules::default());
        assert!(r.skipped.is_empty());
        assert!(r.issues.contains(&Issue::Covered {
            art: "frame.0".into(),
            by: "row.0.art.0".into(),
            pixels: 200,
        }));
        assert!(r.issues.contains(&Issue::OffScreen {
            art: "frame.1".into(),
            pixels: 200,
        }));
    }
}
