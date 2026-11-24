//! A light's external recordings, reserved together at one drained queue boundary.

use super::*;
use crate::{ExternalPass, ExternalPlan, ExternalRecording, GuestError, SceneStage};

/// Guest surface headers and viewport of a graph node. A colour-only pass
/// uses the backend's scratch depth. These addresses do not rebind the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalTargets {
    pub color: u32,
    pub depth: u32,
    pub rect: (u32, u32, u32, u32),
}

/// One ordered GPU operation in a light.
pub struct ExternalStep {
    pub targets: ExternalTargets,
    pub plan: ExternalPlan,
    pub pass: Box<dyn ExternalPass>,
}

struct Recorded<S> {
    stage: SceneStage<S>,
    colors: Vec<(u32, S)>,
    depth: u32,
    copies: Vec<ExternalRecording>,
}

/// Nothing is issued until the device and guest state can be committed.
/// Dropping the reservation drops every recording, including earlier layers.
#[must_use]
pub struct PreparedSequence<'a, B: Backend> {
    device: &'a mut Device<B>,
    steps: Vec<Recorded<B::Surface>>,
}

impl<B: Backend> Device<B> {
    pub fn external_targets(&self) -> ExternalTargets {
        let v = self.viewport;
        ExternalTargets {
            color: self.render_target,
            depth: self.depth_stencil,
            rect: (v.x, v.y, v.width, v.height),
        }
    }

    /// At a previously drained boundary, record every copy of each step in
    /// order. A callback, missing attachment, incomplete recording or replay
    /// backend that needs its guest packet leaves the whole chain to its caller.
    pub fn prepare_external_sequence(
        &mut self,
        mem: &mut dyn GuestMemory,
        steps: Vec<ExternalStep>,
    ) -> Result<Option<PreparedSequence<'_, B>>, D3dError> {
        if !self.created
            || !self.backend.can_commit_carried()
            || self.fault.is_some()
            || self.active_callback.is_some()
            || self.undelivered.is_some()
            || self.swallowing.is_some()
            || !self.pending_external.is_empty()
            || self.consumed != mem.read_u32(self.cfg.layout.device + field::PUT)?
        {
            return Ok(None);
        }
        let mut recorded = Vec::with_capacity(steps.len());
        for ExternalStep {
            targets,
            plan,
            mut pass,
        } in steps
        {
            if !self.backend.accepts_scene_pass(&*pass) {
                return Ok(None);
            }
            let origin = ExternalOrigin::new(&*pass, "carried sequence", Some(plan.stage));
            let binding = match self.bind_external_at(mem, targets, &plan, &mut *pass, None) {
                Ok(binding) => binding,
                Err(why) => {
                    self.refuse_external(origin, why, None);
                    return Ok(None);
                }
            };
            let stage = SceneStage {
                surfaces: binding.surfaces,
                request: binding.request,
                plan,
                pass: Arc::new(std::sync::Mutex::new(pass)),
            };
            let copies = match self.backend.prepare_carried_stage(&stage) {
                Ok(Some(copies)) if complete_recordings(&stage.plan, &copies) => copies,
                result => {
                    let detail = result.as_ref().err().map(String::as_str);
                    self.refuse_external(origin, ExternalRefusal::Backend, detail);
                    return Ok(None);
                }
            };
            recorded.push(Recorded {
                stage,
                colors: binding.colors,
                depth: if binding.depth_write {
                    targets.depth
                } else {
                    0
                },
                copies,
            });
        }
        for stage in &recorded {
            for &(_, color) in &stage.colors {
                self.materialize_display_copy(color)?;
            }
        }
        Ok(Some(PreparedSequence {
            device: self,
            steps: recorded,
        }))
    }
}

/// Planned guest writes in program order. The fragments read their own
/// writes (including their inline push words). Each patch's original is
/// its value *after* the preceding patches, so reversing a partial commit
/// also undoes overlapping stores.
struct Patch {
    addr: u32,
    before: Vec<u8>,
    after: Vec<u8>,
}

struct LightWrites<'a> {
    memory: &'a mut dyn GuestMemory,
    patches: Vec<Patch>,
}

impl GuestMemory for LightWrites<'_> {
    fn read(&self, address: u32, out: &mut [u8]) -> Result<(), GuestError> {
        self.memory.read(address, out)?;
        let base = u64::from(address);
        let end = base + out.len() as u64;
        for patch in &self.patches {
            let patch_base = u64::from(patch.addr);
            let first = base.max(patch_base);
            let last = end.min(patch_base + patch.after.len() as u64);
            if first < last {
                out[(first - base) as usize..(last - base) as usize].copy_from_slice(
                    &patch.after[(first - patch_base) as usize..(last - patch_base) as usize],
                );
            }
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, after: &[u8]) -> Result<(), GuestError> {
        let before = self.read_vec(addr, after.len())?;
        if before != after {
            self.patches.push(Patch {
                addr,
                before,
                after: after.to_vec(),
            });
        }
        Ok(())
    }
}

