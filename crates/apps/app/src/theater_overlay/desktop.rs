use winit::keyboard::KeyCode;

use std::sync::Arc;

use crate::overlay::Sprite;
use crate::scale::{Viewport, fit};

use super::{Key, Page, View};

pub(crate) fn key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Space => Key::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Accept,
        KeyCode::Period => Key::Step,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::BracketLeft => Key::Slower,
        KeyCode::BracketRight => Key::Faster,
        KeyCode::KeyH => Key::Hide,
        KeyCode::KeyJ => Key::Hud,
        KeyCode::KeyX => Key::Clip,
        KeyCode::F12 => Key::Screenshot,
        KeyCode::KeyC => Key::Camera,
        KeyCode::KeyW => Key::Forward,
        KeyCode::KeyS => Key::Backward,
        KeyCode::KeyA => Key::StrafeLeft,
        KeyCode::KeyD => Key::StrafeRight,
        KeyCode::KeyE => Key::Rise,
        KeyCode::KeyQ => Key::Fall,
        KeyCode::Escape => Key::Leave,
        _ => return None,
    })
}

pub(crate) struct Renderer {
    sprite: Sprite,
    uploaded: Option<Arc<Page>>,
    timeline: Option<Viewport>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: Sprite::new(device, "Theater page"),
            uploaded: None,
            timeline: None,
        }
    }

    pub fn timeline_fraction(&self, x: f64, y: f64) -> Option<f32> {
        let area = self.timeline?;
        let x = x - f64::from(area.x);
        let y = y - f64::from(area.y);
        let (width, height) = (f64::from(area.width), f64::from(area.height));
        (width > 1.0 && (0.0..width).contains(&x) && (0.0..height).contains(&y))
            .then(|| (x / (width - 1.0)).min(1.0) as f32)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        ctx: &gfx::GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        model: &View,
        dst: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        area: Viewport,
    ) {
        self.timeline = None;
        let Some(page) = &model.page else { return };
        if area.is_empty() {
            return;
        }
        let rect = fit(page.size.0 as f32 / page.size.1 as f32, (area.width, area.height));
        let rect = Viewport {
            x: area.x + rect.x,
            y: area.y + rect.y,
            ..rect
        };
        if let Some([x, y, width, height]) = page.timeline {
            let sx = |x| (u64::from(x) * u64::from(rect.width) / u64::from(page.size.0)) as u32;
            let sy = |y| (u64::from(y) * u64::from(rect.height) / u64::from(page.size.1)) as u32;
            self.timeline = Some(Viewport {
                x: rect.x + sx(x),
                y: rect.y + sy(y),
                width: sx(width),
                height: sy(height),
            });
        }
        if self.uploaded.as_ref().is_none_or(|old| !Arc::ptr_eq(old, page)) {
            self.sprite.upload(&ctx.device, &ctx.queue, page.size, &page.rgba);
            self.uploaded = Some(page.clone());
        }
        self.sprite.draw_in(&ctx.device, encoder, dst, format, rect);
    }
}
