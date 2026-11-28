//! The first-person arms and weapon, drawn by the native GPU renderer from
//! the scene (docs/graphics/native-objects.md, "First person").
//!
//! `0x0003fdf0` is a cut producer ([`crate::cut`]): for both of its calls,
//! argument 0 (before the first-person lights `0x000404b0`) and argument 1
//! (after them), its body never runs, so nothing of it reaches the device,
//! and this feature answers the call. The answer is the scene's: the
//! first-person [`View`] holds the parts the arms and weapon draw, and
//! they draw in [`SLOT`] (slot 10), after the world's transparents
//! (slot 9) and with the first person's lights and sorted pass.
//!
//! There is no D3D fallback and no mask edit. A part that cannot be drawn
//! is a named drop ([`Dropped`]): counted under its cause here and as the
//! stage's `scene_dropped`, and nothing draws in its place.
//!
//! Which call draws a part is the guest's own split (`0x0003fdf0`, the
//! stack argument at `[esp+0x58]` tested at `0x0003ffa0`): argument 0 draws
//! layers 1, 3, 8, 20, 7 and then 2 (`0x00040284`, after `0x00044370` with
//! layer 2 chosen), argument 1 draws layer 16 only (`0x0003ffbf` sets it,
//! `0x00040010` jumps to the draw at `0x00040284`). Layer 16 is
//! [`LATE_LAYER`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::entry::{Cx, EntryError, EntryFuture, EntryResult, Ret};
use engine::render::light_gather::{first_person_effects, first_person_items};
use scene_extract::layout::skip::{CURRENT_LAYER, CURRENT_LIST};
use frame_scene::{FrameScene, View, ViewKind, ViewSlot};
use timeline::native::{Metric, Stage};

use crate::cut;
use crate::frame_step;
use crate::native_objects::NativeSet;
use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-first-person";


pub use crate::cut::FIRST_PERSON as CUT;

/// The view slot the first person draws in: slot 10, after the world's
/// transparents (slot 9).
pub const SLOT: ViewSlot = ViewSlot::FirstPerson;

/// The one layer the second call draws; the first draws the others
/// (1, 3, 8, 20, 7 and 2).
pub const LATE_LAYER: u8 = 16;

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// Why a part is not drawn. Each is a named count, never a D3D draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The scene holds no first-person view for the call.
    NoView,
    /// No host is installed to draw.
    NoHost,
    /// The part's rows, instance or range do not read in the scene.
    Unreadable,
    /// The part's model is not resident (it is asked for).
    Unresident,
    /// The model is resident but not this part, or its implementation.
    Unsupported,
    /// The host could not reserve what the draw needs.
    Reservation,
    /// The call's item build refused (a record or shader it does not
    /// read): the call adds no part.
    Items,
    /// The first-person effects (`0x00174220`) stopped at a system they
    /// do not read: the systems after it are not updated this call.
    Effects,
}

impl Dropped {
    pub const ALL: [Dropped; 8] = [
        Dropped::NoView,
        Dropped::NoHost,
        Dropped::Unreadable,
        Dropped::Unresident,
        Dropped::Unsupported,
        Dropped::Reservation,
        Dropped::Items,
        Dropped::Effects,
    ];

    /// The cause's name in `render.scene`.
    pub fn name(self) -> &'static str {
        match self {
            Dropped::NoView => "no_view",
            Dropped::NoHost => "no_host",
            Dropped::Unreadable => "unreadable",
            Dropped::Unresident => "unresident",
            Dropped::Unsupported => "unsupported",
            Dropped::Reservation => "reservation",
            Dropped::Items => "items",
            Dropped::Effects => "effects",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// What one call did: parts drawn, and parts dropped by cause.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub drawn: usize,
    pub dropped: Vec<(Dropped, usize)>,
}

impl Outcome {
    pub fn drop(&mut self, why: Dropped, n: usize) {
        if n > 0 {
            self.dropped.push((why, n));
        }
    }

