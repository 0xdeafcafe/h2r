//! Halo 1's involuntary collision-region and freefall speech.

use std::sync::Arc;

use live_telemetry::layout::{
    BIPED_AIRBORNE_TICKS, BODY_PARENT, BODY_POSITION, BODY_VELOCITY, UNIT_FLAGS, UNIT_SEAT,
};
use map::{
    H1_DIALOGUE_AIRBORNE_PASSENGERS, H1_DIALOGUE_CAN_SCREAM_IN_FREEFALL, H1_FREEFALL_LINE,
    H1_PHYSICAL_DIALOGUE,
};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use super::{cast, dialogue, leaders::guest_fault, reactions::Queue, visibility, *};

const OBJECT_CAPACITY: u32 = 2048;
const REGION_CAPACITY: u32 = 16;
const REGION_DAMAGE: u32 = 24;
const RECORD_BYTES: u32 = REGION_DAMAGE + REGION_CAPACITY;
const VEHICLE_AIRBORNE_TICKS: u32 = 0x34c;
const HISTORY_BYTES: u32 = OBJECT_CAPACITY * RECORD_BYTES;
const DEPLETED: u8 = 4;
const PRIORITY: u32 = 14;

const FALL_RAY_LENGTH: f32 = 6.0;
const FALL_RAY_START: f32 = 0.4;
// Halo 1 physics globals +0x94, identical across the 23 retail maps.
const H1_DANGEROUS_FALL_SPEED: f32 = 0.206_838_46;
const H1_GRAVITY: f32 = 0.003_565_179_2;

#[derive(Clone, Copy)]
pub(super) struct History(u32);

impl History {
    fn address(self, unit: u32) -> Option<u32> {
        let index = unit & 0xffff;
        (index < OBJECT_CAPACITY).then_some(self.0 + index * RECORD_BYTES)
    }

    fn record(self, m: &mut dyn GuestMemory, unit: u32) -> Result<Option<u32>, MemFault> {
        let Some(at) = self.address(unit) else {
            return Ok(None);
        };
        if m.u32(at)? != unit {
            m.fill(at, RECORD_BYTES, 0)?;
            m.set_u32(at, unit)?;
        }
        Ok(Some(at))
    }
}

fn model(m: &dyn GuestMemory, body: u32, group: [u8; 4]) -> Result<Option<(u32, u32)>, MemFault> {
    let Some(definition) = dialogue::tag_address(m, m.u32(body)?, GroupTag::from_chars(group))?
    else {
        return Ok(None);
    };
    let Some(model) =
        dialogue::tag_address(m, m.u32(definition + 0x38)?, GroupTag::from_chars(*b"hlmt"))?
    else {
        return Ok(None);
    };
    let flags = m.u32(model + 0x80)?;
    Ok((flags & 0xffff_0000 == H1_PHYSICAL_DIALOGUE).then_some((model, flags)))
}

