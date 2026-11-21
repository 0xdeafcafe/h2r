//! AI-controlled units with distributed replication entities, read at tick end.

use guest_layout::{
    Datum, Fault, GuestMemory, Unit,
    actor::{Actor, CAPACITY},
    global,
    view::Datums,
};
use sim_objects::Objects;

/// The actor's presentation state for this tick. Handles include their salts.
/// The character and squad are map-local indices (all ones for none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorView {
    pub datum: u32,
    pub unit: u32,
    pub entity: u32,
    pub character: u16,
    pub squad: u16,
    pub team: u8,
    pub posture: u8,
    pub position: [f32; 3],
    pub control: Option<ActorControl>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorControl {
    pub forward: f32,
    pub side: f32,
    pub crouch: bool,
    pub facing_delta_radians: f32,
}

impl ActorControl {
    fn read(m: &dyn GuestMemory, unit: Unit, crouch: bool) -> Result<Self, Fault> {
        let [forward, side, _] = unit.throttle(m)?;
        let [x, y, _] = unit.object().forward(m)?;
        let [dx, dy, _] = unit.desired_facing(m)?;
        // Signed angle from the body's forward to the requested heading.
        let cross = x * dy - y * dx;
        let dot = x * dx + y * dy;
        let delta = if cross == 0.0 && dot == 0.0 {
            0.0
        } else {
            cross.atan2(dot)
        };
        Ok(Self {
            forward,
            side,
            crouch,
            facing_delta_radians: delta,
        })
    }
}

