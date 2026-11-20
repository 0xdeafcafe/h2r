//! Apply Halo 1 drive, contact and flight laws to converted vehicle definitions.
//! All persistent state lives in the vehicle body and travels with it in
//! a snapshot. The converter opts in individual vehicle definitions.

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points::{
    GAME_TIME, UNIT_CONTROL_REFRESH, VEHICLE_CONTACT_FORCES, VEHICLE_CONTACT_SOLVE,
    VEHICLE_GRAVITY_DELTA, VEHICLE_TREAD_UPDATE, VEHICLE_WHEEL_UPDATE,
};
use engine::tags::GroupTag;
use engine::view::{Tags, objects};
use feature::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::{Access, FaultReason};
use tags::object::direct_drive::{H1_DIRECT_DRIVE, H1_SECOND_TRACK, Limits, tread_inputs};

pub struct DirectDrive;

fn vehicle_definition(m: &dyn GuestMemory, object: u32) -> Result<Option<(u32, u32)>, MemFault> {
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
    Ok(Some((body.addr, def)))
}

fn vehicle(m: &dyn GuestMemory, object: u32) -> Result<Option<(u32, u32)>, MemFault> {
    let Some((body, def)) = vehicle_definition(m, object)? else {
        return Ok(None);
    };
    Ok((m.u32(def + 0x260)? == H1_DIRECT_DRIVE
        && m.u32(def + 0x264)? == 0
        && m.u32(def + 0x268)? == 2)
        .then_some((body, def)))
}

