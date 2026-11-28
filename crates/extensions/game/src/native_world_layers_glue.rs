//! The layer hook's context at the guest's indexed draw call: the layer,
//! geometry and light selection the lights glue reads (`indexed_draw`).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

use render_bridge::native_world_layers::{Layer, current};
use render_bridge::native_sky::{SkyItem, Refusal};
use native_render::camera::GuestRead;
use native_render::world_layers::{DRAW_INDEXED_VERTICES, Geometry, Owner, SKY_INDEXED_RETURN, Site};

struct Memory<'a>(&'a runtime::Guest);

impl GuestRead for Memory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(Clone, Copy)]
pub struct DrawContext {
    pub layer: Layer,
    pub frame: u64,
    pub geometry: Option<Geometry>,
    /// The indexed light handler's (pass selector, vertex class).
    pub light_selection: Option<(u8, u16)>,
    bsp: Option<u32>,
    pub(crate) generation: u64,
    pub index_count: Option<u32>,
    /// The call's index pointer, as the guest passed it.
    pub index_address: Option<u32>,
}

thread_local! {
    static POOL: Cell<Option<(u32, u32)>> = const { Cell::new(None) };
    static SKY: RefCell<Option<SkyIssue>> = const { RefCell::new(None) };
    static DRAW: Cell<Option<DrawContext>> = const { Cell::new(None) };
}

/// Count a section once per front-end path and view of its BSP. A
/// section with a mixture of early and late parts appears in both.
#[derive(Default)]
struct SectionCounts {
    frame: u64,
    seen: HashSet<(timeline::native::WorldOpaque, u32, u32, native_render::world::Owner)>,
}

impl SectionCounts {
    fn add(&mut self, frame: u64, view: u32, bsp: u32, path: timeline::native::WorldOpaque,
        owners: impl IntoIterator<Item = native_render::world::Owner>) -> (u64, u64) {
        if frame != self.frame {
            self.seen.clear();
            self.frame = frame;
        }
        let mut draws = 0;
        let mut sections = 0;
        for owner in owners {
            draws += 1;
            sections += u64::from(self.seen.insert((path, view, bsp, owner)));
        }
        (draws, sections)
    }
}

pub fn count_frontend(frame: u64, view: u32, bsp: u32, path: timeline::native::WorldOpaque,
    owners: impl IntoIterator<Item = native_render::world::Owner>) {
    thread_local! { static SECTIONS: RefCell<SectionCounts> = RefCell::default(); }
    let (draws, sections) = SECTIONS.with_borrow_mut(|s| s.add(frame, view, bsp, path, owners));
    path.count(draws, sections);
}

/// Clears the context even when the indexed handler returns a fault or calls
/// out to the guest before its draw. Such a call re-enters this wrapper.
struct Restore(Option<DrawContext>);

impl Drop for Restore {
    fn drop(&mut self) {
        DRAW.set(self.0);
    }
}

pub fn install(rt: &runtime::Runtime) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_world_layers::on(&switches) {
        return;
    }
    rt.edit_dispatch(|dispatch| {
        let index = DRAW_INDEXED_VERTICES;
        let Some(previous) = dispatch.override_handler(index) else {
            return;
        };
        let context_only = dispatch.is_context_only(index);
        dispatch.register_override(
            index,
            "boot:native world layers indexed",
            move |cpu, ctx, selector| {
                let mut pool = None;
                let mut sky = None;
                let context = (|| {
                    let frame = crate::presents::presents() + 1;
                    let current = current();
                    let guest = ctx.memory();
                    let mem = Memory(&guest);
                    pool = render_bridge::native_transparents::pool_issue(&mem, cpu.esp);
                    let return_address = mem.read_u32(cpu.esp)?;
                    let layer = if return_address == SKY_INDEXED_RETURN {
                        sky = Some(SkyIssue::read(&guest, &mem, cpu.esp, cpu.ebp));
                        Layer {
                            number: 15,
                            view: mem.read_u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX),
                            caller_return: return_address,
                            site: Site::Sky,
                            list: 0,
                            stencil_off: false,
                            texaccum: None,
                        }
                    } else {
                        current?
                    };
                    let geometry = if layer.site == Site::Sky {
                        None
                    } else {
                        Geometry::read(&mem, cpu.esp, layer.number)
                    };
                    if timeline::phase::enabled() && layer.site == Site::Scene && matches!(layer.number, 1 | 3 | 5 | 7 | 8)
                        && let Some(owner) = geometry.and_then(|g| match g.owner {
                            Owner::Cluster(n) => Some(native_render::world::Owner::Cluster(n)),
                            Owner::Placement(n) => Some(native_render::world::Owner::Instance(n)),
                            Owner::Object { .. } => None,
                        })
                        && let Some(bsp) = mem.read_u32(render_bridge::native_world::ACTIVE_BSP)
                    {
                        count_frontend(frame, layer.view, bsp, timeline::native::WorldOpaque::GuestIndexed, [owner]);
                    }
                    Some(DrawContext {
                        layer,
                        frame,
                        bsp: mem.read_u32(render_bridge::native_world::ACTIVE_BSP),
                        generation: render_bridge::native_map_work::bsp_generation(),
                        geometry,
                        light_selection: if matches!(layer.number, 12 | 13) {
                            geometry.and_then(|g| g.light_selection(&mem, cpu.esp))
                        } else {
                            None
                        },
                        index_count: cpu.esp.checked_add(8).and_then(|a| mem.read_u32(a)),
                        index_address: if matches!(layer.number, 12 | 13) {
                            cpu.esp.checked_add(12).and_then(|a| mem.read_u32(a))
                        } else {
                            None
                        },
                    })
                })();
                let _pool = RestorePool(POOL.replace(pool));
                let _restore = Restore(DRAW.replace(context));
                let _sky = sky.map(|item| RestoreSky(SKY.replace(Some(item))));
                Ok(previous.invoke(cpu, ctx, selector))
            },
        );
        if context_only {
            dispatch.mark_context_only(index);
        }
    });
}

