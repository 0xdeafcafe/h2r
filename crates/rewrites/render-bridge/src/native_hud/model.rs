//! Model-widget ownership begins at the validated sorted UI item. The guest
//! still updates scenario object flags and configures the view and materials.
//!
//! The view in Rust (docs/graphics/native-ui-producers.md, "The model view in Rust"):
//! at the item call the widget's view is read without the game's draw, its
//! objects from the scenario names (`ui_overlay::model::objects`) and its
//! camera words from `engine::ui_model::camera` with the call's argument as
//! the parent view ([`predict`]). At the draw's camera call that view is
//! compared with the one the game passes ([`check`]): a match is the view
//! the scene's overlay and the host take; a difference sends every later
//! view to the game's read for the session, counted
//! `model_view_rust_mismatch_<part>`. Every call is compared, since the
//! game's draw still runs whole.
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use scene_extract::ui::UiModel;

use engine::{memory::GuestMemory, points::ui_render::*, ui_model};
use runtime::{CpuAbi, GuestFault, abi::TransferAbi};
use timeline::native::{Metric, Stage, Ui};

use hooks::{Builder, Call, Flow};

use super::{host, ui};

const VIEW_ACTIVE: u32 = 0x004b_a010;
const MODEL_PARENT_BOUNDS: u32 = 0x7c;
const MODEL_WIDGET_LOCAL: u32 = 0x5c;
const PRIMARY_COUNT: u32 = 0x58;
const SECONDARY_COUNT: u32 = 0x64;
const PRIMARY_HANDLES_BELOW_FRAME: u32 = 0x7c;
const SECONDARY_HANDLES: u32 = 4;
const PRIMARY_CAPACITY: u32 = 32;
const SECONDARY_CAPACITY: u32 = 8;
const VIEW_STACK_BYTES: u32 = 24;
const GEOMETRY_STACK_BYTES: u32 = 12;
const GEOMETRY_LAYER_CALLER: u32 = 0x0003_bc90;
const GEOMETRY_TRANSPARENT_CALLER: u32 = 0x0004_1010;
const GEOMETRY_CALLER_FROM_STACK: u32 = 0x4c;
const TEXTURES_REQUESTED: u32 = 0x0051_f3d8;
const TEXTURES_APPLIED: u32 = 0x0051_f3c8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Widget {
    thread: u32,
    address: u32,
    stack_after: u32,
    view_stack: Option<u32>,
    refused: bool,
}

thread_local! {
    static WIDGET: Cell<Option<Widget>> = const { Cell::new(None) };
    /// The view the item call read in Rust: the widget, the parent view it
    /// was read against, and the view (none: the read refused).
    static PREDICTED: RefCell<Option<(u32, u32, Option<UiModel>)>> = const { RefCell::new(None) };
}

/// Set by a mismatch: the game's view is read for the rest of the session.
static FALLEN_BACK: AtomicBool = AtomicBool::new(false);

/// The view `widget` draws, read in Rust before its draw runs: the live
/// objects its names select and the camera against `parent`, the parent
/// view's `top, left, bottom, right`. `None`: no live object, lights, a
/// camera that does not read.
pub(super) fn predict(mem: &dyn GuestMemory, widget: u32, parent: u32, mxcsr: u32) -> Option<UiModel> {
    let objects = super::ui_overlay::model::objects(&super::ui_overlay::Ro(mem), widget).ok()??;
    if objects.is_empty() {
        return None;
    }
    let camera = ui_model::camera(mem, widget, parent, mxcsr)?;
    Some(UiModel::new(objects, camera.center, camera.bounds, camera.fov, camera.planes))
}

/// The part of a view the Rust read and the game's disagree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mismatch {
    Objects,
    Camera,
}

fn same_camera(a: &UiModel, b: &UiModel) -> bool {
    let bits = |m: &UiModel| {
        let c = &m.view.camera;
        (
            c.position.map(f32::to_bits),
            c.forward.map(f32::to_bits),
            c.up.map(f32::to_bits),
            c.fov_y.to_bits(),
            c.planes.map(f32::to_bits),
        )
    };
    a.viewport == b.viewport && bits(a) == bits(b)
}

