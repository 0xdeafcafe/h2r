//! Objects in motion on the host (docs/game/rules-world-api.md,
//! "Objects"): `o.position` and `o.velocity`, `teleport`, and the steps of
//! a mover.
//!
//! A script's coordinates are 16.16 world units; the object datum's are
//! `f32` world units ([`to_fixed`], [`from_fixed`]). A read ([`read`]) is
//! of the root ancestor's body, as an attached object's own place is
//! parent-relative. A put ([`put`]) is the engine's `OBJECT_SET_TRANSFORM`
//! (`0xb7430`) with the object's own facing kept, which answers false for a
//! position outside the world: that is a refused call, recorded with the
//! tick's other refusals (`rules.state`).
//!
//! The VM holds the movers (`rules::vm::RulesExt::movers`) and tells the
//! game each tick's step through `Game::step_object`; the host makes it as
//! a put at the next tick boundary, like a `teleport`.
//!
//! **Not built**, for the research the page names. Whether
//! `OBJECT_SET_TRANSFORM` marks the position flag for a host-moved object
//! (R2) and whether a host-made or host-moved object of each group has an
//! entity (R3) decide whether a client sees the move at all; the put
//! passes `notify` as the engine's own callers do, and the velocity a step
//! carries is not pushed (`OBJECT_SET_VELOCITY` needs two vectors in guest
//! memory, and what marks the entity is part of R2). How a client's own
//! predicted biped takes a host-set position (R9) is why a player's unit is
//! not moved: the checker refuses `teleport` and `move_to` of a player.

use ::rules::caps::id;
use ::rules::vm::{Arg, Call as VmCall};
use guest_layout::entry::{Cx, GuestCall};
use guest_layout::view::objects;
use guest_layout::{DatumHandle, GuestMemory, points};
use guest_layout::object::{Object, ObjectHeader, header::DELETE};

/// How far up an attachment chain a read looks for the root.
const MAX_DEPTH: usize = 8;

/// A 16.16 value for a world coordinate; a coordinate that is not finite
/// reads 0, and one outside the range saturates.
pub fn to_fixed(v: f32) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    (f64::from(v) * 65536.0)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// The world coordinate of a 16.16 value.
pub fn from_fixed(v: i32) -> f32 {
    (f64::from(v) / 65536.0) as f32
}

/// The body of the root ancestor of the object `handle` names: the object's
/// own when it has no parent.
fn root(m: &dyn GuestMemory, handle: DatumHandle) -> Option<(DatumHandle, objects::Body)> {
    let mut at = handle;
    for _ in 0..MAX_DEPTH {
        let (_, body) = objects::get(m, at).ok()??;
        match body.parent(m).ok()? {
            Some(parent) => at = parent,
            None => return Some((at, body)),
        }
    }
    None
}

/// `object_pos` and `object_vel`: one axis of the root ancestor's position
/// or velocity, as 16.16. An object that is `none`, gone or not readable
/// reads 0, and so does an axis past z.
pub fn read(m: &dyn GuestMemory, call: &VmCall) -> i32 {
    let (Some(Arg::Object(Some(object))), Some(Arg::Int(axis))) = (call.args.first(), call.args.get(1)) else {
        return 0;
    };
    let Some(axis) = usize::try_from(*axis).ok().filter(|&a| a < 3) else {
        return 0;
    };
    let Some((_, body)) = DatumHandle::from_raw(*object).and_then(|h| root(m, h)) else {
        return 0;
    };
    let at = if call.entry.id == id::OBJECT_VEL {
        body.addr.wrapping_add(Object::VELOCITY.offset)
    } else {
        body.addr.wrapping_add(Object::POSITION.offset)
    };
    m.vec3(at).map_or(0, |v| to_fixed(v[axis]))
}

/// Finds the unique live object of this type at a scenario placement.
pub(super) fn placed_datum(m: &dyn GuestMemory, kind: u8, index: i16) -> Option<u32> {
    let array = objects::array(m).ok()??;
    let mut found = None;
    for row in array.live_iter(m, i32::from(u16::MAX) + 1).ok()? {
        let (handle, header) = row.ok()?;
        let h = ObjectHeader::at(header.addr);
        if h.kind(m).ok()? != kind || h.flags(m).ok()? & DELETE != 0 {
            continue;
        }
        let body = h.body(m).ok()?;
        if body != 0 && Object::at(body).placement_index(m).ok()? == index {
            if found.is_some() {
                return None;
            }
            found = Some(handle.raw());
        }
    }
    found
}

