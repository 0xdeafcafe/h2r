//! Takes a consecutive span of ordinary BSP entries at the sorted handler.
//! The span ends at the first different dispatch or unprepared material.
//! Its outgoing state is installed before the caller advances to that entry.

use std::sync::Arc;

use guest_layout::entry::EntryError;
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use engine::render_loop::layer::HANDLER_CLEANUP;
use native_render::world_layers::Site;
use native_render::world_ranges::{Refusal, SortedPart, SortedRun};
use timeline::native::{WorldSection, world_handler_fallback, world_handler_indexed, world_handler_taken};

use super::opaque_handler::Emission;
use super::{HOST, Mem, OpaqueDraw, OpaqueView, ViewLodScale};
use super::opaque_material::SectionMaterials;
use hooks::{Builder, Call, Cx, Flow, Ret};

// Addresses of the inherited material/vertex caches. The memory view folds
// writes by physical page, so alternate aliases of any field see it too.
const CACHE_PAGES: &[u32] = &[
    0x0048_5b48, 0x0048_6b48, 0x0048_72a0, // shader context through register masks
    0x0051_f0f0, // vertex shader cache, VAF, streams and bound texture headers
    0x0040_4dd0, 0x0040_5000, // sampler stage shadows
    engine::render_loop::states::SHADOW, // device dirty flags and render-state shadow
    0x0040_8518, // static vertex shader declaration mirror
    engine::render_loop::world_lightmap::STAGE_PALETTES,
    engine::render_loop::world_lightmap::LAST_PALETTE,
    0x0050_93d8, // node cache and BSP target status
    0x004b_81fc, // engine render-state mirrors
];

pub(super) fn install(b: &mut Builder, fade: (Option<ViewLodScale>, bool)) {
    b.raw()
        .before(&points::LIGHT_GEOMETRY_DRAW, "native_world_opaque_handler", move |c| {
            sorted(c, fade)
        })
        .after_feature(crate::native_lights::ID)
        .after_feature(crate::native_objects::ID)
        .context_only();
}

pub(super) struct Prepared {
    part: SortedPart,
    draw: Box<dyn OpaqueDraw>,
    material: super::opaque_material::Prepared,
}

fn reason(refusal: Refusal) -> Option<&'static str> {
    match refusal {
        Refusal::OtherHandler | Refusal::OtherLayer | Refusal::Object => None,
        Refusal::Memory => Some("unreadable sorted draw"),
        Refusal::Record => Some("invalid render record"),
        Refusal::Section => Some("unresolved BSP section"),
        Refusal::Part => Some("unresolved geometry part"),
        Refusal::Indices => Some("unresolved index range"),
    }
}

fn section(view: u32, bsp: u32, owner: native_render::world::Owner) -> WorldSection {
    use native_render::world::Owner;
    match owner {
        Owner::Cluster(index) => WorldSection::Cluster { view, bsp, index },
        Owner::Instance(index) => WorldSection::Placement { view, bsp, index },
    }
}

