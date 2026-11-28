//! Scene front-end hooks. Every operation is synchronous against the active
//! render memory so a render-thread fallback can resume at the next guest call.

use engine::memory::GuestMemory;
use engine::points;
use engine::render::light_gather::{LIGHT_VIS, object_shadows::caster, passes};
use engine::render_loop::world_walk::{self, Walk};
use native_render::skip;
use timeline::native::{Metric, Stage};

use hooks::{Builder, Flow};

pub(super) fn install(b: &mut Builder) {
    for (point, walk, site, cleanup) in [
        (&points::WORLD_CLUSTER_RECORDS, Walk::Clusters, world_walk::CLUSTERS_RETURN, 0),
        (&points::WORLD_PLACEMENT_RECORDS, Walk::Placements, world_walk::PLACEMENTS_RETURN, 4),
    ] {
        b.raw().before(point, "native_world_visible_sections", move |c| {
            let visited = {
                let mut m = c.ctx.runtime().memory();
                let vis = match walk {
                    Walk::Clusters => c.cpu.ebx,
                    Walk::Placements => match m.u32(c.cpu.esp.wrapping_add(4)) {
                        Ok(vis) => vis,
                        Err(_) => return Ok(Flow::Continue),
                    },
                };
                let sp = c.cpu.esp;
                if m.u32(sp).ok() != Some(site) && !caster_walk(&*m, sp, walk, vis) {
                    return Ok(Flow::Continue);
                }
                world_walk::publish(&mut *m, vis, walk, c.cpu.mxcsr, c.cpu.x87.control)
            };
            if let Some(sections) = visited {
                Stage::World.count(Metric::WalkSections, u64::from(sections));
                return c.ret(cleanup).map(Flow::Return);
            }
            Ok(Flow::Continue)
        }).context_only();
    }

    b.raw().before(&points::WORLD_VISIBILITY_FLAGS, "native_world_resident_row", |c| {
        let result = {
            let mut m = c.ctx.runtime().memory();
            let sp = c.cpu.esp;
            (|| {
                let (walk, index, resume) = match m.u32(sp).ok()? {
                    0x0013_33b5 => (Walk::Clusters, c.cpu.edi, 0x0013_3509),
                    0x0013_3582 => (Walk::Placements, c.cpu.ebx, 0x0013_36d8),
                    _ => return None,
                };
                let vis = m.u32(sp.checked_add(4)?).ok()?;
                let sections = world_walk::publish_slice(&mut *m, vis, walk, c.cpu.mxcsr,
                    c.cpu.x87.control, Some(index))?;
                Some((resume, sections))
            })()
        };
        if let Some((resume, sections)) = result {
            // Finish this one iteration at the real post-record call site.
            // The loop reloads the list and counts and preserves its cursor.
            c.cpu.esp = c.cpu.esp.wrapping_add(8);
            Stage::World.count(Metric::WalkSections, u64::from(sections));
            return Ok(Flow::Return(runtime::TransferAbi::resume(resume)));
        }
        Ok(Flow::Continue)
    }).context_only();

    crate::native_scene_lists::transparent_bsp::parity::install(b);
    let sections = std::sync::Mutex::new(super::section::SectionCache::default());
    b.raw().before(&points::WORLD_PART_MATERIAL, "native_world_build_section", move |c| {
        if c.cpu.edi != 0 || c.cpu.mxcsr & 0xffc0 != 0x1f80 { return Ok(Flow::Continue); }
        let mut mem = c.ctx.runtime().memory();
        let parts = sections.lock().unwrap_or_else(|e| e.into_inner()).append_guest(&mut mem, c.cpu.esp, c.cpu.ecx);
        drop(mem);
        if let Some((parts, hit)) = parts {
            // The material scratch is only consumed on AL != 0. Resume at
            // the original loop test with all this section's items published.
            c.cpu.edi = parts - 1;
            c.cpu.eax &= !0xff;
            if !hit { Stage::World.count(Metric::SectionsPrepared, 1); }
            return c.ret(8).map(Flow::Return);
        }
        Ok(Flow::Continue)
    }).context_only();

    b.raw().before(&points::WORLD_GEOMETRY_BUILD, "native_world_build_entry", |c| {
        // The transparent handler's own build (layer 15): R2's geometry arm
        // makes the slot and AL in Rust on every frame.
        let handler = crate::effects_shadow::geometry::build(&mut *c.ctx.runtime().memory(), c.cpu.esp);
        if let Some(accepted) = handler {
            c.cpu.eax = c.cpu.eax & !0xff | u32::from(accepted);
            return c.ret(28).map(Flow::Return);
        }
        let built = {
            let mut m = c.ctx.runtime().memory();
            let sp = c.cpu.esp;
            (|| {
                if m.u32(sp).ok()? != BUILD_RETURN || super::layer_build::verifying() { return None; }
                let arg = |n: u32| m.u32(sp.checked_add(4 + 4*n)?).ok();
                let layer = arg(3)?;
                if !matches!(layer, 1 | 3 | 5 | 6 | 7 | 8 | 16) { return None; }
                let slot = arg(6)?;
                let mut old = [0; 24];
                GuestMemory::read(&*m, slot, &mut old).ok()?;
                let entry = super::scene_build::build(&*m, arg(0)?, arg(2)? as u8, layer,
                    arg(4)? as u8, arg(5)?, old, false)?;
                GuestMemory::write(&mut *m, slot, &entry.bytes).ok()?;
                Some(entry.accepted)
            })()
        };
        if let Some(accepted) = built {
            c.cpu.eax = c.cpu.eax & !0xff | u32::from(accepted);
            Stage::World.count(Metric::BuildsSkipped, 1);
            return c.ret(28).map(Flow::Return);
        }
        Ok(Flow::Continue)
    }).context_only();

    b.raw().before(&points::WORLD_PART_EMIT, "native_world_full_part", |c| {
        if c.cpu.eax as u8 == 0 { return Ok(Flow::Continue); }
        let Some(layer) = crate::native_world_layers::current() else { return Ok(Flow::Continue) };
        if layer.site != native_render::world_layers::Site::Scene ||
            !matches!(layer.number, 1 | 3 | 5 | 7 | 8) { return Ok(Flow::Continue); }
        let whole = {
            let m = c.ctx.runtime().memory();
            let sp = c.cpu.esp;
            (|| {
                if m.u32(sp).ok()? != EMITTER_RETURN ||
                    m.u32(sp.checked_add(0x60)?).ok()? != layer.number { return None; }
                let index = (m.u32(sp.checked_add(0x68)?).ok()? >> 8) as u16;
                if index >= 0x352 || m.u32(skip::RENDER_RECORDS +
                    u32::from(index)*skip::RENDER_RECORD_SIZE + 4).ok()? != u32::MAX { return None; }
                super::scene_build::full_range(&*m, m.u32(sp.checked_add(4)?).ok()?,
                    m.u32(sp.checked_add(8)?).ok()?, m.u32(sp.checked_add(12)?).ok()?, c.cpu.ecx as u8 != 0)
            })()
        };
        if whole == Some(true) {
            // Both arms flush textures once then issue the same indexed call.
            // The handler discards EAX and executes its normal stencil tail.
            c.cpu.eax &= !0xff;
        }
        Ok(Flow::Continue)
    }).context_only();

    // A before hook returning a transfer bypasses after observers. Deep
    // timing brackets both of these points, so use the per-item path there.
    if b.switches().feature(crate::render_timing::DEEP, false) { return; }
    b.raw().before(&points::RENDER_STATES, "native_world_shader_states", |c| {
        let Some(layer) = crate::native_world_layers::current() else { return Ok(Flow::Continue) };
        if layer.site != native_render::world_layers::Site::Scene { return Ok(Flow::Continue); }
        let mut m = c.ctx.runtime().memory();
        if !world_shader_state_list(&*m, c.cpu.esp, layer.number) { return Ok(Flow::Continue); }
        crate::render_loop::world_states(c.cpu, &mut m).map(Flow::Return)
    }).context_only();

    b.raw().observe_before(&points::scene_effects::LAYER_BUILT, "native_geometry_list_parity", |o| {
        super::layer_build::finish(&*o.guest(), o.cpu);
    }).context_only();

    b.raw().before(&points::LAYER_BUILD, "native_world_layer_build", |c| {
        let mut mem = c.ctx.runtime().memory();
        let result = match super::layer_build::complete(&mut *mem, c.cpu) {
            Some(super::layer_build::Decision::Compare) => return Ok(Flow::Continue),
            Some(super::layer_build::Decision::Native(transfer, built)) => Some((transfer, built)),
            None => super::layer_build::prefix(&mut *mem, c.cpu),
        };
        if let Some((transfer, built)) = result {
            Stage::World.count(Metric::BuildsSkipped, u64::from(built));
            return Ok(Flow::Return(transfer));
        }
        Ok(Flow::Continue)
    }).context_only();

    b.raw().before(&points::LAYER_ENTRIES, "native_world_empty_entries", |c| {
        let layer = c.cpu.ebx;
        if !matches!(layer, 3 | 6 | 16) { return Ok(Flow::Continue); }
        let empty = {
            let m = c.ctx.runtime().memory();
            let list = c.cpu.edi;
            m.u32(c.cpu.esp).ok() == Some(ENTRIES_RETURN)
                && m.u32(skip::CURRENT_LIST).ok() == Some(list)
                && m.u32(skip::CURRENT_LAYER).ok() == Some(layer)
                && super::claimed_entries(layer)
                && all_masked(&*m, list, layer) == Some(true)
        };
        if empty {
            // The zero-entry build returns EAX=0; EDI/EBX/EBP/ESI are
            // callee-saved. The caller immediately restores EDI and EBX.
            c.cpu.eax = 0;
            Stage::World.count(Metric::EntryPassesSkipped, 1);
            return c.ret(4).map(Flow::Return);
        }
        Ok(Flow::Continue)
    }).after_feature(crate::render_timing::ID).context_only();
}

