//! Marks the guest's sorted geometry entries for the resolved indexed-draw
//! path. The guest runs each handler, including its state and texture setup.
//! The two stack frames identify the entry while the indexed call is live.
//! Push-buffer work and other kinds of handlers keep their ordinary path.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::{memory::GuestMemory, points};
use native_render::camera::GuestRead;
use native_render::transparent::{self, Entry};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, ReservationCause, Stage};

/// The list's order in Rust and its arm (`nor-7a`).
pub mod sort;

/// The first-person list the guest built at `0x004c8740` (the transparent
/// stage still tells it apart from the world's).
pub const FIRST_PERSON_ITEMS: u32 = 0x004c_8740;

pub const ID: &str = "native-transparents";

use points::scene_effects::{
    GEOMETRY_RANGE_READY, SCENE_TRANSPARENTS_END, SUBPART_DRAWS,
};
const GEOMETRY_RANGE_TAIL: u32 = 0x0004_dae0;
const GEOMETRY_EMITTER_CALLER: u32 = 0x0004_d626;
const TRANSPARENT_DRAW_CALLER: u32 = 0x0004_1010;
const LAYER_DRAW_CALLER: u32 = native_render::light_list::HANDLER_RETURN;
const LAYER_ENTRIES_FRAME: u32 = 0xf094;
const LAYER_ENTRIES_RETURN: u32 = 0x0004_45b7;
const LAYER_DRAW_FRAME: u32 = 0xf0a4;
const SORTED_HANDLER_RETURN: u32 = 0x0001_e3f3;
const LIST_COUNT: u32 = 0x004b_6280;
const LIST_FIRST: u32 = 0x004b_6284;
const LIST_ENTRIES: u32 = 0x004b_6288;
const LIST_ORDER: u32 = 0x004b_628c;
const ENTRY_BYTES: u32 = 0x70;
const MAX_SORTED: u32 = 0x400;

pub enum IndexTopology {
    List,
    Strip,
}

pub struct IndexRange {
    pub indices: u32,
    pub count: u32,
    pub topology: IndexTopology,
}

pub trait WalkHost: Send + Sync {
    fn sorted_pools(&self, _mem: &dyn GuestMemory) -> Option<Arc<BTreeSet<u32>>> { None }
    fn retire_bsp(&self) {}

    /// Draw `range` natively, or name why the reservation did not commit.
    fn draw(&self, mem: &mut dyn GuestMemory, stage: Stage, range: IndexRange) -> Result<(), ReservationCause>;
}

static WALK: Mutex<Option<Arc<dyn WalkHost>>> = Mutex::new(None);

