//! Film-only observations at the retail fire, damage and deletion boundaries.
//! Sampling the object table at 15 Hz misses bullets born and removed in a
//! single game tick. The salted object is still resolvable on DELETE entry.

use std::sync::Arc;

use guest_layout::{DatumHandle, GuestMemory, MemFault, points, view::{Record as _, objects}};
use live_proto::film::events::{
    Event, Record,
    ballistics::{Barrel, Endpoint, Impact},
};

use crate::{Telemetry, layout, memory};

fn datum(raw: u32) -> Option<u32> {
    DatumHandle::from_raw(raw).map(DatumHandle::raw)
}

fn finite(v: [f32; 3]) -> Option<[f32; 3]> {
    v.iter().all(|x| x.is_finite()).then_some(v)
}

fn direction(v: [f32; 3]) -> Option<[f32; 3]> {
    finite(v).filter(|v| v.iter().any(|x| *x != 0.0))
}

fn fired(m: &dyn GuestMemory, weapon: u32, index: u32) -> Result<Event, MemFault> {
    let mut shot = Barrel {
        weapon,
        definition: None,
        index,
        unit: None,
        parent: None,
        origin: None,
        aim: None,
    };
    let mut player = None;
    if let Some((objects::WEAPON, body)) = objects::get(m, DatumHandle(weapon))? {
        shot.definition = body.definition(m)?.map(DatumHandle::raw);
        let holder = body.parent(m)?;
        shot.unit = holder.map(DatumHandle::raw);
        if let Some(holder) = holder
            && let Some((objects::BIPED | objects::VEHICLE, unit)) = objects::get(m, holder)?
        {
            shot.origin = finite(unit.position(m)?);
            shot.parent = unit.parent(m)?.map(DatumHandle::raw);
            shot.aim = direction(m.vec3(unit.addr().wrapping_add(layout::UNIT_AIM))?);
            player = DatumHandle::from_raw(m.u32(unit.addr().wrapping_add(layout::UNIT_PLAYER))?)
                .filter(|p| p.index() < 16);
        }
    }
    let unit = player.map_or_else(
        || format!("{:08x}", shot.unit.unwrap_or(weapon)),
        |p| format!("p{}", p.index()),
    );
    Ok(Event::WeaponFire {
        unit,
        weapon: None,
        count: 1,
        barrel: Some(shot),
    })
}

fn impact(m: &dyn GuestMemory, report: u32, target: u32) -> Result<Option<Event>, MemFault> {
    // DAMAGE_REPORT's source geometry, docs/live/live-gamestate.md §7.
    let Some(origin) = finite(m.vec3(report.wrapping_add(0x24))?) else {
        return Ok(None);
    };
    let distance = m.f32(report.wrapping_add(0x4c))?;
    Ok(Some(Event::Impact(Impact {
        target,
        source: datum(m.u32(report.wrapping_add(layout::REPORT_OWNER_OBJECT))?),
        effect: datum(m.u32(report.wrapping_add(layout::REPORT_EFFECT))?),
        code: m.u8(report.wrapping_add(layout::REPORT_CODE))?,
        origin,
        direction: direction(m.vec3(report.wrapping_add(0x18))?),
        distance: (distance.is_finite() && distance >= 0.0).then_some(distance),
    })))
}

fn ended(m: &dyn GuestMemory, object: u32) -> Result<Option<Event>, MemFault> {
    let Some((layout::OBJECT_TYPE_PROJECTILE, body)) = objects::get(m, DatumHandle(object))? else {
        return Ok(None);
    };
    let Some(position) = finite(body.position(m)?) else {
        return Ok(None);
    };
    Ok(Some(Event::ProjectileEnded(Endpoint {
        projectile: object,
        definition: body.definition(m)?.map(DatumHandle::raw),
        parent: body.parent(m)?.map(DatumHandle::raw),
        position,
        velocity: finite(m.vec3(body.addr().wrapping_add(layout::BODY_VELOCITY))?),
    })))
}

pub(crate) fn observe(
    t: &Telemetry,
    source: &'static str,
    o: &::hooks::Obs<'_>,
    read: impl FnOnce(&runtime::Guest) -> Result<Option<Event>, MemFault>,
) {
    if !t.film_observing() {
        return;
    }
    t.protect(|| {
        let record = {
            let g = o.guest();
            match read(&g) {
                Ok(None) => return Ok(()),
                Ok(Some(event)) => memory::game_tick(&g)
                    .ok()
                    .map(|tick| Record { tick, event }),
                Err(_) => None,
            }
        };
        if let Some(record) = record {
            t.film_event(record)?;
        } else {
            t.dropped_film_sample(source);
        }
        Ok(())
    });
}

