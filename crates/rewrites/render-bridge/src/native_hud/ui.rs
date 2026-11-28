//! Live UI producer boundaries. An unavailable entry, stack or draw keeps
//! the guest at the original instruction.
use std::sync::Arc;

use guest_layout::memory::GuestMemory;
use guest_layout::points::{Point, UI_RENDER_LAYERS, ui_render::*};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage, Ui};

use hooks::{Builder, Call, Flow};

use super::{HudVertices, host};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiScope {
    Layers,
    Widget,
    Hud,
    Loading,
    Overlay,
    FrameTail,
}

const SCOPES: [(UiScope, &Point, &Point, Option<&Point>); 5] = [
    (UiScope::Layers, &UI_RENDER_LAYERS, &LAYERS_RETURN, None),
    (UiScope::Widget, &ITEM_CALL, &ITEM_RETURN, None),
    (UiScope::Hud, &HUD_DRAW, &HUD_RETURN, Some(&HUD_HIDDEN_RETURN)),
    (UiScope::Loading, &LOADING_DRAW, &LOADING_RETURN, Some(&LOADING_EMPTY_RETURN)),
    (UiScope::Overlay, &FADE_DRAW, &FADE_RETURN, None),
];

const WIDGET_COUNT_FROM_EBP: u32 = 4;
const WIDGET_LIST_FROM_EBP: u32 = 0xc0c;
const WIDGET_ENTRY_BYTES: u32 = 0xc;
const MAX_WIDGETS: u32 = 256;
const WIDGET_DRAW_SLOT: u32 = 0x10;
const RECT_VERTEX_COUNT: u32 = 4;
const RECT_VERTEX_BYTES: u32 = 20;
const RECT_BYTES: u32 = RECT_VERTEX_COUNT * RECT_VERTEX_BYTES;

fn widget_in_order(
    mem: &dyn GuestMemory,
    ebp: u32,
    ordinal: u32,
    entry: u32,
    widget: u32,
    vtable: u32,
) -> Option<u32> {
    let count = mem.u32(ebp.checked_sub(WIDGET_COUNT_FROM_EBP)?).ok()?;
    if count > MAX_WIDGETS || ordinal >= count || widget == 0 || vtable == 0 {
        return None;
    }
    let expected = ebp.checked_sub(WIDGET_LIST_FROM_EBP)?
        .checked_add(ordinal.checked_mul(WIDGET_ENTRY_BYTES)?)?;
    if expected != entry || mem.u32(entry).ok()? != widget || mem.u32(widget).ok()? != vtable {
        return None;
    }
    let draw = mem.u32(vtable.checked_add(WIDGET_DRAW_SLOT)?).ok()?;
    (draw != 0).then_some(draw)
}

fn enter(
    c: &mut Call<'_>,
    scope: UiScope,
    finish: &Point,
    alternate: Option<&Point>,
) -> Option<TransferAbi> {
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|dispatch| {
        dispatch.has_generated(finish.base)
            && alternate.is_none_or(|point| dispatch.has_generated(point.base))
    }) {
        return None;
    }
    let mut mem = rt.memory();
    let (stack, eligible) = if scope == UiScope::Widget {
        Ui::Items.count();
        let cpu = &c.cpu;
        let draw = widget_in_order(&*mem, cpu.ebp, cpu.ebx, cpu.esi, cpu.ecx, cpu.eax);
        let eligible = draw.is_some_and(|draw| rt.read_dispatch(|d| d.has_generated(draw)));
        if !eligible {
            Ui::ItemRefused.count();
        }
        // The virtual call consumes its already-pushed argument.
        let stack = cpu.esp.checked_add(4)?;
        if draw == Some(MODEL_WIDGET_DRAW) {
            model_view(&mut *mem, c.cpu.ecx, c.cpu.esp);
        }
        // A widget the overlay draws (`super::ui_overlay`) is left out.
        if draw.is_some() && let Some(transfer) = super::menu_item(c, &mut *mem, stack) {
            return Some(transfer);
        }
        if draw.is_some() && let Some(transfer) = replaced(c, &mut *mem, stack) {
            return Some(transfer);
        }
        if draw == Some(MODEL_WIDGET_DRAW) {
            unimplemented!("ui model: a model widget the overlay did not take (widget {:#x})", c.cpu.ecx);
        }
        (stack, eligible)
    } else {
        (c.cpu.esp, true)
    };
    if scope == UiScope::Hud {
        super::kept::set_in_hud(true);
    }
    if let Some(host) = host() {
        host.enter_ui(&mut *mem, scope, stack, eligible);
    }
    None
}