/// The view the overlay and the host take at the camera call: the Rust
/// read's, when it matches the game's (`guest`, read against `parent`).
fn check(predicted: Option<(u32, Option<UiModel>)>, parent: u32, guest: UiModel) -> UiModel {
    if FALLEN_BACK.load(Ordering::Relaxed) {
        Ui::ModelViewRustFallback.count();
        return guest;
    }
    let Some((read_parent, Some(rust))) = predicted else {
        Ui::ModelViewRustRefused.count();
        return guest;
    };
    let mismatch = if rust.view.objects != guest.view.objects {
        Some(Mismatch::Objects)
    } else if read_parent != parent || !same_camera(&rust, &guest) {
        Some(Mismatch::Camera)
    } else {
        None
    };
    match mismatch {
        None => {
            Ui::ModelViewRustMatched.count();
            rust
        }
        Some(part) => {
            if !FALLEN_BACK.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "ui model view: the Rust read differs from the game's ({part:?}); the game's for the session\n  rust  {read_parent:#x} {rust:?}\n  guest {parent:#x} {guest:?}"
                );
            }
            let counter = match part {
                Mismatch::Objects => Ui::ModelViewRustMismatchObjects,
                Mismatch::Camera => Ui::ModelViewRustMismatchCamera,
            };
            counter.count();
            guest
        }
    }
}

pub(super) fn try_idle(c: &mut Call<'_>, mem: &dyn GuestMemory, stack: u32) -> Option<TransferAbi> {
    if ui_model::is_idle(mem, c.cpu.ecx) != Some(true) {
        return None;
    }
    if !c.ctx.runtime().read_dispatch(|d| d.has_resume(ITEM_CONTINUATION)) {
        super::kept::ui(Ui::ModelEntry);
        return None;
    }
    Ui::ModelIdle.count();
    Stage::Ui.count(Metric::BuildsSkipped, 1);
    c.cpu.esp = stack;
    Some(TransferAbi::resume(ITEM_CONTINUATION))
}

/// At the item call of an active model widget: its view read in Rust
/// against the call's argument, the parent view.
pub(super) fn begin(c: &Call<'_>, mem: &dyn GuestMemory, stack_after: u32) {
    let predicted = if FALLEN_BACK.load(Ordering::Relaxed) {
        None
    } else {
        let widget = c.cpu.ecx;
        mem.u32(c.cpu.esp).ok().map(|parent| (widget, parent, predict(mem, widget, parent, c.cpu.mxcsr)))
    };
    PREDICTED.with(|p| *p.borrow_mut() = predicted);
    WIDGET.set(Some(Widget {
        thread: c.ctx.thread_id,
        address: c.cpu.ecx,
        stack_after,
        view_stack: None,
        refused: false,
    }));
}

pub(super) fn end(c: &Call<'_>) -> bool {
    let Some(widget) = WIDGET.get() else { return false };
    if widget.thread != c.ctx.thread_id || widget.stack_after != c.cpu.esp {
        return false;
    }
    if widget.refused {
        Ui::ModelWidgetFallback.count();
    }
    PREDICTED.with(|p| p.borrow_mut().take());
    WIDGET.set(None);
    true
}

fn live(c: &Call<'_>) -> Option<Widget> {
    WIDGET.get().filter(|w| w.thread == c.ctx.thread_id && c.cpu.esp < w.stack_after)
}

fn refuse(mut widget: Widget, reason: Ui) {
    widget.refused = true;
    WIDGET.set(Some(widget));
    super::kept::ui(reason);
}

struct ModelScene {
    /// The parent view's signed edges: `top, left, bottom, right`.
    parent: u32,
    model: UiModel,
}

impl ModelScene {
    /// The full-screen overlay can represent only views wholly within its
    /// canvas ([`UiModel::clip`]).
    fn record(self, overlay: &mut frame_scene::Overlay) -> bool {
        let size = scene_extract::ui::CANVAS;
        let Some(viewport) = self.model.clip() else { return false };
        if overlay.size != [0, 0] && overlay.size != size {
            return false;
        }
        overlay.set_size(size);
        overlay.push_model_view(viewport, self.model.view).is_ok()
    }
}

