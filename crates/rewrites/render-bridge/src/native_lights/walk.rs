//! The geometry-only branch of LAYER_BUILD. Its input is the selected
//! interval of the light's list. Each entry is built against live tags and
//! render records in this view, including vertices rejected by the guest
//! shader-classification check. A different callback owns its whole list.

use guest_layout::{memory::GuestMemory, tags::TAG_ENTRIES_GLOBAL};
use engine::light_passes as lp;
use native_render::{light_passes, shadows, skip};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Memory,
    Callback,
    Material,
    Geometry,
}

type Result<T> = std::result::Result<T, Refusal>;

struct Reader<'a>(&'a dyn GuestMemory);

impl Reader<'_> {
    fn word(&self, base: u32, offset: u32) -> Result<u32> {
        self.0
            .u32(base.checked_add(offset).ok_or(Refusal::Memory)?)
            .map_err(|_| Refusal::Memory)
    }

    fn half(&self, base: u32, offset: u32) -> Result<u16> {
        self.0
            .u16(base.checked_add(offset).ok_or(Refusal::Memory)?)
            .map_err(|_| Refusal::Memory)
    }

    fn byte(&self, base: u32, offset: u32) -> Result<u8> {
        self.0
            .u8(base.checked_add(offset).ok_or(Refusal::Memory)?)
            .map_err(|_| Refusal::Memory)
    }

    fn ptr(&self, base: u32, offset: u32) -> Result<u32> {
        let ptr = self.word(base, offset)?;
        if ptr == 0 {
            Err(Refusal::Memory)
        } else {
            Ok(ptr)
        }
    }

    fn tag(&self, table: u32, datum: u32, group: &[u8; 4]) -> Result<u32> {
        let slot = index(table, datum & 0xffff, 16)?;
        if datum as u16 > i16::MAX as u16
            || self.word(slot, 0)? != u32::from_be_bytes(*group)
            || self.word(slot, 4)? != datum
        {
            return Err(Refusal::Material);
        }
        self.ptr(slot, 8)
    }
}

fn index(base: u32, n: u32, stride: u32) -> Result<u32> {
    n.checked_mul(stride)
        .and_then(|off| base.checked_add(off))
        .ok_or(Refusal::Memory)
}

type Slot = [u8; 24];

fn word(s: &Slot, off: usize) -> u32 {
    u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
}

fn half(s: &Slot, off: usize) -> u16 {
    u16::from_le_bytes(s[off..off + 2].try_into().unwrap())
}

fn sort_key(s: &Slot) -> (u32, u16, u8, u16) {
    (word(s, 16), half(s, 2), s[5], half(s, 8))
}

fn sort(slots: &mut [Slot]) {
    scene_extract::layer_sort::sort(slots, &|a, b| sort_key(&a) > sort_key(&b));
}

struct Geometry {
    record: u32,
    flags: u32,
    model: u32,
    flags_and_section: u32,
}

impl Geometry {
    fn read(m: &Reader<'_>, argument: u32) -> Result<Self> {
        let id = (argument >> 8) as u16;
        if id > i16::MAX as u16 {
            return Err(Refusal::Geometry);
        }
        let record = index(
            skip::RENDER_RECORDS,
            u32::from(id),
            skip::RENDER_RECORD_SIZE,
        )?;
        Ok(Self {
            record,
            flags: m.word(record, 0)?,
            model: m.word(record, 4)?,
            flags_and_section: m.word(record, 12)?,
        })
    }

    // Section info used by the classification selector, and the address
    // of the section-data pointer (different offsets for models and BSPs).
    fn section(&self, m: &Reader<'_>, table: u32) -> Result<(u32, u32)> {
        let section = (self.flags_and_section >> 9) & 0x1ff;
        if self.model != u32::MAX {
            let model = m.tag(table, self.model, b"mode")?;
            let base = index(m.ptr(model, 0x28)?, section, 0x5c)?;
            return Ok((index(base, 4, 1)?, m.ptr(base, 0x34)?));
        }
        let bsp = m.ptr(crate::native_world::ACTIVE_BSP, 0)?;
        let info = if self.flags & (1 << 12) == 0 {
            index(m.ptr(bsp, 0xa0)?, section, 0xb0)?
        } else {
            let placement = (self.flags_and_section >> 18) & 0x7ff;
            let at = index(m.ptr(bsp, 0x144)?, placement, 0x58)?;
            let def = m.half(at, 0x34)?;
            if def > i16::MAX as u16 {
                return Err(Refusal::Geometry);
            }
            index(m.ptr(bsp, 0x13c)?, u32::from(def), 0xc8)?
        };
        Ok((info, m.ptr(info, 0x50)?))
    }
}

struct Build<'a> {
    m: Reader<'a>,
    table: u32,
    flags: u32,
    def_flags: u32,
    layer: u32,
}

impl Build<'_> {
    fn entry(&self, at: u32, n: u32, s: &mut Slot) -> Result<bool> {
        let m = &self.m;
        if m.word(at, 0)? != skip::GEOMETRY_BUILD || m.word(at, 4)? != skip::GEOMETRY_DRAW {
            return Err(Refusal::Callback);
        }
        let shader = m.word(at, 8)?;
        let argument = m.word(at, 12)?;
        let selector = u32::from(m.byte(at, 20)?);
        let variant = u32::from(m.byte(at, 21)?);
        s[..2].copy_from_slice(&(n as u16).to_le_bytes());
        s[2..4].copy_from_slice(&(shader as u16).to_le_bytes());
        let shad = m.tag(self.table, shader, b"shad")?;
        let template = m.word(m.ptr(shad, 0x24)?, 0)?;
        let stem = m.tag(self.table, template, b"stem")?;
        let layout = m.ptr(stem, 0x5c)?;
        let variant = index(m.ptr(layout, 4)?, variant, 10)?;
        let layer_number = u32::from(m.half(variant, 0)? & 0x1ff) + self.layer;
        let passes = m.half(index(m.ptr(layout, 12)?, layer_number, 2)?, 0)?;
        let geom = Geometry::read(m, argument)?;
        let marked = geom.flags & (1 << 10) != 0;
        let imp = if passes >> 9 == 0 {
            None
        } else {
            match self.layer {
                10 => shadows::generation(self.flags, geom.model != u32::MAX, marked),
                11 => shadows::application(
                    self.flags,
                    self.def_flags & 32 != 0,
                    m.half(shad, 0x3e)? != 0,
                    marked,
                ),
                12 | 13 => lp::implementation(
                    self.layer,
                    self.flags,
                    self.def_flags as u8,
                    m.half(shad, 0x3e)?,
                )
                .map(|v| v as usize),
                14 => Some(0),
                _ => return Err(Refusal::Geometry),
            }
        };
        let Some(imp) = imp else {
            s[4] = u8::MAX;
            return Ok(false);
        };
        s[4] = imp as u8;
        let kind = geom.flags_and_section >> 29;
        s[5] = if kind <= 1 { 1 } else { 2 };
        if kind != 0 {
            let key = ((geom.model & 0xffff) << 11) | ((geom.flags_and_section >> 9) & 0x1ff);
            s[12..16].copy_from_slice(&key.to_le_bytes());
        }
        let (info, data) = geom.section(m, self.table)?;
        let part = index(m.ptr(data, 4)?, argument & 0xff, 0x48)?;
        if selector >= u32::from(passes >> 9) {
            return Err(Refusal::Material);
        }
        let pass_index = u32::from(passes & 0x1ff) + selector;
        let pass = index(m.ptr(layout, 20)?, pass_index, 10)?;
        let spas = m.tag(self.table, m.word(pass, 4)?, b"spas")?;
        let block = m.ptr(spas, 0x20)?;
        let implementation = index(m.ptr(block, 4)?, imp as u32, 0x132)?;
        s[16..20].copy_from_slice(&implementation.to_le_bytes());
        let vertex = m.word(implementation, 0x100)?;
        let class = match m.half(info, 0x14)? {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => {
                let second = matches!(m.half(part, 0)?, 4 | 5);
                let which = m.byte(info, 0x10 + u32::from(second))? as i8;
                let vrtx = m.tag(self.table, vertex, b"vrtx")?;
                let candidate = (i32::from(which) + 2) as u32;
                if candidate < m.word(vrtx, 4)? {
                    candidate
                } else {
                    0
                }
            }
            _ => return Err(Refusal::Geometry),
        };
        s[8..10].copy_from_slice(&(class as u16).to_le_bytes());
        s[20..24].copy_from_slice(&m.word(geom.record, 0x10)?.to_le_bytes());
        if vertex == u32::MAX {
            return Ok(false);
        }
        let vrtx = m.tag(self.table, vertex, b"vrtx")?;
        if class >= m.word(vrtx, 4)? {
            return Ok(false);
        }
        let classification = index(m.ptr(vrtx, 8)?, class, 0x1c)?;
        Ok(m.word(classification, 12)? >> 4 != 0)
    }
}

