//! Render-model viewer.
//!
//!   viewer [--disc ISO] <map> <mode tag name>        window with orbit camera
//!   viewer [--disc ISO] --png OUT [--force] [--size WxH] [--yaw D]
//!          [--pitch D] [--shade flat|smooth|normals] <map> <tag>  screenshot
//!   viewer [--disc ISO] --all <map>...                decode every mode tag
//!   viewer [--disc ISO] --list                        disc maps and installed mods
//!
//! `<map>` may be an installed mod: `mod:<package dir>` (as `--list` shows
//! it) opens the patched map the game would load, built in memory.
//!
//! The disc defaults to `H2_DISC` or `disc/halo2.iso`. Nothing is written
//! except an explicitly requested PNG, which is never written through a
//! symlink and never replaces an existing file without `--force`.
mod disc;
mod mesh;
mod out;
mod render;
mod validate;

use anyhow::{Context, Result, bail};
use gfx::{Acquired, GfxContext, OffscreenTarget, Presenter};
use map::{FourCC, TagQuery};
use mesh::Mesh;
use render::{ModelRenderer, Orbit, Shade};
use std::path::PathBuf;
use std::sync::Arc;
use tags::model::RenderModelGeometry;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const USAGE: &str = "usage: viewer [--disc ISO] <map> <mode tag name>
       viewer [--disc ISO] --png OUT [--force] [--size WxH] [--yaw DEG] [--pitch DEG] [--shade flat|smooth|normals] [--lod N] <map> <tag>
       viewer [--disc ISO] --all <map>...   (or --all all)
       viewer [--disc ISO] --list           disc maps and installed mods
<map> is a disc map, a .map file, a Halo 1 map converted under $H2_DATA,
or an installed mod, mod:<package dir> (see --list)";

struct Args {
    disc: Option<PathBuf>,
    all: bool,
    list: bool,
    png: Option<PathBuf>,
    force: bool,
    size: (u32, u32),
    yaw: Option<f32>,
    pitch: Option<f32>,
    shade: Shade,
    lod: Option<usize>,
    frames: Option<u32>,
    positional: Vec<String>,
}

/// Largest `--size` edge; keeps the readback well inside device limits.
const MAX_EDGE: u32 = 8192;
/// Pitch is kept this far from the poles, where the Z-up look-at degenerates.
const MAX_PITCH_DEG: f32 = 89.0;

fn parse_size(v: &str) -> Result<(u32, u32)> {
    let (w, h) = v.split_once('x').context("--size WxH")?;
    let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
    if !(1..=MAX_EDGE).contains(&w) || !(1..=MAX_EDGE).contains(&h) {
        bail!("--size edges must be 1..={MAX_EDGE}, got {w}x{h}");
    }
    Ok((w, h))
}
fn parse_angle(flag: &str, v: &str) -> Result<f32> {
    let deg: f32 = v.parse()?;
    if !deg.is_finite() {
        bail!("{flag} must be a finite number of degrees");
    }
    Ok(deg)
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut a = Args {
        disc: None,
        all: false,
        list: false,
        png: None,
        force: false,
        size: (1024, 768),
        yaw: None,
        pitch: None,
        shade: Shade::Flat,
        lod: None,
        frames: None,
        positional: vec![],
    };
    let mut it = args.into_iter();
    while let Some(s) = it.next() {
        let mut next = || it.next().with_context(|| format!("{s} needs a value"));
        match s.as_str() {
            "--disc" => a.disc = Some(next()?.into()),
            "--all" => a.all = true,
            "--list" => a.list = true,
            "--png" => a.png = Some(next()?.into()),
            "--force" => a.force = true,
            "--size" => a.size = parse_size(&next()?)?,
            "--yaw" => a.yaw = Some(parse_angle("--yaw", &next()?)?.to_radians()),
            "--pitch" => {
                let p = parse_angle("--pitch", &next()?)?;
                a.pitch = Some(p.clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG).to_radians());
            }
            "--lod" => a.lod = Some(next()?.parse()?),
            "--frames" => a.frames = Some(next()?.parse()?),
            "--shade" => {
                a.shade = match next()?.as_str() {
                    "flat" => Shade::Flat,
                    "smooth" => Shade::Smooth,
                    "normals" => Shade::Normals,
                    other => bail!("unknown shade {other}"),
                }
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ => a.positional.push(s),
        }
    }
    Ok(a)
}