fn flight_forces(
    m: &mut dyn GuestMemory,
    contacts: u32,
    force: u32,
    torque: u32,
) -> Result<(), MemFault> {
    use tags::object::h1_flight::*;
    let root = m.u32(contacts + 4)?;
    if m.u32(root + 0x4c)? != 1 {
        return Ok(());
    }
    let phantom = m.u32(root + 0x50)?;
    if phantom == 0 || m.u32(phantom + 0x38)? != 0 || m.u32(phantom + DATA)? != MAGIC {
        return Ok(());
    }
    let t = phantom + DATA;
    let def = root - 0x2ac;
    let limits = Limits {
        forward: m.f32(def + 0x1f4)?,
        reverse: m.f32(def + 0x1f8)?,
        acceleration: m.f32(def + 0x1fc)?,
        braking: m.f32(def + 0x200)?,
    };
    let aircraft = Aircraft {
        kind: m.u32(t + 4)?,
        radius: m.f32(t + 8)?,
        mean_moment_per_mass: m.f32(t + 12)?,
        gun_pitch: m.f32(t + 16)?,
        bank: m.f32(t + 20)?,
        turn: m.f32(t + 24)?,
        limits,
    };
    let Some(handle) = DatumHandle::from_raw(m.u32(contacts)?) else {
        return Ok(());
    };
    let Some((objects::VEHICLE, obj)) = objects::get(m, handle)? else {
        return Ok(());
    };
    let body = obj.addr;
    let dt = m.f32(m.u32(GAME_TIME.base)? + 4)?;
    let input = m.f32(body + 0x1b0)?;
    // The wheel speed and steering state slots are disjoint from the native
    // aircraft state (+370/+374) and are part of the snapshotted vehicle.
    let old = m.f32(body + 0x37c)?;
    let flags = m.u8(body + 0x148)?;
    let brake = flags & 2 != 0 || (m.u32(def + 0x1ec)? & 0x10 != 0 && old * input < 0.0);
    let speed = limits.advance(old, if brake { 0.0 } else { input * 30.0 }, dt);
    let mut rotor = m.f32(body + 0x380)?;
    // H1 0x37c60..0x37c7e checks the vehicle itself when no driver is seated.
    let pilot = match DatumHandle::from_raw(m.u32(body + 0x248)?) {
        Some(id) => objects::get(m, id)?.map(|(_, driver)| driver.addr),
        None => Some(body),
    };
    let driver_is_player = match pilot {
        Some(unit) => m.u32(unit + crate::health::UNIT_PLAYER)? != u32::MAX,
        None => false,
    };
    let motion = Motion {
        forward: m.vec3(body + 0x70)?,
        up: m.vec3(body + 0x7c)?,
        look: m.vec3(body + 0x150)?,
        velocity: m.vec3(body + 0x88)?,
        angular: m.vec3(body + 0x94)?,
        mass: m.f32(contacts + 0x6c)?,
        gravity: m.f32(m.u32(crate::skulls_custom::PHYSICS_CONSTANTS)?)?
            * tags::object::direct_drive::GRAVITY_RATIO,
        power: m.f32(body + 0x25c)?,
        driver_is_player,
        // H2 vehicle_hover at 0xf0ffe sets the hold bit.
        scripted_hover: m.u8(body + 0x348)? & 4 != 0,
        crouch: flags & 1 != 0,
        brake,
    };
    let (linear, angular) = aircraft.forces(&motion, speed, &mut rotor, dt);
    m.set_f32(body + 0x37c, speed)?;
    m.set_f32(body + 0x380, rotor)?;
    for (at, vector) in [(force, linear), (torque, angular)] {
        for (k, value) in vector.into_iter().enumerate() {
            m.set_f32(at + k as u32 * 4, value)?;
        }
    }
    Ok(())
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

/// H2 0x206090 has populated the contact frame, relative point velocity,
/// spring force and its moment. Its caller consumes these sums at 0x205757.
fn ground_forces(
    m: &mut dyn GuestMemory,
    contacts: u32,
    force: u32,
    torque: u32,
) -> Result<bool, MemFault> {
    use tags::object::direct_drive::*;
    let root = m.u32(contacts + 4)?;
    let count = m.u32(root + 0x44)?;
    if count == 0 || count > 16 || m.u32(root + 0x4c)? != 1 {
        return Ok(false);
    }
    let phantom = m.u32(root + 0x50)?;
    if phantom == 0 || m.u32(phantom + 0x38)? > 2 || m.u32(phantom + CONTACT_DATA)? != CONTACT_MAGIC
    {
        return Ok(false);
    }
    let sizes = m.u32(phantom + CONTACT_DATA + 4)?;
    let surfaces = (sizes >> 8) & 255;
    let wheels = CONTACT_DATA + CONTACT_HEADER;
    let table = wheels + count * WHEEL_BYTES;
    if sizes & 255 != count || table + surfaces * SURFACE_BYTES > CONTACT_END {
        return Ok(false);
    }
    let mass = m.f32(contacts + 0x6c)?;
    let driven = m.u32(root - (0x2ac - 0x260))? == H1_DIRECT_DRIVE;
    let full = m.f32(root + 0x14)?;
    let cutoff = if sizes & (1 << 16) != 0 {
        full
    } else {
        m.f32(root + 0x18)?
    };
    for i in 0..count {
        let c = contacts + 0xb10 + i * 0xd8;
        if m.u8(c + 0xb3)? != 0 || m.f32(c + 0x84)? <= 0.0001 {
            continue;
        }
        let material = u32::from(m.u16(c + 0xb0)?);
        let mut scale = [1.0; 5];
        for j in 0..surfaces {
            let row = phantom + table + j * SURFACE_BYTES;
            if m.u32(row)? == material {
                for (k, v) in scale.iter_mut().enumerate() {
                    *v = m.f32(row + 4 + k as u32 * 4)?;
                }
                break;
            }
        }
        let row = phantom + wheels + i * WHEEL_BYTES;
        let wheel = WheelDrag {
            mode: m.u32(row)?,
            rate: m.f32(row + 4)? * scale[0],
            parallel: m.f32(row + 8)?,
            perpendicular: m.f32(row + 12)?,
        };
        let velocity = m.vec3(c + 0x4c)?;
        let normal = m.vec3(c + 0xa0)?;
        let mut drag = wheel.force(
            velocity,
            m.vec3(c + 4)?,
            m.vec3(c + 0x1c)?,
            normal,
            if driven { m.f32(c + 0x8c)? } else { 0.0 },
            full * scale[1],
            cutoff * scale[2],
            mass,
        );
        if scale[3] != 1.0 || scale[4] != 1.0 {
            let load = m.f32(c + 0x88)?;
            let damping = mass * m.f32(root + 0xc)? * dot(velocity, normal);
            let corrected = (load + damping) / scale[3] - damping * scale[4];
            m.set_f32(c + 0x88, corrected)?;
            for (k, f) in drag.iter_mut().enumerate() {
                *f += normal[k] * (corrected - load);
                m.set_f32(c + 0x94 + k as u32 * 4, corrected * normal[k])?;
            }
        }
        let moment = cross(m.vec3(c + 0x40)?, drag);
        for (base, delta) in [
            (force, drag),
            (torque, moment),
            (c + 0x58, drag),
            (c + 0x64, moment),
        ] {
            for (k, value) in delta.into_iter().enumerate() {
                let at = base + k as u32 * 4;
                m.set_f32(at, m.f32(at)? + value)?;
            }
        }
    }
    Ok(true)
}

fn power_record(m: &dyn GuestMemory, def: u32) -> Result<Option<u32>, MemFault> {
    use tags::object::direct_drive::{POWER_DATA, POWER_MAGIC};
    if m.u16(def)? != u16::from(objects::VEHICLE) || m.u32(def + 0x2f8)? != 1 {
        return Ok(None);
    }
    let phantom = m.u32(def + 0x2fc)?;
    if phantom == 0 || m.u32(phantom + 0x38)? > 2 || m.u32(phantom + POWER_DATA)? != POWER_MAGIC {
        return Ok(None);
    }
    Ok(Some(phantom + POWER_DATA))
}

fn gravity_delta(m: &mut dyn GuestMemory, def: u32, delta: u32) -> Result<bool, MemFault> {
    use tags::object::direct_drive::{GRAVITY_RATIO, HAS_SOURCE_GRAVITY};
    let Some(record) = power_record(m, def)? else {
        return Ok(false);
    };
    if m.u32(record + 4)? & HAS_SOURCE_GRAVITY == 0 {
        return Ok(false);
    }
    // 0x1ced2a has applied the level gravity and tick length. H1 0x8399d
    // scales that delta by phys+1c independently of seat power or throttle.
    let scale = m.f32(def + 0x2e0)? * GRAVITY_RATIO;
    for (i, value) in m.vec3(delta)?.into_iter().enumerate() {
        m.set_f32(delta + i as u32 * 4, value * scale)?;
    }
    Ok(true)
}

/// 0xc5eff calls control refresh with ESI = body, EBP = definition.
fn seat_power(m: &mut dyn GuestMemory, body: u32, def: u32) -> Result<bool, MemFault> {
    let Some(record) = power_record(m, def)? else {
        return Ok(false);
    };
    let count = m.u32(record + 4)? & 0xff;
    if count == 0 || count > 2 {
        return Ok(false);
    }
    let driver = m.u32(body + 0x248)?;
    let gunner = m.u32(body + 0x24c)?;
    let occupied = [
        driver != u32::MAX || m.u32(body + 0x134)? & 2 != 0,
        gunner != u32::MAX && gunner != driver,
    ];
    let disabled = m.u8(body + 0x10a)? & 4 != 0;
    let dt = m.f32(m.u32(GAME_TIME.base)? + 4)?;
    let mut changed = false;
    for i in 0..count {
        let active = occupied[i as usize] && !disabled;
        let row = record + 8 + i * 8;
        let seconds = m.f32(if active { row } else { row + 4 })?;
        let step = if seconds == 0.0 { 1.0 } else { dt / seconds };
        let at = body + 0x25c + i * 4;
        let old = m.f32(at)?;
        let next = if active {
            (old + step).min(1.0)
        } else {
            (old - step).max(0.0)
        };
        if next != old {
            changed = true;
            m.set_f32(at, next)?;
        }
    }
    Ok(changed)
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
        Manifest::new("h1-drive", "Halo 1 vehicle physics").doc("docs/halo1/role-audit.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(feature::SnapshotPolicy::Stateless);
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
                    if let (Some(contacts), Some(force), Some(torque)) = (
                        c.arg_u32("contacts"),
                        c.arg_u32("force"),
                        c.arg_u32("torque"),
                    ) && force != 0
                        && torque != 0
                    {
                        flight_forces(&mut *c.ctx.memory(), contacts, force, torque)
                            .map_err(fault)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        b.raw()
            .before(
                &VEHICLE_GRAVITY_DELTA,
                "apply Halo 1 vehicle gravity",
                |call| {
                    let active = {
                        let mut mem = call.ctx.memory();
                        match vehicle_definition(&*mem, call.cpu.eax).map_err(fault)? {
                            Some((_, def)) => {
                                gravity_delta(&mut *mem, def, call.cpu.esi).map_err(fault)?
                            }
                            None => false,
                        }
                    };
                    if active {
                        return call.ret(0).map(Flow::Return);
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        b.raw()
            .before(
                &UNIT_CONTROL_REFRESH,
                "advance Halo 1 powered seats",
                |call| {
                    let mut mem = call.ctx.memory();
                    let sp = call.cpu.esp;
                    if GuestMemory::u32(&*mem, sp).map_err(fault)? == 0xc5f04
                        && seat_power(&mut *mem, call.cpu.esi, call.cpu.ebp).map_err(fault)?
                    {
                        GuestMemory::set_u8(&mut *mem, sp + 4 + 0x13, 1).map_err(fault)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        b.raw()
            .after(
                &VEHICLE_CONTACT_FORCES,
                "apply Halo 1 wheel and material forces",
                |call| {
                    let at = call.cpu.esp.wrapping_sub(12);
                    let mut mem = call.ctx.memory();
                    let args = [at, at + 4, at + 8].map(|a| GuestMemory::u32(&*mem, a));
                    let [contacts, force, torque] = args;
                    let active = ground_forces(
                        &mut *mem,
                        contacts.map_err(fault)?,
                        force.map_err(fault)?,
                        torque.map_err(fault)?,
                    )
                    .map_err(fault)?;
                    if active {
                        // 0x205746 enables H2's additional low-slip braking only
                        // on a true return. The source ramps to rest in its drive law.
                        call.cpu.eax = 0;
                    }
                    Ok(())
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
    fn marked_contacts_add_material_scaled_force_and_moment_once() {
        use tags::object::direct_drive::*;
        let mut m = FlatMemory::new(0x1000, 0x6000);
        let (root, phantom, contacts, force, torque) = (0x2000, 0x2400, 0x3000, 0x4800, 0x4810);
        let c = contacts + 0xb10;
        let data = phantom + CONTACT_DATA;
        for (at, x) in [
            (contacts + 4, root),
            (root + 0x44, 1),
            (root + 0x4c, 1),
            (root + 0x50, phantom),
            (data + 4, 0x101),
            (data + 8, 1),
            (data + 24, 19),
        ] {
            m.set_u32(at, x).unwrap();
        }
        for (at, x) in [
            (contacts + 0x6c, 100.0),
            (root + 0x14, 0.7),
            (root + 0x18, 0.5),
            (c + 4, 1.0),
            (c + 0x24, 1.0),
            (c + 0x44, 2.0),
            (c + 0x4c, 4.0),
            (c + 0x84, 0.1),
            (c + 0xa8, 1.0),
            (data + 12, 2.0),
            (data + 16, 0.5),
            (data + 20, 0.25),
            (data + 28, 0.125),
            (data + 32, 1.0),
            (data + 36, 1.0),
            (data + 40, 1.0),
            (data + 44, 1.0),
            (force + 8, 250.0),
        ] {
            m.set_f32(at, x).unwrap();
        }
        m.set_u16(c + 0xb0, 19).unwrap();
        assert!(!ground_forces(&mut m, contacts, force, torque).unwrap());
        assert_eq!(m.vec3(force).unwrap(), [0.0, 0.0, 250.0]);
        m.set_u32(data, CONTACT_MAGIC).unwrap();
        assert!(ground_forces(&mut m, contacts, force, torque).unwrap());
        assert_eq!(m.vec3(force).unwrap(), [-50.0, 0.0, 250.0]);
        assert_eq!(m.vec3(torque).unwrap(), [0.0, 0.0, 100.0]);
        m.set_u8(c + 0xb3, 1).unwrap();
        ground_forces(&mut m, contacts, force, torque).unwrap();
        assert_eq!(m.vec3(force).unwrap(), [-50.0, 0.0, 250.0]);
    }

    #[test]
    fn unpowered_vehicle_falls_with_source_gravity_and_level_scale() {
        use tags::object::direct_drive::{HAS_SOURCE_GRAVITY, POWER_DATA, POWER_MAGIC};
        let mut m = FlatMemory::new(0x1000, 0x3000);
        let (def, phantom, delta) = (0x1000, 0x2000, 0x3000);
        m.set_u16(def, u16::from(objects::VEHICLE)).unwrap();
        m.set_u32(def + 0x2f8, 1).unwrap();
        m.set_u32(def + 0x2fc, phantom).unwrap();
        m.set_f32(def + 0x2e0, 0.75).unwrap();
        m.set_f32(delta + 8, -4.1712594 * 0.5 / 30.0).unwrap();
        assert!(!gravity_delta(&mut m, def, delta).unwrap());
        m.set_u32(phantom + POWER_DATA, POWER_MAGIC).unwrap();
        m.set_u32(phantom + POWER_DATA + 4, HAS_SOURCE_GRAVITY)
            .unwrap();
        assert!(gravity_delta(&mut m, def, delta).unwrap());
        let expected = -0.0035651792 * 30.0 * 0.75 * 0.5;
        assert!((m.f32(delta + 8).unwrap() - expected).abs() < 1e-6);
    }

    #[test]
    fn driver_and_gunner_power_change_on_their_own_clocks() {
        use tags::object::direct_drive::{POWER_DATA, POWER_MAGIC};
        let mut m = FlatMemory::new(0x4e0000, 0x70000);
        let (body, def, phantom, time) = (0x500000, 0x501000, 0x502000, 0x503000);
        let rows = phantom + POWER_DATA;
        m.set_u16(def, u16::from(objects::VEHICLE)).unwrap();
        for (at, x) in [
            (def + 0x2f8, 1),
            (def + 0x2fc, phantom),
            (rows, POWER_MAGIC),
            (rows + 4, 2),
            (GAME_TIME.base, time),
            (body + 0x248, 42),
            (body + 0x24c, 42),
        ] {
            m.set_u32(at, x).unwrap();
        }
        for (at, x) in [
            (time + 4, 1.0 / 30.0),
            (rows + 8, 1.0),
            (rows + 12, 0.1),
            (rows + 16, 0.0),
            (rows + 20, 0.0),
        ] {
            m.set_f32(at, x).unwrap();
        }
        for _ in 0..30 {
            seat_power(&mut m, body, def).unwrap();
        }
        assert_eq!(m.f32(body + 0x25c).unwrap(), 1.0);
        assert_eq!(m.f32(body + 0x260).unwrap(), 0.0);
        m.set_u32(body + 0x248, u32::MAX).unwrap();
        seat_power(&mut m, body, def).unwrap();
        assert!((m.f32(body + 0x25c).unwrap() - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(m.f32(body + 0x260).unwrap(), 1.0);
        m.set_u8(body + 0x10a, 4).unwrap();
        seat_power(&mut m, body, def).unwrap();
        assert_eq!(m.f32(body + 0x260).unwrap(), 0.0);
    }

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
