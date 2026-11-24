//! On a single-copy game frame with one physical scale, the owned graph
//! can draw into the same images that the scaled backend later resolves
//! for a readback, retargets on a setting change, and uses for the menu.
//! With a display-rate loop the view also draws the display frames.

use d3d::threaded::owner::{Candidate, FrameOwner, Submission};
use d3d::{GfxBackend, ScaledBackend};
use native_render::frame_graph::display_rate::Display;
use std::sync::Arc;
use super::DisplayDevice;

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

pub(crate) fn scaled_owner(device: DisplayDevice, display: Option<Display>) -> FrameOwner<Scaled> {
    FrameOwner {
        events: Arc::new(device.clone()),
        select: Box::new(move |present, generation| {
            let (scene, replay, reference) = device.pair(generation, present)?;
            if reference {
                return Some(Submission::Reference {
                    frame: replay,
                    freeze: native_render::frame_graph::freeze_scaled_inputs,
                });
            }
            let device = device.clone();
            let display = display.clone();
            let to_check = Arc::clone(&replay);
            Some(Submission::Native(Candidate {
                frame: Arc::clone(&replay),
                before_original: Some(native_render::frame_graph::freeze_scaled_inputs),
                eligible: Box::new(move |backend: &Scaled, targets| {
                    backend.can_draw_scene(&to_check, targets)
                }),
                draw: Box::new(move |backend, targets| {
                    let mode = backend.lighting();
                    let mut probe =
                        device.current(generation, || crate::render_probe::OwnedFrame::begin(&scene, &replay, present, mode));
                    let observer = probe.as_mut().and_then(|p| p.observer());
                    let (report, outputs) = match &display {
                        Some(display) => native_render::frame_graph::draw_scaled_display_at_rate(
                            backend, display, &scene, &replay, observer, targets,
                        )?,
                        None => native_render::frame_graph::draw_scaled_display_on(
                            backend, &scene, &replay, observer, targets,
                        )?,
                    };
                    if let Some(probe) = probe { probe.presented(&report); }
                    Ok(outputs)
                }),
            }))
        }),
    }
}
