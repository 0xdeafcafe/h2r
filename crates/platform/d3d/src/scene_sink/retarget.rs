//! A sealed frame's draws moved onto blended view cameras, for a display
//! frame between ticks (docs/graphics/framescene.md, section 7: carried
//! draws with `CameraUse::View` get the view block of the blended camera;
//! world-space geometry stays where tick `B` put it). Screen-space and
//! unverified draws are drawn as the tick drew them.
//!
//! A carried draw's camera was verified at seal (`ReplayFrame::
//! classify_cameras`): its four position rows are bit for bit the view
//! camera's. A typed stage's draw is classified here against the same
//! view blocks and moved through its pass ([`ExternalPass::with_draw`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use frame_types::carried::CameraUse;
use frame_types::{Camera, SlotAt, ViewKey, ViewKind};

use super::camera::{ViewBlock, classify};
use super::replay::ReplayOp;
use crate::backend::{ExternalPass, ResolvedDraw, SceneStage};
use crate::clip_space::epilogue::Position;
use crate::shader::Microcode;

type Rows = [[f32; 4]; 4];

/// The four constant rows `camera` puts in a program's registers, in
/// hardware register order (as [`ViewBlock`] holds them).
fn rows_of(camera: &Camera) -> Option<Rows> {
    if !camera.is_usable() {
        return None;
    }
    let m = camera.view_projection();
    let rows: Rows = std::array::from_fn(|row| m.map(|column| column[row]));
    rows.iter().flatten().all(|v| v.is_finite()).then_some(rows)
}

fn with_rows(draw: &ResolvedDraw, first: usize, rows: &Rows) -> Option<ResolvedDraw> {
    let mut constants = *draw.shader.constants;
    constants.get_mut(first..first + 4)?.copy_from_slice(rows);
    let mut moved = draw.clone();
    moved.shader.constants = Arc::new(constants);
    Some(moved)
}

/// What [`Retarget::op`] did over a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetargetCounts {
    /// Carried draws whose view rows took the blended camera.
    pub draws: u32,
    /// Typed stages whose draw took the blended camera.
    pub stages: u32,
    /// Typed stages in a game view drawn as the tick drew them: the pass
    /// cannot take another draw, or its camera is not the view's.
    pub held_stages: u32,
}

/// An op as the display frame draws it.
pub enum Retargeted<'a> {
    Same(&'a ReplayOp),
    Moved(ReplayOp),
}

impl Retargeted<'_> {
    pub fn get(&self) -> &ReplayOp {
        match self {
            Self::Same(op) => op,
            Self::Moved(op) => op,
        }
    }
}

// `Microcode`'s interior cache takes no part in its Hash or Eq.
#[allow(clippy::mutable_key_type)]
pub struct Retarget {
    /// Tick `B`'s view blocks, which its draws were verified against.
    views: Vec<ViewBlock>,
    /// The blended rows by view index, for views whose camera moved.
    rows: HashMap<u8, Rows>,
    programs: HashMap<Microcode, Position>,
    counts: RetargetCounts,
}

#[allow(clippy::mutable_key_type)]
impl Retarget {
    /// `from`: tick `B`'s view cameras; `to`: the cameras the display
    /// frame draws with, paired by view key. A view that two of `B`'s
    /// cameras share is never moved (classification cannot tell them
    /// apart).
    pub fn new<'a>(
        from: impl IntoIterator<Item = (ViewKey, &'a Camera)>,
        to: impl IntoIterator<Item = (ViewKey, &'a Camera)>,
    ) -> Self {
        let views: Vec<ViewBlock> = from.into_iter().filter_map(ViewBlock::from_view).collect();
        let mut rows = HashMap::new();
        for (key, camera) in to {
            if key.kind != ViewKind::Game {
                continue;
            }
            let mut same = views.iter().filter(|v| v.index == key.view);
            let (Some(block), None) = (same.next(), same.next()) else {
                continue;
            };
            let Some(moved) = rows_of(camera) else {
                continue;
            };
            if moved.map(|row| row.map(f32::to_bits)) != block.words {
                rows.insert(key.view, moved);
            }
        }
        Self { views, rows, programs: HashMap::new(), counts: RetargetCounts::default() }
    }

    /// No view moved: every op draws as the tick drew it.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn counts(&self) -> RetargetCounts {
        self.counts
    }

    /// `op` at `at`, moved onto its view's blended camera where it can be.
    pub fn op<'o>(&mut self, at: SlotAt, op: &'o ReplayOp) -> Retargeted<'o> {
        match op {
            ReplayOp::Draw { draw, textures, camera: CameraUse::View { view, first }, external } => {
                use timeline::native::Stage;
                if external.as_ref().is_some_and(|admission| {
                    !matches!(admission.plan.stage, Stage::Transparents | Stage::Particles)
                }) {
                    return Retargeted::Same(op);
                }
                let Some(rows) = self.rows.get(view) else {
                    return Retargeted::Same(op);
                };
                let Some(moved) = with_rows(draw, usize::from(*first), rows) else {
                    return Retargeted::Same(op);
                };
                self.counts.draws += 1;
                let moved = Arc::new(moved);
                let external = external.as_ref().and_then(|admission| {
                    let pass = admission.pass.lock().ok()?.with_draw(Arc::clone(&moved))?;
                    let mut adjusted = (**admission).clone();
                    adjusted.packet = Arc::clone(&moved);
                    adjusted.pass = Arc::new(Mutex::new(pass));
                    Some(Box::new(adjusted))
                });
                Retargeted::Moved(ReplayOp::Draw {
                    draw: moved,
                    textures: textures.clone(),
                    camera: CameraUse::View { view: *view, first: *first },
                    external,
                })
            }
            ReplayOp::Stage(stage) if at.slot.in_game_view() && !self.rows.is_empty() => {
                match self.stage(at, stage) {
                    Some(moved) => Retargeted::Moved(ReplayOp::Stage(moved)),
                    None => Retargeted::Same(op),
                }
            }
            _ => Retargeted::Same(op),
        }
    }

    fn stage(&mut self, at: SlotAt, stage: &SceneStage<frame_types::TargetRef>) -> Option<SceneStage<frame_types::TargetRef>> {
        let pass = stage.pass.lock().unwrap_or_else(|e| e.into_inner());
        let Some(draw) = pass.resolved_fallback() else {
            self.counts.held_stages += 1;
            return None;
        };
        let (view, first) = match classify(&draw, at, &self.views, &mut self.programs) {
            CameraUse::View { view, first } => (view, first),
            CameraUse::Screen => return None,
            CameraUse::Unknown => {
                self.counts.held_stages += 1;
                return None;
            }
        };
        // Its view's camera did not move.
        let rows = self.rows.get(&view)?;
        let moved: Option<Box<dyn ExternalPass>> = with_rows(&draw, usize::from(first), rows)
            .and_then(|moved| pass.with_draw(Arc::new(moved)));
        let Some(moved) = moved else {
            self.counts.held_stages += 1;
            return None;
        };
        self.counts.stages += 1;
        Some(SceneStage {
            surfaces: stage.surfaces.clone(),
            request: stage.request,
            plan: stage.plan.clone(),
            pass: Arc::new(Mutex::new(moved)),
        })
    }
}
