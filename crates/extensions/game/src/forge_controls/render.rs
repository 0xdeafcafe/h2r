//! The Forge body's park (docs/game/forge-design-slices.md, "The monitor
//! body"). While the player flies a body, their own biped stays in the
//! world: undamageable, and left out of the render list by the "not drawn"
//! exit (skulls_custom's flags, docs/graphics/occlusion-culling.md, "Where
//! to skip"). The body is undamageable too. The player's view is their
//! own: the game draws it from their unit, the body, so this module only
//! reads it back for the tool (`eye`).
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use guest_layout::{GuestMemory, handle::DatumHandle, points, view::objects};
use engine::render_loop::occlusion::guest as vis;
use forge::camera::Camera;
use hooks::{BuildError, Builder, Flow};

use super::lock;
use crate::skulls_custom::{BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE};

static HIDDEN: AtomicU32 = AtomicU32::new(u32::MAX);

/// A unit the park made undamageable, and whether the bit was its to clear.
#[derive(Clone, Copy)]
struct Kept {
    scenario: u32,
    unit: u32,
    body: u32,
    protected: bool,
}
static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// The biped the park hides, while one is: `forge.status`'s `parked`, so a
/// check can tell a hidden biped from a missed park.
pub fn parked() -> Option<u32> {
    Some(HIDDEN.load(Ordering::Relaxed)).filter(|&unit| unit != u32::MAX)
}

