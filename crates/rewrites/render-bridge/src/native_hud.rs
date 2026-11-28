//! Native UI and HUD hooks, on with the native renderer
//! (`impl.render.native`, on by default).
//! Producers may skip emission after a complete native reservation. The UI
//! walk keeps the game's order, state setup and layout callbacks.

mod backdrop;
pub mod blips;
pub mod definitions;
pub mod fade;
pub mod hud_inputs;
pub mod hud_overlay;
pub mod kept;
pub mod modal_blur;
mod outline;
pub mod sensor;
pub mod strings;
mod ui;
pub mod ui_overlay;
pub mod walk;

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use runtime::abi::TransferAbi;
use scene_extract::hud;
use scene_extract::hud::walk::Kind;
use timeline::native::{Metric, Stage, Ui};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-hud";

pub use hud_overlay::HudTagQuad;
pub use outline::{PUSH_BYTES as OUTLINE_PUSH_BYTES, Vertices as OutlineVertices};
pub use ui::UiScope;
pub use ui_overlay::MenuRefusal;
pub use walk::{WalkQuad, WalkText, WalkWidget};

/// Whether the native UI and HUD are on.
pub fn on(_: &hooks::Switches) -> bool {
    true
}

/// The HUD's widget quads drawn with their material read in Rust, the
/// guest's shader setup and draw left out once a shader is proven
/// (docs/graphics/native-hud-overlay.md, "The material in Rust"): on
/// with the native UI, `impl.render.native-hud=off` turns it off.
pub const RUST_HUD: &str = "impl.render.native-hud";

/// Whether the Rust HUD material ([`RUST_HUD`]) is on.
pub fn rust_hud_on(switches: &hooks::Switches) -> bool {
    on(switches) && switches.feature("impl.render.native-hud", true)
}

/// The HUD's bitmap, text and screen-effect widgets laid out in Rust and
/// drawn with no guest draw once proven (docs/graphics/native-hud-overlay.md, "The walk in
/// Rust"): on with the Rust HUD material, `impl.render.native-hud-walk=off`
/// turns it off.
pub const HUD_WALK: &str = "impl.render.native-hud-walk";

/// Whether the Rust HUD walk ([`HUD_WALK`]) is on.
pub fn hud_walk_on(switches: &hooks::Switches) -> bool {
    rust_hud_on(switches) && switches.feature("impl.render.native-hud-walk", true)
}

static RUST_HUD_PLACED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this run placed the Rust HUD's hooks: the host checks each
/// captured widget quad against its Rust material only then.
pub fn rust_hud() -> bool {
    RUST_HUD_PLACED.load(std::sync::atomic::Ordering::Relaxed)
}

pub use points::scene_effects::HUD_QUAD_SETUP;

use points::scene_effects::HUD_QUAD_READY;
const HUD_QUAD_TAIL: u32 = 0x0022_a854;
const HUD_QUAD_LOCAL_BYTES: u32 = 0x70;
const HUD_QUAD_VERTEX_BYTES: u32 = 0x1c;

/// The four vertices after the guest resolves the engine bitmap and
/// multiplies the texture coordinates by its live dimensions.
pub type HudVertices = [[(u8, [f32; 4]); 3]; 4];

fn vertices(mem: &dyn GuestMemory, frame: u32, colour: u32) -> Option<HudVertices> {
    // The helper writes white to register 9 before the vertex loop.
    if colour != u32::MAX {
        return None;
    }
    let base = frame.checked_sub(HUD_QUAD_LOCAL_BYTES)?;
    let mut vertices = [[(0, [0.0; 4]); 3]; 4];
    for (i, vertex) in vertices.iter_mut().enumerate() {
        let at = base.checked_add(i as u32 * HUD_QUAD_VERTEX_BYTES)?;
        let read = |o| {
            let v = f32::from_bits(mem.u32(at.checked_add(o)?).ok()?);
            (v == 0.0 || v.is_normal()).then_some(v)
        };
        *vertex = [
            (9, [1.0; 4]),
            (3, [read(16)?, read(20)?, 0.0, 1.0]),
            (0, [read(0)?, read(4)?, read(8)?, read(12)?]),
        ];
    }
    Some(vertices)
}

