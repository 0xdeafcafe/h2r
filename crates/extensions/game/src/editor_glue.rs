//! The editor lease as a sample of world-space lines.
#![cfg(feature = "gpu")]

use std::sync::Arc;
use std::time::Instant;

use agent::editor::{EditorLink, MarkFrame};
use agent_proto::editor::MarkStyle;
use guest_layout::GuestMemory;
use gfx::world_lines::{ImageMarks, ImageValidity, WorldLine};

pub(crate) fn capture(link: &EditorLink, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
    let frame = link.capture_marks(guest, Instant::now)?;
    Some(Arc::new(EditorSample { link: link.clone(), frame }))
}

#[derive(Debug)]
struct EditorSample {
    link: EditorLink,
    frame: MarkFrame,
}

impl ImageMarks for EditorSample {
    fn validity(&self, now: &dyn Fn() -> Instant) -> Option<ImageValidity> {
        let expires = self.link.frame_mark_deadline(&self.frame, now)?;
        Some(ImageValidity::Lease {
            expires,
            generation: self.frame.generation(),
        })
    }

    fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
        for shape in &self.frame.shapes {
            visitor(WorldLine {
                edges: &shape.edges,
                anchor: shape.anchor,
                color: shape.color,
                dashed: shape.style == Some(MarkStyle::Candidate),
                label: shape.label.as_deref(),
                style: Default::default(),
            });
        }
    }
}
