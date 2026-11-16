//! The user guide's screenshots (docs/editor-guide.md), drawn without a
//! window: the real shell and tools, over made-up data, rendered offscreen
//! with egui-wgpu and written as PNGs to `docs/images/editor/`. Ignored, so
//! `cargo test` never rewrites them; regenerate with
//!
//! ```sh
//! cargo test -p editor screenshots -- --ignored
//! ```
//!
//! Nothing here comes from the game: the map is scenario-edit's invented
//! fixture (no level geometry, so the viewport shows only markers), and the
//! settings schema, variants, playlists and map names are made up here.
//! Widgets are found by their AccessKit labels and clicked with pointer
//! events, as a user would.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use egui_wgpu::wgpu;
use live_proto::hopper::{
    BLOCK, DESCRIPTOR, HOPPERS, Hopper, HopperFile, HopperText, RECORD, VARIANT_AT,
};
use scenario_edit::{BlockKind, Command, Value};
use variants::save::{CONTENT_TYPES, SaveStore, XbeNames};
use variants::schema::{Builtin, Choice, Page, SettingDef};
use variants::{GameType, Schema, SettingId, VariantEditor};

use crate::dialog::ScriptedDialog;
use crate::hopper::HopperTool;
use crate::hopper::catalog::{Catalog, CatalogMap};
use crate::scenario::ScenarioTool;
use crate::shell::{Answer, Shell};
use crate::source::MapSource;
use crate::tool::{Handoff, Tool, ToolContext, ToolError};
use crate::variants::VariantTool;

const SIZE: [u32; 2] = [1280, 800];
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The example map, relative to the scratch directory the test runs in, so
/// the path the window shows is short and the same on every machine.
const MAP: &str = "disc/example.map";

/// A tool the shell owns while the test keeps a handle to it.
struct Shared<T> {
    tool: Rc<RefCell<T>>,
    title: &'static str,
    exts: &'static [&'static str],
}

impl<T: Tool> Tool for Shared<T> {
    fn title(&self) -> &str {
        self.title
    }
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>) {
        self.tool.borrow_mut().ui(ui, cx);
    }
    fn is_dirty(&self) -> bool {
        self.tool.borrow().is_dirty()
    }
    fn extensions(&self) -> &[&str] {
        self.exts
    }
    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        self.tool.borrow_mut().open(path, cx)
    }
    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        self.tool.borrow_mut().save(cx)
    }
    fn save_as(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        self.tool.borrow_mut().save_as(path, cx)
    }
    fn can_undo(&self) -> bool {
        self.tool.borrow().can_undo()
    }
    fn can_redo(&self) -> bool {
        self.tool.borrow().can_redo()
    }
    fn undo(&mut self) -> bool {
        self.tool.borrow_mut().undo()
    }
    fn redo(&mut self) -> bool {
        self.tool.borrow_mut().redo()
    }
    fn take_handoff(&mut self) -> Option<Handoff> {
        self.tool.borrow_mut().take_handoff()
    }
    fn receive(&mut self, work: Handoff, cx: &mut ToolContext<'_>) -> Result<(), Handoff> {
        self.tool.borrow_mut().receive(work, cx)
    }
}

fn shared<T: Tool + 'static>(
    tool: T,
    title: &'static str,
    exts: &'static [&'static str],
) -> (Rc<RefCell<T>>, Box<dyn Tool>) {
    let tool = Rc::new(RefCell::new(tool));
    let boxed = Box::new(Shared {
        tool: tool.clone(),
        title,
        exts,
    });
    (tool, boxed)
}

/// The window, drawn frame by frame into an offscreen texture.
struct Harness {
    ctx: egui::Context,
    shell: Shell,
    render: egui_wgpu::RenderState,
    time: f64,
    events: Vec<egui::Event>,
    /// The last frame's AccessKit nodes: (label or value, bounds, role).
    nodes: Vec<(String, egui::Rect, egui::accesskit::Role)>,
}

