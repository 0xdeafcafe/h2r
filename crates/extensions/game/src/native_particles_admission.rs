//! Exact producer scopes bound to D3D's transactional DrawAdmission.
//! Both helpers publish one packet after their final producer work. Drain
//! before that store and at its epilogue; nothing can escape as "next quad".
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::{
    DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalTarget, ExternalWrites,
};
use d3d::backend::{ExternalAttachments, ExternalDraws};
use engine::memory::GuestMemory;
use render_bridge::effect_materials::Kind;
use render_bridge::native_particles::{Call, Host, Producer};
use gfx::vertex::{PrimitiveType, VertexDataType};
use native_render::effect_geometry::{self, DynamicCall};
use native_render::effect_immediate::{self, Site};
use native_render::flares;
use native_render::particle_gpu::*;
use native_render::particles::{self, DrawIdentity, Vertex};

use crate::native_lights_glue::DeviceMemory;
use timeline::native::{Metric, Stage};

mod flare_pass;
mod material_proof;
pub(crate) mod mirror_host;
mod refusal_samples;
mod particle_capture;

#[derive(Clone)]
enum Source {
    Particle(Call),
    Dynamic(DynamicCall, Option<Arc<[u8]>>),
    Immediate(Site),
}
impl Source {
    fn producer(&self) -> Producer {
        match self {
            Self::Particle(_) => Producer::Particle,
            Self::Dynamic(..) => Producer::Dynamic,
            Self::Immediate(site) => Producer::Immediate(*site),
        }
    }
}
static REFUSALS: crate::native_refusals::Counts = crate::native_refusals::Counts::new();

/// The first refused draw's shape per geometry reason, for
/// `render.particles.refusals`.
static SAMPLES: refusal_samples::Samples = refusal_samples::Samples::new();

/// `render.particles.refusals` (docs/runtime/agent-api.md): the native
/// particle path's geometry refusals, each reason with its count and the
/// first refused draw's shape.
pub fn add_refusals_method(agent: &agent::Agent) {
    agent.add_method(agent_proto::method::RENDER_PARTICLES_REFUSALS, agent::Access::Read, |params| {
        refusal_samples::answer(&params, &SAMPLES)
    });
}

pub(crate) fn report_refusals() {
    REFUSALS.report("native-particles");
}

fn producer_label(producer: Producer) -> &'static str {
    match producer {
        Producer::Particle => "Particle",
        Producer::Dynamic => "Dynamic",
        Producer::Immediate(_) => "Immediate",
        Producer::Flare(_) => "Flare",
    }
}

/// Every `Refused::Geometry` reason with the counter name it logs under:
/// `Particle/Geometry/primitive`. A reason not listed counts as `Geometry`
/// (the test below keeps the list whole).
const GEOMETRY_REASONS: [(&str, &str); 20] = [
    ("indexed", "Geometry/indexed"),
    ("pretransformed", "Geometry/pretransformed"),
    ("layouts", "Geometry/layouts"),
    ("streams", "Geometry/streams"),
    ("stride", "Geometry/stride"),
    ("attributes", "Geometry/attributes"),
    ("attribute_slot", "Geometry/attribute_slot"),
    ("primitive", "Geometry/primitive"),
    ("vertex_count", "Geometry/vertex_count"),
    ("data_len", "Geometry/data_len"),
    ("data_type", "Geometry/data_type"),
    ("inputs", "Geometry/inputs"),
    ("corner_differs", "Geometry/corner_differs"),
    ("unpublished", "Geometry/unpublished"),
    ("data_differs", "Geometry/data_differs"),
    ("indices", "Geometry/indices"),
    ("raw_bytes", "Geometry/raw_bytes"),
    ("raw_attributes", "Geometry/raw_attributes"),
    ("raw_vertex_count", "Geometry/raw_vertex_count"),
    ("other", "Geometry"),
];

fn geometry_label(reason: &str) -> &'static str {
    GEOMETRY_REASONS.iter().find(|(r, _)| *r == reason).map_or("Geometry", |(_, label)| label)
}

fn refusal_label(why: Refused) -> &'static str {
    match why {
        Refused::Geometry(reason) => geometry_label(reason),
        Refused::Effect(reason) => reason,
        Refused::State => "State",
        Refused::Shader(_) => "Shader",
    }
}

fn refuse_copy(copy: d3d::TargetCopy, why: Refused) -> &'static str {
    let copy = match copy {
        d3d::TargetCopy::Reference => "Reference",
        d3d::TargetCopy::Display => "Display",
    };
    REFUSALS.count(copy, refusal_label(why));
    "particle resources"
}

#[derive(Clone)]
struct Pending {
    source: Source,
    id: u64,
}

enum Issuing {
    Particle(Pending),
    Flare(flares::Site),
}
impl Issuing {
    fn producer(&self) -> Producer {
        match self {
            Self::Particle(p) => p.source.producer(),
            Self::Flare(site) => Producer::Flare(*site),
        }
    }
}

