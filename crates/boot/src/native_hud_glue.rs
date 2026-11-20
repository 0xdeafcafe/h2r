//! Ordered handoff from the game's UI callbacks to the native UI renderer.
//! Direct producers keep the guest code as their fallback. A callback that
//! still issues geometry can retain its immutable packet as worker fallback.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws, ExternalPassRequest};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalWrites};
use engine::memory::GuestMemory;
use feature::native_hud::{Host, HudQuadCall, HudVertices, UiScope};
use gfx::vertex::{PrimitiveType, VertexDataType, VertexLayout, VertexRegister};
use native_render::camera::GuestRead;
use native_render::hud::{self, Batch, Expect, HudQuad, Kind, Pairing};
use native_render::ui_gpu::{Refusal, UiGpu};
use timeline::native::{Metric, Stage, Ui};

use crate::native_lights_glue::DeviceMemory;

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The positions of a four-vertex fan as the D3D path resolved it
/// (`ResolvedDraw`'s primitive, count, index presence, layout and
/// streams): attribute 0 of each vertex, its x and y. `None` for any
/// other draw, or a position that is not floats.
pub fn fan_corners(
    primitive: PrimitiveType,
    vertex_count: u32,
    indexed: bool,
    layout: &VertexLayout,
    streams: &[d3d::StreamBytes],
) -> Option<[[f32; 2]; 4]> {
    if primitive != PrimitiveType::TriangleFan || vertex_count != 4 || indexed {
        return None;
    }
    let (n, a) = layout.streams.iter().enumerate().find_map(|(n, s)| {
        s.attributes
            .iter()
            .find(|a| a.register == VertexRegister::POSITION)
            .map(|a| (n, a))
    })?;
    if !matches!(
        a.data_type,
        VertexDataType::Float2 | VertexDataType::Float3 | VertexDataType::Float4
    ) {
        return None;
    }
    let stream = streams.get(n)?;
    let bytes: &[u8] = &stream.data;
    let f = |at: usize| -> Option<f32> {
        let b = bytes.get(at..at + 4)?;
        Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = [[0.0f32; 2]; 4];
    for (i, c) in out.iter_mut().enumerate() {
        let at = i * stream.stride as usize + a.offset as usize;
        *c = [f(at)?, f(at + 4)?];
    }
    Some(out)
}

/// What the compare shares between the hooks and the admission.
struct Compare {
    pairing: Mutex<Pairing>,
    /// Quads still to queue.
    left: AtomicU64,
    /// The plan of the last batch begun, said beside the first resolved
    /// screen quad.
    plan: Mutex<String>,
    said: Mutex<[bool; 2]>,
}

impl Compare {
    fn expect(&self, e: Expect) {
        if self
            .left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            self.pairing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .expect(e);
        }
    }
}

impl d3d::DrawAdmission for Compare {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let d = snapshot.draw;
        let corners = fan_corners(
            d.primitive,
            d.vertex_count,
            d.indices.is_some(),
            &d.layout,
            &d.streams,
        )?;
        let mut p = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        if p.waiting() == 0 {
            return None;
        }
        let paired = p.draw(&corners)?;
        let total = p.paired[0] + p.paired[1];
        let (line, done) = (p.line(), p.waiting() == 0);
        std::mem::drop(p);
        let k = match paired.kind {
            Kind::Screen => 0,
            Kind::Hud => 1,
        };
        let first = {
            let mut said = self.said.lock().unwrap_or_else(|e| e.into_inner());
            !std::mem::replace(&mut said[k], true)
        };
        if first {
            let r = &d.state.render;
            eprintln!(
                "native-hud: compare: the first {:?} quad resolved as {} into {:#010x}, stage 0 {:?}, blend {} {:?} {:?} {:?}; plan: {}",
                paired.kind,
                d.entry,
                snapshot.color_header,
                d.texture_info[0],
                r.blend_enable,
                r.src_blend,
                r.dst_blend,
                r.blend_op,
                self.plan.lock().unwrap_or_else(|e| e.into_inner())
            );
        }
        if total % 64 == 0 || (done && self.left.load(Ordering::Relaxed) == 0) {
            eprintln!("native-hud: compare: {line}");
        }
        None
    }
}

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
        retain_packet: bool,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let draw = snapshot.draw;
        if snapshot.color_header == 0 {
            Ui::Target.count();
            Stage::Ui.count(Metric::RefusedResources, 1);
            return None;
        }
        Some((
            ExternalPlan {
                stage: Stage::Ui,
                view: 4,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(UiPass {
                draw: Arc::new(draw.clone()),
                gpu: self.gpu.clone(),
                frame,
                retain_packet,
            }),
        ))
    }
}

