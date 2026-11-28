//! Retail scene list production, on with the native renderer
//! (`impl.render.native`, on by default). A builder reads under the
//! guest-memory guard and proposes all its writes before any output
//! becomes visible.
mod display_swap;
mod geometry;
mod portal_merge;
mod portal_regions;
mod lights;
mod objects;
mod object_lod;
mod portal_walk;
mod projection;
mod records;
mod setup;
pub use setup::{DisplayGeometry, DisplaySection, display_model};
mod regions;
mod sphere_regions;
mod transparents;
pub(crate) mod view_records;
pub(crate) mod visibility_setup;
mod window;
pub(crate) use transparents::{bsp as transparent_bsp, display_sort_key};

use std::collections::HashMap;
use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};
use engine::points::{self, Point};
use runtime::guest::Access;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use timeline::native::{SceneList, SceneListResult};

use hooks::{BuildError, Builder, Feature, Flow, Manifest, Switches};

const ID: &str = "native-scene-lists";
const NONE: u32 = u32::MAX;
const TAGS: u32 = engine::tags::TAG_ENTRIES_GLOBAL;
const FRAME: u32 = 0x004b_a034;
const OBJECTS: u32 = engine::render_loop::occlusion::guest::OBJECTS;

/// Geometry gather and record inputs for a whole cluster reached only by
/// the presentation lens. Restricted lists and secondary clipping planes
/// require their per-window records.
pub(crate) fn late_cluster_bounds(m: &dyn GuestMemory, id: u16) -> Option<[f32; 6]> {
    let vis = objects::MAIN_VIS;
    if m.u32(vis + objects::VIS_FLAGS).ok()? & 0x2a1 != 0 { return None; }
    let bsp = ptr(m, objects::ACTIVE_BSP).ok()?;
    if u32::from(id) >= m.u32(bsp.checked_add(0x9c)?).ok()? { return None; }
    let sections = ptr(m, bsp.checked_add(0xa0)?).ok()?;
    let section = sections.checked_add(u32::from(id) * 0xb0)?;
    if m.u16(section.checked_add(0x24)?).ok()? == 0 { return None; }
    let mut bounds = [0.0; 6];
    for (i, v) in bounds.iter_mut().enumerate() {
        *v = m.f32(section.checked_add(0x54 + 4 * i as u32)?).ok()?;
    }
    Some(bounds)
}

/// The ordinary main-cluster gather (0x13457b) and record writer's flags.
/// The caller admits only whole portal windows without a secondary plane.
pub fn display_cluster_fog_flags(m: &dyn GuestMemory, id: u16) -> Option<u32> {
    let sky = objects::cluster_uses_sky(m, u32::from(id)).ok()?;
    let flags = 0xf000 | (u16::from(sky) << 10);
    setup::section_flags(m, objects::MAIN_VIS, ptr(m, TAGS).ok()?, flags).ok()
}

pub struct NativeSceneLists;

fn enabled(switches: &Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// The view's requested geometry detail. The MODE tag numbers its tiers
/// from coarsest to finest, opposite the "LOD 0" convention in most tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LodDetail {
    Retail,
    Scaled(f32),
    Highest,
}

impl LodDetail {
    pub const fn pixel_multiplier(self) -> f32 {
        match self {
            Self::Scaled(value) => value,
            Self::Retail | Self::Highest => 1.0,
        }
    }

    pub const fn guest_word(self) -> u32 {
        match self {
            Self::Highest => 0x7fff_ffff,
            other => other.pixel_multiplier().to_bits(),
        }
    }

    fn from_guest_word(word: u32) -> Self {
        let scale = f32::from_bits(word);
        match word {
            0x7fff_ffff => Self::Highest,
            _ if scale.is_normal() && scale > 0.0 && scale != 1.0 => Self::Scaled(scale),
            _ => Self::Retail,
        }
    }
}

/// The renderer's two LOD selections, sampled at the same view boundary.
/// The first word serves the guest and exact reference. The display word
/// follows it, in the render context's private allocation.
#[derive(Clone, Copy)]
pub struct ViewLodScale(u32);

impl ViewLodScale {
    pub const MODEL_VIEW: u32 = 1 << 31;

    pub const fn new(guest_word: u32) -> Self {
        Self(guest_word)
    }

    pub fn read(self, m: &dyn GuestMemory) -> LodDetail {
        m.u32(self.0).map_or(LodDetail::Retail, LodDetail::from_guest_word)
    }