fn walk_quad(mem: &mut dyn GuestMemory, frame: u32, colour: u32) -> bool {
    Stage::Ui.count(Metric::WalkExamined, 1);
    // A HUD item is counted by its kind when nothing draws it
    // (`kept`, `scene_kept_overlay_<kind>_<cause>`).
    let kind = kept::in_hud().then(|| {
        let caller = frame.checked_add(4).and_then(|at| mem.u32(at).ok()).unwrap_or(0);
        kept::quad_kind(caller)
    });
    let Some(vertices) = vertices(mem, frame, colour) else {
        Stage::Ui.count(Metric::WalkRefusedInputs, 1);
        if let Some(kind) = kind {
            kept::count(kind, "inputs");
        }
        return false;
    };
    let Some(host) = host() else {
        if let Some(kind) = kind {
            kept::count(kind, "host");
        }
        ui::record(false);
        return false;
    };
    if let Some(at) = hud_overlay::tag_quad(mem, frame) {
        if let Some(kind) = kind {
            kept::begin(kind);
        }
        if host.draw_tag_quad(mem, &at, &vertices) {
            timeline::native::Ui::OverlayHudTagQuads.count();
            kept::end(true);
            ui::record(true);
            return true;
        }
        timeline::native::Ui::OverlayHudTagFallback.count();
    } else if let Some(at) = hud_overlay::menu_shader_quad(mem, frame) {
        // A menu shader bitmap: the game's draw at the sorted list
        // (`ui_overlay::plan`), its quad from the overlay here. Kept, it
        // counts under the capture's cause (`scene_kept_ui_menu_shader_*`).
        let offered = kind.is_none() && kept::begin_ui(kept::UiKind::MenuShader);
        if host.draw_tag_quad(mem, &at, &vertices) {
            timeline::native::Ui::OverlayMenuShaderQuads.count();
            if offered {
                kept::end(true);
            }
            ui::record(true);
            return true;
        }
        timeline::native::Ui::OverlayMenuShaderFallback.count();
    }
    // The screen capture's cause is the one that counts.
    if let Some(kind) = kind {
        kept::begin(kind);
    }
    let drawn = host.draw_quad(mem, &vertices);
    kept::end(drawn);
    ui::record(drawn);
    drawn
}

/// The shader quad at [`HUD_QUAD_SETUP`]: a HUD widget's quad the host
/// draws from its Rust material, its setup and draw left out.
fn setup_quad(mem: &mut dyn GuestMemory, frame: u32, colour: u32) -> bool {
    // A shadowed widget's quad is checked against Rust's layout first.
    walk::observe(mem, frame);
    let Some(at) = hud_overlay::tag_quad(mem, frame) else { return false };
    let Some(vertices) = vertices(mem, frame, colour) else { return false };
    let drawn = host().is_some_and(|host| host.rust_hud_quad(mem, &at, &vertices));
    if drawn {
        Ui::OverlayHudRustDrawn.count();
        Stage::Ui.count(Metric::BuildsSkipped, 1);
        ui::record(true);
    }
    drawn
}