fn load(disc: &disc::Disc, map_name: &str, tag: &str) -> Result<RenderModelGeometry> {
    let mut map = disc.open_map(map_name)?;
    let id = map
        .tag(TagQuery::NameInGroup(tag, FourCC(*b"mode")))
        .with_context(|| format!("mode tag {tag:?} in {map_name}"))?
        .id;
    let mut raw = disc.resources(map_name)?;
    Ok(RenderModelGeometry::read(&mut map, id, &mut raw)?)
}

fn sections_for(model: &RenderModelGeometry, lod: Option<usize>) -> Vec<usize> {
    let Some(lod) = lod else {
        return model.default_sections();
    };
    let mut out = Vec::new();
    for r in &model.regions {
        if let Some(s) = r
            .permutations
            .first()
            .and_then(|p| p.lod_sections[..=lod.min(5)].iter().rev().flatten().next())
            && !out.contains(s)
        {
            out.push(*s);
        }
    }
    out
}

/// A mod's `--list` line: what to pass, its name, its base and `MapId`.
fn list_line(m: &disc::InstalledMod) -> String {
    format!(
        "{}{}  {} (on {}, map_id {})",
        disc::MOD_PREFIX,
        m.dir,
        m.name,
        m.base,
        &m.map_id[..16.min(m.map_id.len())]
    )
}

fn describe(model: &RenderModelGeometry, mesh: &Mesh) {
    println!(
        "{}: {} regions, {} sections, {} nodes, {} materials; drawing {} sections, {} parts, {} triangles",
        model.name.as_deref().unwrap_or("?"),
        model.regions.len(),
        model.sections.len(),
        model.nodes.len(),
        model.materials.len(),
        mesh.sections,
        mesh.parts,
        mesh.indices.len() / 3
    );
    println!(
        "bounds min {:?} max {:?}",
        mesh.min.map(|v| (v * 1000.0).round() / 1000.0),
        mesh.max.map(|v| (v * 1000.0).round() / 1000.0)
    );
}

