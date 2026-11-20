//! h2-editor: the Dear ImGui editor suite. See crates/apps/studio/README.md.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use dear_imgui_rs::Context;
use dear_imgui_wgpu::{FramebufferExtent, GammaMode, WgpuInitInfo, WgpuRenderer};
use dear_imgui_winit::{HiDpiMode, WinitPlatform};
use studio::app::{Action, App, ToolKind};
use studio::ui::Studio;
use studio::viewport::Gpu;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const USAGE: &str = "usage: h2-editor call <call> [args] | h2-editor --serve [--socket PATH] [...]
       h2-editor [--socket PATH | --no-listen] [--disc <iso> | --map <file.map> | --folder <maps dir>] \
[--halo1 <iso|maps dir>] [--mappack <iso|folder>] [--data <dir>] [--package <dir>[#H1|#H2]] [--halo1-map <map> [--convert h2|h1]] [--overlay on|off] [--tool scenario|tags|variants|playlists|ui-screens|hud] [--tags-map <map>] [--tag <group>:<path>] [--ui-screen <screen>/<state> [--ui-press up,a,...]] \
[--hud-map <map>|--hud-package <key>] [--hud <nhdt name>] [--hud-widget <name>] [--pick <map>] [--select <block>:<index>] [--hopper <file.dat>|default] \
[--variant <built-in index>] [--copy-variant <name>] [--playlist <1..16>] [--preset <n>] \
[--focus] [--camera <yaw>,<pitch>,<distance>] [--dialog unsaved|package] \
[--no-disc] [--nudge <x>,<y>,<z>] [--page <variant settings page>] [--screenshot <out.png> [--size <w>x<h>] [--scale <s>] [--do <steps>] \
[--crop <target>]... [--pad <points>] [--crop-height <points>] [--callout <target>=<badge>]...]
--serve: the agent API with no window (docs/editor/agent-api.md); the window listens too, \
on --socket, else $H2_STUDIO, else the cache directory's studio.sock (--no-listen: not at all). \
`h2-editor call` (or `h2 studio`) is the client.
--do steps (separated by ;): hover|click|rclick|dclick|move <target>, down, up, \
type <text>, key <name>[*n], wait <frames>, sleep <ms>, wheel <dy>. A target: x,y | win:<name> | \
[<window>>]<label>[#n], each optionally @dx,dy from its top left (see studio::script).";

struct Args {
    source: Option<PathBuf>,
    halo1: Option<PathBuf>,
    /// The Halo 2 Map Pack (`--mappack`), else found beside the disc.
    mappack: Option<PathBuf>,
    halo1_map: Option<String>,
    /// Convert the shown Halo 1 map (waiting for it) and open the result.
    convert: Option<studio::app::ConvertSandbox>,
    overlay: Option<bool>,
    package: Option<String>,
    data: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    size: [f32; 2],
    scale: f32,
    pick: Option<String>,
    tool: Option<ToolKind>,
    /// The UI Screens tool's screen and state, and presses on it.
    ui_screen: Option<String>,
    ui_press: Option<String>,
    /// The HUD tool's map (or package), HUD and selected widget.
    hud_map: Option<String>,
    hud_package: Option<String>,
    hud: Option<String>,
    hud_widget: Option<String>,
    /// The Tags tool's tag, `group:path`.
    tag: Option<String>,
    /// Another map of the source browsed in the Tags tool (`shared`).
    tags_map: Option<String>,
    hopper: Option<String>,
    variant: Option<usize>,
    playlist: Option<usize>,
    select: Option<String>,
    copy_variant: Option<String>,
    preset: Option<usize>,
    focus: bool,
    /// Yaw and pitch in degrees, distance in world units.
    camera: Option<[f32; 3]>,
    dialog: Option<String>,
    /// Open no disc and no Halo 1 source (the first run).
    no_disc: bool,
    /// Move the selection by this much, as an edit.
    nudge: Option<[f32; 3]>,
    /// The variant settings page to show alone.
    page: Option<String>,
    steps: Vec<studio::script::Step>,
    crop: Vec<String>,
    pad: f32,
    crop_height: Option<f32>,
    callouts: Vec<(String, String)>,
    /// The agent API with no window.
    serve: bool,
    /// Where the agent API listens.
    socket: Option<PathBuf>,
    /// The window does not listen.
    no_listen: bool,
}

fn args() -> Result<Args> {
    let mut a = Args {
        source: None,
        halo1: None,
        mappack: None,
        halo1_map: None,
        convert: None,
        overlay: None,
        package: None,
        data: None,
        screenshot: None,
        size: [1600.0, 1000.0],
        scale: 2.0,
        pick: None,
        tool: None,
        ui_screen: None,
        ui_press: None,
        hud_map: None,
        hud_package: None,
        hud: None,
        hud_widget: None,
        tag: None,
        tags_map: None,
        hopper: None,
        variant: None,
        playlist: None,
        select: None,
        copy_variant: None,
        preset: None,
        focus: false,
        camera: None,
        dialog: None,
        no_disc: false,
        nudge: None,
        page: None,
        steps: Vec::new(),
        crop: Vec::new(),
        pad: 8.0,
        crop_height: None,
        callouts: Vec::new(),
        serve: false,
        socket: None,
        no_listen: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| anyhow!("{flag} needs a value\n{USAGE}"))
        };
        match flag.as_str() {
            "--disc" | "--map" | "--folder" => a.source = Some(value()?.into()),
            "--data" => a.data = Some(value()?.into()),
            "--halo1" => a.halo1 = Some(value()?.into()),
            "--mappack" => a.mappack = Some(value()?.into()),
            "--halo1-map" => a.halo1_map = Some(value()?),
            "--convert" => {
                a.convert = Some(match value()?.as_str() {
                    "h2" => studio::app::ConvertSandbox::Halo2,
                    "h1" => studio::app::ConvertSandbox::Halo1,
                    other => bail!("--convert wants h2 or h1, not {other:?}"),
                })
            }
            "--overlay" => {
                a.overlay = Some(match value()?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => bail!("--overlay wants on or off, not {other:?}"),
                })
            }
            "--package" => a.package = Some(value()?),
            "--screenshot" => a.screenshot = Some(value()?.into()),
            "--pick" => a.pick = Some(value()?),
            "--ui-screen" => a.ui_screen = Some(value()?),
            "--ui-press" => a.ui_press = Some(value()?),
            "--hud-map" => a.hud_map = Some(value()?),
            "--hud-package" => a.hud_package = Some(value()?),
            "--hud" => a.hud = Some(value()?),
            "--hud-widget" => a.hud_widget = Some(value()?),
            "--tag" => a.tag = Some(value()?),
            "--tags-map" => a.tags_map = Some(value()?),
            "--tool" => {
                a.tool = Some(match value()?.as_str() {
                    "scenario" => ToolKind::Scenario,
                    "variants" => ToolKind::Variants,
                    "playlists" => ToolKind::Playlists,
                    "ui-screens" => ToolKind::UiScreens,
                    "hud" => ToolKind::Hud,
                    "tags" => ToolKind::Tags,
                    t => bail!("unknown tool {t}\n{USAGE}"),
                })
            }
            "--hopper" => a.hopper = Some(value()?),
            "--variant" => a.variant = Some(value()?.parse()?),
            "--playlist" => a.playlist = Some(value()?.parse::<usize>()?.clamp(1, 16) - 1),
            "--select" => a.select = Some(value()?),
            "--copy-variant" => a.copy_variant = Some(value()?),
            "--preset" => a.preset = Some(value()?.parse()?),
            "--focus" => a.focus = true,
            "--camera" => {
                let v = value()?;
                let n: Vec<f32> = v
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| anyhow!("--camera wants <yaw>,<pitch>,<distance>"))?;
                let [yaw, pitch, distance] = n[..] else {
                    bail!("--camera wants <yaw>,<pitch>,<distance>");
                };
                a.camera = Some([yaw, pitch, distance]);
            }
            "--dialog" => a.dialog = Some(value()?),
            "--no-disc" => a.no_disc = true,
            "--page" => a.page = Some(value()?),
            "--nudge" => {
                let v = value()?;
                let n: Vec<f32> = v
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .map_err(|_| anyhow!("--nudge wants <x>,<y>,<z>"))?;
                let [x, y, z] = n[..] else {
                    bail!("--nudge wants <x>,<y>,<z>");
                };
                a.nudge = Some([x, y, z]);
            }
            "--do" => a.steps.extend(studio::script::parse(&value()?)?),
            "--crop" => a.crop.push(value()?),
            "--pad" => a.pad = value()?.parse()?,
            "--crop-height" => a.crop_height = Some(value()?.parse()?),
            "--callout" => {
                let v = value()?;
                let (target, badge) = v
                    .rsplit_once('=')
                    .ok_or_else(|| anyhow!("--callout wants <target>=<badge>"))?;
                a.callouts.push((target.to_owned(), badge.to_owned()));
            }
            "--serve" => a.serve = true,
            "--socket" => a.socket = Some(value()?.into()),
            "--no-listen" => a.no_listen = true,
            "--scale" => a.scale = value()?.parse()?,
            "--size" => {
                let v = value()?;
                let (w, h) = v
                    .split_once('x')
                    .ok_or_else(|| anyhow!("--size wants <w>x<h>"))?;
                a.size = [w.parse()?, h.parse()?];
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => bail!("unknown argument {flag}\n{USAGE}"),
        }
    }
    Ok(a)
}