/// What the screen draws become: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    fn draw_tag_quad(&self, _mem: &mut dyn GuestMemory, _at: &HudTagQuad, _vertices: &HudVertices) -> bool {
        false
    }

    /// A HUD widget's quad before its shader setup ([`HUD_QUAD_SETUP`]):
    /// true when the host drew it from its shader's Rust material, so the
    /// setup and the draw are left out. A shader not yet proven against
    /// the guest's own setup is not drawn here.
    fn rust_hud_quad(&self, _mem: &mut dyn GuestMemory, _at: &HudTagQuad, _vertices: &HudVertices) -> bool {
        false
    }

    /// A proven bitmap widget laid out in Rust ([`walk`]): true when the
    /// host drew every quad of it from the shader's Rust material at the
    /// widget's anchor, so the guest's draw is left out. The host writes
    /// each quad's globals before it reads the quad's material. False:
    /// nothing was drawn, and the guest draws the widget.
    fn rust_hud_widget(&self, _mem: &mut dyn GuestMemory, _widget: &WalkWidget) -> bool {
        false
    }

    /// A proven text widget laid out in Rust ([`walk`]): true when the
    /// host drew every glyph quad of it through the shader's Rust
    /// material over its own glyph pages at the widget's anchor, so the
    /// guest's draw is left out. The host writes the widget's globals
    /// before it reads the material. False: nothing was drawn.
    fn rust_hud_text(&self, _mem: &mut dyn GuestMemory, _text: &WalkText) -> bool {
        false
    }

    /// A proven string of the string draw laid out in Rust ([`strings`]):
    /// true when the host drew every glyph quad of it as text runs over
    /// its own glyph pages, so the string draw is left out. False:
    /// nothing was drawn, and the guest draws the string.
    fn rust_string(&self, _mem: &mut dyn GuestMemory, _run: &strings::StringRun) -> bool {
        false
    }

    fn draw_quad(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn draw_text_quad(&self, _mem: &mut dyn GuestMemory, _shader: u32, _vertices: &HudVertices) -> bool {
        false
    }

    fn screen_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn fade_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        false
    }

    fn loading_bitmap_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        false
    }

    /// The loading screen's spinner fan: a screen draw as
    /// [`Self::draw_quad`]'s unless the host takes it apart.
    fn loading_spinner(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.draw_quad(mem, vertices)
    }

    fn backdrop_wash(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    /// A motion sensor blip's quad ([`blips`]) under the state the device
    /// holds, as the push buffer's Begin, vertices and End the body would
    /// write: true when drawn, so the body is left out.
    fn sensor_blip(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn backdrop_composite(
        &self,
        _mem: &mut dyn GuestMemory,
        _vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        false
    }

    fn outline(&self, _mem: &mut dyn GuestMemory, _vertices: &OutlineVertices) -> bool {
        false
    }

    fn inline_ui_quad(&self, _mem: &mut dyn GuestMemory, _primitive: u32, _bytes: Vec<u8>) -> bool {
        false
    }

    /// The UI walk is about to draw `widget` (a validated sorted item):
    /// true when the host has drawn it itself, in order, so the game's
    /// draw is skipped (the Skulls page's model picture,
    /// docs/game/skulls.md "The model picture").
    fn replace_widget(&self, _mem: &mut dyn GuestMemory, _widget: u32) -> bool {
        false
    }

    /// A model widget's view has been read at its item call, with its
    /// bounds (`left, top, right, bottom`, canvas pixels) and the camera
    /// and objects it draws (`ui_overlay::model::view`). The overlay draws
    /// the widget; the host may queue a pass of its own beside it (the
    /// Forge monitor on the Appearance page,
    /// docs/game/forge-monitor-appearance.md, ML4b).
    fn model_view(
        &self,
        _mem: &mut dyn GuestMemory,
        _bounds: [u16; 4],
        _view: &frame_scene::OverlayModelView,
    ) {
    }

    /// A screen's sorted widget list is ready ([`ui_overlay`]): whether the
    /// host may place overlay passes in it now. `None`: the host does not
    /// draw menus. `Some(false)`: not now (counted as `Pass`); the game
    /// draws the list.
    fn ui_overlay(&self, _mem: &mut dyn GuestMemory) -> Option<bool> {
        None
    }

    /// Whether the host has its own sorted-draw replacement for this widget.
    fn ui_custom_widget(&self, _mem: &dyn GuestMemory, _widget: u32) -> bool {
        false
    }

    /// Where a list class the host registered at run time (its `vtable`)
    /// puts its scroll arrows for a list `height` units tall
    /// ([`ui_overlay::ListArrows`]). `None`: the host does not know the
    /// class, and its list is the game's (`draw_runtime_class`).
    fn ui_list_arrows(&self, _vtable: u32, _height: i32) -> Option<[[i16; 2]; 2]> {
        None
    }

    /// The loaded map's menu font layout and shared overlay glyph pages.
    fn ui_text(&self, _mem: &dyn GuestMemory) -> Option<Box<dyn ui_overlay::UiText>> {
        None
    }

    /// Draws `part`, the items of a run of a list's widgets, from the
    /// overlay stage, placed at this point in the frame. True: placed, and
    /// the run's widget draws are left out; false: the game draws them.
    fn ui_pass(&self, _mem: &mut dyn GuestMemory, _part: &Arc<frame_scene::Overlay>) -> bool {
        false
    }

    /// The blur under a dialog is about to run (`modal_blur`): true when
    /// the host has armed its draws to be left out and `call.blur` drawn
    /// from the overlay in their place at the wrapper's return.
    fn modal_blur(&self, _mem: &mut dyn GuestMemory, _call: &modal_blur::ModalBlur) -> bool {
        false
    }

    /// The armed blur's wrapper returned (`esp` its return slot): true when
    /// the overlay's blur replaced its draws; false when the game's draws
    /// were given back.
    fn finish_modal_blur(&self, _mem: &mut dyn GuestMemory, _esp: u32) -> bool {
        false
    }

    fn enter_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32, _eligible: bool) {}

    fn leave_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32) {}
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one nothing is planned or answered.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    // HOST is only held to clone the host: never across its calls.
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A screen's sorted list at [`ui_overlay::LIST_READY`]: its claim, when
/// the host draws menus, or why the game draws it.
fn menu_list(mem: &mut dyn GuestMemory, ebp: u32, edi: u32) {
    let Some(host) = host() else { return };
    match host.ui_overlay(mem) {
        Some(true) => {}
        Some(false) => MenuRefusal::Pass.panic(),
        None => return,
    }
    if let Err(why) = ui_overlay::check_frame(mem) {
        why.panic();
    }
    let _extract = timeline::native::Timer::start(Stage::Ui, Metric::ExtractNs);
    let Some(list) = ui_overlay::at_list(&*mem, ebp, edi) else { return };
    let claim = ui_overlay::claim_list(mem, &list, host.as_ref());
    Ui::OverlayMenuClaimed.count_n(u64::from(claim.claimed()));
    // A list at this frame left open is closed (and checked) first.
    if let Some(stale) = ui_overlay::open(claim).first() {
        unimplemented!("menus: claim at {:#x} left open, skipped {} of {} draws", stale.frame, stale.skipped, stale.claimed());
    }
}

