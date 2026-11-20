//! On a single-copy game frame with one physical scale, the owned graph
//! can draw into the same images that the scaled backend later resolves
//! for a readback, retargets on a setting change, and uses for the menu.

use d3d::threaded::owner::{Candidate, FrameOwner};
use d3d::{GfxBackend, ScaledBackend};
use std::sync::{Arc, OnceLock};

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

pub fn scaled_owner() -> FrameOwner<Scaled> {
    FrameOwner {
        select: Box::new(move |present| {
            let (scene, replay) = super::inputs(present)?;
            let checked = Arc::new(OnceLock::new());
            let prepared = Arc::clone(&checked);
            let to_check = Arc::clone(&replay);
            Some(Candidate {
                frame: Arc::clone(&replay),
                eligible: Box::new(move |backend: &Scaled, targets| {
                    backend
                        .scene_targets(&to_check, targets)
                        .is_some_and(|physical| prepared.set(physical).is_ok())
                }),
                draw: Box::new(move |backend, _| {
                    let targets = checked.get().ok_or("scaled scene has no checked targets")?;
                    let mode = backend.lighting();
                    let mut probe =
                        crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode);
                    let (report, outputs) = backend.with_scene_view(|view| {
                        native_render::frame_graph::draw_display_on(
                            view,
                            &scene,
                            &replay,
                            mode,
                            replay.flags,
                            replay.gamma.as_deref(),
                            probe.observer(),
                            targets,
                        )
                    })?;
                    let outputs = backend.scene_presented(&replay, outputs);
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