impl DrawAdmission for Admission {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let scope = SCOPES.with(|scopes| scopes.borrow().active(self.owner))?;
        if !scope.eligible {
            return None;
        }
        self.plan(snapshot, scope.frame, true)
    }
}

struct Direct<'a> {
    admission: &'a Admission,
    frame: u32,
}

impl DrawAdmission for Direct<'_> {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        self.admission.plan(snapshot, self.frame, false)
    }
}

struct UiPass {
    draw: Arc<d3d::ResolvedDraw>,
    gpu: Arc<Mutex<UiGpu>>,
    frame: u32,
    retain_packet: bool,
}

impl ExternalPass for UiPass {
    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let result = if request.surface_size != (self.draw.target.width, self.draw.target.height) {
            Err(Refusal::Target)
        } else {
            self.gpu.lock().unwrap_or_else(|e| e.into_inner()).prepare(target, &self.draw, self.frame)
        };
        Some(result.map_err(|reason| {
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

    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.retain_packet.then(|| self.draw.clone())
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
    compare: Option<Arc<Compare>>,
    device: d3d::abi::SharedDevice<B>,
    admission: Arc<Admission>,
}

impl<B: d3d::Backend> Glue<B> {
    fn fan<const N: usize>(
        &self,
        mem: &mut dyn GuestMemory,
        vertices: &[[(u8, [f32; 4]); N]; 4],
        pushed: bool,
    ) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            Ui::Reservation.count();
            return false;
        };
        if pushed && !inline_screen_space(mem, device.config().layout) {
            Ui::Order.count();
            return false;
        }
        let candidate = Direct { admission: &self.admission, frame: device.swap_count() };
        let mut memory = DeviceMemory(mem);
        let result = if pushed {
            device.try_native_pushed_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        } else {
            device.try_native_immediate_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        };
        let drawn = matches!(result, Ok(true));
        if !drawn {
            Ui::Reservation.count();
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

fn inline_screen_space(mem: &dyn GuestMemory, layout: d3d::layout::GuestLayout) -> bool {
    use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

    // Begin and End take two words each. Each vertex takes a two-word
    // packed colour and two three-word float2 writes. Avoid skipping a
    // push-buffer wrap or the guest's deferred fence on End.
    const FAN_BYTES: u32 = (2 + 4 * (2 + 3 + 3) + 2) * 4;
    const BEGIN_OR_FENCE_PENDING: u32 = 0x1800;
    mem.u32(layout.p_device).ok().is_some_and(|device| {
        device == layout.device
            && mem.u32(device + FLAGS).is_ok_and(|flags| flags & BEGIN_OR_FENCE_PENDING == 0)
            && mem.u32(device + PUT).ok()
                .and_then(|put| put.checked_add(FAN_BYTES))
                .is_some_and(|end| mem.u32(device + THRESHOLD).is_ok_and(|limit| end <= limit))
    })
}

impl<B: d3d::Backend + Send + 'static> Host for Glue<B> {
    fn draw_quad(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, false)
    }

    fn screen_fan(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, true)
    }

    fn fade_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        self.fan(mem, vertices, false)
    }