pub fn set_walk_host(host: Arc<dyn WalkHost>) {
    *WALK.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub(crate) fn retire_bsp() {
    let walk = WALK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(walk) = walk { walk.retire_bsp(); }
}

fn sorted_data(mem: &dyn GuestMemory, ordinal: u32) -> Option<u32> {
    let first = mem.u32(LIST_FIRST).ok()?;
    let end = mem.u32(LIST_COUNT).ok()?;
    if end > MAX_SORTED || ordinal >= end.checked_sub(first)? {
        return None;
    }
    let order = mem.u32(LIST_ORDER).ok()?;
    let entries = mem.u32(LIST_ENTRIES).ok()?;
    if order == 0 || entries == 0 {
        return None;
    }
    let index = u32::from(mem.u16(order.checked_add(2 * ordinal)?).ok()?);
    if !(first..end).contains(&index) {
        return None;
    }
    entries.checked_add(index * ENTRY_BYTES + transparent::DATA_OFFSET)
}

thread_local! {
    static POOLS: RefCell<Option<Arc<BTreeSet<u32>>>> = const { RefCell::new(None) };
    static WALK_DATA: Cell<Option<u32>> = const { Cell::new(None) };
}

fn sorted_layer_call(layer: u32, caller: u32) -> bool {
    use native_render::world_layers::sorted_site;

    // Special layers have distinct target setup and known return sites.
    // The ordinary layers run inside the same two handlers on their own lists.
    sorted_site(layer, caller).is_some()
        || ((points::scene_effects::EFFECT_LAYERED_A.base..0x0004_20a0).contains(&caller)
            && matches!(layer, 1 | 2 | 3 | 7 | 8 | 15 | 16 | 20))
        || ((points::scene_effects::EFFECT_LAYERED_B.base..0x0004_2760).contains(&caller)
            && matches!(layer, 1 | 2 | 3 | 7 | 8 | 15 | 16))
}

fn live_layer(mem: &dyn GuestRead, esp: u32, layer: u32) -> bool {
    mem.read_u32(native_render::skip::CURRENT_LAYER) == Some(layer)
        && esp.checked_add(0xf094).and_then(|at| mem.read_u32(at)) == Some(0x0004_45b7)
}

/// The DrawIndexedVertices API frame includes the return address and three
/// arguments below the standard emitter frame. Only its four proved calls
/// may carry a replacement of sorted-layer geometry into a sealed scene.
pub fn layered_indexed_call(mem: &dyn GuestRead, api_esp: u32) -> bool {
    let Some(ret) = mem.read_u32(api_esp) else { return false };
    if ret != GEOMETRY_RANGE_TAIL && !SUBPART_DRAWS.iter().any(|&(_, resume)| resume == ret) {
        return false;
    }
    let Some(esp) = api_esp.checked_add(16) else { return false };
    let word = |offset| mem.read_u32(esp.checked_add(offset)?);
    if word(0x18) != Some(GEOMETRY_EMITTER_CALLER) || word(0x68) != Some(LAYER_DRAW_CALLER) {
        return false;
    }
    let (Some(layer), Some(caller)) = (word(0x78), word(LAYER_DRAW_FRAME)) else { return false };
    live_layer(mem, esp, layer) && sorted_layer_call(layer, caller)
}

/// The standard handler saves the signed record index at its ESP+0x3c
/// (0x4d131) and keeps the corresponding record pointer in EBP (0x4d141).
/// The emitter saves that EBP before reusing it. Its frame includes the
/// handler's three arguments and return address. The handler has already
/// reused the original packed part argument by this point (0x4d147).
fn emits_object(mem: &dyn GuestRead, esp: u32) -> Option<bool> {
    use native_render::{objects::RECORD_MODEL, skip::{RENDER_RECORDS, RENDER_RECORD_SIZE}};
    const RECORD_COUNT: u32 = 0x004c_0b78;
    const RECORD_CAPACITY: u32 = 0x352;
    let index = mem.read_u32(esp.checked_add(0x64)?)?;
    if index >= mem.read_u32(RECORD_COUNT)?.min(RECORD_CAPACITY) {
        return None;
    }
    let record = RENDER_RECORDS + index * RENDER_RECORD_SIZE;
    if mem.read_u32(esp.checked_add(8)?)? != record {
        return None;
    }
    // 0x4d157 selects model geometry by this datum, including unlit
    // objects for which a lighting-cache owner cannot be recovered.
    Some(mem.read_u32(record + RECORD_MODEL)? != u32::MAX)
}

fn walk_stage(mem: &dyn GuestRead, esp: u32) -> Option<Stage> {
    let word = |offset| mem.read_u32(esp.checked_add(offset)?);
    if word(0x18)? != GEOMETRY_EMITTER_CALLER {
        return None;
    }
    let layer = word(0x78)?;
    match word(0x68)? {
        TRANSPARENT_DRAW_CALLER if layer == 15 => {
            // Pools use the resolved D3D issue below, so a replay can keep
            // the admission beside the exact packet and its texture inputs.
            (!sorted_pool(mem, esp)?).then_some(Stage::Transparents)
        }
        LAYER_DRAW_CALLER => {
            use native_render::world_layers::{Site, layer_site, sorted_site};
            let caller = word(LAYER_DRAW_FRAME)?;
            // The layer's sorted dispatcher has already applied the pass and
            // chosen this range. Require the live handler and current layer
            // to agree even at the first-person renderer's overlay call.
            let overlay_fog = layer == 16 && caller == scene_extract::late::FIRST_PERSON_OVERLAY_RETURN;
            if (scene_extract::late::scene_layer_call(layer, caller) || overlay_fog)
                && live_layer(mem, esp, layer)
            {
                return Some(if layer == 16 { Stage::Fog } else { Stage::Water });
            }
            let site = layer_site(layer, caller);
            let list = mem.read_u32(native_render::skip::CURRENT_LIST)?;
            if list == 0 { return None; }
            let items = mem.read_u32(list.checked_add(0x14)?)?;
            if items == FIRST_PERSON_ITEMS {
                return (site == Site::FirstPerson).then_some(Stage::FirstPerson);
            }
            // Resident plans consume their ranges before the emitter.
            // A residual model draw reaches this point with its streams,
            // constants and render states already bound by the handler.
            if items != 0 && (matches!(site, Site::Scene | Site::AlternateView) || sorted_site(layer, caller).is_some())
                && mem.read_u32(native_render::skip::CURRENT_LAYER) == Some(layer)
                && word(LAYER_ENTRIES_FRAME) == Some(LAYER_ENTRIES_RETURN)
                && emits_object(mem, esp)?
            {
                Some(Stage::Objects)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn sorted_pool(mem: &dyn GuestRead, esp: u32) -> Option<bool> {
    let scope = SCOPE.take();
    let handler = scope.live_handler(mem, esp);
    SCOPE.set(scope);
    let handler = handler?;
    if WALK_DATA.get() != Some(handler.data) { return None; }
    Some(POOLS.with_borrow(|p| p.as_ref().is_some_and(|p| p.contains(&handler.shader))))
}

/// The pool's resolved indexed call, with its three arguments still on the stack.
pub fn pool_issue(mem: &dyn GuestRead, esp: u32) -> Option<(u32, u32)> {
    WALK_DATA.get()?;
    let ret = mem.read_u32(esp)?;
    if ret != GEOMETRY_RANGE_TAIL && !SUBPART_DRAWS.iter().any(|&(_, end)| end == ret) {
        return None;
    }
    let frame = esp.checked_add(16)?;
    if mem.read_u32(frame.checked_add(0x18)?)? != GEOMETRY_EMITTER_CALLER
        || mem.read_u32(frame.checked_add(0x68)?)? != TRANSPARENT_DRAW_CALLER
        || mem.read_u32(frame.checked_add(0x78)?)? != 15
        || !sorted_pool(mem, frame)?
    {
        return None;
    }
    Some((mem.read_u32(engine::render_loop::occlusion::guest::VIEW_INDEX)?,
        mem.read_u32(esp.checked_add(8)?)?))
}

fn subpart_range(mem: &dyn GuestRead, esp: u32) -> Option<IndexRange> {
    let topology = match mem.read_u32(esp)? {
        5 => IndexTopology::List,
        6 => IndexTopology::Strip,
        _ => return None,
    };
    Some(IndexRange {
        topology,
        count: mem.read_u32(esp.checked_add(4)?)?,
        indices: mem.read_u32(esp.checked_add(8)?)?,
    })
}

impl IndexRange {
    fn has_indices_for(&self, stage: Stage) -> bool {
        // Both indexed backends consume the full list and rasterize its
        // complete triangles. Preserve the tail in the device snapshot.
        self.indices != 0 && self.count >= 3
            && (matches!(stage, Stage::Transparents | Stage::FirstPerson | Stage::Objects) || !matches!(self.topology, IndexTopology::List)
                || self.count % 3 == 0)
    }
}

fn walk_range(mem: &mut dyn GuestMemory, stage: Stage, range: IndexRange) -> bool {
    stage.count(Metric::WalkExamined, 1);
    if !range.has_indices_for(stage) {
        stage.count(Metric::WalkRefusedInputs, 1);
        return false;
    }
    let host = WALK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let result = host.map_or(Err(ReservationCause::Declined), |host| host.draw(mem, stage, range));
    match result {
        Ok(()) => stage.count(Metric::WalkSuppressed, 1),
        Err(cause) => {
            stage.count(Metric::WalkRefusedReservation, 1);
            stage.reservation_cause(cause);
        }
    }
    result.is_ok()
}

pub fn on(_: &hooks::Switches) -> bool {
    true
}

static ACTIVATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapGeneration {
    activation: u64,
    globals: u32,
    bsp: u32,
    bsp_index: u16,
}

impl MapGeneration {
    fn read(mem: &dyn GuestRead) -> Option<Self> {
        let globals = mem.read_u32(points::GAME_GLOBALS.base)?;
        let bsp = mem.read_u32(crate::native_world::ACTIVE_BSP)?;
        if globals == 0 || bsp == 0 {
            return None;
        }
        let index = (mem.read_u32(globals)? >> 16) as i16;
        Some(Self {
            activation: ACTIVATION.load(Ordering::Acquire),
            globals,
            bsp,
            bsp_index: u16::try_from(index).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct Frame {
    esp: u32,
    return_to: u32,
}

impl Frame {
    fn read(mem: &dyn GuestRead, esp: u32) -> Option<Self> {
        let return_to = mem.read_u32(esp)?;
        (return_to != 0).then_some(Self { esp, return_to })
    }

    fn contains(self, mem: &dyn GuestRead, child_esp: u32) -> bool {
        child_esp < self.esp && mem.read_u32(self.esp) == Some(self.return_to)
    }
}

#[derive(Clone, Copy)]
struct Scene {
    esp: u32,
    frame: Option<Frame>,
    map: MapGeneration,
}

#[derive(Clone, Copy)]
struct Handler {
    frame: Frame,
    data: u32,
    entry: Option<Entry>,
    shader: u32,
    part_argument: u32,
}

#[derive(Default)]
pub struct Scope {
    scene: Option<Scene>,
    handler: Option<Handler>,
}

impl Scope {
    pub fn begin_view(&mut self, mem: &dyn GuestRead, esp: u32) {
        self.scene = MapGeneration::read(mem).map(|map| Scene { esp, frame: None, map });
        self.handler = None;
    }

    pub fn begin_entry(&mut self, mem: &dyn GuestRead, esp: u32) {
        if let Some(scene) = &mut self.scene
            && scene.frame.is_none()
            && esp < scene.esp
        {
            // Capture the live scene frame only once the dispatcher enters
            // its first geometry handler.
            scene.frame = Frame::read(mem, scene.esp);
        }
        self.handler = self.read_handler(mem, esp);
    }

    fn read_handler(&self, mem: &dyn GuestRead, esp: u32) -> Option<Handler> {
        let scene = self.scene?;
        let frame = Frame::read(mem, esp)?;
        // The next function after the sorted-list dispatch is the effects
        // allocator. An invocation from another caller has no sorted slot.
        const NEXT_AFTER_SORTED_DISPATCH: u32 = 0x0001_e4e0;
        if !scene.frame?.contains(mem, esp)
            || !(points::SCENE_TRANSPARENTS.base..NEXT_AFTER_SORTED_DISPATCH).contains(&frame.return_to)
        {
            return None;
        }
        let data = mem.read_u32(esp.checked_add(4)?)?;
        if mem.read_u32(data.checked_sub(4)?)? != transparent::GEOMETRY_HANDLER {
            return None;
        }
        Some(Handler {
            frame, data, entry: transparent::read_entry(mem, data),
            shader: mem.read_u32(data)?,
            part_argument: mem.read_u32(data.checked_add(4)?)?,
        })
    }

    /// Returns the exact sorted part only while its call frame survives.
    /// The dispatch reuses the frame for the next handler and always writes
    /// that handler's own entry pointer to its argument slot.
    fn live_handler(&self, mem: &dyn GuestRead, esp: u32) -> Option<Handler> {
        let scene = self.scene?;
        let handler = self.handler?;
        if !scene.frame?.contains(mem, handler.frame.esp)
            || !handler.frame.contains(mem, esp)
            || mem.read_u32(handler.frame.esp.checked_add(4)?) != Some(handler.data)
            || mem.read_u32(handler.data.checked_sub(4)?) != Some(transparent::GEOMETRY_HANDLER)
            || mem.read_u32(handler.data) != Some(handler.shader)
            || mem.read_u32(handler.data.checked_add(4)?) != Some(handler.part_argument)
            || MapGeneration::read(mem) != Some(scene.map)
        {
            return None;
        }
        Some(handler)
    }

    pub fn indexed_entry(&self, mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
        let handler = self.live_handler(mem, esp)?;
        let entry = handler.entry?;
        let live = transparent::read_entry(mem, handler.data)?;
        (live == entry).then_some((self.scene?.map, live))
    }
}

thread_local! {
    // D3D resolves the indexed API call on the same guest execution thread
    // as the hook. The GPU worker receives only immutable resolved data.
    static SCOPE: Cell<Scope> = Cell::new(Scope::default());
}

/// Without a sorted handler the indexed draw cannot use the transparent adapter.
pub fn in_indexed_handler() -> bool {
    SCOPE.with(|slot| {
        let scope = slot.take();
        let active = scope.handler.is_some();
        slot.set(scope);
        active
    })
}

pub fn indexed_entry(mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
    SCOPE.with(|slot| {
        let scope = slot.take();
        let entry = scope.indexed_entry(mem, esp);
        slot.set(scope);
        entry
    })
}

struct ReadMemory<'a>(&'a dyn GuestMemory);

impl GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

pub struct NativeTransparents;

impl Feature for NativeTransparents {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Ordered native indexed draws for sorted transparents")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Both dispatch scopes last one view. A save occurs after the
        // render has drained, and the next view starts with fresh scopes.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        for (site, resume) in SUBPART_DRAWS {
            b.raw().before(site, "native_late_subpart_draw", move |c| {
                // The three DrawIndexedVertices arguments sit below the emitter frame.
                let Some(emitter_esp) = c.cpu.esp.checked_add(12) else {
                    return Ok(Flow::Continue);
                };
                let rt = Arc::clone(c.ctx.runtime());
                let can_resume = rt.read_dispatch(|dispatch| dispatch.has_resume(resume));
                let mut mem = rt.memory();
                let read = ReadMemory(&*mem);
                let Some(stage) = walk_stage(&read, emitter_esp)
                else {
                    return Ok(Flow::Continue);
                };
                if !can_resume {
                    stage.count(Metric::WalkExamined, 1);
                    stage.count(Metric::WalkRefusedResume, 1);
                    return Ok(Flow::Continue);
                }
                let Some(range) = subpart_range(&read, c.cpu.esp) else {
                    stage.count(Metric::WalkExamined, 1);
                    stage.count(Metric::WalkRefusedInputs, 1);
                    return Ok(Flow::Continue);
                };
                if !walk_range(&mut *mem, stage, range) {
                    return Ok(Flow::Continue);
                }
                c.cpu.esp = emitter_esp;
                Ok(Flow::Return(TransferAbi::resume(resume)))
            }).context_only().optional();
        }
        b.raw().before(&GEOMETRY_RANGE_READY, "native_scene_walk_indexed", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let can_resume = rt.read_dispatch(|dispatch| dispatch.has_resume(GEOMETRY_RANGE_TAIL));
            let mut mem = rt.memory();
            let Some(stage) = walk_stage(&ReadMemory(&*mem), c.cpu.esp) else {
                return Ok(Flow::Continue);
            };
            if !can_resume {
                stage.count(Metric::WalkExamined, 1);
                stage.count(Metric::WalkRefusedResume, 1);
                return Ok(Flow::Continue);
            }
            let strip = c.cpu.ebx as u8 != 0;
            let range = IndexRange { indices: c.cpu.edi, count: c.cpu.esi, topology: if strip { IndexTopology::Strip } else { IndexTopology::List } };
            if !walk_range(&mut *mem, stage, range) {
                return Ok(Flow::Continue);
            }
            c.cpu.edx = 5 + u32::from(strip);
            Ok(Flow::Return(TransferAbi::resume(GEOMETRY_RANGE_TAIL)))
        }).context_only().optional();
        b.raw()
            .observe_before(&points::GAME_ACTIVATE, "native_transparents_generation", |_| {
                ACTIVATION.fetch_add(1, Ordering::AcqRel);
            })
            .context_only();
        b.raw()
            .observe_before(&points::SCENE_TRANSPARENTS, "native_transparents_view", |c| {
                WALK_DATA.set(None);
                let mem = c.guest();
                let host = WALK.lock().unwrap_or_else(|e| e.into_inner()).clone();
                POOLS.replace(host.and_then(|h| h.sorted_pools(&*mem)));
                let mut scope = Scope::default();
                scope.begin_view(&ReadMemory(&*mem), c.cpu.esp);
                SCOPE.with(|slot| slot.set(scope));
            })
            .context_only();
        b.raw()
            .observe_before(&SCENE_TRANSPARENTS_END, "native_transparents_end", |_| {
                WALK_DATA.set(None);
                POOLS.replace(None);
                SCOPE.with(|slot| slot.set(Scope::default()));
            })
            .context_only();
        b.raw()
            .observe_before(&points::TRANSPARENT_GEOMETRY, "native_transparents_entry", |c| {
                let mem = c.guest();
                let data = (mem.u32(c.cpu.esp).ok() == Some(SORTED_HANDLER_RETURN))
                    .then(|| sorted_data(&*mem, c.cpu.esi)).flatten();
                WALK_DATA.set(data);
                SCOPE.with(|slot| {
                    let mut scope = slot.take();
                    scope.begin_entry(&ReadMemory(&*mem), c.cpu.esp);
                    slot.set(scope);
                });
            })
            .context_only();
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use native_render::skip::CURRENT_LIST;

    #[test]
    fn transparent_lists_keep_the_partial_final_triangle_in_the_snapshot() {
        let mut range = IndexRange { indices: 0x4000, count: 2, topology: IndexTopology::List };
        assert!(!range.has_indices_for(Stage::Transparents));
        for count in [3, 4, 5, 6] {
            range.count = count;
            assert!(range.has_indices_for(Stage::Transparents));
            assert!(range.has_indices_for(Stage::FirstPerson));
        }
        range.indices = 0;
        assert!(!range.has_indices_for(Stage::Transparents));
    }

    #[test]
    fn subpart_walk_requires_the_live_sorted_layer_15_handler() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x0021_0000);
        let scene = 0x005f_1000;
        let handler = scene - 0x100;
        let stack = handler - 0x200;
        let data = 0x0060_0000 + transparent::DATA_OFFSET;
        for (addr, value) in [
            (points::GAME_GLOBALS.base, 0x005e_0000),
            (crate::native_world::ACTIVE_BSP, 0x005d_0000),
            (scene, 0x1234),
            (handler, SORTED_HANDLER_RETURN),
            (handler + 4, data),
            (data - 4, transparent::GEOMETRY_HANDLER),
            (native_render::skip::RENDER_RECORDS + 4, u32::MAX),
            (stack, 6),
            (stack + 4, 7),
            (stack + 8, 0x005a_0000),
            (stack + 12 + 0x18, GEOMETRY_EMITTER_CALLER),
            (stack + 12 + 0x68, TRANSPARENT_DRAW_CALLER),
            (stack + 12 + 0x78, 15),
        ] {
            mem.set_u32(addr, value).unwrap();
        }
        let frame = stack + 12;
        let read = ReadMemory(&mem);
        let mut scope = Scope::default();
        scope.begin_view(&read, scene);
        scope.begin_entry(&read, handler);
        WALK_DATA.set(Some(data));
        SCOPE.set(scope);
        assert_eq!(walk_stage(&read, frame), Some(Stage::Transparents));

        POOLS.replace(Some(Arc::new([0].into())));
        assert_eq!(walk_stage(&read, frame), None);
        let issue = frame - 16;
        mem.set_u32(issue, GEOMETRY_RANGE_TAIL).unwrap();
        mem.set_u32(issue + 8, 7).unwrap();
        assert_eq!(pool_issue(&ReadMemory(&mem), issue), Some((0, 7)));
        mem.set_u32(issue, 0x1234).unwrap();
        assert_eq!(pool_issue(&ReadMemory(&mem), issue), None);
        POOLS.replace(None);

        // No lighting owner for this object. The emitter still supplies the
        // exact sorted range and the device resolves its streams and shader.
        mem.set_u32(native_render::skip::RENDER_RECORDS + 4, 7).unwrap();
        let read = ReadMemory(&mem);
        let mut scope = SCOPE.take();
        scope.begin_entry(&read, handler);
        assert!(scope.indexed_entry(&read, frame).is_none());
        SCOPE.set(scope);
        assert_eq!(walk_stage(&read, frame), Some(Stage::Transparents));
        WALK_DATA.set(Some(data + ENTRY_BYTES));
        assert_eq!(walk_stage(&read, frame), None);
        WALK_DATA.set(Some(data));
        mem.set_u32(data + 4, 3).unwrap();
        assert_eq!(walk_stage(&ReadMemory(&mem), frame), None);
        mem.set_u32(data + 4, 0).unwrap();
        mem.set_u32(frame + 0x78, 7).unwrap();
        assert_eq!(walk_stage(&ReadMemory(&mem), frame), None);
        WALK_DATA.set(None);
        SCOPE.set(Scope::default());
    }

    #[test]
    fn layered_ranges_need_the_matching_handler_and_live_layer_frame() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x0021_0000);
        let stack = 0x005e_0000;
        let api = stack - 16;
        for (addr, value) in [
            (api, GEOMETRY_RANGE_TAIL),
            (stack + 0x18, GEOMETRY_EMITTER_CALLER),
            (stack + 0x68, LAYER_DRAW_CALLER),
            (stack + 0xf094, 0x0004_45b7),
        ] {
            mem.set_u32(addr, value).unwrap();
        }
        for (layer, caller, expected) in [
            (18, 0x0004_2015, true),
            (19, 0x0004_205f, true),
            (21, 0x0004_2721, true),
            (19, 0x0004_2015, false),
            (21, 0x0004_205f, false),
            (3, 0x0004_1d00, true),
            (15, 0x0004_2500, true),
            (20, 0x0004_2500, false),
            (3, 0x0004_20a0, false),
            (3, 0x0004_2760, false),
        ] {
            mem.set_u32(stack + 0x78, layer).unwrap();
            mem.set_u32(stack + LAYER_DRAW_FRAME, caller).unwrap();
            mem.set_u32(native_render::skip::CURRENT_LAYER, layer).unwrap();
            assert_eq!(layered_indexed_call(&ReadMemory(&mem), api), expected);
        }
        mem.set_u32(stack + 0x78, 18).unwrap();
        mem.set_u32(stack + LAYER_DRAW_FRAME, 0x0004_2015).unwrap();
        assert!(!layered_indexed_call(&ReadMemory(&mem), api));
        mem.set_u32(native_render::skip::CURRENT_LAYER, 18).unwrap();
        mem.set_u32(stack + 0xf094, 0).unwrap();
        assert!(!layered_indexed_call(&ReadMemory(&mem), api));
    }

    #[test]
    fn sealed_layer_candidate_starts_at_one_of_the_four_indexed_api_calls() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x0021_0000);
        let stack = 0x005e_0000;
        let api = stack - 16;
        for (addr, value) in [
            (stack + 0x18, GEOMETRY_EMITTER_CALLER),
            (stack + 0x68, LAYER_DRAW_CALLER),
            (stack + 0x78, 19),
            (stack + 0xf094, 0x0004_45b7),
            (stack + LAYER_DRAW_FRAME, 0x0004_205f),
            (native_render::skip::CURRENT_LAYER, 19),
        ] {
            mem.set_u32(addr, value).unwrap();
        }
        for ret in std::iter::once(GEOMETRY_RANGE_TAIL).chain(SUBPART_DRAWS.map(|(_, resume)| resume)) {
            mem.set_u32(api, ret).unwrap();
            assert!(layered_indexed_call(&ReadMemory(&mem), api));
        }
        mem.set_u32(api, GEOMETRY_RANGE_TAIL + 1).unwrap();
        assert!(!layered_indexed_call(&ReadMemory(&mem), api));
        mem.set_u32(api, GEOMETRY_RANGE_TAIL).unwrap();
        mem.set_u32(stack + 0x68, TRANSPARENT_DRAW_CALLER).unwrap();
        assert!(!layered_indexed_call(&ReadMemory(&mem), api));
    }

    #[test]
    fn sorted_data_uses_live_indices_and_refuses_stale_entries() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x001e_0000);
        let (entries, order) = (0x005a_1000, 0x005a_0000);
        for (at, value) in [(LIST_FIRST, 2), (LIST_COUNT, 4), (LIST_ENTRIES, entries), (LIST_ORDER, order)] {
            mem.set_u32(at, value).unwrap();
        }
        mem.set_u16(order, 3).unwrap();
        mem.set_u16(order + 2, 2).unwrap();
        assert_eq!(sorted_data(&mem, 0), Some(entries + 3 * ENTRY_BYTES + transparent::DATA_OFFSET));
        assert_eq!(sorted_data(&mem, 1), Some(entries + 2 * ENTRY_BYTES + transparent::DATA_OFFSET));
        assert_eq!(sorted_data(&mem, 2), None);
        mem.set_u16(order + 2, 1).unwrap();
        assert_eq!(sorted_data(&mem, 1), None);
    }

    #[test]
    fn subpart_calls_take_the_live_topology_count_and_index_address() {
        let mut mem = FlatMemory::new(0x1000, 0x100);
        for topology in [5, 6] {
            mem.set_u32(0x1010, topology).unwrap();
            mem.set_u32(0x1014, 9).unwrap();
            mem.set_u32(0x1018, 0x14000).unwrap();
            let range = subpart_range(&ReadMemory(&mem), 0x1010).unwrap();
            assert_eq!((range.indices, range.count), (0x14000, 9));
            assert_eq!(matches!(range.topology, IndexTopology::Strip), topology == 6);
        }
        mem.set_u32(0x1010, 8).unwrap();
        assert!(subpart_range(&ReadMemory(&mem), 0x1010).is_none());
    }

    #[test]
    fn a_sorted_late_range_requires_its_geometry_emitter_and_scene_layer_call() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x001e_0000);
        let stack = 0x005a_0000;
        let classify = |m: &FlatMemory| walk_stage(&ReadMemory(m), stack);
        for (layer, caller, stage) in [
            (6, scene_extract::late::SCENE_WATER_RETURN, Stage::Water),
            (16, scene_extract::late::SCENE_FOG_RETURN, Stage::Fog),
            (16, scene_extract::late::FIRST_PERSON_OVERLAY_RETURN, Stage::Fog),
        ] {
            for (offset, value) in [(0x18, GEOMETRY_EMITTER_CALLER), (0x68, LAYER_DRAW_CALLER),
                (0x78, layer), (0xf094, 0x0004_45b7), (LAYER_DRAW_FRAME, caller)] {
                mem.set_u32(stack + offset, value).unwrap();
            }
            mem.set_u32(native_render::skip::CURRENT_LAYER, layer).unwrap();
            assert_eq!(classify(&mem), Some(stage));
            for offset in [0x18, 0x68, 0xf094, LAYER_DRAW_FRAME] {
                let saved = mem.u32(stack + offset).unwrap();
                mem.set_u32(stack + offset, 0).unwrap();
                assert_eq!(classify(&mem), None);
                mem.set_u32(stack + offset, saved).unwrap();
            }
            mem.set_u32(native_render::skip::CURRENT_LAYER, 3).unwrap();
            assert_eq!(classify(&mem), None);
        }
    }

    #[test]
    fn residual_objects_require_the_emitters_live_model_record_and_layer() {
        use native_render::skip::{CURRENT_LAYER, RENDER_RECORDS, RENDER_RECORD_SIZE};
        let mut mem = FlatMemory::new(0x0040_0000, 0x0020_0000);
        let (sp, list) = (0x005a_0000, 0x004a_0000);
        let record = RENDER_RECORDS + RENDER_RECORD_SIZE;
        for (at, word) in [
            (sp + 0x18, GEOMETRY_EMITTER_CALLER), (sp + 0x68, LAYER_DRAW_CALLER),
            (sp + LAYER_ENTRIES_FRAME, LAYER_ENTRIES_RETURN),
            (CURRENT_LIST, list), (list + 0x14, 0x0052_0000),
            (0x004c_0b78, 2), (sp + 0x64, 1), (sp + 8, record),
            (record + 4, 0xface_0009),
        ] {
            mem.set_u32(at, word).unwrap();
        }
        let classify = |mem: &FlatMemory| walk_stage(&ReadMemory(mem), sp);
        for (layer, caller) in [(1, 0x0002_be4a), (3, 0x0002_bed5), (3, 0x0002_bd74),
            (17, 0x0002_c30b), (18, 0x0004_2015), (19, 0x0004_205f), (21, 0x0004_2721)]
        {
            mem.set_u32(CURRENT_LAYER, layer).unwrap();
            mem.set_u32(sp + 0x78, layer).unwrap();
            mem.set_u32(sp + LAYER_DRAW_FRAME, caller).unwrap();
            assert_eq!(classify(&mem), Some(Stage::Objects), "layer {layer}");
        }
        for (at, invalid) in [
            (sp + 0x18, 0), (sp + 0x68, 0), (sp + LAYER_ENTRIES_FRAME, 0), (sp + LAYER_DRAW_FRAME, 0),
            (CURRENT_LAYER, 1), (sp + 8, RENDER_RECORDS), (0x004c_0b78, 1),
            (sp + 0x64, u32::MAX), (record + 4, u32::MAX), (list + 0x14, FIRST_PERSON_ITEMS),
        ] {
            let original = mem.u32(at).unwrap();
            mem.set_u32(at, invalid).unwrap();
            assert_eq!(classify(&mem), None, "at {at:#x}");
            mem.set_u32(at, original).unwrap();
        }
        assert!(IndexRange { indices: 0x4000, count: 5, topology: IndexTopology::List }
            .has_indices_for(Stage::Objects));
    }

    #[test]
    fn an_index_range_requires_the_first_person_stack_and_owned_list() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x001e_0000);
        let (stack, list) = (0x005a_0000, 0x004a_0000);
        mem.set_u32(stack + 0x18, GEOMETRY_EMITTER_CALLER).unwrap();
        mem.set_u32(stack + 0x68, LAYER_DRAW_CALLER).unwrap();

        mem.set_u32(CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 0x14, FIRST_PERSON_ITEMS).unwrap();
        let classify = |mem: &FlatMemory| walk_stage(&ReadMemory(mem), stack);
        let args = stack - 12;
        for (layer, caller) in [(1, 0x0004_00ab), (2, 0x0004_0289), (3, 0x0004_0136),
                                (8, 0x0004_01a7), (20, 0x0004_020b), (7, 0x0004_0271)] {
            mem.set_u32(stack + 0x78, layer).unwrap();
            mem.set_u32(stack + LAYER_DRAW_FRAME, caller).unwrap();
            mem.set_u32(args, 5).unwrap();
            mem.set_u32(args + 4, 5).unwrap();
            mem.set_u32(args + 8, 0x0058_0000).unwrap();
            let stage = classify(&mem).unwrap();
            assert_eq!(stage, Stage::FirstPerson);
            assert!(subpart_range(&ReadMemory(&mem), args).unwrap().has_indices_for(stage));
        }
        mem.set_u32(list + 0x14, 0x0050_0000).unwrap();
        assert_eq!(classify(&mem), None);
        mem.set_u32(list + 0x14, FIRST_PERSON_ITEMS).unwrap();
        mem.set_u32(stack + LAYER_DRAW_FRAME, 0x0002_c039).unwrap();
        assert_eq!(classify(&mem), None);
    }
}
