//! The host of `render_bridge::native_fog_quad`: the planar fog's screen
//! quad placed on the shared device where the guest's fan `0x00034420`
//! would draw, over the bound view colour and depth
//! (docs/graphics/retire-d3d-fog-quad.md).

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::flare_gpu::DepthTest;
use native_render::fog_quad::{FogQuadDraw, FogQuadGpu, FogQuadTarget};
use render_bridge::native_fog_quad::{self, Host};

use crate::native_lights_glue::DeviceMemory;

/// The quad's programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogQuadGpu)>> = Mutex::new(None);

struct FogQuadPass {
    draw: FogQuadDraw,
    /// The W clip a W-buffered draw issued now takes; the setup sets
    /// `ZEnable(2)`, so the fan is W-buffered whatever the device holds.
    test: Option<DepthTest>,
}

impl d3d::ExternalPass for FogQuadPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.test = Some(DepthTest { w_buffered: true, range: clip.range, clamp: clip.clamp });
    }

    // Its numbers are the call's and its depth test is frozen when issued,
    // so a scene replay can record it again on its attachments.
    fn replays_scene(&self) -> bool {
        self.test.is_some()
    }

    /// Drawn on every copy (the reference and the display).
    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let Some(test) = self.test else {
            native_fog_quad::unported("depth_clip");
        };
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FogQuadGpu::new()));
        }
        let Some((_, pass)) = gpu.as_mut() else { return 0 };
        let target = FogQuadTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            color: t.color,
            color_format: t.color_format,
            depth: t.depth,
            guest: t.guest_size,
            z_scale: t.z_scale,
        };
        match pass.record(target, t.encoder, &self.draw, test) {
            Ok(_) => 1,
            Err(why) => native_fog_quad::unported(why),
        }
    }
}

struct Draw<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> Host for Draw<B> {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let pass = FogQuadPass { draw: draw.clone(), test: None };
        matches!(device.external_pass_now(&mut DeviceMemory(mem), Box::new(pass)), Ok(true))
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_fog_quad::on(&s)) {
        return;
    }
    native_fog_quad::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
