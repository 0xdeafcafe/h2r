//! Ordered handoff from the game's UI callbacks to the native UI renderer.
//! A single-draw pass retains its immutable packet as worker fallback, so a
//! direct producer commits without waiting for the render thread. A model
//! draw with sibling ranges keeps the guest code as its fallback; a kept
//! scene carries it as its head and sibling packets under one admission.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws, ExternalPassRequest, IndexedStreams};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalWrites};
use engine::memory::GuestMemory;
use render_bridge::native_hud::{
    Host, HudTagQuad, HudVertices, OUTLINE_PUSH_BYTES, OutlineVertices, UiScope, kept,
};
use gfx::vertex::PrimitiveType;
use native_render::ui_gpu::{Refusal, UiGpu};
use timeline::native::{Metric, Stage, Ui};

use crate::hud_overlay::ScreenDraw;
use crate::native_lights_glue::DeviceMemory;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scope {
    owner: u64,
    kind: UiScope,
    stack: u32,
    frame: u32,
    eligible: bool,
}

#[derive(Default)]
struct ScopeStack(Vec<Scope>);

impl ScopeStack {
    fn enter(&mut self, mut scope: Scope) {
        // The guest stack grows down. Discard any scope bypassed by an
        // unwind before inheriting ownership at the new callback.
        self.0.retain(|old| old.owner == scope.owner && old.stack > scope.stack);
        scope.eligible &= self.permits(scope.owner);
        self.0.push(scope);
    }

    fn active(&self, owner: u64) -> Option<Scope> {
        self.0.last().copied().filter(|scope| scope.owner == owner)
    }

    fn permits(&self, owner: u64) -> bool {
        self.active(owner).is_none_or(|scope| scope.eligible)
    }

    fn screen_source(&self, owner: u64) -> Option<ScreenDraw> {
        match self.active(owner).filter(|scope| scope.eligible)?.kind {
            UiScope::Hud | UiScope::Layers | UiScope::Widget => Some(ScreenDraw::Widget),
            UiScope::Loading => Some(ScreenDraw::Loading),
            UiScope::Overlay | UiScope::FrameTail => None,
        }
    }

    fn position(&self, scope: Scope) -> Option<usize> {
        self.0.iter().rposition(|s| {
            (s.owner, s.kind, s.stack) == (scope.owner, scope.kind, scope.stack)
        })
    }
}

thread_local! {
    static SCOPES: RefCell<ScopeStack> = RefCell::new(ScopeStack::default());
}

struct Admission {
    owner: u64,
    gpu: Arc<Mutex<UiGpu>>,
}

impl Admission {
    fn plan(
        &self,
        snapshot: &DrawSnapshot<'_>,
        frame: u32,
        stage: Stage,
        siblings: Arc<[IndexedStreams]>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let draw = snapshot.draw;
        if snapshot.color_header == 0 {
            Ui::Target.count();
            stage.count(Metric::RefusedResources, 1);
            return None;
        }
        Some((
            ExternalPlan {
                stage,
                view: 4,
                ranges: 1 + siblings.len(),
                slots: Vec::new(),
            },
            Box::new(UiPass {
                draw: Arc::new(draw.clone()),
                gpu: self.gpu.clone(),
                frame,
                siblings,
            }),
        ))
    }
}

// Both frame-tail helpers draw through the pushed fan in 0x00034420.
fn tail_fan(entry: &str, primitive: PrimitiveType, vertices: u32) -> bool {
    entry == "inline Begin/End" && primitive == PrimitiveType::TriangleFan && vertices == 4
}