/// The sorted widget call (`points::ui_render::ITEM_CALL`, the argument
/// pushed, `stack` above it) of a claimed entry: the run's pass placed at
/// its first, then the draw left out, resuming after the call as a
/// host-drawn widget does.
pub(crate) fn menu_item(c: &mut hooks::Call<'_>, mem: &mut dyn GuestMemory, stack: u32) -> Option<TransferAbi> {
    use engine::points::ui_render::ITEM_CONTINUATION;
    use ui_overlay::Item;
    let (frame, ordinal) = (c.cpu.ebp, c.cpu.ebx);
    match ui_overlay::at_item(frame, ordinal) {
        Item::Game => return None,
        Item::Skip => {}
        Item::Place(run) => {
            let part = {
                let _extract = timeline::native::Timer::start(Stage::Ui, Metric::ExtractNs);
                ui_overlay::run_part(frame, run, mem)
            };
            let Some(part) = part else { unimplemented!("menus: run not read again after a guest draw") };
            // A run of containers has nothing to draw: it needs no pass.
            // The pass and the scene's overlay share the run's items.
            let part = Arc::new(part);
            let placed = part.is_empty() || host().is_some_and(|host| host.ui_pass(mem, &part));
            if !placed {
                MenuRefusal::Pass.panic();
            }
            ui_overlay::placed(frame, run, placed);
            if !part.is_empty() {
                Ui::OverlayMenuPasses.count();
                if crate::scene_step::with_overlay(|o| o.append(&part).is_ok()) == Some(false) {
                    eprintln!("ui overlay: the scene's overlay refused a menu run's items");
                }
            }
        }
    }
    // The guest state the left-out draw would have written.
    ui_overlay::left_out(mem, frame, ordinal);
    Ui::OverlayMenuSkipped.count();
    Stage::Ui.count(Metric::BuildsSkipped, 1);
    c.cpu.esp = stack;
    Some(TransferAbi::resume(ITEM_CONTINUATION))
}

/// The stack argument `n` of a call at its entry.
fn argument(mem: &dyn GuestMemory, esp: u32, n: u32) -> Option<u32> {
    mem.u32(esp.checked_add(4 + 4 * n)?).ok()
}

/// A proven widget's draw: the host's quads, then the globals the
/// guest's draw would leave.
fn own_widget(mem: &mut dyn GuestMemory, widget: &WalkWidget, last: &[(u32, u32)]) -> bool {
    let drawn = host().is_some_and(|host| host.rust_hud_widget(mem, widget));
    walk::owned(drawn);
    if !drawn {
        return false;
    }
    for &(at, word) in last {
        if mem.write(at, &word.to_le_bytes()).is_err() {
            eprintln!("hud walk: a widget's global at {at:#x} did not take its word");
        }
    }
    Ui::OverlayHudRustDrawn.count_n(widget.quads.len() as u64);
    Stage::Ui.count(Metric::BuildsSkipped, widget.quads.len() as u64);
    ui::record(true);
    true
}

/// A proven text widget's draw: the host's glyph quads.
fn own_text(mem: &mut dyn GuestMemory, text: &WalkText) -> bool {
    let drawn = host().is_some_and(|host| host.rust_hud_text(mem, text));
    walk::owned_text(drawn);
    if drawn {
        Ui::OverlayHudTextQuads.count_n(text.quads.len() as u64);
        Stage::Ui.count(Metric::BuildsSkipped, text.quads.len() as u64);
        ui::record(true);
    }
    drawn
}

/// A proven string's draw: the host's text runs.
fn own_string(mem: &mut dyn GuestMemory, run: &strings::StringRun) -> bool {
    let drawn = host().is_some_and(|host| host.rust_string(mem, run));
    strings::owned(mem, drawn);
    if drawn {
        Stage::Ui.count(Metric::BuildsSkipped, run.quads.len() as u64);
        ui::record(true);
    }
    drawn
}

/// The string draw's hooks ([`strings`]): Rust's layout and, once its
/// site is proven, its draw at the entry; the check at the return.
fn place_strings(b: &mut Builder) {
    b.raw().before(&points::STRING_DRAW, "native_ui_string", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let owned = {
            let mut mem = rt.memory();
            let fonts = host().and_then(|host| host.ui_text(&*mem));
            match strings::plan(&*mem, c.cpu.esp, c.cpu.eax, c.cpu.ecx, fonts.as_deref()) {
                strings::Plan::Guest => false,
                strings::Plan::Own(run) => own_string(&mut *mem, &run),
            }
        };
        if !owned {
            return Ok(Flow::Continue);
        }
        c.ret(strings::ARGUMENT_BYTES).map(Flow::Return)
    }).context_only().optional();
    b.raw().after(&points::STRING_DRAW, "native_ui_string_done", |_| {
        strings::finish();
        Ok(())
    }).optional();
}

