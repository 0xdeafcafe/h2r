//! A texture binding from an earlier submitted frame. Replay samples an
//! immutable result or checks the allocation behind a live surface view.
//! The guest retains a proxy. Only the render worker resolves it to GPU
//! objects. The response owns those objects after the proxy is dropped.

use std::sync::{Arc, Mutex, OnceLock, mpsc};

use crate::gfx_backend::HostTexture;

#[derive(Debug)]
pub(crate) struct Image {
    pub device: wgpu::Device,
    pub texture: HostTexture,
}

impl Image {
    pub fn for_device(&self, device: &wgpu::Device) -> Option<HostTexture> {
        (self.device == *device).then(|| self.texture.shared())
    }
}

/// Physical textures actually bound by the reference and display encoders.
/// In single-copy mode both may share one allocation at display resolution.
#[derive(Debug)]
pub struct FrozenImages {
    pub(crate) reference: Image,
    pub(crate) display: Option<Image>,
}

impl FrozenImages {
    pub(crate) fn same_copies(&self) -> bool {
        self.display.as_ref().is_some_and(|display| {
            self.reference.device == display.device
                && self.reference.texture.same_sample(&display.texture)
        })
    }

    pub(crate) fn single(device: &wgpu::Device, texture: &HostTexture) -> Self {
        let image = || Image {
            device: device.clone(),
            texture: texture.shared(),
        };
        Self {
            reference: image(),
            display: Some(image()),
        }
    }
}

#[derive(Debug)]
struct Resolution {
    images: OnceLock<Option<FrozenImages>>,
    answer: Mutex<Option<mpsc::Receiver<Option<FrozenImages>>>>,
}

#[derive(Clone, Debug)]
pub struct FrozenSample {
    pub(crate) proxy: Option<u64>,
    resolution: Arc<Resolution>,
}

impl FrozenSample {
    pub(crate) fn ready(images: FrozenImages) -> Self {
        Self {
            proxy: None,
            resolution: Arc::new(Resolution {
                images: OnceLock::from(Some(images)),
                answer: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn pending(proxy: u64) -> (Self, mpsc::Sender<Option<FrozenImages>>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                proxy: Some(proxy),
                resolution: Arc::new(Resolution {
                    images: OnceLock::new(),
                    answer: Mutex::new(Some(rx)),
                }),
            },
            tx,
        )
    }

    /// Called by a recording worker. The producing job is ordered after
    /// its original present and before any release of this guest proxy.
    pub(crate) fn images(&self) -> Option<&FrozenImages> {
        self.resolution
            .images
            .get_or_init(|| {
                self.resolution
                    .answer
                    .lock()
                    .ok()?
                    .take()?
                    .recv()
                    .ok()
                    .flatten()
            })
            .as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::gfx_backend::GfxBackend;
    use crate::gfx_backend::guest_draw_pass::commands::SceneDevice;
    use crate::scaled::{ScaledBackend, ScaledTexture};
    use gfx::texture::{TextureDesc, TextureKind, decode_texture};
    use gfx::{D3DFormat, GfxContext};

    #[test]
    fn replay_binds_the_projection_of_the_original_sampled_texture() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut reference = GfxBackend::new(ctx.clone());
        let mut display = GfxBackend::new(ctx);
        let desc =
            |width| TextureDesc::new(D3DFormat::LinA8R8G8B8, TextureKind::D2, width, 1, 1, 1);
        let decode = |width| {
            decode_texture(
                &desc(width),
                &vec![0; width as usize * 4],
                None,
                Default::default(),
            )
            .unwrap()
        };
        let pair = ScaledTexture {
            reference: reference.upload_texture(&decode(1)).unwrap(),
            view: Some(display.upload_texture(&decode(4)).unwrap()),
        };
        let one = pair.reference.texture.clone();
        let four = pair.view.as_ref().unwrap().texture.clone();
        let scaled = ScaledBackend::new(reference, display, 4).unwrap();
        let sample = scaled.retain_scene_sample(&pair).unwrap();
        drop(pair);

        assert!(!sample.images().unwrap().same_copies());
        assert_eq!(
            scaled
                .reference()
                .frozen_texture(&sample, 1)
                .unwrap()
                .texture,
            one
        );
        assert_eq!(
            scaled.view().frozen_texture(&sample, 4).unwrap().texture,
            four
        );
        // ScaledScenePass already owns the fourfold raster scale, even
        // though its commands take guest coordinates on a unit grid.
        let native = scaled.frozen_texture(&sample, 1).unwrap();
        assert_eq!(native.reference.texture, one);
        assert_eq!(native.view.unwrap().texture, four);
    }

    #[test]
    fn an_abandoned_proxy_resolves_to_unavailable() {
        let (sample, send) = FrozenSample::pending(18);
        drop(send);
        assert!(sample.images().is_none());
        assert!(sample.clone().images().is_none());
    }
}