#[derive(Default)]
struct Scope {
    waiting: Option<Pending>,
    issuing: Option<Issuing>,
}
type RendererSlot = Option<(wgpu::Device, u64, ParticleRenderer)>;
type Renderer = Arc<Mutex<RendererSlot>>;
struct Admission {
    scope: Mutex<Scope>,
    renderer: Renderer,
    flare_gpu: Arc<Mutex<flares::FlareGpu>>,
    captured_gpu: Arc<Mutex<native_render::ui_gpu::UiGpu>>,
    serial: AtomicU64,
}
struct ImmediateAdmission<B: d3d::backend::Backend + 'static> {
    device: d3d::abi::SharedDevice<B>,
    admission: Arc<Admission>,
}
struct Read<'a>(&'a dyn GuestMemory);
impl native_render::camera::GuestRead for Read<'_> {
    fn read_u32(&self, a: u32) -> Option<u32> {
        self.0.u32(a).ok()
    }
}
impl<B: d3d::backend::Backend + 'static> ImmediateAdmission<B> {
    /// A draw under a cut setup, from the material held at the cut
    /// (docs/graphics/retire-d3d-r2.md, "The cut"). A draw the device
    /// cannot make panics with its cause. Always `true`.
    #[allow(clippy::too_many_arguments)]
    fn supplied_draw(
        &self, mem: &mut dyn GuestMemory, kind: Kind, primitive: PrimitiveType, data: Vec<u8>,
        carried: &[Option<[f32; 4]>; 16], supplied: &d3d::SuppliedState, candidate: &Direct<'_>,
    ) -> bool {
        let outcome = match self.device.lock() {
            Ok(mut device) => device
                .try_native_supplied_draw(&mut DeviceMemory(mem), primitive, data, carried, supplied, candidate)
                .unwrap_or(Err("device_error")),
            Err(_) => Err("device_lock"),
        };
        if let Err(cause) = outcome {
            unimplemented!("{kind:?} material: the supplied draw: {cause}");
        }
        true
    }

    fn arm(&self, source: Source) -> bool {
        let mut s = self
            .admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // A missed completion never grants ownership to a subsequent call.
        s.issuing = None;
        s.waiting = Some(Pending {
            source,
            id: self.admission.serial.fetch_add(1, Ordering::Relaxed),
        });
        true
    }
}
impl<B: d3d::backend::Backend + 'static> Host for ImmediateAdmission<B> {
    fn try_emit(&self, mem: &mut dyn GuestMemory, call: &Call) -> bool {
        let data = particle_capture::corner_bytes().to_vec();
        let carried = call.emission.attributes.map(|r| r.map(|v| v.map(f32::from_bits)));
        let pending = Pending {
            source: Source::Particle(call.clone()),
            id: self.admission.serial.fetch_add(1, Ordering::Relaxed),
        };
        let Some(supplied) = material_proof::held(Kind::Particle) else {
            unimplemented!("particle_material: an emission before any particle setup");
        };
        let candidate = Direct { admission: &self.admission, pending, stage: Stage::Particles };
        self.supplied_draw(mem, Kind::Particle, PrimitiveType::QuadList, data, &carried, &supplied, &candidate)
    }

    fn try_dynamic(&self, mem: &mut dyn GuestMemory, esp: u32, data: Option<u32>) -> bool {
        let pending = self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).waiting.clone();
        let Some(mut pending) = pending else { return false };
        let Source::Dynamic(call, bytes) = &mut pending.source else { return false };
        let data = match (call.kind, data) {
            (effect_geometry::Kind::EffectQuad, None) => call.context,
            (effect_geometry::Kind::Contrail, Some(data)) => data,
            _ => return false,
        };
        Stage::Effects.count(Metric::WalkExamined, 1);
        if effect_geometry::read_call(&Read(mem), esp, call.stride) != Ok(*call) {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        }
        let Ok(generated) = effect_geometry::read_generated(&Read(mem), data, call) else {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        };
        let Some(primitive) = PrimitiveType::from_xbox(call.primitive) else {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        };
        *bytes = Some(generated.as_slice().into());
        let held = match call.kind {
            effect_geometry::Kind::Contrail => Some(
                material_proof::held(Kind::Contrail)
                    .unwrap_or_else(|| unimplemented!("contrail_material: a contrail draw before any contrail setup")),
            ),
            effect_geometry::Kind::EffectQuad => None,
        };
        if call.kind == effect_geometry::Kind::Contrail {
            // The setup's inputs are this draw's alone.
            render_bridge::effect_materials::take_contrail_inputs(call.context);
        }
        if let Some(supplied) = held {
            let candidate = Direct { admission: &self.admission, pending, stage: Stage::Effects };
            let drawn = self.supplied_draw(mem, Kind::Contrail, primitive, generated, &[None; 16], &supplied, &candidate);
            self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).waiting = None;
            Stage::Effects.count(Metric::WalkSuppressed, 1);
            return drawn;
        }
        let candidate = Direct { admission: &self.admission, pending, stage: Stage::Effects };
        let drawn = self.device.lock().is_ok_and(|mut device| {
            device.try_native_inline_draw(
                &mut DeviceMemory(mem), primitive, generated, &[None; 16], &candidate,
            ).ok() == Some(true)
        });
        if drawn {
            self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).waiting = None;
        }
        Stage::Effects.count(if drawn { Metric::WalkSuppressed } else { Metric::WalkRefusedReservation }, 1);
        drawn
    }

    fn material_cut(&self, mem: &mut dyn GuestMemory, kind: Kind) -> Result<(), &'static str> {
        let supplied = material_proof::cut(mem, kind)?;
        let device = self.device.lock().map_err(|_| "device_lock")?;
        device.check_supplied(&DeviceMemory(&mut *mem), &supplied)?;
        material_proof::hold(kind, supplied);
        Ok(())
    }

    fn arm(&self, _: &mut dyn GuestMemory, call: &Call) -> bool {
        self.arm(Source::Particle(call.clone()))
    }
    fn arm_dynamic(&self, _: &mut dyn GuestMemory, call: &DynamicCall) -> bool {
        self.arm(Source::Dynamic(*call, None))
    }
    fn arm_effect(&self, mem: &mut dyn GuestMemory, site: Site) -> bool {
        self.arm(Source::Immediate(site));
        // END is not published; the weather layer's last vertex is not
        // either (its BEGIN is that vertex call's first push). Drain prior
        // work without ownership, then own only this leaf's completed draw.
        self.publish(mem, Producer::Immediate(site), 0);
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing
            .is_some()
    }
    fn arm_flare(&self, mem: &mut dyn GuestMemory, site: flares::Site) {
        let mut device = self.device.lock().unwrap_or_else(|e| e.into_inner());
        // Drain with no owner. End has yet to submit the fan, whether its
        // four vertices are published inline or the last is still to come.
        *self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()) = Scope::default();
        if device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() == Some(true) {
            self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).issuing =
                Some(Issuing::Flare(site));
        } else {
            flare_pass::kept(site, "boundary");
        }
    }
    fn publish(&self, mem: &mut dyn GuestMemory, producer: Producer, data: u32) {
        let mut device = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let pending = {
            let mut s = self
                .admission
                .scope
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            s.issuing = None;
            s.waiting.take().filter(|p| p.source.producer() == producer)
        };
        let Some(mut p) = pending else { return };
        if let Source::Dynamic(call, bytes) = &mut p.source {
            let Ok(generated) = effect_geometry::read_generated(&Read(mem), data, call) else {
                return;
            };
            *bytes = Some(generated.into());
        }
        // The guest has not published this packet yet. Earlier draws have
        // no active owner, even if they have identical geometry and state.
        if device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() != Some(true) {
            return;
        }
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing = Some(Issuing::Particle(p));
    }
    fn finish(&self, mem: &mut dyn GuestMemory, producer: Producer) {
        let mut device = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let active = {
            let mut s = self
                .admission
                .scope
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let active = s.issuing.as_ref().is_some_and(|p| p.producer() == producer);
            if matches!(producer, Producer::Flare(_)) && !active {
                // Shared helper called outside the flare pass, or End already consumed it.
                return;
            }
            s.waiting = None; // allocation failure / skipped emission
            active
        };
        if active {
            let _ = device.drain_admission_boundary(&mut DeviceMemory(mem));
        }
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing = None;
    }
}

pub fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| render_bridge::native_particles::on(&s)) {
        return;
    }
    let admission = Arc::new(Admission {
        scope: Mutex::new(Scope::default()),
        renderer: Default::default(),
        flare_gpu: Default::default(),
        captured_gpu: Default::default(),
        serial: AtomicU64::new(1),
    });
    device
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .add_draw_admission(admission.clone());
    render_bridge::native_particles::set_host(Arc::new(ImmediateAdmission {
        device: device.clone(),
        admission,
    }));
    // R2's particle mirror draws its batches on the same device.
    mirror_host::install(device);
}

