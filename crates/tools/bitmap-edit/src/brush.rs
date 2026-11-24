//! The brush and the eraser: a round stamp, repeated along a stroke. A
//! clone stamp is the same stamp reading its colour from another place
//! of a layer ([`stamp_with`]).

use serde::{Deserialize, Serialize};

use crate::canvas::Canvas;
use crate::color::{Channels, blend};
use crate::error::Error;
use crate::rect::Rect;

/// A brush. The eraser is the same brush with `erase` set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brush {
    /// Diameter in pixels.
    pub size: f32,
    /// 0 is a soft edge across the whole radius, 1 a hard one (one
    /// pixel of anti-aliasing).
    pub hardness: f32,
    /// Coverage of one stamp, 0 to 1. Stamps along a stroke add up, as a
    /// brush's flow does.
    pub opacity: f32,
    /// Distance between stamps as a fraction of `size`.
    pub spacing: f32,
    /// `R,G,B,A`; the alpha scales `opacity` when every channel is
    /// written, and is the value painted when alpha is written alone.
    pub color: [u8; 4],
    /// Take alpha away instead of adding colour.
    pub erase: bool,
    /// The channels written.
    pub channels: Channels,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            size: 16.0,
            hardness: 0.8,
            opacity: 1.0,
            spacing: 0.25,
            color: [255, 255, 255, 255],
            erase: false,
            channels: Channels::ALL,
        }
    }
}

impl Brush {
    /// Checks the parameters are finite and in range.
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |what: &str, v: f32| Error::Params(format!("{what} {v} is out of range"));
        if !(self.size.is_finite() && self.size > 0.0 && self.size <= 4096.0) {
            return Err(bad("size", self.size));
        }
        if !(self.hardness.is_finite() && (0.0..=1.0).contains(&self.hardness)) {
            return Err(bad("hardness", self.hardness));
        }
        if !(self.opacity.is_finite() && (0.0..=1.0).contains(&self.opacity)) {
            return Err(bad("opacity", self.opacity));
        }
        if !(self.spacing.is_finite() && self.spacing > 0.0 && self.spacing <= 4.0) {
            return Err(bad("spacing", self.spacing));
        }
        if !self.channels.any() {
            return Err(Error::Params("no channel is written".into()));
        }
        Ok(())
    }

    /// Coverage 0 to 1 at `dist` pixels from the stamp's centre, before
    /// `opacity`.
    pub fn coverage(&self, dist: f32) -> f32 {
        let r = self.size * 0.5;
        let edge = (r - dist + 0.5).clamp(0.0, 1.0);
        if self.hardness >= 1.0 {
            return edge;
        }
        let inner = r * self.hardness;
        if dist <= inner {
            return edge;
        }
        let t = ((r - dist) / (r - inner)).clamp(0.0, 1.0);
        edge.min(t * t * (3.0 - 2.0 * t))
    }

    /// Distance between stamps, in pixels (at least one).
    pub fn step(&self) -> f32 {
        (self.size * self.spacing).max(1.0)
    }
}

/// One stamp centred at (`cx`, `cy`) (pixel centres are at `x + 0.5`).
/// Returns the pixels it may have changed, clipped to the canvas.
pub fn stamp(canvas: &mut Canvas, brush: &Brush, cx: f32, cy: f32) -> Rect {
    stamp_with(canvas, brush, cx, cy, &|_, _| brush.color)
}

/// One stamp whose colour at each pixel is `colour_at`'s (a clone stamp
/// reads the pixel under another place of a layer); as [`stamp`]
/// otherwise. The blend takes it as the brush's colour.
pub fn stamp_with(
    canvas: &mut Canvas,
    brush: &Brush,
    cx: f32,
    cy: f32,
    colour_at: &dyn Fn(u32, u32) -> [u8; 4],
) -> Rect {
    let r = brush.size * 0.5 + 1.0;
    let x0 = (cx - r).floor().max(0.0) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let x1 = ((cx + r).ceil().max(0.0) as u32).min(canvas.width());
    let y1 = ((cy + r).ceil().max(0.0) as u32).min(canvas.height());
    if x1 <= x0 || y1 <= y0 {
        return Rect::default();
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let a = brush.coverage((dx * dx + dy * dy).sqrt()) * brush.opacity;
            if a <= 0.0 {
                continue;
            }
            let old = canvas.pixel(x, y);
            let new = blend(old, colour_at(x, y), a, brush.erase, brush.channels);
            if new != old {
                canvas.set_pixel(x, y, new);
            }
        }
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// A stroke in progress: the last point and how far the next stamp is.
#[derive(Clone, Debug, Default)]
pub struct Stroke {
    last: Option<(f32, f32)>,
    /// Distance walked since the last stamp.
    walked: f32,
}

impl Stroke {
    pub fn new() -> Stroke {
        Stroke::default()
    }

    /// Extends the stroke to (`x`, `y`): a stamp at the first point, then
    /// stamps every [`Brush::step`] along the line. Returns what the
    /// stamps touched.
    pub fn to(&mut self, canvas: &mut Canvas, brush: &Brush, x: f32, y: f32) -> Rect {
        self.to_with(canvas, brush, x, y, &|_, _| brush.color)
    }

    /// As [`Stroke::to`], each stamp's colour at a pixel being
    /// `colour_at`'s there ([`stamp_with`]).
    pub fn to_with(
        &mut self,
        canvas: &mut Canvas,
        brush: &Brush,
        x: f32,
        y: f32,
        colour_at: &dyn Fn(u32, u32) -> [u8; 4],
    ) -> Rect {
        let Some((lx, ly)) = self.last else {
            self.last = Some((x, y));
            self.walked = 0.0;
            return stamp_with(canvas, brush, x, y, colour_at);
        };
        let (dx, dy) = (x - lx, y - ly);
        let len = (dx * dx + dy * dy).sqrt();
        let step = brush.step();
        let mut dirty = Rect::default();
        if len > 0.0 {
            let mut at = step - self.walked;
            while at <= len {
                let t = at / len;
                dirty = dirty.union(stamp_with(
                    canvas,
                    brush,
                    lx + dx * t,
                    ly + dy * t,
                    colour_at,
                ));
                at += step;
            }
            // `at` is the next stamp's distance along this segment.
            self.walked = len - (at - step);
        }
        self.last = Some((x, y));
        dirty
    }
}

/// A whole stroke through `points`; the union of what it touched.
pub fn paint(canvas: &mut Canvas, brush: &Brush, points: &[[f32; 2]]) -> Rect {
    let mut s = Stroke::new();
    let mut dirty = Rect::default();
    for p in points {
        dirty = dirty.union(s.to(canvas, brush, p[0], p[1]));
    }
    dirty
}
