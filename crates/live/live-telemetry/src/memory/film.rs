//! Census for the change-only film track. All gameplay classes share the
//! object header and base transform, including attached and sleeping objects.

use super::{
    ReadResult, channel_animation, fault, layout, normalised_aim, object_headers, object_rows,
    transform, vector,
};
use engine::objectives as objective;
use live_proto::film::objects::{AnimPose, HillPose, ObjectPose, ObjectiveKind, Scene};
use live_proto::game::MAX_PROJECTILE_SPEED;
use runtime::{Guest, GuestFault};
use std::collections::BTreeSet;

fn datum(raw: u32) -> Option<u32> {
    (raw != u32::MAX).then_some(raw)
}

fn unreadable(addr: u32) -> GuestFault {
    GuestFault {
        addr,
        len: 4,
        access: runtime::guest::Access::Read,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// A one-off census. Ongoing recording uses a reader to recognise long-lived,
/// high-speed projectiles across multiple observations.
pub fn film_scene(guest: &Guest) -> ReadResult<Scene> {
    FilmSceneReader::default().read(guest)
}

#[derive(Default)]
pub struct FilmSceneReader {
    headers: Vec<u8>,
    /// All projectile handles, including candidates too fast to present after
    /// only one observation. A new salt means a new candidate.
    previous_projectiles: BTreeSet<u32>,
}

impl FilmSceneReader {
    /// A readable header table identifies every live slot. A broken body is
    /// reported by its salted handle so other objects still advance. A fault
    /// on the table itself leaves both the scene and projectile history intact.
    pub fn read(&mut self, guest: &Guest) -> ReadResult<Scene> {
        object_headers(guest, &mut self.headers)?;
        let mut projectiles = BTreeSet::new();
        let mut scene = Scene::default();
        for (id, kind, body) in object_rows(&self.headers) {
            if kind == layout::OBJECT_TYPE_PROJECTILE {
                projectiles.insert(id);
            }
            match read_object(
                guest,
                id,
                kind,
                body,
                self.previous_projectiles.contains(&id),
            ) {
                Ok(Some(object)) => scene.objects.push(object),
                Ok(None) => {}
                Err(_) => scene.missing.objects.push(id),
            }
        }
        scene.objects.sort_unstable_by_key(|o| o.datum);
        if classify_objectives(guest, &mut scene).is_err() {
            // Without the game-engine selector, the previous hill and
            // objective classifications remain the last reliable observation.
            scene.missing.objectives = true;
            scene.missing.hill = true;
        }
        self.previous_projectiles = projectiles;
        Ok(scene)
    }
}

fn read_object(
    guest: &Guest,
    id: u32,
    kind: u8,
    body: u32,
    seen_projectile: bool,
) -> ReadResult<Option<ObjectPose>> {
    if body == 0
        || body
            .checked_add(layout::MANAGER_OFFSETS.end as u32 + layout::CHANNEL_FRAME + 4)
            .is_none()
    {
        return Err(unreadable(body));
    }
    let (definition, [pos, forward, up]) = transform(guest, body)?;
    if !pos.iter().chain(&forward).chain(&up).all(|n| n.is_finite()) {
        return Err(unreadable(body + layout::BODY_POSITION));
    }
    let parent = datum(guest.u32(body + layout::BODY_PARENT)?);
    // Live presents slow or attached projectiles immediately. A fast one
    // also qualifies once it has survived across film samples (a mod, for
    // example, can have a missile faster than a retail bullet).
    if kind == layout::OBJECT_TYPE_PROJECTILE && parent.is_none() && !seen_projectile {
        let vel = vector(guest, body + layout::BODY_VELOCITY)?;
        if !vel.iter().all(|v| v.is_finite()) {
            return Err(unreadable(body + layout::BODY_VELOCITY));
        }
        if vel.iter().map(|v| v * v).sum::<f64>() >= MAX_PROJECTILE_SPEED.powi(2) {
            return Ok(None);
        }
    }
    let unit = matches!(
        kind,
        layout::OBJECT_TYPE_BIPED | layout::OBJECT_TYPE_VEHICLE
    );
    let seat = if unit && parent.is_some() {
        u16::try_from(guest.u16(body + layout::UNIT_SEAT)? as i16).ok()
    } else {
        None
    };
    let player = if unit {
        datum(guest.u32(body + layout::UNIT_PLAYER)?)
    } else {
        None
    };
    let aim = if unit {
        let raw = vector(guest, body + layout::UNIT_AIM)?;
        if !raw.iter().all(|x| x.is_finite()) {
            return Err(unreadable(body + layout::UNIT_AIM));
        }
        normalised_aim(raw)
    } else {
        None
    };
    // All three device subclasses embed the common device datum. Controls
    // and light fixtures can change position without a machine channel.
    let travel = if (7..=9).contains(&kind) {
        let p = f64::from(guest.f32(body + layout::DEVICE_POSITION)?);
        if !p.is_finite() {
            return Err(unreadable(body + layout::DEVICE_POSITION));
        }
        (0.0..=1.0).contains(&p).then_some(p)
    } else {
        None
    };
    Ok(Some(ObjectPose {
        datum: id,
        kind,
        definition,
        name: None,
        placement: u16::try_from(guest.u16(body + layout::BODY_PLACEMENT_INDEX)? as i16).ok(),
        spawn: u8::try_from(guest.u8(body + layout::BODY_NETGAME_EQUIPMENT)? as i8).ok(),
        parent,
        pos,
        forward,
        up,
        seat,
        aim,
        animation: channel_animation(guest, body)?.map(|a| AnimPose {
            graph: a.graph,
            inherited: a.inherited,
            index: a.index,
            frame: a.frame,
        }),
        travel,
        depleted: guest.u16(body + layout::BODY_DAMAGE_FLAGS)? & layout::BODY_DEPLETED != 0,
        player,
        objective: None,
    }))
}

fn active_hill(guest: &Guest) -> ReadResult<Option<HillPose>> {
    let hill = guest.u32(objective::KOTH_GLOBALS)?;
    if hill == 0 {
        return Ok(None);
    }
    let field = hill
        .checked_add(objective::KOTH_HILL)
        .ok_or_else(|| unreadable(hill))?;
    if guest.u32(field)? == u32::MAX {
        return Ok(None);
    }
    let flags = objective::netgame_flags(guest).map_err(fault)?;
    let (index, corners) = objective::hill_corners(guest, hill, &flags)
        .map_err(fault)?
        .ok_or_else(|| unreadable(field))?;
    Ok(Some(HillPose {
        index,
        corners: corners.into_iter().map(|p| p.map(f64::from)).collect(),
    }))
}

fn classify_objectives(guest: &Guest, scene: &mut Scene) -> ReadResult<()> {
    let globals = guest.u32(objective::GAME_ENGINE)?;
    if globals == 0 {
        return Ok(());
    }
    let kind = match guest.u32(
        globals
            .checked_add(objective::ENGINE_INDEX)
            .ok_or_else(|| unreadable(globals))?,
    )? {
        1 => ObjectiveKind::Flag,
        3 => ObjectiveKind::Ball,
        9 => ObjectiveKind::Bomb,
        4 => {
            match active_hill(guest) {
                Ok(hill) => scene.hill = hill,
                Err(_) => scene.missing.hill = true,
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let Ok(items) = objective::registered(guest, globals) else {
        scene.missing.objectives = true;
        return Ok(());
    };
    for item in items {
        if let Ok(i) = scene
            .objects
            .binary_search_by_key(&item.object.raw(), |o| o.datum)
        {
            scene.objects[i].objective = Some(kind);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::fixture;

    #[test]
    fn census_follows_every_engine_class_and_salted_lifecycle() {
        let mut guest = fixture::memory();
        for kind in 0u8..=12 {
            fixture::put_placed(
                &mut guest,
                u32::from(kind),
                kind,
                -1,
                -1,
                [1., 2., 3.],
                u32::MAX,
            );
        }
        let vehicle = fixture::object_datum(1);
        guest
            .set_u32(fixture::body(0) + layout::BODY_PARENT, vehicle)
            .unwrap();
        guest
            .set_u16(fixture::body(0) + layout::UNIT_SEAT, 2)
            .unwrap();
        guest
            .set_f32(fixture::body(7) + layout::DEVICE_POSITION, 0.375)
            .unwrap();
        let scene = film_scene(&guest).unwrap();
        assert_eq!(
            scene.objects.iter().map(|o| o.kind).collect::<Vec<_>>(),
            (0..=12).collect::<Vec<_>>()
        );
        assert_eq!(
            (scene.objects[0].parent, scene.objects[0].seat),
            (Some(vehicle), Some(2))
        );
        assert_eq!(scene.objects[7].travel, Some(0.375));

        let header = fixture::OBJECT_DATA + 2 * layout::OBJECT_HEADER_SIZE;
        guest.set_u16(header, 0).unwrap();
        let scene = film_scene(&guest).unwrap();
        assert!(
            !scene
                .objects
                .iter()
                .any(|o| o.kind == layout::OBJECT_TYPE_WEAPON)
        );
        guest.set_u16(header, 0xb473).unwrap();
        let scene = film_scene(&guest).unwrap();
        assert!(scene.objects.iter().any(|o| o.datum == 0xb473_0002));
    }

    #[test]
    fn unavailable_body_does_not_turn_into_a_despawn() {
        let mut guest = fixture::memory();
        fixture::put_object(&mut guest, 2, u32::MAX);
        let field = fixture::OBJECT_DATA + 2 * layout::OBJECT_HEADER_SIZE + layout::OBJECT_BODY;
        for address in [0, 0x0a00_0000, u32::MAX] {
            guest.set_u32(field, address).unwrap();
            let scene = film_scene(&guest).unwrap();
            assert_eq!(scene.missing.objects, [fixture::object_datum(2)]);
        }
    }

    #[test]
    fn projectiles_use_flight_or_attachment_and_objectives_use_salted_registration() {
        let mut guest = fixture::memory();
        fixture::put_placed(&mut guest, 0, 2, -1, 3, [1., 2., 3.], u32::MAX);
        fixture::put_placed(&mut guest, 1, 5, -1, -1, [1., 2., 3.], u32::MAX);
        guest
            .set_u32(fixture::ENGINE + objective::ENGINE_INDEX, 1)
            .unwrap();
        guest
            .set_u32(fixture::ENGINE + objective::OBJECTIVE_COUNT, 1)
            .unwrap();
        let flag = fixture::object_datum(0);
        guest
            .set_u32(fixture::ENGINE + objective::OBJECTIVES, flag)
            .unwrap();
        guest
            .set_f32(fixture::body(1) + layout::BODY_VELOCITY, 400.)
            .unwrap();
        let mut reader = FilmSceneReader::default();
        let scene = reader.read(&guest).unwrap();
        assert_eq!(scene.objects.len(), 1);
        assert_eq!(reader.read(&guest).unwrap().objects.len(), 2);
        assert_eq!(scene.objects[0].objective, Some(ObjectiveKind::Flag));
        assert_eq!(scene.objects[0].spawn, Some(3));
        guest
            .set_u32(fixture::body(1) + layout::BODY_PARENT, flag)
            .unwrap();
        assert_eq!(film_scene(&guest).unwrap().objects.len(), 2);
        guest
            .set_u32(fixture::body(1) + layout::BODY_PARENT, u32::MAX)
            .unwrap();
        guest
            .set_f32(fixture::body(1) + layout::BODY_VELOCITY, 20.)
            .unwrap();
        assert_eq!(film_scene(&guest).unwrap().objects.len(), 2);
    }
    #[test]
    fn bad_transform_keeps_its_salted_slot_and_does_not_hide_other_objects() {
        let mut guest = fixture::memory();
        fixture::put_placed(&mut guest, 0, 2, -1, -1, [0.; 3], u32::MAX);
        fixture::put_placed(&mut guest, 1, 2, -1, -1, [1.; 3], u32::MAX);
        let mut census = FilmSceneReader::default();
        assert!(census.read(&guest).unwrap().missing.objects.is_empty());
        let f = fixture::body(0) + layout::BODY_FORWARD;
        guest.set_f32(f, f32::NAN).unwrap();
        let partial = census.read(&guest).unwrap();
        assert_eq!(partial.missing.objects, [fixture::object_datum(0)]);
        assert!(
            partial
                .objects
                .iter()
                .any(|o| o.datum == fixture::object_datum(1))
        );
        guest.set_f32(f, 1.).unwrap();
        let recovered = census.read(&guest).unwrap();
        assert!(recovered.missing.objects.is_empty());
        assert!(
            recovered
                .objects
                .iter()
                .any(|o| o.datum == fixture::object_datum(0))
        );
    }

    #[test]
    fn moving_hill_uses_the_active_flag_indices_in_engine_order() {
        let mut guest = fixture::memory();
        let (hill, scenario, flags) = (0x170000, 0x171000, 0x172000);
        guest
            .set_u32(fixture::ENGINE + objective::ENGINE_INDEX, 4)
            .unwrap();
        guest.set_u32(objective::KOTH_GLOBALS, hill).unwrap();
        guest
            .set_u32(guest_layout::points::SCENARIO.base, scenario)
            .unwrap();
        guest
            .set_u32(scenario + objective::NETGAME_FLAGS, 8)
            .unwrap();
        guest
            .set_u32(scenario + objective::NETGAME_FLAGS + 4, flags)
            .unwrap();
        guest.set_u32(hill + 0x60, 4).unwrap();
        for i in 0..8 {
            let entry = flags + i * objective::NETGAME_FLAG_STRIDE;
            guest
                .set_u16(
                    entry + objective::FLAG_TYPE,
                    objective::kind::HILL + (i / 4) as u16,
                )
                .unwrap();
            guest.set_f32(entry, i as f32).unwrap();
        }
        for active in 0..2 {
            guest.set_u32(hill + objective::KOTH_HILL, active).unwrap();
            for i in 0..4 {
                guest.set_u32(hill + 4 * i, 4 * active + 3 - i).unwrap();
            }
            let polygon = film_scene(&guest).unwrap().hill.unwrap();
            assert_eq!(u32::from(polygon.index), active);
            assert_eq!(polygon.corners.len(), 4);
            assert_eq!(polygon.corners[0], [f64::from(4 * active + 3), 0., 0.]);
            assert_eq!(polygon.corners[3], [f64::from(4 * active), 0., 0.]);
        }
    }
}