    /// Every part the call dropped.
    pub fn dropped_total(&self) -> usize {
        self.dropped.iter().map(|(_, n)| n).sum()
    }
}

/// What draws the first person: boot's glue over the GPU models.
pub trait Host: Send + Sync {
    fn retire_map(&self) {}

    /// The model set drawn natively now; starts the load on a first call.
    fn native(&self, mem: &mut dyn GuestMemory, prepare_draw: bool) -> Option<Arc<NativeSet>>;
    /// Asks for a first-person model that is not resident (cheap to repeat).
    fn want(&self, model: u32);
    /// Draws `parts` (indices into `scene.parts`, in the view's order) of
    /// `view` in `slot` for call `mode`. Every part it does not draw is
    /// in the outcome's drops; it makes no guest draw.
    fn draw(
        &self,
        mem: &mut dyn GuestMemory,
        scene: &FrameScene,
        view: &View,
        slot: ViewSlot,
        mode: u32,
        parts: &[u32],
    ) -> Outcome;
    /// Plans `parts` as [`Host::draw`] would, reserving and drawing
    /// nothing ([`probe`]): `drawn` the parts it would draw.
    fn plan(
        &self,
        _mem: &mut dyn GuestMemory,
        _scene: &FrameScene,
        _view: &View,
        parts: &[u32],
    ) -> Outcome {
        let mut out = Outcome::default();
        out.drop(Dropped::Reservation, parts.len());
        out
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static DROPS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub(crate) fn retire_map() {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(host) = host {
        host.retire_map();
    }
}

/// Calls answered, and parts drawn.
pub fn counts() -> (u64, u64) {
    (CALLS.load(Ordering::Relaxed), DRAWN.load(Ordering::Relaxed))
}

/// The stage's counters for `render.scene`: calls answered, parts drawn
/// and parts dropped by cause.
pub fn counters() -> Vec<(String, u64)> {
    let (calls, drawn) = counts();
    let mut out = vec![
        ("first_person_calls".to_owned(), calls),
        ("first_person_drawn".to_owned(), drawn),
    ];
    out.extend(drops().into_iter().map(|(name, n)| (format!("first_person_dropped_{name}"), n)));
    out
}

/// Parts dropped, by cause (`render.scene`: `first_person_dropped_<cause>`).
pub fn drops() -> Vec<(&'static str, u64)> {
    Dropped::ALL
        .iter()
        .map(|c| (c.name(), DROPS[c.index()].load(Ordering::Relaxed)))
        .collect()
}

/// Whether call `mode` draws a part of `layer`.
pub fn draws_layer(mode: u32, layer: u8) -> bool {
    (mode == 1) == (layer == LATE_LAYER)
}

/// One call, from the scene: the readable parts of `view` that `mode`
/// draws are offered to the host in the view's order; every other is
/// dropped under its cause.
pub fn offer(
    host: &dyn Host,
    mem: &mut dyn GuestMemory,
    scene: &FrameScene,
    view: &View,
    mode: u32,
) -> Outcome {
    offer_by(host, mem, scene, view, mode, &mut |host, mem, parts| host.draw(mem, scene, view, SLOT, mode, parts))
}

/// [`offer`] planning only ([`Host::plan`]): what the native path would
/// draw of the call's parts, and what it would drop.
pub fn probe(
    host: &dyn Host,
    mem: &mut dyn GuestMemory,
    scene: &FrameScene,
    view: &View,
    mode: u32,
) -> Outcome {
    offer_by(host, mem, scene, view, mode, &mut |host, mem, parts| host.plan(mem, scene, view, parts))
}

/// The parts of `view` call `mode` draws, in the view's order.
pub fn call_parts(scene: &FrameScene, view: &View, mode: u32) -> usize {
    let range = view.order.start as usize..view.order.end as usize;
    scene.order.get(range).map_or(0, |order| {
        order
            .iter()
            .filter(|&&i| scene.parts.get(i as usize).is_some_and(|p| draws_layer(mode, p.layer)))
            .count()
    })
}

type Draw<'a> = dyn FnMut(&dyn Host, &mut dyn GuestMemory, &[u32]) -> Outcome + 'a;

fn offer_by(
    host: &dyn Host,
    mem: &mut dyn GuestMemory,
    scene: &FrameScene,
    view: &View,
    mode: u32,
    draw: &mut Draw<'_>,
) -> Outcome {
    let mut out = Outcome::default();
    let range = view.order.start as usize..view.order.end as usize;
    let Some(order) = scene.order.get(range) else {
        out.drop(Dropped::Unreadable, view.order.len());
        return out;
    };
    let set = host.native(mem, false);
    let mut offered = Vec::with_capacity(order.len());
    for &index in order {
        let Some(part) = scene.parts.get(index as usize) else {
            out.drop(Dropped::Unreadable, 1);
            continue;
        };
        if !draws_layer(mode, part.layer) {
            continue;
        }
        let rows = part.rows.start as usize..part.rows.end as usize;
        let (Some(instance), Some(_)) = (scene.instances.get(part.instance as usize), scene.rows.get(rows)) else {
            out.drop(Dropped::Unreadable, 1);
            continue;
        };
        let Some(set) = set.as_deref() else {
            out.drop(Dropped::Unresident, 1);
            continue;
        };
        if !set.definitions.contains_key(&(instance.model as u16)) {
            host.want(instance.model);
            out.drop(Dropped::Unresident, 1);
        } else if !set.accepts(instance.model, &part.key) {
            out.drop(Dropped::Unsupported, 1);
        } else {
            offered.push(index);
        }
    }
    if offered.is_empty() {
        return out;
    }
    let drawn = draw(host, mem, &offered);
    out.drawn += drawn.drawn;
    out.dropped.extend(drawn.dropped);
    out
}

/// The prologue of `0x0003fdf0` ([`first_person_items::prologue`]),
/// before the first-person effects: `false` when it does not read.
pub fn begin(mem: &mut dyn GuestMemory) -> bool {
    first_person_items::prologue(&*mem).is_ok_and(|w| w.apply(mem).is_ok())
}

/// The call's items into the scene, as `0x0003fdf0`'s body makes them
/// after its prologue and effects, up to its draws: the record loop's
/// writes ([`first_person_items::records`]), then each layer the call
/// draws, with the current layer and list the guest's layer draw reads,
/// into the first person's view, then the epilogue. `false` when the
/// build refuses or its writes fault: the view holds no part of this
/// call.
pub fn build(mem: &mut dyn GuestMemory, eax: u32, call: u32) -> bool {
    let Ok(items) = first_person_items::records(&*mem, eax) else {
        return false;
    };
    if items.writes.apply(mem).is_err() {
        return false;
    }
    for &(layer, gate) in first_person_items::layers(call) {
        let gated = gate.is_none_or(|at| mem.u8(at).is_ok_and(|g| g != 0));
        if items.layers & (1 << layer) != 0 && gated {
            let _ = mem.set_u32(CURRENT_LAYER, u32::from(layer));
            let _ = mem.set_u32(CURRENT_LAYER + 4, 0);
            let _ = mem.set_u32(CURRENT_LIST, first_person_items::LIST);
            crate::scene_step::first_person_layer(&*mem, layer);
        } else if gate.is_none() || layer != 2 {
            // `0x00044370` writes nothing for a layer it does not draw.
            let _ = mem.set_u32(CURRENT_LAYER, 0);
        }
    }
    first_person_items::restore(&*mem).is_ok_and(|w| w.apply(mem).is_ok())
}

/// Answers one call of `0x0003fdf0` (EAX `eax`, the stack argument
/// `call`) where the first-person effects change nothing: [`begin`],
/// then [`finish`].
pub fn answer(mem: &mut dyn GuestMemory, eax: u32, call: u32) -> Outcome {
    if !begin(mem) {
        let mut out = Outcome::default();
        out.drop(Dropped::Items, 1);
        count(&out);
    }
    finish(mem, eax, call)
}

/// The rest of a call after its prologue and effects: builds its items
/// into the first person's view ([`build`]), offers the view's parts, and
/// counts what was drawn and what was dropped. The view's camera is the
/// render camera the game view drew with: `0x0003fdf0` and its call site
/// set none of their own.
pub fn finish(mem: &mut dyn GuestMemory, eax: u32, call: u32) -> Outcome {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _ = crate::scene_step::open_kind(&*mem, ViewKind::FirstPerson);
    if !build(mem, eax, call) {
        let mut out = Outcome::default();
        out.drop(Dropped::Items, 1);
        count(&out);
    }
    let mode = call;
    let out = crate::scene_step::with_open(|open| match (host.as_deref(), open) {
        (None, Some((_, view))) => {
            let mut out = Outcome::default();
            out.drop(Dropped::NoHost, view.order.len());
            out
        }
        (Some(host), Some((scene, view))) if view.key.kind == ViewKind::FirstPerson => {
            offer(host, mem, scene, view, mode)
        }
        // No first-person view: the call has nothing to draw, which is
        // not a part dropped, but it is counted.
        _ => {
            let mut out = Outcome::default();
            out.drop(Dropped::NoView, 1);
            out
        }
    });
    count(&out);
    out
}

fn count(out: &Outcome) {
    DRAWN.fetch_add(out.drawn as u64, Ordering::Relaxed);
    for &(why, n) in &out.dropped {
        DROPS[why.index()].fetch_add(n as u64, Ordering::Relaxed);
        Stage::FirstPerson.count(Metric::SceneDropped, n as u64);
    }
}

pub struct NativeFirstPerson;

impl Feature for NativeFirstPerson {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "First-person arms and weapon drawn from the scene by the native GPU renderer")
            .doc("docs/graphics/native-objects.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        // A cut (`crate::cut::OWN_PRODUCER`): every call is answered here,
        // after the frame step's writes for it, and the body never runs.
        let point = cut::point(&CUT);
        b.raw()
            .implement(point, "native_first_person_skip", move |cpu, ctx, addr| {
                if addr != point.base {
                    return original(cpu, ctx, addr);
                }
                hooks::entry::start_render(cpu, ctx, addr, |cx: Cx| -> EntryFuture { Box::pin(cut_call(cx)) })
            })
            .optional()
            .context_only();
        Ok(())
    }
}

/// The original body at `addr`.
fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(runtime::TransferAbi::fault(event))
}

/// One call of `0x0003fdf0` (the view in EAX, the call on the
/// stack): the frame step's writes, the prologue, the first-person effects
/// (`0x00174220`, [`first_person_effects::run`], whose particle updates
/// are guest calls), then the items and their draws ([`finish`]).
async fn cut_call(cx: Cx) -> EntryResult {
    let entry = cx.entry();
    let ret = Ret::new(CUT.abi_result, CUT.pop);
    cx.write(|m| frame_step::before(m, CUT.name))?;
    // The call is the stack's first argument; one that does not read is
    // a drop, never a guest draw.
    let Ok(call) = cx.arg(0) else {
        count(&Outcome { drawn: 0, dropped: vec![(Dropped::Unreadable, 1)] });
        return Ok(ret);
    };
    if !cx.write(|m| begin(m)) {
        count(&Outcome { drawn: 0, dropped: vec![(Dropped::Items, 1)] });
    } else {
        match first_person_effects::run(&cx, entry.esp).await {
            Ok(_) => {}
            Err(first_person_effects::Stop::Entry(e)) => return Err(e),
            Err(first_person_effects::Stop::Refused(_)) => {
                count(&Outcome { drawn: 0, dropped: vec![(Dropped::Effects, 1)] });
            }
        }
    }
    cx.write(|m| finish(m, entry.eax, call));
    Ok::<_, EntryError>(ret)
}