    pub fn display(self, m: &dyn GuestMemory) -> LodDetail {
        m.u32(self.0 + 4).map_or(LodDetail::Retail, |word| {
            LodDetail::from_guest_word(word & !Self::MODEL_VIEW)
        })
    }
}

impl NativeSceneLists {
    /// Install LOD adjustment at the guest functions invoked by the fallback
    /// paths, and the D3D draw-boundary geometry swap when the guest's own
    /// draws render the display (`d3d_display`: no distinct readback copy).
    /// These hooks also apply when native scene lists are disabled.
    pub fn install_guest_lod(b: &mut Builder, scale: ViewLodScale, d3d_display: bool) {
        object_lod::install(b, scale);
        display_swap::install(b, scale, d3d_display);
    }

    /// Configure the replacement object setup and record writer for this view.
    pub fn with_lod_scale(scale: ViewLodScale) -> impl Feature {
        ScaledSceneLists(Some(scale))
    }
}

struct ScaledSceneLists(Option<ViewLodScale>);

impl Feature for NativeSceneLists {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Rust scene list builders").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        ScaledSceneLists(None).build(b)
    }
}

impl Feature for ScaledSceneLists {
    fn manifest(&self) -> Manifest {
        NativeSceneLists.manifest()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        // The guest's view projection and visibility producers, replaced
        // unless `impl.render.native.visibility=off` (retire-d3d.md R4);
        // the scene step's shadow arms answer only while it is on.
        let visibility = hooks::switches::native_visibility_on(b.switches());
        crate::scene_step::set_native_visibility(visibility);
        if !enabled(b.switches()) {
            return Ok(());
        }
        view_records::install(b, visibility);
        visibility_setup::install(b, visibility);
        engine::light_frusta::arm::set_on(visibility);
        if visibility {
            portal_walk::install(b);
            install(
                b,
                Seam::Function(&points::VIEW_PROJECTION_BUILD, 0),
                projection::build,
            );
        }
        install(
            b,
            Seam::Function(&points::SCENE_CLUSTER_GATHER, 4),
            objects::gather,
        );
        install(
            b,
            Seam::Function(&points::SCENE_OBJECT_ORDER, 4),
            objects::order,
        );
        install(
            b,
            Seam::Function(&points::SCENE_GEOMETRY_GATHER, 4),
            geometry::gather,
        );
        if visibility {
            window::install(b);
        }
        let scale = self.0;
        // A call the setup cannot build runs the guest's body ([`setup_call`]).
        install(
            b,
            Seam::Function(&points::OBJECT_RENDER_SETUP, 20),
            move |m, cpu| setup_call(m, cpu, scale.map_or(LodDetail::Retail, |scale| scale.read(m))),
        );
        install(
            b,
            Seam::CallSite(
                &points::OBJECT_RECORD_APPEND,
                points::OBJECT_RECORD_APPENDED,
                SceneList::ObjectRecord,
            ),
            move |m, cpu| records::build(m, cpu, scale.map_or(1.0, |scale| scale.read(m).pixel_multiplier())),
        );
        install(
            b,
            Seam::Function(&points::VISIBLE_LIGHT_LIST, 4),
            lights::visible,
        );
        install(
            b,
            Seam::Function(&points::LIGHT_GEOMETRY_FLAGS, 16),
            lights::geometry,
        );
        install(
            b,
            Seam::Function(&points::LIGHT_EXTRA_CASTER, 4),
            lights::extra_caster,
        );
        if visibility {
            regions::install(b);
            sphere_regions::install(b);
            portal_merge::install(b);
            portal_regions::install(b);
        }
        transparents::install_display(b);
        install(
            b,
            Seam::Function(&points::TRANSPARENT_EFFECT_ENTRY, 8),
            transparents::effect,
        );
        install(
            b,
            Seam::Function(&points::TRANSPARENT_POLYGON_ENTRY, 32),
            transparents::polygon,
        );
        install(
            b,
            Seam::Sites(
                &points::TRANSPARENT_GEOMETRY_ENTRY,
                &points::TRANSPARENT_GEOMETRY_QUEUED,
                SceneList::TransparentGeometry,
            ),
            transparents::geometry,
        );
        install(
            b,
            Seam::Sites(
                &points::TRANSPARENT_ORDER_BEGIN,
                &points::TRANSPARENT_ORDER_READY,
                SceneList::Transparents,
            ),
            transparents::build,
        );
        transparents::install_check(b);
        Ok(())
    }
}

