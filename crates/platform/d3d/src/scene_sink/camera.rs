//! Classify carried draws from their original vertex program and constants
//! while the replay and the matching sealed scene are together.

use std::collections::HashMap;

use frame_types::carried::CameraUse;
use frame_types::{Camera, Slot, SlotAt, ViewKey, ViewKind};

use super::replay::{ReplayFrame, ReplayOp};
use crate::backend::{ResolvedDraw, Viewport};
use crate::clip_space::epilogue::{self, Position};
use crate::clip_space::pass_through::pass_through;
use crate::shader::{Microcode, VertexProcessing};

pub(super) struct ViewBlock {
    pub(super) index: u8,
    rect: [u32; 4],
    // In hardware register order, including the sign of each zero.
    pub(super) words: [[u32; 4]; 4],
}

impl ViewBlock {
    pub(super) fn from_view((key, cam): (ViewKey, &Camera)) -> Option<Self> {
        if key.kind != ViewKind::Game {
            return None;
        }
        let [top, left, bottom, right] = cam.viewport;
        if top < 0 || left < 0 || !cam.is_usable() {
            return None;
        }
        let matrix = cam.view_projection();
        if !matrix.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        Some(Self {
            index: key.view,
            rect: [
                left as u32,
                top as u32,
                (right - left) as u32,
                (bottom - top) as u32,
            ],
            words: std::array::from_fn(|row| matrix.map(|column| column[row].to_bits())),
        })
    }

    fn matches(&self, viewport: &Viewport, constants: &[[f32; 4]]) -> bool {
        self.rect == [viewport.x, viewport.y, viewport.width, viewport.height]
            && constants
                .iter()
                .zip(self.words)
                .all(|(actual, expected)| actual.map(f32::to_bits) == expected)
    }
}

/// The projection used by the program's position output. A perspective
/// matrix sitting elsewhere in its register file tells us nothing.
fn position(program: &Microcode) -> Position {
    if !program.ends_at_first_final() {
        return Position::Unknown;
    }
    match epilogue::position(program) {
        // Fixed also includes programs which never write a usable position.
        // Only label pixels passed straight through their input as screen.
        Position::Fixed if pass_through(program).is_none() => Position::Unknown,
        other => other,
    }
}

// `Microcode`'s interior cache takes no part in its Hash or Eq.
#[allow(clippy::mutable_key_type)]
pub(super) fn classify(
    draw: &ResolvedDraw,
    at: SlotAt,
    views: &[ViewBlock],
    programs: &mut HashMap<Microcode, Position>,
) -> CameraUse {
    let VertexProcessing::Program { microcode, .. } = &draw.shader.vertex else {
        return if draw.layout.pretransformed {
            CameraUse::Screen
        } else {
            CameraUse::Unknown
        };
    };
    let pos = *programs
        .entry(microcode.clone())
        .or_insert_with(|| position(microcode));
    match pos {
        Position::Fixed => CameraUse::Screen,
        Position::Rows(first) if at.slot.in_game_view() => {
            // A view index alone cannot identify its camera if the guest
            // re-entered that view with a second camera in one frame.
            let mut candidates = views.iter().filter(|v| v.index == at.view);
            let Some(view) = candidates.next() else {
                return CameraUse::Unknown;
            };
            let start = usize::from(first);
            if candidates.next().is_none()
                && draw
                    .shader
                    .constants
                    .get(start..start + 4)
                    .is_some_and(|rows| view.matches(&draw.viewport, rows))
            {
                CameraUse::View {
                    view: at.view,
                    first,
                }
            } else {
                CameraUse::Unknown
            }
        }
        _ => CameraUse::Unknown,
    }
}

impl ReplayFrame {
    /// Classify once at seal, from the unscaled draw packets, against the
    /// matching sealed scene's views (`FrameScene::view_cameras`).
    /// Reclassifying replaces all prior labels, including those a different
    /// view proved.
    pub fn classify_cameras<'a>(&mut self, views: impl IntoIterator<Item = (ViewKey, &'a Camera)>) {
        let views: Vec<_> = views.into_iter().filter_map(ViewBlock::from_view).collect();
        // `Microcode` hashes and compares its program words; its interior cache
        // never takes part, so it is a sound key.
        #[allow(clippy::mutable_key_type)]
        let mut programs = HashMap::new();
        for (at, op) in &mut self.ops {
            if let ReplayOp::Draw { draw, camera, .. } = op {
                *camera = classify(draw, *at, &views, &mut programs);
            }
        }
    }

    /// The carried draws of this game view, including its overlay. Order
    /// is screen, verified view, unknown. Outside draws have no game view.
    pub fn camera_counts(&self, view: u8) -> [u32; 3] {
        let mut counts = [0u32; 3];
        for (at, op) in &self.ops {
            if at.view == view
                && at.slot != Slot::Outside
                && let ReplayOp::Draw { camera, .. } = op
            {
                let i = match camera {
                    CameraUse::Screen => 0,
                    CameraUse::View { .. } => 1,
                    CameraUse::Unknown => 2,
                };
                counts[i] += 1;
            }
        }
        counts
    }
}
