//! A bitmap's layers, the ops on them as values, undo and the dirty log.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::adjust::{self, Adjust};
use crate::brush::{self, Brush, Stroke};
use crate::canvas::Canvas;
use crate::color::Channels;
use crate::error::Error;
use crate::fill;
use crate::rect::Rect;
use crate::resample::resample;
use crate::transform::{self, Channel, Turn};

/// Undo steps kept (the oldest drop first). A step is a tile table, so
/// this costs pointers, and the tiles an edit changed.
pub const UNDO_LIMIT: usize = 256;

/// One layer of a bitmap: an image's face (cube maps) or depth slice
/// (volumes), at mip 0. A plain 2D image has the one layer `0, 0, 0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId {
    /// The image's index in the bitmap.
    pub image: u32,
    pub face: u32,
    pub slice: u32,
}

impl LayerId {
    pub const fn new(image: u32, face: u32, slice: u32) -> LayerId {
        LayerId { image, face, slice }
    }
}

impl fmt::Display for LayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.image, self.face, self.slice)
    }
}

/// One editable layer: its canvas now, and as it was opened (which shares
/// every tile that has not been written to).
#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub canvas: Canvas,
    pub source: Canvas,
    /// Why the layer cannot be edited (its format's channels are only a
    /// hypothesis), or `None`.
    pub locked: Option<String>,
}

impl Layer {
    /// The tiles written since the layer was opened.
    pub fn changed_tiles(&self) -> Vec<(u32, u32)> {
        self.canvas.changed_tiles(&self.source)
    }

    pub fn is_edited(&self) -> bool {
        !self.changed_tiles().is_empty()
    }
}

/// An edit, as a value: what `bitmap.*` methods and the window's tools
/// both perform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BitmapOp {
    /// One stroke through `points` (pixel coordinates, a pixel's centre
    /// at `x + 0.5`): a stamp at every point and, from each to the next,
    /// stamps at most the brush's spacing apart, so two points are a
    /// line.
    Paint {
        layer: LayerId,
        points: Vec<[f32; 2]>,
        brush: Brush,
    },
    /// A clone stroke through `points`, placed as [`BitmapOp::Paint`]'s:
    /// each stamp copies the pixels under it from `source` (a point of
    /// the layer `from`, the layer painted without one), as they are now,
    /// at the offset from `source` to the stroke's first point. The
    /// brush's `erase` is ignored.
    Clone {
        layer: LayerId,
        #[serde(default)]
        from: Option<LayerId>,
        source: [f32; 2],
        points: Vec<[f32; 2]>,
        brush: Brush,
    },
    /// Flood from `at` (limited to `rect` when given), or, without `at`,
    /// every pixel of `rect`.
    Fill {
        layer: LayerId,
        #[serde(default)]
        at: Option<[u32; 2]>,
        #[serde(default)]
        rect: Option<Rect>,
        color: [u8; 4],
        #[serde(default)]
        tolerance: f32,
        #[serde(default)]
        channels: Channels,
    },
    Adjust {
        layer: LayerId,
        adjust: Adjust,
        #[serde(default)]
        rect: Option<Rect>,
        #[serde(default)]
        channels: Channels,
    },
    /// `rgba` (`width * height`) written into `rect` (the whole layer
    /// without one), resampled when the sizes differ.
    Import {
        layer: LayerId,
        #[serde(default)]
        rect: Option<Rect>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// What `from` reads of every pixel of `rect` (the whole layer
    /// without one) written to the channels `to`; the other channels stay.
    CopyChannel {
        layer: LayerId,
        from: Channel,
        to: Channels,
        #[serde(default)]
        rect: Option<Rect>,
    },
    /// The pixels of `rect` (the whole layer without one) flipped or
    /// turned by `turn`; the rest of the layer stays.
    Turn {
        layer: LayerId,
        turn: Turn,
        #[serde(default)]
        rect: Option<Rect>,
    },
}