pub(super) struct Built {
    slots: Vec<Slot>,
    count: usize,
}

impl Built {
    pub(super) fn count(&self) -> usize {
        self.count
    }

    /// The sorted entries the handlers would run, as the scene reads them.
    fn sorted(&self) -> Vec<scene_extract::light_groups::Sorted> {
        self.slots[..self.count]
            .iter()
            .map(|s| scene_extract::light_groups::Sorted {
                index: half(s, 0),
                implementation: s[4],
                vertex_class: half(s, 8),
            })
            .collect()
    }

    pub(super) fn store(&self, mem: &mut dyn GuestMemory, buffer: u32) -> bool {
        self.slots.is_empty() || mem.write(buffer, self.slots.as_flattened()).is_ok()
    }
}

pub(super) fn build(mem: &dyn GuestMemory, list: u32, layer: u32, buffer: u32) -> Result<Built> {
    build_for(mem, list, layer, Output::Guest { buffer })
}

/// Plan the same entries before the layer begins. Every field used by
/// the sort and by `Built::sorted` is assigned by a successful entry,
/// so the typed walk needs neither stack storage nor its previous bytes.
pub(super) fn sorted_for_pass(
    mem: &dyn GuestMemory,
    list: u32,
    layer: u32,
    flags: u32,
    definition_flags: u32,
) -> Result<Vec<scene_extract::light_groups::Sorted>> {
    let pass = Output::Typed {
        flags,
        definition_flags,
    };
    Ok(build_for(mem, list, layer, pass)?.sorted())
}

#[derive(Clone, Copy)]
enum Output {
    Guest { buffer: u32 },
    Typed { flags: u32, definition_flags: u32 },
}

fn build_for(mem: &dyn GuestMemory, list: u32, layer: u32, output: Output) -> Result<Built> {
    let m = Reader(mem);
    let first = m.word(list, 12)?;
    let end = m.word(list, 4)?;
    let capacity = m.word(list, 0)?;
    if !matches!(layer, 10..=14)
        || first > end
        || end > capacity
        || end > u32::from(u16::MAX)
        || end - first > 2560
    {
        return Err(Refusal::Geometry);
    }
    let mut slots = Vec::<Slot>::new();
    let mut count = 0;
    if first != end {
        let table = m.ptr(TAG_ENTRIES_GLOBAL, 0)?;
        let items = m.ptr(list, 20)?;
        let (flags, def_flags) = match output {
            Output::Typed {
                flags,
                definition_flags,
            } => (flags, definition_flags),
            Output::Guest { .. } => {
                let flags = m.word(lp::global::PASS_FLAGS, 0)?;
                // Only the receiver and diffuse choosers read this flag.
                let def_flags = if matches!(layer, 11 | 12) {
                    m.word(m.ptr(lp::global::DEFINITION, 0)?, 0)?
                } else {
                    0
                };
                (flags, def_flags)
            }
        };
        let build = Build {
            m,
            table,
            flags,
            def_flags,
            layer,
        };
        for n in first..end {
            let at = index(items, n, skip::ITEM_SIZE)?;
            if build.m.word(at, 16)? & (1 << layer) == 0 {
                continue;
            }
            if count == slots.len() {
                let mut slot = [0; 24];
                if let Output::Guest { buffer } = output {
                    mem.read(index(buffer, count as u32, 24)?, &mut slot)
                        .map_err(|_| Refusal::Memory)?;
                }
                slots.push(slot);
            }
            if build.entry(at, n, &mut slots[count])? {
                count += 1;
            }
        }
        sort(&mut slots[..count]);
    }
    Ok(Built { slots, count })
}

#[derive(Debug)]
pub struct Batch {
    pub layer: u32,
    pub view: u32,
    /// (guest index pointer, u16 index count), one complete part per draw.
    pub ranges: Vec<(u32, u32)>,
    pub counter: u32,
    pub slots: u32,
    first: u32,
    list: u32,
    primitive: u32,
}