/// Visit replicated actors once in pool order. Resolve both sides of the
/// actor/unit link each tick so a deleted or reused slot cannot leak through.
/// Only read movement controls when the caller needs them.
pub fn visit_actors(
    m: &dyn GuestMemory,
    controls: bool,
    mut visit: impl FnMut(ActorView),
) -> Result<(), Fault> {
    let Some(actors) = Datums::<Actor>::from_global(m, global::ACTORS.base)? else {
        return Ok(());
    };
    let Some(objects) = Objects::read(m)? else {
        return Ok(());
    };
    for row in actors.live_iter(m, CAPACITY)? {
        let (id, actor) = row?;
        let Some(handle) = actor.unit(m)? else {
            continue;
        };
        let Some(obj) = objects.resolve(m, Datum(handle.raw()))? else {
            continue;
        };
        if obj.body.addr == 0
            || obj.flags(m)? & guest_layout::object::header::DELETE != 0
            || !obj.kind(m)?.is_some_and(|kind| kind.is_unit())
        {
            continue;
        }
        let unit = obj.body.as_unit();
        if unit.actor(m)?.raw() != id.raw() || !unit.player(m)?.is_none() {
            continue;
        }
        let entity = obj.body.entity(m)?;
        if entity == u32::MAX {
            continue;
        }
        // A passenger's position is relative to its parent, while the
        // bounds update keeps its centre in world coordinates.
        let position = if obj.body.parent(m)?.is_none() {
            obj.body.position(m)?
        } else {
            obj.body.bound_center(m)?
        };
        let crouch = actor.control_flags(m)? & 1 != 0;
        visit(ActorView {
            datum: id.raw(),
            unit: handle.raw(),
            entity,
            character: actor.character(m)?.map_or(u16::MAX, |h| h.index()),
            squad: actor.squad(m)? as u16,
            team: u8::try_from(actor.team(m)?).unwrap_or(u8::MAX),
            posture: guest_layout::actor::posture(m, actor.animation_mode(m)?, crouch)?,
            position,
            control: if controls {
                Some(ActorControl::read(m, unit, crouch)?)
            } else {
                None
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{
        Object, actor,
        testing::FakeGuest,
        view::{fake, objects::Header},
    };

    #[test]
    fn reads_only_units_owned_by_the_live_actor_and_published_as_entities() {
        let mut m = FakeGuest::new();
        let actors = fake::datums::<Actor>(&mut m, 0x1_0000, 4);
        let objects = fake::datums::<Header>(&mut m, 0x2_0000, 4);
        m.put_u32(global::ACTORS.base, actors.array.addr)
            .put_u32(global::OBJECTS.base, objects.array.addr);
        let (id, actor) = fake::push(&mut m, &actors);
        let (handle, header) = fake::push(&mut m, &objects);
        let body = Object::at(0x3_0000);
        let unit = body.as_unit();
        m.map(body.addr, Unit::SIZE);
        actor.set_unit(&mut m, Some(handle)).unwrap();
        actor
            .set_character(&mut m, Some(guest_layout::DatumHandle(0x9333_000c)))
            .unwrap();
        actor.set_squad(&mut m, 6).unwrap();
        actor.set_team(&mut m, 2).unwrap();
        header.set_body(&mut m, body.addr).unwrap();
        unit.set(&mut m, Unit::ACTOR, Datum(id.raw())).unwrap();
        unit.set(&mut m, Unit::PLAYER, Datum::NONE).unwrap();
        body.set(&mut m, Object::ENTITY, 0x9000_0041).unwrap();
        body.set(&mut m, Object::PARENT, Datum::NONE).unwrap();
        body.set(&mut m, Object::POSITION, [4.0, 2.0, -1.0])
            .unwrap();
        body.set(&mut m, Object::FORWARD, [1.0, 0.0, 0.0]).unwrap();
        unit.set(&mut m, Unit::DESIRED_FACING, [0.0, 1.0, 0.0])
            .unwrap();
        unit.set(&mut m, Unit::THROTTLE, [0.25, -0.5, 0.0]).unwrap();
        actor.set_control_flags(&mut m, 1).unwrap();

        // Invent the full string ID at the selector operand for this fixture.
        let mode = 0x0c00_0084;
        m.put_u32(0x0010_ef65, mode);
        actor.set_animation_mode(&mut m, mode).unwrap();
        let sample = |g: &FakeGuest, controls| {
            let mut rows = Vec::new();
            visit_actors(g, controls, |row| rows.push(row)).unwrap();
            rows
        };
        let row = &sample(&m, true)[0];
        assert_eq!(
            (row.datum, row.unit, row.entity),
            (id.raw(), handle.raw(), 0x9000_0041)
        );
        assert_eq!(
            (row.character, row.squad, row.team, row.posture),
            (12, 6, 2, 0x94)
        );
        let control = row.control.unwrap();
        assert_eq!(
            (control.forward, control.side, control.crouch),
            (0.25, -0.5, true)
        );
        assert!((control.facing_delta_radians - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(row.position, [4.0, 2.0, -1.0]);
        // Cower changes the pose with movement control replication off.
        let cowering = sample(&m, false)[0];
        assert_eq!(cowering.posture, 0x94);
        assert!(cowering.control.is_none());
        actor.set_control_flags(&mut m, 0).unwrap();
        let standing = sample(&m, false)[0];
        assert_eq!(standing.posture, 0x84);
        assert!(standing.control.is_none());

        actor.set_animation_mode(&mut m, mode | 0x100).unwrap();
        body.set(&mut m, Object::PARENT, Datum(0x8111_0003))
            .unwrap();
        body.set(&mut m, Object::BOUND_CENTER, [-20.0, 2.0, 10.0])
            .unwrap();
        let row = sample(&m, false)[0];
        assert_eq!(
            row.posture,
            u8::MAX,
            "colliding low byte is not the same mode"
        );
        assert_eq!(row.position, [-20.0, 2.0, 10.0]);
        assert_eq!(
            actor::animation_mode(&m, 0x84).unwrap(),
            Some((mode, false))
        );
        assert_eq!(
            actor::animation_mode(&m, u8::MAX).unwrap(),
            Some((u32::MAX, false))
        );
        assert_eq!(actor::animation_mode(&m, 0x1e).unwrap(), None);
        assert_eq!(actor::posture(&m, u32::MAX, false).unwrap(), u8::MAX);
        assert_eq!(actor::posture(&m, 0, false).unwrap(), u8::MAX);

        body.set(&mut m, Object::ENTITY, u32::MAX).unwrap();
        assert!(sample(&m, false).is_empty());
        body.set(&mut m, Object::ENTITY, 27).unwrap();
        unit.set(&mut m, Unit::PLAYER, Datum(0x8234_0000)).unwrap();
        assert!(sample(&m, false).is_empty());
        unit.set(&mut m, Unit::PLAYER, Datum::NONE).unwrap();
        unit.set(&mut m, Unit::ACTOR, Datum(id.raw() ^ 0x10000))
            .unwrap();
        assert!(sample(&m, false).is_empty());
        unit.set(&mut m, Unit::ACTOR, Datum(id.raw())).unwrap();
        header
            .set_kind(&mut m, guest_layout::ObjectType::Weapon as u8)
            .unwrap();
        assert!(sample(&m, false).is_empty());
        header
            .set_kind(&mut m, guest_layout::ObjectType::Biped as u8)
            .unwrap();
        actor
            .set_unit(
                &mut m,
                Some(guest_layout::DatumHandle(handle.raw() ^ 0x10000)),
            )
            .unwrap();
        assert!(sample(&m, false).is_empty());
    }
}
