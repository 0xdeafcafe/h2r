//! Real frames of the studio UI rendered offscreen with wgpu and written
//! as PNGs: the same [`Studio`] and [`App`] as the window, no window. The
//! guide's screenshots come from here (`h2-editor --screenshot`), and a
//! test renders one frame of the invented test map.
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use dear_imgui_rs::Context;
use dear_imgui_wgpu::{FramebufferExtent, GammaMode, WgpuInitInfo, WgpuRenderer};

use crate::app::App;
use crate::probe;
use crate::script::{Button, Scene, Step};
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
    /// What the last frame drew, to aim scripted input at.
    pub scene: Scene,
    /// Numbered boxes drawn over the frame: (target, badge text).
    pub callouts: Vec<(String, String)>,
    /// Where the scripted mouse is, in points.
    mouse: [f32; 2],
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
        // Screenshots want the camera where it was put, not gliding there.
        studio.viewport.camera.smooth = false;
        Ok(Self {
            device,
            queue,
            renderer,
            studio,
            size,
            scale,
            target,
            scene: Scene::default(),
            callouts: Vec::new(),
            mouse: [-1.0e4, -1.0e4],
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
        probe::begin();
        let mut gpu = Gpu {
            device: &self.device,
            queue: &self.queue,
            renderer: &mut self.renderer,
        };
        self.studio.frame(ui, app, &mut gpu);
        self.scene = Scene {
            items: probe::items(),
            windows: probe::windows(),
        };
        draw_callouts(ui, &self.scene, &self.callouts);
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
        let size = Self::pixels(self.size, self.scale);
        Ok((self.read(&self.target)?, size))
    }

    /// A 2D RGBA8 texture's texels as tightly packed rows (the texture
    /// needs `COPY_SRC`).
    pub fn read(&self, texture: &wgpu::Texture) -> Result<Vec<u8>> {
        let (w, h) = (texture.width(), texture.height());
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
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
        Ok(out)
    }

    /// Render `frames` frames; the last frame's viewport as the window
    /// shows it (cropped from the frame) and as the viewport rendered it
    /// (its own target), both tightly packed RGBA8 of the same size.
    pub fn viewport(&mut self, app: &mut App, frames: usize) -> Result<ViewportShot> {
        let (frame, [fw, fh]) = self.capture(app, frames)?;
        let (rect, target) = self
            .studio
            .viewport
            .shown()
            .ok_or_else(|| anyhow!("no viewport was drawn"))?;
        let rendered = self.read(target)?;
        let (w, h) = (target.width(), target.height());
        let x0 = (rect[0] * self.scale).round() as u32;
        let y0 = (rect[1] * self.scale).round() as u32;
        let (w, h) = (w.min(fw.saturating_sub(x0)), h.min(fh.saturating_sub(y0)));
        let crop = |src: &[u8], stride: u32, x: u32, y: u32| -> Vec<u8> {
            (0..h)
                .flat_map(|r| {
                    let start = (((y + r) * stride + x) * 4) as usize;
                    src[start..start + (w * 4) as usize].to_vec()
                })
                .collect()
        };
        Ok(ViewportShot {
            shown: crop(&frame, fw, x0, y0),
            rendered: crop(&rendered, target.width(), 0, 0),
            size: [w, h],
        })
    }

    /// Render `frames` frames and write the last to `path` as a PNG.
    pub fn screenshot(&mut self, app: &mut App, frames: usize, path: &Path) -> Result<()> {
        let (rgba, [w, h]) = self.capture(app, frames)?;
        write_png(path, &rgba, w, h)
    }

    /// Settle for `frames` frames, play `steps`, then write the frame to
    /// `path`, cropped to the union of `crop` (grown by `pad` points; made
    /// `height` points tall from its top) when given.
    #[allow(clippy::too_many_arguments)]
    pub fn scripted(
        &mut self,
        app: &mut App,
        frames: usize,
        steps: &[Step],
        crop: &[String],
        pad: f32,
        height: Option<f32>,
        path: &Path,
    ) -> Result<()> {
        for _ in 0..frames.max(1) {
            self.frame(app, 1.0 / 60.0)?;
        }
        for step in steps {
            self.step(app, step)
                .with_context(|| format!("--do {step:?}"))?;
        }
        let (rgba, [w, h]) = self.capture(app, 2)?;
        // What a script can aim at, for writing one.
        if std::env::var_os("H2_EDITOR_PROBE").is_some() {
            for win in &self.scene.windows {
                eprintln!("window {:?} {:?}", win.name, win.rect);
            }
            for i in &self.scene.items {
                eprintln!("item {:?} in {:?} {:?}", i.label, i.window, i.rect);
            }
        }
        if crop.is_empty() {
            return write_png(path, &rgba, w, h);
        }
        let mut r = self.scene.union(crop)?;
        if let Some(h) = height {
            r[3] = r[1] + h;
        }
        let s = self.scale;
        let px = |v: f32, max: u32| ((v * s).round().max(0.0) as u32).min(max);
        let (x0, y0) = (px(r[0] - pad, w), px(r[1] - pad, h));
        let (x1, y1) = (px(r[2] + pad, w), px(r[3] + pad, h));
        if x1 <= x0 || y1 <= y0 {
            anyhow::bail!("--crop: an empty rectangle");
        }
        let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
        for y in y0..y1 {
            let row = (y * w * 4) as usize;
            out.extend_from_slice(&rgba[row + (x0 * 4) as usize..row + (x1 * 4) as usize]);
        }
        write_png(path, &out, x1 - x0, y1 - y0)
    }

    fn frames(&mut self, app: &mut App, n: usize) -> Result<()> {
        for _ in 0..n {
            self.frame(app, 1.0 / 60.0)?;
        }
        Ok(())
    }

    fn aim(&mut self, target: &str) -> Result<()> {
        let (_, at) = self.scene.find(target)?;
        self.mouse = at;
        self.ctx.io_mut().add_mouse_pos_event(at);
        Ok(())
    }

    fn button(&mut self, b: dear_imgui_rs::MouseButton, down: bool) {
        self.ctx.io_mut().add_mouse_button_event(b, down);
    }

    fn step(&mut self, app: &mut App, step: &Step) -> Result<()> {
        use dear_imgui_rs::MouseButton as M;
        match step {
            Step::Hover(t) => {
                self.aim(t)?;
                // Past the tooltip delay, with the mouse at rest.
                self.frames(app, 45)?;
            }
            Step::Click(t, b) => {
                self.aim(t)?;
                self.frames(app, 2)?;
                let (button, times) = match b {
                    Button::Left => (M::Left, 1),
                    Button::Right => (M::Right, 1),
                    Button::Double => (M::Left, 2),
                };
                for _ in 0..times {
                    self.button(button, true);
                    self.frames(app, 1)?;
                    self.button(button, false);
                    self.frames(app, 1)?;
                }
                self.frames(app, 3)?;
            }
            Step::Move(t) => {
                let (_, to) = self.scene.find(t)?;
                let from = if self.mouse[0] < -1.0e3 {
                    to
                } else {
                    self.mouse
                };
                for i in 1..=12 {
                    let k = i as f32 / 12.0;
                    let p = [
                        from[0] + (to[0] - from[0]) * k,
                        from[1] + (to[1] - from[1]) * k,
                    ];
                    self.ctx.io_mut().add_mouse_pos_event(p);
                    self.frames(app, 1)?;
                }
                self.mouse = to;
                self.frames(app, 3)?;
            }
            Step::Down => {
                self.button(M::Left, true);
                self.frames(app, 2)?;
            }
            Step::Up => {
                self.button(M::Left, false);
                self.frames(app, 3)?;
            }
            Step::Type(text) => {
                self.ctx.io_mut().add_input_characters_utf8(text);
                self.frames(app, 2)?;
            }
            Step::Key(k, n) => {
                for _ in 0..*n {
                    self.ctx.io_mut().add_key_event(*k, true);
                    self.frames(app, 1)?;
                    self.ctx.io_mut().add_key_event(*k, false);
                    self.frames(app, 1)?;
                }
            }
            Step::Wait(n) => self.frames(app, *n)?,
            Step::Sleep(ms) => {
                std::thread::sleep(std::time::Duration::from_millis(*ms));
                self.frames(app, 3)?;
            }
            Step::Wheel(dy) => {
                self.ctx.io_mut().add_mouse_wheel_event([0.0, *dy]);
                self.frames(app, 3)?;
            }
        }
        Ok(())
    }
}