fn main() -> Result<()> {
    let args = parse(std::env::args().skip(1))?;
    let disc = disc::Disc::open(&disc::disc_path(args.disc.clone())?)?;
    if args.list {
        for name in disc.map_names() {
            println!("{name}");
        }
        for m in disc::installed_mods(&disc::data_root()) {
            println!("{}", list_line(&m));
        }
        return Ok(());
    }
    if args.all {
        let maps = if args.positional.iter().any(|m| m == "all") {
            disc.map_names()
        } else {
            args.positional.clone()
        };
        if maps.is_empty() {
            bail!("{USAGE}");
        }
        let ok = validate::run(&disc, &maps)?;
        if !ok {
            std::process::exit(1);
        }
        return Ok(());
    }
    let [map_name, tag] = args.positional.as_slice() else {
        bail!("{USAGE}")
    };
    let model = load(&disc, map_name, tag)?;
    let mesh = Mesh::build(&model, &sections_for(&model, args.lod))?;
    describe(&model, &mesh);
    let mut orbit = Orbit::frame(&mesh);
    if let Some(y) = args.yaw {
        orbit.yaw = y;
    }
    if let Some(p) = args.pitch {
        orbit.pitch = p;
    }
    if let Some(out) = &args.png {
        let ctx = GfxContext::headless()?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = OffscreenTarget::new(&ctx.device, args.size, format);
        let r = ModelRenderer::new(&ctx, format, &mesh);
        r.draw(&ctx, &target.target(), &orbit, args.shade, false);
        let pixels = target.read_pixels(&ctx);
        out::write_with(out, args.force, |w| {
            assets::png::write_rgba(w, args.size.0, args.size.1, &pixels)
        })
        .with_context(|| format!("write {}", out.display()))?;
        println!("wrote {}", out.display());
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        title: format!("h2 viewer: {tag}"),
        mesh,
        orbit,
        shade: args.shade,
        wireframe: false,
        drag: None,
        cursor: (0.0, 0.0),
        frames_left: args.frames,
        gpu: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct Gpu {
    window: Arc<Window>,
    ctx: GfxContext,
    presenter: Presenter,
    renderer: ModelRenderer,
}
struct App {
    title: String,
    mesh: Mesh,
    orbit: Orbit,
    shade: Shade,
    wireframe: bool,
    drag: Option<MouseButton>,
    cursor: (f64, f64),
    frames_left: Option<u32>,
    gpu: Option<Gpu>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title(&self.title)
                    .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0)),
            )
            .expect("create window"),
        );
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("surface");
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface))).expect("gpu");
        let presenter =
            Presenter::new(&ctx, surface, (size.width, size.height)).expect("configure");
        let renderer = ModelRenderer::new(&ctx, presenter.format(), &self.mesh);
        println!(
            "drag: orbit (left) / pan (right); wheel: zoom; S: shading; W: wireframe; R: reset; Esc: quit"
        );
        window.request_redraw();
        self.gpu = Some(Gpu {
            window,
            ctx,
            presenter,
            renderer,
        });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                gpu.presenter.resize(&gpu.ctx, (s.width, s.height));
                gpu.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.drag = (state == ElementState::Pressed).then_some(button);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (dx, dy) = (position.x - self.cursor.0, position.y - self.cursor.1);
                self.cursor = (position.x, position.y);
                match self.drag {
                    Some(MouseButton::Left) => {
                        self.orbit.yaw -= dx as f32 * 0.01;
                        self.orbit.pitch = (self.orbit.pitch + dy as f32 * 0.01).clamp(-1.55, 1.55);
                    }
                    Some(MouseButton::Right) => {
                        let s = self.orbit.distance * 0.0015;
                        let (sy, cy) = self.orbit.yaw.sin_cos();
                        // Pan in the camera's horizontal plane and along Z.
                        self.orbit.target[0] += (sy * dx as f32) * s;
                        self.orbit.target[1] -= (cy * dx as f32) * s;
                        self.orbit.target[2] += dy as f32 * s;
                    }
                    _ => return,
                }
                gpu.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.05,
                };
                self.orbit.distance *= (1.0 - d * 0.1).clamp(0.5, 1.5);
                gpu.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Character("s") => {
                        self.shade = match self.shade {
                            Shade::Flat => Shade::Smooth,
                            Shade::Smooth => Shade::Normals,
                            Shade::Normals => Shade::Flat,
                        }
                    }
                    Key::Character("w") => self.wireframe = !self.wireframe,
                    Key::Character("r") => self.orbit = Orbit::frame(&self.mesh),
                    _ => return,
                }
                gpu.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                match gpu.presenter.acquire(&gpu.ctx) {
                    Ok(Acquired::Frame(frame)) => {
                        let target = gpu.presenter.target(&frame);
                        gpu.renderer.draw(
                            &gpu.ctx,
                            &target,
                            &self.orbit,
                            self.shade,
                            self.wireframe,
                        );
                        gpu.window.pre_present_notify();
                        gpu.presenter.present(&gpu.ctx, frame);
                    }
                    Ok(Acquired::Skip) => gpu.window.request_redraw(),
                    Err(e) => eprintln!("frame error: {e}"),
                }
                if let Some(n) = self.frames_left.as_mut() {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        el.exit();
                    } else {
                        gpu.window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(v: &[&str]) -> Result<Args> {
        parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn size_rejects_zero_huge_and_malformed() {
        for bad in [
            "0x10",
            "10x0",
            "8193x10",
            "10x99999999999",
            "10",
            "axb",
            "-1x5",
        ] {
            assert!(args(&["--size", bad]).is_err(), "{bad}");
        }
        assert_eq!(args(&["--size", "1x8192"]).unwrap().size, (1, 8192));
    }

    #[test]
    fn pitch_is_clamped_and_angles_must_be_finite() {
        let p = args(&["--pitch", "90"]).unwrap().pitch.unwrap();
        assert!((p - 89f32.to_radians()).abs() < 1e-6);
        let p = args(&["--pitch", "-1000"]).unwrap().pitch.unwrap();
        assert!((p + 89f32.to_radians()).abs() < 1e-6);
        for bad in ["NaN", "inf", "-inf"] {
            assert!(args(&["--pitch", bad]).is_err());
            assert!(args(&["--yaw", bad]).is_err());
        }
    }

    #[test]
    fn mods_are_listed_by_what_opens_them() {
        assert!(args(&["--list"]).unwrap().list);
        let m = disc::InstalledMod {
            dir: "lockout-hogs".into(),
            name: "Lockout Hogs".into(),
            base: "lockout".into(),
            map_id: "ab".repeat(32),
        };
        assert_eq!(
            list_line(&m),
            "mod:lockout-hogs  Lockout Hogs (on lockout, map_id abababababababab)"
        );
    }

    #[test]
    fn force_is_opt_in() {
        assert!(!args(&["--png", "x.png"]).unwrap().force);
        assert!(args(&["--png", "x.png", "--force"]).unwrap().force);
    }
}