impl BitmapOp {
    pub fn layer(&self) -> LayerId {
        match self {
            BitmapOp::Paint { layer, .. }
            | BitmapOp::Clone { layer, .. }
            | BitmapOp::Fill { layer, .. }
            | BitmapOp::Adjust { layer, .. }
            | BitmapOp::Import { layer, .. }
            | BitmapOp::CopyChannel { layer, .. }
            | BitmapOp::Turn { layer, .. } => *layer,
        }
    }
}

/// What an edit changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub layer: LayerId,
    /// The pixels it may have changed; empty when it changed none.
    pub dirty: Rect,
    /// The tiles `dirty` overlaps.
    pub tiles: Vec<(u32, u32)>,
}

/// Dirty pixels of a layer since the log was last taken
/// ([`Bitmap::take_dirty`]): what a live push or a re-encode reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub layer: LayerId,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
struct Step {
    layer: LayerId,
    canvas: Canvas,
}

/// A clone stroke in progress.
#[derive(Clone, Debug)]
struct Cloning {
    /// The layer read, as it was when the stroke began (so the stroke
    /// never copies from what it has painted).
    source: Canvas,
    /// The point its first stamp copies.
    at: [f32; 2],
    /// From the source to the stroke, in whole pixels; set by the first
    /// stamp.
    offset: Option<(i64, i64)>,
}

#[derive(Clone, Debug)]
struct Active {
    layer: LayerId,
    before: Canvas,
    brush: Brush,
    stroke: Stroke,
    dirty: Rect,
    /// `Some` for a clone stroke.
    clone: Option<Cloning>,
}

impl Active {
    /// What the stroke's stamps at (`x`, `y`) touched. A drag (`spaced`)
    /// goes on from its last point at the brush's spacing
    /// ([`Stroke::to`]); otherwise it is the one stamp there. A clone
    /// stroke reads its source at the offset its first stamp sets, and
    /// past the source's edge paints nothing (a transparent pixel over
    /// leaves the pixel as it is).
    fn stamp(&mut self, canvas: &mut Canvas, x: f32, y: f32, spaced: bool) -> Rect {
        let r = match &mut self.clone {
            None if spaced => self.stroke.to(canvas, &self.brush, x, y),
            None => brush::stamp(canvas, &self.brush, x, y),
            Some(c) => {
                // The first stamp sets the offset from the source point.
                let offset = match c.offset {
                    Some(offset) => offset,
                    None => {
                        let offset = ((x - c.at[0]).round() as i64, (y - c.at[1]).round() as i64);
                        c.offset = Some(offset);
                        offset
                    }
                };
                let colour_at = &|px: u32, py: u32| {
                    c.source
                        .pixel_at(i64::from(px) - offset.0, i64::from(py) - offset.1)
                };
                if spaced {
                    self.stroke.to_with(canvas, &self.brush, x, y, colour_at)
                } else {
                    brush::stamp_with(canvas, &self.brush, x, y, colour_at)
                }
            }
        };
        self.dirty = self.dirty.union(r);
        r
    }
}

/// The layers of one bitmap tag, with undo.
#[derive(Debug, Default)]
pub struct Bitmap {
    layers: Vec<Layer>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    pending: Vec<Dirty>,
    /// The same log for the window's canvas, which reads it apart from
    /// the live push.
    view: Vec<Dirty>,
    active: Option<Active>,
    rev: u64,
}

impl Bitmap {
    pub fn new() -> Bitmap {
        Bitmap::default()
    }

    /// Adds a layer holding `canvas`, as opened. `locked` is why it cannot
    /// be edited, if it cannot.
    pub fn add_layer(&mut self, id: LayerId, canvas: Canvas, locked: Option<String>) {
        let source = canvas.clone();
        self.layers.push(Layer {
            id,
            canvas,
            source,
            locked,
        });
    }

    /// Takes every layer of image `image` out (an added image undone), in
    /// order. Refused, with none taken, during a stroke.
    pub fn remove_image(&mut self, image: u32) -> Vec<Layer> {
        if self.active.is_some() {
            return Vec::new();
        }
        let (out, keep): (Vec<Layer>, Vec<Layer>) = std::mem::take(&mut self.layers)
            .into_iter()
            .partition(|l| l.id.image == image);
        self.layers = keep;
        if !out.is_empty() {
            self.rev += 1;
        }
        out
    }