/// `<block>:<index>`, the block by its name without case or spaces
/// (`vehicles:0`, `netgameflags:3`).
fn parse_selection(v: &str) -> Result<(scenario_edit::BlockKind, usize)> {
    let (block, index) = v
        .split_once(':')
        .ok_or_else(|| anyhow!("--select wants <block>:<index>"))?;
    let want = block.replace([' ', '_'], "").to_ascii_lowercase();
    let kind = scenario_edit::layout::BLOCKS
        .iter()
        .map(|b| b.kind)
        .find(|k| format!("{k:?}").to_ascii_lowercase() == want)
        .ok_or_else(|| anyhow!("--select: no block {block:?}"))?;
    Ok((kind, index.parse()?))
}

/// Moves the selected placement by `d`, as an edit.
fn nudge_selection(app: &mut App, d: [f32; 3]) {
    let (Some((kind, index)), Some(t)) = (app.session.selected(), app.session.selected_transform())
    else {
        return;
    };
    let [x, y, z] = t.position;
    let edit = scenario_edit::Command::set(
        kind,
        index,
        "position",
        scenario_edit::Value::Vec3([x + d[0], y + d[1], z + d[2]]),
    );
    if let Err(e) = app.session.apply(edit) {
        app.error(e.to_string());
    }
}