// Return addresses of the standard emitter, item builder and layer dispatcher.
const EMITTER_RETURN: u32 = 0x0004_d626;
/// The caster calls `0x0002cbc0`, which saves EBX before both walks and
/// pushes one argument for the placement walk. Match both return slots.
fn caster_walk(m: &dyn GuestMemory, sp: u32, walk: Walk, vis: u32) -> bool {
    let (inner, outer_offset) = match walk {
        Walk::Clusters => (0x0002_cbd1, 8),
        Walk::Placements => (0x0002_cbd7, 12),
    };
    vis == LIGHT_VIS && m.u32(sp).ok() == Some(inner)
        && sp.checked_add(outer_offset).and_then(|slot| passes::caller(m, slot))
            == Some(caster::RECORDS_RETURN)
}

const BUILD_RETURN: u32 = 0x0003_bb2e;
const ENTRIES_RETURN: u32 = 0x0004_45b7;

// Stack at RENDER_STATES, through the standard material handler only.
// 16b90 saves five words and pushes the list. 1bbf0 saves EBX and
// pushes seven arguments. 4d9c0 saves three words and pushes five.
fn world_shader_state_list(m: &dyn GuestMemory, sp: u32, layer: u32) -> bool {
    if !matches!(layer, 1 | 3 | 5 | 7 | 8) || sp.checked_add(0x6c).is_none() { return false; }
    if m.u32(sp).ok() != Some(0x0001_6efe) || m.u32(sp+0x1c).ok() != Some(0x0001_bd4c)
        || m.u32(sp+0x40).ok() != Some(0x0004_da93) || m.u32(sp+0x64).ok() != Some(0x0004_d5d7) {
        return false;
    }
    let Ok(index) = m.u32(sp+0x68) else { return false };
    index < 0x352 && m.u32(skip::RENDER_RECORDS + index*skip::RENDER_RECORD_SIZE + 4).ok() == Some(u32::MAX)
}

