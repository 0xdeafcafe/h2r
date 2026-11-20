use winit::keyboard::KeyCode;

use crate::menu_overlay::{MenuRenderer, PlatePosition, line_area};
use crate::scale::Viewport;

use super::{Key, View};

pub(crate) fn key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Space => Key::Space,
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
    plates: [MenuRenderer; 3],
    timeline: Option<Viewport>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            plates: std::array::from_fn(|_| MenuRenderer::new(device)),
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
        size: (u32, u32),
        area: Viewport,
    ) {
        self.timeline = None;
        if area.is_empty() {
            return;
        }
        self.timeline = line_area(&model.transport, 1, area, PlatePosition::BottomCenter);
        for (renderer, (lines, pos)) in self.plates.iter_mut().zip([
            (&model.players, PlatePosition::LeftCenter),
            (&model.camera, PlatePosition::TopRight),
            (&model.transport, PlatePosition::BottomCenter),
        ]) {
            renderer.draw_at(
                &ctx.device,
                &ctx.queue,
                encoder,
                lines,
                dst,
                format,
                size,
                area,
                pos,
            );
        }
    }
}