fn render_state() -> Option<egui_wgpu::RenderState> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let renderer = egui_wgpu::Renderer::new(&device, FORMAT, Default::default());
    Some(egui_wgpu::RenderState {
        adapter,
        available_adapters: Vec::new(),
        instance,
        device,
        queue,
        target_format: FORMAT,
        renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
    })
}

impl Harness {
    fn new(mut shell: Shell, render: egui_wgpu::RenderState) -> Self {
        shell.set_render_state(Some(render.clone()));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Harness {
            ctx,
            shell,
            render,
            time: 0.0,
            events: Vec::new(),
            nodes: Vec::new(),
        }
    }

    /// Runs one frame and uploads its textures.
    fn frame(&mut self) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE[0] as f32, SIZE[1] as f32),
            )),
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let shell = &mut self.shell;
        let mut out = self.ctx.run_ui(input, |ui| shell.ui(ui));
        // Long steps, so every animation (headers opening) has finished,
        // except between a press and its release, which must be quick to
        // count as a click.
        let pressed = self.ctx.input(|i| i.pointer.any_down());
        self.time += if pressed { 0.05 } else { 1.0 };
        let mut r = self.render.renderer.write();
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                r.update_texture(&self.render.device, &self.render.queue, *id, delta);
            }
        }
        for id in &out.textures_delta.free {
            r.free_texture(id);
        }
        drop(r);
        out.textures_delta.clear();
        if let Some(tree) = &out.platform_output.accesskit_update {
            self.nodes = tree
                .nodes
                .iter()
                .filter_map(|(_, n)| {
                    let text = n.label().or(n.value())?.to_owned();
                    let b = n.bounds()?;
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(b.x0 as f32, b.y0 as f32),
                        egui::pos2(b.x1 as f32, b.y1 as f32),
                    );
                    Some((text, rect, n.role()))
                })
                .collect();
        }
        out
    }

    fn frames(&mut self, n: usize) -> egui::FullOutput {
        for _ in 1..n {
            self.frame();
        }
        self.frame()
    }

    /// The widget whose label (or value) is `text`, else contains it; the
    /// leftmost, if more than one.
    fn find(&self, text: &str) -> egui::Rect {
        let leftmost = |f: &dyn Fn(&str) -> bool| {
            self.nodes
                .iter()
                .filter(|(t, ..)| f(t))
                .min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x))
        };
        let exact = leftmost(&|t| t == text);
        match exact.or_else(|| leftmost(&|t| t.contains(text))) {
            Some((_, r, _)) => *r,
            None => {
                let have: Vec<_> = self.nodes.iter().map(|(t, ..)| t.as_str()).collect();
                panic!("no widget {text:?}; have {have:?}")
            }
        }
    }

    fn click_at(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.events.push(egui::Event::PointerMoved(pos));
        self.frame();
        self.events.push(press(true));
        self.frame();
        self.events.push(press(false));
        self.frame();
        self.events.push(egui::Event::PointerGone);
        self.frame();
    }

    fn click(&mut self, text: &str) {
        let at = self.find(text).center();
        self.click_at(at, egui::PointerButton::Primary);
    }

    fn right_click(&mut self, text: &str) {
        let at = self.find(text).center();
        self.click_at(at, egui::PointerButton::Secondary);
    }

    /// Settles the layout, renders the last frame and writes `<name>.png`.
    fn shot(&mut self, out_dir: &Path, name: &str) {
        let out = self.frames(3);
        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let (device, queue) = (&self.render.device, &self.render.queue);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: SIZE,
            pixels_per_point: out.pixels_per_point,
        };
        let mut enc = device.create_command_encoder(&Default::default());
        let mut renderer = self.render.renderer.write();
        let extra = renderer.update_buffers(device, queue, &mut enc, &jobs, &screen);
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("screenshot"),
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
                })
                .forget_lifetime();
            renderer.render(&mut pass, &jobs, &screen);
        }
        drop(renderer);
        // 1280 * 4 bytes is a multiple of wgpu's 256-byte row alignment.
        let row = SIZE[0] * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * SIZE[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(SIZE[1]),
                },
            },
            wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
        );
        queue.submit(extra.into_iter().chain([enc.finish()]));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let rgba = buf.slice(..).get_mapped_range().unwrap().to_vec();
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let path = out_dir.join(format!("{name}.png"));
        let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut png = png::Encoder::new(file, SIZE[0], SIZE[1]);
        png.set_color(png::ColorType::Rgb);
        png.set_depth(png::BitDepth::Eight);
        let mut w = png.write_header().unwrap();
        w.write_image_data(&rgb).unwrap();
        w.finish().unwrap();
        eprintln!("wrote {}", path.display());
    }
}