/// Read the model view at the nested camera call, after the guest resolves
/// its object and light names. The latter are separate datum namespaces.
fn read_view(
    mem: &dyn GuestMemory,
    widget: u32,
    frame: u32,
    center: u32,
    up: u32,
    stack: u32,
    mxcsr: u32,
) -> Option<ModelScene> {
    let word = |offset| mem.u32(frame.checked_add(offset)?).ok();
    if word(MODEL_WIDGET_LOCAL)? != widget {
        return None;
    }
    let primary = word(PRIMARY_COUNT)?;
    let secondary = word(SECONDARY_COUNT)?;
    if !(1..=PRIMARY_CAPACITY).contains(&primary) || secondary > SECONDARY_CAPACITY {
        return None;
    }
    let objects_at = frame.checked_sub(PRIMARY_HANDLES_BELOW_FRAME)?;
    let objects = (0..primary).map(|index| {
        let handle = mem.u32(objects_at.checked_add(4 * index)?).ok()?;
        (handle != u32::MAX).then_some(handle)
    }).collect::<Option<Vec<_>>>()?;
    for index in 0..secondary {
        if word(SECONDARY_HANDLES + index * 4)? == u32::MAX {
            return None;
        }
    }
    let vec3 = |ptr: u32| {
        let mut v = [0.0; 3];
        for (i, component) in v.iter_mut().enumerate() {
            *component = mem.f32(ptr.checked_add(i as u32 * 4)?).ok()?;
        }
        v.iter().all(|f| f.is_finite()).then_some(v)
    };
    let parent = word(MODEL_PARENT_BOUNDS)?;
    let camera = ui_model::camera(mem, widget, parent, mxcsr)?;
    let position = vec3(center)?;
    if position.map(f32::to_bits) != camera.center {
        return None;
    }
    for (i, coordinate) in camera.bounds.into_iter().enumerate() {
        if mem.u16(stack.checked_add(i as u32 * 4)?).ok()? != coordinate {
            return None;
        }
    }
    if mem.u32(stack.checked_add(20)?).ok()? != camera.fov {
        return None;
    }
    let view = frame_scene::OverlayModelView {
        camera: frame_scene::OverlayCamera {
            position,
            forward: vec3(mem.u32(stack.checked_add(16)?).ok()?)?,
            up: vec3(up)?,
            fov_y: f32::from_bits(camera.fov),
            planes: camera.planes.map(f32::from_bits),
        },
        objects,
    };
    Some(ModelScene { parent, model: UiModel { viewport: camera.bounds, view } })
}

fn view(c: &Call<'_>) {
    let Some(mut widget) = live(c) else { return };
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|d| {
        d.has_generated(MODEL_VIEW_RETURN.base) && d.has_generated(MODEL_GEOMETRY_CALL.base)
    }) {
        refuse(widget, Ui::ModelEntry);
        return;
    }
    let mut mem = rt.memory();
    let Some(guest) = read_view(
        &*mem, widget.address, c.cpu.ebp, c.cpu.eax, c.cpu.ecx, c.cpu.esp, c.cpu.mxcsr,
    ) else {
        refuse(widget, Ui::ModelCamera);
        return;
    };
    let predicted = PREDICTED
        .with(|p| p.borrow_mut().take())
        .filter(|&(at, _, _)| at == widget.address)
        .map(|(_, parent, model)| (parent, model));
    let scene = ModelScene { parent: guest.parent, model: check(predicted, guest.parent, guest.model) };
    if let Some(host) = host() {
        host.model_view(&mut *mem, scene.model.viewport, &scene.model.view);
    }
    // Snapshot the view at the point it draws. The nested material hooks
    // still own geometry and its depth/stencil state. This records no skip.
    if super::ui_overlay::screen_rect(&*mem) == Some(super::ui_overlay::FULL_SCREEN)
        && super::ui_overlay::FULL_SCREEN.iter().enumerate().all(|(i, &edge)| {
            scene.parent.checked_add(i as u32 * 2).and_then(|at| mem.u16(at).ok()) == Some(edge as u16)
        })
    {
        crate::scene_step::with_overlay(|overlay| scene.record(overlay));
    }
    widget.view_stack = Some(c.cpu.esp);
    WIDGET.set(Some(widget));
}

