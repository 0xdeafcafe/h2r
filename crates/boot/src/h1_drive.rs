//! Connect Halo 1's per-tick ground drive to the native wheel solver.
//! All persistent state lives in the vehicle body and travels with it in
//! a snapshot. The converter opts in individual vehicle definitions.

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points::{
    GAME_TIME, VEHICLE_CONTACT_SOLVE, VEHICLE_TREAD_UPDATE, VEHICLE_WHEEL_UPDATE,
};
use engine::tags::GroupTag;
use engine::view::{Tags, objects};
use feature::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::{Access, FaultReason};
use tags::object::direct_drive::{H1_DIRECT_DRIVE, H1_SECOND_TRACK, Limits, tread_inputs};

pub struct DirectDrive;

fn vehicle(m: &dyn GuestMemory, object: u32) -> Result<Option<(u32, u32)>, MemFault> {
    let Some(id) = DatumHandle::from_raw(object) else {
        return Ok(None);
    };
    let Some((objects::VEHICLE, body)) = objects::get(m, id)? else {
        return Ok(None);
    };
    let Some(tag) = body.definition(m)? else {
        return Ok(None);
    };
    let Some(entry) = Tags.entry(m, tag)? else {
        return Ok(None);
    };
    if entry.group(m)? != GroupTag::from_chars(*b"vehi") {
        return Ok(None);
    }
    let def = entry.address(m)?;
    if def == 0 {
        return Ok(None);
    }
    Ok((m.u32(def + 0x260)? == H1_DIRECT_DRIVE
        && m.u32(def + 0x264)? == 0
        && m.u32(def + 0x268)? == 2)
        .then_some((body.addr, def)))
}

fn heading_error(forward: [f32; 3], up: [f32; 3], look: [f32; 3]) -> f32 {
    let [x, y, z] = up;
    let [a, b, c] = forward;
    let left = [y * c - z * b, z * a - x * c, x * b - y * a];
    let dot = |v: [f32; 3]| v[0] * look[0] + v[1] * look[1] + v[2] * look[2];
    dot(left).atan2(dot(forward))
}

fn update(m: &mut dyn GuestMemory, object: u32, tank: bool) -> Result<(), MemFault> {
    let Some((body, def)) = vehicle(m, object)? else {
        return Ok(());
    };
    let limits = Limits {
        forward: m.f32(def + 0x1f4)?,
        reverse: m.f32(def + 0x1f8)?,
        acceleration: m.f32(def + 0x1fc)?,
        braking: m.f32(def + 0x200)?,
    };
    let dt = m.f32(m.u32(GAME_TIME.base)? + 4)?;
    let old_speed = m.f32(body + 0x378)?;
    let old_aux = m.f32(body + 0x380)?;
    let previous = if tank {
        (old_speed.abs() - old_aux.abs())
            .max(0.0)
            .copysign(old_speed)
    } else if m.u8(body + 0x38b)? == 0 {
        -old_speed
    } else {
        old_speed
    };
    let stick = m.f32(body + 0x1b0)?;
    let braking =
        m.u8(body + 0x148)? & 2 != 0 || (m.u32(def + 0x1ec)? & 0x10 != 0 && previous * stick < 0.0);
    let target = if braking { 0.0 } else { stick * 30.0 };
    let forward = limits.advance(previous, target, dt);
    m.set_f32(body + 0x354, forward)?;
    let yaw = heading_error(
        m.vec3(body + 0x70)?,
        m.vec3(body + 0x7c)?,
        m.vec3(body + 0x150)?,
    );
    if tank {
        let turn = limits.tread_turn(old_aux, forward, yaw, dt);
        let (total, fraction) = tread_inputs(forward, turn);
        m.set_f32(body + 0x378, total)?;
        m.set_f32(body + 0x380, turn)?;
        m.set_f32(body + 0x358, fraction * limits.forward.max(0.001))?;
        m.set_u8(body + 0x38b, 1)?;
    } else {
        m.set_f32(body + 0x378, forward.abs())?;
        // H1 0x397ec..0x39876: reversing changes the sign of the target
        // angle, then each wheel slews toward it at the authored turn rate.
        let max = m.f32(def + 0x204)?.to_radians();
        let min = m.f32(def + 0x208)?.to_radians();
        let step = m.f32(def + 0x210)?.to_radians() * dt;
        if min.is_finite() && max.is_finite() && min <= max && step.is_finite() && step >= 0.0 {
            let target = (if forward < 0.0 { -yaw } else { yaw }).clamp(min, max);
            let steer = old_aux + (target - old_aux).clamp(-step, step);
            m.set_f32(body + 0x380, steer)?;
            m.set_f32(body + 0x35c, steer)?;
        }
        m.set_u8(body + 0x38b, u8::from(forward >= 0.0))?;
    }
    Ok(())
}