pub(crate) fn install(b: &mut ::hooks::Builder, telemetry: Arc<Telemetry>) {
    if telemetry.hooks.weapon_fire {
        let t = telemetry.clone();
        b.raw()
            .observe_before(&points::WEAPON_FIRE, "film barrel fire", move |o| {
                if let (Some(weapon), Some(index)) = (o.arg_u32("weapon"), o.arg_u32("barrel")) {
                    observe(&t, "weapon_fire", o, |g| fired(g, weapon, index).map(Some));
                } else {
                    t.dropped_film_sample("weapon_fire");
                }
            })
            .optional();
    }
    if telemetry.hooks.damage_report {
        let t = telemetry.clone();
        b.raw()
            .observe_before(&points::DAMAGE_REPORT, "film impact geometry", move |o| {
                if let (Some(report), Some(victim)) = (o.arg_u32("report"), o.arg_u32("victim")) {
                    observe(&t, "impact", o, |g| impact(g, report, victim));
                } else {
                    t.dropped_film_sample("impact");
                }
            })
            .optional();
    }
    if telemetry.hooks.object_delete {
        b.raw()
            .observe_before(&points::OBJECT_DELETE, "film projectile end", move |o| {
                if let Some(object) = o.arg_u32("object") {
                    observe(&telemetry, "projectile_end", o, |g| ended(g, object));
                } else {
                    telemetry.dropped_film_sample("projectile_end");
                }
            })
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::fixture::*;

    #[test]
    fn a_bullet_between_samples_keeps_its_final_point_and_salt() {
        let mut g = memory();
        put_placed(
            &mut g,
            2,
            layout::OBJECT_TYPE_PROJECTILE,
            -1,
            -1,
            [6., -3., 1.],
            u32::MAX,
        );
        let projectile = object_datum(2);
        g.set_u32(body(2), 0xc0120034).unwrap();
        g.set_f32(body(2) + layout::BODY_VELOCITY, 400.0).unwrap();
        let Some(Event::ProjectileEnded(event)) = ended(&g, projectile).unwrap() else {
            panic!()
        };
        assert_eq!(event.position, [6., -3., 1.]);
        assert_eq!(event.velocity, Some([400., 0., 0.]));
        assert_eq!(event.definition, Some(0xc0120034));
        assert_eq!(event.parent, None);
        assert!(ended(&g, projectile ^ (1 << 16)).unwrap().is_none());
        put_placed(
            &mut g,
            2,
            layout::OBJECT_TYPE_EQUIPMENT,
            -1,
            -1,
            [1.; 3],
            u32::MAX,
        );
        assert!(ended(&g, projectile).unwrap().is_none());
    }

    #[test]
    fn fire_uses_the_barrel_and_salted_holder_even_for_a_secondary_weapon() {
        let mut g = memory();
        put_placed(
            &mut g,
            0,
            layout::OBJECT_TYPE_BIPED,
            -1,
            -1,
            [3., 4., 5.],
            u32::MAX,
        );
        put_unit(&mut g, 0, 0x80400001, [1., 0., 0.], 0);
        put_weapon(&mut g, 2, 0, 1, 0x81020001);
        g.set_u32(body(0) + layout::UNIT_PLAYER, 0x81000003)
            .unwrap();
        let Event::WeaponFire {
            unit,
            barrel: Some(shot),
            count,
            ..
        } = fired(&g, object_datum(2), 1).unwrap()
        else {
            panic!()
        };
        assert_eq!(unit, "p3");
        assert_eq!(count, 1);
        assert_eq!(shot.definition, Some(0x81020001));
        assert_eq!(shot.unit, Some(object_datum(0)));
        assert_eq!(shot.index, 1);
        assert_eq!(shot.origin, Some([3., 4., 5.]));
        assert_eq!(shot.aim, Some([1., 0., 0.]));
    }

    #[test]
    fn damage_carries_the_report_geometry_even_without_a_victim_body() {
        let mut g = memory();
        let r = GLOBALS + 0x1000;
        g.set_f32(r + 0x24, 42.5).unwrap();
        g.set_f32(r + 0x20, -1.0).unwrap();
        g.set_u32(r + layout::REPORT_EFFECT, 0xc0100040).unwrap();
        g.set_u32(r + layout::REPORT_OWNER_OBJECT, u32::MAX)
            .unwrap();
        let Some(Event::Impact(event)) = impact(&g, r, u32::MAX).unwrap() else {
            panic!()
        };
        assert_eq!(event.origin, [42.5, 0., 0.]);
        assert_eq!(event.direction, Some([0., 0., -1.]));
        assert_eq!(event.effect, Some(0xc0100040));
        assert_eq!(event.source, None);
        g.set_f32(r + 0x24, f32::NAN).unwrap();
        assert!(impact(&g, r, 1).unwrap().is_none());
    }
}
