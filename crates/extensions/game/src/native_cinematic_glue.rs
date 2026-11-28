//! Draws the Halo 1 cinematic screen pass (`h1_cinematics`) with
//! `native_render::cinematic_gpu` over the frame's colour target
//! (docs/graphics/native-post-hud.md). The feature and the stage know
//! nothing of the frame graph; this glue pushes the stage.
use crate::h1_cinematics::{Request, install_draw, video_images};
use frame_types::Slot;
use native_render::cinematic_gpu::{Cache, Target, Video};
use std::sync::{Arc, Mutex};

struct Pass {
    request: Arc<Request>,
    cache: Arc<Mutex<Cache>>,
}

impl native_render::Stage for Pass {
    fn name(&self) -> &'static str {
        "cinematic"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        let (request, cache) = (Arc::clone(&self.request), Arc::clone(&self.cache));
        let (device, queue, color, format) = (t.device.clone(), t.queue.clone(), t.color.clone(), t.color_format);
        let (guest_size, size) = (view.guest_size, view.size);
        crate::native_targets_glue::encoded(move |encoder| {
            let target = Target {
                device: &device,
                queue: &queue,
                encoder,
                color: &color,
                color_format: format,
                viewport: [0.0, 0.0, size.0 as f32, size.1 as f32],
                guest_size,
                size,
                display: false,
            };
            let map = request.map.as_deref();
            let images = || map.and_then(video_images);
            let video = map.map(|map| Video { map, images: &images });
            cache.lock().unwrap_or_else(|e| e.into_inner()).encode(target, request.params, video);
        })
    }
}

/// Installs the cinematic pass.
pub fn install() {
    let cache = Arc::new(Mutex::new(Cache::default()));
    install_draw(Arc::new(move |mem, request| {
        let pass = Pass { request: Arc::new(request), cache: Arc::clone(&cache) };
        crate::native_targets_glue::push(&*mem, Slot::Transparents, pass);
    }));
}