    fn loading_bitmap_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        self.fan(mem, vertices, false)
    }

    fn inline_ui_quad(&self, mem: &mut dyn GuestMemory, primitive: u32, bytes: Vec<u8>) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Some(primitive) = PrimitiveType::from_xbox(primitive) else { return false };
        let Ok(mut device) = self.device.lock() else { return false };
        let candidate = Direct { admission: &self.admission, frame: device.swap_count() };
        let drawn = device.try_native_inline_draw(
            &mut DeviceMemory(mem), primitive, bytes, &[None; 16], &candidate,
        ).ok() == Some(true);
        if !drawn {
            Ui::Reservation.count();
        }
        drawn
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

    fn begin(&self, mem: &mut dyn GuestMemory, record: u32) {
        let Some(compare) = &self.compare else { return };
        if compare.left.load(Ordering::Relaxed) == 0 { return; }
        *compare.plan.lock().unwrap_or_else(|e| e.into_inner()) = match Batch::read(&Ro(mem), record) {
            Ok(batch) => format!(
                "batch at {record:#010x} over {:#010x} ({}x{}, linear {}), {:?}",
                batch.bitmap.at, batch.bitmap.width, batch.bitmap.height, batch.bitmap.linear, batch.blend
            ),
            Err(reason) => format!("batch at {record:#010x}: {}", reason.describe()),
        };
    }

    fn screen_quad(&self, mem: &mut dyn GuestMemory, corners: u32) -> bool {
        if let Some(compare) = &self.compare
            && mem.u16(hud::QUADS_OFF).ok() == Some(0)
            && let Some(quad) = hud::read_quad(&Ro(mem), corners)
        {
            compare.expect(Expect { kind: Kind::Screen, corners: quad.map(|v| [v.x, v.y]) });
        }
        false
    }

    fn end(&self, _mem: &mut dyn GuestMemory) {}

    fn hud_quad(&self, mem: &mut dyn GuestMemory, call: HudQuadCall) {
        if let Some(compare) = &self.compare
            && let Some(quad) = HudQuad::read(&Ro(mem), call.corners, call.rect, call.args)
        {
            compare.expect(Expect { kind: Kind::Hud, corners: quad.corners });
        }
    }
}

pub(crate) fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let switches = feature::Switches::from_env().unwrap_or_default();
    let compare = feature::native_hud::compare_on(&switches).map(|n| {
        Arc::new(Compare {
            pairing: Mutex::new(Pairing::default()),
            left: AtomicU64::new(n),
            plan: Mutex::new(String::new()),
            said: Mutex::new([false; 2]),
        })
    });
    if !feature::native_hud::on(&switches) && compare.is_none() {
        return;
    }
    static OWNER: AtomicU64 = AtomicU64::new(0);
    let admission = Arc::new(Admission {
        owner: OWNER.fetch_add(1, Ordering::Relaxed),
        gpu: Arc::new(Mutex::new(UiGpu::default())),
    });
    if let Ok(mut device) = device.lock() {
        if let Some(compare) = &compare {
            device.add_draw_admission(compare.clone());
        } else {
            device.add_draw_admission(admission.clone());
        }
    }
    feature::native_hud::set_host(Arc::new(Glue {
        compare,
        device: device.clone(),
        admission,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

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
    fn ui_inline_fan_keeps_the_guest_wrap_or_deferred_fence() {
        use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

        let layout = d3d::GuestLayout::HALO2;
        let mut mem = FakeGuest::new();
        let put = 0x8000;
        mem.put_u32(layout.p_device, layout.device)
            .put_u32(layout.device + FLAGS, 0)
            .put_u32(layout.device + PUT, put)
            .put_u32(layout.device + THRESHOLD, put + 144);
        assert!(inline_screen_space(&mem, layout));
        mem.put_u32(layout.device + THRESHOLD, put + 128);
        assert!(!inline_screen_space(&mem, layout));
        mem.put_u32(layout.device + THRESHOLD, put + 4096)
            .put_u32(layout.device + FLAGS, 0x1000);
        assert!(!inline_screen_space(&mem, layout));
    }
}
