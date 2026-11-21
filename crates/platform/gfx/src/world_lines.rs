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
}

/// How long a sample can appear when its image is shown again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageValidity {
    Image,
    Lease {
        expires: Instant,
        generation: u64,
    },
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
        if let Some(ImageValidity::Lease { expires, generation }) = self
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