    /// Puts layers taken by [`Bitmap::remove_image`] back as they were,
    /// edits and all (an added image redone).
    pub fn restore_layers(&mut self, layers: Vec<Layer>) {
        if layers.is_empty() {
            return;
        }
        self.rev += 1;
        for l in layers {
            self.view.push(Dirty {
                layer: l.id,
                rect: l.canvas.bounds(),
            });
            self.layers.push(l);
        }
    }

    /// Forgets the redo steps: an edit outside the layers' pixels (an
    /// image added) was made after them.
    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// Changes with every edit, undo and redo.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Whether any layer differs from how it was opened.
    pub fn is_edited(&self) -> bool {
        self.layers.iter().any(Layer::is_edited)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    fn index(&self, id: LayerId) -> Result<usize, Error> {
        self.layers
            .iter()
            .position(|l| l.id == id)
            .ok_or(Error::NoLayer(id))
    }

    fn writable(&self, id: LayerId) -> Result<usize, Error> {
        let i = self.index(id)?;
        match &self.layers[i].locked {
            Some(why) => Err(Error::ReadOnly(why.clone())),
            None => Ok(i),
        }
    }

    fn note(&mut self, layer: LayerId, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        self.rev += 1;
        for log in [&mut self.pending, &mut self.view] {
            match log.iter_mut().find(|d| d.layer == layer) {
                Some(d) => d.rect = d.rect.union(rect),
                None => log.push(Dirty { layer, rect }),
            }
        }
    }

    fn record(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// The pixels changed since the last call, one rectangle per layer.
    pub fn take_dirty(&mut self) -> Vec<Dirty> {
        std::mem::take(&mut self.pending)
    }

    /// The pixels changed since the last call, for the canvas on screen:
    /// a log of its own, so the live push and the window each see every
    /// change.
    pub fn take_view_dirty(&mut self) -> Vec<Dirty> {
        std::mem::take(&mut self.view)
    }

    /// The pixels changed since the last [`Bitmap::take_dirty`], not
    /// taken.
    pub fn dirty(&self) -> &[Dirty] {
        &self.pending
    }

    /// `r` of the layer, clipped to it.
    pub fn pixels(&self, id: LayerId, r: Rect) -> Result<(Rect, Vec<u8>), Error> {
        let l = self.layer(id).ok_or(Error::NoLayer(id))?;
        Ok(l.canvas.read_rect(r))
    }

    /// Performs an edit: one undo step, all or nothing. A stroke or a
    /// turn that changes no pixel is no step.
    pub fn apply(&mut self, op: &BitmapOp) -> Result<Applied, Error> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        let id = op.layer();
        match op {
            BitmapOp::Paint { points, brush, .. } => {
                return self.stroke_op(id, points, brush.clone(), None);
            }
            BitmapOp::Clone {
                points,
                brush,
                from,
                source,
                ..
            } => {
                if source.iter().any(|v| !v.is_finite()) {
                    return Err(Error::Params("the clone source is not a point".to_owned()));
                }
                return self.stroke_op(id, points, brush.clone(), Some((*from, *source)));
            }
            _ => {}
        }
        let i = self.writable(id)?;
        let before = self.layers[i].canvas.clone();
        let result = run(&mut self.layers[i].canvas, op);
        let dirty = match result {
            Ok(d) => d,
            Err(e) => {
                self.layers[i].canvas = before;
                return Err(e);
            }
        };
        if dirty.is_empty() {
            self.layers[i].canvas = before;
            return Ok(Applied {
                layer: id,
                ..Applied::default()
            });
        }
        self.record(Step {
            layer: id,
            canvas: before,
        });
        self.note(id, dirty);
        Ok(Applied {
            layer: id,
            dirty,
            tiles: self.layers[i].canvas.tiles_in(dirty),
        })
    }

    /// Starts a stroke on a layer: for a drag, which calls
    /// [`Bitmap::stroke_to`] as the mouse moves and [`Bitmap::end_stroke`]
    /// when it lets go (one undo step).
    pub fn begin_stroke(&mut self, layer: LayerId, brush: Brush) -> Result<(), Error> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        brush.validate()?;
        let i = self.writable(layer)?;
        self.active = Some(Active {
            layer,
            before: self.layers[i].canvas.clone(),
            brush,
            stroke: Stroke::new(),
            dirty: Rect::default(),
            clone: None,
        });
        Ok(())
    }

