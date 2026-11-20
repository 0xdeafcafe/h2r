//! Present a sealed game frame through the graph on its original 1x
//! attachments. The D3D worker checks the retained command stream before
//! making any replacement. Other frames continue on that same device.

use std::sync::Arc;

use d3d::scene_sink::ReplayFrame;
use d3d::threaded::owner::{Candidate, FrameOwner};
use native_render::frame_graph;

use crate::frame_owner::{Service, pair_for_present};

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
    let lighting = Arc::from(lighting) as Arc<dyn Fn() -> d3d::Lighting + Send + Sync>;
    FrameOwner {
        select: Box::new(move |present| {
            // Parity's reference image must come from the guest path.
            if crate::frame_parity::wanted(present) || crate::frame_parity::armed() {
                return None;
            }
            let (scene, replay) = pair_for_present(present)?;
            let lighting = Arc::clone(&lighting);
            Some(Candidate {
                frame: Arc::clone(&replay),
                draw: Box::new(move |backend, targets| {
                    let mode = lighting();
                    let mut probe = crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode);
                    let report = frame_graph::draw_display_on(
                        backend, &scene, &replay, mode, replay.flags, replay.gamma.as_deref(), probe.observer(), targets,
                    )?;
                    probe.presented(&report);
                    Ok(())
                }),
            })
        }),
        declined: crate::render_probe::owner_declined,
        stopped: crate::render_probe::owner_stopped,
    }
}
