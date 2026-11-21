//! Present a sealed game frame through the graph on its original
//! attachments. The D3D worker checks the retained command stream before
//! making any replacement. Other frames continue on that same device.

use std::sync::Arc;

use d3d::scene_sink::ReplayFrame;
use d3d::threaded::owner::{Candidate, FrameOwner, Submission};
use native_render::frame_graph;

use crate::frame_owner::{Service, pair_for_present};

mod scaled;
pub use scaled::scaled_owner;

fn needs_reference(present: u64) -> bool {
    crate::frame_parity::wanted(present) || crate::frame_parity::armed()
}

/// Seal at the game-view Swap so both sets of commands are available to
/// the display worker. The ordinary frame-end hook finds a sealed builder.
pub fn record(service: &Service) -> impl FnMut(ReplayFrame) + Send + 'static + use<> {
    let mut collect = service.recorder();
    move |frame| {
        let game_view = frame.ops.iter().any(|(at, _)| at.slot.in_game_view());
        collect(frame);
        if game_view {
            render_bridge::scene_step::seal();
        }
    }
}

pub fn owner(lighting: Box<dyn Fn() -> d3d::Lighting + Send + Sync>) -> FrameOwner<d3d::GfxBackend> {
    let lighting: Arc<dyn Fn() -> d3d::Lighting + Send + Sync> = lighting.into();
    FrameOwner {
        select: Box::new(move |present| {
            let (scene, replay) = pair_for_present(present)?;
            if needs_reference(present) {
                return Some(Submission::Reference {
                    frame: replay,
                    freeze: frame_graph::freeze_display_inputs,
                });
            }
            let lighting = Arc::clone(&lighting);
            let to_check = Arc::clone(&replay);
            Some(Submission::Native(Candidate {
                frame: Arc::clone(&replay),
                before_original: Some(frame_graph::freeze_display_inputs),
                eligible: Box::new(move |gpu: &d3d::GfxBackend, targets| gpu.matches_scene_targets(&to_check, targets)),
                draw: Box::new(move |backend, targets| {
                    let mode = lighting();
                    let mut probe = crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode);
                    let (report, outputs) = frame_graph::draw_display_on(
                        backend, &scene, &replay, mode, replay.flags, replay.gamma.as_deref(), probe.observer(), targets,
                    )?;
                    probe.presented(&report);
                    Ok(outputs)
                }),
            }))
        }),
        declined: crate::render_probe::owner_declined,
        failed: crate::frame_cutover::failed,
        stopped: crate::render_probe::owner_stopped,
    }
}

fn failed(present: u64) {
    agent::render::private_graph::discard_display(present);
    crate::frame_owner::present_failed(present);
}