/// A new map can reuse both a slot and its salt. Drop the old epoch's
/// bookkeeping without touching the new map.
pub fn forget_park() {
    HIDDEN.store(u32::MAX, Ordering::Relaxed);
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The body of `unit` when it still names the same object.
fn same(m: &dyn GuestMemory, k: &Kept, scenario: Option<u32>) -> bool {
    Some(k.scenario) == scenario
        && DatumHandle::from_raw(k.unit)
            .and_then(|h| objects::get(m, h).ok().flatten())
            .is_some_and(|(_, body)| body.addr == k.body)
}

/// Called by the admitted local session at a game boundary: `biped` is the
/// player's kept biped, hidden and undamageable; `body` the unit they fly,
/// undamageable. A unit no longer named gets back only the bit the park
/// added, and only while it is the same object.
fn park(m: &mut dyn GuestMemory, biped: Option<u32>, body: Option<u32>) {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let scenario = m.u32(points::SCENARIO.base).ok();
    let want: Vec<u32> = [biped, body].into_iter().flatten().collect();
    kept.retain(|k| {
        let alive = same(&*m, k, scenario);
        if want.contains(&k.unit) && alive {
            return true;
        }
        if k.protected
            && alive
            && let Some(at) = k.body.checked_add(BODY_DAMAGE_FLAGS)
            && let Ok(flags) = m.u16(at)
        {
            let _ = m.set_u16(at, flags & !CANNOT_TAKE_DAMAGE);
        }
        false
    });
    HIDDEN.store(u32::MAX, Ordering::Relaxed);
    let Some(scenario) = scenario else { return };
    for unit in want {
        let found = DatumHandle::from_raw(unit).and_then(|h| objects::get(&*m, h).ok().flatten());
        let Some((_, object)) = found else { continue };
        if !kept.iter().any(|k| k.unit == unit) {
            let Some(at) = object.addr.checked_add(BODY_DAMAGE_FLAGS) else { continue };
            let Ok(flags) = m.u16(at) else { continue };
            let protected =
                flags & CANNOT_TAKE_DAMAGE == 0 && m.set_u16(at, flags | CANNOT_TAKE_DAMAGE).is_ok();
            kept.push(Kept {
                scenario,
                unit,
                body: object.addr,
                protected,
            });
        }
        if Some(unit) == biped {
            HIDDEN.store(unit, Ordering::Relaxed);
        }
    }
}

/// The view the game draws for local seat 0, as the tool's camera.
fn drawn_eye(m: &dyn GuestMemory) -> Option<Camera> {
    let v = crate::spectator_view::drawn_view(m, 0)?;
    Some(Camera {
        position: v.eye,
        yaw: v.yaw,
        pitch: v.pitch,
    })
}

fn hidden_flags(m: &dyn GuestMemory, index: u16, hidden: u32) -> Option<(u32, u32, u8)> {
    if m.u32(vis::MAIN_VIS + vis::VIS_KIND).ok()? != 0 {
        return None;
    }
    let list = m.u32(vis::MAIN_VIS + vis::VIS_OBJECTS).ok()?;
    if index >= m.u16(list.checked_add(vis::LIST_COUNT)?).ok()? {
        return None;
    }
    let handles = m.u32(list.checked_add(vis::LIST_HANDLES)?).ok()?;
    if m.u32(handles.checked_add(4 * u32::from(index))?).ok()? != hidden {
        return None;
    }
    let flags = m.u32(list.checked_add(vis::LIST_FLAGS)?).ok()?;
    let at = flags.checked_add(2 * u32::from(index))?;
    Some((flags, at, m.u8(at).ok()? | vis::FLAG_NOT_DRAWN))
}

pub fn camera_hooks(b: &mut Builder) -> Result<(), BuildError> {
    b.raw()
        .before(
            &points::GAME_TICK,
            "park the Forge body's biped",
            |call| {
                let state = lock().state;
                if !state.active && HIDDEN.load(Ordering::Relaxed) == u32::MAX {
                    return Ok(Flow::Continue);
                }
                let mut m = call.ctx.memory();
                if state.active {
                    lock().eye = drawn_eye(&*m);
                }
                let admitted = crate::forge_map::admitted(&*m);
                let (biped, body) = if admitted { (state.parked, state.body) } else { (None, None) };
                if biped.is_some()
                    || body.is_some()
                    || !KEPT.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
                {
                    park(&mut *m, biped, body);
                }
                Ok(Flow::Continue)
            },
        )
        .last()
        .after_feature("sim-metrics");
    b.raw()
        .before(
            &points::OBJECT_RENDER_SETUP,
            "hide parked Forge biped",
            |call| {
                let hidden = HIDDEN.load(Ordering::Relaxed);
                if hidden == u32::MAX || call.cpu.esi != vis::MAIN_VIS {
                    return Ok(Flow::Continue);
                }
                let mut mem = call.ctx.memory();
                let Some(i) = call.cpu.esp.checked_add(4).and_then(|at| mem.u16(at).ok()) else {
                    return Ok(Flow::Continue);
                };
                let Some((flags, at, value)) = hidden_flags(&*mem, i, hidden) else {
                    return Ok(Flow::Continue);
                };
                // Check the return address before publishing a render-list flag.
                let target = mem.u32(call.cpu.esp)?;
                mem.set_u8(at, value)?;
                call.cpu.esp = call.cpu.esp.wrapping_add(24);
                let transfer = runtime::TransferAbi::ret(target);
                call.cpu.eax &= !0xff;
                call.cpu.ecx = at;
                call.cpu.edx = flags;
                Ok(Flow::Return(transfer))
            },
        )
        .first()
        .context_only();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{testing::FakeGuest, view::fake};
    use guest_layout::ObjectHeader;

    #[test]
    fn a_park_restores_only_the_bit_it_added_to_the_same_unit() {
        let mut m = FakeGuest::new();
        let (object_pool, biped_body, flying_body) = (0x5000, 0x7000, 0x7400);
        let objects = fake::datums::<objects::Header>(&mut m, object_pool, 2);
        let (biped, b) = fake::push(&mut m, &objects);
        let (flying, f) = fake::push(&mut m, &objects);
        m.put_u32(points::OBJECTS.base, object_pool)
            .put_u32(points::SCENARIO.base, 0x9000)
            .put_u8(b.addr + ObjectHeader::KIND.offset, objects::BIPED)
            .put_u32(b.addr + ObjectHeader::BODY.offset, biped_body)
            .put_u16(biped_body + BODY_DAMAGE_FLAGS, 1)
            .put_u8(f.addr + ObjectHeader::KIND.offset, objects::BIPED)
            .put_u32(f.addr + ObjectHeader::BODY.offset, flying_body)
            .put_u16(flying_body + BODY_DAMAGE_FLAGS, 0);
        forget_park();
        assert_eq!(parked(), None);
        park(&mut m, Some(biped.raw()), Some(flying.raw()));
        assert_eq!(parked(), Some(biped.raw()), "the biped is hidden, the body is not");
        assert_eq!(m.u16(biped_body + BODY_DAMAGE_FLAGS).unwrap(), 1 | CANNOT_TAKE_DAMAGE);
        assert_eq!(m.u16(flying_body + BODY_DAMAGE_FLAGS).unwrap(), CANNOT_TAKE_DAMAGE);
        park(&mut m, None, None);
        assert_eq!(parked(), None);
        assert_eq!(m.u16(biped_body + BODY_DAMAGE_FLAGS).unwrap(), 1);
        assert_eq!(m.u16(flying_body + BODY_DAMAGE_FLAGS).unwrap(), 0);

        // A bit the unit had before the park stays.
        m.put_u16(biped_body + BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE);
        park(&mut m, Some(biped.raw()), None);
        park(&mut m, None, None);
        assert_eq!(m.u16(biped_body + BODY_DAMAGE_FLAGS).unwrap(), CANNOT_TAKE_DAMAGE);

        // Another object in the slot (a new salt) is left alone.
        m.put_u16(biped_body + BODY_DAMAGE_FLAGS, 0);
        park(&mut m, Some(biped.raw()), None);
        m.put_u16(b.addr + ObjectHeader::SALT.offset, 0x751);
        park(&mut m, None, None);
        assert_eq!(m.u16(biped_body + BODY_DAMAGE_FLAGS).unwrap(), CANNOT_TAKE_DAMAGE);
    }

    #[test]
    fn the_render_skip_matches_the_handle_in_the_main_list_and_preserves_flags() {
        let mut m = FakeGuest::new();
        let (list, handles, flags, unit) = (0x3000, 0x5000, 0x7000, 0xc123_0004);
        m.put_u32(vis::MAIN_VIS + vis::VIS_KIND, 0)
            .put_u32(vis::MAIN_VIS + vis::VIS_OBJECTS, list)
            .put_u16(list + vis::LIST_COUNT, 2)
            .put_u32(list + vis::LIST_HANDLES, handles)
            .put_u32(list + vis::LIST_FLAGS, flags)
            .put_u32(handles, unit - 1)
            .put_u32(handles + 4, unit)
            .put_u8(flags + 2, 8);
        assert_eq!(hidden_flags(&m, 0, unit), None);
        assert_eq!(hidden_flags(&m, 1, unit), Some((flags, flags + 2, 9)));
        assert_eq!(hidden_flags(&m, 2, unit), None);
        m.put_u32(vis::MAIN_VIS + vis::VIS_KIND, 1);
        assert_eq!(hidden_flags(&m, 1, unit), None);
    }
}
