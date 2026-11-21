//! Retain only the incoming pixels on which a private read depends.
//! The capture happens while the original attachment handles still
//! refer to the scene's inputs, before either recorder issues a write.

use std::collections::HashMap;

use d3d::gfx_backend::FrozenTarget;
use d3d::scaled::ScaledScenePass;
use gfx::surface::SurfaceId;

use super::*;

type Inputs = HashMap<TargetRef, FrozenTarget>;
type ScaledTargets = HashMap<TargetRef, ScaledSurface<SurfaceId, SurfaceId>>;

fn capture(
    frame: &ReplayFrame,
    mut copy: impl FnMut(&[TargetRef]) -> Result<Inputs, String>,
) -> Result<(), String> {
    if frame.target_seeds_sealed() {
        return frame.frozen_target_seeds().map(|_| ());
    }
    let final_read = (frame.ops.len(), frame.presented);
    let mut reads: Vec<_> = pass_rows(frame)
        .into_iter()
        .map(|row| (row.ops.end, row.colour))
        .collect();
    reads.push(final_read);
    let required = |reads: &[(usize, TargetRef)]| {
        frame
            .private_seed_targets(reads)
            .ok_or_else(|| "sealed frame has an unresolved private input".to_owned())
    };
    let result = (|| {
        let all = required(&reads);
        if let Ok(names) = &all {
            match copy(names) {
                Ok(images) => return Ok(images),
                Err(error) => {
                    // An optional pass snapshot may visit a resource that
                    // the final frame never uses. Keep the final capture
                    // available when its inputs can be represented.
                    let final_names = required(&[final_read])?;
                    if final_names == *names {
                        return Err(error);
                    }
                    return copy(&final_names);
                }
            }
        }
        copy(&required(&[final_read])?)
    })();
    frame.seal_target_seeds(result.map(|images| images.into_iter().collect()))?;
    frame.frozen_target_seeds().map(|_| ())
}

/// Called on the worker after the transaction's targets have been
/// matched, before the first write to an original 1x attachment.
pub fn freeze_display_inputs(
    gpu: &mut GfxBackend,
    frame: &ReplayFrame,
    targets: &HashMap<TargetRef, SurfaceId>,
) -> Result<(), String> {
    capture(frame, |names| {
        gpu.freeze_frame_inputs(frame, targets, names)
    })
}

/// Preserve the source corresponding to an original scaled presentation.
/// A distinct reference/view history can disqualify its private replay.
/// The worker still executes the complete original command stream.
pub fn freeze_scaled_inputs(
    backend: &mut ScaledBackend<GfxBackend, GfxBackend>,
    frame: &ReplayFrame,
    targets: &ScaledTargets,
) -> Result<(), String> {
    capture(frame, |names| {
        backend.freeze_original_scene_inputs(frame, targets, names)
    })
}

pub(super) fn freeze_native_scaled_inputs(
    pass: &mut ScaledScenePass<'_>,
    frame: &ReplayFrame,
) -> Result<(), String> {
    capture(frame, |names| pass.freeze_inputs(names))
}
