//! The Game panel's picture: the paired game's own frames, uploaded from
//! the picture ring (`play::picture`) to a texture and drawn above the
//! session controls (docs/editor/game-view.md, "Attached mode").
//!
//! The panel only reads. The game keeps its window and its input.

use std::time::Instant;

use dear_imgui_rs::Ui;
use dear_imgui_wgpu::ExternalTextureId;

use crate::app::App;
use crate::play::picture::{Picture, State, Stats};
use crate::theme;
use crate::viewport::Gpu;

struct Texture {
    texture: wgpu::Texture,
    id: ExternalTextureId,
    size: (u32, u32),
    /// The frame's `rev` last uploaded.
    rev: u64,
}

/// The picture's UI-only state.
#[derive(Default)]
pub struct GameViewUi {
    texture: Option<Texture>,
    /// The line under the picture, as last drawn.
    pub line: String,
    /// The frame (`rev`, size) the texture holds.
    pub uploaded: Option<(u64, [u32; 2])>,
}

impl GameViewUi {
    /// Draws the picture, or why there is none. `header` is the game's map,
    /// phase and the link plate's state.
    pub fn draw(&mut self, ui: &Ui, app: &mut App, header: &str, gpu: &mut Gpu<'_>) {
        let now = Instant::now();
        let Some(picture) = app.pairing.picture.as_mut() else {
            self.line.clear();
            self.uploaded = None;
            ui.text_disabled("Pair with a game to see its picture.");
            return;
        };
        picture.poll(now);
        let stats = picture.stats(now);
        self.line = line(&stats);
        ui.text(header);
        match &stats.state {
            State::Failed(why) => {
                ui.text_colored(theme::WARN, "No picture");
                ui.text_wrapped(why);
                return;
            }
            State::Connecting => {
                ui.text_disabled("Opening the game's picture...");
                return;
            }
            _ => {}
        }
        if let Some(texture) = self.upload(picture, gpu) {
            let (w, h) = self.texture.as_ref().map_or((1, 1), |t| t.size);
            let wide = ui.content_region_avail()[0].max(64.0);
            let size = [wide, wide * h as f32 / w as f32];
            ui.image(texture, size);
        } else if stats.frames == 0 {
            ui.text_disabled("Waiting for the first frame...");
        }
        let colour = match stats.state {
            State::Open => theme::OK,
            _ => theme::WARN,
        };
        ui.text_colored(colour, &self.line);
    }

    fn upload(&mut self, picture: &Picture, gpu: &mut Gpu<'_>) -> Option<dear_imgui_rs::TextureId> {
        let frame = picture.frame()?;
        let size = (frame.width, frame.height);
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        if self.texture.as_ref().is_none_or(|t| t.size != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("game picture"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            // A new size keeps the registered id and swaps its view.
            let id = match self.texture.as_ref().map(|t| t.id) {
                Some(id) => gpu
                    .renderer
                    .update_external_texture(id, &view)
                    .map(|()| id),
                None => gpu.renderer.register_external_texture(&view),
            };
            let id = match id {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("game picture texture: {e}");
                    return None;
                }
            };
            self.texture = Some(Texture {
                texture,
                id,
                size,
                rev: 0,
            });
        }
        let t = self.texture.as_mut()?;
        if t.rev != frame.rev {
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &t.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &frame.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * size.0),
                    rows_per_image: Some(size.1),
                },
                wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
            );
            t.rev = frame.rev;
        }
        self.uploaded = Some((t.rev, [size.0, size.1]));
        Some(t.id.texture_id())
    }
}

/// The line under the picture: its size and rate, or how long it has been
/// still.
pub fn line(stats: &Stats) -> String {
    let size = stats
        .size
        .map_or_else(String::new, |[w, h]| format!("{w}x{h} · "));
    match &stats.state {
        State::Stalled => format!(
            "{size}no new frame for {:.1} s",
            stats.age.map_or(0.0, |a| a.as_secs_f64())
        ),
        State::Closed => "the game closed its picture".to_owned(),
        State::Failed(why) => why.clone(),
        State::Connecting => "opening the picture".to_owned(),
        State::Open => format!(
            "{size}{:.0} fps · {} dropped{}",
            stats.fps,
            stats.dropped,
            stats.tick.map_or_else(String::new, |t| format!(" · tick {t}")),
        ),
    }
}