    /// Starts a clone stroke on a layer, for a drag as [`Bitmap::begin_stroke`]:
    /// each stamp copies the pixels under it from `source` (a point of the
    /// layer `from`, the layer painted without one), as they are now, at
    /// the offset from `source` to the stroke's first point (whole
    /// pixels). The brush's `erase` is ignored: a clone stroke copies.
    pub fn begin_clone(
        &mut self,
        layer: LayerId,
        mut brush: Brush,
        from: Option<LayerId>,
        source: [f32; 2],
    ) -> Result<(), Error> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        brush.validate()?;
        if source.iter().any(|v| !v.is_finite()) {
            return Err(Error::Params("the clone source is not a point".to_owned()));
        }
        let i = self.writable(layer)?;
        let from = from.unwrap_or(layer);
        let snapshot = if from == layer {
            self.layers[i].canvas.clone()
        } else {
            let j = self
                .layers
                .iter()
                .position(|l| l.id == from)
                .ok_or(Error::NoLayer(from))?;
            self.layers[j].canvas.clone()
        };
        brush.erase = false;
        self.active = Some(Active {
            layer,
            before: self.layers[i].canvas.clone(),
            brush,
            stroke: Stroke::new(),
            dirty: Rect::default(),
            clone: Some(Cloning {
                source: snapshot,
                at: source,
                offset: None,
            }),
        });
        Ok(())
    }

    /// A stroke as one op ([`BitmapOp::Paint`], [`BitmapOp::Clone`]): a
    /// stamp at every point of `points` and, from each to the next, stamps
    /// at most the brush's spacing apart with the last on the next point,
    /// so two points are a line from one to the other. One undo step.
    fn stroke_op(
        &mut self,
        layer: LayerId,
        points: &[[f32; 2]],
        brush: Brush,
        clone: Option<(Option<LayerId>, [f32; 2])>,
    ) -> Result<Applied, Error> {
        if points.is_empty() {
            return Err(Error::Params(
                "a stroke needs at least one point".to_owned(),
            ));
        }
        if points
            .iter()
            .any(|p| !p[0].is_finite() || !p[1].is_finite())
        {
            return Err(Error::Params("a stroke point is not a number".to_owned()));
        }
        let (step, reach) = (brush.step(), brush.size * 0.5 + 1.0);
        match clone {
            Some((from, source)) => self.begin_clone(layer, brush, from, source)?,
            None => self.begin_stroke(layer, brush)?,
        }
        let (w, h) = self
            .layers
            .iter()
            .find(|l| l.id == layer)
            .map_or((0, 0), |l| (l.canvas.width(), l.canvas.height()));
        self.stamp_at(points[0][0], points[0][1])?;
        for pair in points.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
            // A point given twice is stamped once.
            if len == 0.0 {
                continue;
            }
            // At least one, and the last is the point itself.
            let stamps = (len / step).ceil().max(1.0) as u32;
            // Only the stamps a brush at the point could reach the canvas
            // with are placed.
            let Some((t0, t1)) = clip(from, to, -reach, w as f32 + reach, -reach, h as f32 + reach)
            else {
                continue;
            };
            let first = ((t0 * (stamps as f32)).ceil() as u32).clamp(1, stamps);
            let last = ((t1 * (stamps as f32)).floor() as u32).min(stamps);
            for k in first..=last {
                let (cx, cy) = if k == stamps {
                    (to[0], to[1])
                } else {
                    let t = (k as f32) / (stamps as f32);
                    (
                        from[0] + (to[0] - from[0]) * t,
                        from[1] + (to[1] - from[1]) * t,
                    )
                };
                self.stamp_at(cx, cy)?;
            }
        }
        Ok(self.end_stroke().unwrap_or(Applied {
            layer,
            ..Applied::default()
        }))
    }

    /// Extends the stroke to (`x`, `y`); returns the pixels this touched.
    pub fn stroke_to(&mut self, x: f32, y: f32) -> Result<Rect, Error> {
        if !x.is_finite() || !y.is_finite() {
            return Err(Error::Params("a stroke point is not a number".to_owned()));
        }
        let Some(a) = self.active.as_mut() else {
            return Err(Error::NoStroke);
        };
        let layer = a.layer;
        let i = self
            .layers
            .iter()
            .position(|l| l.id == layer)
            .ok_or(Error::NoLayer(layer))?;
        let r = a.stamp(&mut self.layers[i].canvas, x, y, true);
        self.note(layer, r);
        Ok(r)
    }

    /// The one stamp of the stroke in progress at (`x`, `y`), wherever
    /// [`Bitmap::stroke_op`] puts its stamps.
    fn stamp_at(&mut self, x: f32, y: f32) -> Result<Rect, Error> {
        if !x.is_finite() || !y.is_finite() {
            return Err(Error::Params("a stroke point is not a number".to_owned()));
        }
        let Some(a) = self.active.as_mut() else {
            return Err(Error::NoStroke);
        };
        let layer = a.layer;
        let i = self
            .layers
            .iter()
            .position(|l| l.id == layer)
            .ok_or(Error::NoLayer(layer))?;
        let r = a.stamp(&mut self.layers[i].canvas, x, y, false);
        self.note(layer, r);
        Ok(r)
    }

    /// Ends the stroke: one undo step when it changed anything. A stroke
    /// whose stamps wrote no pixel a value it did not have (a stamp of
    /// the colour already there, a clone stroke over its source) is no
    /// step.
    pub fn end_stroke(&mut self) -> Option<Applied> {
        let a = self.active.take()?;
        let changed = self
            .layer(a.layer)
            .is_some_and(|l| !l.canvas.changed_tiles(&a.before).is_empty());
        if a.dirty.is_empty() || !changed {
            return Some(Applied {
                layer: a.layer,
                ..Applied::default()
            });
        }
        let tiles = self
            .layer(a.layer)
            .map(|l| l.canvas.tiles_in(a.dirty))
            .unwrap_or_default();
        self.record(Step {
            layer: a.layer,
            canvas: a.before,
        });
        Some(Applied {
            layer: a.layer,
            dirty: a.dirty,
            tiles,
        })
    }

    /// Drops the stroke in progress and its pixels.
    pub fn cancel_stroke(&mut self) {
        if let Some(a) = self.active.take()
            && let Some(l) = self.layers.iter_mut().find(|l| l.id == a.layer)
        {
            let rect = a.dirty;
            l.canvas = a.before;
            self.note(a.layer, rect);
        }
    }

    pub fn stroke_active(&self) -> bool {
        self.active.is_some()
    }

    fn swap(&mut self, from_undo: bool) -> Option<Applied> {
        if self.active.is_some() {
            return None;
        }
        let step = if from_undo {
            self.undo.pop()?
        } else {
            self.redo.pop()?
        };
        let l = self.layers.iter_mut().find(|l| l.id == step.layer)?;
        let now = std::mem::replace(&mut l.canvas, step.canvas);
        let dirty = l.canvas.changed_rect(&now);
        let tiles = l.canvas.changed_tiles(&now);
        let layer = step.layer;
        let other = Step { layer, canvas: now };
        if from_undo {
            self.redo.push(other);
        } else {
            self.undo.push(other);
        }
        self.note(layer, dirty);
        Some(Applied {
            layer,
            dirty,
            tiles,
        })
    }

    /// Undoes the last edit; what changed, or `None` with nothing to undo.
    pub fn undo(&mut self) -> Option<Applied> {
        self.swap(true)
    }

    pub fn redo(&mut self) -> Option<Applied> {
        self.swap(false)
    }
}