/// The root object's position and datum to ignore in a sight trace.
pub(super) fn sight_endpoint(m: &dyn GuestMemory, object: u32) -> Option<([f32; 3], u32)> {
    let (root, body) = root(m, DatumHandle::from_raw(object)?)?;
    let at = body.position(m).ok()?;
    at.iter().all(|v| v.is_finite()).then_some((at, root.raw()))
}

/// One object put at a place, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub object: u32,
    pub at: [f32; 3],
}

impl Place {
    /// A place from a script's 16.16 coordinates.
    pub fn of_fixed(object: u32, at: [i32; 3]) -> Place {
        Place {
            object,
            at: at.map(from_fixed),
        }
    }
}

/// The put a `teleport` call asks: `None` for a call through `none`, which
/// does nothing.
pub fn plan(call: &VmCall) -> Option<Place> {
    match call.args[..] {
        [Arg::Object(Some(object)), Arg::Fixed(x), Arg::Fixed(y), Arg::Fixed(z)] if call.entry.id == id::TELEPORT => {
            Some(Place::of_fixed(object, [x, y, z]))
        }
        _ => None,
    }
}

/// The object's forward and up, which a put keeps.
fn facing(m: &dyn GuestMemory, object: u32) -> Option<([f32; 3], [f32; 3])> {
    let (_, body) = objects::get(m, DatumHandle::from_raw(object)?).ok()??;
    Some((body.forward(m).ok()?, body.up(m).ok()?))
}

/// Puts the object at the place with its facing kept. The engine answers
/// false for a position outside the world, which is the error.
pub async fn put(cx: &Cx, place: Place) -> anyhow::Result<()> {
    let Some((forward, up)) = cx.read(|m| facing(m, place.object)) else {
        anyhow::bail!("object {:#010x} is not in the world", place.object);
    };
    // The pose rides the stack under the call's own nine words, as the AI
    // control's teleports pass theirs (`h1_ai_control::actors`).
    let base = cx.entry().esp.wrapping_sub(36);
    let args = [place.object, base, base.wrapping_add(12), base.wrapping_add(24), 0, 1, 1, 0, 0]
        .into_iter()
        .chain([place.at, forward, up].into_iter().flatten().map(f32::to_bits));
    let result = cx
        .call(GuestCall::new(points::OBJECT_SET_TRANSFORM.base).args(args))
        .await
        .map_err(guest)?;
    anyhow::ensure!(
        result.eax as u8 != 0,
        "{:?} is outside the world, and the engine would not put object {:#010x} there",
        place.at,
        place.object
    );
    Ok(())
}

