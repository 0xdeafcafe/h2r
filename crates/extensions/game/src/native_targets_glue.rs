//! The host of `render_bridge::native_targets`: the scene's target select
//! and clear made on the shared device without their guest D3D entries
//! (docs/graphics/framescene.md, "Slice 6").

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use render_bridge::native_targets::{self, Bind, Host};

use crate::native_lights_glue::DeviceMemory;

struct Targets<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    logged: Mutex<bool>,
}

impl<B: d3d::Backend + Send + 'static> Targets<B> {
    fn refused(&self, what: &str, e: d3d::D3dError) -> bool {
        let mut logged = self.logged.lock().unwrap_or_else(|e| e.into_inner());
        if !std::mem::replace(&mut *logged, true) {
            eprintln!("native-targets: the device refused {what}: {e} (the guest body runs; logged once)");
        }
        false
    }
}

impl<B: d3d::Backend + Send + 'static> Host for Targets<B> {
    fn bind(&self, mem: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let mut m = DeviceMemory(mem);
        let made = match bind {
            Bind::Kept => return true,
            Bind::SameSize => device.set_render_target_same_size(&mut m, colour, depth),
            Bind::Full => device.set_render_target(&mut m, colour, depth),
        };
        drop(device);
        made.map_or_else(|e| self.refused("a bind", e), |()| true)
    }

    fn clear(&self, mem: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let made = device.clear(&mut DeviceMemory(mem), 0, 0, flags, colour, f32::from_bits(z), stencil);
        drop(device);
        made.map_or_else(|e| self.refused("a clear", e), |()| true)
    }

    fn bound(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_targets::on(&s)) {
        return;
    }
    native_targets::set_host(Arc::new(Targets { device: Arc::clone(device), logged: Mutex::new(false) }));
}
