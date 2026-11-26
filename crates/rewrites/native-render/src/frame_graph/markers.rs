//! The marker skin in the frame graph (docs/game/gametype-markers.md, "The
//! game"). The game publishes a [`Batch`] of the frame's markers
//! ([`crate::markers::publish`]); each game view of the frame gets one
//! stage after the last op of its own slots, which is after the
//! transparents and the post passes and before the overlay (the HUD). The
//! stage draws the batch on the scene's colour and depth with the view's
//! view-projection, against the guest's W-buffer ([`WDepth`], nearer
//! smaller, so no reversed depth), inside the view's viewport.
//!
//! A frame with no published batch, no usable camera for a view, or a depth
//! that is not a 24-bit W-buffer plans no stage and replays as before.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces,
    SceneStage,
};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{D24_SCALE, FrameScene, TargetRef, ViewKind};
use gfx::D3DFormat;
use gfx::external::ExternalEncoded;

use crate::frame_graph::RECEIVER_DEPTH_BIAS;
use crate::markers::{self, Batch, Pass, Targets, WDepth};

/// Each device's pass for its attachment formats, kept across frames.
static PASSES: Mutex<
    Vec<(
        wgpu::Device,
        wgpu::TextureFormat,
        wgpu::TextureFormat,
        Arc<Pass>,
    )>,
> = Mutex::new(Vec::new());

fn pass_for(
    device: &wgpu::Device,
    colour: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
) -> Arc<Pass> {
    let mut all = PASSES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, _, _, pass)) = all
        .iter()
        .find(|(d, c, z, _)| d == device && *c == colour && *z == depth)
    {
        return Arc::clone(pass);
    }
    let made = Arc::new(Pass::new(device, colour, depth, false));
    all.push((device.clone(), colour, depth, Arc::clone(&made)));
    made
}

/// For each guest view with game-view ops, the index of its last one and
/// the view: where its marker stage goes, before the overlay's ops.
pub(super) fn last_ops(frame: &ReplayFrame) -> Vec<(usize, u8)> {
    let mut seen = HashSet::new();
    let mut out: Vec<(usize, u8)> = frame
        .ops
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, (at, op))| at.slot.in_game_view() && !op.is_marker() && seen.insert(at.view))
        .map(|(i, (at, _))| (i, at.view))
        .collect();
    out.reverse();
    out
}

/// The marker stage to apply after the op at each index of `frame`, from
/// the batch the game last published. Empty when there is none.
pub(crate) fn plan(scene: &FrameScene, frame: &ReplayFrame) -> HashMap<usize, ReplayOp> {
    let mut after = HashMap::new();
    let Some(batch) = markers::published() else {
        return after;
    };
    let (Some(colour), Some(depth)) = (frame.colour, frame.depth) else {
        return after;
    };
    if depth.format != D3DFormat::D24S8 as u32 && depth.format != D3DFormat::LinD24S8 as u32 {
        return after;
    }
    for (i, view) in last_ops(frame) {
        let Some(v) = scene
            .views
            .iter()
            .find(|v| v.key.kind == ViewKind::Game && v.key.view == view)
        else {
            continue;
        };
        let camera = &v.camera;
        if !camera.is_usable() {
            continue;
        }
        let [top, left, bottom, right] = camera.viewport.map(|x| u32::try_from(x).unwrap_or(0));
        let (Some(width), Some(height)) = (
            right.checked_sub(left).filter(|&w| w > 0),
            bottom.checked_sub(top).filter(|&h| h > 0),
        ) else {
            continue;
        };
        let stage = MarkerStage {
            batch: Arc::clone(&batch),
            matrix: camera.view_projection(),
            depth: WDepth {
                far: camera.far,
                scale: D24_SCALE,
                bias: RECEIVER_DEPTH_BIAS,
            },
        };
        after.insert(
            i,
            ReplayOp::Stage(SceneStage {
                surfaces: ExternalSurfaces {
                    color: TargetRef::SceneColour,
                    depth: TargetRef::SceneDepth,
                    sampled: vec![],
                    slots: vec![],
                    slot_sizes: vec![],
                },
                request: ExternalPassRequest {
                    rect: (left, top, width, height),
                    surface_size: (colour.width, colour.height),
                    display_projection_factor: 1.0,
                    z_scale: D24_SCALE,
                },
                plan: ExternalPlan {
                    stage: timeline::native::Stage::Effects,
                    view: u32::from(view),
                    ranges: 1,
                    slots: vec![],
                },
                pass: Arc::new(Mutex::new(Box::new(stage) as Box<dyn ExternalPass>)),
            }),
        );
    }
    after
}

/// One view's markers.
struct MarkerStage {
    batch: Arc<Batch>,
    matrix: [[f32; 4]; 4],
    depth: WDepth,
}

impl ExternalPass for MarkerStage {
    fn replays_scene(&self) -> bool {
        true
    }

    fn record_encoded(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        Some(self.encode_on(target, request))
    }
}

impl MarkerStage {
    fn encode_on(
        &self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        let (gx, gy, gw, gh) = request.rect;
        let (sw, sh) = request.surface_size;
        if sw == 0 || sh == 0 {
            return Err("marker stage has an empty surface");
        }
        let (kx, ky) = (
            target.size.0 as f32 / sw as f32,
            target.size.1 as f32 / sh as f32,
        );
        let viewport = [
            gx as f32 * kx,
            gy as f32 * ky,
            gw as f32 * kx,
            gh as f32 * ky,
        ];
        let pass = pass_for(&target.device, target.color_format, target.depth_format);
        let prepared = pass
            .prepare(&target.device, self.matrix, &self.batch, Some(self.depth))
            .ok_or("marker stage has no markers")?;
        markers::note_drawn();
        let (colour, depth) = (target.color.clone(), target.depth.clone());
        Ok(ExternalEncoded {
            ranges: 1,
            draws: prepared.draws(),
            passes: 1,
            commands: Box::new(move |encoder| {
                prepared.record(
                    encoder,
                    &Targets {
                        color: &colour,
                        depth: &depth,
                    },
                    Some(viewport),
                );
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{Slot, SlotAt};
    use gfx::Clear;

    fn op(slot: Slot, view: u8) -> (SlotAt, ReplayOp) {
        (
            SlotAt { slot, view },
            ReplayOp::Clear {
                colour: TargetRef::SceneColour,
                depth: TargetRef::None,
                clear: Clear::NONE,
            },
        )
    }

    #[test]
    fn a_views_stage_follows_its_last_game_view_op_and_comes_before_the_overlay() {
        let frame = ReplayFrame {
            ops: vec![
                op(Slot::Outside, 0),
                op(Slot::Opaque, 0),
                op(Slot::Transparents, 0),
                op(Slot::AfterPost, 0),
                op(Slot::Opaque, 1),
                op(Slot::AfterPost, 1),
                op(Slot::Overlay, 1),
                op(Slot::Outside, 0),
            ],
            ..Default::default()
        };
        assert_eq!(last_ops(&frame), [(3, 0), (5, 1)]);
        assert!(last_ops(&ReplayFrame::default()).is_empty());
    }
}
