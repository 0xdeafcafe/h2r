use qrcodegen::{QrCode, QrCodeEcc};
use winit::keyboard::KeyCode;

use crate::menu_overlay::{MenuRenderer, PlatePosition};
use crate::overlay::Sprite;
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
        KeyCode::Escape => Key::Leave,
        _ => return None,
    })
}

pub(crate) struct Renderer {
    plates: [MenuRenderer; 3],
    qr: Sprite,
    qr_drawn: Option<(String, u32, u32)>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            plates: std::array::from_fn(|_| MenuRenderer::new(device)),
            qr: Sprite::new(device, "app theater QR"),
            qr_drawn: None,
        }
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
        if area.is_empty() {
            return;
        }
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
        if let Some(url) = &model.web_url {
            self.draw_qr(ctx, encoder, url, dst, format, size, area);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_qr(
        &mut self,
        ctx: &gfx::GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        url: &str,
        dst: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        area: Viewport,
    ) {
        // The QR fits over the map picture while the URL stays readable below.
        let room = (area.height / 3).min(area.width / 3);
        if self
            .qr_drawn
            .as_ref()
            .is_none_or(|(old, r, _)| old != url || *r != room)
        {
            self.qr_drawn = None;
            if let Some((side, rgba)) = qr_pixels(url, room) {
                self.qr.upload(&ctx.device, &ctx.queue, (side, side), &rgba);
                self.qr_drawn = Some((url.to_owned(), room, side));
            }
        }
        if let Some((_, _, side)) = self.qr_drawn {
            let x = area.x + area.width * 83 / 640;
            let y = area.y + area.height * 218 / 480;
            self.qr
                .draw(&ctx.device, encoder, dst, format, size, (x, y));
        }
    }
}

fn qr_pixels(url: &str, room: u32) -> Option<(u32, Vec<u8>)> {
    let qr = QrCode::encode_text(url, QrCodeEcc::Medium).ok()?;
    let modules = qr.size() as u32 + 8;
    let zoom = room / modules;
    if zoom == 0 {
        return None;
    }
    let side = modules * zoom;
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let gray = if qr.get_module((x / zoom) as i32 - 4, (y / zoom) as i32 - 4) {
                0
            } else {
                255
            };
            pixels.extend([gray, gray, gray, 255]);
        }
    }
    Some((side, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theater_qr_has_quiet_zone_and_integer_modules() {
        let (side, pixels) = qr_pixels("https://example.com/theater/film", 180).unwrap();
        assert!(side <= 180);
        assert_eq!(pixels.len(), (side * side * 4) as usize);
        assert!(pixels[..(side * 4) as usize].iter().all(|&c| c == 255));
        assert!(pixels.contains(&0));
        assert!(qr_pixels("https://example.com/theater/film", 20).is_none());
    }
}