/// Exact source bytes and declaration, independently checked against the
/// resolved draw. No matching by material or primitive shape alone.
fn geometry(
    draw: &d3d::ResolvedDraw,
    source: &Source,
) -> Result<(Vec<Vertex>, Option<RawVertices>, Cow<'static, [u32]>), Refused> {
    if draw.indices.is_some() {
        return Err(Refused::Geometry("indexed"));
    }
    if draw.layout.pretransformed {
        return Err(Refused::Geometry("pretransformed"));
    }
    let [layout] = draw.layout.streams.as_slice() else {
        return Err(Refused::Geometry("layouts"));
    };
    let [stream] = draw.streams.as_slice() else {
        return Err(Refused::Geometry("streams"));
    };
    if layout.stream != 0 || layout.stride != stream.stride {
        return Err(Refused::Geometry("stride"));
    }
    match source {
        Source::Immediate(site) => {
            if stream.stride != 256 {
                return Err(Refused::Effect("ImmediateDeclaration"));
            }
            let mut written = 0u16;
            for a in &layout.attributes {
                let r = a.register.0;
                if r >= 16
                    || a.data_type != VertexDataType::Float4
                    || a.offset != u32::from(r) * 16
                    || written & (1 << r) != 0
                {
                    return Err(Refused::Effect("ImmediateDeclaration"));
                }
                written |= 1 << r;
            }
            if written != draw.shader.provided_inputs {
                return Err(Refused::Effect("ImmediateDeclaration"));
            }
            let (vertices, indices) = effect_immediate::decode(
                *site,
                draw.primitive.xbox_value(),
                draw.vertex_count,
                &stream.data,
                written,
                draw.shader.vertex_defaults,
            )
            .map_err(|e| Refused::Effect(e.name()))?;
            Ok((vertices, None, indices.into()))
        }
        Source::Particle(call) => {
            let [a] = layout.attributes.as_slice() else {
                return Err(Refused::Geometry("attributes"));
            };
            if draw.primitive != PrimitiveType::QuadList {
                return Err(Refused::Geometry("primitive"));
            }
            if draw.vertex_count != 4 {
                return Err(Refused::Geometry("vertex_count"));
            }
            if stream.stride != 8 {
                return Err(Refused::Geometry("stride"));
            }
            if stream.data.len() != 32 {
                return Err(Refused::Geometry("data_len"));
            }
            if a.offset != 0 || a.register.0 != 0 {
                return Err(Refused::Geometry("attribute_slot"));
            }
            if a.data_type != VertexDataType::Short4 {
                return Err(Refused::Geometry("data_type"));
            }
            if draw.shader.provided_inputs != 1 {
                return Err(Refused::Geometry("inputs"));
            }
            let expected = call.emission.vertices(draw.shader.vertex_defaults);
            let actual: [Vertex; 4] = std::array::from_fn(|corner| {
                let mut registers = draw.shader.vertex_defaults;
                registers[0] = std::array::from_fn(|i| {
                    let n = corner * 8 + i * 2;
                    i16::from_le_bytes([stream.data[n], stream.data[n + 1]]) as f32
                });
                Vertex { registers }
            });
            if particles::differing_registers(&expected, &actual) != 0 {
                return Err(Refused::Geometry("corner_differs"));
            }
            Ok((expected.to_vec(), None, Cow::Borrowed(&particles::QUAD_INDICES)))
        }
        Source::Dynamic(call, bytes) => {
            let bytes = bytes.as_ref().ok_or(Refused::Geometry("unpublished"))?;
            let primitive = match call.primitive {
                6 => PrimitiveType::TriangleStrip,
                7 => PrimitiveType::TriangleFan,
                8 => PrimitiveType::QuadList,
                _ => return Err(Refused::Geometry("primitive")),
            };
            if draw.primitive != primitive {
                return Err(Refused::Geometry("primitive"));
            }
            if draw.vertex_count != call.vertices {
                return Err(Refused::Geometry("vertex_count"));
            }
            if stream.stride != call.stride {
                return Err(Refused::Geometry("stride"));
            }
            if stream.data.as_ref() != bytes.as_ref() {
                return Err(Refused::Geometry("data_differs"));
            }
            let attributes = layout
                .attributes
                .iter()
                .map(|a| {
                    let format = match a.data_type {
                        VertexDataType::Float3 => wgpu::VertexFormat::Float32x3,
                        VertexDataType::Float2 => wgpu::VertexFormat::Float32x2,
                        VertexDataType::D3dColor => wgpu::VertexFormat::Unorm8x4Bgra,
                        _ => return Err(Refused::Geometry("data_type")),
                    };
                    Ok(wgpu::VertexAttribute {
                        format,
                        offset: a.offset as u64,
                        shader_location: a.register.0 as u32,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let raw = RawVertices::new(call.kind, bytes.clone(), attributes)?;
            let mask = layout
                .attributes
                .iter()
                .try_fold(0u16, |mask, a| {
                    (a.register.0 < 16).then(|| mask | (1u16 << a.register.0))
                })
                .ok_or(Refused::Geometry("attribute_slot"))?;
            if mask != draw.shader.provided_inputs {
                return Err(Refused::Geometry("inputs"));
            }
            Ok((
                Vec::new(),
                Some(raw),
                effect_geometry::indices(call)
                    .map_err(|_| Refused::Geometry("indices"))?
                    .into(),
            ))
        }
    }
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        // Consume even a mismatched first draw. A refused call cannot own
        // another packet; epilogue cancellation handles the zero-draw case.
        let pending = self
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing
            .take()?;
        match pending {
            Issuing::Particle(p) => self.plan(snapshot, &p, Stage::Particles, true),
            Issuing::Flare(site) => flare_pass::plan(site, snapshot, &self.flare_gpu),
        }
    }

    fn admit_counted(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let mut scope = self.scope.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&scope.issuing, Some(Issuing::Flare(flares::Site::Visibility))) {
            return None;
        }
        scope.issuing = None;
        drop(scope);
        flare_pass::plan(flares::Site::Visibility, snapshot, &self.flare_gpu)
    }
}

struct Direct<'a> {
    admission: &'a Admission,
    pending: Pending,
    stage: Stage,
}

impl DrawAdmission for Direct<'_> {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        self.admission.plan(snapshot, &self.pending, self.stage, false)
    }
}