impl DrawAdmission for Admission {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let scope = SCOPES.with(|scopes| scopes.borrow().active(self.owner))?;
        if scope.kind == UiScope::FrameTail {
            let draw = snapshot.draw;
            if scope.eligible && tail_fan(draw.entry, draw.primitive, draw.vertex_count) {
                return self.plan(snapshot, scope.frame, Stage::Post, Vec::new().into());
            }
            return None;
        }
        Ui::GuestBuilderDraw.count();
        let planned = if scope.eligible { self.plan(snapshot, scope.frame, Stage::Ui, Vec::new().into()) } else { None };
        // A HUD draw no producer offered first is `untraced`
        // (`kept::untraced`); a kept item's own draw is not counted again.
        if scope.kind == UiScope::Hud {
            render_bridge::native_hud::kept::untraced(if planned.is_some() { "packet" } else { "scope" });
        }
        planned
    }
}

struct Direct<'a> {
    admission: &'a Admission,
    frame: u32,
    siblings: Arc<[IndexedStreams]>,
}

impl<'a> Direct<'a> {
    fn new(admission: &'a Admission, frame: u32) -> Self {
        Self { admission, frame, siblings: Vec::new().into() }
    }
}

impl DrawAdmission for Direct<'_> {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        self.admission.plan(snapshot, self.frame, Stage::Ui, self.siblings.clone())
    }
}

/// A sibling range drawn as `head` is: the head's state, textures and
/// constants with the range's own geometry.
fn sibling_draw(head: &d3d::ResolvedDraw, sibling: &IndexedStreams) -> d3d::ResolvedDraw {
    let mut draw = head.clone();
    draw.layout = sibling.layout.clone();
    draw.vertex_count = sibling.vertex_count;
    draw.streams = sibling.streams.clone();
    draw.indices = Some(sibling.indices.clone());
    draw
}

struct UiPass {
    draw: Arc<d3d::ResolvedDraw>,
    gpu: Arc<Mutex<UiGpu>>,
    frame: u32,
    siblings: Arc<[IndexedStreams]>,
}

impl UiPass {
    fn prepare(&self, target: &ExternalAttachments, request: ExternalPassRequest)
        -> Result<ExternalDraws, Refusal>
    {
        if request.surface_size != (self.draw.target.width, self.draw.target.height) {
            return Err(Refusal::Target);
        }
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        let copy = self.draw.for_copy(target.copy);
        let mut total = gpu.prepare(target, copy.as_ref(), self.frame)?;
        for sibling in self.siblings.iter() {
            let draw = sibling_draw(&copy, sibling);
            let part = gpu.prepare(target, &draw, self.frame)?;
            total.ranges += part.ranges;
            total.packets.extend(part.packets);
            total.discards |= part.discards;
        }
        Ok(total)
    }
}

impl ExternalPass for UiPass {
    // The packet and the issue frame are immutable; `prepare` takes the
    // attachments and sampled stages from the copy being recorded.
    fn replays_scene(&self) -> bool {
        true
    }

    fn requires_independent_samples(&self) -> bool {
        !self.siblings.is_empty()
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        Some(self.prepare(target, request).map_err(|reason| {
            match reason {
                Refusal::Geometry => Ui::Geometry,
                Refusal::Target => Ui::Target,
                Refusal::State => Ui::State,
                Refusal::Shader => Ui::Shader,
                Refusal::Texture => Ui::Texture,
                Refusal::Upload => Ui::Upload,
            }.count();
            "native UI draw"
        }))
    }

    /// The worker's fallback must be the whole guest draw: with sibling
    /// ranges there is no single one, and the guest code stays the fallback.
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.siblings.is_empty().then(|| Arc::clone(&self.draw))
    }

    /// The head draw, then each sibling range as the draw `prepare` builds
    /// for it (the head's state and textures, the sibling's geometry): a
    /// kept scene carries the batch as these draws under one admission.
    fn resolved_packets(&self) -> Vec<Arc<d3d::ResolvedDraw>> {
        std::iter::once(Arc::clone(&self.draw))
            .chain(self.siblings.iter().map(|sibling| Arc::new(sibling_draw(&self.draw, sibling))))
            .collect()
    }

    fn diagnostic_draw(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        Some(self.draw.clone())
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: !gfx::translate::color_writes(self.draw.state.render.color_write).is_empty(),
            depth_stencil: self.draw.writes_depth_stencil(),
        }
    }

    fn colour_only(&self) -> bool {
        self.draw.target.depth_format.is_none()
    }
}

