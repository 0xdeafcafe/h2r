//! The host of `render_bridge::native_fog_quad`: the planar fog's screen
//! quad placed on the shared device where the guest's fan `0x00034420`
//! would draw, over the bound view colour and depth, and the other setup's
//! texture-shader fog (view modes 1, 4 and 5) over the same targets with
//! the rasterizer's ramp bitmap (docs/graphics/retire-d3d-fog-quad.md).

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::flare_gpu::DepthTest;
use native_render::fog_quad::{FogOtherDraw, FogOtherTarget, FogQuadDraw, FogQuadGpu, FogQuadTarget};
use render_bridge::native_fog_quad::{self, Host};

use crate::native_decals_glue::{DecalBitmap, decode_bitmap, texture_view};
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

    // Its numbers are the call's and its depth test is frozen at the bind
    // (after the device asks this), so a scene replay can record it again
    // on its attachments.
    fn replays_scene(&self) -> bool {
        true
    }

    /// Drawn on every copy (the reference and the display).
    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let Some(test) = self.test else {
            native_fog_quad::unported("depth_clip");
        };
        let mut gpu = programs(t.device);
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

/// The quad's programs for `device`, made again when the device changes.
fn programs(device: &wgpu::Device) -> std::sync::MutexGuard<'static, Option<(wgpu::Device, FogQuadGpu)>> {
    let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if gpu.as_ref().is_none_or(|(d, _)| d != device) {
        *gpu = Some((device.clone(), FogQuadGpu::new()));
    }
    gpu
}

struct FogOtherPass {
    draw: FogOtherDraw,
    /// Image 0 of the rasterizer's ramp bitmap, decoded when issued;
    /// `None` for the weather plate's block, which samples stage 0 alone.
    ramp: Option<Arc<DecalBitmap>>,
    /// The device's depth test when issued; the strips test only when
    /// `[0x0048575d]` is 0 (`FogOtherDraw::depth_test`).
    test: Option<DepthTest>,
}

impl d3d::ExternalPass for FogOtherPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.test = Some(DepthTest { w_buffered: true, range: clip.range, clamp: clip.clamp });
    }

    fn replays_scene(&self) -> bool {
        true
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let test = match (self.draw.depth_test, self.test) {
            (false, _) => None,
            (true, Some(test)) => Some(test),
            (true, None) => native_fog_quad::unported("depth_clip"),
        };
        let ramp = self.ramp.as_ref().map(|b| texture_view(t.device, t.queue, b));
        let mut gpu = programs(t.device);
        let Some((_, pass)) = gpu.as_mut() else { return 0 };
        let target = FogOtherTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            color: t.color,
            color_format: t.color_format,
            depth: t.depth,
            guest: t.guest_size,
            z_scale: t.z_scale,
            ramp: ramp.as_ref(),
        };
        match pass.record_other(target, t.encoder, &self.draw, test) {
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
        matches!(device.external_stage_now(&mut DeviceMemory(mem), None, plan(), Box::new(pass)), Ok(true))
    }

    fn draw_other(&self, mem: &mut dyn GuestMemory, draw: &FogOtherDraw) -> bool {
        // The bridge resolves stage 2's header (`FogOtherDraw::ramp`), asking
        // the guest's texture cache when it streams; 0 for the plate.
        let header = (draw.ramp != 0).then_some(draw.ramp);
        let Ok(mut device) = self.device.lock() else { return false };
        let ramp = header.map(|h| decode_bitmap(&*mem, h, device.config().address_map).unwrap_or_else(|why| native_fog_quad::unported(why)));
        let pass = FogOtherPass { draw: draw.clone(), ramp, test: None };
        matches!(device.external_stage_now(&mut DeviceMemory(mem), None, plan(), Box::new(pass)), Ok(true))
    }
}

/// Either pass as a fog scene stage over the bound targets
/// (`d3d::Device::external_stage_now`): a replaying scene sink keeps it
/// typed and records it again, where it refuses an untyped pass.
fn plan() -> d3d::ExternalPlan {
    d3d::ExternalPlan { stage: timeline::native::Stage::Fog, view: 0, ranges: 1, slots: Vec::new() }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_fog_quad::on(&s)) {
        return;
    }
    native_fog_quad::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