/// A walk Rust loops ([`walk::plan_loop`]): each widget shown is laid
/// out and drawn in the guest's order, after the widgets before it.
/// False when a widget is not Rust's to draw: the guest loops, and its
/// draws of the widgets already drawn are left out ([`walk::done`]).
fn own_loop(mem: &mut dyn GuestMemory, fp: walk::Fp, player: u32, record: u32, shown: &[(Kind, u32, [f32; 4])]) -> bool {
    for &(kind, widget, inputs) in shown {
        let drawn = match kind {
            Kind::Screen => false,
            Kind::Bitmap => match walk::own_bitmap(&*mem, fp, player, record, widget, &inputs) {
                Some((laid, last)) => own_widget(mem, &laid, &last),
                None => false,
            },
            Kind::Text => {
                let fonts = host().and_then(|host| host.ui_text(&*mem));
                match walk::own_text(&*mem, fp, fonts.as_deref(), record, widget, &inputs) {
                    Some(text) => own_text(mem, &text),
                    None => false,
                }
            }
        };
        if !drawn {
            walk::gave_back();
            return false;
        }
        walk::drew(kind, widget);
    }
    walk::looped();
    true
}

/// The definitions' walks in Rust ([`definitions`]), at `HUD_DEFINITIONS`:
/// each definition the record names, in the guest's order, by Rust's own
/// loop. `Ok` when every one was Rust's; otherwise `Err` with the step the
/// guest resumes at, its walk handed over.
fn own_definitions(mem: &mut dyn GuestMemory, fp: walk::Fp, user: u32, record: u32, walks: &[hud::walk::DefinitionWalk]) -> Result<(), usize> {
    for w in walks {
        walk::enter(&*mem, w.datum, user, record, w.slot);
        let owned = match walk::plan_loop() {
            walk::LoopPlan::Guest => false,
            walk::LoopPlan::Own(shown) => own_loop(mem, fp, user, record, &shown),
        };
        if !owned {
            definitions::hand_off(w.datum, w.slot);
            return Err(w.step);
        }
        walk::leave();
    }
    Ok(())
}

/// `HUD_DEFINITIONS`' hooks: the order's check at the return, and Rust's
/// walks once the order is proven.
fn place_definitions(b: &mut Builder) {
    b.raw().before(&points::HUD_DEFINITIONS, "native_hud_definitions", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let (user, record) = (c.cpu.edi, c.cpu.esi);
        let walks = {
            let mem = rt.memory();
            hud::record(&*mem, record).and_then(|r| hud::walk::definitions(&r))
        };
        let definitions::Plan::Own(walks) = definitions::enter(user, record, walks) else {
            return Ok(Flow::Continue);
        };
        // A walk handed back resumes the body at its step: each step the
        // record names past the first needs its resume site.
        let resumable = walks.iter().skip(1).all(|w| {
            definitions::RESUME[w.step].is_some_and(|(pc, _)| rt.read_dispatch(|d| d.has_resume(pc)))
        });
        if !resumable {
            walk::count("definitions_resume");
            definitions::leave();
            return Ok(Flow::Continue);
        }
        let fp = walk::Fp { mxcsr: c.cpu.mxcsr, control: c.cpu.x87.control };
        // The body's frame for a resume: the caller's EBX pushed
        // (`0x001394be`), written before anything is drawn.
        let Some(frame) = c.cpu.esp.checked_sub(4) else { return Ok(Flow::Continue) };
        let caller_ebx = c.cpu.ebx.to_le_bytes();
        let step = {
            let mut mem = rt.memory();
            if mem.write(frame, caller_ebx).is_err() {
                walk::count("definitions_frame");
                definitions::leave();
                return Ok(Flow::Continue);
            }
            // The body's first write (`0x001394bf`).
            let ui = mem.u32(definitions::UI).ok().filter(|&ui| ui != 0);
            let Some(at) = ui.and_then(|ui| ui.checked_add(definitions::UI_RECORD)) else {
                walk::count("definitions_read");
                definitions::leave();
                return Ok(Flow::Continue);
            };
            if mem.write(at, record.to_le_bytes()).is_err() {
                walk::count("definitions_read");
                definitions::leave();
                return Ok(Flow::Continue);
            }
            own_definitions(&mut *mem, fp, user, record, &walks)
        };
        let step = match step {
            Ok(()) => {
                definitions::owned();
                return c.ret(0).map(Flow::Return);
            }
            Err(step) => step,
        };
        // The first walk the record names: the body runs from its entry,
        // and walks nothing before it.
        if Some(step) == walks.first().map(|w| w.step) {
            return Ok(Flow::Continue);
        }
        // Every step past the first has its site (`resumable`); the frame
        // holds the caller's EBX, and EBX is what the step's code expects.
        let Some((pc, ebx)) = definitions::RESUME[step] else { unreachable!("step {step} past the first has a resume site") };
        c.cpu.esp = frame;
        if let Some(ebx) = ebx {
            c.cpu.ebx = ebx;
        }
        Ok(Flow::Return(TransferAbi::resume(pc)))
    }).context_only().optional();
    b.raw().after(&points::HUD_DEFINITIONS, "native_hud_definitions_leave", |_| {
        definitions::leave();
        Ok(())
    }).optional();
}

