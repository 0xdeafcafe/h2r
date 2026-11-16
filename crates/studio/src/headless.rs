//! Real frames of the studio UI rendered offscreen with wgpu and written
//! as PNGs: the same [`Studio`] and [`App`] as the window, no window. The
//! guide's screenshots come from here (`h2-editor --screenshot`), and a
//! test renders one frame of the invented test map.
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use dear_imgui_rs::Context;
use dear_imgui_wgpu::{FramebufferExtent, GammaMode, WgpuInitInfo, WgpuRenderer};

use crate::app::App;
use crate::ui::Studio;
use crate::viewport::Gpu;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// An offscreen studio.
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: WgpuRenderer,
    pub studio: Studio,
    /// Logical size in points and the framebuffer scale (2 for a Retina
    /// look).
    size: [f32; 2],
    scale: f32,
    target: wgpu::Texture,
    // Declared last: the renderer and studio must go before the context.
    ctx: Context,
}

impl Headless {
    /// A studio of `size` points at `scale` pixels per point. Errors when
    /// the machine has no wgpu adapter.
    pub fn new(size: [f32; 2], scale: f32) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no wgpu adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .context("request a wgpu device")?;
        let (mut ctx, fonts) = crate::context(None);
        let io = ctx.io_mut();
        io.set_display_size(size);
        io.set_display_framebuffer_scale([scale, scale]);
        let mut renderer = WgpuRenderer::new(
            WgpuInitInfo::new(device.clone(), queue.clone(), FORMAT),
            &mut ctx,
        )
        .map_err(|e| anyhow!("imgui renderer: {e}"))?;
        renderer.set_gamma_mode(GammaMode::Auto);
        let px = Self::pixels(size, scale);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("studio screenshot"),
            size: wgpu::Extent3d {
                width: px[0],
                height: px[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut studio = Studio::new(fonts);
        studio.dialogs = false;
        Ok(Self {
            device,
            queue,
            renderer,
            studio,
            size,
            scale,
            target,
            ctx,
        })
    }

    fn pixels(size: [f32; 2], scale: f32) -> [u32; 2] {
        [
            (size[0] * scale).round().max(1.0) as u32,
            (size[1] * scale).round().max(1.0) as u32,
        ]
    }

    /// Build and render one frame (`dt` seconds after the last).
    pub fn frame(&mut self, app: &mut App, dt: f32) -> Result<()> {
        self.ctx.io_mut().set_delta_time(dt.max(1e-4));
        self.ctx.io_mut().set_display_size(self.size);
        let ui = self.ctx.frame();
        let mut gpu = Gpu {
            device: &self.device,
            queue: &self.queue,
            renderer: &mut self.renderer,
        };
        self.studio.frame(ui, app, &mut gpu);
        let view = self.target.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio screenshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let pending = self
                .ctx
                .try_render(
                    self.renderer
                        .renderer_consumer()
                        .map_err(|e| anyhow!("imgui renderer: {e}"))?,
                )
                .map_err(|e| anyhow!("imgui render: {e}"))?;
            self.renderer
                .render(
                    pending,
                    &mut pass,
                    FramebufferExtent::from_texture(&self.target),
                )
                .map_err(|e| anyhow!("imgui draw: {e}"))?;
        }
        self.queue.submit([enc.finish()]);
        Ok(())
    }

    /// Run `frames` frames (layout settles over the first few) and return
    /// the last as tightly packed RGBA8 rows.
    pub fn capture(&mut self, app: &mut App, frames: usize) -> Result<(Vec<u8>, [u32; 2])> {
        for _ in 0..frames.max(1) {
            self.frame(app, 1.0 / 60.0)?;
        }
        let [w, h] = Self::pixels(self.size, self.scale);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("wgpu poll: {e}"))?;
        let data = buf
            .slice(..)
            .get_mapped_range()
            .map_err(|e| anyhow!("map the readback: {e}"))?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            out.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        Ok((out, [w, h]))
    }

    /// Render `frames` frames and write the last to `path` as a PNG.
    pub fn screenshot(&mut self, app: &mut App, frames: usize, path: &Path) -> Result<()> {
        let (rgba, [w, h]) = self.capture(app, frames)?;
        write_png(path, &rgba, w, h)
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        let _ = self.ctx.end_frame();
        let _ = self.renderer.shutdown(&mut self.ctx);
    }
}

/// Write tightly packed RGBA8 rows as a PNG.
pub fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use scenario_edit::{BlockKind, fixture};

    /// One real frame of the shell over the invented test map, read back:
    /// the UI drew something other than the clear colour, and the PNG
    /// writes. Skipped when the machine has no wgpu adapter.
    #[test]
    fn renders_the_studio_offscreen() {
        let mut h = match Headless::new([1280.0, 800.0], 1.0) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let dir = std::env::temp_dir().join(format!("h2-editor-shot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let map = dir.join("example.map");
        std::fs::write(&map, fixture::fixture()).unwrap();
        let mut app = App::new();
        app.set_data_root(dir.join("data"));
        app.dispatch(Action::OpenSource(map));
        app.dispatch(Action::Select(Some((BlockKind::Weapons, 1))));
        let (rgba, [w, hgt]) = h.capture(&mut app, 4).unwrap();
        assert_eq!(rgba.len(), (w * hgt * 4) as usize);
        let distinct: std::collections::HashSet<[u8; 4]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(97)
            .copied()
            .collect();
        assert!(distinct.len() > 8, "the frame is nearly uniform");
        let out = dir.join("studio.png");
        write_png(&out, &rgba, w, hgt).unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 1000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