fn region_damage(
    m: &mut dyn GuestMemory,
    history: History,
    queue: Queue,
    unit: u32,
    damage: u32,
) -> Result<(), MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(());
    };
    let injury = m.f32(damage + 0x28)?;
    if !injury.is_finite() || injury <= 0.0 || m.u8(body + 0x10a)? & DEPLETED != 0 {
        return Ok(());
    }
    let Some((model, _)) = model(m, body, *b"bipd")? else {
        return Ok(());
    };
    // The projectile collision result's +0x44 is the collision region
    // (0x16961d). Damage setup copies it to the packet at 0xd7e1d.
    let region = m.u32(damage + 0xc)?;
    if region >= REGION_CAPACITY || region >= m.u32(model + 0x9c)? {
        return Ok(());
    }
    let entry = m.u32(model + 0xa0)? + region * 8;
    let threshold = m.f32(entry)?;
    if threshold <= 0.0 || !threshold.is_finite() {
        return Ok(());
    }
    let Some(record) = history.record(m, unit)? else {
        return Ok(());
    };
    let mask = 1 << region;
    let broken = m.u16(record + 8)?;
    if broken & mask != 0 {
        return Ok(());
    }
    // Halo 1 0x9512f accumulates applied body damage in one byte per
    // region. 0x95189 destroys the region only above its authored fraction.
    let accumulator = record + REGION_DAMAGE + region;
    let total = ((f32::from(m.u8(accumulator)?) + injury * 255.0) as u32) as u8;
    m.set_u8(accumulator, total)?;
    if f32::from(total) * (1.0 / 255.0) > threshold {
        m.set_u16(record + 8, broken | mask)?;
        let offset = if m.u32(entry + 4)? & 0x200 == 0 {
            0xd0
        } else {
            0xe0
        };
        if dialogue::source_offset_row(m, unit, offset)?.is_some() {
            queue.push(m, unit, offset, PRIORITY)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Motion {
    Biped(f32),
    Vehicle([f32; 3]),
}

pub(super) struct Fall {
    position: [f32; 3],
    motion: Motion,
}

fn dangerous_biped_fall(speed: f32, hit_fraction: Option<f32>) -> bool {
    let Some(fraction) = hit_fraction else {
        return true;
    };
    let height = FALL_RAY_LENGTH * fraction - FALL_RAY_START;
    speed <= 0.0
        && speed * speed + 2.0 * H1_GRAVITY * height
            >= H1_DANGEROUS_FALL_SPEED * H1_DANGEROUS_FALL_SPEED
}

fn occupant_motion(
    m: &mut dyn GuestMemory,
    body: u32,
    parent: u32,
    record: u32,
    now: u32,
    rate: u32,
) -> Result<Option<Fall>, MemFault> {
    let Some(vehicle) = object_address(m, parent)? else {
        return Ok(None);
    };
    let Some((_, flags)) = model(m, vehicle, *b"vehi")? else {
        return Ok(None);
    };
    let seat = m.u16(body + UNIT_SEAT)?;
    if seat == u16::MAX || flags & H1_DIALOGUE_AIRBORNE_PASSENGERS == 0 {
        return Ok(None);
    }
    if m.u32(record + 12)? != parent || m.u16(record + 20)? != seat {
        m.set_u32(record + 12, parent)?;
        m.set_u32(record + 16, now)?;
        m.set_u16(record + 20, seat)?;
        return Ok(None);
    }
    // H1 0x4b5e8 requires four seconds in this seat and one second with
    // the parent vehicle airborne. H2 0xf6bd0 maintains that timer.
    if now.wrapping_sub(m.u32(record + 16)?) <= 4 * rate
        || u32::from(m.u8(vehicle + VEHICLE_AIRBORNE_TICKS)?) <= rate
    {
        return Ok(None);
    }
    let mut position = m.vec3(vehicle + BODY_POSITION)?;
    position[2] -= FALL_RAY_START;
    let [vx, vy, vz] = m.vec3(vehicle + BODY_VELOCITY)?;
    Ok(Some(Fall {
        position,
        // H1 projects two seconds of motion with 1/2 g t² subtracted.
        motion: Motion::Vehicle([vx * 2.0, vy * 2.0, vz * 2.0 - H1_GRAVITY * 1800.0]),
    }))
}

fn custom_animation(m: &dyn GuestMemory, body: u32) -> Result<bool, MemFault> {
    // The native HaloScript predicate calls 0x11b930 to check the
    // animation manager's active custom channel.
    let offset = read_i16(m, body + 0x12a)?;
    if offset <= 0 {
        return Ok(false);
    }
    let manager = body + offset as u32;
    Ok(m.u32(manager + 0x68)? != NONE
        && m.u32(manager)? != NONE
        && m.u16(manager + 6)? != u16::MAX
        && m.u32(manager + 0x7c)? == 0x0e00_00c2)
}

pub(super) fn falling(
    m: &mut dyn GuestMemory,
    history: History,
    unit: u32,
) -> Result<Option<Fall>, MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(None);
    };
    if m.u8(body + 0xaa)? != 0
        || m.u8(body + 0x10a)? & DEPLETED != 0
        || m.u32(body + cast::UNIT_ACTOR)? == NONE
    {
        return Ok(None);
    }
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 {
        return Ok(None);
    }
    let rate = u32::from(m.u16(clock + 2)?);
    let parent = m.u32(body + BODY_PARENT)?;
    let airborne = i32::from(m.u8(body + BIPED_AIRBORNE_TICKS)? as i8);
    if rate == 0
        || (parent == NONE
            && (airborne <= rate as i32 || m.u32(body + UNIT_FLAGS)? & (1 << 9) != 0))
        || (parent != NONE && !reactions::ai_vocalizes(m)?)
        || dialogue::source_offset_row(m, unit, H1_FREEFALL_LINE)?.is_none()
    {
        // Forget the old seat even when the biped is back on the ground.
        if parent == NONE
            && let Some(record) = history.address(unit)
            && m.u32(record)? == unit
            && m.u32(record + 12)? != NONE
        {
            m.set_u32(record + 12, NONE)?;
        }
        return Ok(None);
    }
    if custom_animation(m, body)? {
        return Ok(None);
    }
    let Some(record) = history.record(m, unit)? else {
        return Ok(None);
    };
    let now = m.u32(clock + 8)?;
    let fall = if parent == NONE {
        let Some((_, flags)) = model(m, body, *b"bipd")? else {
            return Ok(None);
        };
        if flags & H1_DIALOGUE_CAN_SCREAM_IN_FREEFALL == 0 {
            return Ok(None);
        }
        m.set_u32(record + 12, NONE)?;
        let mut position = m.vec3(body + BODY_POSITION)?;
        position[2] -= FALL_RAY_START;
        Some(Fall {
            position,
            motion: Motion::Biped(m.f32(body + BODY_VELOCITY + 8)? / 30.0),
        })
    } else {
        occupant_motion(m, body, parent, record, now, rate)?
    };
    if fall.is_none()
        || (m.u8(record + 10)? != 0 && now.wrapping_sub(m.u32(record + 4)?) <= rate / 2)
    {
        return Ok(None);
    }
    m.set_u32(record + 4, now)?;
    m.set_u8(record + 10, 1)?;
    Ok(fall)
}

