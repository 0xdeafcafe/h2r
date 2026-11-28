//! Halo 1 point physics and emitter clocks on native Halo 2 attachments.
//!
//! Only the marked tags built by h1objects take these paths. The guest owns
//! the particle, emitter and effect records, including their lifetime and
//! collision queries. No timing or particle state is kept on the host.

use guest_layout::datum::{DatumArray, layout};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::tag_entry_checked;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::{Access, FaultReason};
use tags::effects::h1_attachment::{
    DIRECTIONAL_BIRTH, KILL_IN_AIR, KILL_IN_LIQUID, KILL_ON_CONTACT, MASS_BY_RADIUS, POINT_PHYSICS,
    POPULATION_CLOCK, PointPhysics, RADIAL_BIRTH, minimum_population,
};

const EFFECTS: u32 = 0x004e_a93c;
const SYSTEMS: u32 = 0x0051_0c74;
const EMITTERS: u32 = 0x0051_ec88;
const PARTICLES: u32 = 0x0051_ec84;
const STRUCTURE_COLLISION: u32 = 0x004e_033c;
const STRUCTURE: u32 = 0x004e_0348;

const EXPIRED: u16 = 1;
const COLLIDED: u16 = 2;
const INACTIVE: u16 = 8;

pub struct H1ParticlePhysics;

impl Feature for H1ParticlePhysics {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "h1-particle-physics",
            "Halo 1 physics for attached particle systems",
        )
        .doc("docs/halo1/objects.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use hooks::points::{EFFECT_STEP_EMITTERS, PARTICLE_INITIALISE, PARTICLE_MOVEMENT_STAGE};
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw().after(
            &PARTICLE_INITIALISE,
            "apply Halo 1 particle birth velocity",
            |call| {
                // 0x00249170 preserves EDI (the new particle), pops two
                // arguments and returns to the native marker transform.
                let particle = call.cpu.edi;
                let at = call.cpu.esp.wrapping_sub(8);
                let mut mem = call.ctx.memory();
                let emitter = GuestMemory::u32(&*mem, at).map_err(fault)?;
                apply_birth(&mut *mem, particle, emitter).map_err(fault)
            },
        );
        b.raw().before(
            &PARTICLE_MOVEMENT_STAGE,
            "apply Halo 1 point forces",
            |call| {
                let (stage, head, stack) = (call.cpu.eax, call.cpu.esi, call.cpu.esp);
                let mut mem = call.ctx.memory();
                if let Some(p) = read_physics(&*mem, stage).map_err(fault)? {
                    let dt = GuestMemory::f32(&*mem, stack + 8).map_err(fault)?;
                    apply_forces(&mut *mem, DatumHandle(head), p, dt).map_err(fault)?;
                }
                Ok(Flow::Continue)
            },
        );
        b.raw().after(
            &PARTICLE_MOVEMENT_STAGE,
            "apply Halo 1 contact deaths",
            |call| {
                // stage dispatch preserves the pmov in EDI, stage index in EBX,
                // and the head datum in ESI (caller 0x002ba3a0).
                let (root, index, head) = (call.cpu.edi, call.cpu.ebx, call.cpu.esi);
                let mut mem = call.ctx.memory();
                let stage = GuestMemory::u32(&*mem, root + 0x10).map_err(fault)? + index * 0x14;
                if let Some(p) = read_physics(&*mem, stage).map_err(fault)?
                    && p.particle_type_flags & KILL_ON_CONTACT != 0
                {
                    for_particles(&mut *mem, DatumHandle(head), |mem, particle, flags| {
                        if flags & COLLIDED != 0 {
                            mem.set_u16(particle + 2, flags | EXPIRED)?;
                        }
                        Ok(())
                    })
                    .map_err(fault)?;
                }
                Ok(())
            },
        );
        b.raw().after(
            &EFFECT_STEP_EMITTERS,
            "advance the Halo 1 population clock",
            |call| {
                // ret 12 leaves the three arguments just below ESP. The caller
                // has yet to select the next event (0x001793c6).
                let at = call.cpu.esp.wrapping_sub(12);
                let mut mem = call.ctx.memory();
                let handle = DatumHandle(GuestMemory::u32(&*mem, at).map_err(fault)?);
                update_population_clock(&mut *mem, handle).map_err(fault)
            },
        );
        Ok(())
    }
}