/// The existing pre-trim checks, for FrameScene's read-only extraction.
pub(crate) fn light_candidate_eligible(
    m: &dyn GuestMemory,
    record: u32,
    definition: u32,
    flags: u32,
) -> Option<bool> {
    lights::candidate_eligible(m, record, definition, flags).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Memory,
    Layout,
    Unsupported,
    Residency,
    Lighting,
    Palette,
    ObjectModel,
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Memory
    }
}

type Result<T, E = Refusal> = std::result::Result<T, E>;

fn at(base: u32, offset: u32) -> Result<u32> {
    base.checked_add(offset).ok_or(Refusal::Layout)
}

fn ptr(m: &dyn GuestMemory, addr: u32) -> Result<u32> {
    match m.u32(addr)? {
        0 | NONE => Err(Refusal::Layout),
        value => Ok(value),
    }
}

fn stack(m: &dyn GuestMemory, cpu: &CpuAbi, offset: u32) -> Result<u32> {
    Ok(m.u32(at(cpu.esp, offset)?)?)
}

/// The shadow arm of a portal producer (walk, merge, window record). Until
/// [`Audit::PROOF`] guest calls in a row match the Rust result, every call
/// runs the guest and is compared. From then Rust answers every call for
/// the BSP. A difference while proving sends every later call to the
/// guest for the session. A new BSP needs a new proof (`rescope`), and
/// never lifts a fallback.
#[derive(Debug, Default)]
struct Audit {
    clean: u32,
    fallen: bool,
}

impl Audit {
    const PROOF: u32 = 16;

    /// Whether Rust answers this call. `false`: the guest runs, and its
    /// result is compared unless the arm has fallen back.
    fn answers(&mut self) -> bool {
        !self.fallen && self.clean >= Self::PROOF
    }

    /// Whether a guest call is still worth comparing.
    fn compares(&self) -> bool {
        !self.fallen
    }

    /// A compared guest call's result. Returns `true` on the difference
    /// that falls the arm back.
    fn compared(&mut self, same: bool) -> bool {
        if same {
            self.clean = self.clean.saturating_add(1);
            return false;
        }
        self.clean = 0;
        !std::mem::replace(&mut self.fallen, true)
    }

    fn rescope(&mut self) {
        self.clean = 0;
    }

    #[cfg(test)]
    fn fallen_back(&self) -> bool {
        self.fallen
    }
}