fn trace(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    scratch: u32,
    from: [f32; 3],
    delta: [f32; 3],
    continuation: impl FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault>
    + Send
    + 'static,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut m = rt.memory();
    let vector = scratch + 12;
    for (i, (position, displacement)) in from.into_iter().zip(delta).enumerate() {
        m.set_f32(scratch + i as u32 * 4, position)?;
        m.set_f32(vector + i as u32 * 4, displacement)?;
    }
    // Static structure only, as in the Halo 1 probe at 0x4920d.
    let stack = scratch - 24;
    for (i, value) in [1, scratch, vector, NONE, NONE, scratch + 0x20]
        .into_iter()
        .enumerate()
    {
        m.set_u32(stack + i as u32 * 4, value)?;
    }
    cpu.esp = stack;
    ctx.call_guest_named(
        cpu,
        &mut m,
        visibility::COLLISION_TEST_VECTOR,
        "Halo 1 freefall ground check",
        continuation,
    )
}

pub(super) fn probe(
    call: &mut hooks::Call<'_>,
    fall: Fall,
    queue: Queue,
) -> Result<Flow, GuestFault> {
    let saved = *call.cpu;
    let addr = call.addr;
    let original = call
        .ctx
        .runtime()
        .resolve_original(addr)
        .expect("speech update is a generated hook");
    let scratch = saved.esp.wrapping_sub(0xc0);
    let result = scratch + 0x20;
    let finish = move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, scream| {
        if scream {
            queue
                .push(&mut *ctx.memory(), saved.eax, H1_FREEFALL_LINE, PRIORITY)
                .map_err(guest_fault)?;
        }
        *cpu = saved;
        Ok(original.invoke(cpu, ctx, addr))
    };
    let distance = match fall.motion {
        Motion::Biped(_) => FALL_RAY_LENGTH,
        Motion::Vehicle(_) => 8.0,
    };
    trace(
        call.cpu,
        call.ctx,
        scratch,
        fall.position,
        [0.0, 0.0, -distance],
        move |cpu, ctx| {
            let hit = cpu.eax as u8 != 0;
            match fall.motion {
                Motion::Biped(speed) => {
                    let fraction = hit.then(|| ctx.memory().f32(result + 4)).transpose()?;
                    finish(cpu, ctx, dangerous_biped_fall(speed, fraction))
                }
                Motion::Vehicle(_) if hit => finish(cpu, ctx, false),
                Motion::Vehicle(projected) => {
                    let length = projected.iter().map(|v| v * v).sum::<f32>().sqrt();
                    if length <= f32::EPSILON {
                        return finish(cpu, ctx, true);
                    }
                    let delta = projected.map(|v| v * (8.0 / length));
                    trace(cpu, ctx, scratch, fall.position, delta, move |cpu, ctx| {
                        // H1 reports the fall when the projected path has no
                        // ground with a normal pointing upward past 0.3.
                        let scream = cpu.eax as u8 == 0 || ctx.memory().f32(result + 0x30)? <= 0.3;
                        finish(cpu, ctx, scream)
                    })
                }
            }
        },
    )
    .map(Flow::Return)
}

pub(super) fn install(b: &mut Builder, queue: Queue) -> Result<History, BuildError> {
    let history = History(b.guest_alloc(HISTORY_BYTES, Permissions::RW)?);
    b.raw().before(
        &points::HS_CONVERSATION_MAP_DISPOSE,
        "clear Halo 1 physical dialogue history",
        move |call| {
            call.ctx
                .memory()
                .fill(history.0, HISTORY_BYTES, 0)
                .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
    b.raw().after(
        &points::BODY_DAMAGE,
        "check Halo 1 collision region damage dialogue",
        move |call| {
            let m = &mut *call.ctx.memory();
            // The native function preserves EDI and pops the object's argument.
            let unit = m.u32(call.cpu.esp.wrapping_sub(12))?;
            region_damage(m, history, queue, unit, call.cpu.edi).map_err(guest_fault)
        },
    );
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freefall_checks_air_clearance_and_projected_impact_speed() {
        assert!(dangerous_biped_fall(-0.1, None));
        assert!(!dangerous_biped_fall(0.1, Some(1.0)));
        assert!(!dangerous_biped_fall(-0.1, Some(0.2)));
        assert!(dangerous_biped_fall(-0.1, Some(0.9)));
        assert!(dangerous_biped_fall(-0.25, Some(0.2)));
    }
}