/// The motion sensor's check ([`sensor`]): Rust's point and draws at the
/// HUD render's call of `HUD_SENSOR_DRAW`, the guest's blip draws and
/// composite as they come, the verdict at the return. Observers only: the
/// guest draws the sensor, and the Halo 1 HUD's hook on the call stays
/// free to skip it.
fn place_sensor(b: &mut Builder) {
    b.raw().observe_before(&points::HUD_SENSOR_DRAW, "native_hud_sensor_enter", |o| {
        let mem = o.guest();
        if mem.u32(o.cpu.esp).ok() != Some(sensor::DRAW_RETURN) {
            return;
        }
        let (user, mxcsr, control) = (o.cpu.ecx, o.cpu.mxcsr, o.cpu.x87.control);
        let at = o.cpu.eax;
        let point = (|| Some([mem.u16(at).ok()? as i16, mem.u16(at.checked_add(2)?).ok()? as i16]))();
        let expected = engine::hud_sensor::point(&*mem, mxcsr)
            .and_then(|p| Ok((p, engine::hud_sensor::draw(&*mem, mxcsr, control, user, p)?)))
            .ok();
        sensor::enter(user, point, expected);
    }).optional();
    b.raw().observe_after(&points::HUD_SENSOR_DRAW, "native_hud_sensor_leave", |_| sensor::leave()).optional();
    b.raw().observe_before(&points::HUD_SENSOR_BLIP, "native_hud_sensor_blip", |o| {
        if !sensor::checking() {
            return;
        }
        let mem = o.guest();
        let esp = o.cpu.esp;
        let arg = |n: u32| mem.u32(esp.wrapping_add(4 + 4 * n)).ok();
        if mem.u32(esp).ok() != Some(sensor::BLIP_RETURN) {
            return;
        }
        let blip = (|| {
            let at = arg(0)?;
            Some(engine::hud_sensor::Blip {
                position: [mem.u32(at).ok()?, mem.u32(at.wrapping_add(4)).ok()?],
                shade: arg(1)?,
                size: arg(2)?,
                colour: arg(3)?,
                pulse: arg(4)? as u8 != 0,
            })
        })();
        match blip {
            Some(b) => sensor::blip(b),
            None => sensor::unread(),
        }
    }).optional();
    b.raw().observe_before(&points::HUD_SENSOR_COMPOSITE, "native_hud_sensor_composite", |o| {
        if !sensor::checking() {
            return;
        }
        let mem = o.guest();
        let esp = o.cpu.esp;
        if mem.u32(esp).ok() != Some(sensor::COMPOSITE_RETURN) {
            return;
        }
        let read = || {
            let at = mem.u32(esp.wrapping_add(4)).ok()?;
            Some(([mem.u32(at).ok()?, mem.u32(at.wrapping_add(4)).ok()?], mem.u32(esp.wrapping_add(8)).ok()?))
        };
        match read() {
            Some((centre, value)) => sensor::composite(centre, value),
            None => sensor::unread(),
        }
    }).optional();
}

/// The blips' cut ([`blips`]): a blip whose setup would change nothing
/// is the host's quad and returns without its body. The sensor's check
/// observes the blip before this runs.
fn place_blips(b: &mut Builder) {
    b.raw().observe_before(&points::HUD_SENSOR_DRAW, "native_hud_blips_enter", |_| blips::edge()).optional();
    b.raw().observe_after(&points::HUD_SENSOR_DRAW, "native_hud_blips_leave", |_| blips::edge()).optional();
    b.raw().before(&points::HUD_SENSOR_BLIP, "native_hud_blips", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let mut mem = rt.memory();
        let host = host();
        let turn = blips::turn(&mut *mem, c.cpu.esp, c.cpu.mxcsr, |m, v| {
            host.as_ref().is_some_and(|h| h.sensor_blip(m, v))
        });
        match turn {
            blips::Turn::Guest => Ok(Flow::Continue),
            blips::Turn::Return { mxcsr } => {
                c.cpu.mxcsr = mxcsr;
                Ok(Flow::Return(mem.ret(c.cpu, 20)?))
            }
        }
    }).context_only().optional();
}

