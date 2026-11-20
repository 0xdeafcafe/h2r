//! Pairs the layer hook with the actual indexed D3D call and state.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

use d3d::backend::{
    DrawAdmission, DrawSnapshot, ExternalAttachments, ExternalDraws, ExternalPass,
    ExternalPassRequest, ExternalPlan, ExternalSlot, ExternalTarget, ExternalWrites, ResolvedDraw,
};
use engine::memory::GuestMemory as _;
use render_bridge::native_world_layers::{Layer, current};
use native_render::camera::GuestRead;
use native_render::world_layers::{
    DRAW_INDEXED_VERTICES, Geometry, Owner, SKY_INDEXED_RETURN, Site, Texaccum,
};
use native_render::world_layers_gpu::{Cache, IndexedPass};

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
    bsp: Option<u32>,
    generation: u64,
    pub index_count: Option<u32>,
}

thread_local! {
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

/// Clears the context even when the D3D handler returns a fault or calls
/// out to the guest before its draw. Such a call re-enters this wrapper.
struct Restore(Option<DrawContext>);

impl Drop for Restore {
    fn drop(&mut self) {
        DRAW.set(self.0);
    }
}

pub fn install<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &d3d::abi::SharedDevice<B>,
) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_world_layers::on(&switches) {
        return;
    }
    if let Ok(mut device) = device.lock() {
        device.add_opaque_world_admission(Arc::new(Admission::default()));
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
                let context = (|| {
                    let frame = crate::d3d_diag::presents() + 1;
                    let current = current();
                    let guest = ctx.memory();
                    let mem = Memory(&guest);
                    let return_address = mem.read_u32(cpu.esp)?;
                    let layer = if return_address == SKY_INDEXED_RETURN {
                        Layer {
                            number: 15,
                            view: u32::MAX,
                            caller_return: return_address,
                            site: Site::Sky,
                            list: 0,
                            stencil_off: false,
                            texaccum: None,
                        }
                    } else {
                        current?
                    };
                    let geometry = Geometry::read(&mem, cpu.esp, layer.number);
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
                        index_count: cpu.esp.checked_add(8).and_then(|a| mem.read_u32(a)),
                    })
                })();
                let _restore = Restore(DRAW.replace(context));
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
        layer, frame, geometry: Some(geometry), index_count: Some(count),
        // The sorted range has no guest memory to read the BSP from here;
        // without it the section cache is bypassed for this range.
        bsp: None, generation: render_bridge::native_map_work::bsp_generation(),
    })))
}

pub fn indexed_draw() -> Option<DrawContext> {
    DRAW.get()
}

/// The native model and BSP buffers store complete triangle lists. A
/// partial emission from the same geometry record cannot use that range.
pub fn matches_triangle_range(draw: &ResolvedDraw, expected: usize) -> bool {
    let Some(indices) = draw.indices.as_deref() else { return false };
    use gfx::vertex::PrimitiveType::{TriangleList, TriangleStrip};
    match draw.primitive {
        TriangleList => indices.len() == expected,
        TriangleStrip => indices.windows(3).filter(|t| {
            t[0] != t[1] && t[1] != t[2] && t[0] != t[2]
        }).count() * 3 == expected,
        _ => false,
    }
}

#[derive(Default)]
struct Admission {
    cache: Arc<Cache>,
}

pub fn owns_scene_world(switches: &hooks::Switches) -> bool {
    render_bridge::native_world_layers::on(switches)
}

/// Layer 3 can draw a material that never reads the accumulation level.
fn layer3_texaccum(
    target: Option<Texaccum>,
    color_header: u32,
    textures: [Option<u32>; 4],
    sampled: [bool; 4],
) -> Option<Option<Texaccum>> {
    use native_render::object_program::TEXACCUM_HEADER;
    if !textures.into_iter().zip(sampled).any(|(header, active)| {
        active && header == Some(TEXACCUM_HEADER)
    }) {
        return Some(None);
    }
    let target = target?;
    (target.texture == TEXACCUM_HEADER && target.surface? != color_header).then_some(Some(target))
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let c = DRAW.get()?;
        let geometry = c.geometry?;
        let layer = c.layer;
        let draw = snapshot.draw;
        if layer.site != Site::Scene
            || layer.list == 0
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || c.index_count? as usize != draw.indices.as_ref()?.len()
        {
            return None;
        }
        let mut slots = Vec::new();
        let texaccum = match layer.number {
            1 => {
                let target = layer.texaccum?;
                if target.surface? != snapshot.color_header {
                    return None;
                }
                None
            }
            3 => {
                let target = layer3_texaccum(
                    layer.texaccum, snapshot.color_header, draw.textures, snapshot.sampled_stages,
                )?;
                if let Some(target) = target {
                    // Pin the layer-1 attachment separately from engine
                    // texture 3. Preflight checks their views on every copy.
                    slots.push(Some(ExternalSlot { header: target.surface?, written: false }));
                }
                target
            }
            5 | 7 | 8 => None,
            _ => return None,
        };
        let stage = match geometry.owner {
            Owner::Cluster(_) | Owner::Placement(_) => timeline::native::Stage::World,
            Owner::Object { .. } => timeline::native::Stage::Objects,
        };
        let pass = IndexedPass::for_section(draw, Arc::clone(&self.cache), c.frame, texaccum,
            (c.generation, c.bsp?), geometry, layer.number)?;
        Some((
            ExternalPlan {
                stage,
                view: layer.view,
                ranges: 1,
                slots,
            },
            Box::new(Replacement { pass }),
        ))
    }
}

struct Replacement {
    pass: IndexedPass,
}

impl ExternalPass for Replacement {
    fn uses_validated_resources(&self) -> bool {
        self.pass.uses_validated_resources()
    }

    fn encode(&mut self, _target: ExternalTarget<'_>) {}

    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        self.pass.resolved_fallback()
    }

    fn writes(&self) -> ExternalWrites {
        self.pass.writes()
    }

    fn sampled(&self) -> Vec<u32> {
        self.pass.sampled()
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        self.pass.record_draws(target, request)
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn only_live_accumulation_samples_require_a_separate_pinned_level() {
        let target = Texaccum {
            texture: native_render::object_program::TEXACCUM_HEADER,
            surface: Some(20),
            size: (32, 32),
        };
        let textures = [Some(target.texture), Some(30), None, None];
        assert_eq!(layer3_texaccum(None, 10, textures, [false, true, false, false]), Some(None));
        assert_eq!(layer3_texaccum(None, 10, textures, [true; 4]), None);
        assert_eq!(layer3_texaccum(Some(target), 20, textures, [true; 4]), None);
        assert_eq!(layer3_texaccum(Some(target), 10, textures, [true; 4]), Some(Some(target)));
    }
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
