//! On a single-copy game frame with one physical scale, the owned graph
//! can draw into the same images that the scaled backend later resolves
//! for a readback, retargets on a setting change, and uses for the menu.

use d3d::threaded::owner::{Candidate, FrameOwner};
use d3d::{GfxBackend, ScaledBackend};
use std::sync::Arc;

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

pub fn scaled_owner() -> FrameOwner<Scaled> {
    FrameOwner {
        select: Box::new(move |present| {
            let (scene, replay) = super::inputs(present)?;
            let to_check = Arc::clone(&replay);
            Some(Candidate {
                frame: Arc::clone(&replay),
                eligible: Box::new(move |backend: &Scaled, targets| {
                    backend.can_draw_scene(&to_check, targets)
                }),
                draw: Box::new(move |backend, targets| {
                    let mode = backend.lighting();
                    let mut probe =
                        crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode);
                    let (report, outputs) = native_render::frame_graph::draw_scaled_display_on(
                        backend,
                        &scene,
                        &replay,
                        probe.observer(),
                        targets,
                    )?;
                    probe.presented(&report);
                    Ok(outputs)
                }),
            })
        }),
        declined: crate::render_probe::owner_declined,
        failed: super::failed,
        stopped: crate::render_probe::owner_stopped,
    }
}
