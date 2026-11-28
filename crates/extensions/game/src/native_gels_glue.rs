//! Target 13's guest producer site. The light draws its shader gel from
//! the scene (`crate::native_gels_scene`, docs/graphics/native-gels.md,
//! "The gel from the scene"), so the guest's gel quad has no drawer: the
//! quad site panics by name. [`Outputs`] is what the light graph reads a
//! produced gel from; nothing produces one into it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use native_render::gels::Key;
use render_bridge::native_gels::{Call, Host};

/// Level zero of target 13 as a producer left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub key: Key,
    pub guest_size: (u32, u32),
}

impl Binding {
    /// Generated target 13 is linear: its shader coordinates are guest
    /// texels, even when the sampled target is larger.
    pub fn tex_scale(self) -> [f32; 4] {
        [1.0 / self.guest_size.0 as f32, 1.0 / self.guest_size.1 as f32, 1.0, 1.0]
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

    /// The gel produced after `before` for `definition`'s `shader` into
    /// `surface`.
    pub fn binding_after(&self, before: Checkpoint, definition: u32, shader: u32, surface: u32) -> Option<Binding> {
        self.produced_after(before, definition, shader).filter(|binding| binding.key.surface == surface)
    }

    pub(crate) fn produced_after(&self, before: Checkpoint, definition: u32, shader: u32) -> Option<Binding> {
        let binding = (*self.binding.lock().ok()?)?;
        let key = binding.key;
        (key.sequence > before.0 && key.definition == definition && key.shader == shader).then_some(binding)
    }
}

struct Glue {
    pending: Mutex<Option<Call>>,
}

impl Host for Glue {
    fn template(&self, _: &dyn GuestMemory, call: Option<Call>) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = call;
    }

    fn quad(&self, _: &mut dyn GuestMemory) {
        let call = *self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let shader = call.map_or(0, |c| c.shader);
        unimplemented!("gels: guest gel quad for shader {shader:#x}; the light draws its gel from the scene")
    }

    fn quad_end(&self, _: &mut dyn GuestMemory) {}
}

/// Installed with the world's hosts; the light graph keeps the returned
/// [`Outputs`].
pub fn install() -> Arc<Outputs> {
    render_bridge::native_gels::set_host(Arc::new(Glue { pending: Mutex::new(None) }));
    Arc::new(Outputs::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_light_without_new_production_cannot_take_the_previous_lights_gel() {
        let outputs = Outputs::default();
        let before_first = outputs.checkpoint();
        let key = Key { sequence: 1, definition: 2, shader: 3, surface: 4 };
        outputs.sequence.store(1, Ordering::Release);
        *outputs.binding.lock().unwrap() = Some(Binding { key, guest_size: (32, 16) });
        let binding = outputs.binding_after(before_first, 2, 3, 4).unwrap();
        assert_eq!(binding.tex_scale(), [1.0 / 32.0, 1.0 / 16.0, 1.0, 1.0]);
        let before_second = outputs.checkpoint();
        assert!(outputs.binding_after(before_second, 2, 3, 4).is_none());
        assert!(outputs.binding_after(before_first, 2, 3, 5).is_none());
    }
}