/// The walk's hooks ([`walk`]): Rust's selection and, once proven, its
/// loop at the walk's entry; the guest's widget calls for the check; and
/// each widget's draw.
fn place_walk(b: &mut Builder) {
    b.raw().before(&points::HUD_WALK, "native_hud_walk_enter", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let owned = {
            let mut mem = rt.memory();
            let esp = c.cpu.esp;
            let (Some(player), Some(record), Some(slot)) = (argument(&*mem, esp, 0), argument(&*mem, esp, 1), argument(&*mem, esp, 2)) else {
                return Ok(Flow::Continue);
            };
            definitions::walked(record, c.cpu.eax, slot);
            // Rust's walk of the definitions handed this one over: its
            // entry ran there, and the guest walks it.
            if definitions::take_hand_off(record, c.cpu.eax, slot) {
                return Ok(Flow::Continue);
            }
            walk::enter(&*mem, c.cpu.eax, player, record, slot);
            let fp = walk::Fp { mxcsr: c.cpu.mxcsr, control: c.cpu.x87.control };
            match walk::plan_loop() {
                walk::LoopPlan::Guest => false,
                walk::LoopPlan::Own(shown) => own_loop(&mut *mem, fp, player, record, &shown),
            }
        };
        if !owned {
            return Ok(Flow::Continue);
        }
        // The walk is left out: its widgets are drawn.
        c.ret(12).map(Flow::Return)
    }).context_only().optional();
    b.raw().after(&points::HUD_WALK, "native_hud_walk_leave", |_| {
        walk::leave();
        Ok(())
    }).optional();
    b.raw().before(&points::HUD_SCREEN_WIDGET, "native_hud_walk_screen", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let mut mem = rt.memory();
        let (Some(player), Some(inputs)) = (argument(&*mem, c.cpu.esp, 0), argument(&*mem, c.cpu.esp, 1)) else {
            return Ok(Flow::Continue);
        };
        walk::called(&*mem, Kind::Screen, c.cpu.eax, inputs);
        let walk::ScreenPlan::Own(draw) = walk::plan_screen(&*mem, player, c.cpu.eax, inputs) else {
            return Ok(Flow::Continue);
        };
        // The body is left out: the guest runs the call alone, on the
        // frame the body would have built, and returns through its tail.
        let saved = walk::Saved { ebp: c.cpu.ebp, ebx: c.cpu.ebx, edi: c.cpu.edi, esi: c.cpu.esi };
        let Some((esp, ebp)) = walk::call_frame(&mut *mem, c.cpu.esp, saved, &draw) else {
            return Ok(Flow::Continue);
        };
        c.cpu.esp = esp;
        c.cpu.ebp = ebp;
        c.cpu.eax = draw.effect;
        c.cpu.eip = points::SCREEN_EFFECT.base;
        Ok(Flow::Return(TransferAbi::call(points::SCREEN_EFFECT.base, walk::SCREEN_CALL_RETURN)))
    }).context_only().optional();
    b.raw().after(&points::HUD_SCREEN_WIDGET, "native_hud_walk_screen_done", |_| {
        walk::finish_screen();
        Ok(())
    }).optional();
    b.raw().before(&points::HUD_TEXT_WIDGET, "native_hud_walk_text", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let owned = {
            let mut mem = rt.memory();
            let esp = c.cpu.esp;
            let (Some(record), Some(widget), Some(inputs)) = (argument(&*mem, esp, 1), argument(&*mem, esp, 2), argument(&*mem, esp, 3)) else {
                return Ok(Flow::Continue);
            };
            walk::called(&*mem, Kind::Text, widget, inputs);
            if walk::done(Kind::Text, widget) {
                true
            } else {
                let fp = walk::Fp { mxcsr: c.cpu.mxcsr, control: c.cpu.x87.control };
                let fonts = host().and_then(|host| host.ui_text(&*mem));
                match walk::plan_text(&*mem, fp, fonts.as_deref(), record, widget, inputs) {
                    walk::TextPlan::Guest => false,
                    walk::TextPlan::Own(text) => own_text(&mut *mem, &text),
                }
            }
        };
        if !owned {
            return Ok(Flow::Continue);
        }
        c.ret(16).map(Flow::Return)
    }).context_only().optional();
    b.raw().after(&points::HUD_TEXT_WIDGET, "native_hud_walk_text_done", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        walk::finish_text(&*rt.memory());
        Ok(())
    }).optional();
    b.raw().before(&points::HUD_BITMAP_WIDGET, "native_hud_walk_bitmap", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        let owned = {
            let mut mem = rt.memory();
            let esp = c.cpu.esp;
            let arguments = (argument(&*mem, esp, 0), argument(&*mem, esp, 1), argument(&*mem, esp, 2), argument(&*mem, esp, 3));
            let (Some(player), Some(record), Some(widget), Some(inputs)) = arguments else {
                return Ok(Flow::Continue);
            };
            walk::called(&*mem, Kind::Bitmap, widget, inputs);
            if walk::done(Kind::Bitmap, widget) {
                true
            } else {
                let fp = walk::Fp { mxcsr: c.cpu.mxcsr, control: c.cpu.x87.control };
                match walk::plan(&*mem, fp, player, record, widget, inputs) {
                    walk::Plan::Guest => false,
                    walk::Plan::Own(laid, last) => own_widget(&mut *mem, &laid, &last),
                }
            }
        };
        if !owned {
            return Ok(Flow::Continue);
        }
        c.ret(16).map(Flow::Return)
    }).context_only().optional();
    b.raw().after(&points::HUD_BITMAP_WIDGET, "native_hud_walk_bitmap_done", |c| {
        let rt = Arc::clone(c.ctx.runtime());
        walk::finish(&*rt.memory());
        Ok(())
    }).optional();
}