fn textures_applied(mem: &dyn GuestMemory) -> bool {
    (0..4).all(|stage| {
        let requested = mem.u32(TEXTURES_REQUESTED + 4 * stage).ok();
        requested.is_some() && requested == mem.u32(TEXTURES_APPLIED + 4 * stage).ok()
    })
}

fn plan(c: &Call<'_>, mem: &dyn GuestMemory, widget: Widget) -> Option<(u32, Vec<ui_model::Range>)> {
    let esp = c.cpu.esp;
    let view = widget.view_stack?;
    // Require both the widget view's call frame and the standard material
    // handler. The same geometry function is shared with the main scene.
    if esp >= view
        || mem.u32(view.checked_sub(4)?).ok() != Some(MODEL_VIEW_CONTINUATION)
        || mem.u8(VIEW_ACTIVE).ok() != Some(1)
    {
        return None;
    }
    let word = |offset| mem.u32(esp.checked_add(offset)?).ok();
    if !matches!(word(GEOMETRY_CALLER_FROM_STACK), Some(GEOMETRY_LAYER_CALLER | GEOMETRY_TRANSPARENT_CALLER)) {
        refuse(widget, Ui::ModelCaller);
        return None;
    }
    Stage::Ui.count(Metric::WalkExamined, 1);
    if !c.ctx.runtime().read_dispatch(|d| d.has_resume(MODEL_GEOMETRY_RETURN)) {
        refuse(widget, Ui::ModelEntry);
        return None;
    }
    let next_stack = esp.checked_add(GEOMETRY_STACK_BYTES)?;
    let masked = c.cpu.eax as u8 != 0;
    let (Some(section), Some(group), Some(mask)) = (word(0), word(4), word(8)) else {
        refuse(widget, Ui::ModelGeometry);
        return None;
    };
    match ui_model::ranges(mem, section, group, masked.then_some(mask), c.cpu.ecx as u8 != 0) {
        Ok(ranges) => Some((next_stack, ranges)),
        Err(reason) => {
            refuse(widget, match reason {
                ui_model::Refusal::Inputs => Ui::ModelGeometry,
                ui_model::Refusal::RangeLimit => Ui::ModelRangeLimit,
            });
            Stage::Ui.count(Metric::WalkRefusedInputs, 1);
            None
        }
    }
}

fn finish(cpu: &mut CpuAbi, next_stack: u32, count: u32) -> TransferAbi {
    cpu.eax = count;
    cpu.esp = next_stack;
    super::kept::ui(Ui::ModelNativeGroup);
    ui::record(true);
    TransferAbi::resume(MODEL_GEOMETRY_RETURN)
}

fn record(mem: &mut dyn GuestMemory, widget: Widget, ranges: &[ui_model::Range]) -> bool {
    let drawn = host().is_some_and(|h| h.model_ranges(mem, ranges));
    if !drawn {
        refuse(widget, Ui::ModelReservation);
        ui::record(false);
    }
    drawn
}