fn guest(e: impl Into<guest_layout::entry::EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::caps::ENTRIES;
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::fake;
    use guest_layout::view::objects::Header;

    const OBJECTS: u32 = 0x1007_0000;
    const BODIES: u32 = 0x1008_0000;

    fn call(entry: u16, args: Vec<Arg>) -> VmCall {
        VmCall {
            entry: &ENTRIES[entry as usize],
            args,
        }
    }

    /// Two objects: the second attached to the first. Returns the memory
    /// and their datums.
    fn world() -> (FakeGuest, [u32; 2]) {
        let mut m = FakeGuest::new();
        m.put_u32(points::OBJECTS.base, OBJECTS);
        let array = fake::datums::<Header>(&mut m, OBJECTS, 4);
        let mut out = [0; 2];
        for (i, slot) in out.iter_mut().enumerate() {
            let (handle, header) = fake::push(&mut m, &array);
            let at = BODIES + i as u32 * 0x400;
            m.map(at, 0x400);
            header.set_body(&mut m, at).unwrap();
            *slot = handle.raw();
        }
        let body = |i: u32| objects::Body::at(BODIES + i * 0x400);
        for i in 0..2 {
            body(i).set_parent(&mut m, None).unwrap();
        }
        body(0).set_position(&mut m, [1.5, -2.0, 3.0]).unwrap();
        body(0).set_forward(&mut m, [0.0, 1.0, 0.0]).unwrap();
        body(0).set_up(&mut m, [0.0, 0.0, 1.0]).unwrap();
        m.put_f32(BODIES + Object::VELOCITY.offset, 4.0)
            .put_f32(BODIES + Object::VELOCITY.offset + 4, 0.0)
            .put_f32(BODIES + Object::VELOCITY.offset + 8, -0.5);
        body(1).set_position(&mut m, [100.0, 100.0, 100.0]).unwrap();
        (m, out)
    }

    #[test]
    fn coordinates_convert_to_16_16_and_back_without_drift() {
        assert_eq!(to_fixed(1.5), 98_304);
        assert_eq!(to_fixed(-2.0), -131_072);
        assert_eq!(to_fixed(f32::NAN), 0);
        assert_eq!(to_fixed(f32::INFINITY), 0);
        assert_eq!(to_fixed(1.0e9), i32::MAX);
        assert_eq!(from_fixed(98_304), 1.5);
        // An f32 holds every whole number to 2^24 exactly.
        for v in [0, 1, -1, 65_536, -98_304, 1 << 24, -(1 << 24)] {
            assert_eq!(to_fixed(from_fixed(v)), v, "{v}");
        }
    }

    #[test]
    fn a_read_gives_one_axis_of_the_position_or_velocity() {
        let (m, [a, _]) = world();
        let pos = |axis| read(&m, &call(id::OBJECT_POS, vec![Arg::Object(Some(a)), Arg::Int(axis)]));
        let vel = |axis| read(&m, &call(id::OBJECT_VEL, vec![Arg::Object(Some(a)), Arg::Int(axis)]));
        assert_eq!([pos(0), pos(1), pos(2)], [98_304, -131_072, 196_608]);
        assert_eq!([vel(0), vel(1), vel(2)], [262_144, 0, -32_768]);
        assert_eq!((pos(3), pos(-1)), (0, 0), "an axis past z reads 0");
        assert_eq!(read(&m, &call(id::OBJECT_POS, vec![Arg::Object(None), Arg::Int(0)])), 0);
        assert_eq!(read(&m, &call(id::OBJECT_POS, vec![Arg::Object(Some(0x8000_0777)), Arg::Int(0)])), 0);
    }

    #[test]
    fn a_child_reads_its_root_ancestors_place() {
        let (mut m, [a, b]) = world();
        let child = objects::Body::at(BODIES + 0x400);
        child.set_parent(&mut m, DatumHandle::from_raw(a)).unwrap();
        let x = read(&m, &call(id::OBJECT_POS, vec![Arg::Object(Some(b)), Arg::Int(0)]));
        assert_eq!(x, 98_304, "the parent's x, not the child's own 100");
    }

    #[test]
    fn a_teleport_call_plans_a_put_in_world_units_and_none_plans_nothing() {
        let c = call(
            id::TELEPORT,
            vec![Arg::Object(Some(7)), Arg::Fixed(98_304), Arg::Fixed(-131_072), Arg::Fixed(0)],
        );
        assert_eq!(plan(&c), Some(Place { object: 7, at: [1.5, -2.0, 0.0] }));
        let none = call(
            id::TELEPORT,
            vec![Arg::Object(None), Arg::Fixed(0), Arg::Fixed(0), Arg::Fixed(0)],
        );
        assert_eq!(plan(&none), None);
        assert_eq!(plan(&call(id::STOP, vec![Arg::Object(Some(7))])), None);
    }

    #[test]
    fn a_put_keeps_the_objects_facing_and_an_outside_position_is_refused() {
        use guest_layout::entry::{EntryFuture, Regs, Ret, testing};
        const STACK: u32 = 0x800100;
        let outcome = |eax: u32| {
            let (m, [a, _]) = world();
            testing::run(
                &move |cx| -> EntryFuture {
                    Box::pin(async move {
                        let r = put(&cx, Place { object: a, at: [5.0, 6.0, 7.0] }).await;
                        Ok(Ret::new(u32::from(r.is_ok()), 0))
                    })
                },
                m,
                Regs { esp: STACK, ..Default::default() },
                move |call, _| {
                    assert_eq!(call.target, points::OBJECT_SET_TRANSFORM.base);
                    let pose = STACK - 36;
                    assert_eq!(call.args[..9], [a, pose, pose + 12, pose + 24, 0, 1, 1, 0, 0]);
                    let floats: Vec<_> = call.args[9..].iter().map(|&bits| f32::from_bits(bits)).collect();
                    assert_eq!(floats, [5.0, 6.0, 7.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
                    Regs { eax, ..Default::default() }
                },
            )
        };
        assert_eq!(outcome(1).result, Ok(Ret::new(1, 0)));
        assert_eq!(outcome(0).result, Ok(Ret::new(0, 0)), "al 0: outside the world");
    }
}