fn run(canvas: &mut Canvas, op: &BitmapOp) -> Result<Rect, Error> {
    match op {
        BitmapOp::Paint { .. } | BitmapOp::Clone { .. } => {
            Err(Error::Params("a stroke is not a one-shot op".to_owned()))
        }
        BitmapOp::CopyChannel { from, to, rect, .. } => {
            if !to.any() {
                return Err(Error::Params("no channel is written".to_owned()));
            }
            let r = inside(canvas, *rect)?;
            transform::copy_channel(canvas, *from, *to, r)
        }
        BitmapOp::Turn { turn: t, rect, .. } => {
            let r = inside(canvas, *rect)?;
            transform::turn(canvas, *t, r)
        }
        BitmapOp::Fill {
            at,
            rect,
            color,
            tolerance,
            channels,
            ..
        } => {
            if !channels.any() {
                return Err(Error::Params("no channel is written".to_owned()));
            }
            match (at, rect) {
                (Some([x, y]), _) => fill::flood(canvas, *x, *y, *color, *tolerance, *rect, *channels),
                (None, Some(r)) => {
                    if !canvas.bounds().encloses(*r) {
                        return Err(Error::Rect(format!(
                            "{}x{} at {},{} is outside the {}x{} layer",
                            r.w,
                            r.h,
                            r.x,
                            r.y,
                            canvas.width(),
                            canvas.height()
                        )));
                    }
                    Ok(fill::fill_rect(canvas, *r, *color, *channels))
                }
                (None, None) => Err(Error::Params(
                    "a fill needs a point (`at`) or a rectangle".to_owned(),
                )),
            }
        }
        BitmapOp::Adjust {
            adjust,
            rect,
            channels,
            ..
        } => {
            if !channels.any() {
                return Err(Error::Params("no channel is written".to_owned()));
            }
            let r = rect.unwrap_or_else(|| canvas.bounds());
            adjust::apply(canvas, adjust, r, *channels)
        }
        BitmapOp::Import {
            rect,
            width,
            height,
            rgba,
            ..
        } => {
            let target = rect.unwrap_or_else(|| canvas.bounds());
            if target.is_empty() || !canvas.bounds().encloses(target) {
                return Err(Error::Rect(format!(
                    "{}x{} at {},{} is not inside the {}x{} layer",
                    target.w,
                    target.h,
                    target.x,
                    target.y,
                    canvas.width(),
                    canvas.height()
                )));
            }
            let data = resample(rgba, *width, *height, target.w, target.h)?;
            canvas.write_rect(target, &data)?;
            Ok(target)
        }
    }
}