// ---------- made-up data ----------

fn choice(value: i32, label: &str) -> Choice {
    Choice {
        value,
        label: label.into(),
        default: false,
    }
}

/// A small settings schema with invented titles, help and values.
fn schema() -> Schema {
    let mut settings = BTreeMap::new();
    let mut def = |id: u32, title: &str, help: &str, choices: Vec<Choice>| {
        settings.insert(
            SettingId(id),
            SettingDef {
                id: SettingId(id),
                title: title.into(),
                help: help.into(),
                choices,
            },
        );
    };
    let on_off = || vec![choice(0, "Off"), choice(1, "On")];
    def(91, "Team play", "Play in teams.", on_off());
    def(0, "Rounds", "How many rounds a game has.", Vec::new());
    def(
        10,
        "Time limit",
        "How long a round lasts.",
        vec![
            choice(0, "None"),
            choice(300, "5 minutes"),
            choice(600, "10 minutes"),
            choice(900, "15 minutes"),
        ],
    );
    def(90, "Max players", "The most players in a game.", Vec::new());
    def(16, "Lives per round", "0 is unlimited.", Vec::new());
    def(
        17,
        "Respawn time",
        "Seconds before a player respawns.",
        Vec::new(),
    );
    def(
        18,
        "Suicide penalty",
        "Extra seconds after a suicide.",
        Vec::new(),
    );
    def(20, "Motion sensor", "Whether players have one.", Vec::new());
    def(
        83,
        "Weapons on map",
        "Which weapons the map places.",
        Vec::new(),
    );
    let ids = |l: &[u32]| l.iter().map(|&i| SettingId(i)).collect();
    let pages = vec![
        Page {
            id: 100,
            title: "Match".into(),
            settings: ids(&[91, 0, 10, 90]),
        },
        Page {
            id: 9,
            title: "Players".into(),
            settings: ids(&[16, 17, 18, 20]),
        },
        Page {
            id: 12,
            title: "Weapons".into(),
            settings: ids(&[83]),
        },
    ];
    let mut defaults = BTreeMap::new();
    for t in GameType::ALL {
        defaults.insert(t, vec![(SettingId(10), 600), (SettingId(0), 1)]);
    }
    let builtin = |name: &str, game_type, description, pairs| Builtin {
        name: name.into(),
        game_type,
        description,
        pairs,
    };
    Schema {
        settings,
        pages,
        defaults,
        builtins: vec![
            builtin("Example Slayer", GameType::Slayer, 1, vec![]),
            builtin(
                "Example Team Slayer",
                GameType::Slayer,
                2,
                vec![(SettingId(91), 1)],
            ),
            builtin("Example CTF", GameType::Ctf, 3, vec![(SettingId(91), 1)]),
            builtin("Example King", GameType::King, 4, vec![]),
            builtin("Example Oddball", GameType::Oddball, 5, vec![]),
        ],
        source: "made-up example schema".into(),
    }
}

