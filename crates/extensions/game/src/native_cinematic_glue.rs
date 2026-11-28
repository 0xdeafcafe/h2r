//! Draws the Halo 1 cinematic screen pass (`h1_cinematics`) with
//! `native_render::cinematic_gpu` over the device's colour target
//! (docs/graphics/native-post-hud.md). The device types stay here: the
//! feature and the stage know nothing of them.
use crate::h1_cinematics::{Request, install_draw, video_images};
use native_render::cinematic_gpu::{Cache, Target, Video};
use std::sync::{Arc, Mutex};

struct Pass {
    request: Request,
    cache: Arc<Mutex<Cache>>,
}

impl d3d::ExternalPass for Pass {
    fn colour_only(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let target = Target {
            device: t.device,
            queue: t.queue,
            encoder: t.encoder,
            color: t.color,
            color_format: t.color_format,
            viewport: t.viewport,
            guest_size: t.guest_size,
            size: t.size,
            display: t.copy == d3d::TargetCopy::Display,
        };
        let map = self.request.map.as_deref();
        let images = || map.and_then(video_images);
        let video = map.map(|map| Video { map, images: &images });
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .encode(target, self.request.params, video);
    }
}

/// Installs the cinematic pass over `device`.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let device = device.clone();
    let cache = Arc::new(Mutex::new(Cache::default()));
    install_draw(Arc::new(move |mem, request| {
        let mut d = device.lock().unwrap_or_else(|e| e.into_inner());
        let pass = Pass {
            request,
            cache: Arc::clone(&cache),
        };
        let _ = d.external_pass_now(
            &mut crate::native_lights_glue::DeviceMemory(mem),
            Box::new(pass),
        );
    }));
}