fn all_masked(m: &dyn GuestMemory, list: u32, layer: u32) -> Option<bool> {
    if list == 0 || layer >= 32 { return None; }
    let first = m.u32(list.checked_add(0xc)?).ok()?;
    let count = m.u32(list.checked_add(4)?).ok()?;
    if first >= count || count - first > skip::MAX_ITEMS { return None; }
    let items = m.u32(list.checked_add(0x14)?).ok()?;
    if items == 0 { return None; }
    items.checked_add(count.checked_mul(skip::ITEM_SIZE)?)?;
    for slot in first..count {
        if m.u32(items + slot * skip::ITEM_SIZE + 0x10).ok()? & (1 << layer) != 0 {
            return Some(false);
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    #[test]
    fn shadow_record_walks_require_the_caster_and_the_correct_visibility() {
        let sp = 0x1000;
        let mut m = FlatMemory::new(sp, 16);
        for (walk, inner, offset) in [(Walk::Clusters, 0x2cbd1, 8), (Walk::Placements, 0x2cbd7, 12)] {
            m.set_u32(sp, inner).unwrap();
            m.set_u32(sp + offset, caster::RECORDS_RETURN).unwrap();
            assert!(caster_walk(&m, sp, walk, LIGHT_VIS));
            assert!(!caster_walk(&m, sp, walk, engine::render::light_gather::MAIN_VIS));
            m.set_u32(sp + offset, 0).unwrap();
            assert!(!caster_walk(&m, sp, walk, LIGHT_VIS));
        }
    }

    #[test]
    fn whole_pass_requires_every_remaining_item_to_have_lost_this_layer() {
        let mut m = FlatMemory::new(0x1000, 256);
        m.set_u32(0x1004, 3).unwrap();
        m.set_u32(0x100c, 1).unwrap();
        m.set_u32(0x1014, 0x1040).unwrap();
        m.set_u32(0x1050, 1<<3).unwrap(); // Outside the active slice.
        m.set_u32(0x1068, 1<<16).unwrap();
        assert_eq!(all_masked(&m, 0x1000, 3), Some(true));
        m.set_u32(0x1080, 1<<3).unwrap(); // A single guest draw remains.
        assert_eq!(all_masked(&m, 0x1000, 3), Some(false));
        m.set_u32(0x1014, 0xfff0).unwrap();
        assert_eq!(all_masked(&m, 0x1000, 3), None);
    }

    #[test]
    fn shader_state_skip_requires_a_bsp_record_and_the_complete_opaque_call_chain() {
        let mut m = FlatMemory::new(skip::RENDER_RECORDS, 1024);
        let sp = skip::RENDER_RECORDS + 512;
        for (n, pc) in [(0, 0x16efe), (0x1c, 0x1bd4c), (0x40, 0x4da93), (0x64, 0x4d5d7)] {
            m.set_u32(sp+n, pc).unwrap();
        }
        m.set_u32(skip::RENDER_RECORDS+4, u32::MAX).unwrap();
        assert!(world_shader_state_list(&m, sp, 1));
        assert!(!world_shader_state_list(&m, sp, 12));
        m.set_u32(skip::RENDER_RECORDS+4, 0).unwrap();
        assert!(!world_shader_state_list(&m, sp, 1));
        m.set_u32(skip::RENDER_RECORDS+4, u32::MAX).unwrap();
        m.set_u32(sp+0x40, 17).unwrap();
        assert!(!world_shader_state_list(&m, sp, 1));
    }

    #[test]
    fn the_master_switch_gates_the_walk() {
        for (switches, on) in [("", true), ("impl.render.native=off", false)] {
            assert_eq!(crate::native_world::on(&hooks::Switches::parse(switches).unwrap()), on);
        }
    }
}