impl Built {
    fn batch(&self, mem: &dyn GuestMemory, list: u32, layer: u32, buffer: u32) -> Option<Batch> {
        if self.count == 0 {
            return None;
        }
        let m = Reader(mem);
        let slots = &self.slots[..self.count];
        let first = &slots[0];
        let items = m.ptr(list, 20).ok()?;
        let item = |slot| index(items, u32::from(half(slot, 0)), skip::ITEM_SIZE).ok();
        let leader = item(first)?;
        let shader = m.word(leader, 8).ok()?;
        let selectors = m.half(leader, 20).ok()?;
        let arg = m.word(leader, 12).ok()?;
        let geom = Geometry::read(&m, arg).ok()?;
        // Temporary state around an individual part or visibility-selected
        // subparts prevents sharing the leader's shader and stream setup.
        if geom.flags & (1 << 11) != 0 || m.byte(geom.record, 0x1a).ok()? != 0xff {
            return None;
        }
        let table = m.ptr(TAG_ENTRIES_GLOBAL, 0).ok()?;
        let shad = m.tag(table, shader, b"shad").ok()?;
        if m.byte(shad, 0x16).ok()? & 1 != 0 {
            return None;
        }
        if slots.len() > 1 && m.word(m.ptr(shad, 0x24).ok()?, 0x3c).ok()? != 0 {
            return None; // Material animation may call an object function per draw.
        }
        let stem = m.tag(table, m.word(shad, 4).ok()?, b"stem").ok()?;
        // The handler can invoke another template on each material bind.
        if [0x44, 0x50]
            .into_iter()
            .any(|off| m.word(stem, off).ok() != Some(u32::MAX))
        {
            return None;
        }
        let (_, section) = geom.section(&m, table).ok()?;
        if geom.flags & (1 << 6) != 0
            && m.word(geom.record, 0x14).ok()? != 0
            && m.word(section, 8).ok()? != 0
        {
            return None;
        }
        let parts = m.ptr(section, 4).ok()?;
        let indices = m.ptr(section, 0x24).ok()?;
        let mut key = None;
        let mut ranges = Vec::with_capacity(slots.len());
        for s in slots {
            let at = item(s)?;
            let part_arg = m.word(at, 12).ok()?;
            if part_arg >> 8 != arg >> 8
                || m.half(at, 20).ok()? != selectors
                || m.word(at, 8).ok()? != shader
                || word(s, 16) != word(first, 16)
                || half(s, 8) != half(first, 8)
                || s[4] != first[4]
            {
                return None;
            }
            let part = index(parts, part_arg & 0xff, 0x48).ok()?;
            // These are the per-part inputs before the emitter: the section
            // classification, topology flags and shader LOD scalar. All other
            // handler inputs are the same record, shader and selector.
            let mode = (
                m.half(part, 0).ok()?,
                m.half(part, 2).ok()?,
                m.word(part, 0x2c).ok()?,
            );
            if key.replace(mode).is_some_and(|k| k != mode) {
                return None;
            }
            let start = u32::from(m.half(part, 6).ok()?);
            let count = u32::from(m.half(part, 8).ok()?);
            if count < 3 || start + count > m.word(section, 0x20).ok()? {
                return None;
            }
            ranges.push((index(indices, start, 2).ok()?, count));
        }
        let strip_topology = geom.flags & (1 << 9) != 0 && key?.1 & 8 == 0;
        Some(Batch {
            layer,
            view: m
                .word(engine::render_loop::occlusion::guest::VIEW_INDEX, 0)
                .ok()?,
            ranges,
            counter: buffer.checked_sub(4)?,
            slots: self.count as u32,
            first: buffer,
            list,
            primitive: if strip_topology { 6 } else { 5 },
        })
    }
}

use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use timeline::phase::{self, Counter};

thread_local! {
    static PENDING: RefCell<Option<Batch>> = const { RefCell::new(None) };
    // The first fallback reason wins. A layer is pending until the native
    // recorder commits. An empty layer has no draw dependency.
    static LIGHT: Cell<Option<Counter>> = const { Cell::new(None) };
    static IN_LAYER: Cell<bool> = const { Cell::new(false) };
}

// Offsets in the two verified call frames. The indexed draw is reached
// from the full-part branch of the emitter inside LIGHT_GEOMETRY_DRAW.
const FULL_PART_RETURN: u32 = 0x0004_dae0;
const GEOMETRY_EMITTER_RETURN: u32 = 0x0004_d626;
const EMITTER_FRAME: u32 = 0x28;
const HANDLER_FRAME: u32 = 0x78;
const LIGHT_TAIL_RETURN: u32 = 0x0003_1508;
const BUILDER_TO_LAYER_CALLER: u32 = 0xf02c;

const ID: &str = "native-light-walk";

/// The whole-layer route whose entry loop returns zero handlers. An
/// explicit walk opt-out also leaves the per-light body on its guest
/// layer calls, as does timing those calls.
pub fn on(switches: &hooks::Switches) -> bool {
    true
        && switches.feature(ID, true)
        && !switches.feature(crate::render_timing::ID, false)
}

pub(super) fn reset() {
    PENDING.take();
    LIGHT.set(None);
    IN_LAYER.set(false);
}

fn refused(why: Counter) {
    if matches!(
        LIGHT.get(),
        Some(Counter::LightWalkNative | Counter::LightWalkEmpty)
    ) {
        LIGHT.set(Some(why));
    }
}

pub fn committed(layer: u32, count: usize) {
    IN_LAYER.set(false);
    if LIGHT.get() == Some(Counter::LightWalkEmpty) {
        LIGHT.set(Some(Counter::LightWalkNative));
    }
    phase::count(
        Counter::LightWalkHandlersSkipped,
        count.saturating_sub(1) as u64,
    );
    if matches!(layer, 10 | 11 | 14) {
        phase::count(Counter::ShadowWalkLayers, 1);
    }
}

pub fn waiting() -> bool {
    PENDING.with_borrow(Option::is_some)
}

/// The complete-part draw inside the standard emitter, after its texture
/// flush. This excludes subpart draws, nested callbacks and earlier draws.
pub fn take_batch(mem: &dyn GuestMemory, esp: u32) -> Option<Batch> {
    let m = Reader(mem);
    PENDING.with_borrow_mut(|pending| {
        let b = pending.as_ref()?;
        if m.word(esp, 0).ok()? != FULL_PART_RETURN
            || m.word(esp, EMITTER_FRAME).ok()? != GEOMETRY_EMITTER_RETURN
            || m.word(esp, HANDLER_FRAME + 28).ok()? != b.first
            || esp.checked_add(HANDLER_FRAME + 40)? != b.counter
            || m.word(b.counter, 0).ok()? != b.slots
            || m.word(esp, 4).ok()? != b.primitive
            || (m.word(esp, 12).ok()?, m.word(esp, 8).ok()?) != b.ranges[0]
            || m.word(skip::CURRENT_LIST, 0).ok()? != b.list
            || m.word(skip::CURRENT_LAYER, 0).ok()? != b.layer
        {
            return None;
        }
        pending.take()
    })
}

fn end_layer() {
    if IN_LAYER.replace(false) {
        refused(Counter::LightWalkGpu);
    }
    PENDING.take();
}

// Whether the light's steps draw `layer` (`light_passes`, with the flags
// the pass set and its definition's): each layer is its own reservation,
// so a light's other layers neither keep nor take this one's handlers.
// Target clears and the other pass setup run either way. With the
// additional casters' pair, which draws on a suffix of the same lists
// between layers 11 and 14, every layer keeps its handlers.
fn light_layer_draws(mem: &dyn GuestMemory, layer: u32) -> Option<bool> {
    let m = Reader(mem);
    let flags = m.word(lp::global::PASS_FLAGS, 0).ok()?;
    let definition_flags = m.word(m.ptr(lp::global::DEFINITION, 0).ok()?, 0).ok()?;
    let inputs = light_passes::read(&super::Ro(mem), flags, definition_flags)?;
    if inputs.additional_casters {
        return Some(false);
    }
    Some(light_passes::steps(&inputs).drawn() & 1 << layer != 0)
}