impl Admission {
    fn plan(
        &self,
        snapshot: &DrawSnapshot<'_>,
        pending: &Pending,
        stage: Stage,
        captured: bool,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let refusal = if snapshot.color_header == 0 {
            Some("MissingColorTarget")
        } else if snapshot.depth_header == 0 || snapshot.draw.target.depth_format.is_none() {
            Some("MissingDepthTarget")
        } else if snapshot.draw.clip.is_none() || !snapshot.draw.clip_rects.is_empty() {
            Some("UnsupportedClip")
        } else if snapshot.draw.glyph_cache != 0 {
            Some("GlyphFiltering")
        } else if snapshot.draw.hw.zpass_count_enable {
            Some("UnexpectedSampleCount")
        } else if !snapshot.draw.state.bad.is_empty() {
            Some("UnresolvedState")
        } else {
            None
        };
        if let Some(reason) = refusal {
            REFUSALS.count(producer_label(pending.source.producer()), reason);
            return None;
        }
        let (vertices, raw_vertices, indices) = match geometry(snapshot.draw, &pending.source) {
            Ok(geometry) => geometry,
            Err(Refused::Geometry(_))
                if captured && matches!(&pending.source, Source::Particle(_))
                    && particle_capture::owns(snapshot.draw) =>
            {
                return Some((
                    ExternalPlan { stage, view: 4, ranges: 1, slots: Vec::new() },
                    Box::new(particle_capture::Pass::new(snapshot.draw, &self.captured_gpu)),
                ));
            }
            Err(reason) => {
                let producer = producer_label(pending.source.producer());
                if let Refused::Geometry(why) = &reason {
                    SAMPLES.record(why, producer, snapshot.draw);
                }
                REFUSALS.count(producer, refusal_label(reason));
                return None;
            }
        };
        Some((
            ExternalPlan {
                stage,
                view: 4,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(Pass {
                draw: Arc::new(snapshot.draw.clone()),
                id: pending.id,
                color: snapshot.color_header,
                depth: snapshot.depth_header,
                vertices,
                raw_vertices,
                indices,
                renderer: self.renderer.clone(),
                prepared: Vec::new(),
                frame: crate::d3d_diag::presents(),
            }),
        ))
    }
}

struct Pass {
    draw: Arc<d3d::ResolvedDraw>,
    id: u64,
    color: u32,
    depth: u32,
    vertices: Vec<Vertex>,
    raw_vertices: Option<RawVertices>,
    indices: Cow<'static, [u32]>,
    renderer: Renderer,
    prepared: Vec<(PreparedDraw, DrawIdentity, TargetKey)>,
    /// The guest frame the draw was admitted in. The renderer keeps a
    /// texture group only while draws of recent frames use it: a view of
    /// a frame-buffer copy or depth alias is new each frame.
    frame: u64,
}
/// What a snapshot needs of one target copy, whether it comes from an
/// isolated recording's `ExternalTarget` or the open pass's attachments.
struct CopyView<'a> {
    device: &'a wgpu::Device,
    size: (u32, u32),
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    stages: [Option<&'a wgpu::TextureView>; 4],
    copy: d3d::TargetCopy,
}
impl<'a> CopyView<'a> {
    fn of_target(t: &ExternalTarget<'a>) -> Self {
        Self {
            device: t.device,
            size: t.size,
            color_format: t.color_format,
            depth_format: t.depth_format,
            stages: t.stages,
            copy: t.copy,
        }
    }
    fn of_attachments(t: &'a ExternalAttachments, _request: &d3d::ExternalPassRequest) -> Self {
        Self {
            device: &t.device,
            size: t.size,
            color_format: t.color_format,
            depth_format: t.depth_format,
            stages: t.stages.each_ref().map(Option::as_ref),
            copy: t.copy,
        }
    }
}
// An admission belongs to one D3D device. A backend device replacement
// releases its old caches. Draws already prepared own their handles.
fn renderer_for<'a>(
    renderer: &'a mut RendererSlot,
    device: &wgpu::Device,
) -> (u64, &'a mut ParticleRenderer) {
    static DEVICES: AtomicU64 = AtomicU64::new(1);
    if renderer.as_ref().is_none_or(|(owner, _, _)| owner != device) {
        let id = DEVICES.fetch_add(1, Ordering::Relaxed);
        *renderer = Some((device.clone(), id, ParticleRenderer::new(id)));
    }
    let (_, id, renderer) = renderer.as_mut().expect("renderer for the current device");
    (*id, renderer)
}
impl Pass {
    fn snapshot(
        &self,
        t: &CopyView<'_>,
        renderer: &mut ParticleRenderer,
        device_id: u64,
    ) -> Result<ImmediateDrawSnapshot<'_>, Refused> {
        let draw = self.draw.for_copy(t.copy);
        let d = draw.as_ref();
        // The draw's own surface scaled to this copy's host texture.
        let scale = d3d::scaled::RenderScale::between((d.target.width, d.target.height), t.size)
            .expect("a bound target has a size");
        let caps = gfx::translate::Capabilities::from_features(t.device.features());
        let program = d3d::nv2a::resolved::resolve(d, t.stages.map(|v| v.is_some()), caps, scale)
            .map_err(|reason| Refused::Shader(reason.into()))?;
        let mut render = d.state.render;
        if program.pixel_options.w_buffer {
            render.solid_offset_enable = false;
            render.wireframe_offset_enable = false;
            render.point_offset_enable = false;
        }
        let mut primitive =
            gfx::translate::primitive_state(&render, d.primitive.host_topology(), caps);
        primitive.strip_index_format = (primitive.topology
            == wgpu::PrimitiveTopology::TriangleStrip)
            .then_some(wgpu::IndexFormat::Uint32);
        let clip = d.display_rect(d.clip.expect("plan admits only clipped draws"));
        let scissor = scale.rect(clip.x, clip.y, clip.width, clip.height);
        let mut textures = [None, None, None, None];
        for (n, slot) in textures.iter_mut().enumerate() {
            if let (Some(view), Some(dim), Some(header)) = (
                t.stages[n],
                program.pixel_options.texture_dims[n],
                d.textures[n],
            ) {
                *slot = Some(TextureBinding {
                    device: device_id,
                    generation: self.id,
                    identity: header as u64,
                    dimension: dim,
                    view: view.clone(),
                    sampler: renderer.sampler(t.device, &program.samplers[n]),
                });
            }
        }
        Ok(ImmediateDrawSnapshot {
            identity: DrawIdentity {
                device: device_id,
                generation: self.id,
                view: 4,
                call: self.id,
                copy: u32::from(t.copy == d3d::TargetCopy::Display),
            },
            target: TargetKey {
                color: self.color as u64,
                depth: self.depth as u64,
                size: t.size,
                color_format: t.color_format,
                depth_format: t.depth_format,
            },
            shader: ShaderSnapshot {
                microcode: program.words,
                pixel: program.pixel,
                pixel_options: program.pixel_options,
                flat_shading: program.vertex_options.flat_shading,
                input_fetch: program.vertex_options.input_fetch,
            },
            vertex_uniforms: program.vertex_uniforms,
            pixel_uniforms: program.pixel_uniforms,
            textures,
            vertices: Cow::Borrowed(&self.vertices),
            raw_vertices: self.raw_vertices.as_ref().map(Cow::Borrowed),
            indices: Cow::Borrowed(&self.indices),
            primitive,
            depth: gfx::translate::depth_stencil_state(
                &render,
                Some(t.depth_format),
                d.primitive.host_topology(),
            )
            .ok_or(Refused::State)?,
            blend: gfx::translate::blend_state(&render),
            color_writes: gfx::translate::color_writes(render.color_write),
            blend_constant: gfx::translate::blend_constant(&render),
            stencil_reference: render.stencil.reference as u32,
            scissor,
        })
    }
}
impl Pass {
    /// The same particle drawing `draw`, with nothing prepared.
    fn drawing(&self, draw: Arc<d3d::ResolvedDraw>) -> Self {
        Self {
            draw,
            id: self.id,
            color: self.color,
            depth: self.depth,
            vertices: self.vertices.clone(),
            raw_vertices: self.raw_vertices.clone(),
            indices: self.indices.clone(),
            renderer: Arc::clone(&self.renderer),
            prepared: Vec::new(),
            frame: self.frame,
        }
    }

    /// Whether `next` can follow this draw in one [`Run`]: the same
    /// renderer, guest targets, frame and writes. State, constants,
    /// textures and blend stay per draw: each is its own packet in the
    /// run, in order.
    fn joins(&self, next: &Pass) -> bool {
        Arc::ptr_eq(&self.renderer, &next.renderer)
            && (self.color, self.depth, self.frame) == (next.color, next.depth, next.frame)
            && self.writes() == next.writes()
    }
}