/// A model widget's view, read at its item call against the call's
/// argument (the parent view at `[esp]`), handed to the host
/// ([`super::Host::model_view`]).
fn model_view(mem: &mut dyn GuestMemory, widget: u32, esp: u32) {
    let Some(host) = host() else { return };
    let Ok(parent) = mem.u32(esp) else {
        unimplemented!("ui model: the item call's argument does not read");
    };
    let model = match super::ui_overlay::model::view(&super::ui_overlay::Ro(&*mem), widget, parent) {
        Ok(Some(model)) => model,
        Ok(None) => return,
        Err(why) => unimplemented!("ui model: the view does not read ({why:?})"),
    };
    host.model_view(mem, model.viewport, &model.view);
}

/// A widget the host draws itself ([`super::Host::replace_widget`]): the
/// virtual draw is skipped as an idle model widget's is, resuming after the
/// call with its argument consumed.
fn replaced(c: &mut Call<'_>, mem: &mut dyn GuestMemory, stack: u32) -> Option<TransferAbi> {
    if !c.ctx.runtime().read_dispatch(|d| d.has_resume(ITEM_CONTINUATION)) {
        return None;
    }
    if !host()?.replace_widget(mem, c.cpu.ecx) {
        return None;
    }
    Stage::Ui.count(Metric::BuildsSkipped, 1);
    c.cpu.esp = stack;
    Some(TransferAbi::resume(ITEM_CONTINUATION))
}

fn leave(c: &mut Call<'_>, scope: UiScope) {
    if scope == UiScope::Hud {
        super::kept::set_in_hud(false);
    }
    if let Some(host) = host() {
        let rt = Arc::clone(c.ctx.runtime());
        host.leave_ui(&mut *rt.memory(), scope, c.cpu.esp);
    }
}

fn fade_vertices(width: u32, height: u32) -> [[(u8, [f32; 4]); 1]; 4] {
    let w = width as i16 as f32;
    let h = height as i16 as f32;
    [[0.0, 0.0], [h, 0.0], [h, w], [0.0, w]].map(|[x, y]| [(0, [x, y, 0.0, 1.0])])
}

pub(super) fn record(drawn: bool) {
    if drawn {
        Stage::Ui.count(Metric::BuildsSkipped, 1);
    }
    Stage::Ui.count(if drawn { Metric::WalkSuppressed } else { Metric::WalkRefusedReservation }, 1);
}

fn fade_fan(c: &mut Call<'_>) -> Option<TransferAbi> {
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|dispatch| dispatch.has_resume(FADE_FAN_TAIL)) {
        return None;
    }
    let mut mem = rt.memory();
    if mem.u32(c.cpu.esp).ok()? != 7 { return None; }
    let esp = c.cpu.esp.checked_add(4)?;
    Stage::Ui.count(Metric::WalkExamined, 1);
    let vertices = fade_vertices(c.cpu.edi, c.cpu.esi);
    let drawn = host().is_some_and(|host| host.fade_fan(&mut *mem, &vertices));
    record(drawn);
    if drawn {
        c.cpu.esp = esp;
        Some(TransferAbi::resume(FADE_FAN_TAIL))
    } else {
        None
    }
}