fn store(root: &Path) -> SaveStore {
    SaveStore::new(
        root,
        Some(XbeNames {
            file_names: (0..CONTENT_TYPES).map(|i| format!("type{i}")).collect(),
            labels: (0..CONTENT_TYPES).map(|i| format!("Type {i}")).collect(),
        }),
    )
}

const MAPS: [&str; 8] = [
    "Basin", "Causeway", "Dunes", "Harbor", "Quarry", "Relay", "Spire", "Yard",
];

fn catalog() -> Catalog {
    Catalog {
        maps: MAPS
            .iter()
            .enumerate()
            .map(|(i, name)| CatalogMap {
                id: i as u32,
                name: (*name).into(),
                description: format!("An example map called {name}."),
                key: None,
            })
            .collect(),
    }
}

/// A playlist file with 16 made-up playlists.
fn playlists(schema: &Schema) -> HopperFile {
    const LISTS: [(&str, &str, u32); HOPPERS] = [
        (
            "Rumble Pit",
            "Free for all, every player for themselves.",
            4,
        ),
        ("Team Skirmish", "Small teams, quick games.", 5),
        ("Team Slayer", "Teams of four. Most kills wins.", 6),
        ("Big Team", "Large teams on large maps.", 4),
        ("Objective", "Flags and hills.", 3),
        ("Doubles", "Teams of two.", 3),
        ("Snipers", "Long range only.", 2),
        ("Swat", "No shields, no radar.", 2),
        ("Head to Head", "One against one.", 2),
        ("Classics", "Old favourites.", 3),
        ("Party", "Anything goes.", 2),
        ("Weekend", "This weekend only.", 1),
        ("Ranked", "Every game counts.", 2),
        ("Social", "Games that don't count.", 2),
        ("Training", "Learn the maps.", 1),
        ("Custom", "Your own list.", 1),
    ];
    let variants: Vec<_> = schema
        .builtins
        .iter()
        .map(|b| {
            let mut v = schema.build(b).unwrap();
            v.set_builtin_flag(true);
            v
        })
        .collect();
    let mut f = HopperFile::blank();
    for (i, (title, description, n)) in LISTS.iter().enumerate() {
        let h = f.hopper(i).unwrap();
        f.set_hopper(
            i,
            &Hopper {
                hopper_type: 4,
                status: 1,
                record_count: *n,
                name: String::new(),
                ranges: [1, 8, 4],
                dwords: h.dwords.clone(),
            },
        )
        .unwrap();
        f.set_record_count(i, *n).unwrap();
        f.set_text(
            i,
            &HopperText {
                title: (*title).into(),
                description: (*description).into(),
            },
        )
        .unwrap();
        for j in 0..*n as usize {
            let at = DESCRIPTOR * HOPPERS + i * BLOCK + 0x48 + j * RECORD + VARIANT_AT;
            let v = &variants[(i + j) % variants.len()];
            f.image_mut()[at..at + live_proto::hopper::VARIANT_SIZE].copy_from_slice(&v.to_bytes());
            let mut r = f.record(i, j).unwrap();
            r.id = ((i * 3 + j) % MAPS.len()) as u32;
            r.dwords[0] = [10, 20, 5, 15, 10, 30][j % 6];
            f.set_record(i, j, &r).unwrap();
        }
    }
    f
}

// ---------- the screenshots ----------