pub struct NativeHud;

impl Feature for NativeHud {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "The HUD's and the UI's screen draws met by the native GPU renderer",
        )
        .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        ui::place(b);
        modal_blur::place(b);
        // The menus on the overlay (`ui_overlay`): a list's claim opens
        // when it is sorted and closes, checked, when its draws are done.
        // A claimed draw is left out by resuming after its call: without
        // that, nothing is claimed.
        b.raw().before(&ui_overlay::LIST_READY, "native_ui_overlay_list", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let continuation = engine::points::ui_render::ITEM_CONTINUATION;
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(continuation)) {
                return Ok(Flow::Continue);
            }
            if c.cpu.x87.control & 0x0c00 != 0 || c.cpu.mxcsr & 0x6000 != 0 {
                unimplemented!("menus: rounding mode not to nearest");
            }
            let mut mem = rt.memory();
            menu_list(&mut *mem, c.cpu.ebp, c.cpu.edi);
            Ok(Flow::Continue)
        }).context_only().optional();
        b.raw().before(&ui_overlay::LIST_DONE, "native_ui_overlay_list_done", |c| {
            if let Some(claim) = ui_overlay::close(c.cpu.ebp).first() {
                unimplemented!("menus: claim at {:#x} skipped {} of {} draws", claim.frame, claim.skipped, claim.claimed());
            }
            Ok(Flow::Continue)
        }).context_only().optional();
        let rust = rust_hud_on(b.switches());
        RUST_HUD_PLACED.store(rust, std::sync::atomic::Ordering::Relaxed);
        if rust {
            b.raw().before(&HUD_QUAD_SETUP, "native_hud_rust_quad", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                    return Ok(Flow::Continue);
                }
                let mut mem = rt.memory();
                if !setup_quad(&mut *mem, c.cpu.ebp, c.cpu.ebx) {
                    return Ok(Flow::Continue);
                }
                Ok(Flow::Return(TransferAbi::resume(HUD_QUAD_TAIL)))
            }).context_only().optional();
        }
        if hud_walk_on(b.switches()) {
            place_walk(b);
            place_definitions(b);
            place_sensor(b);
            place_strings(b);
        }
        if b.switches().feature("impl.render.native-hud-blips", true) {
            place_blips(b);
        }
        b.raw().before(&HUD_QUAD_READY, "native_hud_walk_quad", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                return Ok(Flow::Continue);
            }
            let Some(esp) = c.cpu.esp.checked_add(4) else { return Ok(Flow::Continue) };
            let mut mem = rt.memory();
            if mem.u32(c.cpu.esp).ok() != Some(7) || !walk_quad(&mut *mem, c.cpu.ebp, c.cpu.ebx) {
                return Ok(Flow::Continue);
            }
            // The fan primitive word is already on the stack.
            c.cpu.esp = esp;
            Ok(Flow::Return(TransferAbi::resume(HUD_QUAD_TAIL)))
        }).context_only().optional();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn the_hud_walk_uses_the_live_corner_order_and_separate_uv_register() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let frame = 0x1800;
        for i in 0..4 {
            let at = frame - HUD_QUAD_LOCAL_BYTES + i * HUD_QUAD_VERTEX_BYTES;
            for (j, word) in [i as f32, -0.0, 0.5, 2.0, 10.0 + i as f32, 20.0].into_iter().enumerate() {
                mem.set_u32(at + 4 * j as u32, word.to_bits()).unwrap();
            }
        }
        let quad = vertices(&mem, frame, u32::MAX).unwrap();
        assert_eq!(quad[2][0], (9, [1.0; 4]));
        assert_eq!(quad[2][1], (3, [12.0, 20.0, 0.0, 1.0]));
        assert_eq!(quad[2][2].0, 0);
        assert_eq!(quad[2][2].1.map(f32::to_bits), [2.0, -0.0, 0.5, 2.0].map(f32::to_bits));
        assert!(vertices(&mem, frame, 0).is_none());
        mem.set_u32(frame - HUD_QUAD_LOCAL_BYTES, f32::NAN.to_bits()).unwrap();
        assert!(vertices(&mem, frame, u32::MAX).is_none());
    }
}