/// Adjacent particle draws admitted as one queued pass
/// (`ExternalPass::queued_run`, docs/graphics/native-particles.md, "One
/// admission a run"): the render thread records every member's packet
/// into the open pass in one call, in draw order, as the members would
/// one by one. A member that does not record refuses the run, and the
/// queue draws every member's fallback in order.
struct Run {
    members: Vec<Pass>,
}

impl ExternalPass for Run {
    fn replays_scene(&self) -> bool {
        true
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn extend_run(&mut self, next: &dyn ExternalPass) -> bool {
        let Some(next) = next.as_any().and_then(|n| n.downcast_ref::<Pass>()) else { return false };
        let Some(last) = self.members.last() else { return false };
        if !last.joins(next) {
            return false;
        }
        self.members.push(next.drawing(Arc::clone(&next.draw)));
        true
    }

    fn encode(&mut self, _: ExternalTarget<'_>) {}

    fn writes(&self) -> ExternalWrites {
        self.members.first().map(|m| m.writes()).unwrap_or_default()
    }

    /// The queue holds each member's fallback; a run has no single one.
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        None
    }

    fn resolved_packets(&self) -> Vec<Arc<d3d::ResolvedDraw>> {
        self.members.iter().map(|m| Arc::clone(&m.draw)).collect()
    }

    fn diagnostic_draw(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.members.first().map(|m| Arc::clone(&m.draw))
    }

    fn prepare_copy(&mut self, target: &ExternalTarget<'_>) -> Result<(), &'static str> {
        self.members.iter_mut().try_for_each(|m| m.prepare_copy(target))
    }

    /// Every member's packet, in order. A member's stencil reference is 8
    /// bits, so each draws as a packet and the isolated recording is never
    /// asked for.
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let mut all = ExternalDraws {
            ranges: 0,
            packets: Vec::with_capacity(self.members.len()),
            discards: false,
            writes: Vec::new(),
        };
        for member in &mut self.members {
            let draws = match member.record_draws(t, request) {
                Some(Ok(draws)) => draws,
                Some(Err(why)) => return Some(Err(why)),
                None => return Some(Err("particle run member left the open pass")),
            };
            all.ranges += draws.ranges;
            all.packets.extend(draws.packets);
            all.discards |= draws.discards;
            all.writes.extend(draws.writes);
        }
        Some(Ok(all))
    }

    fn record_ranges(&mut self, _: ExternalTarget<'_>) -> usize {
        0
    }
}

impl ExternalPass for Pass {
    fn replays_scene(&self) -> bool {
        true
    }