/// Dear ImGui's errors as anyhow errors (they need not be `Send`).
fn imgui_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

fn ini_path() -> PathBuf {
    content::store::default_data_root().join("h2-editor.ini")
}

fn main() -> Result<()> {
    // The agent API's client: `h2-editor call <call> [args]`.
    if std::env::args().nth(1).as_deref() == Some("call") {
        return studio_proto::cli::run("h2-editor call", std::env::args().skip(2).collect());
    }
    let args = args()?;
    let mut app = App::new();
    if let Some(d) = &args.data {
        app.set_data_root(d.clone());
    }
    app.refresh_packages();
    let disc = editor_core::default_disc(args.source.clone()).filter(|_| !args.no_disc);
    // The Map Pack: --mappack, else $H2_MAPPACK_ISO, else mappack.iso beside
    // the disc just found, else disc/mappack.iso. Opened after the disc, so
    // its maps resolve their shared caches from it.
    let mappack = editor_core::mappack::default_source(args.mappack.clone(), disc.as_deref())
        .filter(|_| !args.no_disc);
    if let Some(src) = disc {
        app.dispatch(Action::OpenSource(src));
    }
    if let Some(src) = mappack {
        app.dispatch(Action::OpenSource(src));
    }
    // Halo 1 maps: --halo1, else $H1_ISO, $H1_MAP_DIR or disc/halo1.iso.
    if let Some(src) = args
        .halo1
        .clone()
        .or_else(editor_core::halo1::default_source)
        .filter(|_| !args.no_disc)
    {
        app.dispatch(Action::OpenSource(src));
    }
    if let Some(name) = &args.pick {
        app.dispatch(Action::SelectMap(name.clone()));
    }
    if let Some(key) = &args.package {
        app.dispatch(Action::OpenPackageEntry(key.clone()));
    }
    if let Some(name) = &args.halo1_map {
        app.dispatch(Action::SelectHalo1Map(name.clone()));
    }
    if let Some(sandbox) = args.convert {
        app.dispatch(Action::ConvertHalo1(sandbox));
        app.wait_convert();
    }
    if let Some(on) = args.overlay {
        app.dispatch(Action::SetOverlay(on));
    }
    match args.hopper.as_deref() {
        Some("default") => app.dispatch(Action::NewDefaultHopper),
        Some(path) => app.dispatch(Action::OpenHopper(path.into())),
        None => {}
    }
    if let Some(tool) = args.tool {
        app.dispatch(Action::SetTool(tool));
    }
    if let Some(pick) = &args.ui_screen {
        let (screen, state) = pick.split_once('/').unwrap_or((pick, ""));
        let state = match state {
            "" => studio::app::UiScreens::tree()
                .into_iter()
                .find(|(n, _)| *n == screen)
                .and_then(|(_, s)| s.into_iter().next())
                .unwrap_or_default(),
            s => s.to_owned(),
        };
        app.dispatch(Action::SetTool(ToolKind::UiScreens));
        app.dispatch(Action::UiPick(studio::app::Pick::Native {
            screen: screen.to_owned(),
            state,
        }));
        for p in args.ui_press.iter().flat_map(|p| p.split(',')) {
            match boot::ui_kit::input::Input::named(p.trim()) {
                Some(i) => app.dispatch(Action::UiPress(i)),
                None => bail!("--ui-press: no input {p:?}"),
            }
        }
    }
    if args.hud_map.is_some() || args.hud_package.is_some() {
        app.dispatch(Action::SetTool(ToolKind::Hud));
        if let Some(m) = &args.hud_map {
            app.dispatch(Action::HudOpenMap(m.clone()));
        }
        if let Some(k) = &args.hud_package {
            app.dispatch(Action::HudOpenPackage(k.clone()));
        }
        let session = app.hud.session.as_ref();
        if let Some(name) = &args.hud {
            let id = session
                .and_then(|s| s.huds.iter().find(|e| e.name == *name || e.name.ends_with(&format!("\\{name}"))))
                .map(|e| e.id)
                .ok_or_else(|| anyhow!("--hud: no HUD {name:?}"))?;
            app.dispatch(Action::HudPick(id));
        }
        if let Some(name) = &args.hud_widget {
            let key = app
                .hud
                .session
                .as_ref()
                .and_then(|s| s.hud())
                .and_then(|h| h.widgets.iter().find(|w| w.name == *name))
                .map(|w| w.key)
                .ok_or_else(|| anyhow!("--hud-widget: no widget {name:?}"))?;
            app.dispatch(Action::HudSelect(Some(key)));
        }
    }
    if let Some(m) = &args.tags_map {
        app.dispatch(Action::SetTool(ToolKind::Tags));
        app.dispatch(Action::BrowseTags(Some(m.clone())));
        if app.browsing().is_none() {
            bail!("--tags-map: {m} could not be browsed (the log says why)");
        }
    }
    if let Some(t) = &args.tag {
        app.dispatch(Action::SetTool(ToolKind::Tags));
        match studio::app::tag_api::find_tag(app.shown_tags(), t) {
            Some(id) => app.dispatch(Action::Tag(studio::app::TagAction::Select(id))),
            None => bail!("--tag: no tag {t:?} in the map shown"),
        }
    }
    if let Some(i) = args.variant {
        app.dispatch(Action::OpenVariant(variants::panel::Origin::Builtin(i)));
    }
    if let Some(i) = args.playlist {
        app.hopper_selected = i;
    }
    if let Some(sel) = &args.select {
        app.dispatch(Action::Select(Some(parse_selection(sel)?)));
    }
    if let Some(name) = &args.copy_variant {
        app.dispatch(Action::CopyBuiltin(name.clone()));
    }
    if let Some(i) = args.preset {
        app.hopper_preset = Some(i);
    }
    if args.focus {
        app.dispatch(Action::FocusSelected);
    }
    if let Some([yaw, pitch, distance]) = args.camera {
        let c = app.session.camera_mut();
        c.yaw = yaw.to_radians();
        c.pitch = pitch.to_radians();
        c.distance = distance;
    }
    if let Some(d) = args.nudge {
        nudge_selection(&mut app, d);
    }
    match args.dialog.as_deref() {
        None => {}
        Some("package") => app.open_package_form(),
        // An unsaved edit (the selection nudged, or a variant copy) and
        // Quit: the unsaved-changes prompt.
        Some("unsaved") => {
            nudge_selection(&mut app, [0.5, 0.0, 0.0]);
            app.dispatch(Action::Quit);
        }
        Some(d) => bail!("unknown dialog {d}\n{USAGE}"),
    }
    if let Some(out) = &args.screenshot {
        // The Maps list's names and pictures, so a shot shows them.
        app.wait_cards();
        let mut h = studio::headless::Headless::new(args.size, args.scale)?;
        studio::probe::want(studio::script::labels(
            studio::script::targets(&args.steps)
                .chain(args.crop.iter().map(String::as_str))
                .chain(args.callouts.iter().map(|(t, _)| t.as_str())),
        ));
        h.callouts = args.callouts.clone();
        h.studio.variants.only_page = args.page.clone();
        h.scripted(
            &mut app,
            6,
            &args.steps,
            &args.crop,
            args.pad,
            args.crop_height,
            out,
        )?;
        println!("wrote {}", out.display());
        return Ok(());
    }
    let socket = args
        .socket
        .clone()
        .unwrap_or_else(studio_proto::default_socket);
    if args.serve {
        return serve(app, &socket);
    }
    // The window listens too, so an agent drives what the user sees.
    let listening = if args.no_listen {
        None
    } else {
        match studio::api::server::listen(&socket) {
            Ok(l) => {
                app.info(format!("agent API on {}", socket.display()));
                Some(l)
            }
            Err(e) => {
                app.info(format!("agent API off: {e}"));
                None
            }
        }
    };
    let mut api = studio::api::Api::new();
    api.window = true;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = Runner {
        app,
        win: None,
        api,
        listening,
    };
    event_loop.run_app(&mut runner)?;
    Ok(())
}

