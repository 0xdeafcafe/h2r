//! Fresh attachments for a private render-scale draw that visits both
//! frame-sized and guest-sized levels. Use the same grid selection,
//! mixed depth updates, copies and sampling as the single-copy display.

use std::collections::HashMap;

use frame_scene::{AuxTarget, TargetRef};
use gfx::{Clear, D3DFormat, surface::SurfaceId};

use super::{ScaledBackend, ScaledSurface};
use crate::backend::Backend;
use crate::gfx_backend::GfxBackend;
use crate::gfx_backend::guest_draw_pass::commands::{Commands, SceneDevice};
use crate::scene_sink::{ReplayFrame, ReplayOp, TargetSeed};

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;
type Surface = ScaledSurface<SurfaceId, SurfaceId>;

impl ReplayFrame {
    /// Only levels matching the main frame dimensions use display-scale pixels.
    pub fn has_mixed_target_sizes(&self) -> bool {
        self.colour.is_some_and(|colour| {
            self.depth
                .iter()
                .chain(&self.aux)
                .any(|target| (target.width, target.height) != (colour.width, colour.height))
        })
    }
}

/// The caller's render resources stay together while the temporary
/// scaled backend records into new attachments. No window present or
/// guest visibility query runs on this path.
pub struct MixedPrivatePass<'a> {
    source: &'a mut GfxBackend,
    backend: Scaled,
    commands: Commands<Scaled>,
    targets: HashMap<TargetRef, Surface>,
    count_samples: bool,
    gpu_aliases: bool,
    lighting: crate::Lighting,
}

impl<'a> MixedPrivatePass<'a> {
    pub fn new(
        source: &'a mut GfxBackend,
        frame: &ReplayFrame,
        scale: u32,
    ) -> Result<Self, String> {
        if !frame.refusals.is_empty() {
            return Err(format!(
                "scene cannot be replayed: {}",
                frame.refusals.join(", ")
            ));
        }
        if let Some(reason) = frame.private_target_refusal() {
            return Err(reason.into());
        }
        let colour = frame
            .colour
            .ok_or("missing frame colour for scaled capture")?;
        source.flush()?;
        let reference = GfxBackend::new(source.ctx.clone());
        let placeholder = GfxBackend::new(source.ctx.clone());
        let mut backend = Scaled::with_single_copy(reference, placeholder, scale, true)?;
        // Keep the configured shader provider, its renderer's shader IDs,
        // and its live upload cache on the exact same backend.
        std::mem::swap(&mut backend.view, source);
        backend.scene_active = true;
        backend.view.prune_scene_uploads();
        let count_samples = std::mem::replace(&mut backend.view.count_samples, false);
        let gpu_aliases = std::mem::replace(&mut backend.view.single_copy_aliases, true);
        let lighting = backend.view.scene_lighting();
        if lighting == crate::Lighting::E3 {
            backend.view.set_hard_shadows(false);
        }
        let mut pass = Self {
            source,
            backend,
            commands: Commands::default(),
            targets: HashMap::new(),
            count_samples,
            gpu_aliases,
            lighting,
        };
        // ScaledBackend learns the main frame size from its first colour.
        // Every subsequent attachment is allocated at its original logical size.
        pass.allocate(TargetRef::SceneColour, colour)?;
        if let Some(z) = frame.depth {
            pass.allocate(TargetRef::SceneDepth, z)?;
        }
        for (i, &desc) in frame.aux.iter().enumerate() {
            let id = u16::try_from(i).map_err(|_| "too many private scene targets")?;
            pass.allocate(TargetRef::Aux(id), desc)?;
        }
        for (key, seed) in &frame.initial {
            let surface = pass.surface(*key)?;
            match seed {
                TargetSeed::Colour(rgba) => pass.backend.clear(
                    surface,
                    None,
                    Clear {
                        color: Some(*rgba),
                        ..Clear::NONE
                    },
                )?,
                TargetSeed::Upload { desc, bytes } => {
                    pass.backend.write_scene_target(surface, desc, bytes, 1)?;
                }
            }
        }
        Ok(pass)
    }

    fn allocate(&mut self, key: TargetRef, desc: AuxTarget) -> Result<(), String> {
        let size = (desc.width, desc.height);
        let scaled = self.backend.frame.is_none() || self.backend.frame == Some(size);
        let factor = if scaled { self.backend.scale } else { 1 };
        let limit = self
            .backend
            .view
            .ctx
            .device
            .limits()
            .max_texture_dimension_2d
            / factor;
        if desc.width == 0 || desc.height == 0 || desc.width > limit || desc.height > limit {
            return Err(format!("invalid {key:?} size {size:?} at {factor}x"));
        }
        let format = D3DFormat::from_raw(desc.format)
            .ok_or_else(|| format!("unknown scene target format {}", desc.format))?;
        let surface = self
            .backend
            .create_surface(format, desc.width, desc.height, desc.depth)?;
        self.targets.insert(key, surface);
        if scaled && surface.view.is_none() {
            return Err(format!("cannot allocate the scaled {key:?} attachment"));
        }
        Ok(())
    }

    fn surface(&self, key: TargetRef) -> Result<Surface, String> {
        self.targets
            .get(&key)
            .copied()
            .ok_or_else(|| format!("missing scene target {key:?}"))
    }

    pub fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        self.commands
            .apply(&mut self.backend, &self.targets, false, 1, op)
    }

    pub fn read_target(&mut self, key: TargetRef) -> Result<([u32; 2], Vec<u8>), String> {
        let image = self.backend.image(self.surface(key)?);
        self.backend.view.read_scene_target(image)
    }
}

impl Drop for MixedPrivatePass<'_> {
    fn drop(&mut self) {
        // A refusal can leave a partially recorded pass. Retire its uses
        // before returning the borrowed GPU with its original query policy.
        let _ = self.backend.view.flush();
        for (_, surface) in self.targets.drain() {
            let _ = self.backend.destroy_surface(surface);
        }
        self.backend.view.count_samples = self.count_samples;
        self.backend.view.single_copy_aliases = self.gpu_aliases;
        if self.lighting == crate::Lighting::E3 {
            self.backend.view.set_hard_shadows(true);
        }
        std::mem::swap(&mut self.backend.view, self.source);
    }
}
