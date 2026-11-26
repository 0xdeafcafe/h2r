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
use frame_scene::{FrameScene, View, ViewKind, ViewSlot};
use runtime::guest::{Access, FaultReason};
use runtime::GuestFault;
use timeline::native::{Metric, Stage};

use crate::cut::{self, Cut};
use crate::frame_step;
use crate::native_objects::NativeSet;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-first-person";

/// The first-person list the guest built at `0x004c8740` (the transparent
/// stage still tells it apart from the world's).
pub(crate) const FIRST_PERSON_ITEMS: u32 = 0x004c_8740;

/// The cut: `0x0003fdf0` takes the view in `eax` and the call (0 or 1) on
/// the stack, `ret 4`; its answer is `al = 1`, which no caller reads.
pub const CUT: Cut = Cut {
    name: "first_person",
    entry: 0x0003_fdf0,
    abi_result: 1,
    pop: 4,
};

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
}

impl Dropped {
    pub const ALL: [Dropped; 6] = [
        Dropped::NoView,
        Dropped::NoHost,
        Dropped::Unreadable,
        Dropped::Unresident,
        Dropped::Unsupported,
        Dropped::Reservation,
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
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static DROPS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

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
    let drawn = host.draw(mem, scene, view, SLOT, mode, &offered);
    out.drawn += drawn.drawn;
    out.dropped.extend(drawn.dropped);
    out
}

/// Answers one call of `0x0003fdf0`: opens the first person's view, offers
/// its parts, and counts what was drawn and what was dropped.
pub fn answer(mem: &mut dyn GuestMemory, mode: u32) -> Outcome {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _ = crate::scene_step::open_kind(&*mem, ViewKind::FirstPerson);
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
            .before(point, "native_first_person_skip", move |c| {
                if c.addr != point.base {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                frame_step::before(&mut *mem, CUT.name).map_err(|f| GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write { Access::Write } else { Access::Read },
                    reason: FaultReason::Unmapped,
                })?;
                // The call is the stack's first argument; one that does not
                // read is a drop, never a guest draw.
                match mem.u32(c.cpu.esp.wrapping_add(4)) {
                    Ok(mode) => {
                        answer(&mut *mem, mode);
                    }
                    Err(_) => count(&Outcome {
                        drawn: 0,
                        dropped: vec![(Dropped::Unreadable, 1)],
                    }),
                }
                let transfer = mem.ret(c.cpu, u32::from(CUT.pop))?;
                c.cpu.eax = CUT.abi_result;
                Ok(Flow::Return(transfer))
            })
            .optional()
            .context_only()
            .last();
        Ok(())
    }
}