fn sorted(c: &mut Call<'_>, fade: (Option<ViewLodScale>, bool)) -> Result<Flow, runtime::GuestFault> {
    let Some(layer) = crate::native_world_layers::current() else { return Ok(Flow::Continue) };
    if layer.site != Site::Scene || !matches!(layer.number, 1 | 3 | 5 | 7 | 8) {
        return Ok(Flow::Continue);
    }
    let esp = c.cpu.esp;
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let caller = mem.u32(esp)?;
    if caller != native_render::light_list::HANDLER_RETURN {
        return Ok(Flow::Continue);
    }
    let Ok(sorted) = SortedRun::read(&Mem(&mut *mem), layer.list, esp, c.cpu.esi) else {
        return Ok(Flow::Continue);
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return Ok(Flow::Continue);
    };
    let bsp = mem.u32(super::ACTIVE_BSP)?;
    let part = match sorted.next(&Mem(&mut *mem), bsp, 0) {
        Ok(part) => part,
        Err(why) => {
            if let Some(why) = reason(why) {
                let shader = mem.u32(esp + 4)?;
                world_handler_fallback(shader, why);
                if timeline::phase::enabled() && let Some(owner) = SortedPart::owner_at(&Mem(&mut *mem), bsp, esp) {
                    section(layer.view, bsp, owner).fallback(shader, why);
                }
            }
            return Ok(Flow::Continue);
        }
    };
    let fcw = c.cpu.x87.control;
    let mxcsr = c.cpu.mxcsr;
    let fade = super::FadePlan::resolve(&*mem, fade.0, fade.1);
    let shader = part.shader;
    let first_section = section(layer.view, bsp, part.owner);
    let mut materials = SectionMaterials::default();
    let begin = host.opaque_view(&*mem, layer).and_then(|view| {
        let first = prepare(&mut *mem, &*view, &mut materials, part, fcw, mxcsr)?;
        Ok((view, first))
    });
    drop(mem);
    let (view, first) = match begin {
        Ok(prepared) => prepared,
        Err(why) => {
            first_section.fallback(shader, why);
            world_handler_fallback(shader, why);
            return Ok(Flow::Continue);
        }
    };
    let setter = hooks::entry::RegisterSetter {
        address: engine::render_loop::states::SIMPLE,
        cache_pages: CACHE_PAGES,
        // Boot installs its shared dirty-page barrier around the D3D ABI.
        // The host drains that same watch before the register write.
        name: "boot:d3d write barrier",
        apply: Box::new(move |mem, header, value| host.try_opaque_simple(mem, header, value)),
    };
    c.answer_render_async_with(setter, move |cx| Box::pin(async move {
        handoff(&cx, first, &*view, &sorted, layer.view, bsp, &mut materials, fcw, mxcsr,
            fade).await
    }))
}

pub(super) fn prepare(mem: &mut dyn GuestMemory, view: &dyn OpaqueView, materials: &mut SectionMaterials,
    part: SortedPart, fcw: u16, mxcsr: u32) -> Result<Prepared, &'static str>
{
    let material = materials.prepare(mem, &part, fcw, mxcsr)?;
    let draw = view.prepare_opaque(mem, &part)?;
    Ok(Prepared { part, draw, material })
}

/// Run until the first interleaved handler or the end of the sorted list.
/// The caller can enter guest code as soon as this returns. All state and
/// resource lifetime transitions for the last consumed entry have completed.
pub(super) async fn handoff(cx: &Cx, mut current: Prepared, view: &dyn OpaqueView, sorted: &SortedRun,
    view_index: u32, bsp: u32, materials: &mut SectionMaterials, fcw: u16, mxcsr: u32,
    fade: super::FadePlan) -> Result<Ret, EntryError>
{
    for offset in 1..=sorted.remaining {
        let Prepared { part, material, draw } = current;
        match super::opaque_handler::handoff(cx, &part, draw, fcw, mxcsr, &material, fade).await? {
            Emission::Native(n) => world_handler_taken(part.shader, n),
            Emission::NoDraw => world_handler_taken(part.shader, 0),
            Emission::Indexed(why, n) => world_handler_indexed(part.shader, why, n),
        }
        section(view_index, bsp, part.owner).taken();
        let next = cx.write(|mem| {
            if offset == sorted.remaining || mem.u32(super::ACTIVE_BSP).ok() != Some(bsp)
                || mem.u32(sorted.counter).ok() != Some(sorted.remaining - (offset - 1)) {
                return None;
            }
            // A streaming or resource release callback can run during the
            // prior item. Decode this item's live range and preflight its
            // resource references only after that callback has returned.
            let part = sorted.next(&Mem(mem), bsp, offset).ok()?;
            prepare(mem, view, materials, part, fcw, mxcsr).ok()
        });
        let Some(next) = next else {
            // The original loop advances once after ret 28. Leave it on the
            // last entry taken, including when the next slot was refused.
            return Ok(Ret::new(0, HANDLER_CLEANUP as u16).with_esi(part.sorted_slot));
        };
        cx.write(|m| {
            let p = &next.part;
            let argument = u32::from(p.part) | u32::from(p.record) << 8
                | u32::from(p.projected_size) << 24;
            let frame = [p.shader, p.layer_argument, u32::from(p.lod), p.layer,
                u32::from(p.pass), argument, p.sorted_slot];
            for (i, value) in frame.into_iter().enumerate() {
                m.set_u32(cx.entry().esp + 4 + 4 * i as u32, value)?;
            }
            m.set_u32(sorted.counter, sorted.remaining - offset)
        })?;
        current = next;
    }
    unreachable!("the sorted list contains its first accepted entry")
}
