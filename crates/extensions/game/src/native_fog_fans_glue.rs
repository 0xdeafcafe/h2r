//! The host of `render_bridge::native_fog_fans`: the second fog's fans
//! placed on the shared device where the guest's fan `0x00034420` would
//! draw (docs/graphics/retire-d3d-fog-fans.md). The target a fan draws
//! into and the targets it samples are named to the device as slots by
//! their surface headers, so the pass draws on the frame graph's surfaces
//! for their roles; a bitmap stage's texture is decoded from the guest
//! header `0x000143c0` binds (`native_decals_glue::decode_bitmap`), which
//! the bridge resolves through the guest's texture cache.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::fog_fans::{FanTarget, FogFansGpu};
use native_render::glow::PostSurface;
use render_bridge::native_fog_fans::{self, Fan, Host, Source};

use crate::native_decals_glue::{DecalBitmap, decode_bitmap, texture_view};
use crate::native_lights_glue::DeviceMemory;

/// The fans' programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogFansGpu)>> = Mutex::new(None);

/// What a stage reads on the GPU: a slot, or a decoded image.
#[derive(Clone)]
enum Bound {
    Slot(usize),
    Texture(Arc<DecalBitmap>),
}

struct FanPass {
    fan: Fan,
    stages: [Option<Bound>; 4],
}

/// The plan's slot of the fan's target.
const DEST: usize = 0;

impl d3d::ExternalPass for FanPass {
    // It draws into its slots, not the bound target.
    fn colour_only(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    // Its numbers are the call's and its images loaded, so a scene replay
    // can record it again on its slots.
    fn replays_scene(&self) -> bool {
        true
    }

    /// Drawn on every copy (the reference and the display).
    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let slot = |n: usize| t.slots.get(n).and_then(Option::as_ref);
        let Some(dest) = slot(DEST) else {
            native_fog_fans::unported("fan_slots");
        };
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FogFansGpu::new()));
        }
        let Some((_, pass)) = gpu.as_mut() else { return 0 };
        let mut views: [Option<(wgpu::TextureView, (u32, u32))>; 4] = Default::default();
        for (n, b) in self.stages.iter().enumerate() {
            views[n] = match b {
                None => None,
                // A fan that samples the target it draws into reads a copy.
                Some(Bound::Slot(DEST)) => match pass.snapshot(t.device, t.encoder, &dest.attachment) {
                    Ok(v) => Some((v, dest.guest_size)),
                    Err(why) => native_fog_fans::unported(why),
                },
                Some(Bound::Slot(s)) => {
                    let Some(c) = slot(*s) else { native_fog_fans::unported("fan_slots") };
                    Some((c.sample.clone(), c.guest_size))
                }
                Some(Bound::Texture(b)) => Some((texture_view(t.device, t.queue, b), b.size())),
            };
        }
        let target = FanTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            dest: PostSurface::from(dest),
            dest_format: dest.format,
            stages: views.each_ref().map(|v| v.as_ref().map(|(view, size)| (view, *size))),
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
    fn fan(&self, mem: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str> {
        let Ok(mut device) = self.device.lock() else { return Err("placed_lock") };
        let address_map = device.config().address_map;
        // Slot 0 the fan's target, written; each other surface once, read.
        let mut slots = vec![Some(d3d::ExternalSlot { header: fan.dest, written: true })];
        let mut stages: [Option<Bound>; 4] = Default::default();
        for (n, s) in fan.sources.iter().enumerate() {
            stages[n] = match *s {
                None => None,
                Some(Source::Surface(header)) => {
                    let found = slots.iter().position(|s| s.is_some_and(|s| s.header == header));
                    let at = match found {
                        Some(at) => at,
                        None => {
                            slots.push(Some(d3d::ExternalSlot { header, written: false }));
                            slots.len() - 1
                        }
                    };
                    Some(Bound::Slot(at))
                }
                Some(Source::Texture(header)) => {
                    let b = decode_bitmap(&*mem, header, address_map)?;
                    // A linear texture is fetched in texels; the fans' bitmap
                    // stages are normalised.
                    if b.linear() {
                        return Err("bitmap_linear");
                    }
                    Some(Bound::Texture(b))
                }
            };
        }
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Fog, view: 0, ranges: 1, slots };
        let pass = FanPass { fan: fan.clone(), stages };
        // The fan's setup selects its own target (`0x00014bc0`) and is cut,
        // so the pass names the fan's target, not the one bound: a fog
        // scene stage placed after the drained draws, nothing rebound.
        let (w, h) = device.surface_size(&DeviceMemory(mem), fan.dest).ok_or("placed_header")?;
        let targets = d3d::ExternalTargets { color: fan.dest, depth: 0, rect: (0, 0, w, h) };
        match device.external_stage_now(&mut DeviceMemory(mem), Some(targets), plan, Box::new(pass)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("placed_refused"),
            Err(_) => Err("placed_device"),
        }
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_fog_fans::on(&s)) {
        return;
    }
    native_fog_fans::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
