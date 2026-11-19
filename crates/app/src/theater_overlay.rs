//! Host-side Theater plates and clean screenshots of the published game
//! frame. The shell calls these after the guest blit.

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use debug_menu::Line;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Left,
    Right,
    Up,
    Down,
    Slower,
    Faster,
    Camera,
    Hide,
    Hud,
    Clip,
    Screenshot,
    Leave,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub players: Vec<Line>,
    pub camera: Vec<Line>,
    pub transport: Vec<Line>,
    pub web_url: Option<String>,
}

/// The shell owns pixels and host input. The caller owns playback and
/// provides a snapshot of the overlay even when the guest is paused.
pub trait Controller: Send + Sync {
    fn update(&self) -> View;
    fn key(&self, key: Key, down: bool, repeat: bool) -> bool;
    fn release_keys(&self);
    fn take_screenshot(&self) -> Option<PathBuf>;
    fn screenshot_saved(&self, result: Result<PathBuf, String>);
    fn take_leave(&self) -> bool;
}

#[derive(Clone)]
pub struct Overlay(pub Arc<dyn Controller>);

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TheaterOverlay").finish_non_exhaustive()
    }
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
mod desktop;

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) use desktop::{Renderer, key};

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) fn screenshot(
    ctx: &gfx::GfxContext,
    frame: Option<&crate::ShownFrame>,
    dir: &Path,
) -> Result<PathBuf, String> {
    use std::io::BufWriter;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write(
        ctx: &gfx::GfxContext,
        frame: &crate::ShownFrame,
        dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        let pixels = gfx::readback::read_texture(
            ctx,
            &frame.texture,
            wgpu::TextureAspect::All,
            4,
            frame.size,
        );
        std::fs::create_dir_all(dir)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = dir.join(format!("theater-{now}.png"));
        let file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), frame.size.0, frame.size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let result = encoder.write_header().and_then(|mut out| {
            out.write_image_data(&pixels)?;
            out.finish()
        });
        if let Err(error) = result {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(path)
    }

    let frame = frame.ok_or("No frame to capture yet")?;
    write(ctx, frame, dir).map_err(|e| e.to_string())
}
