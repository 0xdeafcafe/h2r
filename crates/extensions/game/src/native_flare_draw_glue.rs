//! The host of `render_bridge::native_flare_draw`: each glow's passes
//! placed on the shared device where the guest's glow `0x00046ea0` would
//! draw, over the bound view colour (docs/graphics/d3d-visibility.md,
//! "The glow in Rust").

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::flare_draw_gpu::{FlareDrawGpu, MASK_DEPTH};
use native_render::flare_gpu::{self, DepthTest};
use render_bridge::native_flare_draw::{self, GlowDraw, Host};
use render_bridge::native_flare_query::Quad;

use crate::native_lights_glue::DeviceMemory;

/// The glow's pipelines on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FlareDrawGpu)>> = Mutex::new(None);

struct GlowPass {
    draw: GlowDraw,
    test: DepthTest,
    /// Counted once, however many target copies record.
    dropped: bool,
}

impl d3d::ExternalPass for GlowPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    /// The W clip the masked sample's fan takes.
    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.test = DepthTest { w_buffered: clip.w_buffered.unwrap_or(true), range: clip.range, clamp: clip.clamp };
    }

    /// Drawn on every copy (the reference and the display).
    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FlareDrawGpu::new(t.device)));
        }
        let Some((_, pass)) = gpu.as_mut() else { return };
        let mask = Quad { x0: 0, y0: 0, x1: 1, y1: 1, z: MASK_DEPTH, w: MASK_DEPTH, area: 1 };
        // A fan the W clip culls passes nowhere.
        let reference = flare_gpu::reference(&mask, self.test, t.z_scale).unwrap_or(2.0);
        let recorded = pass.record(t.device, t.encoder, t.color, t.depth, t.guest_size, reference, &self.draw);
        if let (Err(why), false) = (recorded, self.dropped) {
            self.dropped = true;
            native_flare_draw::dropped(why);
        }
    }
}

struct Draw<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> Host for Draw<B> {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: GlowDraw) -> bool {
        let pass = GlowPass { draw, test: DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: true }, dropped: false };
        let Ok(mut device) = self.device.lock() else { return false };
        matches!(device.external_pass_now(&mut DeviceMemory(mem), Box::new(pass)), Ok(true))
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_flare_draw::on(&s)) {
        return;
    }
    native_flare_draw::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
