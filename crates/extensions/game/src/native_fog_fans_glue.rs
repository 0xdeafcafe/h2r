//! The host of `render_bridge::native_fog_fans`: the second fog's
//! composite fan placed on the shared device where the guest's fan
//! `0x00034420` would draw, into the view's target and sampling target 3
//! (docs/graphics/retire-d3d-fog-fans.md). Both are named to the device
//! as slots by their surface headers, so the pass draws on the frame
//! graph's surfaces for their roles.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::fog_fans::{FogFansGpu, FogFansTarget};
use native_render::glow::PostSurface;
use render_bridge::native_fog_fans::{self, CompositeFan, Host};

use crate::native_lights_glue::DeviceMemory;

/// The fans' programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogFansGpu)>> = Mutex::new(None);

/// The plan's slots: the view's target, then target 3.
const DEST: usize = 0;
const SOURCE: usize = 1;

struct CompositePass {
    fan: CompositeFan,
}

impl d3d::ExternalPass for CompositePass {
    // It draws into its slots, not the bound target.
    fn colour_only(&self) -> bool {
        true
    }

    // Its numbers are the call's, so a scene replay can record it again
    // on its slots.
    fn replays_scene(&self) -> bool {
        true
    }

    /// Drawn on every copy (the reference and the display).
    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let slot = |n: usize| t.slots.get(n).and_then(Option::as_ref);
        let (Some(dest), Some(source)) = (slot(DEST), slot(SOURCE)) else {
            native_fog_fans::unported("composite_slots");
        };
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FogFansGpu::new()));
        }
        let Some((_, pass)) = gpu.as_mut() else { return 0 };
        let target = FogFansTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            dest: PostSurface::from(dest),
            dest_format: dest.format,
            source: &source.sample,
            source_guest: source.guest_size,
        };
        match pass.record(target, t.encoder, &self.fan.draw) {
            Ok(()) => 1,
            Err(why) => native_fog_fans::unported(why),
        }
    }
}

struct Draw<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> Host for Draw<B> {
    fn composite(&self, mem: &mut dyn GuestMemory, fan: &CompositeFan) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let slots = [(fan.dest, true), (fan.source, false)];
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Fog,
            view: 0,
            ranges: 1,
            slots: slots.map(|(header, written)| Some(d3d::ExternalSlot { header, written })).to_vec(),
        };
        let pass = CompositePass { fan: fan.clone() };
        match device.prepare_external_pass(&mut DeviceMemory(mem), plan, Box::new(pass)) {
            Ok(Some(reservation)) => {
                reservation.commit();
                true
            }
            _ => false,
        }
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_fog_fans::on(&s)) {
        return;
    }
    native_fog_fans::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
