//! Select the attachment actually read at the start of this frame. The
//! source may be the imported guest allocation, a scaled image, or a
//! legacy E3 shadow with a grid the private baseline cannot reproduce.

use frame_scene::TargetRef;
use gfx::surface::SurfaceId;
use std::collections::HashMap;

use super::{Lighting, ScaledBackend, ScaledSurface};
use crate::gfx_backend::{FrozenTarget, GfxBackend};
use crate::scene_sink::ReplayFrame;

type Surface = ScaledSurface<SurfaceId, SurfaceId>;

impl ScaledBackend<GfxBackend, GfxBackend> {
    /// Original-path preflight on this worker. A successful copy belongs
    /// solely to private recording and leaves both stores unchanged.
    pub fn freeze_original_scene_inputs(
        &mut self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
        names: &[TargetRef],
    ) -> Result<HashMap<TargetRef, FrozenTarget>, String> {
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
