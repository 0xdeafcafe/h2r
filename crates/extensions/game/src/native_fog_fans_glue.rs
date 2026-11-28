//! The host of `render_bridge::native_fog_fans`: the second fog's fans
//! placed on the shared device where the guest's fan `0x00034420` would
//! draw (docs/graphics/retire-d3d-fog-fans.md). The target a fan draws
//! into and the targets it samples are named to the device as slots by
//! their surface headers, so the pass draws on the frame graph's surfaces
//! for their roles; the bitmaps are the ones [`load`] decoded at map load.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::fog_fans::{FanTarget, FogFansGpu};
use native_render::glow::PostSurface;
use native_render::gpu::Image;
use render_bridge::native_fog_fans::{self, Fan, Host, Source};

use crate::native_lights_glue::DeviceMemory;

/// The fans' programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogFansGpu)>> = Mutex::new(None);

/// The images a map's fans sample by (bitmap datum, image), and the
/// load they came from.
static IMAGES: Mutex<(u64, Option<Arc<HashMap<(u32, u32), Image>>>)> = Mutex::new((0, None));

/// The most images of one fog patch bitmap decoded.
const MAX_IMAGES: usize = 64;

/// At map load: every image of every fog patch's bitmap (`fpch` `+0x18`,
/// the datum of its tag reference at `+0x14`) and image 0 of the
/// rasterizer bitmap the layers' composite binds at stage 2
/// (`[[0x00485a80] + 0x14]`). Says on stderr what loaded.
pub fn load<R: std::io::Read + std::io::Seek>(map: &mut map::Map<R>, files: &mut assets::ResourceFiles) {
    let mut images = HashMap::new();
    let mut missing = Vec::new();
    let meta = match tags::shader::Metadata::read(map) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("native-fog-fans: the tag metadata does not read ({e}); a fog fan panics on its bitmap");
            *IMAGES.lock().unwrap_or_else(|e| e.into_inner()) = (next_generation(), Some(Arc::new(images)));
            return;
        }
    };
    let word = |a: u32| meta.bytes(a, 4).ok().and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let patches: Vec<u32> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"fpch")
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .collect();
    let mut bitmaps: Vec<(u32, usize)> = patches
        .iter()
        .filter_map(|&at| word(at + engine::fog_fans::layers_global::BITMAP_DATUM))
        .filter(|&d| d != u32::MAX)
        .map(|d| (d, MAX_IMAGES))
        .collect();
    let matg = map.tags().iter().find(|t| t.group.0 == *b"matg").and_then(|t| t.location.as_ref().map(|l| l.address));
    if let Some(d) = matg.and_then(|matg| {
        crate::native_lights_glue::rasterizer_bitmap(
            &|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec),
            matg,
            crate::native_lights_glue::RASTERIZER_DATUM_14,
        )
    }) {
        bitmaps.push((d.0, 1));
    }
    bitmaps.sort();
    bitmaps.dedup_by(|a, b| a.0 == b.0 && {
        b.1 = b.1.max(a.1);
        true
    });
    for (datum, count) in bitmaps {
        for image in 0..count {
            match native_render::load::decode_image(map, files, map::TagId(datum), image) {
                Some(i) => {
                    images.insert((datum, image as u32), i);
                }
                None if image == 0 => missing.push(datum),
                None => break,
            }
        }
    }
    eprintln!(
        "native-fog-fans: {} images of {} fog patches and the rasterizer bitmap; bitmaps that do not decode: {missing:x?}",
        images.len(),
        patches.len()
    );
    *IMAGES.lock().unwrap_or_else(|e| e.into_inner()) = (next_generation(), Some(Arc::new(images)));
}

fn next_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What a stage reads on the GPU: a slot, or a decoded image.
#[derive(Clone)]
enum Bound {
    Slot(usize),
    Image((u32, u32), Image),
}

struct FanPass {
    fan: Fan,
    stages: [Option<Bound>; 4],
    generation: u64,
}

/// The plan's slot of the fan's target.
const DEST: usize = 0;

impl d3d::ExternalPass for FanPass {
    // It draws into its slots, not the bound target.
    fn colour_only(&self) -> bool {
        true
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
                Some(Bound::Image(key, image)) => {
                    Some((pass.bitmap(t.device, t.queue, self.generation, *key, image), (image.width, image.height)))
                }
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
        let (generation, images) = IMAGES.lock().unwrap_or_else(|e| e.into_inner()).clone();
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
                Some(Source::Bitmap { datum, image }) => {
                    let i = images.as_ref().and_then(|m| m.get(&(datum, image))).ok_or("bitmap")?;
                    Some(Bound::Image((datum, image), i.clone()))
                }
            };
        }
        let Ok(mut device) = self.device.lock() else { return Err("placed") };
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Fog, view: 0, ranges: 1, slots };
        let pass = FanPass { fan: fan.clone(), stages, generation };
        match device.prepare_external_pass(&mut DeviceMemory(mem), plan, Box::new(pass)) {
            Ok(Some(reservation)) => {
                reservation.commit();
                Ok(())
            }
            _ => Err("placed"),
        }
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_fog_fans::on(&s)) {
        return;
    }
    native_fog_fans::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