impl LightWrites<'_> {
    fn commit(self) -> Result<bool, D3dError> {
        for (i, patch) in self.patches.iter().enumerate() {
            if !self.memory.try_write(patch.addr, &patch.after) {
                for previous in self.patches[..i].iter().rev() {
                    self.memory.write(previous.addr, &previous.before)?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl<B: Backend> PreparedSequence<'_, B> {
    /// Apply a view light's D3D calls on the same bound viewport: the
    /// layer target setter and its mode-0 state, stencil and scissors off,
    /// depth-clip restore and final target setter. The upload at 0x3f7330
    /// writes the device register file only. Stage handlers still own any
    /// light whose shader, target, viewport or cache needs another change.
    pub fn commit_light<'w>(
        self,
        mem: &mut dyn GuestMemory,
        clip: [u32; 2],
        frame: &[u32; 36],
        shadow: Option<&[u32; 36]>,
        view_layers: u8,
        clip_header: Option<u32>,
        guest_writes: impl IntoIterator<Item = (u32, &'w [u8])>,
    ) -> Result<bool, D3dError> {
        let d = self.device;
        let dev = d.cfg.layout.device;
        let put = mem.read_u32(dev + field::PUT)?;
        // Each viewport fragment writes at most 13 words. A ring wrap or
        // MakeRequestedSpace has effects outside the setter transaction.
        let room = (u32::from(view_layers) + 2) * 13 * 4;
        let threshold = mem.read_u32(dev + field::THRESHOLD)?;
        if !d.backend.can_commit_carried()
            || view_layers > 2
            || put != d.consumed
            || put.checked_add(room).is_none_or(|end| end > threshold)
            || mem.read_u32(d.cfg.layout.p_device)? != dev
            || mem.read_words(dev + field::VIEWPORT, 6)?.as_slice() != &d.bound_target()[2..]
        {
            return Ok(false);
        }
        let clip_slot = clip_header
            .and_then(|h| (h >> 18 == 1).then_some(h & 0x3ffff))
            .and_then(|method| d.mirror_slot(method));
        if view_layers != 0 && clip_slot.is_none() {
            return Ok(false);
        }

        let mut writes = LightWrites {
            memory: mem,
            patches: Vec::new(),
        };
        // These setters change only the viewport, scissors, pushed render
        // states and the viewport/clip methods. Restrict the push buffer
        // to those bits of work so all host state can also be unwound.
        let before = (
            d.hw,
            d.consumed,
            d.viewport,
            d.scissor_exclusive,
            d.scissors.clone(),
            d.pushed.clone(),
            d.constants.clone(),
            d.constants_generation,
            d.stats,
        );
        let result = (|| -> Result<bool, D3dError> {
            let viewport = dev + field::VIEWPORT;
            for _ in 0..view_layers {
                d.set_viewport(&mut writes, viewport)?;
                // 0x000156f3: the stipple Simple setter precedes SampleAlpha.
                d.set_render_state_simple(&mut writes, clip_header.unwrap(), 0)?;
                d.set_render_state(&mut writes, 158, 0)?;
                d.set_render_state(&mut writes, slot::STENCILENABLE, 0)?;
            }
            if !d.light_passes_tail(&mut writes, clip)? {
                return Ok(false);
            }
            d.set_viewport(&mut writes, viewport)?;
            for (addr, bytes) in guest_writes {
                writes.write(addr, bytes)?;
            }
            writes.commit()
        })();
        if !matches!(result, Ok(true)) {
            (
                d.hw,
                d.consumed,
                d.viewport,
                d.scissor_exclusive,
                d.scissors,
                d.pushed,
                d.constants,
                d.constants_generation,
                d.stats,
            ) = before;
            d.fault = None;
            return result;
        }
        d.constants[31 * 4..40 * 4].copy_from_slice(frame);
        if let Some(projection) = shadow {
            d.constants[40 * 4..49 * 4].copy_from_slice(projection);
        }
        d.constants_generation += 1;
        for step in self.steps {
            d.backend.commit_carried_stage(step.stage, step.copies);
            for (header, surface) in step.colors {
                d.present_cameras.color_written(surface);
                d.mark_color_written(header);
            }
            if step.depth != 0 {
                d.mark_depth_header_written(step.depth);
            }
            d.stats.external_passes += 1;
        }
        Ok(true)
    }
}
