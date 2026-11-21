//! Select the attachment actually read at the start of this frame. The
//! source may be the imported guest allocation, a scaled image, or a
//! legacy E3 shadow with a grid the private baseline cannot reproduce.

use frame_scene::TargetRef;
use gfx::surface::SurfaceId;
use std::collections::HashMap;

use super::{Lighting, ScaledBackend, ScaledSurface};
use crate::gfx_backend::guest_draw_pass::live::live_draws_match;
use crate::gfx_backend::{FrozenTarget, GfxBackend};
use crate::scene_sink::ReplayFrame;

type Surface = ScaledSurface<SurfaceId, SurfaceId>;

impl ScaledBackend<GfxBackend, GfxBackend> {
    /// Verify the projection that a draw using this retained proxy will
    /// sample. Replacing a scaled surface leaves the old view valid on the
    /// GPU, but it no longer follows writes to the replacement allocation.
    pub(super) fn scene_live_views_match(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
    ) -> bool {
        live_draws_match(frame, |source, colour, binding| {
            let (Some(&s), Some(&dest), Some(desc), Some(images)) = (
                targets.get(&source),
                targets.get(&colour),
                frame.desc(source),
                binding.images(),
            ) else {
                return false;
            };
            if self.scale == 1 {
                if images.reference.device != self.reference.ctx.device
                    || !self.reference.live_view_matches(
                        s.reference,
                        dest.reference,
                        desc,
                        1,
                        &images.reference.texture,
                    )
                {
                    return false;
                }
                if !self.awake() {
                    return true;
                }
                // A single native draw at 1x cannot reproduce diverging
                // reference/display views left by legacy shadow rendering.
                let Some(img) = images.display.as_ref() else {
                    return false;
                };
                let Some(view) = self.view_surface(&s).or(s.guest) else {
                    return false;
                };
                let Some(dest_view) = self.view_surface(&dest).or(dest.guest) else {
                    return false;
                };
                return images.same_copies()
                    && img.device == self.view.ctx.device
                    && self
                        .view
                        .live_view_matches(view, dest_view, desc, 1, &img.texture);
            }
            if !self.single_copy {
                return false;
            }
            let Some(img) = images.display.as_ref() else {
                return false;
            };
            let Some(id) = self.view_surface(&s).or(s.guest) else {
                return false;
            };
            let Some(dest_image) = self.view_surface(&dest).or(dest.guest) else {
                return false;
            };
            let Some(guest_dest) = dest.guest else {
                return false;
            };
            let Some(main) = frame.colour else {
                return false;
            };
            let expected_grid = if (desc.width, desc.height) == (main.width, main.height) {
                self.scale
            } else {
                1
            };
            self.image(s) == id
                && img.device == self.view.ctx.device
                && self
                    .view
                    .live_view_matches(id, dest_image, desc, expected_grid, &img.texture)
                && (dest_image == guest_dest
                    || self.view.live_view_matches(
                        id,
                        guest_dest,
                        desc,
                        expected_grid,
                        &img.texture,
                    ))
        })
    }

    /// Original-path preflight on this worker. A successful copy belongs
    /// solely to private recording and leaves both stores unchanged.
    pub fn freeze_original_scene_inputs(
        &mut self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
        names: &[TargetRef],
    ) -> Result<HashMap<TargetRef, FrozenTarget>, String> {
        if !self.scene_live_views_match(frame, targets) {
            return Err("earlier live sample differs from the current scaled attachment".into());
        }
        frame.validate_live_bindings();
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        if self.single() {
            // A previous reference operation can have written an imported
            // guest image. Order it before capturing through the view.
            self.reference.flush()?;
            return self.freeze_view_inputs(frame, targets, names);
        }
        if self.awake()
            && (self.scale != 1
                || self.lighting != Lighting::E3
                || !names.iter().all(|name| {
                    targets
                        .get(name)
                        .is_some_and(|s| self.scene_coherent.contains(&s.reference))
                }))
        {
            return Err(
                "incoming attachment has separate legacy reference and display histories".into(),
            );
        }
        let reference = targets
            .iter()
            .map(|(&name, s)| (name, s.reference))
            .collect();
        self.reference.freeze_frame_inputs(frame, &reference, names)
    }

    pub(super) fn freeze_view_inputs(
        &mut self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
        names: &[TargetRef],
    ) -> Result<HashMap<TargetRef, FrozenTarget>, String> {
        if !self.scene_live_views_match(frame, targets) {
            return Err("earlier live sample cannot bind the scaled scene source".into());
        }
        frame.validate_live_bindings();
        let physical = names
            .iter()
            .map(|&name| {
                let s = *targets
                    .get(&name)
                    .ok_or_else(|| format!("missing incoming scaled attachment {name:?}"))?;
                let desc = frame
                    .desc(name)
                    .ok_or_else(|| format!("unknown incoming scaled attachment {name:?}"))?;
                let id = self.image(s);
                // Private allocation scales frame-sized levels and preserves
                // each smaller auxiliary's raster grid. A legacy E3 input can
                // have an additional shadow scale. It cannot be copied into
                // a different raster without changing the input values.
                let main = frame.colour.ok_or("missing scene colour descriptor")?;
                let expected = if (main.width, main.height) == (desc.width, desc.height) {
                    self.scale
                } else {
                    1
                };
                if !self.view.scene_target_matches(id, desc, expected) {
                    return Err(format!(
                        "incoming scaled attachment {name:?} differs from its private grid"
                    ));
                }
                Ok((name, id))
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.view.freeze_scene_targets(&physical)
    }
}
