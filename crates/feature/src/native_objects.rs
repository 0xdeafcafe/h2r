//! Scene objects drawn through resident render models and the game's pose
//! and material inputs. The walk reads the sorted layer-3 entries before
//! the guest prepares their draws. A complete span of objects can return
//! directly from the geometry handler after GPU reservation. It uses the
//! render context's snapshot of guest memory, including on the pipelined
//! render thread.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::GuestRead;
use native_render::object_walk::{self, Entry, Run, SortedObjects};
use native_render::objects::{ItemKey, SCENE_LAYER3_RETURN};
use timeline::native::{Metric, ObjectWalk, Stage, Timer};

use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-objects";

pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_render_on(switches)
}

/// What the host draws natively right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeSet {
    /// The map the set is for (the per-map counts start again at another).
    pub map: String,
    /// The load the set belongs to: the host's models for one map, BSP
    /// and load (`Host::draw` refuses a view planned against another).
    pub generation: u64,
    /// Object definitions by tag index (the low word of the body's `+0`
    /// datum), each with its model's node count.
    pub definitions: HashMap<u16, usize>,
    /// The parts the host draws, by (definition, section, part): what a
    /// layer-3 item's render record names ([`ItemKey`]). An item
    /// is offered to the host only when its shader is the part's and its
    /// record's implementation one the part has; the rest keep their D3D
    /// draws.
    pub parts: HashMap<(u16, u16, u8), SetPart>,
}

/// One native part: its material's shader (tag index), the
/// implementations it has (bit `n`), and those of them whose program
/// samples the texture accumulation target (drawn only while the game has
/// it bound, [`native_render::object_nodes::texaccum_bound`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetPart {
    pub shader: u16,
    pub implementations: u8,
    pub texaccum: u8,
}

impl NativeSet {
    /// The part an item of `definition` with `key` draws, when the host
    /// draws it with the key's shader and implementation.
    pub fn part(&self, definition: u32, key: &ItemKey) -> Option<SetPart> {
        if !self.definitions.contains_key(&(definition as u16)) {
            return None;
        }
        let p = *self.parts.get(&(definition as u16, key.section, key.part))?;
        (p.shader == key.shader as u16 && key.implementation < 8 && p.implementations & (1 << key.implementation) != 0).then_some(p)
    }

    pub fn accepts(&self, definition: u32, key: &ItemKey) -> bool {
        self.part(definition, key).is_some()
    }

    pub fn node_count(&self, definition: u32) -> Option<usize> {
        self.definitions.get(&(definition as u16)).copied()
    }
}

/// Why an accepted item is given back before the host sees it, or `None`:
/// its object did not read with its model's node count, its record's node
/// rows do not read, or its part samples the texture accumulation target
/// and the game has none bound. `rows`: its record's.
pub fn item_refusal(
    m: &dyn GuestRead,
    set: &NativeSet,
    definition: u32,
    key: &ItemKey,
    frame_ok: bool,
    rows: &Result<Vec<native_render::object_nodes::Row>, String>,
) -> Option<String> {
    if !frame_ok {
        return Some("its object does not read with its model's node count".into());
    }
    let Some(part) = set.part(definition, key) else {
        return Some("not a native part".into());
    };
    if let Err(e) = rows {
        return Some(e.clone());
    }
    if part.texaccum & (1 << key.implementation) != 0 && !native_render::object_nodes::texaccum_bound(m) {
        return Some("it samples the texture accumulation target and none is bound".into());
    }
    None
}

/// What draws the objects: boot's glue over the GPU models and the device.
pub trait Host: Send + Sync {
    /// Start the tag-index load on map activation or a snapshot's first view.
    fn preload(&self, _mem: &dyn GuestMemory) {}

    /// Reserve and commit all of a sorted run. `consume` updates the guest
    /// loop only after all resources and both target copies are ready.
    fn draw_walk(
        &self,
        _mem: &mut dyn GuestMemory,
        _run: &[Entry],
        _consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        Err(ObjectWalk::Reservation)
    }
}

struct WalkLayer {
    list: u32,
    base: u32,
    counter: u32,
    initialized: bool,
    entries: Vec<Entry>,
    runs: VecDeque<Run>,
}