    fn with_draw(&self, draw: Arc<d3d::ResolvedDraw>) -> Option<Box<dyn ExternalPass>> {
        Some(Box::new(self.drawing(draw)))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// This particle, then the next, as one queued pass ([`Run`]).
    fn queued_run(&self, next: &dyn ExternalPass) -> Option<Box<dyn ExternalPass>> {
        let next = next.as_any()?.downcast_ref::<Pass>()?;
        self.joins(next).then(|| {
            Box::new(Run { members: vec![self.drawing(Arc::clone(&self.draw)), next.drawing(Arc::clone(&next.draw))] })
                as Box<dyn ExternalPass>
        })
    }

    fn encode(&mut self, _: ExternalTarget<'_>) {} // only transactional recording is supported
    fn writes(&self) -> ExternalWrites {
        let r = &self.draw.state.render;
        ExternalWrites {
            color: r.color_write.xbox_value() != 0,
            depth_stencil: (r.depth_mode != gfx::state::DepthBufferMode::Disabled && r.depth_write)
                || r.stencil.enable,
        }
    }
    /// One packet per pass: the guest's draw, or the draw a producer stands in for.
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }
    fn prepare_copy(&mut self, target: &ExternalTarget<'_>) -> Result<(), &'static str> {
        let copy = u32::from(target.copy == d3d::TargetCopy::Display);
        self.prepared.retain(|p| p.1.copy != copy);
        let renderer = self.renderer.clone();
        let mut renderer = renderer.lock().unwrap_or_else(|e| e.into_inner());
        let (id, renderer) = renderer_for(&mut renderer, target.device);
        renderer.begin_frame(self.frame);
        let refusal = |why| refuse_copy(target.copy, why);
        let snapshot = self
            .snapshot(&CopyView::of_target(target), renderer, id)
            .map_err(&refusal)?;
        let draw = renderer
            .prepare(target.device, &snapshot)
            .map_err(refusal)?;
        let prepared = (draw, snapshot.identity, snapshot.target);
        drop(snapshot);
        self.prepared.push(prepared);
        Ok(())
    }
    /// One packet appended to the D3D pass open on these attachments: no
    /// command buffer or render pass of its own. `None` (the isolated
    /// recording below) only for what a packet cannot carry.
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let view = CopyView::of_attachments(t, &request);
        // Sampling the colour or depth being drawn is a feedback loop the
        // open pass cannot express.
        let attached = [t.color.texture(), t.depth.texture()];
        if view.stages.iter().flatten().any(|v| attached.contains(&v.texture())) {
            return Some(Err("particle samples its own target"));
        }
        let renderer = self.renderer.clone();
        let mut renderer = renderer.lock().unwrap_or_else(|e| e.into_inner());
        let (id, renderer) = renderer_for(&mut renderer, &t.device);
        renderer.begin_frame(self.frame);
        let refusal = |why| refuse_copy(t.copy, why);
        let snapshot = match self.snapshot(&view, renderer, id) {
            Ok(s) => s,
            Err(why) => return Some(Err(refusal(why))),
        };
        Some(
            match renderer.draws(&t.device, &t.queue, &t.uploads, &snapshot) {
                // A draw that rasterises nothing is still the guest's
                // draw, taken: one range, no packet.
                Ok(drawn) => {
                    let (packets, discards) = drawn.map_or((Vec::new(), false), |(p, d)| (vec![p], d));
                    Ok(ExternalDraws { ranges: 1, packets, discards, writes: Vec::new() })
                }
                Err(why) => Err(refusal(why)),
            },
        )
    }
    fn record_ranges(&mut self, target: ExternalTarget<'_>) -> usize {
        let copy = u32::from(target.copy == d3d::TargetCopy::Display);
        let Some(i) = self.prepared.iter().position(|p| p.1.copy == copy) else {
            return 0;
        };
        let (draw, identity, key) = self.prepared.swap_remove(i);
        if key.size != target.size
            || key.color_format != target.color_format
            || key.depth_format != target.depth_format
        {
            return 0;
        }
        draw.record(target.encoder, identity, key, target.color, target.depth);
        1
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use d3d::backend::{StreamBytes, TargetInfo, Viewport};
    use d3d::shader::{ShaderInputs, VertexProcessing};
    use d3d::state::StateWords;
    use gfx::vertex::{StreamLayout, VertexAttribute, VertexLayout, VertexRegister};

    pub(in crate::native_particles_admission) fn particle_draw() -> (d3d::ResolvedDraw, Source) {
        particle()
    }

    fn particle() -> (d3d::ResolvedDraw, Source) {
        let words = StateWords::defaults(false);
        let mut attributes = [None; 16];
        attributes[3] = Some([0.25f32.to_bits(), 0, 0, 1f32.to_bits()]);
        let source = Source::Particle(Call {
            esp: 0x100,
            return_address: 0x200,
            emission: particles::Emission { attributes },
        });
        let mut defaults = [[0., 0., 0., 1.]; 16];
        defaults[3] = [0.25, 0., 0., 1.];
        let data: Vec<u8> = [[0i16, 0, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0], [0, 1, 0, 0]]
            .into_iter()
            .flatten()
            .flat_map(i16::to_le_bytes)
            .collect();
        (
            d3d::ResolvedDraw {
                entry: "synthetic particle",
                primitive: PrimitiveType::QuadList,
                vertex_count: 4,
                layout: VertexLayout {
                    streams: vec![StreamLayout {
                        stream: 0,
                        stride: 8,
                        attributes: vec![VertexAttribute {
                            register: VertexRegister(0),
                            data_type: VertexDataType::Short4,
                            offset: 0,
                        }],
                    }],
                    pretransformed: false,
                },
                streams: vec![StreamBytes {
                    data: data.into(),
                    stride: 8,
                }],
                indices: None,
                state: words.translate(&gfx::xdk::RenderStateTable::default()),
                extras: words.stage_extras(),
                raw: Arc::new(words),
                target: TargetInfo {
                    width: 64,
                    height: 64,
                    format: gfx::D3DFormat::A8R8G8B8,
                    depth_format: Some(gfx::D3DFormat::D24S8),
                },
                shader: ShaderInputs {
                    vertex: VertexProcessing::Program {
                        start: 0,
                        microcode: Vec::<u32>::new().into(),
                    },
                    provided_inputs: 1,
                    vertex_defaults: defaults,
                    constants: Arc::new([[0.; 4]; 192]),
                    pixel_shader: Some(Arc::new([0; 60])),
                    combiner_state: Arc::new([0; 57]),
                    final_combiner_latch: true,
                    affine_projection: false,
                },
                viewport: Viewport {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                    min_z: 0.,
                    max_z: 1.,
                },
                clip: Some(d3d::PixelRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                }),
                clip_rects: Vec::new(),
                hw: Default::default(),
                texture_info: [None; 4],
                textures: [None; 4],
                retail_constants: None,
                ui_fit: None,
                glyph_cache: 0,
            },
            source,
        )
    }
    #[test]
    fn particle_matches_live_defaults_and_declared_corner_fetch() {
        let (mut draw, source) = particle();
        assert!(geometry(&draw, &source).is_ok());
        draw.shader.vertex_defaults[3][0] = 0.5;
        assert_eq!(geometry(&draw, &source).err(), Some(Refused::Geometry("corner_differs")));
        let (mut draw, source) = particle();
        draw.layout.streams[0].attributes[0].data_type = VertexDataType::NormShort4;
        assert_eq!(geometry(&draw, &source).err(), Some(Refused::Geometry("data_type")));
    }

    /// The reason `geometry` refuses `draw` with, for a synthetic draw one
    /// field away from a good one.
    fn why(draw: &d3d::ResolvedDraw, source: &Source) -> &'static str {
        match geometry(draw, source) {
            Err(Refused::Geometry(why)) => why,
            other => panic!("not a geometry refusal: {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn each_particle_geometry_check_names_itself() {
        let bad = |edit: &dyn Fn(&mut d3d::ResolvedDraw)| {
            let (mut draw, source) = particle();
            edit(&mut draw);
            why(&draw, &source)
        };
        assert_eq!(bad(&|d| d.indices = Some(vec![0u32].into())), "indexed");
        assert_eq!(bad(&|d| d.layout.pretransformed = true), "pretransformed");
        assert_eq!(bad(&|d| d.layout.streams.clear()), "layouts");
        assert_eq!(bad(&|d| d.streams.clear()), "streams");
        assert_eq!(bad(&|d| d.layout.streams[0].stride = 12), "stride");
        assert_eq!(bad(&|d| { d.layout.streams[0].stride = 16; d.streams[0].stride = 16; }), "stride");
        assert_eq!(bad(&|d| d.layout.streams[0].attributes.clear()), "attributes");
        assert_eq!(bad(&|d| d.layout.streams[0].attributes[0].offset = 4), "attribute_slot");
        assert_eq!(bad(&|d| d.primitive = PrimitiveType::TriangleList), "primitive");
        assert_eq!(bad(&|d| d.vertex_count = 6), "vertex_count");
        assert_eq!(bad(&|d| d.streams[0].data = vec![0u8; 16].into()), "data_len");
        assert_eq!(bad(&|d| d.layout.streams[0].attributes[0].data_type = VertexDataType::Float4), "data_type");
        assert_eq!(bad(&|d| d.shader.provided_inputs = 3), "inputs");
        assert_eq!(bad(&|d| d.shader.vertex_defaults[3][0] = 0.5), "corner_differs");
    }

    #[test]
    fn each_geometry_reason_has_its_own_counter_name() {
        for (reason, label) in GEOMETRY_REASONS {
            if reason != "other" {
                assert_eq!(label, format!("Geometry/{reason}"));
            }
            assert_eq!(refusal_label(Refused::Geometry(reason)), label);
        }
        assert_eq!(refusal_label(Refused::Geometry("not listed")), "Geometry");
    }

    #[test]
    fn a_refused_particle_draw_counts_under_its_reason_and_keeps_one_sample() {
        let (mut draw, source) = particle();
        draw.primitive = PrimitiveType::TriangleList;
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        };
        let pending = Pending { source, id: 1 };
        SAMPLES.clear();
        assert!(admission.plan(&snapshot(&draw), &pending, Stage::Particles, false).is_none());
        draw.vertex_count = 6;
        assert!(admission.plan(&snapshot(&draw), &pending, Stage::Particles, false).is_none());
        let answer = SAMPLES.answer();
        let primitive = &answer["samples"]["primitive"];
        assert_eq!(primitive["producer"], "Particle");
        assert_eq!(primitive["count"], 2);
        assert_eq!(primitive["shape"]["vertex_count"], 4, "the first draw's shape stays");
        SAMPLES.clear();
    }
    fn snapshot(draw: &d3d::ResolvedDraw) -> DrawSnapshot<'_> {
        DrawSnapshot {
            draw,
            vertex_shader: 0,
            pixel_shader: 0,
            bound_textures: [0; 4],
            sampled_stages: [false; 4],
            palettes: [0; 4],
            color_header: 1,
            depth_header: 2,
        }
    }

