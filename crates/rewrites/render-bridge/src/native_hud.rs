//! Native UI and HUD hooks, on with the native renderer
//! (`impl.render.native`, on by default).
//! Producers may skip emission after a complete native reservation. The UI
//! walk keeps the game's order, state setup and layout callbacks.

mod backdrop;
pub mod hud_overlay;
mod model;
mod outline;
mod ui;
pub mod ui_overlay;

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage, Ui};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-hud";

pub use hud_overlay::{Claim as HudClaim, GateRecord as HudGateRecord, HudBitmaps, HudRefusal};
pub use outline::{PUSH_BYTES as OUTLINE_PUSH_BYTES, Vertices as OutlineVertices};
pub use ui::UiScope;
pub use ui_overlay::MenuRefusal;

/// Whether the native UI and HUD are on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

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
    let Some(vertices) = vertices(mem, frame, colour) else {
        Stage::Ui.count(Metric::WalkRefusedInputs, 1);
        return false;
    };
    let drawn = host().is_some_and(|host| host.draw_quad(mem, &vertices));
    ui::record(drawn);
    drawn
}

/// What the screen draws become: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    fn draw_quad(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
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

    fn backdrop_wash(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn backdrop_composite(
        &self,
        _mem: &mut dyn GuestMemory,
        _vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        false
    }

    fn model_ranges(&self, _mem: &mut dyn GuestMemory, _ranges: &[engine::ui_model::Range]) -> bool {
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

    /// One HUD draw is at its first bitmap widget ([`hud_overlay`]): the
    /// host may draw the draw's widgets itself from the overlay stage,
    /// placed at this point in the frame. `None`: the host does not draw
    /// HUDs. `Some(Ok(n))`: it did, `n` of its items the game's plain
    /// widget quads, which are then left out. `Some(Err(why))`: the game
    /// draws them.
    fn hud_bitmaps(&self, _mem: &mut dyn GuestMemory, _at: &HudBitmaps) -> Option<Result<u32, HudRefusal>> {
        None
    }

    /// A claimed HUD draw left out other quads than its pass drew: the
    /// host stops claiming that `nhdt` (`claim.nhdt`).
    fn hud_mismatch(&self, _claim: &HudClaim) {}

    /// A screen's sorted widget list is ready ([`ui_overlay`]): whether the
    /// host may place overlay passes in it now. `None`: the host does not
    /// draw menus. `Some(false)`: not now (counted as `Pass`); the game
    /// draws the list.
    fn ui_overlay(&self, _mem: &mut dyn GuestMemory) -> Option<bool> {
        None
    }

    /// Draws `part`, the items of a run of a list's widgets, from the
    /// overlay stage, placed at this point in the frame. True: placed, and
    /// the run's widget draws are left out; false: the game draws them.
    fn ui_pass(&self, _mem: &mut dyn GuestMemory, _part: &frame_scene::Overlay) -> bool {
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

/// A HUD draw at its first bitmap widget: closes the claim before it and
/// asks the host for this one, counting why when it is the game's.
fn hud_bitmaps(mem: &mut dyn GuestMemory, at: &HudBitmaps) {
    let host = host();
    if let Some(mismatch) = hud_overlay::close()
        && let Some(host) = &host
    {
        host.hud_mismatch(&mismatch);
    }
    match host.and_then(|host| host.hud_bitmaps(mem, at)) {
        Some(Ok(quads)) if quads > 0 => {
            hud_overlay::open(at, quads);
        }
        // Nothing shown: nothing drawn, and nothing of the game's left out.
        Some(Ok(_)) => {}
        Some(Err(why)) => why.counter().count(),
        None => {}
    }
}

/// The HUD scope left: its last claim closes.
pub(crate) fn hud_scope_left() {
    if let Some(mismatch) = hud_overlay::close()
        && let Some(host) = host()
    {
        host.hud_mismatch(&mismatch);
    }
}

/// A screen's sorted list at [`ui_overlay::LIST_READY`]: its claim, when
/// the host draws menus, or why the game draws it.
fn menu_list(mem: &mut dyn GuestMemory, ebp: u32, edi: u32) {
    let Some(list) = ui_overlay::at_list(&*mem, ebp, edi) else { return };
    match host().and_then(|host| host.ui_overlay(mem)) {
        Some(true) => {}
        Some(false) => {
            MenuRefusal::Pass.counter().count();
            return;
        }
        None => return,
    }
    match ui_overlay::claim_list(&*mem, &list) {
        Ok(claim) => {
            Ui::OverlayMenuClaimed.count_n(u64::from(claim.claimed()));
            // A list at this frame left open is closed (and checked) first.
            ui_overlay::open(claim);
        }
        Err(why) => why.counter().count(),
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
            let part = ui_overlay::run_part(frame, run)?;
            // A run of containers has nothing to draw: it needs no pass.
            let placed = part.is_empty() || host().is_some_and(|host| host.ui_pass(mem, &part));
            ui_overlay::placed(frame, run, placed);
            if !placed {
                MenuRefusal::Pass.counter().count();
                return None;
            }
            if !part.is_empty() {
                Ui::OverlayMenuPasses.count();
                if crate::scene_step::with_overlay(|o| o.append(&part).is_ok()) == Some(false) {
                    eprintln!("ui overlay: the scene's overlay refused a menu run's items");
                }
            }
        }
    }
    Ui::OverlayMenuSkipped.count();
    Stage::Ui.count(Metric::BuildsSkipped, 1);
    c.cpu.esp = stack;
    Some(TransferAbi::resume(ITEM_CONTINUATION))
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
        // The overlay's HUD (`hud_overlay`): asked at a HUD draw's first
        // bitmap gate, before any hook that answers the gate itself.
        b.raw().before(&points::HUD_WIDGET_GATE, "native_hud_overlay", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            // A claimed draw's quads are left out by resuming at the
            // quad's tail: without it, nothing is claimed.
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                return Ok(Flow::Continue);
            }
            let mut mem = rt.memory();
            let cpu = &c.cpu;
            if let Some(at) = hud_overlay::at_gate(&*mem, cpu.esp, cpu.ebp, cpu.edi, cpu.ecx, cpu.edx) {
                hud_bitmaps(&mut *mem, &at);
            }
            Ok(Flow::Continue)
        }).first().context_only().optional();
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
            let mut mem = rt.memory();
            menu_list(&mut *mem, c.cpu.ebp, c.cpu.edi);
            Ok(Flow::Continue)
        }).context_only().optional();
        b.raw().before(&ui_overlay::LIST_DONE, "native_ui_overlay_list_done", |c| {
            ui_overlay::close(c.cpu.ebp);
            Ok(Flow::Continue)
        }).context_only().optional();
        b.raw().before(&HUD_QUAD_READY, "native_hud_walk_quad", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                return Ok(Flow::Continue);
            }
            let Some(esp) = c.cpu.esp.checked_add(4) else { return Ok(Flow::Continue) };
            let mut mem = rt.memory();
            // A widget quad the overlay drew is left out: not drawn here
            // and not emitted.
            let owned = mem.u32(c.cpu.esp).ok() == Some(7)
                && hud_overlay::owns_quad(&*mem, c.cpu.ebp);
            if !owned
                && (mem.u32(c.cpu.esp).ok() != Some(7)
                    || !walk_quad(&mut *mem, c.cpu.ebp, c.cpu.ebx))
            {
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
