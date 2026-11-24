//! Target-13 producer host. Scoped draw admission replaces exact gel
//! packets after complete two-copy recording. Lights consume the real
//! target-13 surface through Outputs::binding_after.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use engine::memory::GuestMemory;
use render_bridge::native_gels::{Call, Host};
use native_render::camera::GuestRead;
use native_render::gels::{self, Key, Target};

pub use crate::native_gels_replace::Binding;
use crate::native_gels_replace::Replacements;
use crate::native_lights_glue::DeviceMemory;

static REFUSALS: crate::native_refusals::Counts = crate::native_refusals::Counts::new();

pub(crate) fn report_refusals() {
    REFUSALS.report("native-gels");
}

fn refuse(reason: &'static str) {
    REFUSALS.count("production", reason);
}

struct Ro<'a>(&'a dyn GuestMemory);
impl GuestRead for Ro<'_> {
    fn read_u32(&self, address: u32) -> Option<u32> {
        self.0.u32(address).ok()
    }
}

/// Pass this owner to the light graph when installing the host. Take a
/// checkpoint at the light's entry, then ask for the binding produced
/// after it. A light with no new production gets None.
#[derive(Default)]
pub struct Outputs {
    binding: Mutex<Option<Binding>>,
    sequence: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
pub struct Checkpoint(u64);

impl Outputs {
    /// Take at the light's entry, before its constants run. A whole-light
    /// skip then cannot accidentally consume the previous light's gel.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.sequence.load(Ordering::Acquire))
    }

    /// Add the returned slot to the consuming light's ExternalPlan. The
    /// handoff then supplies target 13 as native/D3D last wrote it.
    pub fn binding_after(
        &self,
        before: Checkpoint,
        definition: u32,
        shader: u32,
        surface: u32,
    ) -> Option<Binding> {
        let binding = (*self.binding.lock().ok()?)?;
        let key = binding.key;
        (key.sequence > before.0
            && key.definition == definition
            && key.shader == shader
            && key.surface == surface)
            .then_some(binding)
    }
}

#[derive(Default)]
struct Loaded {
    name: String,
    generation: u64,
}

#[derive(Clone)]
struct Pending {
    call: Call,
    name: String,
    sequence: u64,
}

struct Glue<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    replacements: Arc<Replacements>,
    admission_registered: Once,
    loaded: Arc<Mutex<Loaded>>,
    pending: Mutex<Option<Pending>>,
    outputs: Arc<Outputs>,
}

impl<B: d3d::Backend + Send + 'static> Glue<B> {
    fn generation(&self, name: &str) -> u64 {
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.name != name {
            loaded.name = name.into();
            loaded.generation = loaded.generation.wrapping_add(1);
        }
        loaded.generation
    }

    fn replace(
        &self,
        mem: &mut dyn GuestMemory,
        pending: &Pending,
        target: Target,
    ) -> Result<(), &'static str> {
        let mut device = self.device.lock().map_err(|_| "device lock")?;
        if !device
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| "gel D3D boundary failed")?
        {
            return Err("gel waits for a preceding guest callback");
        }
        let generation = self
            .loaded
            .lock()
            .map_err(|_| "gel map generation lock")?
            .generation;
        let view = mem.u32(0x004b_a050).map_err(|_| "gel view unavailable")?;
        // Only the enabled feature's replacement path reaches here. Keep
        // disabled runs out of the device's admission list.
        self.admission_registered.call_once(|| {
            device.add_draw_admission(self.replacements.clone());
        });
        let key = Key {
            sequence: pending.sequence,
            definition: pending.call.definition,
            shader: pending.call.shader,
            surface: target.surface,
        };
        self.replacements.arm(key, target, view, generation);
        *self
            .outputs
            .binding
            .lock()
            .map_err(|_| "gel binding lock")? = Some(Binding {
            key,
            guest_size: target.size,
        });
        Ok(())
    }
}

impl<B: d3d::Backend + Send + 'static> Host for Glue<B> {
    fn template(&self, mem: &dyn GuestMemory, call: Option<Call>) {
        self.replacements.cancel();
        self.outputs
            .binding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(call) = call else { return };
        let Ok(name) = crate::health::loaded_map_name(mem) else {
            return;
        };
        self.generation(&name);
        let sequence = self.outputs.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            call,
            name,
            sequence,
        });
    }

    fn quad(&self, mem: &mut dyn GuestMemory) {
        self.replacements.cancel();
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(pending) = pending else {
            return;
        };
        if mem.u32(gels::DEFINITION).ok() != Some(pending.call.definition)
            || mem.u32(pending.call.definition.wrapping_add(0xe0)).ok() != Some(pending.call.shader)
            || crate::health::loaded_map_name(mem).ok().as_ref() != Some(&pending.name)
        {
            refuse("gel invocation changed before its quad");
            return;
        }
        let Some(target) = Target::read(&Ro(mem)) else {
            refuse("target 13 descriptor is unavailable or invalid at the quad");
            return;
        };
        if let Err(e) = self.replace(mem, &pending, target) {
            refuse(e);
        }
    }

    fn quad_end(&self, mem: &mut dyn GuestMemory) {
        if !self.replacements.active() {
            return;
        }
        // The last inline mip draw can still be queued when the helper
        // returns. Drain it while this invocation owns the scope.
        let drained = self.device.lock().ok().and_then(|mut device| {
            device.begin_external_boundary(&mut DeviceMemory(mem)).ok()
        });
        self.replacements.cancel();
        if drained != Some(true) {
            refuse("gel quad exit could not drain its pending draw");
        }
    }
}

/// Installed with the world's hosts. The lights lane can retain the
/// returned Outputs and bind its target-13 slots at the post-constants seam.
pub fn install<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) -> Arc<Outputs> {
    let outputs = Arc::new(Outputs::default());
    let replacements = Arc::new(Replacements::default());
    render_bridge::native_gels::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        replacements,
        admission_registered: Once::new(),
        loaded: Arc::new(Mutex::new(Loaded::default())),
        pending: Mutex::new(None),
        outputs: Arc::clone(&outputs),
    }));
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_light_without_new_production_cannot_take_the_previous_lights_gel() {
        let outputs = Outputs::default();
        let before_first = outputs.checkpoint();
        let key = Key {
            sequence: 1,
            definition: 2,
            shader: 3,
            surface: 4,
        };
        outputs.sequence.store(1, Ordering::Release);
        *outputs.binding.lock().unwrap() = Some(Binding {
            key,
            guest_size: (32, 16),
        });
        let binding = outputs.binding_after(before_first, 2, 3, 4).unwrap();
        assert_eq!(binding.slot().header, 4);
        assert!(!binding.slot().written);
        assert_eq!(binding.tex_scale(), [1.0 / 32.0, 1.0 / 16.0, 1.0, 1.0]);
        let before_second = outputs.checkpoint();
        assert!(outputs.binding_after(before_second, 2, 3, 4).is_none());
        assert!(outputs.binding_after(before_first, 2, 3, 5).is_none());
    }
}