/// Install the sorted handler's identity only while resolving one of its
/// native index spans. On a render-thread continuation the current layer
/// TLS may belong to another view, so use the layer retained by prepare.
pub fn enter_opaque_range(
    part: &native_render::world_ranges::SortedPart, layer: Layer, frame: u64, count: u32,
) -> impl Drop {
    let geometry = Geometry {
        shader: part.shader, variant: part.variant, implementation: part.implementation,
        sorted_slot: part.sorted_slot, record: part.record, part: part.part,
        owner: match part.owner {
            native_render::world::Owner::Cluster(n) => Owner::Cluster(n),
            native_render::world::Owner::Instance(n) => Owner::Placement(n),
        },
    };
    Restore(DRAW.replace(Some(DrawContext {
        layer, frame, geometry: Some(geometry), light_selection: None, index_count: Some(count), index_address: None,
        // The sorted range has no guest memory to read the BSP from here;
        // without it the section cache is bypassed for this range.
        bsp: None, generation: render_bridge::native_map_work::bsp_generation(),
    })))
}

struct RestorePool(Option<(u32, u32)>);

impl Drop for RestorePool {
    fn drop(&mut self) { POOL.set(self.0); }
}

pub(crate) fn pool_issue() -> Option<(u32, u32)> { POOL.get() }

/// Run `f` as if inside the guest's sorted pool issue `(view, index count)`.
#[cfg(test)]
pub(crate) fn with_pool_issue<R>(issue: (u32, u32), f: impl FnOnce() -> R) -> R {
    let _pool = RestorePool(POOL.replace(Some(issue)));
    f()
}

/// The sky strip at its issue: the item extracted, and its material's
/// layer-15 pass evaluated from the plan (`native_sky_glue::evaluate`),
/// which the strip's pass draws with.
pub(crate) struct SkyIssue {
    pub item: Result<SkyItem, Refusal>,
    pub constants: Result<native_render::sky_material::SkyConstants, &'static str>,
    /// The pass's program and textures from the material.
    pub look: Result<Arc<native_render::sky_material::SkyLook>, &'static str>,
    /// The item's section geometry from the load, and its part.
    pub geometry: Result<(Arc<native_render::sky_material::SkySection>, u32), &'static str>,
}

impl SkyIssue {
    fn read(guest: &runtime::Guest, mem: &Memory<'_>, esp: u32, pass: u32) -> Self {
        let item = SkyItem::read(mem, esp, pass);
        let (constants, look, geometry) = match &item {
            Ok(item) => (
                crate::native_sky_glue::evaluate(guest, item.shader, item.pass),
                crate::native_sky_glue::look_of(guest, item.shader, item.pass),
                crate::native_sky_glue::section_of(guest, item.model, item.section).map(|s| (s, item.part)),
            ),
            // Not asked: the item's refusal is the strip's cause.
            Err(_) => (Err("extraction"), Err("extraction"), Err("extraction")),
        };
        Self { item, constants, look, geometry }
    }
}

struct RestoreSky(Option<SkyIssue>);

impl Drop for RestoreSky {
    fn drop(&mut self) {
        if SKY.replace(self.0.take()).is_some() {
            // The strip's issue went unread.
            render_bridge::native_sky::count("device");
        }
    }
}

/// Consume the item captured for this issue, only after checking its draw context.
pub(crate) fn take_sky() -> Option<SkyIssue> {
    SKY.take()
}

pub fn indexed_draw() -> Option<DrawContext> {
    DRAW.get()
}

pub fn owns_scene_world(switches: &hooks::Switches) -> bool {
    render_bridge::native_world_layers::on(switches)
}

#[cfg(test)]
mod section_count_tests {
    use super::SectionCounts;
    use native_render::world::Owner::*;
    use timeline::native::WorldOpaque::*;

    #[test]
    fn counts_ranges_and_distinct_sections_in_each_view_and_path() {
        let mut s = SectionCounts::default();
        let owners = [Cluster(3), Cluster(3), Instance(3)];
        assert_eq!(s.add(5, 0, 42, GuestIndexed, owners), (3, 2));
        assert_eq!(s.add(5, 0, 42, GuestIndexed, owners), (3, 0));
        assert_eq!(s.add(5, 0, 42, Prereserved, owners), (3, 2));
        assert_eq!(s.add(5, 1, 42, GuestIndexed, owners), (3, 2));
        assert_eq!(s.add(5, 0, 43, GuestIndexed, owners), (3, 2));
        assert_eq!(s.add(6, 0, 42, GuestIndexed, owners), (3, 2));
    }
}