#[test]
#[ignore = "writes docs/images/editor/*.png; run with --ignored to regenerate"]
fn screenshots() {
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images/editor");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_dir = out_dir.canonicalize().unwrap();
    let Some(render) = render_state() else {
        eprintln!("no wgpu adapter; no screenshots written");
        return;
    };

    let scratch = std::env::temp_dir().join(format!("h2-editor-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("disc")).unwrap();
    std::fs::write(scratch.join(MAP), scenario_edit::fixture::fixture()).unwrap();
    std::env::set_current_dir(&scratch).unwrap();

    // Saved variants, with one already saved.
    let schema = schema();
    let saves = VariantTool::save_root(Some(Path::new("data")));
    let mut variants_editor = VariantEditor::new(schema.clone(), Some(store(&saves)));
    variants_editor.new_variant(GameType::Slayer);
    variants_editor.set_name("Friday Night").unwrap();
    variants_editor.set_setting(SettingId(10), 900).unwrap();
    variants_editor.save().unwrap();

    let mut hoppers = HopperTool::new();
    // As in the app, which has the system's file dialog: shows Browse...
    hoppers.set_file_dialog(Box::new(ScriptedDialog::new([])));
    hoppers
        .open_bytes(
            playlists(&schema).image(),
            Some(PathBuf::from("playlists/multiplayer_hopper.dat")),
        )
        .unwrap();
    hoppers.set_catalog(PathBuf::from(MAP), catalog());
    hoppers.set_variant_sources(&schema, Some(store(&saves)));

    let mut scenario = ScenarioTool::new();
    scenario.set_data_root(PathBuf::from("$TMPDIR/h2boot-data"));
    let (scenario, scenario_box) = shared(scenario, "Scenario", &["map"]);

    let mut variant_tool = VariantTool::new();
    variant_tool.use_editor(variants_editor, Some(Path::new(MAP)));
    let (variant_tool, variant_box) = shared(variant_tool, "Game variants", &[]);

    let mut shell = Shell::new();
    shell.add_tool(Box::new(hoppers));
    shell.add_tool(scenario_box);
    shell.add_tool(variant_box);
    shell.set_source(MapSource::open(Path::new(MAP)).unwrap());
    let mut h = Harness::new(shell, render);

    // Scenario: open the example map, expand Weapons, select one.
    h.shell.select_tool(1);
    h.shell.select_map(Some("example".into()));
    h.frames(2);
    h.click("Weapons (2)");
    h.click("1: blaster");
    assert_eq!(
        scenario.borrow().editor().selected(),
        Some((BlockKind::Weapons, 1))
    );
    h.shot(&out_dir, "window");

    // Move it: what a drag in the viewport does.
    scenario
        .borrow_mut()
        .editor_mut()
        .apply(Command::set(
            BlockKind::Weapons,
            1,
            "position",
            Value::Vec3([13.5, 7.0, 12.0]),
        ))
        .unwrap();
    h.shot(&out_dir, "scenario-weapon");

    h.right_click("1: blaster");
    h.shot(&out_dir, "scenario-menu");
    h.click_at(egui::pos2(640.0, 795.0), egui::PointerButton::Primary);

    // Opening another file over the unsaved edit.
    h.shell.request_open(PathBuf::from("disc/other.map"));
    h.shot(&out_dir, "unsaved-prompt");
    h.shell.answer(Answer::Cancel);

    h.click("Save as package...");
    h.shot(&out_dir, "scenario-package");

    // Playlists.
    h.shell.select_tool(0);
    h.frames(2);
    h.click("Team Slayer");
    h.shot(&out_dir, "hopper-playlist");
    h.click("Example CTF - Capture the Flag");
    h.shot(&out_dir, "hopper-variant-menu");
    h.click_at(egui::pos2(640.0, 795.0), egui::PointerButton::Primary);

    // Game variants: a built-in, then an edited copy of it.
    h.shell.select_tool(2);
    h.frames(2);
    h.click("Example Team Slayer");
    h.shot(&out_dir, "variants-builtin");
    {
        let mut t = variant_tool.borrow_mut();
        let e = t.editor_mut().unwrap();
        e.copy_builtin("Saturday Teams").unwrap();
        e.set_setting(SettingId(10), 900).unwrap();
        e.set_setting(SettingId(17), 3).unwrap();
        assert!(e.issues().is_empty(), "{:?}", e.issues());
    }
    h.frames(2);
    h.click("Changes from the");
    h.shot(&out_dir, "variants-edit");

    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
}