    #[test]
    fn one_flare_end_admits_one_fan_and_leaves_visibility_to_the_query() {
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        };
        let arm = || {
            admission.scope.lock().unwrap().issuing = Some(Issuing::Flare(flares::Site::Mask));
        };
        let (mut fan, _) = particle();
        fan.primitive = PrimitiveType::TriangleFan;
        fan.layout.streams[0].stride = 256;
        fan.layout.streams[0].attributes[0].data_type = VertexDataType::Float4;
        fan.streams[0] = StreamBytes { stride: 256, data: vec![0; 1024].into() };
        for end in ["D3DDevice_End", "inline Begin/End"] {
            arm();
            fan.entry = end;
            let (plan, pass) = admission.admit(&snapshot(&fan)).expect("scoped fan");
            assert_eq!((plan.stage, plan.ranges), (Stage::Flares, 1));
            assert!(pass.replays_scene());
            assert_eq!(pass.resolved_fallback().as_deref(), Some(&fan));
            assert!(admission.admit(&snapshot(&fan)).is_none());
        }
        arm();
        fan.hw.zpass_count_enable = true;
        assert!(admission.admit(&snapshot(&fan)).is_none());
        fan.hw.zpass_count_enable = false;
        assert!(admission.admit(&snapshot(&fan)).is_none());
    }

    #[test]
    fn visibility_fans_are_counted_only_inside_their_one_read_only_scope() {
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        };
        let arm = || {
            admission.scope.lock().unwrap().issuing = Some(Issuing::Flare(flares::Site::Visibility));
        };
        let (mut fan, _) = particle();
        fan.entry = "D3DDevice_End";
        fan.primitive = PrimitiveType::TriangleFan;
        fan.layout.streams[0].stride = 256;
        fan.layout.streams[0].attributes[0].data_type = VertexDataType::Float4;
        fan.streams[0] = StreamBytes { stride: 256, data: vec![0; 1024].into() };
        fan.state.render.color_write = gfx::state::ColorWriteMask::from_xbox(0);
        fan.state.render.depth_write = false;
        assert!(admission.admit_counted(&snapshot(&fan)).is_none());
        arm();
        assert!(admission.admit(&snapshot(&fan)).is_none());

        fan.hw.zpass_count_enable = true;
        arm();
        let (plan, pass) = admission.admit_counted(&snapshot(&fan)).unwrap();
        assert_eq!((plan.stage, plan.ranges), (Stage::Flares, 1));
        assert!(pass.counts_samples() && pass.replays_scene());
        assert_eq!(pass.resolved_fallback().as_deref(), Some(&fan));
        assert!(admission.admit_counted(&snapshot(&fan)).is_none());

