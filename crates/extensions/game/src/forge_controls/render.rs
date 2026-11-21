//! Forge uses the same private world view used by the director camera. The
//! parked biped's flags and render-list exit are named in skulls_custom and
//! docs/graphics/occlusion-culling.md (“Where to skip”).
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use engine::{
    GuestMemory,
    handle::DatumHandle,
    points,
    render_loop::{occlusion::guest as vis, views::VIEW_STRIDE},
    view::objects,
};
use forge::camera::Camera;
use hooks::{BuildError, Builder, Flow};

use super::lock;
use crate::skulls_custom::{BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE, local_units};
use crate::spectator_view::{Pose, draw_world_view, retail_view};

static HIDDEN: AtomicU32 = AtomicU32::new(u32::MAX);

#[derive(Clone, Copy)]
struct Parked {
    scenario: u32,
    unit: u32,
    body: u32,
    protected: bool,
}
static PARKED: Mutex<Option<Parked>> = Mutex::new(None);

/// The unit the monitor parks and hides, while one is: `forge.status`'s
/// `parked`, so a check can tell a hidden biped from a missed park.
pub fn parked() -> Option<u32> {
    Some(HIDDEN.load(Ordering::Relaxed)).filter(|&unit| unit != u32::MAX)
}

/// A new map can reuse both a slot and its salt. Drop the old epoch's
/// bookkeeping without touching the new map.
pub fn forget_park() {
    HIDDEN.store(u32::MAX, Ordering::Relaxed);
    *PARKED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Called by the admitted local session at a game boundary.
fn park(m: &mut dyn GuestMemory, monitor: bool) {
    let mut state = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    if !monitor && state.is_none() {
        return;
    }
    let scenario = m.u32(points::SCENARIO.base).ok();
    let current = monitor
        .then(|| {
            local_units(m)
                .ok()?
                .into_iter()
                .find(|u| u.user == 0 && u.kind == objects::BIPED)
        })
        .flatten();
    let same = state.is_some_and(|p| {
        Some(p.scenario) == scenario
            && current.is_some_and(|u| u.unit == p.unit && u.body == p.body)
    });
    if !same {
        HIDDEN.store(u32::MAX, Ordering::Relaxed);
        if let Some(old) = state.take()
            && old.protected
            && Some(old.scenario) == scenario
            && let Some(h) = DatumHandle::from_raw(old.unit)
            && let Ok(Some((kind, body))) = objects::get(m, h)
            && kind == objects::BIPED
            && body.addr == old.body
            && let Some(at) = body.addr.checked_add(BODY_DAMAGE_FLAGS)
            && let Ok(flags) = m.u16(at)
        {
            let _ = m.set_u16(at, flags & !CANNOT_TAKE_DAMAGE);
        }
    }
    if let (Some(scenario), Some(u)) = (scenario, current)
        && let Some(at) = u.body.checked_add(BODY_DAMAGE_FLAGS)
        && let Ok(flags) = m.u16(at)
    {
        let p = state.get_or_insert(Parked {
            scenario,
            unit: u.unit,
            body: u.body,
            protected: false,
        });
        if flags & CANNOT_TAKE_DAMAGE == 0 && m.set_u16(at, flags | CANNOT_TAKE_DAMAGE).is_ok() {
            p.protected = true;
        }
        HIDDEN.store(u.unit, Ordering::Relaxed);
    }
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
            "park the Forge monitor's biped",
            |call| {
                let monitor = lock().monitor;
                if monitor || HIDDEN.load(Ordering::Relaxed) != u32::MAX {
                    let mut m = call.ctx.memory();
                    let admitted = monitor && crate::forge_map::admitted(&*m);
                    park(&mut *m, admitted);
                }
                Ok(Flow::Continue)
            },
        )
        .last()
        .after_feature("sim-metrics");
    let scratch = b.guest_alloc(VIEW_STRIDE, runtime::guest::Permissions::RW)?;
    b.raw()
        .before(&points::RENDER_VIEWS, "draw Forge monitor", move |call| {
            if lock().active {
                draw_world_view(call, scratch, |guest, views| {
                    let mut state = lock();
                    if !state.active {
                        return None;
                    }
                    let retail =
                        views
                            .first()
                            .and_then(|&v| retail_view(guest, v))
                            .map(|(p, _)| Camera {
                                position: p.position,
                                yaw: p.yaw,
                                pitch: p.pitch,
                            });
                    state.eye = retail;
                    if !state.monitor {
                        return None;
                    }
                    let c = state.camera.or(retail)?;
                    Some((
                        Pose {
                            position: c.position,
                            yaw: c.yaw,
                            pitch: c.pitch,
                        },
                        None,
                    ))
                });
            }
            Ok(Flow::Continue)
        })
        .first()
        .context_only();
    b.raw()
        .before(
            &points::OBJECT_RENDER_SETUP,
            "hide parked Forge biped",
            |call| {
                let hidden = HIDDEN.load(Ordering::Relaxed);
                if hidden == u32::MAX || call.cpu.esi != vis::MAIN_VIS || !lock().monitor {
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
    use engine::{
        testing::FakeGuest,
        view::{fake, players},
    };
    use guest_layout::{ObjectHeader, player::Player};

    #[test]
    fn a_park_restores_only_the_bit_it_added_to_the_same_biped() {
        let mut m = FakeGuest::new();
        let (player_pool, object_pool, body) = (0x3000, 0x5000, 0x7000);
        let players = fake::datums::<players::Player>(&mut m, player_pool, 1);
        let objects = fake::datums::<objects::Header>(&mut m, object_pool, 1);
        let (_, player) = fake::push(&mut m, &players);
        let (unit, object) = fake::push(&mut m, &objects);
        m.put_u32(points::PLAYERS.base, player_pool)
            .put_u32(points::OBJECTS.base, object_pool)
            .put_u32(points::SCENARIO.base, 0x9000)
            .put_u16(player.addr + Player::LOCAL_USER.offset, 0)
            .put_u16(player.addr + Player::FLAGS.offset, 0)
            .put_u32(player.addr + Player::UNIT.offset, unit.raw())
            .put_u8(object.addr + ObjectHeader::KIND.offset, objects::BIPED)
            .put_u32(object.addr + ObjectHeader::BODY.offset, body)
            .put_u16(body + BODY_DAMAGE_FLAGS, 1);
        forget_park();
        assert_eq!(parked(), None);
        park(&mut m, true);
        assert_eq!(HIDDEN.load(Ordering::Relaxed), unit.raw());
        assert_eq!(parked(), Some(unit.raw()));
        assert_eq!(
            m.u16(body + BODY_DAMAGE_FLAGS).unwrap(),
            1 | CANNOT_TAKE_DAMAGE
        );
        park(&mut m, false);
        assert_eq!(HIDDEN.load(Ordering::Relaxed), u32::MAX);
        assert_eq!(m.u16(body + BODY_DAMAGE_FLAGS).unwrap(), 1);

        m.put_u16(body + BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE);
        park(&mut m, true);
        park(&mut m, false);
        assert_eq!(m.u16(body + BODY_DAMAGE_FLAGS).unwrap(), CANNOT_TAKE_DAMAGE);

        m.put_u16(body + BODY_DAMAGE_FLAGS, 0);
        park(&mut m, true);
        m.put_u16(object.addr + ObjectHeader::SALT.offset, 0x751);
        park(&mut m, false);
        assert_eq!(m.u16(body + BODY_DAMAGE_FLAGS).unwrap(), CANNOT_TAKE_DAMAGE);
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