pub fn fault(f: MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

/// The two source initialisers use a unit random direction. Halo 2 has
/// already sampled per-particle random values before this hook, so births
/// do not disturb either engine's random stream or add mutable host state.
fn birth_motion(kind: u32, constants: [f32; 3], u: f32, v: f32) -> ([f32; 3], [f32; 3]) {
    let z = 2.0 * u - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let (sin, cos) = (std::f32::consts::TAU * v).sin_cos();
    let direction = [r * cos, r * sin, z];
    if kind == RADIAL_BIRTH {
        // Halo 1 0x0010f2d0 scales each axis once for offset and again
        // for the radial component of velocity.
        let offset = std::array::from_fn(|i| direction[i] * constants[i]);
        let velocity = std::array::from_fn(|i| offset[i] * constants[i]);
        (offset, velocity)
    } else {
        // Halo 1 0x0010f150 mixes a uniform random direction and the
        // marker's forward vector, then applies 0.05 * the source speed.
        let speed = 0.05 * constants[0];
        let weight = constants[1];
        let mut velocity = direction.map(|x| x * weight * speed);
        velocity[0] += (1.0 - weight) * speed;
        ([0.0; 3], velocity)
    }
}

fn apply_birth(m: &mut dyn GuestMemory, particle: u32, emitter: u32) -> Result<(), MemFault> {
    let property = emitter + 0x28;
    if m.u32(property + 8)? != 0x1c {
        return Ok(());
    }
    let data = m.u32(property + 12)?;
    if data == 0 {
        return Ok(());
    }
    let kind = m.u32(data + 0xc)?;
    if kind != RADIAL_BIRTH && kind != DIRECTIONAL_BIRTH {
        return Ok(());
    }
    let constants = m.vec3(data + 0x10)?;
    if !constants.into_iter().all(f32::is_finite) {
        return Ok(());
    }
    let u = f32::from(m.u16(particle + 0x14)?) / 65536.0;
    let v = f32::from(m.u16(particle + 0x16)?) / 65536.0;
    let (pos, vel) = birth_motion(kind, constants, u, v);
    for i in 0..3 {
        m.set_f32(particle + 0x1c + i as u32 * 4, pos[i])?;
        m.set_f32(particle + 0x28 + i as u32 * 4, vel[i])?;
    }
    Ok(())
}

fn read_physics(m: &dyn GuestMemory, stage: u32) -> Result<Option<PointPhysics>, MemFault> {
    if m.u32(stage)? != 1 || m.u32(stage + 4)? != 2 {
        return Ok(None);
    }
    let p = m.u32(stage + 8)?;
    if m.u32(p + 0xc)? != 0x1c {
        return Ok(None);
    }
    let a = m.u32(p + 0x10)?;
    if a == 0 || m.u32(a + 0xc)? != POINT_PHYSICS || m.u32(p + 0x14 + 0xc)? != 0x1c {
        return Ok(None);
    }
    let b = m.u32(p + 0x14 + 0x10)?;
    if b == 0 {
        return Ok(None);
    }
    let read = |at| -> Result<[u32; 4], MemFault> {
        Ok([
            m.u32(at + 0xc)?,
            m.u32(at + 0x10)?,
            m.u32(at + 0x14)?,
            m.u32(at + 0x18)?,
        ])
    };
    Ok(PointPhysics::from_words([read(a)?, read(b)?]))
}

fn pool(m: &dyn GuestMemory, global: u32) -> Result<DatumArray, MemFault> {
    Ok(DatumArray::at(m.u32(global)?))
}

/// The linked list is native and salted. Bound the walk by the allocated
/// particle slots, including when a malformed list points back to itself.
fn for_particles(
    m: &mut dyn GuestMemory,
    mut handle: DatumHandle,
    mut visit: impl FnMut(&mut dyn GuestMemory, u32, u16) -> Result<(), MemFault>,
) -> Result<(), MemFault> {
    let data = pool(m, PARTICLES)?;
    let limit = data.header_field(m, layout::HIGH_WATER)?.clamp(0, 0x10000);
    for _ in 0..limit {
        let Some(p) = data.try_get(m, handle)? else {
            break;
        };
        handle = DatumHandle(m.u32(p + 4)?);
        let flags = m.u16(p + 2)?;
        if flags & (EXPIRED | INACTIVE) == 0 && m.f32(p + 8)? <= 1.0 {
            visit(m, p, flags)?;
        }
    }
    Ok(())
}

/// Halo 1's scaled masses from 0x000806f0. The engine combines the particle
/// and medium mass before scaling by radius cubed if the tag requests it.
const MEDIUM_MASS: [f32; 2] = [0.0011 * 118613.34, 118613.34];

pub fn point_velocity(p: PointPhysics, v: [f32; 3], liquid: bool, dt: f32) -> [f32; 3] {
    let medium = usize::from(liquid);
    let r2 = p.radius * p.radius;
    let mass = (p.mass + MEDIUM_MASS[medium])
        * if p.flags & MASS_BY_RADIUS != 0 {
            r2 * p.radius
        } else {
            1.0
        };
    let fraction = if mass > 0.0 {
        (dt * r2 * p.drag[medium] / mass).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [v[0], v[1], v[2] + p.acceleration[medium] * dt].map(|x| x * (1.0 - fraction))
}

fn apply_forces(
    m: &mut dyn GuestMemory,
    head: DatumHandle,
    p: PointPhysics,
    dt: f32,
) -> Result<(), MemFault> {
    if head.is_none() || !dt.is_finite() || dt <= 0.0 {
        return Ok(());
    }
    for_particles(m, head, |mem, particle, flags| {
        let pos = mem.vec3(particle + 0x1c)?;
        let old = mem.vec3(particle + 0x28)?;
        let start = std::array::from_fn(|i| pos[i] - old[i] * dt);
        let liquid = in_liquid(mem, start)?;
        let kill = if liquid { KILL_IN_LIQUID } else { KILL_IN_AIR };
        if p.particle_type_flags & kill != 0 {
            return mem.set_u16(particle + 2, flags | EXPIRED);
        }
        let velocity = point_velocity(p, old, liquid, dt);
        // The native ageing pass has already stepped position with the
        // old velocity. Move it to the endpoint of the Halo 1 step so
        // the native collision sweep starts at the original position.
        for i in 0..3 {
            let off = 4 * i as u32;
            mem.set_f32(particle + 0x1c + off, start[i] + velocity[i] * dt)?;
            mem.set_f32(particle + 0x28 + off, velocity[i])?;
        }
        // The native bounce path throttles traces with the top three
        // particle flag bits (0x002b91a0). Halo 1 tests every live step.
        mem.set_u16(particle + 2, flags & 0x1fff)
    })
}

/// Halo 2 finds the cluster with its packed BSP walk (0x0014a280),
/// then classifies the cluster's liquid plane (0x00178d3b).
pub fn in_liquid(m: &dyn GuestMemory, point: [f32; 3]) -> Result<bool, MemFault> {
    let bsp = m.u32(STRUCTURE_COLLISION)?;
    let structure = m.u32(STRUCTURE)?;
    if bsp == 0 || structure == 0 {
        return Ok(false);
    }
    let liquid_count = m.u32(structure + 0x64)?;
    if liquid_count == 0 {
        return Ok(false);
    }
    let node_count = m.u32(bsp)?;
    let plane_count = m.u32(bsp + 8)?;
    let nodes = m.u32(bsp + 4)?;
    let planes = m.u32(bsp + 0xc)?;
    let mut child = 0;
    for _ in 0..64 {
        if child >= node_count {
            return Ok(false);
        }
        let node = nodes + child * 8;
        let plane_index = u32::from(m.u16(node)?);
        if plane_index >= plane_count {
            return Ok(false);
        }
        let plane = planes + plane_index * 16;
        let front = signed_distance(m, plane, point)? >= 0.0;
        child = m.u32(node + if front { 4 } else { 1 })? >> 8;
        if child & 0x80_0000 != 0 {
            break;
        }
    }
    if child == 0xff_ffff || child & 0x80_0000 == 0 {
        return Ok(false);
    }
    let leaf = child & 0x7f_ffff;
    if leaf >= m.u32(structure + 0x2c)? {
        return Ok(false);
    }
    let cluster = u32::from(m.u16(m.u32(structure + 0x30)? + leaf * 8)?);
    if cluster >= m.u32(structure + 0x9c)? {
        return Ok(false);
    }
    let c = m.u32(structure + 0xa0)? + cluster * 0xb0;
    let liquid = m.u8(c + 0x70)?;
    let index = u32::from(liquid & 0x7f);
    if liquid == 0xff || index >= liquid_count {
        return Ok(false);
    }
    let volume = m.u32(structure + 0x68)? + index * 0x18;
    if m.u16(volume + 2)? == u16::MAX {
        return Ok(false);
    }
    Ok(liquid & 0x80 == 0 || signed_distance(m, volume + 4, point)? < 0.0)
}

fn signed_distance(m: &dyn GuestMemory, plane: u32, point: [f32; 3]) -> Result<f32, MemFault> {
    let [x, y, z] = m.vec3(plane)?;
    Ok(point[2] * z + point[1] * y + point[0] * x - m.f32(plane + 12)?)
}

fn adjusted_elapsed(elapsed: f32, duration: f32, live: u32, minimum: f32) -> f32 {
    if (live as f32) < minimum && elapsed < duration {
        duration - (duration - elapsed) * 0.3
    } else {
        elapsed
    }
}

fn update_population_clock(m: &mut dyn GuestMemory, handle: DatumHandle) -> Result<(), MemFault> {
    let Some(effect) = pool(m, EFFECTS)?.try_get(m, handle)? else {
        return Ok(());
    };
    let tag_id = DatumHandle(m.u32(effect + 4)?);
    let Some(entry) = tag_entry_checked(m, tag_id)? else {
        return Ok(());
    };
    if m.u32(entry)? != u32::from_be_bytes(*b"effe") {
        return Ok(());
    }
    let root = m.u32(entry + 8)?;
    if root == 0 || m.u32(root + 0x28)? != POPULATION_CLOCK {
        return Ok(());
    }
    let count = m.u32(root + 0x14)?.min(128);
    let index = u32::from(m.u16(effect + 0x5c)?);
    if index >= count {
        return Ok(());
    }
    let events = m.u32(root + 0x18)?;
    let word = m.u32(events + index * 0x38)?;
    if word & 0x7fff_7fff == 0 {
        return Ok(());
    }
    let elapsed = m.f32(effect + 0x60)?;
    let duration = m.f32(effect + 0x64)?;
    if !(elapsed < duration && duration > 0.0) {
        return Ok(());
    }
    let minimum = minimum_population(word, elapsed / duration, m.f32(effect + 0x6c)?);
    let population = effect_population(m, effect, events, count)?;
    let next = adjusted_elapsed(elapsed, duration, population, minimum);
    if next != elapsed {
        m.set_f32(effect + 0x60, next)?;
    }
    Ok(())
}

/// Visual states are parallel streams in the converted tag. Count the
/// largest stream per phase and retain still-living streams of earlier
/// phases, rather than counting every representation of the same birth.
fn effect_population(
    m: &mut dyn GuestMemory,
    effect: u32,
    events: u32,
    event_count: u32,
) -> Result<u32, MemFault> {
    let systems = pool(m, SYSTEMS)?;
    let emitters = pool(m, EMITTERS)?;
    let mut counts = [0; 128];
    for i in 0..16 {
        let slot = effect + 0x110 + i * 8;
        let system_def = m.u32(slot)?;
        if system_def == 0 {
            continue;
        }
        let Some(system) = systems.try_get(m, DatumHandle(m.u32(slot + 4)?))? else {
            continue;
        };
        let Some(emitter) = emitters.try_get(m, DatumHandle(m.u32(system + 4)?))? else {
            continue;
        };
        let mut live = 0;
        let first = DatumHandle(m.u32(emitter + 4)?);
        for_particles(m, first, |_, _, _| {
            live += 1;
            Ok(())
        })?;
        for (i, count) in counts.iter_mut().enumerate().take(event_count as usize) {
            let event = events + i as u32 * 0x38;
            let start = m.u32(event + 0x34)?;
            let n = m.u32(event + 0x30)?;
            if let Some(delta) = system_def.checked_sub(start)
                && delta % 0x38 == 0
                && delta / 0x38 < n
            {
                *count = (*count).max(live);
                break;
            }
        }
    }
    Ok(counts.into_iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::effects::h1_attachment::population_word;

    use runtime::{Guest, guest::Permissions};

    fn fixture_memory() -> Guest {
        let mut m = Guest::new();
        m.map_zeroed(0x004e_0000, 0x8_0000, Permissions::RW)
            .unwrap();
        m.map_zeroed(0x0060_0000, 0x1_0000, Permissions::RW)
            .unwrap();
        m
    }

    fn datum_pool(m: &mut Guest, global: u32, array: u32, data: u32, stride: u32, count: u32) {
        for (at, word) in [
            (global, array),
            (array + layout::DATA, data),
            (array + layout::ELEMENT_SIZE, stride),
            (array + layout::HIGH_WATER, count),
        ] {
            m.set_u32(at, word).unwrap();
        }
        for i in 0..count {
            m.set_u16(data + i * stride, 0xe100).unwrap();
        }
    }

    #[test]
    fn boot_places_the_three_particle_hooks_on_its_feature_plan() {
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        assert!(registry.add(&H1ParticlePhysics).unwrap());
        let plan = registry.build(&hooks::Base).unwrap();
        for point in [
            &hooks::points::PARTICLE_INITIALISE,
            &hooks::points::PARTICLE_MOVEMENT_STAGE,
            &hooks::points::EFFECT_STEP_EMITTERS,
        ] {
            let chain = plan.chains.get(&point.base).unwrap();
            assert!(chain.shadow.is_some());
        }
    }

    #[test]
    fn reads_the_current_liquid_plane_and_corrects_the_native_sweep() {
        let mut m = fixture_memory();
        let (bsp, structure, particle) = (0x0060_0000, 0x0060_1000, 0x0060_2000);
        datum_pool(&mut m, PARTICLES, particle + 0x400, particle, 0x40, 1);
        for (at, word) in [
            (STRUCTURE_COLLISION, bsp),
            (STRUCTURE, structure),
            (bsp, 1),
            (bsp + 4, bsp + 0x40),
            (bsp + 8, 1),
            (bsp + 0xc, bsp + 0x50),
            (structure + 0x2c, 1),
            (structure + 0x30, structure + 0x200),
            (structure + 0x64, 1),
            (structure + 0x68, structure + 0x220),
            (structure + 0x9c, 1),
            (structure + 0xa0, structure + 0x240),
            (particle + 4, u32::MAX),
        ] {
            m.set_u32(at, word).unwrap();
        }
        // Both sides of the fixture's one plane find leaf 0 of cluster 0.
        m.set_u32(bsp + 0x41, 0x8000_0000).unwrap();
        m.set_u32(bsp + 0x44, 0x8000_0080).unwrap();
        m.set_f32(bsp + 0x58, 1.0).unwrap();
        m.set_u8(structure + 0x240 + 0x70, 0x80).unwrap();
        m.set_f32(structure + 0x220 + 0xc, 1.0).unwrap();
        assert!(in_liquid(&m, [0.0, 0.0, -1.0]).unwrap());
        assert!(!in_liquid(&m, [0.0, 0.0, 1.0]).unwrap());

        let p = PointPhysics {
            flags: 0,
            particle_type_flags: 0,
            mass: 59.0,
            acceleration: [3.0, 5.0],
            drag: [0.0; 2],
            radius: 1.0,
        };
        let handle = DatumHandle(0xe100_0000);
        m.set_f32(particle + 0x24, 1.0).unwrap();
        m.set_f32(particle + 0x1c, 1.0).unwrap();
        m.set_f32(particle + 0x28, 1.0).unwrap();
        apply_forces(&mut m, handle, p, 0.5).unwrap();
        assert_eq!(m.vec3(particle + 0x28).unwrap(), [1.0, 0.0, 1.5]);
        assert_eq!(m.vec3(particle + 0x1c).unwrap(), [1.0, 0.0, 1.75]);
        m.set_f32(particle + 0x24, -1.0).unwrap();
        apply_forces(
            &mut m,
            handle,
            PointPhysics {
                particle_type_flags: KILL_IN_LIQUID,
                ..p
            },
            0.5,
        )
        .unwrap();
        assert_ne!(m.u16(particle + 2).unwrap() & EXPIRED, 0);
    }

    #[test]
    fn population_hook_uses_live_salted_particles_and_skips_retail_effects() {
        use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, index_layout};
        let mut m = fixture_memory();
        let (effect, system, emitter, particle) =
            (0x0060_0000, 0x0060_1000, 0x0060_2000, 0x0060_3000);
        datum_pool(&mut m, EFFECTS, effect + 0x400, effect, 0x190, 1);
        datum_pool(&mut m, SYSTEMS, system + 0x400, system, 0x54, 1);
        datum_pool(&mut m, EMITTERS, emitter + 0x400, emitter, 0x4c, 1);
        datum_pool(&mut m, PARTICLES, particle + 0x400, particle, 0x40, 1);
        let (index, entries, tag, event, system_def) = (
            0x0060_4000,
            0x0060_4200,
            0x0060_4400,
            0x0060_4600,
            0x0060_4800,
        );
        let handle = DatumHandle(0xe100_0000);
        for (at, word) in [
            (TAG_INDEX_GLOBAL, index),
            (TAG_ENTRIES_GLOBAL, entries),
            (index + index_layout::TAG_COUNT, 1),
            (entries, u32::from_be_bytes(*b"effe")),
            (entries + 4, handle.0),
            (entries + 8, tag),
            (effect + 4, handle.0),
            (effect + 0x110, system_def),
            (effect + 0x114, handle.0),
            (tag + 0x14, 1),
            (tag + 0x18, event),
            (event, population_word([1.0; 2], false)),
            (event + 0x30, 1),
            (event + 0x34, system_def),
            (system + 4, handle.0),
            (emitter + 4, handle.0),
            (particle + 4, u32::MAX),
        ] {
            m.set_u32(at, word).unwrap();
        }
        m.set_f32(effect + 0x60, 1.0).unwrap();
        m.set_f32(effect + 0x64, 2.0).unwrap();
        update_population_clock(&mut m, handle).unwrap();
        assert_eq!(m.f32(effect + 0x60).unwrap(), 1.0);
        m.set_u32(tag + 0x28, POPULATION_CLOCK).unwrap();
        update_population_clock(&mut m, handle).unwrap();
        assert_eq!(m.f32(effect + 0x60).unwrap(), 1.0);
        m.set_u16(particle + 2, EXPIRED).unwrap();
        update_population_clock(&mut m, handle).unwrap();
        assert_eq!(m.f32(effect + 0x60).unwrap(), 1.7);
    }

    #[test]
    fn retimes_the_remaining_phase_only_below_the_scaled_threshold() {
        let word = population_word([10.0, 2.0], true);
        assert_eq!(minimum_population(word, 0.5, 0.5), 3.0);
        assert_eq!(adjusted_elapsed(1.0, 2.0, 2, 3.0), 1.7);
        assert_eq!(adjusted_elapsed(1.0, 2.0, 3, 3.0), 1.0);
        assert_eq!(adjusted_elapsed(2.0, 2.0, 0, 3.0), 2.0);
    }

    #[test]
    fn birth_velocity_uses_the_marker_axis_and_source_scatter() {
        assert_eq!(
            birth_motion(DIRECTIONAL_BIRTH, [80.0, 0.0, 0.0], 0.5, 0.0),
            ([0.0; 3], [4.0, 0.0, 0.0]),
        );
        assert_eq!(
            birth_motion(RADIAL_BIRTH, [0.25, 0.0, 1.25], 1.0, 0.0),
            ([0.0, 0.0, 1.25], [0.0, 0.0, 1.5625]),
        );
    }

    #[test]
    fn applies_buoyancy_and_medium_drag_to_inherited_velocity() {
        let p = PointPhysics {
            flags: MASS_BY_RADIUS,
            particle_type_flags: 0,
            mass: 60.0,
            acceleration: [1.2, 3.2],
            drag: [200.0, 15000.0],
            radius: 2.0,
        };
        let dt = 1.0 / 30.0;
        let dry = point_velocity(p, [2.0, 1.0, 0.0], false, dt);
        let wet = point_velocity(p, [2.0, 1.0, 0.0], true, dt);
        assert!(dry[0] < 2.0 && dry[0] > 1.9);
        assert!(wet[0] < 2.0 && wet[0] > dry[0]);
        assert!(wet[2] > dry[2] && dry[2] > 0.0);
    }
}