        arm();
        fan.state.render.color_write = gfx::state::ColorWriteMask::ALL;
        assert!(admission.admit_counted(&snapshot(&fan)).is_none());
        fan.state.render.color_write = gfx::state::ColorWriteMask::from_xbox(0);
        assert!(admission.admit_counted(&snapshot(&fan)).is_none());
        arm();
        fan.state.render.depth_mode = gfx::state::DepthBufferMode::W;
        fan.state.render.depth_write = true;
        assert!(admission.admit_counted(&snapshot(&fan)).is_none());
    }

    fn admission() -> Admission {
        Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        }
    }

    /// Particle `id`'s admission of `draw`, as `try_emit` makes it.
    fn single(admission: &Admission, draw: &d3d::ResolvedDraw, source: &Source, id: u64) -> Box<dyn ExternalPass> {
        let pending = Pending { source: source.clone(), id };
        let (plan, pass) = admission.plan(&snapshot(draw), &pending, Stage::Particles, false).expect("a particle");
        assert_eq!(plan.ranges, 1);
        pass
    }

    /// N adjacent particles queue as one run: its packets are the N single
    /// admissions' draws, in their order, each keeping its own state (here
    /// a stencil reference) and identity.
    #[test]
    fn adjacent_particles_join_one_queued_run_with_their_packets_in_order() {
        let admission = admission();
        let (draw, source) = particle();
        let draws: Vec<_> = (0..5u8)
            .map(|i| {
                let mut d = draw.clone();
                d.state.render.stencil.reference = i;
                d
            })
            .collect();
        let singles: Vec<_> = draws.iter().zip(10..).map(|(d, id)| single(&admission, d, &source, id)).collect();
        let mut run = singles[0].queued_run(&*singles[1]).expect("adjacent particles join");
        for next in &singles[2..] {
            assert!(run.extend_run(&**next));
        }
        let fallbacks: Vec<_> = singles.iter().map(|p| p.resolved_fallback().expect("a single's draw")).collect();
        assert_eq!(run.resolved_packets(), fallbacks);
        assert!(run.resolved_packets().iter().zip(&draws).all(|(p, d)| **p == *d));
        assert!(run.resolved_fallback().is_none());
        assert!(run.replays_scene());
        assert_eq!(run.writes(), singles[0].writes());
        let run = run.as_any().and_then(|r| r.downcast_ref::<Run>()).expect("a particle run");
        assert_eq!(run.members.iter().map(|m| m.id).collect::<Vec<_>>(), [10, 11, 12, 13, 14]);
        // The device's single passes are not altered.
        assert!(singles.iter().all(|p| p.resolved_fallback().is_some()));
    }

    /// A particle that writes otherwise, draws through another renderer or
    /// is not a particle starts its own queued admission.
    #[test]
    fn a_particle_writing_otherwise_starts_its_own_run() {
        let admission = admission();
        let (draw, source) = particle();
        let first = single(&admission, &draw, &source, 1);
        let mut masked = draw.clone();
        masked.state.render.color_write = gfx::state::ColorWriteMask::from_xbox(0);
        let masked = single(&admission, &masked, &source, 2);
        assert_ne!(masked.writes(), first.writes());
        assert!(first.queued_run(&*masked).is_none());
        let elsewhere = single(&self::admission(), &draw, &source, 3);
        assert!(first.queued_run(&*elsewhere).is_none());
        let mut run = first.queued_run(&*single(&admission, &draw, &source, 4)).expect("same particle state");
        assert!(!run.extend_run(&*masked));
        assert!(!run.extend_run(&*elsewhere));
        assert_eq!(run.resolved_packets().len(), 2);
    }

    #[test]
    fn only_published_particle_bytes_can_supply_resolved_geometry() {
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        };
        let (mut draw, source) = particle();
        let pending = Pending { source, id: 1 };
        draw.entry = "inline Begin/End";
        draw.shader.vertex_defaults[3][0] = 0.5;
        assert!(admission.plan(&snapshot(&draw), &pending, Stage::Particles, false).is_none());
        let (_, pass) = admission.plan(&snapshot(&draw), &pending, Stage::Particles, true).unwrap();
        assert_eq!(pass.resolved_fallback().as_deref(), Some(&draw));

        draw.layout.streams[0].attributes[0].data_type = VertexDataType::NormShort4;
        assert!(admission.plan(&snapshot(&draw), &pending, Stage::Particles, true).is_some());
        let mut other = particle_capture::corner_bytes();
        other[8] ^= 1;
        draw.streams[0].data = other.to_vec().into();
        assert!(admission.plan(&snapshot(&draw), &pending, Stage::Particles, true).is_none());
    }

    #[test]
    fn a_mismatch_consumes_the_scope_and_cannot_take_the_following_quad() {
        let (draw, source) = particle();
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderer: Default::default(),
            flare_gpu: Default::default(),
            captured_gpu: Default::default(),
            serial: AtomicU64::new(1),
        };
        assert!(
            admission.admit(&snapshot(&draw)).is_none(),
            "no active producer"
        );
        admission.scope.lock().unwrap().issuing = Some(Issuing::Particle(Pending { source, id: 1 }));
        let mut wrong = draw.clone();
        wrong.vertex_count = 3;
        assert!(admission.admit(&snapshot(&wrong)).is_none());
        assert!(
            admission.admit(&snapshot(&draw)).is_none(),
            "a later identical quad is not owned"
        );
    }
    #[test]
    fn immediate_effect_requires_exact_register_declaration_and_rasterization() {
        let (mut draw, _) = particle();
        let source = Source::Immediate(Site::WeatherLayer);
        draw.primitive = PrimitiveType::QuadStrip;
        draw.layout.streams[0].stride = 256;
        draw.layout.streams[0].attributes[0].data_type = VertexDataType::Float4;
        draw.streams[0] = StreamBytes {
            data: vec![0; 1024].into(),
            stride: 256,
        };
        let (v, raw, indices) = geometry(&draw, &source).unwrap();
        assert!(raw.is_none());
        assert_eq!(v[0].registers[3], draw.shader.vertex_defaults[3]);
        assert_eq!(indices.as_ref(), &[0, 1, 3, 0, 3, 2]);
        draw.layout.streams[0].attributes[0].offset = 16;
        assert!(matches!(
            geometry(&draw, &source),
            Err(Refused::Effect("ImmediateDeclaration"))
        ));
        draw.layout.streams[0].attributes[0].offset = 0;
        draw.primitive = PrimitiveType::PointList;
        assert!(matches!(
            geometry(&draw, &Source::Immediate(Site::WeatherParticles)),
            Err(Refused::Effect("WeatherPointRasterization"))
        ));
        draw.primitive = PrimitiveType::LineList;
        let (_, _, indices) = geometry(&draw, &Source::Immediate(Site::WeatherParticles)).unwrap();
        assert_eq!(indices.as_ref(), &[0, 1, 2, 3]);
        draw.shader.provided_inputs |= 1 << 3;
        assert!(matches!(
            geometry(&draw, &Source::Immediate(Site::WeatherParticles)),
            Err(Refused::Effect("ImmediateDeclaration"))
        ));
    }
    #[test]
    fn raw_ribbon_bytes_and_live_register_mapping_must_match() {
        let (mut draw, _) = particle();
        let call = DynamicCall {
            kind: effect_geometry::Kind::Contrail,
            esp: 0x100,
            return_address: 0x200,
            builder: effect_geometry::CONTRAIL_BUILD,
            context: 0x300,
            vertices: 4,
            stride: 24,
            primitive: 6,
        };
        let bytes = vec![0; 96];
        let source = Source::Dynamic(call, Some(bytes.as_slice().into()));
        draw.primitive = PrimitiveType::TriangleStrip;
        draw.shader.provided_inputs = (1 << 0) | (1 << 9) | (1 << 3);
        draw.layout.streams[0] = StreamLayout {
            stream: 0,
            stride: 24,
            attributes: vec![
                VertexAttribute {
                    register: VertexRegister(0),
                    data_type: VertexDataType::Float3,
                    offset: 0,
                },
                VertexAttribute {
                    register: VertexRegister(9),
                    data_type: VertexDataType::Float2,
                    offset: 12,
                },
                VertexAttribute {
                    register: VertexRegister(3),
                    data_type: VertexDataType::D3dColor,
                    offset: 20,
                },
            ],
        };
        draw.streams[0] = StreamBytes {
            data: bytes.clone().into(),
            stride: 24,
        };
        let (_, raw, indices) = geometry(&draw, &source).unwrap();
        assert_eq!(raw.unwrap().bytes(), bytes);
        assert_eq!(indices.as_ref(), &[0, 1, 2, 3]);
        let mut changed = bytes;
        changed[95] = 1;
        draw.streams[0].data = changed.into();
        assert_eq!(geometry(&draw, &source).err(), Some(Refused::Geometry("data_differs")));
    }
    #[test]
    fn resolved_program_keeps_the_final_combiner_latch_and_w_clip_without_depth_writes() {
        let (mut draw, _) = particle();
        draw.hw.control0 = Some(d3d::nv2a::CONTROL0_Z_PERSPECTIVE);
        draw.state.render.depth_mode = gfx::state::DepthBufferMode::Disabled;
        let p =
            d3d::nv2a::resolved::resolve(&draw, [false; 4], gfx::translate::Capabilities::NONE, 2u32)
                .unwrap();
        assert!(p.pixel.explicit_final);
        assert!(p.vertex_uniforms.w_clip.is_some());
        assert!(!p.pixel_options.w_buffer);
        assert_eq!(p.vertex_uniforms.shift, [gfx::renderer::window_shift(1, 2); 2]);
    }

    fn cut_material() -> (native_render::effect_material::Material, native_render::effect_material::Resolved) {
        use native_render::effect_material::{Material, Resolved, Texture};
        let mut combiner = [0u32; 60];
        combiner[0] = 0x0102_0304;
        let m = Material {
            states: vec![(0x53, 0), (0x3b, 1)],
            stages: vec![(1, 2, 5)],
            combiner,
            combiner_driven: 0,
            textures: [Some(Texture::Bitmap { bitmap: 2, image: 0 }), Some(Texture::Empty), None, None],
            constants: vec![(0x12, [1.0, 2.0, 3.0, 4.0]), (0xae, [0.5; 4])],
            program: (5, 7),
        };
        let mut program = vec![0x78, 0x20, 2, 0];
        program.extend((0u8..32).map(|b| b + 1));
        let r = Resolved { textures: [Some(Some(0xb000)), Some(None), None, None], program };
        (m, r)
    }

    /// A cut setup's draw takes exactly its material: the states, combiner,
    /// whole constant rows, the tag's program without its header, and the
    /// textures (an empty stage unbinds, an unset one keeps the device's).
    #[test]
    fn a_cut_setups_material_is_the_state_its_draw_is_supplied() {
        let (m, r) = cut_material();
        let s = material_proof::supplied(&m, &r).unwrap();
        assert_eq!(s.render, [(0x53, 0), (0x3b, 1)]);
        assert_eq!(s.stages, [(1, 2, 5)]);
        assert_eq!(s.pixel_shader, m.combiner);
        assert_eq!(s.constants, [(0x12, [1.0, 2.0, 3.0, 4.0], 0xf), (0xae, [0.5; 4], 0xf)]);
        assert_eq!(s.microcode.len(), 8);
        assert_eq!(s.microcode[0], u32::from_le_bytes([1, 2, 3, 4]));
        assert_eq!(s.textures, [Some(0xb000), Some(0), None, None]);
        assert_eq!(s.vertex_format, None);
    }

    /// What the material does not hold is refused by name, so the guest's
    /// setup runs: a combiner word the draw drives, a program whose length
    /// is not its header's.
    #[test]
    fn a_material_the_supplied_draw_cannot_hold_keeps_the_guest_setup_by_name() {
        let (m, r) = cut_material();
        let driven = native_render::effect_material::Material { combiner_driven: 1 << 10, ..m.clone() };
        assert_eq!(material_proof::supplied(&driven, &r).err(), Some("combiner_driven"));
        let mut short = r.clone();
        short.program.pop();
        assert_eq!(material_proof::supplied(&m, &short).err(), Some("vertex_program_size"));
    }}