struct Glue<B: d3d::Backend> {
    device: d3d::abi::SharedDevice<B>,
    admission: Arc<Admission>,
}

impl<B: d3d::Backend> Glue<B> {
    fn fan<const N: usize>(
        &self,
        mem: &mut dyn GuestMemory,
        vertices: &[[(u8, [f32; 4]); N]; 4],
        push_bytes: Option<u32>,
    ) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            Ui::Reservation.count();
            return false;
        };
        if push_bytes.is_some_and(|bytes| !inline_space(mem, device.config().layout, bytes, false)) {
            Ui::Order.count();
            return false;
        }
        let candidate = Direct::new(&self.admission, device.swap_count());
        let mut memory = DeviceMemory(mem);
        let result = if push_bytes.is_some() {
            device.try_native_pushed_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        } else {
            device.try_native_immediate_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        };
        let drawn = matches!(result, Ok(true));
        if drawn {
            // The guest's resolved packet, not the overlay: a HUD item
            // drawn so is counted `packet` (`kept::carried`).
            render_bridge::native_hud::kept::carried();
        } else {
            Ui::Reservation.count();
            render_bridge::native_hud::kept::refuse("reservation");
        }
        drawn
    }

    /// A screen draw from the overlay (`hud_overlay::draw_screen`): its
    /// immediate fan, with the guest's combiner and blend at the draw.
    fn screen_overlay<const N: usize>(
        &self,
        mem: &mut dyn GuestMemory,
        what: ScreenDraw,
        vertices: &[[(u8, [f32; 4]); N]; 4],
    ) -> bool
    where
        B: Send + 'static,
    {
        if !self.permits_draw() {
            kept::refuse("order");
            return false;
        }
        crate::hud_overlay::draw_screen(&self.device, mem, what, |d, m, a| {
            d.try_native_immediate_draw(m, PrimitiveType::TriangleFan, vertices, a)
        })
    }

    /// A menu-frame screen draw of `what`, offered by `draw` (the overlay,
    /// then the packet path): kept, it counts `scene_kept_ui_<kind>_<cause>`
    /// under the first cause named (`packet` when the packet path drew
    /// it). In the HUD, and under an offer already open, the producer's
    /// item is the one counted.
    fn offer(&self, what: ScreenDraw, draw: impl FnOnce() -> bool) -> bool {
        let offered = !kept::in_hud() && kept::begin_ui(what.kind());
        let drawn = draw();
        if offered {
            kept::end(drawn);
        }
        drawn
    }

    /// A bar of `0x00052ce7` on the overlay, with the combiner the last
    /// record with an image left (`0x00052857` writes none for a record
    /// without); the capture takes a fan's, a quad's, a polygon's and a
    /// strip's corners round the quad (`hud_overlay::corner_order`). Else
    /// the packet path.
    fn inline_bar(&self, mem: &mut dyn GuestMemory, primitive: u32, bytes: Vec<u8>) -> bool
    where
        B: Send + 'static,
    {
        if !self.permits_draw() {
            kept::refuse("order");
            return false;
        }
        let Some(primitive) = PrimitiveType::from_xbox(primitive) else {
            kept::refuse("primitive");
            return false;
        };
        let carried = [None; 16];
        if crate::hud_overlay::draw_screen(&self.device, mem, ScreenDraw::Bar, |d, m, a| {
            d.try_native_inline_draw(m, primitive, bytes.clone(), &carried, a)
        }) {
            return true;
        }
        let Ok(mut device) = self.device.lock() else { return false };
        let candidate = Direct::new(&self.admission, device.swap_count());
        let drawn = device.try_native_inline_draw(
            &mut DeviceMemory(mem), primitive, bytes, &[None; 16], &candidate,
        ).ok() == Some(true);
        if drawn {
            kept::carried();
        } else {
            Ui::Reservation.count();
            kept::refuse("reservation");
        }
        drawn
    }

    fn permits_draw(&self) -> bool {
        SCOPES.with(|scopes| scopes.borrow().permits(self.admission.owner))
    }

    fn scope(&self, kind: UiScope, stack: u32, frame: u32, eligible: bool) -> Scope {
        Scope { owner: self.admission.owner, kind, stack, frame, eligible }
    }
}

