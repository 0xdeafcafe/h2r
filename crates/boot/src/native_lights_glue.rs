//! The native lights' host (docs/graphics/native-renderer.md, "Stage 4:
//! lights and shadows", slice 3), started: the device side of a skipped
//! light, with nothing drawn yet. Inert unless `impl.render.native_lights`
//! is on (the hooks are not placed otherwise).
//!
//! - [`Host::tail`]: `0x00031170`'s tail on the device, in render order,
//!   through `d3d::Device::light_passes_tail` over the hook's guest memory
//!   (lock order as the D3D entries': the guest's memory, which the hook
//!   holds, then the device).
//! - [`Host::target`]: the device's bound target
//!   (`d3d::Device::bound_target`), which the feature records before the
//!   view's lights and compares at each light.
//! - [`Host::accepts`] says no to every light until the native light
//!   passes exist (slice 3's drawing), so every light keeps its D3D passes
//!   and [`Host::open`] is never reached.

use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};
use feature::native_lights::Host;
use native_render::camera::RenderCamera;
use native_render::lights::{Light, Run};

/// The hook's guest memory as the device's.
pub struct DeviceMemory<'a>(pub &'a mut dyn GuestMemory);

fn guest_error(f: MemFault) -> d3d::GuestError {
    d3d::GuestError {
        addr: f.addr,
        len: f.len,
    }
}

impl d3d::GuestMemory for DeviceMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(guest_error)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        self.0.write(addr, data).map_err(guest_error)
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, _light: &Light) -> bool {
        false
    }

    fn open(&self, _camera: &RenderCamera, _run: Arc<Run>) -> bool {
        false
    }

    fn tail(&self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.light_passes_tail(&mut DeviceMemory(mem), clip).unwrap_or(false)
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

/// Installs the host on the device (`d3d_glue`, with the world's).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn the_device_sees_the_hooks_memory_and_its_faults() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut dm = DeviceMemory(&mut m);
        d3d::GuestMemory::write_u32(&mut dm, 0x1010, 0xdead_beef).unwrap();
        assert_eq!(d3d::GuestMemory::read_u32(&dm, 0x1010).unwrap(), 0xdead_beef);
        let e = d3d::GuestMemory::read_u32(&dm, 0x10fe).unwrap_err();
        assert_eq!((e.addr, e.len), (0x10fe, 4));
        assert!(d3d::GuestMemory::write(&mut dm, 0x0ffc, &[0; 4]).is_err());
        assert_eq!(m.u32(0x1010).unwrap(), 0xdead_beef);
    }
}