fn fp_supported(cpu: &CpuAbi) -> Result<()> {
    // All SSE exceptions masked, round to nearest, gradual underflow.
    if cpu.mxcsr & !0x3f != 0x1f80 {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

fn tag(m: &dyn GuestMemory, table: u32, datum: u32) -> Result<u32> {
    if datum == NONE {
        return Err(Refusal::Layout);
    }
    let entry = at(table, (datum & 0xffff) * 16)?;
    if m.u32(at(entry, 4)?)? != datum {
        return Err(Refusal::Layout);
    }
    ptr(m, at(entry, 8)?)
}

#[derive(Clone, Copy)]
struct Pool {
    data: u32,
    end: u32,
    stride: u32,
}

impl Pool {
    fn read(m: &dyn GuestMemory, header: u32, stride: u32) -> Result<Self> {
        let end = m.u32(at(header, 0x38)?)?;
        if !(1..=0x10000).contains(&stride) || end > 0x10000 || m.u32(at(header, 0x24)?)? != stride
        {
            return Err(Refusal::Layout);
        }
        Ok(Self {
            data: ptr(m, at(header, 0x44)?)?,
            end,
            stride,
        })
    }

    fn slot(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        let i = datum & 0xffff;
        if i >= self.end || datum == NONE || datum >> 16 == 0 {
            return Err(Refusal::Layout);
        }
        let slot = at(self.data, i * self.stride)?;
        if m.u16(slot)? != (datum >> 16) as u16 {
            return Err(Refusal::Layout);
        }
        Ok(slot)
    }

    fn body(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        ptr(m, at(self.slot(m, datum)?, 8)?)
    }
}

struct Write {
    addr: u32,
    bytes: Bytes,
}

enum Bytes {
    Byte([u8; 1]),
    Half([u8; 2]),
    Word([u8; 4]),
    Array(Vec<u8>),
}

impl Bytes {
    fn slice(&self) -> &[u8] {
        match self {
            Self::Byte(bytes) => bytes,
            Self::Half(bytes) => bytes,
            Self::Word(bytes) => bytes,
            Self::Array(bytes) => bytes,
        }
    }
}

#[derive(Default)]
struct Output {
    writes: Vec<Write>,
    eax: Option<u32>,
    ecx: Option<u32>,
    edx: Option<u32>,
    al: Option<u8>,
    pop: u32,
    /// Run the guest body this call, writing nothing: an arm's checked
    /// call ([`crate::native_transparents::sort`]).
    guest: bool,
}

impl Output {
    fn add(&mut self, addr: u32, bytes: Bytes) {
        self.writes.push(Write { addr, bytes });
    }

    fn byte(&mut self, addr: u32, value: u8) {
        self.add(addr, Bytes::Byte([value]));
    }

    fn half(&mut self, addr: u32, value: u16) {
        self.add(addr, Bytes::Half(value.to_le_bytes()));
    }

    fn word(&mut self, addr: u32, value: u32) {
        self.add(addr, Bytes::Word(value.to_le_bytes()));
    }

    fn array(&mut self, addr: u32, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.add(addr, Bytes::Array(bytes));
        }
    }

    fn check_write(&self, mem: &Guest) -> Result<()> {
        for Write { addr, bytes } in &self.writes {
            mem.check(*addr, bytes.slice().len(), Access::Write, 1)
                .map_err(|_| Refusal::Memory)?;
        }
        Ok(())
    }

    fn publish(&self, mem: &mut Guest, pc: u32) -> Result<(), GuestFault> {
        for Write { addr, bytes } in &self.writes {
            mem.write_bytes_from(pc, *addr, bytes.slice(), 1)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Seam {
    Function(&'static Point, u32),
    Sites(&'static Point, &'static Point, SceneList),
    CallSite(&'static Point, u32, SceneList),
}

/// The counter an unbuilt output is filed under.
fn result_of(reason: Refusal) -> SceneListResult {
    match reason {
        Refusal::Memory => SceneListResult::Memory,
        Refusal::Layout => SceneListResult::Layout,
        Refusal::Unsupported => SceneListResult::Unsupported,
        Refusal::Residency => SceneListResult::Residency,
        Refusal::Lighting => SceneListResult::Lighting,
        Refusal::Palette => SceneListResult::Palette,
        Refusal::ObjectModel => SceneListResult::ObjectModel,
    }
}

/// `OBJECT_RENDER_SETUP` (docs/graphics/native-objects.md, "Object setup").
/// A built call publishes its records, stamps and residency requests. A
/// call the setup cannot build runs the guest body, counted `fallback` and
/// under its cause ([`install`]); the guest's setup issues no draw, it
/// writes the records the native scene then draws. The body owns what the
/// build refuses: a root object's fresh light sample (`Refusal::Lighting`:
/// the first person's arms, and the light the weapon copies from them), a
/// locked pose (`Refusal::Palette`), a view other than the main and light
/// views (`Refusal::Unsupported`: the menus' model views, Choose Profile's
/// Spartan) and a residency request that needs a new queue entry
/// (`Refusal::Residency`). The "not drawn" exit is no answer for these: the
/// object is lost in every layer, and a light sample never taken stays
/// stale for every later frame.
fn setup_call(m: &dyn GuestMemory, cpu: &CpuAbi, lod: LodDetail) -> (SceneList, Result<Output>) {
    setup::build(m, cpu, lod)
}

/// What the object setup's residency step does for one geometry resource
/// (`0x0012de70`'s reproducible arms): the guest words it writes, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum ResidencyRequest {
    /// The block is resident, or its request is already queued: the cache
    /// stamps and counters the guest would write.
    Made { resident: bool, writes: Vec<(u32, Vec<u8>)> },
    /// The request needs a new queue entry, a priority raise or a wait:
    /// the guest cache's own service, which the setup leaves to the guest.
    NeedsGuestService,
    /// The resource or the cache does not read.
    Unreadable,
}

/// The residency request the setup makes for `resource` (a geometry
/// resource record) with `request` as the guest's flags give it.
pub fn residency_request(m: &dyn GuestMemory, resource: u32, request: bool) -> ResidencyRequest {
    setup::residency_request(m, resource, request)
}

/// The stage can depend on the view kind, which is read by the builder.
fn install(
    b: &mut Builder,
    seam: Seam,
    build: impl Fn(&dyn GuestMemory, &CpuAbi) -> (SceneList, Result<Output>) + Send + Sync + 'static,
) {
    let point = match seam {
        Seam::Function(point, _) | Seam::Sites(point, _, _) | Seam::CallSite(point, _, _) => {
            point
        }
    };
    b.raw()
        .before(point, "build scene list", move |c| {
            if c.addr != point.base {
                return Ok(Flow::Continue);
            }
            let runtime = Arc::clone(c.ctx.runtime());
            let missing = match seam {
                Seam::Function(_, _) => None,
                Seam::Sites(_, ready, stage) => {
                    (!runtime.read_dispatch(|d| d.has_generated(ready.base))).then_some(stage)
                }
                Seam::CallSite(_, ready, stage) => {
                    (!runtime.read_dispatch(|d| d.has_resume(ready))).then_some(stage)
                }
            };
            if let Some(stage) = missing {
                stage.count(SceneListResult::Fallback);
                stage.count(SceneListResult::Layout);
                return Ok(Flow::Continue);
            }
            let mut mem = runtime.memory();
            let (stage, output) = build(&*mem, c.cpu);
            if matches!(&output, Ok(o) if o.guest) {
                return Ok(Flow::Continue);
            }
            let output = match output.and_then(|output| output.check_write(&mem).map(|()| output)) {
                Ok(output) => output,
                Err(reason) => {
                    stage.count(SceneListResult::Fallback);
                    stage.count(result_of(reason));
                    return Ok(Flow::Continue);
                }
            };
            // Write preflight and publication share the same memory guard.
            // Read the return address before making any guest writes.
            let transfer = match seam {
                Seam::Function(_, pop) => match mem.ret(c.cpu, pop) {
                    Ok(transfer) => transfer,
                    Err(_) => {
                        stage.count(SceneListResult::Fallback);
                        stage.count(SceneListResult::Memory);
                        return Ok(Flow::Continue);
                    }
                },
                Seam::Sites(_, ready, _) => TransferAbi::jump(ready.base),
                Seam::CallSite(_, ready, _) => TransferAbi::resume(ready),
            };
            output.publish(&mut mem, c.addr)?;
            if let Some(eax) = output.eax {
                c.cpu.eax = eax;
            }
            if let Some(ecx) = output.ecx {
                c.cpu.ecx = ecx;
            }
            if let Some(edx) = output.edx {
                c.cpu.edx = edx;
            }
            c.cpu.esp = c.cpu.esp.wrapping_add(output.pop);
            if let Some(al) = output.al {
                c.cpu.eax = (c.cpu.eax & !0xff) | u32::from(al);
            }
            stage.count(SceneListResult::Native);
            Ok(Flow::Return(transfer))
        })
        .optional()
        .context_only()
        .last();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_scale_comes_from_the_supplied_view() {
        use engine::memory::FlatMemory;
        let address = 0x10000;
        let scale = ViewLodScale::new(address);
        let mut live = FlatMemory::new(address, 4);
        assert_eq!(scale.read(&live), LodDetail::Retail);
        live.set_f32(address, 6.0).unwrap();
        let render_snapshot = live.clone();
        live.set_f32(address, 0.5).unwrap();
        assert_eq!(scale.read(&live), LodDetail::Scaled(0.5));
        assert_eq!(scale.read(&render_snapshot), LodDetail::Scaled(6.0));

        for invalid in [-1.0, f32::NAN, f32::INFINITY] {
            live.set_f32(address, invalid).unwrap();
            assert_eq!(scale.read(&live), LodDetail::Retail);
        }
        assert_eq!(scale.read(&FlatMemory::new(address + 4, 4)), LodDetail::Retail);
    }

    #[test]
    fn a_portal_arm_answers_every_call_after_its_proof_and_falls_back_on_a_difference() {
        let mut arm = Audit::default();
        for _ in 0..Audit::PROOF {
            assert!(!arm.answers());
            assert!(!arm.compared(true));
        }
        assert!((0..256).all(|_| arm.answers()), "no call is sampled once proven");
        // A new BSP asks for a new proof.
        arm.rescope();
        assert!(!arm.answers());
        for _ in 0..Audit::PROOF {
            arm.compared(true);
        }
        assert!(arm.answers());
        // The first difference is reported once; the guest runs from then.
        assert!(arm.compared(false));
        assert!(!arm.compared(false));
        assert!(arm.fallen_back() && !arm.compares());
        for _ in 0..Audit::PROOF {
            arm.compared(true);
        }
        arm.rescope();
        assert!((0..256).all(|_| !arm.answers()));
    }

    #[test]
    fn native_scene_lists_follow_the_master_switch() {
        for (spec, expected) in [("", true), ("impl.render.native=off", false), ("impl.render=off", true)] {
            assert_eq!(enabled(&Switches::parse(spec).unwrap()), expected);
        }
    }
}
