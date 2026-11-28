//! The host of `render_bridge::native_fog_quad`: the planar fog's screen
//! quad as a stage where the guest's fan `0x00034420` would draw, over the
//! bound view colour and depth, and the other setup's
//! texture-shader fog (view modes 1, 4 and 5) over the same targets with
//! the rasterizer's ramp bitmap (docs/graphics/retire-d3d-fog-quad.md).

use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use native_render::flare_gpu::DepthTest;
use native_render::fog_quad::{FogOtherDraw, FogOtherTarget, FogQuadDraw, FogQuadGpu, FogQuadTarget};
use render_bridge::native_fog_quad::{self, Host};

use crate::native_decals_glue::{DecalBitmap, decode_bitmap, texture_view};
use crate::native_targets_glue::{encoded, push};
use frame_types::Slot;
use native_render::program::ViewInputs;
use native_render::{Stage, StageWork};

/// The quad's programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogQuadGpu)>> = Mutex::new(None);

/// The W clip a W-buffered draw takes: the setup sets `ZEnable(2)`, so
/// the fans are W-buffered whatever was bound.
fn w_test(view: &ViewInputs) -> DepthTest {
    match view.w_clip {
        Some((range, clamp)) => DepthTest { w_buffered: true, range, clamp },
        None => DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: false },
    }
}

/// The quad's programs for `device`, made again when the device changes.
fn programs(device: &wgpu::Device) -> std::sync::MutexGuard<'static, Option<(wgpu::Device, FogQuadGpu)>> {
    let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if gpu.as_ref().is_none_or(|(d, _)| d != device) {
        *gpu = Some((device.clone(), FogQuadGpu::new()));
    }
    gpu
}

struct FogQuadPass {
    draw: Arc<FogQuadDraw>,
}

impl Stage for FogQuadPass {
    fn name(&self) -> &'static str {
        "fog quad"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> StageWork {
        let (draw, test, guest, z_scale) = (Arc::clone(&self.draw), w_test(view), view.guest_size, view.z_scale);
        let (device, queue, uploads, color, color_format, depth) =
            (t.device.clone(), t.queue.clone(), t.uploads.clone(), t.color.clone(), t.color_format, t.depth.clone());
        encoded(move |encoder| {
            let mut gpu = programs(&device);
            let Some((_, pass)) = gpu.as_mut() else { return };
            let target = FogQuadTarget {
                device: &device,
                queue: &queue,
                uploads: &uploads,
                color: &color,
                color_format,
                depth: &depth,
                guest,
                z_scale,
            };
            if let Err(why) = pass.record(target, encoder, &draw, test) {
                native_fog_quad::unported(why);
            }
        })
    }
}

struct FogOtherPass {
    draw: Arc<FogOtherDraw>,
    /// Image 0 of the rasterizer's ramp bitmap, decoded when issued;
    /// `None` for the weather plate's block, which samples stage 0 alone.
    ramp: Option<Arc<DecalBitmap>>,
}

impl Stage for FogOtherPass {
    fn name(&self) -> &'static str {
        "fog other"
    }

    /// The strips test depth only when `[0x0048575d]` is 0
    /// (`FogOtherDraw::depth_test`).
    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> StageWork {
        let test = self.draw.depth_test.then(|| w_test(view));
        let ramp = self.ramp.as_ref().map(|b| texture_view(&t.device, &t.queue, b));
        let (draw, guest, z_scale) = (Arc::clone(&self.draw), view.guest_size, view.z_scale);
        let (device, queue, uploads, color, color_format, depth) =
            (t.device.clone(), t.queue.clone(), t.uploads.clone(), t.color.clone(), t.color_format, t.depth.clone());
        encoded(move |encoder| {
            let mut gpu = programs(&device);
            let Some((_, pass)) = gpu.as_mut() else { return };
            let target = FogOtherTarget {
                device: &device,
                queue: &queue,
                uploads: &uploads,
                color: &color,
                color_format,
                depth: &depth,
                guest,
                z_scale,
                ramp: ramp.as_ref(),
            };
            if let Err(why) = pass.record_other(target, encoder, &draw, test) {
                native_fog_quad::unported(why);
            }
        })
    }
}

struct Draw;

impl Host for Draw {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool {
        push(&*mem, Slot::AfterLights, FogQuadPass { draw: Arc::new(draw.clone()) });
        true
    }

    fn draw_other(&self, mem: &mut dyn GuestMemory, draw: &FogOtherDraw) -> bool {
        // The bridge resolves stage 2's header (`FogOtherDraw::ramp`), asking
        // the guest's texture cache when it streams; 0 for the plate.
        let ramp = (draw.ramp != 0)
            .then(|| decode_bitmap(&*mem, draw.ramp).unwrap_or_else(|why| native_fog_quad::unported(why)));
        push(&*mem, Slot::AfterLights, FogOtherPass { draw: Arc::new(draw.clone()), ramp });
        true
    }
}

pub fn install() {
    native_fog_quad::set_host(Arc::new(Draw));
}