fn drive_contacts(m: &mut dyn GuestMemory, object: u32, contacts: u32) -> Result<(), MemFault> {
    let Some((body, def)) = vehicle(m, object)? else {
        return Ok(());
    };
    if m.u32(contacts + 4)? != def + 0x2ac {
        return Ok(());
    }
    let forward = m.f32(body + 0x354)?;
    let points = m.u32(def + 0x2f4)?;
    let count = m.u32(def + 0x2f0)?;
    // The native solver allocates sixteen contact entries.
    if count > 16 {
        return Ok(());
    }
    match m.u16(def + 0x1f0)? {
        0 => {
            let turn = m.f32(body + 0x380)?;
            write_tracks(m, points, contacts, count, forward, turn)
        }
        1 => {
            // H2 0xf2005..0xf2027 locks the wheel instantly and latches it
            // for half a second. H1 0x3979a ramps both groups toward rest.
            for i in 0..count {
                m.set_u8(contacts + 0xb93 + i * 0xd8, 0)?;
                m.set_f32(contacts + 0xb9c + i * 0xd8, forward)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn write_tracks(
    m: &mut dyn GuestMemory,
    points: u32,
    contacts: u32,
    count: u32,
    forward: f32,
    turn: f32,
) -> Result<(), MemFault> {
    // 0xf1e70 alternates left/right by array index. The H1 tag assigns
    // every contact to one of the two powered groups independently.
    for i in 0..count {
        let second = m.u32(points + i * 0x4c + 4)? & H1_SECOND_TRACK != 0;
        let speed = if second {
            forward + turn
        } else {
            forward - turn
        };
        m.set_f32(contacts + 0xb9c + i * 0xd8, speed)?;
    }
    Ok(())
}

fn fault(f: MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

impl Feature for DirectDrive {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-drive", "Halo 1 ground vehicle drive").doc("docs/halo1/role-audit.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        for (point, tank) in [
            (&VEHICLE_TREAD_UPDATE, true),
            (&VEHICLE_WHEEL_UPDATE, false),
        ] {
            b.raw()
                .before(point, "set the source wheel speed", move |c| {
                    if let Some(id) = c.arg_u32("object") {
                        update(&mut *c.ctx.memory(), id, tank).map_err(fault)?;
                    }
                    Ok(Flow::Continue)
                })
                .context_only();
        }
        b.raw()
            .before(
                &VEHICLE_CONTACT_SOLVE,
                "apply the source contact speeds",
                |c| {
                    if let (Some(id), Some(contacts)) = (c.arg_u32("object"), c.arg_u32("contacts"))
                    {
                        drive_contacts(&mut *c.ctx.memory(), id, contacts).map_err(fault)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn wheel_update_drives_only_opted_in_definitions() {
        use engine::datum::layout;
        use engine::points::{OBJECTS, TAG_ENTRIES, TAG_INDEX};
        let mut m = FlatMemory::new(0x4e0000, 0x70000);
        let (array, header, body) = (0x500000, 0x500100, 0x500200);
        let (index, entry, def, time) = (0x500700, 0x500800, 0x500900, 0x500d00);
        let (object_id, tag_id) = (0x1234_0000, 0x5678_0000);
        for (at, value) in [
            (OBJECTS.base, array),
            (array + layout::HIGH_WATER, 1),
            (array + layout::ELEMENT_SIZE, 12),
            (array + layout::DATA, header),
            (header + 8, body),
            (body, tag_id),
            (TAG_INDEX.base, index),
            (index + 0x18, 1),
            (TAG_ENTRIES.base, entry),
            (entry, GroupTag::from_chars(*b"vehi").0),
            (entry + 4, tag_id),
            (entry + 8, def),
            (GAME_TIME.base, time),
            (def + 0x268, 2),
        ] {
            m.set_u32(at, value).unwrap();
        }
        m.set_u16(def + 0x1f0, 1).unwrap();
        m.set_u16(header, 0x1234).unwrap();
        m.set_u8(header + 3, objects::VEHICLE).unwrap();
        m.set_f32(time + 4, 1.0 / 30.0).unwrap();
        for (at, value) in [(0x1f4, 7.65), (0x1f8, 3.0), (0x1fc, 2.475), (0x200, 9.9)] {
            m.set_f32(def + at, value).unwrap();
        }
        m.set_f32(def + 0x204, 30.0).unwrap();
        m.set_f32(def + 0x208, -30.0).unwrap();
        m.set_f32(def + 0x210, 180.0).unwrap();
        m.set_f32(body + 0x70, 1.0).unwrap();
        m.set_f32(body + 0x84, 1.0).unwrap();
        m.set_f32(body + 0x154, 1.0).unwrap();
        m.set_f32(body + 0x1b0, -1.0).unwrap();
        update(&mut m, object_id, false).unwrap();
        assert_eq!(m.f32(body + 0x354).unwrap(), 0.0);
        m.set_u32(def + 0x260, H1_DIRECT_DRIVE).unwrap();
        update(&mut m, object_id, false).unwrap();
        assert!((m.f32(body + 0x354).unwrap() + 0.0825).abs() < 1e-6);
        assert_eq!(m.u8(body + 0x38b).unwrap(), 0);
        assert!((m.f32(body + 0x35c).unwrap().to_degrees() + 6.0).abs() < 1e-5);
        m.set_f32(body + 0x1b0, 1.0).unwrap();
        m.set_u32(def + 0x1ec, 0x10).unwrap();
        update(&mut m, object_id, false).unwrap();
        assert_eq!(m.f32(body + 0x354).unwrap(), 0.0);
        update(&mut m, object_id, false).unwrap();
        assert!((m.f32(body + 0x378).unwrap() - 0.0825).abs() < 1e-6);
        assert_eq!(m.u8(body + 0x38b).unwrap(), 1);
        let contacts = 0x502000;
        m.set_u32(contacts + 4, def + 0x2ac).unwrap();
        m.set_u32(def + 0x2f0, 4).unwrap();
        m.set_u8(contacts + 0xb93, 1).unwrap();
        drive_contacts(&mut m, object_id, contacts).unwrap();
        assert_eq!(m.u8(contacts + 0xb93).unwrap(), 0);
        for i in 0..4 {
            assert_eq!(
                m.f32(contacts + 0xb9c + i * 0xd8).unwrap(),
                m.f32(body + 0x354).unwrap()
            );
        }
    }

    #[test]
    fn contact_speed_uses_powered_groups_in_any_order() {
        let mut mem = FlatMemory::new(0x1000, 0x2400);
        let (points, contacts) = (0x1000, 0x1800);
        for i in [0, 1, 4] {
            mem.set_u32(points + i * 0x4c + 4, H1_SECOND_TRACK | 2)
                .unwrap();
        }
        write_tracks(&mut mem, points, contacts, 5, -2.5, 1.0).unwrap();
        for (i, expected) in [-1.5, -1.5, -3.5, -3.5, -1.5].into_iter().enumerate() {
            assert_eq!(
                mem.f32(contacts + 0xb9c + i as u32 * 0xd8).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn heading_is_signed_about_the_vehicles_own_up_axis() {
        let forward = [0.0, 1.0, 0.0];
        let up = [1.0, 0.0, 0.0];
        assert_eq!(heading_error(forward, up, forward), 0.0);
        for sign in [-1.0, 1.0] {
            assert_eq!(
                heading_error(forward, up, [0.0, 0.0, sign]),
                sign * std::f32::consts::FRAC_PI_2
            );
        }
    }
}
