//! Model-widget ownership begins at the validated sorted UI item. The guest
//! still updates scenario object flags and configures the view and materials.
use std::cell::Cell;
use std::sync::Arc;

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

pub(super) fn begin(c: &Call<'_>, stack_after: u32) {
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
    /// Signed edges in the draw's viewport coordinate system.
    parent: u32,
    bounds: [u16; 4],
    view: frame_scene::OverlayModelView,
}

impl ModelScene {
    /// The full-screen overlay can represent only views wholly within its
    /// canvas. Clamping a viewport would change the camera projection.
    fn record(self, overlay: &mut frame_scene::Overlay) -> bool {
        let size = scene_extract::ui::CANVAS;
        let [left, top, right, bottom] = self.bounds.map(|w| i32::from(w as i16));
        if (overlay.size != [0, 0] && overlay.size != size)
            || left < 0 || top < 0 || right <= left || bottom <= top
            || right > size[0] as i32 || bottom > size[1] as i32 {
            return false;
        }
        let viewport = frame_scene::ClipRect {
            x: left as u32, y: top as u32,
            width: (right - left) as u32, height: (bottom - top) as u32,
        };
        overlay.set_size(size);
        overlay.push_model_view(viewport, self.view).is_ok()
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
        },
        objects,
    };
    Some(ModelScene { parent, bounds: camera.bounds, view })
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
    let Some(scene) = read_view(
        &*mem, widget.address, c.cpu.ebp, c.cpu.eax, c.cpu.ecx, c.cpu.esp, c.cpu.mxcsr,
    ) else {
        refuse(widget, Ui::ModelCamera);
        return;
    };
    if let Some(host) = host() {
        host.model_view(&mut *mem, scene.bounds, &scene.view);
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

    #[test]
    fn view_uses_resolved_objects_in_order_and_camera_axes_from_call() {
        let mut mem = FakeGuest::new();
        let (widget, frame, def, bounds, stack) = (0x1000, 0x3000, 0x5000, 0x7000, 0x9000);
        let (center, forward, up) = (0xa000, 0xb000, 0xc000);
        mem.put_u32(frame + MODEL_WIDGET_LOCAL, widget)
            .put_u32(frame + MODEL_PARENT_BOUNDS, bounds)
            .put_u32(frame + PRIMARY_COUNT, 2)
            .put_u32(frame + SECONDARY_COUNT, 1)
            .put_u32(frame + SECONDARY_HANDLES, 9)
            .put_u32(frame - PRIMARY_HANDLES_BELOW_FRAME, 42)
            .put_u32(frame - PRIMARY_HANDLES_BELOW_FRAME + 4, 17)
            .put_u32(widget + 0x70, def)
            .put_f32(def + 0x34, 1.5)
            .put_f32(0x0045_dbc0, 0.5)
            .put_f32(0x0045_e5b0, 2.5)
            .put_f32(0x0045_e5ac, 0.5)
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
