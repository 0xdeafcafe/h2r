//! Present a sealed game frame through the graph on its original
//! attachments. The D3D worker checks the retained command stream before
//! making any replacement. Other frames continue on that same device.
//! With a display-rate loop the owner's present also decides whether the
//! window shows the scene or a display frame toward it
//! (docs/graphics/framescene.md, section 7).

use std::sync::Arc;

use d3d::scene_sink::ReplayFrame;
use d3d::threaded::owner::{Candidate, FrameOwner, Submission};
use native_render::frame_graph;
use native_render::frame_graph::display_rate::Display;

use crate::frame_owner::{DisplayDevice, Service};

mod scaled;
pub(crate) use scaled::scaled_owner;

/// Seal at the game-view Swap so both sets of commands are available to
/// the display worker. The ordinary frame-end hook finds a sealed builder.
pub fn record(service: &Service) -> impl FnMut(ReplayFrame) + Send + 'static + use<> {
    service.scene_recorder(true)
}

pub(crate) fn owner(
    device: DisplayDevice,
    lighting: Arc<dyn Fn() -> d3d::Lighting + Send + Sync>,
    display: Option<Display>,
) -> FrameOwner<d3d::GfxBackend> {
    FrameOwner {
        events: Arc::new(device.clone()),
        select: Box::new(move |present, generation| {
            let (scene, replay, reference) = device.pair(generation, present)?;
            if reference {
                return Some(Submission::Reference {
                    frame: replay,
                    freeze: frame_graph::freeze_display_inputs,
                });
            }
            let lighting = Arc::clone(&lighting);
            let device = device.clone();
            let display = display.clone();
            let to_check = Arc::clone(&replay);
            Some(Submission::Native(Candidate {
                frame: Arc::clone(&replay),
                before_original: Some(frame_graph::freeze_display_inputs),
                eligible: Box::new(move |gpu: &d3d::GfxBackend, targets| gpu.matches_scene_targets(&to_check, targets)),
                draw: Box::new(move |backend, targets| {
                    let mode = lighting();
                    let mut probe = device.current(generation, || crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode));
                    let observer = probe.as_mut().and_then(|p| p.observer());
                    let (report, outputs) = match &display {
                        Some(display) => frame_graph::draw_display_at_rate(
                            backend, display, &scene, &replay, mode, observer, targets,
                        )?,
                        None => frame_graph::draw_display_on(
                            backend, &scene, &replay, mode, replay.flags, replay.gamma.as_deref(), observer, targets,
                        )?,
                    };
                    if let Some(probe) = probe { probe.presented(&report); }
                    Ok(outputs)
                }),
            }))
        }),
    }
}