/// The callouts' one colour (an orange the theme never uses).
pub const CALLOUT: [f32; 4] = [1.0, 0.55, 0.12, 1.0];

/// A box round each target and a numbered badge on it, over everything.
fn draw_callouts(ui: &dear_imgui_rs::Ui, scene: &Scene, callouts: &[(String, String)]) {
    if callouts.is_empty() {
        return;
    }
    let dl = ui.get_foreground_draw_list();
    for (target, badge) in callouts {
        let Ok((r, _)) = scene.find(target) else {
            continue;
        };
        let (p0, p1) = ([r[0] + 2.0, r[1] + 2.0], [r[2] - 2.0, r[3] - 2.0]);
        dl.add_rect(p0, p1, CALLOUT)
            .thickness(3.0)
            .rounding(4.0)
            .build();
        if badge.is_empty() {
            continue;
        }
        let size = ui.calc_text_size(badge);
        let rad = (size[0].max(size[1]) / 2.0 + 6.0).max(13.0);
        // Inside a panel's top left corner; left of a small widget, or
        // at its right end when that is off the frame (the menu bar).
        let mid = (r[1] + r[3]) / 2.0;
        let c = if r[3] - r[1] > 5.0 * rad {
            [p0[0] + rad + 6.0, p0[1] + rad + 6.0]
        } else if p0[0] - 2.0 * rad - 4.0 >= 0.0 {
            [p0[0] - rad - 4.0, mid]
        } else {
            [p1[0] - rad - 6.0, mid]
        };
        dl.add_circle(c, rad, CALLOUT).filled(true).build();
        dl.add_text(
            [c[0] - size[0] / 2.0, c[1] - size[1] / 2.0],
            [0.08, 0.08, 0.1, 1.0],
            badge,
        );
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        let _ = self.ctx.end_frame();
        let _ = self.renderer.shutdown(&mut self.ctx);
    }
}