/// `h2-editor --serve`: the agent API with no window, until killed.
fn serve(mut app: App, socket: &std::path::Path) -> Result<()> {
    let (_listener, rx) = studio::api::server::listen(socket)
        .map_err(|e| anyhow!("listening on {}: {e}", socket.display()))?;
    eprintln!("h2-editor: agent API on {} (no window)", socket.display());
    let mut api = studio::api::Api::new();
    for inc in rx.iter() {
        let r = api.handle(&mut app, inc.request.clone());
        inc.answer(r);
    }
    Ok(())
}

struct Runner {
    app: App,
    win: Option<Win>,
    api: studio::api::Api,
    /// The agent API's socket and its queue (none when it could not bind).
    listening: Option<(
        studio::api::server::Listener,
        std::sync::mpsc::Receiver<studio::api::server::Incoming>,
    )>,
}

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    platform: WinitPlatform,
    renderer: WgpuRenderer,
    studio: Studio,
    title: String,
    // Declared last: dropped after the platform and renderer.
    ctx: Context,
}

impl Win {
    fn new(event_loop: &ActiveEventLoop, title: String) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(&title)
                    .with_inner_size(LogicalSize::new(1600.0, 1000.0)),
            )?,
        );
        // The editor runs unbundled: give its Dock tile the app icon.
        app_icon::set_dock_icon();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        let caps = surface.get_capabilities(&adapter);
        let format = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ]
        .into_iter()
        .find(|f| caps.formats.contains(f))
        .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (mut ctx, fonts) = studio::context(Some(ini_path()));
        let mut platform = WinitPlatform::new(&mut ctx).map_err(imgui_err)?;
        platform
            .attach_window(window.clone(), HiDpiMode::Default, &mut ctx)
            .map_err(imgui_err)?;
        let mut renderer = WgpuRenderer::new(
            WgpuInitInfo::new(device.clone(), queue.clone(), format),
            &mut ctx,
        )
        .map_err(imgui_err)?;
        renderer.set_gamma_mode(GammaMode::Auto);
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            platform,
            renderer,
            studio: Studio::new(fonts),
            title,
            ctx,
        })
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn redraw(&mut self, app: &mut App) -> Result<()> {
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                bail!("surface acquisition failed with a wgpu validation error")
            }
        };
        self.platform
            .prepare_frame(&mut self.ctx, &self.window)
            .map_err(imgui_err)?;
        let ui = self.ctx.frame();
        let mut gpu = Gpu {
            device: &self.device,
            queue: &self.queue,
            renderer: &mut self.renderer,
        };
        self.studio.frame(ui, app, &mut gpu);
        self.platform
            .prepare_render(ui, &self.window)
            .map_err(imgui_err)?;

        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("h2-editor"),
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
            let consumer = self.renderer.renderer_consumer().map_err(imgui_err)?;
            let pending = self.ctx.try_render(consumer).map_err(imgui_err)?;
            self.renderer
                .render(
                    pending,
                    &mut pass,
                    FramebufferExtent::from_texture(&frame.texture),
                )
                .map_err(imgui_err)?;
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        if reconfigure {
            self.surface.configure(&self.device, &self.config);
        }
        let title = app.title();
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.ctx.save_ini_settings_to_disk(ini_path());
        let _ = self.ctx.end_frame();
        if let Err(e) = self.renderer.shutdown(&mut self.ctx) {
            eprintln!("renderer shutdown: {e}");
        }
        if let Err(e) = self.platform.shutdown(&mut self.ctx) {
            eprintln!("platform shutdown: {e}");
        }
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_none() {
            match Win::new(event_loop, self.app.title()) {
                Ok(w) => self.win = Some(w),
                Err(e) => {
                    eprintln!("h2-editor: {e:#}");
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(win) = self.win.as_mut() else {
            return;
        };
        if let Err(e) = win.platform.handle_window_event(&mut win.ctx, id, &event) {
            eprintln!("h2-editor: {e}");
            event_loop.exit();
            return;
        }
        match event {
            WindowEvent::Resized(size) => win.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = win.window.inner_size();
                win.resize(size.width, size.height);
            }
            WindowEvent::CloseRequested => self.app.dispatch(Action::Quit),
            WindowEvent::DroppedFile(path) => self.app.dispatch(Action::OpenSource(path)),
            WindowEvent::RedrawRequested => {
                if let Err(e) = win.redraw(&mut self.app) {
                    eprintln!("h2-editor: {e:#}");
                }
            }
            _ => {}
        }
        if self.app.should_quit() {
            win.shutdown();
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        // Agent calls run between frames, on the app the window draws.
        if let Some((_, rx)) = &self.listening {
            self.api.pump(&mut self.app, rx);
        }
        if let Some(w) = &self.win {
            w.window.request_redraw();
        }
    }
}
