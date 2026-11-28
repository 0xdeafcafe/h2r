//! The host of `render_bridge::native_fog_fans`: the second fog's fans as
//! stages where the guest's fan `0x00034420` would draw
//! (docs/graphics/retire-d3d-fog-fans.md). Each fan binds its own target
//! (`native_targets_glue::target_ref` of its header) and draws on it; a
//! fan that samples that target reads a copy of it. A bitmap stage's
//! texture is decoded from the guest header `0x000143c0` binds
//! (`native_decals_glue::decode_bitmap`), which the bridge resolves
//! through the guest's texture cache.

use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use frame_types::{Slot, TargetRef};
use native_render::fog_fans::{FanTarget, FogFansGpu};
use native_render::glow::PostSurface;
use native_render::program::ViewInputs;
use native_render::{FrameOp, Stage, StageWork, frame_builder};
use render_bridge::native_fog_fans::{self, Fan, Host, Source};

use crate::native_decals_glue::{DecalBitmap, decode_bitmap, texture_view};
use crate::native_targets_glue::{encoded, push, target_ref};

/// The fans' programs on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FogFansGpu)>> = Mutex::new(None);

/// What a stage reads on the GPU: the fan's own target, or a decoded image.
#[derive(Clone)]
enum Bound {
    Dest,
    Texture(Arc<DecalBitmap>),
}

struct FanPass {
    fan: Arc<Fan>,
    stages: [Option<Bound>; 4],
}

impl Stage for FanPass {
    fn name(&self) -> &'static str {
        "fog fan"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> StageWork {
        let textures: [Option<(wgpu::TextureView, (u32, u32))>; 4] = self.stages.each_ref().map(|b| match b {
            Some(Bound::Texture(b)) => Some((texture_view(&t.device, &t.queue, b), b.size())),
            Some(Bound::Dest) | None => None,
        });
        let dest_read = self.stages.each_ref().map(|b| matches!(b, Some(Bound::Dest)));
        let (fan, guest) = (Arc::clone(&self.fan), view.guest_size);
        let (device, queue, uploads, color, color_format) =
            (t.device.clone(), t.queue.clone(), t.uploads.clone(), t.color.clone(), t.color_format);
        encoded(move |encoder| {
            let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
            if gpu.as_ref().is_none_or(|(d, _)| *d != device) {
                *gpu = Some((device.clone(), FogFansGpu::new()));
            }
            let Some((_, pass)) = gpu.as_mut() else { return };
            let mut views = textures.clone();
            for (n, dest) in dest_read.iter().enumerate() {
                if *dest {
                    // A fan that samples the target it draws into reads a copy.
                    match pass.snapshot(&device, encoder, &color) {
                        Ok(v) => views[n] = Some((v, guest)),
                        Err(why) => native_fog_fans::unported(why),
                    }
                }
            }
            let target = FanTarget {
                device: &device,
                queue: &queue,
                uploads: &uploads,
                dest: PostSurface { view: &color, guest_size: guest },
                dest_format: color_format,
                stages: views.each_ref().map(|v| v.as_ref().map(|(view, size)| (view, *size))),
            };
            if let Err(why) = pass.record(target, encoder, &fan.draw) {
                native_fog_fans::unported(why);
            }
        })
    }
}

struct Draw;

impl Host for Draw {
    fn fan(&self, mem: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str> {
        let mut stages: [Option<Bound>; 4] = Default::default();
        for (n, s) in fan.sources.iter().enumerate() {
            stages[n] = match *s {
                None => None,
                Some(Source::Surface(header)) if header == fan.dest => Some(Bound::Dest),
                Some(Source::Surface(header)) => unimplemented!("fog fans: sampled surface {header:#x}"),
                Some(Source::Texture(header)) => {
                    let b = decode_bitmap(&*mem, header)?;
                    // A linear texture is fetched in texels; the fans' bitmap
                    // stages are normalised.
                    if b.linear() {
                        return Err("bitmap_linear");
                    }
                    Some(Bound::Texture(b))
                }
            };
        }
        // The fan's setup selects its own target (`0x00014bc0`) and is cut,
        // so the fan binds its target, colour only.
        frame_builder().push(FrameOp::Bind { colour: target_ref(&*mem, fan.dest, true), depth: TargetRef::None });
        push(&*mem, Slot::AfterLights, FanPass { fan: Arc::new(fan.clone()), stages });
        Ok(())
    }
}

pub fn install() {
    native_fog_fans::set_host(Arc::new(Draw));
}