fn geometry(c: &mut Call<'_>) -> Result<Flow, GuestFault> {
    let Some(widget) = live(c) else { return Ok(Flow::Continue) };
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let Some((next_stack, ranges)) = plan(c, &*mem, widget) else {
        return Ok(Flow::Continue);
    };
    if ranges.is_empty() {
        return Ok(Flow::Return(finish(c.cpu, next_stack, 0)));
    }
    let count = ranges.iter().map(|r| r.count).sum();
    if textures_applied(&*mem) {
        return Ok(if record(&mut *mem, widget, &ranges) {
            Flow::Return(finish(c.cpu, next_stack, count))
        } else {
            Flow::Continue
        });
    }
    if !rt.read_dispatch(|d| {
        d.has_generated(MODEL_TEXTURE_COMMIT) && d.has_generated(MODEL_GEOMETRY_EMITTER)
    }) {
        refuse(widget, Ui::ModelTextureCommit);
        return Ok(Flow::Continue);
    }

    // Use the same state commit that the emitter would run. If recording
    // fails, the applied mirror makes the retail emitter's repeat commit a
    // no-op. No stream or range of the replacement has been published.
    let saved = *c.cpu;
    let transfer = c.ctx.call_guest_named(
        c.cpu, &mut mem, MODEL_TEXTURE_COMMIT, "native UI model texture commit",
        move |cpu, ctx| {
            *cpu = saved;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            if WIDGET.get() == Some(widget) {
                if !textures_applied(&*mem) {
                    refuse(widget, Ui::ModelTextureCommit);
                } else if record(&mut *mem, widget, &ranges) {
                    return Ok(finish(cpu, next_stack, count));
                }
            }
            mem.call(cpu, MODEL_GEOMETRY_EMITTER, MODEL_GEOMETRY_RETURN)
        },
    )?;
    Ok(Flow::Return(transfer))
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw()
        .before(&MODEL_VIEW_CALL, "native_ui_model_view", |c| {
            crate::scene_step::set_model_view(true);
            view(c);
            Ok(Flow::Continue)
        })
        .context_only()
        .optional();
    b.raw()
        .before(&MODEL_VIEW_RETURN, "native_ui_model_view_done", |c| {
            crate::scene_step::set_model_view(false);
            if let Some(mut widget) = live(c)
                && widget.view_stack.and_then(|s| s.checked_add(VIEW_STACK_BYTES)) == Some(c.cpu.esp)
            {
                widget.view_stack = None;
                WIDGET.set(Some(widget));
            }
            Ok(Flow::Continue)
        })
        .context_only()
        .optional();
    b.raw()
        .before(&MODEL_GEOMETRY_CALL, "native_ui_model_geometry", geometry)
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const WIDGET: u32 = 0x1000;
    const FRAME: u32 = 0x3000;
    const DEF: u32 = 0x5000;
    const BOUNDS: u32 = 0x7000;
    const STACK: u32 = 0x9000;
    const CENTER: u32 = 0xa000;
    const UP: u32 = 0xc000;

    /// A model widget at its camera call: the game's frame and arguments,
    /// and the names, scenario and handles they were resolved from.
    fn fixture() -> FakeGuest {
        let mut mem = FakeGuest::new();
        let (widget, frame, def, bounds, stack) = (WIDGET, FRAME, DEF, BOUNDS, STACK);
        let (center, forward, up) = (CENTER, 0xb000, UP);
        let (names, scenario, table, handles) = (0xd000, 0xe000, 0xf000, 0x1_0000);
        mem.put_u32(frame + MODEL_WIDGET_LOCAL, widget)
            .put_u32(frame + MODEL_PARENT_BOUNDS, bounds)
            .put_u32(frame + PRIMARY_COUNT, 2)
            .put_u32(frame + SECONDARY_COUNT, 0)
            .put_u32(frame - PRIMARY_HANDLES_BELOW_FRAME, 42)
            .put_u32(frame - PRIMARY_HANDLES_BELOW_FRAME + 4, 17)
            .put_u32(widget + 0x70, def)
            .put_u32(0x0054_d598, 4)
            .put_u32(def + 0x0c, 2)
            .put_u32(def + 0x10, names)
            .put_u32(def + 0x14, 0)
            .put_u32(names, u32::from(b'A'))
            .put_u32(names + 32, u32::from(b'B'))
            .put_u32(0x004e_0350, scenario)
            .put_u32(scenario + 0x48, 2)
            .put_u32(scenario + 0x4c, table)
            .put_u32(table, u32::from(b'A'))
            .put_u32(table + 36, u32::from(b'B'))
            .put_u32(0x004d_e2d0, handles)
            .put_u32(handles, 42)
            .put_u32(handles + 4, 17)
            .put_f32(def + 0x34, 1.5)
            .put_f32(0x0045_dbc0, 0.5)
            .put_f32(0x0045_e5b0, 2.5)
            .put_f32(0x0045_e5ac, 0.5)
            .put_f32(ui_model::VIEW_NEAR, 0.0625)
            .put_f32(ui_model::VIEW_FAR, 200.0)
            .put_u32(stack + 16, forward)
            .put_f32(stack + 20, 0.75);
        for (i, outer) in [0u16, 0, 480, 640].into_iter().enumerate() {
            let at = i as u32 * 2;
            mem.put_u16(bounds + at, outer)
                .put_u16(def + 0x38 + at, [12i16, -10, -12, 10][i] as u16)
                .put_u16(stack + i as u32 * 4, [310, 228, 330, 252][i]);
        }
        for i in 0..3 {
            let at = i * 4;
            mem.put_f32(widget + 0x58 + at, 0.0)
                .put_f32(def + 0x1c + at, 1.0)
                .put_f32(def + 0x28 + at, i as f32)
                .put_f32(center + at, i as f32)
                .put_f32(forward + at, if i == 1 { 1.0 } else { 0.0 })
                .put_f32(up + at, if i == 2 { 1.0 } else { 0.0 });
        }
        mem
    }

    #[test]
    fn view_uses_resolved_objects_in_order_and_camera_axes_from_call() {
        let mem = fixture();
        let (widget, frame, stack, center, up) = (WIDGET, FRAME, STACK, CENTER, UP);
        let scene = read_view(&mem, widget, frame, center, up, stack, 0x1f80).unwrap();
        let mut overlay = frame_scene::Overlay::default();
        assert!(scene.record(&mut overlay));
        assert_eq!(overlay.items[0].clip, frame_scene::ClipRect { x: 310, y: 228, width: 20, height: 24 });
        let view = overlay.model_view_of(&overlay.items[0]).unwrap();
        assert_eq!(view.objects, vec![42, 17]);
        assert_eq!(view.camera.forward, [0.0, 1.0, 0.0]);
        assert_eq!(view.camera.up, [0.0, 0.0, 1.0]);
        assert_eq!(view.camera.position, [0.0, 1.0, 2.0]);
        assert_eq!(view.camera.fov_y, 0.75);
        assert_eq!(view.camera.planes, [0.0625, 200.0]);
    }

    /// The item call's Rust read of the view, before the game's draw runs,
    /// is the view the game passes its camera call.
    #[test]
    fn the_rust_view_is_the_view_the_game_draws() {
        let mem = fixture();
        let guest = read_view(&mem, WIDGET, FRAME, CENTER, UP, STACK, 0x1f80).unwrap();
        let rust = predict(&mem, WIDGET, BOUNDS, 0x1f80).unwrap();
        assert_eq!(rust, guest.model);
        let mut lit = mem.clone();
        lit.put_u32(DEF + 0x14, 1);
        assert_eq!(predict(&lit, WIDGET, BOUNDS, 0x1f80), None, "a widget with lights is not read");
    }

    /// A matching view is the Rust read's; one that differs is the game's,
    /// and so is every view after it for the session.
    #[test]
    fn a_view_that_differs_falls_back_for_the_session() {
        FALLEN_BACK.store(false, Ordering::Relaxed);
        let mem = fixture();
        let guest = read_view(&mem, WIDGET, FRAME, CENTER, UP, STACK, 0x1f80).unwrap().model;
        let rust = predict(&mem, WIDGET, BOUNDS, 0x1f80).unwrap();
        assert_eq!(check(Some((BOUNDS, Some(rust.clone()))), BOUNDS, guest.clone()), rust);
        assert_eq!(check(Some((BOUNDS, None)), BOUNDS, guest.clone()), guest, "a refused read takes the game's");
        assert!(!FALLEN_BACK.load(Ordering::Relaxed), "a refusal is not a mismatch");
        let mut moved = rust.clone();
        moved.viewport[0] += 1;
        assert_eq!(check(Some((BOUNDS, Some(moved))), BOUNDS, guest.clone()), guest);
        assert!(FALLEN_BACK.load(Ordering::Relaxed));
        assert_eq!(check(Some((BOUNDS, Some(rust))), BOUNDS, guest.clone()), guest, "the session takes the game's");
        FALLEN_BACK.store(false, Ordering::Relaxed);
    }

    #[test]
    fn model_builder_only_skips_a_noop_texture_commit() {
        let mut mem = FakeGuest::new();
        assert!(!textures_applied(&mem));
        for i in 0..4u32 {
            mem.put_u32(TEXTURES_REQUESTED + 4 * i, 100 + i)
                .put_u32(TEXTURES_APPLIED + 4 * i, 100 + i);
        }
        assert!(textures_applied(&mem));
        mem.put_u32(TEXTURES_REQUESTED + 8, 500);
        assert!(!textures_applied(&mem));
    }
}
