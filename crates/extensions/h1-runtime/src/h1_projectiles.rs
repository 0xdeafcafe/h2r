//! Halo 1 integrates projectile acceleration in time and scales direct
//! damage by impact speed. Both operate on the current collision substep.

use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use guest_layout::points::{DAMAGE_DATA_INIT, VECTOR_LENGTH_3D};
use runtime::{Guest, GuestFault};
use tags::H1_PROJECTILE_VELOCITY_CURVE;

const FLIGHT_RETURN: u32 = 0x000f_966a;
const IMPACT_RETURN: u32 = 0x000f_ad79;

pub struct Projectiles;

impl Feature for Projectiles {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-projectiles", "Halo 1 projectile flight and impact")
            .doc("docs/halo1/objects.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        b.raw()
            .before(&VECTOR_LENGTH_3D, "integrate Halo 1 projectile velocity", |c| {
                flight_call(&mut c.ctx.memory(), c.cpu.esp, c.cpu.ebp, c.cpu.ebx)?;
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw()
            .before(&DAMAGE_DATA_INIT, "scale Halo 1 impact damage", |c| {
                impact_call(&mut c.ctx.memory(), c.cpu.esp, c.cpu.ebx, c.cpu.edi)?;
                Ok(Flow::Continue)
            })
            .context_only();
        Ok(())
    }
}

fn flight_call(mem: &mut Guest, esp: u32, body: u32, tag: u32) -> Result<(), GuestFault> {
    // This call sees the candidate velocity before H2 computes the range
    // cutoff, applies gravity, and casts the collision ray.
    if mem.u32(esp)? == FLIGHT_RETURN && marked(mem, tag)? {
        // 0xf95e5 has advanced the private H2 linear-ramp accumulator.
        // Start its next substep at zero so it never adds a second speed
        // change. The sweep distance and the tag's effect inputs remain live.
        mem.set_f32(body + 0x194, 0.0)?;
        integrate(mem, esp + 4, body, tag)?;
    }
    Ok(())
}

fn impact_call(mem: &mut Guest, esp: u32, body: u32, tag: u32) -> Result<(), GuestFault> {
    if mem.u32(esp)? == IMPACT_RETURN && marked(mem, tag)? {
        // The caller pushed the damage datum and the return address. +0x10
        // is the magnitude before H2 normalizes the impact vector.
        let frame = esp + 8;
        let speed = mem.f32(frame + 0x10)?;
        let incoming = mem.f32(body + 0x188)?;
        let fraction = impact(speed, mem.f32(tag + 0x17c)?, mem.f32(tag + 0x180)?);
        mem.set_f32(frame + 0x28, incoming * fraction)?;
    }
    Ok(())
}

fn marked(mem: &Guest, tag: u32) -> Result<bool, GuestFault> {
    Ok(mem.u32(tag + 0x198)? == H1_PROJECTILE_VELOCITY_CURVE)
}

// H1 0xc1a60 and 0xc3f0e. Split the step where it crosses either end
// of the acceleration interval. The ray uses mean velocity over the
// whole step, while the next step starts at the terminal velocity.
fn step(
    initial: f32,
    terminal: f32,
    bounds: [f32; 2],
    distance: f32,
    speed: f32,
    dt: f32,
) -> (f32, f32) {
    let span = bounds[1] - bounds[0];
    if span <= 0.0 || dt <= 0.0 || speed <= 0.0 || initial == terminal {
        return (speed, speed);
    }
    let coast = ((bounds[0] - distance) / speed).clamp(0.0, dt);
    let acceleration = (terminal * terminal - initial * initial) / (2.0 * span);
    let time_to_terminal = (terminal - speed) / acceleration;
    if time_to_terminal <= 0.0 {
        return (speed, speed);
    }
    let accelerating = (dt - coast).min(time_to_terminal);
    let end = speed + acceleration * accelerating;
    let travelled =
        speed * coast + (speed + end) * 0.5 * accelerating + end * (dt - coast - accelerating);
    (end, travelled / dt)
}

fn impact(speed: f32, initial: f32, terminal: f32) -> f32 {
    if initial == terminal {
        1.0
    } else {
        ((speed - terminal) / (initial - terminal)).clamp(0.0, 1.0)
    }
}

fn integrate(mem: &mut Guest, frame: u32, body: u32, tag: u32) -> Result<(), GuestFault> {
    // H2's +4 bit 3 selects water at 0xf96c8. Both pairs of H1
    // distances are still available on the rebuilt projectile.
    let medium = if mem.u32(body + 4)? & 8 == 0 {
        0x168
    } else {
        0x174
    };
    let bounds = [mem.f32(tag + medium)?, mem.f32(tag + medium + 4)?];
    let velocity = [
        mem.f32(frame + 0x10)?,
        mem.f32(frame + 0x14)?,
        mem.f32(frame + 0x18)?,
    ];
    let speed = velocity.iter().map(|v| v * v).sum::<f32>().sqrt();
    if speed == 0.0 {
        return Ok(());
    }
    let (end, mean) = step(
        mem.f32(tag + 0x17c)?,
        mem.f32(tag + 0x180)?,
        bounds,
        mem.f32(body + 0x168)?,
        speed,
        mem.f32(frame + 0x20)?,
    );
    if speed == end && speed == mean {
        return Ok(());
    }
    // 0xf9675 averages the length about to be returned with this
    // saved scalar for its range cutoff. A step that crosses either
    // bound has a piecewise mean, not the midpoint of its endpoints.
    mem.set_f32(frame + 0xac, 2.0 * mean - end)?;
    for (axis, v) in velocity.into_iter().enumerate() {
        let offset = axis as u32 * 4;
        let direction = v / speed;
        let average = mem.f32(frame + 0x48 + offset)?;
        mem.set_f32(frame + 0x10 + offset, v + direction * (end - speed))?;
        mem.set_f32(frame + 0x48 + offset, average + direction * (mean - speed))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_converted_projectile_changes_the_guest_flight_and_damage_frames() {
        let mut mem = Guest::new();
        let (esp, body, tag) = (0x1000, 0x2000, 0x3000);
        mem.map_zeroed(esp, 0x3000, runtime::guest::Permissions::RW)
            .unwrap();
        mem.set_f32(tag + 0x17c, 12.0).unwrap();
        mem.set_f32(tag + 0x180, 6.0).unwrap();
        mem.set_f32(tag + 0x16c, 900.0).unwrap();
        mem.set_f32(tag + 0x178, 9.0).unwrap();
        mem.set_u32(body + 4, 8).unwrap(); // water
        mem.set_f32(body + 0x188, 0.75).unwrap();
        let frame = esp + 4;
        mem.set_f32(frame + 0x10, 12.0).unwrap();
        mem.set_f32(frame + 0x48, 12.0).unwrap();
        mem.set_f32(frame + 0x20, 0.5).unwrap();
        mem.set_u32(esp, FLIGHT_RETURN).unwrap();
        flight_call(&mut mem, esp, body, tag).unwrap();
        assert_eq!(mem.f32(frame + 0x10).unwrap(), 12.0);
        mem.set_u32(tag + 0x198, H1_PROJECTILE_VELOCITY_CURVE)
            .unwrap();
        mem.set_f32(body + 0x194, 6.0).unwrap();
        flight_call(&mut mem, esp, body, tag).unwrap();
        assert_eq!(mem.f32(body + 0x194).unwrap(), 0.0);
        assert_eq!(mem.f32(frame + 0x10).unwrap(), 9.0);
        assert_eq!(mem.f32(frame + 0x48).unwrap(), 10.5);
        assert_eq!(mem.f32(frame + 0xac).unwrap(), 12.0);
        // A different caller cannot dereference its unrelated registers.
        mem.set_u32(esp, 0).unwrap();
        flight_call(&mut mem, esp, 0, 0).unwrap();
        impact_call(&mut mem, esp, 0, 0).unwrap();
        mem.set_u32(esp, IMPACT_RETURN).unwrap();
        mem.set_f32(esp + 8 + 0x10, 9.0).unwrap();
        impact_call(&mut mem, esp, body, tag).unwrap();
        assert_eq!(mem.f32(esp + 8 + 0x28).unwrap(), 0.375);
        assert_eq!(mem.f32(body + 0x188).unwrap(), 0.75);
    }

    #[test]
    fn velocity_and_damage_follow_the_medium_and_speed() {
        // Coast for a second, decelerate for a second, then coast at
        // terminal speed for one second. Distance is 12 + 9 + 6.
        assert_eq!(step(12.0, 6.0, [12.0, 21.0], 0.0, 12.0, 3.0), (6.0, 9.0));
        assert_eq!(step(12.0, 6.0, [0.0, 9.0], 0.0, 12.0, 0.5), (9.0, 10.5));
        assert_eq!(step(12.0, 6.0, [0.0, 0.0], 3.0, 9.0, 1.0), (9.0, 9.0));
        assert_eq!(impact(9.0, 12.0, 6.0), 0.5);
        assert_eq!(impact(15.0, 12.0, 6.0), 1.0);
        assert_eq!(impact(3.0, 12.0, 6.0), 0.0);
        assert_eq!(impact(5.0, 12.0, 12.0), 1.0);
    }
}
