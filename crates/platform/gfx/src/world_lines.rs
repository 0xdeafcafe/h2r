//! Immutable world-space geometry carried by a presented image.

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Instant;

use smallvec::SmallVec;

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
    /// Size of one font pixel of the label (1 is the 8x8 font as is).
    pub label_scale: f32,
    /// Centre the label on its anchor, with a drop shadow, and scale it
    /// by the output height over 1080p (at least 1). Pulse and fade apply
    /// to it too.
    pub label_centered: bool,
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
            label_scale: 1.0,
            label_centered: false,
        }
    }
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
}
