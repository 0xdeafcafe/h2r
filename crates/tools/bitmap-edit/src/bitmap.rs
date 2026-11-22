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
    /// at `x + 0.5`).
    Paint {
        layer: LayerId,
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
}

impl BitmapOp {
    pub fn layer(&self) -> LayerId {
        match self {
            BitmapOp::Paint { layer, .. }
            | BitmapOp::Fill { layer, .. }
            | BitmapOp::Adjust { layer, .. }
            | BitmapOp::Import { layer, .. } => *layer,
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

#[derive(Clone, Debug)]
struct Active {
    layer: LayerId,
    before: Canvas,
    brush: Brush,
    stroke: Stroke,
    dirty: Rect,
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

    /// Performs an edit: one undo step, all or nothing.
    pub fn apply(&mut self, op: &BitmapOp) -> Result<Applied, Error> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        let id = op.layer();
        if let BitmapOp::Paint { points, brush, .. } = op {
            if points.is_empty() {
                return Err(Error::Params("a stroke needs at least one point".to_owned()));
            }
            if points.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
                return Err(Error::Params("a stroke point is not a number".to_owned()));
            }
            self.begin_stroke(id, brush.clone())?;
            for p in points {
                self.stroke_to(p[0], p[1])?;
            }
            return Ok(self.end_stroke().unwrap_or(Applied {
                layer: id,
                ..Applied::default()
            }));
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
        });
        Ok(())
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
        let r = a.stroke.to(&mut self.layers[i].canvas, &a.brush, x, y);
        a.dirty = a.dirty.union(r);
        self.note(layer, r);
        Ok(r)
    }

    /// Ends the stroke: one undo step when it changed anything.
    pub fn end_stroke(&mut self) -> Option<Applied> {
        let a = self.active.take()?;
        if a.dirty.is_empty() {
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
        BitmapOp::Paint { .. } => Err(Error::Params("a stroke is not a one-shot op".to_owned())),
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

/// Brush strokes without a [`Bitmap`], for a canvas on its own: `paint`
/// through `points`.
pub fn paint_canvas(canvas: &mut Canvas, brush: &Brush, points: &[[f32; 2]]) -> Result<Rect, Error> {
    brush.validate()?;
    Ok(brush::paint(canvas, brush, points))
}