// The spinner has already computed its two UV pairs per vertex. ESP here
// includes the primitive argument. The base below is the frame after Begin.
fn spinner_vertices(mem: &dyn GuestMemory, frame: u32) -> Option<HudVertices> {
    let f = |offset| mem.u32(frame.checked_add(offset)?).ok().map(f32::from_bits);
    let [left, right, top, bottom] = [f(0x18)?, f(0x1c)?, f(0x20)?, f(0x24)?];
    let corners = [[left, top], [right, top], [right, bottom], [left, bottom]];
    let mut vertices = [[(0, [0.0; 4]); 3]; 4];
    for (i, (vertex, [x, y])) in vertices.iter_mut().zip(corners).enumerate() {
        let offset = i as u32 * 8;
        *vertex = [
            (1, [f(0x28 + offset)?, f(0x2c + offset)?, 0.0, 1.0]),
            (2, [f(0x48 + offset)?, f(0x4c + offset)?, 0.0, 1.0]),
            (0, [x, y, engine::ui_sprite::D24_FULL_SCALE, engine::ui_sprite::D24_FULL_SCALE]),
        ];
    }
    Some(vertices)
}

fn loading_fan(c: &mut Call<'_>, spinner: bool) -> Option<TransferAbi> {
    let tail = if spinner { LOADING_SPINNER_TAIL } else { LOADING_BITMAP_TAIL };
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|dispatch| dispatch.has_resume(tail)) {
        return None;
    }
    let mut mem = rt.memory();
    if mem.u32(c.cpu.esp).ok()? != 7 { return None; }
    let esp = c.cpu.esp.checked_add(4)?;
    Stage::Ui.count(Metric::WalkExamined, 1);
    let mut mxcsr = c.cpu.mxcsr;
    let host = host()?;
    let drawn = if spinner {
        let Some(vertices) = spinner_vertices(&*mem, esp) else { return geometry_refused() };
        host.loading_spinner(&mut *mem, &vertices)
    } else {
        let vertices = u8::try_from(c.cpu.ebp).ok().and_then(|register| {
            engine::ui_sprite::mapped_rectangle(
                &*mem, esp, c.cpu.esi, c.cpu.ebx, register, &mut mxcsr,
            )
        });
        let Some(vertices) = vertices else { return geometry_refused() };
        host.loading_bitmap_fan(&mut *mem, &vertices)
    };
    record(drawn);
    if drawn {
        c.cpu.mxcsr = mxcsr;
        c.cpu.esp = esp;
        Some(TransferAbi::resume(tail))
    } else {
        None
    }
}

fn geometry_refused() -> Option<TransferAbi> {
    Ui::Geometry.count();
    Stage::Ui.count(Metric::WalkRefusedInputs, 1);
    None
}

fn overlay_rect(c: &mut Call<'_>) -> Option<TransferAbi> {
    use native_render::effect_geometry::{EFFECT_QUAD_BUILD, PRIMITIVE_TABLE};

    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    // Dynamic helper at 0x4b252, after BeginPush and before writing to it.
    // Only the UI's callback and its exact four-vertex copy are ours.
    let frame = c.cpu.esp;
    let word = |offset| mem.u32(frame.checked_add(offset)?).ok();
    if word(12)? != RECT_SUBMIT_RETURN { return None; }
    Stage::Ui.count(Metric::WalkExamined, 1);
    if word(16)? != 0 || word(20)? != 3 || word(24)? != RECT_VERTEX_COUNT
        || word(28)? != EFFECT_QUAD_BUILD || c.cpu.ebx != RECT_BYTES || c.cpu.edi != RECT_BYTES / 4
    {
        return geometry_refused();
    }
    let saved = [word(0)?, word(4)?, word(8)?];
    let next = frame.checked_add(36)?;
    let data = word(32)?;
    if data == 0 || data.checked_add(RECT_BYTES - 1).is_none() {
        return geometry_refused();
    }
    let primitive = mem.u32(PRIMITIVE_TABLE + 3 * 4).ok()?;
    if !matches!(primitive, 6..=8) {
        super::kept::count(super::kept::UiKind::Bar, "primitive");
        return None;
    }
    let bytes = mem.read::<{ RECT_BYTES as usize }>(data).ok()?.to_vec();
    let drawn = host().is_some_and(|host| host.inline_ui_quad(&mut *mem, primitive, bytes));
    record(drawn);
    if drawn {
        [c.cpu.edi, c.cpu.esi, c.cpu.ebx] = saved;
        c.cpu.eax = u32::MAX;
        c.cpu.esp = next;
        Some(TransferAbi::ret(RECT_SUBMIT_RETURN))
    } else {
        None
    }
}