/// `rect`, or the whole canvas; it must lie inside the canvas.
fn inside(canvas: &Canvas, rect: Option<Rect>) -> Result<Rect, Error> {
    let r = rect.unwrap_or_else(|| canvas.bounds());
    if !canvas.bounds().encloses(r) {
        return Err(Error::Rect(format!(
            "{}x{} at {},{} is outside the {}x{} layer",
            r.w,
            r.h,
            r.x,
            r.y,
            canvas.width(),
            canvas.height()
        )));
    }
    Ok(r)
}

/// The part of the segment from `a` to `b` that lies in the rectangle
/// `x0..=x1`, `y0..=y1`, as `t` from 0 to 1; `None` when none of it does.
fn clip(a: [f32; 2], b: [f32; 2], x0: f32, x1: f32, y0: f32, y1: f32) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (axis, (lo, hi)) in [(x0, x1), (y0, y1)].into_iter().enumerate() {
        let (s, e) = (a[axis], b[axis]);
        let d = e - s;
        if d == 0.0 {
            if s < lo || s > hi {
                return None;
            }
            continue;
        }
        let (at_lo, at_hi) = ((lo - s) / d, (hi - s) / d);
        if d > 0.0 {
            t0 = t0.max(at_lo);
            t1 = t1.min(at_hi);
        } else {
            t0 = t0.max(at_hi);
            t1 = t1.min(at_lo);
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// Brush strokes without a [`Bitmap`], for a canvas on its own: `paint`
/// through `points`.
pub fn paint_canvas(canvas: &mut Canvas, brush: &Brush, points: &[[f32; 2]]) -> Result<Rect, Error> {
    brush.validate()?;
    Ok(brush::paint(canvas, brush, points))
}
