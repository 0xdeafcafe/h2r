//! Host-rendered Theater pages and clean screenshots of the published game
//! frame. The shell calls these after the guest blit.

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Accept,
    Step,
    Left,
    Right,
    Up,
    Down,
    Slower,
    Faster,
    Hide,
    Hud,
    Clip,
    Screenshot,
    Camera,
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    Rise,
    Fall,
    Leave,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub rgba: Vec<u8>,
    pub size: (u32, u32),
    /// Scrub hit target in page pixels, when the playback bar is active.
    pub timeline: Option<[u32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub page: Option<Arc<Page>>,
    pub playing: bool,
}

/// The shell owns pixels and host input. The caller owns playback and
/// provides a snapshot of the overlay even when the guest is paused.
pub trait Controller: Send + Sync {
    fn update(&self) -> View;
    fn key(&self, key: Key, down: bool, repeat: bool) -> bool;
    fn release_keys(&self);
    fn mouse_look(&self, dx: f64, dy: f64) -> bool;
    fn mouse_zoom(&self, amount: f32) -> bool;
    fn scrub(&self, fraction: f32) -> bool;
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
    use std::time::{SystemTime, UNIX_EPOCH};

    let frame = frame.ok_or("No frame to capture yet")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = dir.join(format!("theater-{now}.png"));
    write(ctx, frame, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A screenshot of `frame` at exactly `path` (`host.screenshot`,
/// [`agent_proto::host_shot`]; the theater's [`screenshot`] names its
/// own). The frame's size.
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) fn screenshot_to(
    ctx: &gfx::GfxContext,
    frame: &crate::ShownFrame,
    path: &Path,
) -> Result<(u32, u32), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write(ctx, frame, path).map_err(|e| e.to_string())?;
    Ok(frame.size)
}

/// The frame's RGBA pixels as RGB. The window is opaque, but the frame's
/// alpha holds whatever the last pass wrote there; kept in the PNG, a
/// viewer blends the picture over white.
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) fn opaque_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// The frame's pixels as an opaque PNG at `path`; a half-written file is
/// removed.
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
fn write(
    ctx: &gfx::GfxContext,
    frame: &crate::ShownFrame,
    path: &Path,
) -> anyhow::Result<()> {
    use std::io::BufWriter;

    let pixels = gfx::readback::read_texture(
        ctx,
        &frame.texture,
        wgpu::TextureAspect::All,
        4,
        frame.size,
    );
    let file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), frame.size.0, frame.size.1);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let result = encoder.write_header().and_then(|mut out| {
        out.write_image_data(&opaque_rgb(&pixels))?;
        out.finish()
    });
    if let Err(error) = result {
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod opaque_tests {
    #[test]
    fn a_screenshot_drops_the_frames_alpha() {
        let rgba = [10, 20, 30, 0, 40, 50, 60, 128];
        assert_eq!(super::opaque_rgb(&rgba), vec![10, 20, 30, 40, 50, 60]);
    }
}
