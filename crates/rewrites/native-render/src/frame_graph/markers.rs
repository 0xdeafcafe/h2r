//! The marker skin in the frame graph (docs/game/gametype-markers.md, "The
//! game"). The game publishes a [`Batch`] of the frame's markers
//! ([`crate::markers::publish`]); each game view of the frame gets one
//! stage after its last stage, which is after the transparents and the
//! post passes and before the overlay (the HUD). The stage draws the batch
//! on the scene's colour and depth with the view's view-projection,
//! against the W-buffer ([`WDepth`], nearer smaller, so no reversed
//! depth), inside the view's viewport.

use std::sync::{Arc, Mutex};

use frame_scene::D24_SCALE;
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalEncoded};

use super::{Cameras, guest_rect, last_stages, nothing, texel_viewport};
use crate::markers::{self, Batch, Pass, Targets, WDepth};
use crate::program::ViewInputs;
use crate::stage::{FrameOp, Stage, StageWork};

/// How far towards the eye a marker meets the scene's depth, in view
/// depth (world units): the scene's depth differs from the marker's by
/// rounding.
pub const RECEIVER_DEPTH_BIAS: f32 = 0.005;

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

/// The marker stage of each game view, by the index of the op it follows,
/// from the batch the game last published. Empty when there is none.
pub(crate) fn plan(ops: &[FrameOp], cameras: &Cameras) -> Vec<(usize, SlotAt, Box<dyn Stage>)> {
    let Some(batch) = markers::published() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, view) in last_stages(ops, Slot::in_game_view) {
        let Some(camera) = cameras.get(view) else { continue };
        let Some(rect) = guest_rect(&camera) else { continue };
        let stage = MarkerStage {
            batch: Arc::clone(&batch),
            matrix: camera.view_projection(),
            depth: WDepth { far: camera.far, scale: D24_SCALE, bias: RECEIVER_DEPTH_BIAS },
            rect,
        };
        out.push((i, SlotAt { slot: Slot::AfterPost, view }, Box::new(stage) as Box<dyn Stage>));
    }
    out
}

/// One view's markers.
struct MarkerStage {
    batch: Arc<Batch>,
    matrix: [[f32; 4]; 4],
    depth: WDepth,
    rect: (u32, u32, u32, u32),
}

impl Stage for MarkerStage {
    fn name(&self) -> &'static str {
        "markers"
    }

    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        StageWork::Encoded(self.encode_on(t).unwrap_or_else(|_| nothing()))
    }
}

impl MarkerStage {
    fn encode_on(&self, target: &ExternalAttachments) -> Result<ExternalEncoded, &'static str> {
        let viewport = texel_viewport(self.rect, target.size);
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