pub(super) fn install(b: &mut Builder) {
    let lights = super::on(b.switches());
    let shadows = crate::native_shadows::on(b.switches());
    b.raw()
        .observe_before(&points::RENDER_LIGHT_PASSES, "light walk start", |_| {
            reset();
            LIGHT.set(Some(Counter::LightWalkEmpty));
        })
        .context_only();
    b.raw()
        .observe_before(&points::LIGHT_RESTORE_TARGET, "light walk result", |o| {
            let ret = engine::render::light_gather::passes::caller(&*o.guest(), o.cpu.esp);
            if LIGHT.get().is_some() && ret == Some(LIGHT_TAIL_RETURN) {
                end_layer();
                if let Some(result) = LIGHT.take() {
                    phase::count(result, 1);
                    if !matches!(result, Counter::LightWalkNative | Counter::LightWalkEmpty) {
                        phase::count(Counter::LightWalkFallback, 1);
                    }
                }
            }
        })
        .context_only();
    b.raw()
        .observe_before(&points::LAYER_END, "light walk end layer", |_| end_layer())
        .context_only();
    b.raw()
        .before(
            &points::LAYER_BUILD,
            "walk gathered light geometry",
            move |c| {
                if !matches!(c.cpu.ebx, 10..=14) {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut guest = rt.memory();
                let mem: &mut dyn GuestMemory = &mut *guest;
                let esp = c.cpu.esp;
                let args = |off| esp.checked_add(off).and_then(|a| mem.u32(a).ok());
                let layer = c.cpu.ebx;
                let list = c.cpu.edi;
                // Stack from Draw::start and Entries::start, also used by the ports.
                // The layer draw's return: the guest's site when the passes
                // body made the draw.
                let caller = esp
                    .checked_add(BUILDER_TO_LAYER_CALLER)
                    .and_then(|slot| engine::render::light_gather::passes::caller(&*mem, slot));
                let Some(caller) = caller else {
                    refused(Counter::LightWalkMemory);
                    return Ok(Flow::Continue);
                };
                // A first-person light's layer 12 or 13 the scene holds
                // whole is drawn from it alone; otherwise the guest builds
                // and runs its handlers, as without the walk.
                if matches!(layer, 12 | 13) && super::first_person_site(layer, caller) {
                    if !lights
                        || list != shadows::RECEIVERS
                        || args(4) != Some(list)
                        || args(8) != Some(c.cpu.ebp)
                        || args(12) != Some(layer)
                        || mem.u32(skip::CURRENT_LIST).ok() != Some(list)
                        || mem.u32(skip::CURRENT_LAYER).ok() != Some(layer)
                    {
                        return Ok(Flow::Continue);
                    }
                    let Some(buffer) = esp.checked_add(0x1c).filter(|&slot| args(16) == Some(slot))
                    else {
                        return Ok(Flow::Continue);
                    };
                    end_layer();
                    let Ok(built) = build(mem, list, layer, buffer) else {
                        return Ok(Flow::Continue);
                    };
                    let group = crate::scene_step::light_group(
                        mem,
                        list,
                        layer,
                        frame_scene::GroupSite::FirstPerson,
                        &built.sorted(),
                    );
                    if built.count() != 0
                        && let Some(group) = &group
                        && super::draw_alone(mem, layer, group)
                    {
                        phase::count(Counter::LightWalkBuilds, 1);
                        phase::count(Counter::LightWalkHandlersSkipped, built.count() as u64);
                        c.cpu.eax = 0;
                        return Ok(Flow::Return(guest.ret(c.cpu, 16)?));
                    }
                    return Ok(Flow::Continue);
                }
                let site = shadows::site(layer, caller);
                let shadow = site.is_some();
                if !(shadow || super::SPLIT_LAYERS.contains(&(layer, caller))) {
                    refused(Counter::LightWalkGraph);
                    return Ok(Flow::Continue);
                }
                let expected_list = if layer == 10 {
                    shadows::CASTERS
                } else {
                    shadows::RECEIVERS
                };
                if list != expected_list
                    || args(4) != Some(list)
                    || args(8) != Some(c.cpu.ebp)
                    || args(12) != Some(layer)
                    || mem.u32(skip::CURRENT_LIST).ok() != Some(list)
                    || mem.u32(skip::CURRENT_LAYER).ok() != Some(layer)
                {
                    refused(Counter::LightWalkGeometry);
                    return Ok(Flow::Continue);
                }
                let Some(buffer) = esp.checked_add(0x1c).filter(|&slot| args(16) == Some(slot))
                else {
                    refused(Counter::LightWalkMemory);
                    return Ok(Flow::Continue);
                };
                end_layer();
                if (shadow && !shadows) || (!shadow && !lights) {
                    refused(Counter::LightWalkGeometry);
                    return Ok(Flow::Continue);
                }
                let whole = match site {
                    Some(shadows::Site::Object) => true,
                    Some(shadows::Site::AdditionalCaster) => false,
                    _ => {
                        matches!(
                            LIGHT.get(),
                            Some(Counter::LightWalkEmpty | Counter::LightWalkNative)
                        ) && light_layer_draws(mem, layer) == Some(true)
                    }
                };
                let built = match build(mem, list, layer, buffer) {
                    Ok(built) => built,
                    Err(why) => {
                        refused(match why {
                            Refusal::Memory => Counter::LightWalkMemory,
                            Refusal::Callback => Counter::LightWalkCallback,
                            Refusal::Material => Counter::LightWalkMaterial,
                            Refusal::Geometry => Counter::LightWalkGeometry,
                        });
                        return Ok(Flow::Continue);
                    }
                };
                // The group into the scene, with what the first handler supplies
                // (framescene.md, slice 4).
                let group_site = match site {
                    None | Some(shadows::Site::Light) => frame_scene::GroupSite::Light,
                    Some(shadows::Site::AdditionalCaster) => {
                        frame_scene::GroupSite::AdditionalCasters
                    }
                    Some(shadows::Site::Object) => frame_scene::GroupSite::ObjectShadow,
                };
                let group =
                    crate::scene_step::light_group(mem, list, layer, group_site, &built.sorted());
                // A shadow caster layer the scene holds whole is drawn from it
                // alone, including the additional casters' suffix. The guest's
                // begin, clear and end still run; only its entry loop is emptied.
                if built.count() != 0
                    && site.is_some()
                    && (whole || site == Some(shadows::Site::AdditionalCaster))
                    && let Some(group) = &group
                    && crate::native_shadows::draw_alone(mem, layer, group)
                {
                    phase::count(Counter::LightWalkBuilds, 1);
                    phase::count(Counter::LightWalkHandlersSkipped, built.count() as u64);
                    if LIGHT.get() == Some(Counter::LightWalkEmpty) {
                        LIGHT.set(Some(Counter::LightWalkNative));
                    }
                    c.cpu.eax = 0;
                    return Ok(Flow::Return(guest.ret(c.cpu, 16)?));
                }
                // A light's layer 12 or 13 the scene holds whole is drawn from it
                // alone: the entry loop gets no entries, so no handler runs. The
                // receiver layers 11 and 14 keep their resolved-draw path.
                if whole
                    && site.is_none()
                    && built.count() != 0
                    && let Some(group) = &group
                    && super::draw_alone(mem, layer, group)
                {
                    phase::count(Counter::LightWalkBuilds, 1);
                    phase::count(Counter::LightWalkHandlersSkipped, built.count() as u64);
                    if LIGHT.get() == Some(Counter::LightWalkEmpty) {
                        LIGHT.set(Some(Counter::LightWalkNative));
                    }
                    c.cpu.eax = 0;
                    return Ok(Flow::Return(guest.ret(c.cpu, 16)?));
                }
                // Building an equivalent private array is independent of owning
                // any draw: a layer that does not batch keeps its handlers.
                if built.count() != 0 && !whole {
                    refused(Counter::LightWalkGraph);
                }
                let batch = whole
                    .then(|| built.batch(mem, list, layer, buffer))
                    .flatten();
                if whole && built.count() != 0 && batch.is_none() {
                    refused(Counter::LightWalkGeometry);
                }
                if !built.store(mem, buffer) {
                    refused(Counter::LightWalkMemory);
                    return Ok(Flow::Continue);
                }
                IN_LAYER.set(batch.is_some());
                PENDING.set(batch);
                phase::count(Counter::LightWalkBuilds, 1);
                c.cpu.eax = built.count() as u32;
                Ok(Flow::Return(guest.ret(c.cpu, 16)?))
            },
        )
        .after_feature(crate::native_world::ID)
        .context_only();
}

pub struct NativeLightWalk;

impl Feature for NativeLightWalk {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Walk gathered light and caster geometry")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> std::result::Result<(), BuildError> {
        if on(b.switches()) {
            install(b);
            b.snapshot(hooks::SnapshotPolicy::section(Vec::new, |_, _| {
                reset();
                Ok(())
            }));
        } else {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;

    const ITEMS: u32 = 0x8000;
    const BUFFER: u32 = 0x18000;
    const PARTS: u32 = 0x15000;
    const SHADER: u32 = 0xb000;
    const INDICES: u32 = 0x16000;
    const RECORD: u32 = skip::RENDER_RECORDS + skip::RENDER_RECORD_SIZE;
    const PROGRAM: u32 = 0x17000;

    fn fixture(layer: u32) -> FlatMemory {
        let mut m = FlatMemory::new(0, 0x520000);
        let list = if layer == 10 {
            shadows::CASTERS
        } else {
            shadows::RECEIVERS
        };
        let (table, template, pass, vertex) = (0x9000, 0xc000, 0xd000, 0xe000);
        let (post, layout, variant, layers, passes) = (0xa000, 0xa100, 0xa200, 0xa300, 0xa400);
        for (id, group, payload) in [
            (1, b"shad", SHADER),
            (2, b"stem", template),
            (3, b"spas", pass),
            (4, b"vrtx", vertex),
        ] {
            let tag = table + id * 16;
            m.set_u32(tag, u32::from_be_bytes(*group)).unwrap();
            m.set_u32(tag + 4, id).unwrap();
            m.set_u32(tag + 8, payload).unwrap();
        }
        for (at, value) in [
            (TAG_ENTRIES_GLOBAL, table),
            (list, 5),
            (list + 4, 4),
            (list + 8, 1 << layer),
            (list + 12, 1),
            (list + 20, ITEMS),
            (lp::global::DEFINITION, 0x8600),
            (lp::global::PASS_FLAGS, if layer == 10 { 4 } else { 1 }),
            (SHADER + 4, 2),
            (SHADER + 0x24, post),
            (post, 2),
            (template + 0x44, u32::MAX),
            (template + 0x50, u32::MAX),
            (template + 0x5c, layout),
            (layout + 4, variant),
            (layout + 12, layers),
            (layout + 20, passes),
            (passes + 4, 3),
            (pass + 0x20, 0xd100),
            (0xd104, 0xd200),
            (vertex + 4, 1),
            (vertex + 8, PROGRAM),
            (PROGRAM + 12, 1 << 4),
            (RECORD + 4, u32::MAX),
            (crate::native_world::ACTIVE_BSP, 0x11000),
            (0x110a0, 0x12000),
            (0x12050, 0x14000),
            (0x14004, PARTS),
            (0x14020, 256),
            (0x14024, INDICES),
        ] {
            m.set_u32(at, value).unwrap();
        }
        m.set_u8(RECORD + 0x1a, 0xff).unwrap();
        m.set_u16(layers + layer * 2, 1 << 9).unwrap();
        for imp in 0..=4 {
            m.set_u32(0xd200 + imp * 0x132 + 0x100, 4).unwrap();
        }
        for part in 0..3 {
            let item = ITEMS + 24 * (part + 1);
            for (off, value) in [
                (0, skip::GEOMETRY_BUILD),
                (4, skip::GEOMETRY_DRAW),
                (8, 1),
                (12, 256 + part),
                (16, 1 << layer),
                (20, 0),
            ] {
                m.set_u32(item + off, value).unwrap();
            }
            m.set_u16(PARTS + 0x48 * part + 6, (part * 8) as u16)
                .unwrap();
            m.set_u16(PARTS + 0x48 * part + 8, 6).unwrap();
        }
        m.write(BUFFER, &[0x55; 72]).unwrap();
        m
    }

    #[test]
    fn the_whole_layer_route_requires_the_walk_untimed() {
        use hooks::Switches;
        assert!(on(&Switches::default()));
        assert!(!on(&Switches::default().with(ID, false)));
        assert!(!on(
            &Switches::default().with(crate::render_timing::ID, true)
        ));
    }

    #[test]
    fn gathered_interval_keeps_the_guest_tie_order_and_unwritten_slot_bytes() {
        let mut m = fixture(12);
        let b = build(&m, shadows::RECEIVERS, 12, BUFFER).unwrap();
        assert_eq!(b.count(), 3);
        let group = b.batch(&m, shadows::RECEIVERS, 12, BUFFER).unwrap();
        assert_eq!(
            group.ranges,
            [(INDICES + 16, 6), (INDICES + 32, 6), (INDICES, 6)]
        );
        assert_eq!(group.primitive, 5);
        assert!(b.store(&mut m, BUFFER));
        for (n, id) in [2, 3, 1].into_iter().enumerate() {
            let at = BUFFER + n as u32 * 24;
            assert_eq!(m.u16(at).unwrap(), id);
            assert_eq!(m.u8(at + 4).unwrap(), 1);
            assert_eq!(m.u32(at + 12).unwrap(), 0x5555_5555);
        }
    }

    #[test]
    fn typed_groups_use_this_calls_flags_and_all_its_sorted_entries() {
        use crate::native_lights::typed_groups;
        use frame_scene::{GroupSite, LightStep, LightTarget};

        let mut m = fixture(12);
        let list = shadows::RECEIVERS;
        let steps = [LightStep {
            layer: 12,
            target: LightTarget::View,
            stencil: Some(0),
            draws: true,
        }];
        let collect = |m: &FlatMemory, site| typed_groups::plan(m, site, &steps, 0, 32, None);
        // The preceding light left a suffix and a different flag word.
        assert!(collect(&m, GroupSite::Light).is_none());
        assert_eq!(
            typed_groups::plan_or_refusal(&m, GroupSite::Light, &steps, 0, 32, None).err(),
            Some(typed_groups::GroupRefusal::Suffix),
            "named"
        );
        m.set_u32(list + 12, 0).unwrap();
        m.set_u32(lp::global::DEFINITION, u32::MAX).unwrap();
        for site in [GroupSite::Light, GroupSite::FirstPerson] {
            let groups = collect(&m, site).unwrap();
            assert_eq!(groups.len(), 1);
            let g = &groups[0];
            assert_eq!((g.group.site, g.group.entries.clone()), (site, 0..3));
            assert_eq!(
                g.entries.iter().map(|e| e.slot).collect::<Vec<_>>(),
                [2, 3, 1]
            );
            assert!(g.entries.iter().all(|e| e.binding.implementation == 4));
            assert!(g.rows.iter().all(Vec::is_empty));
        }
        // The last entry has a callback the typed group cannot express.
        m.set_u32(ITEMS + 3 * 24, 0x8400).unwrap();
        assert!(collect(&m, GroupSite::Light).is_none());
        // The layer's sort reads each item's build and draw first, so the
        // callback is refused there, before any entry is read.
        let refusal = typed_groups::plan_or_refusal(&m, GroupSite::Light, &steps, 0, 32, None).err();
        assert_eq!(refusal, Some(typed_groups::GroupRefusal::Sort));
        assert_eq!(refusal.unwrap().name(), "sort");
        assert_eq!(m.u32(lp::global::DEFINITION).unwrap(), u32::MAX);
        assert_eq!(m.bytes(BUFFER, 72).unwrap(), [0x55; 72]);
    }

    #[test]
    fn a_faded_entry_draws_only_while_the_host_holds_the_stipple_at_rest() {
        use crate::native_lights::typed_groups::{self, GroupRefusal};
        use frame_scene::{EntryState, GroupSite, LightStep, LightTarget};
        use scene_extract::light_groups::Cause;

        let mut m = fixture(12);
        m.set_u32(shadows::RECEIVERS + 12, 0).unwrap();
        m.set_u32(lp::global::DEFINITION, u32::MAX).unwrap();
        m.set_u8(RECORD + 0x1a, 0x80).unwrap();
        let steps = [LightStep { layer: 12, target: LightTarget::View, stencil: Some(0), draws: true }];
        // With no host nothing is held: the light keeps its guest passes.
        assert_eq!(
            typed_groups::plan_or_refusal(&m, GroupSite::Light, &steps, 0, 32, None).err(),
            Some(GroupRefusal::Entry(Cause::State)),
        );
        let mut asked = 0;
        let mut held = |state: EntryState| {
            let mut ask = || {
                asked += 1;
                state
            };
            typed_groups::plan_held(&m, GroupSite::Light, &steps, 0, 32, None, &mut ask)
        };
        let all = EntryState { aux_depth: true, fade: true, ..EntryState::default() };
        let groups = held(all).unwrap();
        assert!(groups[0].entries.iter().all(|e| e.state == EntryState { aux_depth: false, fade: true, ..EntryState::default() }));
        assert_eq!(held(EntryState { aux_depth: true, fade: false, ..EntryState::default() }).err(), Some(GroupRefusal::Entry(Cause::State)));
        assert_eq!(asked, 4, "asked for each entry with state, up to the first refused");
        // Every light and shadow layer takes it while the host holds it.
        let fade = EntryState { aux_depth: false, fade: true, ..EntryState::default() };
        for layer in [10, 11, 14] {
            assert!(typed_groups::state_drawn(layer, fade, &mut || all), "layer {layer}");
            assert!(!typed_groups::state_drawn(layer, fade, &mut EntryState::default), "layer {layer}: not held");
        }
        assert!(typed_groups::state_drawn(13, all, &mut || all));
        assert!(typed_groups::state_drawn(14, EntryState::default(), &mut || unreachable!("a plain entry asks nothing")));
    }

    #[test]
    fn a_flagged_entry_draws_in_every_layer_and_asks_the_host_for_the_structure_upload_only_when_it_has_one() {
        use crate::native_lights::typed_groups;
        use frame_scene::EntryState;

        let flagged = EntryState { shader_flag: true, ..EntryState::default() };
        let uploading = EntryState { structure: true, ..flagged };
        let nothing_held = EntryState::default();
        let upload_held = EntryState { structure: true, ..nothing_held };
        assert!(typed_groups::state_drawn(12, flagged, &mut || nothing_held), "no block, nothing to hold");
        assert!(!typed_groups::state_drawn(13, uploading, &mut || nothing_held), "a block the device does not hold");
        assert!(typed_groups::state_drawn(13, uploading, &mut || upload_held));
        for layer in [10, 11, 14] {
            assert!(typed_groups::state_drawn(layer, uploading, &mut || upload_held), "layer {layer}");
            assert!(!typed_groups::state_drawn(layer, uploading, &mut || nothing_held), "layer {layer}: not held");
        }
    }

    #[test]
    fn typed_casters_predict_the_draw_target_and_restore_before_receivers() {
        use crate::native_lights::typed_groups;
        use engine::shadow_constants::global::{TARGET_STRIDE, TARGETS};
        use frame_scene::{GroupSite, LightStep, LightTarget};

        let mut m = fixture(10);
        m.set_u32(shadows::CASTERS + 12, 0).unwrap();
        m.set_u32(shadows::RECEIVERS + 8, 1 << 11).unwrap();
        // A receiver with an empty sorted group still has begin state.
        for (at, value) in [
            (0x0048_58b8, 2),
            (0x0048_58b4, u32::MAX), // preceding pass's target
            (0x004b_8464, 9),        // preceding pass's raster
            (0x004b_846c, 17),       // inherited compare in 10
            (0x0045_e46c, 7),
            (0x0045_dc0c, 11),
            (0x0004_96f1, 19), // receiver compare operand
            (0x0004_9705, 31), // receiver clip operand
        ] {
            m.set_u32(at, value).unwrap();
        }
        m.set_u8(0x0047_fe84, 1).unwrap();
        for target in [2, 9, 10] {
            m.set_u32(TARGETS + target * TARGET_STRIDE + 0x80, 10 + target)
                .unwrap();
            m.set_u32(TARGETS + target * TARGET_STRIDE + 0x84, 20 + target)
                .unwrap();
        }
        let steps = [
            LightStep {
                layer: 10,
                target: LightTarget::ShadowMap,
                stencil: None,
                draws: true,
            },
            LightStep {
                layer: 11,
                target: LightTarget::View,
                stencil: Some(0),
                draws: true,
            },
        ];
        let projection = Some([[13; 4]; frame_scene::PROJECTION_ROWS]);
        for (site, flags, target) in [
            (GroupSite::ObjectShadow, 0x14, 10),
            (GroupSite::Light, 0x4, 9),
            (GroupSite::AdditionalCasters, 0x4, 9),
        ] {
            let groups = typed_groups::plan(&m, site, &steps, flags, 0, projection).unwrap();
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].group.projection, projection);
            let casting = groups[0].group.pass.unwrap();
            assert_eq!(casting.flags, flags);
            assert_eq!(
                (casting.target, casting.size),
                (target, [10 + target, 20 + target])
            );
            assert_eq!(
                (
                    casting.offset_enable,
                    casting.raster,
                    casting.shadow_function
                ),
                (1, 2, 17)
            );
            let apply = groups[1].group.pass.unwrap();
            assert_eq!((apply.target, apply.size), (2, [12, 22]));
            assert_eq!(
                (apply.offset_enable, apply.raster, apply.shadow_function),
                (0, 0, 19)
            );
            assert_eq!(
                (
                    apply.offset_constant,
                    apply.offset_slope,
                    apply.clip_control
                ),
                (7, 11, 31)
            );
            assert_eq!(m.u32(0x0048_58b4).unwrap(), u32::MAX);
        }
        // A caller naming a draw which the live list did not select.
        m.set_u32(shadows::RECEIVERS + 8, 0).unwrap();
        assert!(typed_groups::plan(&m, GroupSite::Light, &steps, 4, 0, projection).is_none());
        assert_eq!(
            typed_groups::plan_or_refusal(&m, GroupSite::Light, &steps, 4, 0, projection).err(),
            Some(typed_groups::GroupRefusal::Mask)
        );
    }

    /// The cut frame's groups (`scene_frame::groups`, made before any
    /// layer begins) against the layer builds the guest's passes make at
    /// each layer draw, from the same item lists: the same entries in the
    /// same sorted order, for the diffuse and specular layers and for the
    /// casters with the shadow setup's resize laid over memory.
    #[test]
    fn the_cut_lights_groups_equal_the_guests_layer_builds() {
        use crate::native_lights::{Ro, scene_frame};
        use engine::shadow_constants::{self as sc, global::{TARGET_STRIDE, TARGETS}};
        use native_render::light_passes::{self as passes, Inputs};

        let guest = |m: &FlatMemory, list: u32, layer: u32| {
            let built = build(m, list, layer, BUFFER).unwrap();
            let items = m.u32(list + 20).unwrap();
            built
                .sorted()
                .into_iter()
                .map(|s| scene_extract::light_groups::entry(&Ro(m), items, s).unwrap())
                .unzip::<_, _, Vec<_>, Vec<_>>()
        };
        for (layer, flags) in [(12, 1), (13, 3)] {
            let mut m = fixture(layer);
            m.set_u32(shadows::RECEIVERS + 12, 0).unwrap();
            m.set_u32(lp::global::PASS_FLAGS, flags).unwrap();
            let definition_flags = m.u32(m.u32(lp::global::DEFINITION).unwrap()).unwrap();
            let (entries, rows) = guest(&m, shadows::RECEIVERS, layer);
            let steps = passes::steps(&Inputs {
                flags,
                definition_flags,
                receivers: 1 << layer,
                ..Inputs::default()
            });
            let groups = scene_frame::groups(&m, &steps, flags, definition_flags, None).unwrap();
            assert_eq!(groups.len(), 1, "layer {layer}: one group per drawn layer");
            let g = &groups[0];
            assert_eq!(u32::from(g.group.layer), layer);
            assert_eq!((&g.entries, &g.rows), (&entries, &rows), "layer {layer}");
            if layer == 12 {
                assert_eq!(g.entries.len(), 3);
            }
        }

        // The casters: the setup's block is the projection, and its resize
        // of target 9 is the pass's size though memory still holds zero.
        let mut m = fixture(10);
        m.set_u32(shadows::CASTERS + 12, 0).unwrap();
        let (entries, _) = guest(&m, shadows::CASTERS, 10);
        let steps = passes::steps(&Inputs {
            flags: 4,
            casters: 1 << 10,
            ..Inputs::default()
        });
        let shadow = sc::ShadowGlobals {
            block: core::array::from_fn(|n| n as u32 + 1),
            mode: 1,
            size: [256, 128],
            target: Some([256, 128, 0x0080_0100]),
        };
        assert_eq!(m.u32(TARGETS + 9 * TARGET_STRIDE + 0x80).unwrap(), 0);
        let groups = scene_frame::groups(&m, &steps, 4, 0, Some(&shadow)).unwrap();
        assert_eq!(groups.len(), 1, "layers 11, 14 and 12 do not draw");
        let g = &groups[0];
        assert_eq!((g.group.layer, &g.entries), (10, &entries));
        assert_eq!(g.group.projection.unwrap()[0], [1, 2, 3, 4]);
        assert_eq!(g.group.projection.unwrap()[8], [33, 34, 35, 36]);
        let pass = g.group.pass.unwrap();
        assert_eq!((pass.target, pass.size), (9, [256, 128]));
        assert_eq!(m.u32(TARGETS + 9 * TARGET_STRIDE + 0x80).unwrap(), 0, "nothing written");
    }

    /// The cut caster's object-shadow groups (`object_shadow_scene::groups`,
    /// made after its item lists, before layer 10 begins) against the
    /// layer build the guest's mask branch makes at its layer 10 draw: the
    /// same entries in the same order, the mask's target 10 and the
    /// receivers on the view's target, each group once.
    #[test]
    fn the_cut_casters_object_shadow_groups_equal_the_guests_layer_build() {
        use crate::native_lights::{Ro, object_shadow_scene};
        use engine::shadow_constants::global::{TARGET_STRIDE, TARGETS};
        use frame_scene::GroupSite;

        let mut m = fixture(10);
        m.set_u32(shadows::CASTERS + 12, 0).unwrap();
        m.set_u32(lp::global::PASS_FLAGS, 0x14).unwrap();
        m.set_u32(0x0048_58b8, 2).unwrap();
        for target in [2, 10] {
            m.set_u32(TARGETS + target * TARGET_STRIDE + 0x80, 256).unwrap();
            m.set_u32(TARGETS + target * TARGET_STRIDE + 0x84, 128).unwrap();
        }
        // The receivers do not hold layer 11: the branch draws nothing.
        assert_eq!(object_shadow_scene::groups(&m), Some(Vec::new()));

        m.set_u32(shadows::RECEIVERS + 8, 1 << 11).unwrap();
        let built = build(&m, shadows::CASTERS, 10, BUFFER).unwrap();
        let items = m.u32(shadows::CASTERS + 20).unwrap();
        let entries: Vec<_> = built
            .sorted()
            .into_iter()
            .map(|s| scene_extract::light_groups::entry(&Ro(&m), items, s).unwrap().0)
            .collect();
        let groups = object_shadow_scene::groups(&m).unwrap();
        assert_eq!(groups.iter().map(|g| g.group.layer).collect::<Vec<_>>(), [10, 11]);
        assert!(groups.iter().all(|g| g.group.site == GroupSite::ObjectShadow));
        assert_eq!(groups[0].entries, entries);
        assert_eq!(entries.len(), 3);
        assert!(groups[1].entries.is_empty(), "the fixture's receivers are empty");
        let (mask, apply) = (groups[0].group.pass.unwrap(), groups[1].group.pass.unwrap());
        assert_eq!((mask.target, mask.flags, mask.size), (10, 0x14, [256, 128]));
        assert_eq!(apply.target, 2);
        assert!(groups[0].group.projection.is_some());
    }

    #[test]
    fn long_partition_preserves_the_original_permutation_of_equal_keys() {
        let mut slots = vec![[0; 24]; 9];
        for (i, s) in slots.iter_mut().enumerate() {
            s[0] = i as u8;
        }
        sort(&mut slots);
        assert_eq!(
            slots.iter().map(|s| s[0]).collect::<Vec<_>>(),
            [4, 1, 2, 3, 0, 5, 6, 7, 8]
        );
        for (i, s) in slots.iter_mut().enumerate() {
            s[8] = (i % 3) as u8;
        }
        sort(&mut slots);
        assert!(slots.is_sorted_by_key(sort_key));
    }

    #[test]
    fn unknown_callback_in_the_last_slot_never_publishes_a_prefix() {
        let mut m = fixture(12);
        m.set_u32(ITEMS + 3 * 24, 0x8484).unwrap();
        assert!(matches!(
            build(&m, shadows::RECEIVERS, 12, BUFFER),
            Err(Refusal::Callback)
        ));
        assert_eq!(m.bytes(BUFFER, 72).unwrap(), [0x55; 72]);
    }

    #[test]
    fn pass_filtering_and_classification_apply_before_sorting_or_ownership() {
        let mut m = fixture(10);
        let b = build(&m, shadows::CASTERS, 10, BUFFER).unwrap();
        assert_eq!(b.slots[0][4], 2);
        assert!(b.batch(&m, shadows::CASTERS, 10, BUFFER).is_some());
        m.set_u32(RECORD, 1 << 10).unwrap();
        assert_eq!(build(&m, shadows::CASTERS, 10, BUFFER).unwrap().count(), 0);
        m.set_u32(RECORD, 0).unwrap();
        m.set_u32(PROGRAM + 12, 0).unwrap();
        assert_eq!(build(&m, shadows::CASTERS, 10, BUFFER).unwrap().count(), 0);
    }

    #[test]
    fn state_or_topology_change_keeps_every_handler() {
        let mut m = fixture(12);
        for (at, value) in [
            (PARTS + 0x48 + 0x2c, 1),
            (PARTS + 0x48, 8 << 16),
            (0xa000 + 0x3c, 1),
            (RECORD, 1 << 11),
        ] {
            let old = m.u32(at).unwrap();
            m.set_u32(at, value).unwrap();
            let b = build(&m, shadows::RECEIVERS, 12, BUFFER).unwrap();
            assert!(b.batch(&m, shadows::RECEIVERS, 12, BUFFER).is_none());
            m.set_u32(at, old).unwrap();
        }
    }

    #[test]
    fn gathers_a_model_section_with_its_pose_and_vertex_class() {
        let mut m = fixture(12);
        let model = 0x2010_0005;
        let model_tag = 0x9000 + 16 * 5;
        let section = 0x20000 + 2 * 0x5c;
        for (at, v) in [
            (RECORD + 4, model),
            (RECORD + 12, (2 << 29) | (2 << 9)),
            (RECORD + 16, 0x21900),
            (model_tag, u32::from_be_bytes(*b"mode")),
            (model_tag + 4, model),
            (model_tag + 8, 0x1f000),
            (0x1f028, 0x20000),
            (section + 0x34, 0x14000),
            (0xe004, 2),
            (PROGRAM + 0x1c + 12, 16),
        ] {
            m.set_u32(at, v).unwrap();
        }
        m.set_u16(section + 4 + 0x14, 1).unwrap();
        let built = build(&m, shadows::RECEIVERS, 12, BUFFER).unwrap();
        assert_eq!(built.count(), 3);
        assert_eq!(half(&built.slots[0], 8), 1);
        assert_eq!(word(&built.slots[0], 12), 5 << 11 | 2);
        assert_eq!(word(&built.slots[0], 20), 0x21900);
        assert!(built.batch(&m, shadows::RECEIVERS, 12, BUFFER).is_some());
        m.set_u32(model_tag + 4, model + (1 << 16)).unwrap();
        assert!(matches!(
            build(&m, shadows::RECEIVERS, 12, BUFFER),
            Err(Refusal::Material)
        ));
    }

    #[test]
    fn placement_uses_its_definition_and_refuses_visibility_selected_subparts() {
        let mut m = fixture(10);
        let placement = 0x20000 + 3 * 0x58;
        let definition = 0x21000 + 2 * 0xc8;
        for (at, v) in [
            (RECORD, (1 << 12) | (1 << 6)),
            (RECORD + 12, 3 << 18),
            (0x11144, 0x20000),
            (0x1113c, 0x21000),
            (definition + 0x50, 0x14000),
            (RECORD + 0x14, 0x22000),
        ] {
            m.set_u32(at, v).unwrap();
        }
        m.set_u16(placement + 0x34, 2).unwrap();
        let b = build(&m, shadows::CASTERS, 10, BUFFER).unwrap();
        assert_eq!(b.count(), 3);
        assert!(b.batch(&m, shadows::CASTERS, 10, BUFFER).is_some());
        m.set_u32(0x14008, 3).unwrap();
        assert!(b.batch(&m, shadows::CASTERS, 10, BUFFER).is_none());
    }

    #[test]
    fn each_layer_a_light_draws_batches_alone_unless_additional_casters_run() {
        let mut m = fixture(12);
        assert_eq!(light_layer_draws(&m, 12), Some(true));
        // Diffuse and specular, both: each layer is the light's to batch.
        m.set_u32(lp::global::PASS_FLAGS, 3).unwrap();
        m.set_u32(shadows::RECEIVERS + 8, 3 << 12).unwrap();
        m.set_u32(ITEMS + 24 + 16, 3 << 12).unwrap();
        assert_eq!(light_layer_draws(&m, 12), Some(true));
        assert_eq!(light_layer_draws(&m, 13), Some(true));
        // Specular without context zero: layer 13 is not reached.
        m.set_u32(lp::global::PASS_FLAGS, 2).unwrap();
        assert_eq!(light_layer_draws(&m, 13), Some(false));
        // Shadow, stencil and layer 14 work leave layer 12 its own.
        m.set_u32(lp::global::PASS_FLAGS, 4 | 3).unwrap();
        m.set_u32(shadows::RECEIVERS + 8, 0xf << 11).unwrap();
        m.set_u32(shadows::CASTERS + 8, 1 << 10).unwrap();
        for layer in [10, 11, 12, 13, 14] {
            assert_eq!(light_layer_draws(&m, layer), Some(true), "layer {layer}");
        }
        m.set_u32(light_passes::OVERRIDE, 0x8280).unwrap();
        m.set_u8(0x8285, 1).unwrap();
        m.set_u32(light_passes::ADDITIONAL_COUNT, 2).unwrap();
        for layer in [10, 11, 12, 13, 14] {
            assert_eq!(light_layer_draws(&m, layer), Some(false), "layer {layer}");
        }
    }

    #[test]
    fn admission_is_one_shot_and_bound_to_the_exact_indexed_call() {
        let mut m = fixture(12);
        let list = shadows::RECEIVERS;
        let b = build(&m, list, 12, BUFFER)
            .unwrap()
            .batch(&m, list, 12, BUFFER)
            .unwrap();
        let sp = b.counter - 0x78 - 40;
        for (at, v) in [
            (sp, 0x0004_dae0),
            (sp + 4, b.primitive),
            (sp + 8, b.ranges[0].1),
            (sp + 12, b.ranges[0].0),
            (sp + 0x28, 0x0004_d626),
            (sp + 0x78 + 28, BUFFER),
            (b.counter, b.slots),
            (skip::CURRENT_LIST, list),
            (skip::CURRENT_LAYER, 12),
        ] {
            m.set_u32(at, v).unwrap();
        }
        PENDING.set(Some(b));
        assert!(take_batch(&m, sp + 4).is_none());
        assert!(take_batch(&m, sp).is_some());
        assert!(take_batch(&m, sp).is_none());
    }
}