thread_local! {
    static WALK_LAYER: RefCell<Option<WalkLayer>> = const { RefCell::new(None) };
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn begin_walk(mem: &mut dyn GuestMemory, ret: u32) {
    WALK_LAYER.with(|state| *state.borrow_mut() = None);
    if ret != SCENE_LAYER3_RETURN {
        return;
    }
    let Some(host) = host() else { return };
    // Use the same map generation check as activation on snapshot loads.
    host.preload(mem);
    let Some(list) = Mem(mem).read_u32(native_render::skip::CURRENT_LIST) else {
        return;
    };
    WALK_LAYER.with(|state| {
        *state.borrow_mut() = Some(WalkLayer {
            list,
            base: 0,
            counter: 0,
            initialized: false,
            entries: Vec::new(),
            runs: VecDeque::new(),
        })
    });
}

fn initialize_walk(layer: &mut WalkLayer, mem: &mut dyn GuestMemory, esp: u32, esi: u32) {
    let _planning = Timer::start(Stage::Objects, Metric::PrepareNs);
    layer.initialized = true;
    let Some(SortedObjects { base, current, counter, entries }) = object_walk::read(&Mem(mem), layer.list, esp, esi) else {
        ObjectWalk::Unreadable.count(1);
        return;
    };
    let decision = object_walk::decide(&entries, current);
    ObjectWalk::Unsupported.fallback(decision.unsupported);
    ObjectWalk::Order.fallback(decision.interleaved);
    layer.base = base;
    layer.counter = counter;
    layer.entries = entries;
    layer.runs = decision.runs.into();
}

fn walk_geometry(c: &mut crate::Call) -> Result<crate::Flow, runtime::GuestFault> {
    use crate::Flow;
    if WALK_LAYER.with(|state| state.borrow().is_none()) {
        return Ok(Flow::Continue);
    }
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let esp = c.cpu.esp;
    let esi = c.cpu.esi;
    let Ok(caller) = mem.u32(esp) else { return Ok(Flow::Continue) };
    if caller != native_render::light_list::HANDLER_RETURN
        && !c.ctx.live_continuations().iter().any(|k| {
            k.token == caller && k.target == points::LIGHT_GEOMETRY_DRAW.base
                && k.origin == crate::render_loop::SORTED_HANDLER
        })
    {
        return Ok(Flow::Continue);
    }
    // Move the plan out while interacting with the device. No RefCell
    // borrow survives an external boundary.
    let Some(mut layer) = WALK_LAYER.with(|state| state.borrow_mut().take()) else {
        return Ok(Flow::Continue);
    };
    if !layer.initialized {
        initialize_walk(&mut layer, &mut *mem, esp, esi);
    }
    let mut consumed = false;
    if let Some(run) = layer.runs.front() {
        let stride = native_render::skip::ITEM_SIZE;
        if layer.base.checked_add(run.slots.start as u32 * stride) == Some(esi) {
            let run = layer.runs.pop_front().expect("matched the front");
            let skipped = (run.slots.len() - 1) as u32;
            let counter = layer.counter;
            let remaining = (layer.entries.len() - run.slots.start) as u32;
            let last = esi + skipped * stride;
            let mut consume = |mem: &mut dyn GuestMemory| {
                mem.u32(counter) == Ok(remaining)
                    && mem.u32(esp) == Ok(caller)
                    && mem.set_u32(counter, remaining - skipped).is_ok()
            };
            let result = host().ok_or(ObjectWalk::Reservation).and_then(|host| {
                host.draw_walk(&mut *mem, &layer.entries[run.slots], &mut consume)
            });
            match result {
                Ok(()) => {
                    c.cpu.esi = last;
                    ObjectWalk::Native.count(run.objects);
                    consumed = true;
                }
                Err(reason) => reason.fallback(run.objects),
            }
        }
    }
    WALK_LAYER.with(|state| *state.borrow_mut() = Some(layer));
    if consumed {
        drop(mem);
        c.ret(engine::render_loop::layer::HANDLER_CLEANUP).map(Flow::Return)
    } else {
        Ok(Flow::Continue)
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Drops the walk's layer: a snapshot load, whose guest RAM is not the
/// memory it was read from. The loaded item lists are the loaded
/// render's, built again each frame.
pub fn forget_pending() {
    WALK_LAYER.with(|state| *state.borrow_mut() = None);
}

pub struct NativeObjects;

impl Feature for NativeObjects {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Objects' render models drawn and skinned by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Switched off: no hooks, nothing held. On: the host's models are
        // keyed by the map name it reads from guest RAM at every ask, so
        // a load re-keys them. The walk's layer is dropped at a load
        // ([`forget_pending`]).
        if !on(b.switches()) {
            b.snapshot(crate::SnapshotPolicy::Stateless);
            return Ok(());
        }
        b.snapshot(crate::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_pending();
            Ok(())
        }));
        b.game()
            .on_start("native_objects_preload", |o| {
                if let Some(host) = host() {
                    host.preload(&*o.guest());
                }
            })
            .after_feature(crate::native_map_work::ID);
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_objects_skip", |o| {
                if crate::native_map_work::Stage::Objects.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                begin_walk(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        b.raw()
            .before(&points::LIGHT_GEOMETRY_DRAW, "native_object_walk", walk_geometry)
            .after_feature(crate::native_lights::ID)
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "native_object_walk_end", |_| {
                WALK_LAYER.with(|state| *state.borrow_mut() = None);
            })
            .context_only();
        Ok(())
    }
}