// Begin and End take two words each. Each vertex takes a two-word
// packed colour and two three-word float2 writes.
const SCREEN_FAN_PUSH_BYTES: u32 = (2 + 4 * (2 + 3 + 3) + 2) * 4;

fn inline_space(mem: &dyn GuestMemory, layout: d3d::layout::GuestLayout, bytes: u32, open: bool) -> bool {
    use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

    const BEGIN: u32 = 0x800;
    const FENCE_PENDING: u32 = 0x1000;
    let expected = if open { BEGIN } else { 0 };
    mem.u32(layout.p_device).ok().is_some_and(|device| {
        device == layout.device
            && mem.u32(device + FLAGS).is_ok_and(|flags| flags & (BEGIN | FENCE_PENDING) == expected)
            && mem.u32(device + PUT).ok()
                .and_then(|put| put.checked_add(bytes))
                .is_some_and(|end| mem.u32(device + THRESHOLD).is_ok_and(|limit| end <= limit))
    })
}

impl<B: d3d::Backend + Send + 'static> Host for Glue<B> {
    fn draw_tag_quad(&self, mem: &mut dyn GuestMemory, at: &HudTagQuad, vertices: &HudVertices) -> bool {
        self.permits_draw() && crate::hud_overlay::draw_tag_quad(&self.device, mem, at, vertices)
    }

    fn rust_hud_quad(&self, mem: &mut dyn GuestMemory, at: &HudTagQuad, vertices: &HudVertices) -> bool {
        self.permits_draw() && crate::hud_native::draw(&self.device, mem, at, vertices)
    }

    fn rust_hud_widget(&self, mem: &mut dyn GuestMemory, widget: &render_bridge::native_hud::WalkWidget) -> bool {
        self.permits_draw() && crate::hud_native::draw_widget(&self.device, mem, widget)
    }

    fn rust_hud_text(&self, mem: &mut dyn GuestMemory, text: &render_bridge::native_hud::WalkText) -> bool {
        self.permits_draw() && crate::hud_native::draw_text(&self.device, mem, text)
    }

    fn rust_string(&self, mem: &mut dyn GuestMemory, run: &render_bridge::native_hud::strings::StringRun) -> bool {
        self.permits_draw() && crate::hud_native::draw_string(&self.device, mem, run)
    }

    fn draw_quad(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        let source = SCOPES.with(|s| s.borrow().screen_source(self.admission.owner));
        self.offer(source.unwrap_or(ScreenDraw::Widget), || {
            let overlay = match source {
                Some(source) => self.screen_overlay(mem, source, vertices),
                // No overlay scope names this draw.
                None => {
                    kept::refuse("scope");
                    false
                }
            };
            overlay || self.fan(mem, vertices, None)
        })
    }

    fn draw_text_quad(&self, mem: &mut dyn GuestMemory, shader: u32, vertices: &HudVertices) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Some(layout) = self.device.lock().ok().map(|device| device.config().layout) else {
            return false;
        };
        if !inline_space(mem, layout, SCREEN_FAN_PUSH_BYTES, false) {
            return false;
        }
        let cache = || -> Option<_> {
            let bitmap = mem.u32(d3d::glyph::GLYPH_CACHE_BITMAP).ok()?;
            let texture = mem.u32(bitmap.checked_add(d3d::glyph::BITMAP_TEXTURE)?).ok()?;
            if texture == 0 {
                return None;
            }
            let hud_input = render_bridge::native_hud::hud_overlay::inputs(mem)?;
            Some(HudTagQuad {
                bitmap: texture, image: 0, shader, hud_input, widget: false,
            })
        };
        cache().is_some_and(|at| crate::hud_overlay::draw_text_quad(&self.device, mem, &at, vertices))
    }

    fn screen_fan(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        let source = SCOPES.with(|s| s.borrow().screen_source(self.admission.owner));
        self.offer(source.unwrap_or(ScreenDraw::Widget), || {
            if let Some(source) = source {
                let layout = self.device.lock().ok().map(|device| device.config().layout);
                if !layout.is_some_and(|layout| inline_space(mem, layout, SCREEN_FAN_PUSH_BYTES, false)) {
                    kept::refuse("push");
                } else if crate::hud_overlay::draw_screen(&self.device, mem, source, |d, m, a| {
                    d.try_native_pushed_draw(m, PrimitiveType::TriangleFan, vertices, a)
                }) {
                    return true;
                }
            } else {
                kept::refuse("scope");
            }
            self.fan(mem, vertices, Some(SCREEN_FAN_PUSH_BYTES))
        })
    }

    fn fade_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        self.offer(ScreenDraw::Fade, || {
            self.screen_overlay(mem, ScreenDraw::Fade, vertices) || self.fan(mem, vertices, None)
        })
    }

    fn loading_bitmap_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        self.offer(ScreenDraw::Loading, || {
            self.screen_overlay(mem, ScreenDraw::Loading, vertices) || self.fan(mem, vertices, None)
        })
    }

    fn loading_spinner(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.offer(ScreenDraw::Loading, || {
            self.screen_overlay(mem, ScreenDraw::Loading, vertices) || self.fan(mem, vertices, None)
        })
    }

    fn backdrop_wash(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, Some(engine::ui_backdrop::WASH_PUSH_BYTES))
    }

    fn backdrop_composite(
        &self,
        mem: &mut dyn GuestMemory,
        vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        self.fan(mem, vertices, None)
    }

    fn sensor_blip(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        // The body's quad is a four-vertex fan's two triangles.
        self.fan(mem, vertices, Some(engine::hud_sensor::BLIP_PUSH_BYTES))
    }

    fn outline(&self, mem: &mut dyn GuestMemory, vertices: &OutlineVertices) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            return false;
        };
        if !inline_space(mem, device.config().layout, OUTLINE_PUSH_BYTES, true) {
            Ui::Order.count();
            return false;
        }
        let candidate = Direct::new(&self.admission, device.swap_count());
        device.try_native_open_pushed_draw(
            &mut DeviceMemory(mem), PrimitiveType::LineStrip, vertices, &candidate,
        ).ok() == Some(true)
    }

    fn inline_ui_quad(&self, mem: &mut dyn GuestMemory, primitive: u32, bytes: Vec<u8>) -> bool {
        self.offer(ScreenDraw::Bar, || self.inline_bar(mem, primitive, bytes))
    }

    fn replace_widget(&self, mem: &mut dyn GuestMemory, widget: u32) -> bool {
        self.permits_draw() && crate::skulls_model::gpu::replace_widget(&self.device, mem, widget)
    }

    fn model_view(
        &self,
        mem: &mut dyn GuestMemory,
        bounds: [u16; 4],
        view: &frame_scene::OverlayModelView,
    ) {
        if self.permits_draw() {
            crate::monitor_model::gpu::appearance_view(&self.device, mem, bounds, view);
        }
    }

    fn ui_overlay(&self, _mem: &mut dyn GuestMemory) -> Option<bool> {
        // The UI scope's order must hold for a pass placed in it.
        Some(self.permits_draw())
    }

    fn ui_text(&self, _mem: &dyn GuestMemory) -> Option<Box<dyn render_bridge::native_hud::ui_overlay::UiText>> {
        Some(Box::new(crate::hud_overlay::menu_text::MenuText(crate::hud_overlay::fonts()?)))
    }

    fn ui_custom_widget(&self, mem: &dyn GuestMemory, widget: u32) -> bool {
        widget.checked_add(crate::list_class::WIDGET_BLOCK)
            .and_then(|addr| mem.u32(addr).ok())
            .is_some_and(|block| crate::skulls_model::noted(widget, block).is_some())
    }

    fn ui_list_arrows(&self, vtable: u32, height: i32) -> Option<[[i16; 2]; 2]> {
        crate::list_class::arrows_at(vtable, height).map(|a| [a.up, a.down])
    }

    fn ui_pass(&self, mem: &mut dyn GuestMemory, part: &Arc<frame_scene::Overlay>) -> bool {
        if !self.permits_draw() {
            unimplemented!("menus: pass outside its UI scope");
        }
        if !crate::hud_overlay::place_menu(&self.device, mem, part) {
            unimplemented!("menus: pass: the device did not take it");
        }
        true
    }

    fn modal_blur(&self, mem: &mut dyn GuestMemory, call: &render_bridge::native_hud::modal_blur::ModalBlur) -> bool {
        // The UI scope's order must hold for a pass placed in it.
        self.permits_draw() && crate::modal_blur_glue::arm(&self.device, mem, call)
    }

    fn finish_modal_blur(&self, mem: &mut dyn GuestMemory, esp: u32) -> bool {
        crate::modal_blur_glue::finish(&self.device, mem, esp)
    }

    fn enter_ui(&self, mem: &mut dyn GuestMemory, kind: UiScope, stack: u32, eligible: bool) {
        let Ok(mut device) = self.device.lock() else {
            Ui::Order.count();
            return;
        };
        // Work before the callback belongs to its prior producer.
        let ordered = device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() == Some(true);
        if !ordered {
            Ui::Order.count();
        }
        let scope = self.scope(kind, stack, device.swap_count(), ordered && eligible);
        SCOPES.with(|scopes| scopes.borrow_mut().enter(scope));
    }

    fn leave_ui(&self, mem: &mut dyn GuestMemory, kind: UiScope, stack: u32) {
        let scope = self.scope(kind, stack, 0, false);
        let Some(index) = SCOPES.with(|scopes| scopes.borrow().position(scope)) else { return };
        if !self.device.lock().is_ok_and(|mut device| {
            device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() == Some(true)
        }) {
            Ui::Order.count();
        }
        SCOPES.with(|scopes| scopes.borrow_mut().0.truncate(index));
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_hud::on(&switches) {
        return;
    }
    static OWNER: AtomicU64 = AtomicU64::new(0);
    let admission = Arc::new(Admission {
        owner: OWNER.fetch_add(1, Ordering::Relaxed),
        gpu: Arc::new(Mutex::new(UiGpu::default())),
    });
    if let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
    }
    render_bridge::native_hud::set_host(Arc::new(Glue {
        device: device.clone(),
        admission,
    }));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn frame_tail_admits_only_the_pushed_screen_fan() {
        assert!(tail_fan("inline Begin/End", PrimitiveType::TriangleFan, 4));
        assert!(!tail_fan("D3DDevice_End", PrimitiveType::TriangleFan, 4));
        assert!(!tail_fan("inline Begin/End", PrimitiveType::TriangleStrip, 4));
        assert!(!tail_fan("inline Begin/End", PrimitiveType::TriangleFan, 3));
    }

    #[test]
    fn ui_scope_pairs_require_the_same_callback_and_stack() {
        let mut scopes = ScopeStack::default();
        let layers = Scope { owner: 1, kind: UiScope::Layers, stack: 0x2000, frame: 2, eligible: true };
        let widget = Scope { kind: UiScope::Widget, stack: 0x1000, ..layers };
        scopes.enter(layers);
        scopes.enter(widget);
        assert_eq!(scopes.active(1), Some(widget));
        assert_eq!(scopes.active(2), None);
        assert_eq!(scopes.position(Scope { stack: 0x1004, ..widget }), None);
        assert_eq!(scopes.position(Scope { frame: 0, ..widget }), Some(1));
        let nested = Scope { kind: UiScope::Layers, stack: 0x800, eligible: false, ..layers };
        scopes.enter(nested);
        assert!(!scopes.permits(1));
        let child = Scope { stack: 0x700, ..widget };
        scopes.enter(child);
        assert!(!scopes.permits(1));
        let next_widget = Scope { stack: 0x1100, ..widget };
        scopes.enter(next_widget);
        assert_eq!(scopes.position(nested), None);
        assert_eq!(scopes.position(widget), None);
        assert_eq!(scopes.active(1), Some(next_widget));
        assert!(scopes.permits(1));
        scopes.0.truncate(scopes.position(next_widget).unwrap());
        assert_eq!(scopes.active(1), Some(layers));
    }

    #[test]
    fn screen_source_tracks_the_innermost_eligible_producer() {
        let mut scopes = ScopeStack::default();
        let hud = Scope { owner: 1, kind: UiScope::Hud, stack: 0x9000, frame: 1, eligible: true };
        assert_eq!(scopes.screen_source(1), None);
        scopes.enter(hud);
        assert_eq!(scopes.screen_source(1), Some(ScreenDraw::Widget));
        assert_eq!(scopes.screen_source(2), None);
        for (kind, expected) in [
            (UiScope::Layers, Some(ScreenDraw::Widget)),
            (UiScope::Widget, Some(ScreenDraw::Widget)),
            (UiScope::Overlay, None),
            (UiScope::Loading, Some(ScreenDraw::Loading)),
        ] {
            scopes.enter(Scope { kind, stack: 0x8000, ..hud });
            assert_eq!(scopes.screen_source(1), expected);
        }
        scopes.enter(Scope { eligible: false, ..hud });
        scopes.enter(Scope { kind: UiScope::Widget, stack: 0x8000, ..hud });
        assert_eq!(scopes.screen_source(1), None);
    }

    #[test]
    fn loading_resumes_after_a_nested_fade_or_bar() {
        let mut scopes = ScopeStack::default();
        let loading = Scope { owner: 5, kind: UiScope::Loading, stack: 0x9000, frame: 1, eligible: true };
        scopes.enter(loading);
        let child = Scope { kind: UiScope::Overlay, stack: 0x8000, ..loading };
        scopes.enter(child);
        assert_eq!(scopes.screen_source(5), None);
        scopes.0.truncate(scopes.position(child).unwrap());
        assert_eq!(scopes.screen_source(5), Some(ScreenDraw::Loading));
        scopes.enter(Scope { eligible: false, ..loading });
        assert_eq!(scopes.screen_source(5), None);
    }

    #[test]
    fn ui_inline_fan_keeps_the_guest_wrap_or_deferred_fence() {
        use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

        let layout = d3d::GuestLayout::HALO2;
        let mut mem = FakeGuest::new();
        let put = 0x8000;
        mem.put_u32(layout.p_device, layout.device)
            .put_u32(layout.device + FLAGS, 0)
            .put_u32(layout.device + PUT, put)
            .put_u32(layout.device + THRESHOLD, put + 144);
        assert!(inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + THRESHOLD, put + 128);
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + THRESHOLD, put + 4096)
            .put_u32(layout.device + FLAGS, 0x1000);
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + FLAGS, 0x0800);
        assert!(inline_space(&mem, layout, OUTLINE_PUSH_BYTES, true));
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + FLAGS, 0x1800);
        assert!(!inline_space(&mem, layout, OUTLINE_PUSH_BYTES, true));
    }

    /// A one-triangle UI draw, as the scene's UI pass is built over (the
    /// HUD overlay's widget tests share it).
    pub(crate) fn ui_draw() -> d3d::ResolvedDraw {
        use d3d::backend::{StreamBytes, TargetInfo, Viewport};
        use d3d::shader::{ShaderInputs, VertexProcessing};
        use d3d::state::StateWords;
        use gfx::vertex::{StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister};
        let words = StateWords::defaults(false);
        d3d::ResolvedDraw {
            entry: "D3DDevice_DrawVerticesUP",
            primitive: PrimitiveType::TriangleList,
            vertex_count: 3,
            layout: VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    stride: 8,
                    attributes: vec![VertexAttribute { register: VertexRegister(0), data_type: VertexDataType::Short4, offset: 0 }],
                }],
                pretransformed: false,
            },
            streams: vec![StreamBytes { data: vec![0u8; 24].into(), stride: 8 }],
            indices: None,
            state: words.translate(&gfx::xdk::RenderStateTable::default()),
            extras: words.stage_extras(),
            raw: Arc::new(words),
            target: TargetInfo { width: 64, height: 64, format: gfx::D3DFormat::A8R8G8B8, depth_format: None },
            shader: ShaderInputs {
                vertex: VertexProcessing::Program { start: 0, microcode: Vec::<u32>::new().into() },
                provided_inputs: 1,
                vertex_defaults: [[0., 0., 0., 1.]; 16],
                constants: Arc::new([[0.; 4]; 192]),
                pixel_shader: Some(Arc::new([0; 60])),
                combiner_state: Arc::new([0; 57]),
                final_combiner_latch: true,
                affine_projection: false,
            },
            viewport: Viewport { x: 0, y: 0, width: 64, height: 64, min_z: 0., max_z: 1. },
            clip: Some(d3d::PixelRect { x: 0, y: 0, width: 64, height: 64 }),
            clip_rects: Vec::new(),
            hw: Default::default(),
            texture_info: [None; 4],
            textures: [None; 4],
            retail_constants: None,
            ui_fit: None,
            glyph_cache: 0,
        }
    }

    #[test]
    fn a_single_draw_ui_pass_replays_in_a_kept_scene_from_its_packet() {
        let pass = UiPass {
            draw: Arc::new(ui_draw()),
            gpu: Arc::new(Mutex::new(UiGpu::default())),
            frame: 1,
            siblings: Arc::from(Vec::new()),
        };
        assert!(pass.replays_scene(), "the sink's retain_admission requires it");
        let packet = pass.resolved_fallback().expect("the packet the sink compares and replays");
        assert!(Arc::ptr_eq(&packet, &pass.draw));
        assert!(pass.colour_only());
    }

    #[test]
    fn a_ui_pass_with_siblings_is_carried_as_its_packets_under_one_admission() {
        let sibling = || IndexedStreams {
            layout: ui_draw().layout,
            vertex_count: 3,
            streams: Vec::new(),
            indices: vec![0u32, 1, 2].into(),
        };
        let pass = UiPass {
            draw: Arc::new(ui_draw()),
            gpu: Arc::new(Mutex::new(UiGpu::default())),
            frame: 1,
            siblings: Arc::from(vec![sibling(), sibling()]),
        };
        assert!(pass.replays_scene());
        assert!(pass.resolved_fallback().is_none(), "no single packet is the worker's fallback");
        let packets = pass.resolved_packets();
        assert_eq!(packets.len(), 3, "the head and each sibling, in order");
        assert!(Arc::ptr_eq(&packets[0], &pass.draw));
        for packet in &packets[1..] {
            assert_eq!(packet.indices.as_deref(), Some(&[0u32, 1, 2][..]));
            assert!(packet.streams.is_empty());
            assert_eq!(packet.state, pass.draw.state, "the head's state");
            assert_eq!(packet.textures, pass.draw.textures, "the head's textures");
        }
    }
}