/// [`Headless::viewport`]: the viewport as shown and as rendered.
pub struct ViewportShot {
    pub shown: Vec<u8>,
    pub rendered: Vec<u8>,
    pub size: [u32; 2],
}

impl ViewportShot {
    /// Mean of each RGB channel of `rgba`, as stored (sRGB-encoded).
    pub fn mean(rgba: &[u8]) -> [f32; 3] {
        let px = rgba.as_chunks::<4>().0;
        let mut sum = [0.0f64; 3];
        for p in px {
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
        }
        sum.map(|s| (s / px.len().max(1) as f64) as f32)
    }

    /// The share of `rgba`'s pixels whose brightest channel is below
    /// `level` (sRGB-encoded): how much of a picture reads as black.
    pub fn black(rgba: &[u8], level: u8) -> f32 {
        let px = rgba.as_chunks::<4>().0;
        let n = px
            .iter()
            .filter(|p| p[..3].iter().all(|&c| c < level))
            .count();
        n as f32 / px.len().max(1) as f32
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
        let Some((_turn, mut h)) = headless() else {
            return;
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

    /// The UI Screens tool through its widgets, as a user would: pick the
    /// sign-in prompt's code page in the tree, press Up and A on the pad,
    /// hover the picture. The page steps, the picture is on screen, and the
    /// hovered widget's JSON is found. Skipped without a wgpu adapter.
    #[test]
    fn the_ui_screens_tool_steps_a_page_through_its_widgets() {
        use crate::app::{Pick, ToolKind};
        use crate::script::{Button, Step};
        let Some((_turn, mut h)) = headless() else {
            return;
        };
        let mut app = App::new();
        app.dispatch(Action::SetTool(ToolKind::UiScreens));
        h.frames(&mut app, 3).unwrap();
        let click = |t: &str| Step::Click(t.into(), Button::Left);
        for step in [
            click("signin-code"),
            click("code##ui-signin-code-code"),
            click("Up##pad-Up"),
            click("Right##pad-Right"),
            click("Up##pad-Up"),
        ] {
            h.step(&mut app, &step).unwrap();
        }
        assert_eq!(
            app.ui_screens.pick,
            Some(Pick::Native {
                screen: "signin-code".into(),
                state: "code".into()
            })
        );
        assert_eq!(
            app.ui_screens.summary().as_deref(),
            Some("Sign in as Chief | 1 [1] 0 - 0 0 0 | -")
        );
        h.step(&mut app, &click("A##pad-A")).unwrap();
        assert_eq!(
            app.ui_screens.last.as_deref(),
            Some("A: Submit(\"110000\")")
        );
        // The picture: hover the middle of the title.
        let (origin, zoom) = h.studio.ui_screens.shown.expect("the picture is shown");
        let title = {
            let shot = app.ui_screens.shot().unwrap();
            let v = shot.view.as_ref().unwrap();
            v.widgets.iter().find(|w| w.id == "title").unwrap().pixels()
        };
        let at = [
            origin[0] + (title[0] as f32 + title[2] as f32 / 2.0) * zoom,
            origin[1] + (title[1] as f32 + title[3] as f32 / 2.0) * zoom,
        ];
        h.step(&mut app, &Step::Hover(format!("{},{}", at[0], at[1])))
            .unwrap();
        let hovered = h.studio.ui_screens.hovered.clone();
        assert_eq!(hovered.as_deref(), Some("title"));
        assert!(app.ui_screens.widget_json("title").is_some());
        let (rgba, _) = h.capture(&mut app, 2).unwrap();
        let distinct: std::collections::HashSet<[u8; 4]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(97)
            .copied()
            .collect();
        assert!(distinct.len() > 8);
    }

    /// The Tags tool through its widgets: open the weapon group in the
    /// tree, pick the weapon, tick a flag and pick an enum value in the
    /// field panel. The edits land in the document, as undo steps, and
    /// the hex pane shows the element. Skipped without a wgpu adapter.
    #[test]
    fn the_tags_tool_edits_a_field_through_its_widgets() {
        use crate::app::tag_tests;
        use crate::script::{Button, Step};
        let Some((_turn, mut h)) = headless() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-editor-tags-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, ..) = tag_tests::app(&dir);
        h.frames(&mut app, 3).unwrap();
        let click = |t: &str| Step::Click(t.into(), Button::Left);
        h.step(&mut app, &click("weap  (1)")).unwrap();
        h.frames(&mut app, 2).unwrap();
        h.step(&mut app, &click("objects\\test\\blaster")).unwrap();
        h.frames(&mut app, 2).unwrap();
        assert_eq!(app.tags.selected, Some(tag_tests::weap_id()));
        h.step(&mut app, &click("quiet##b1")).unwrap();
        h.frames(&mut app, 2).unwrap();
        let flags = app.tags.read(tag_tests::WEAP + 4, 2).unwrap();
        assert_eq!(flags, [3, 0], "both flags set");
        assert!(app.tags.is_changed(tag_tests::weap_id()));
        // The block lists its elements; the hex pane follows the mouse.
        assert!(
            h.scene
                .items
                .iter()
                .any(|i| i.label.starts_with("parts  [2]"))
        );
        h.step(&mut app, &Step::Hover("loud##b0".into())).unwrap();
        h.frames(&mut app, 2).unwrap();
        assert_eq!(
            h.studio
                .tags
                .panel
                .hovered
                .as_ref()
                .map(|h| h.name.as_str()),
            Some("flags")
        );
        app.dispatch(Action::Undo);
        assert_eq!(app.tags.read(tag_tests::WEAP + 4, 2).unwrap(), [1, 0]);
        let (rgba, _) = h.capture(&mut app, 2).unwrap();
        let distinct: std::collections::HashSet<[u8; 4]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(97)
            .copied()
            .collect();
        assert!(distinct.len() > 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Maps list's Map Pack group through its widgets: with nothing
    /// installed it says how to install it; with an (invented) install it
    /// lists the map by its record's name with the picture beside it, and
    /// a click opens it in the scenario tool. The window is tall enough
    /// for the Tools pane to show the group unscrolled: Dear ImGui names a
    /// clipped row to nobody, so the probe would not find it. Skipped
    /// without a wgpu adapter.
    #[test]
    fn the_map_pack_group_lists_and_opens_a_pack_map() {
        use crate::app::map_pack::content_root;
        use crate::script::{Button, Step};
        use crate::ui::map_pack::Shown;
        let _turn = ONE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = match Headless::new([1280.0, 1600.0], 1.0) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let dir = std::env::temp_dir().join(format!("h2-editor-mp-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::new();
        app.set_data_root(dir.join("data"));
        h.frames(&mut app, 3).unwrap();
        assert_eq!(h.studio.map_pack.shown, Some(Shown::Hint));

        let pack = content_root(&dir.join("data")).join("4d53006400000101");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("ridge.map"), fixture::fixture()).unwrap();
        let e = content::map_list::Entry {
            scenario: "scenarios\\multi\\ridge\\ridge".into(),
            map_id: 1001,
            names: std::array::from_fn(|_| "Ridge".to_owned()),
            descriptions: std::array::from_fn(|_| String::new()),
            sort: 100,
            flags: 0,
            teams: [8; 16],
            preview: [0x20u8, 0x80, 0xe0, 0xbf].repeat(content::map_list::PREVIEW_LEN / 4),
        };
        std::fs::write(pack.join("patch.lvl"), e.to_v2().unwrap()).unwrap();
        app.refresh_packages();
        h.frames(&mut app, 3).unwrap();
        assert_eq!(
            h.studio.map_pack.shown,
            Some(Shown::Maps(vec![("Ridge".into(), true)]))
        );
        h.step(&mut app, &Step::Click("Ridge##pack-ridge".into(), Button::Left))
            .unwrap();
        h.frames(&mut app, 2).unwrap();
        assert_eq!(app.opened_pack_map(), Some("ridge"));
        assert!(app.session.is_open());
        let (rgba, _) = h.capture(&mut app, 2).unwrap();
        // The picture (R 0xe0, G 0x80, B 0x20 at the record's alpha 0xbf,
        // so blended a quarter with the dark panel) fills the thumbnail.
        let orange = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| (150..215).contains(&p[0]) && (80..135).contains(&p[1]) && p[2] < 70)
            .count();
        assert!(orange >= 100, "the thumbnail is drawn: {orange} pixels");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dear ImGui allows one context at a time: the tests take turns.
    static ONE_CONTEXT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn headless() -> Option<(std::sync::MutexGuard<'static, ()>, Headless)> {
        let turn = ONE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        match Headless::new([1280.0, 800.0], 1.0) {
            Ok(h) => Some((turn, h)),
            Err(e) => {
                eprintln!("skipping: {e}");
                None
            }
        }
    }

    /// The window shows the viewport's pixels as the viewport rendered
    /// them. Dear ImGui treats a texel as sRGB-encoded and an sRGB view
    /// would hand it linear values, darkening every mid-tone (a linear 0.05
    /// shows as 13 instead of 63): Coagulation's bases and Midship's hull
    /// came out black that way. Skipped when the machine has no wgpu
    /// adapter.
    #[test]
    fn the_viewport_shows_what_it_rendered() {
        let Some((_turn, mut h)) = headless() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-editor-vp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let map = dir.join("example.map");
        std::fs::write(&map, fixture::fixture()).unwrap();
        let mut app = App::new();
        app.set_data_root(dir.join("data"));
        app.dispatch(Action::OpenSource(map));
        let shot = h.viewport(&mut app, 4).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let (shown, rendered) = (
            ViewportShot::mean(&shot.shown),
            ViewportShot::mean(&shot.rendered),
        );
        assert!(rendered.iter().any(|&c| c > 20.0), "rendered {rendered:?}");
        for c in 0..3 {
            assert!(
                (shown[c] - rendered[c]).abs() < 4.0,
                "shown {shown:?}, rendered {rendered:?}"
            );
        }
    }

    /// Coagulation's base exteriors and Midship's hull, framed as the map
    /// opens (Coagulation with its first flag base selected, as in the
    /// guide's shots), are lit: little of either viewport is black, and the
    /// window shows what was rendered. Before the fix a quarter of
    /// Coagulation's viewport read as black. Needs the owner's
    /// disc (`H2_DISC`, else `disc/halo2.iso`).
    #[test]
    #[ignore = "needs the owner's disc image"]
    fn disc_coagulation_bases_and_midship_hull_are_lit() {
        let iso = std::env::var_os("H2_DISC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../disc/halo2.iso"
                ))
            });
        let Some((_turn, mut h)) = headless() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-editor-lit-{}", std::process::id()));
        for (map, select, most) in [
            ("coagulation", Some((BlockKind::Scenery, 0)), 0.02),
            ("midship", None, 0.02),
        ] {
            let mut app = App::new();
            app.set_data_root(dir.join("data"));
            app.dispatch(Action::OpenSource(iso.clone()));
            app.dispatch(Action::SelectMap(map.into()));
            if let Some(s) = select {
                app.dispatch(Action::Select(Some(s)));
            }
            let shot = h.viewport(&mut app, 4).unwrap();
            let black = ViewportShot::black(&shot.shown, 24);
            let (shown, rendered) = (
                ViewportShot::mean(&shot.shown),
                ViewportShot::mean(&shot.rendered),
            );
            eprintln!(
                "{map}: {:.1}% black, shown {shown:?}, rendered {rendered:?}",
                black * 100.0
            );
            assert!(black < most, "{map}: {:.1}% black", black * 100.0);
            for c in 0..3 {
                assert!(
                    (shown[c] - rendered[c]).abs() < 4.0,
                    "{map}: shown {shown:?}, rendered {rendered:?}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