fn tail_caller(mem: &dyn GuestMemory, stack: u32, caller: u32) -> bool {
    mem.u32(stack).ok() == Some(caller)
}

pub(super) fn place(b: &mut Builder<'_>) {
    super::backdrop::place(b);
    super::screen_draw::place(b);
    for (start, end, caller) in [
        (&TAIL_COPY, &TAIL_COPY_END, TAIL_COPY_CALLER),
        (&TAIL_COMPOSITE, &TAIL_COMPOSITE_END, TAIL_COMPOSITE_CALLER),
    ] {
        b.raw().before(start, "native_frame_tail_draw", move |c| {
            if c.ctx.runtime().read_dispatch(|d| d.has_generated(end.base)) {
                let mut mem = c.ctx.memory();
                if tail_caller(&*mem, c.cpu.esp, caller) && let Some(host) = host() {
                    host.enter_ui(&mut *mem, UiScope::FrameTail, c.cpu.esp, true);
                }
            }
            Ok(Flow::Continue)
        }).context_only().optional();
        b.raw().before(end, "native_frame_tail_draw_end", |c| {
            leave(c, UiScope::FrameTail);
            Ok(Flow::Continue)
        }).context_only().optional();
    }
    for (scope, start, end, alternate) in SCOPES {
        b.raw().before(start, "native_ui_enter", move |c| {
            // The fade made in Rust returns before its scope opens.
            if start.base == FADE_DRAW.base {
                return Ok(Flow::Return(super::fade::enter(c)));
            }
            Ok(enter(c, scope, end, alternate).map_or(Flow::Continue, Flow::Return))
        }).context_only().optional();
        for end in [Some(end), alternate].into_iter().flatten() {
            b.raw().before(end, "native_ui_leave", move |c| {
                leave(c, scope);
                Ok(Flow::Continue)
            }).context_only().optional();
        }
    }
    b.raw().before(&crate::native_particles::DYNAMIC_QUAD_READY, "native_ui_inline_rect", |c| {
        Ok(overlay_rect(c).map_or(Flow::Continue, Flow::Return))
    }).first().context_only().optional();
    b.raw().before(&LOADING_SPINNER_READY, "native_ui_loading_spinner", |c| {
        Ok(loading_fan(c, true).map_or(Flow::Continue, Flow::Return))
    }).context_only().optional();
    b.raw().before(&LOADING_BITMAP_READY, "native_ui_loading_bitmap", |c| {
        Ok(loading_fan(c, false).map_or(Flow::Continue, Flow::Return))
    }).context_only().optional();
    use guest_layout::points::render_aspect::{ANCHOR, GROUPS};
    b.raw().before(&ANCHOR, "native_hud_bitmap_anchor", |c| {
        // A bitmap widget's anchor id (EAX): the centre's is the reticle
        // (`kept::quad_kind`).
        if GuestMemory::u32(&*c.ctx.memory(), c.cpu.esp).ok() == Some(super::kept::BITMAP_ANCHOR_RETURN) {
            super::kept::set_widget_anchor(Some(c.cpu.eax));
        }
        Ok(Flow::Continue)
    }).context_only().after_feature("render-aspect").optional();
    let bitmap_end = GROUPS.iter().find(|(ret, _)| *ret == super::kept::BITMAP_ANCHOR_RETURN)
        .expect("bitmap anchor group");
    b.raw().before(&bitmap_end.1, "native_hud_bitmap_end", |_| {
        super::kept::set_widget_anchor(None);
        Ok(Flow::Continue)
    }).context_only().after_feature("render-aspect").optional();
    b.raw().before(&FADE_FAN_READY, "native_ui_fade_fan", |c| {
        Ok(fade_fan(c).map_or(Flow::Continue, Flow::Return))
    }).context_only().optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn only_the_frame_tail_caller_owns_each_shared_helper() {
        let mut mem = FakeGuest::new();
        let stack = 0x4100;
        for (caller, other) in [(TAIL_COPY_CALLER, TAIL_COMPOSITE_CALLER),
                                (TAIL_COMPOSITE_CALLER, TAIL_COPY_CALLER)] {
            mem.put_u32(stack, caller);
            assert!(tail_caller(&mem, stack, caller));
            assert!(!tail_caller(&mem, stack, other));
            assert!(!tail_caller(&mem, stack + 4, caller));
        }
    }

    #[test]
    fn the_widget_callback_must_match_the_live_order_entry() {
        let mut mem = FakeGuest::new();
        let frame = 0x3000;
        let first = frame - WIDGET_LIST_FROM_EBP;
        let item = first + WIDGET_ENTRY_BYTES;
        let widget = 0x4000;
        let vtable = 0x5000;
        let draw = 0x6000;
        mem.put_u32(frame - WIDGET_COUNT_FROM_EBP, 2)
            .put_u32(first, widget)
            .put_u32(item, widget)
            .put_u32(widget, vtable)
            .put_u32(vtable + WIDGET_DRAW_SLOT, draw);
        assert_eq!(widget_in_order(&mem, frame, 1, item, widget, vtable), Some(draw));
        assert_eq!(widget_in_order(&mem, frame, 1, first, widget, vtable), None);
        assert_eq!(widget_in_order(&mem, frame, 2, item, widget, vtable), None);
        assert_eq!(widget_in_order(&mem, frame, 1, item, widget, vtable + 4), None);
        mem.put_u32(frame - WIDGET_COUNT_FROM_EBP, MAX_WIDGETS + 1);
        assert_eq!(widget_in_order(&mem, frame, 1, item, widget, vtable), None);
    }

    #[test]
    fn fade_uses_signed_16_bit_components_in_the_setters_order() {
        let quad = fade_vertices(0x1234_fffe, 0xabcd_0014);
        assert_eq!(quad.map(|v| v[0].1), [
            [0.0, 0.0, 0.0, 1.0],
            [20.0, 0.0, 0.0, 1.0],
            [20.0, -2.0, 0.0, 1.0],
            [0.0, -2.0, 0.0, 1.0],
        ]);
    }

    #[test]
    fn spinner_reads_both_texture_registers_from_the_completed_frame() {
        let mut mem = FakeGuest::new();
        let frame = 0x1000;
        for (i, f) in [-2.0, 3.0, -5.0, 7.0].into_iter().enumerate() {
            mem.put_f32(frame + 0x18 + 4 * i as u32, f);
        }
        for i in 0..16u32 {
            mem.put_f32(frame + 0x28 + 4 * i, i as f32);
        }
        let fan = spinner_vertices(&mem, frame).unwrap();
        assert_eq!(fan[2][0], (1, [4.0, 5.0, 0.0, 1.0]));
        assert_eq!(fan[2][1], (2, [12.0, 13.0, 0.0, 1.0]));
        assert_eq!(&fan[2][2].1[..2], &[3.0, 7.0]);
        assert_eq!(&fan[3][2].1[..2], &[-2.0, 7.0]);
    }
}
