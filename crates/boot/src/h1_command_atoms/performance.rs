//! Command-specific inputs to the regular H2 actor and animation updates.
use super::*;

const NONE: u32 = u32::MAX;

// H1 calls _ftol at 0x16b738. Quantize at the source rate before
// converting to the host simulation rate, including at fractional rates.
fn ticks(seconds: f32, rate: u16) -> u32 {
    let source = (f64::from(seconds) * 30.0).trunc() as i32 as i16;
    (u32::from(source.max(0) as u16) * u32::from(rate)).div_ceil(30)
}

fn clock(m: &dyn GuestMemory) -> R<(u32, u16)> {
    let time = m.u32(points::GAME_TIME.base)?;
    Ok((m.u32(time + 8)?, m.u16(time + 2)?))
}

// H1's firing controller copies the requested point into +0x610 when
// its target kind becomes 2. H2 performs the same copy at 0x1fcb4b.
fn accepted(kind: u16, actual: [f32; 3], target: [f32; 3]) -> bool {
    kind == 2
        && actual
            .into_iter()
            .zip(target)
            .map(|(a, t)| (f64::from(a) - f64::from(t)).powi(2))
            .sum::<f64>()
            < 0.25
}

fn settled(now: u32, interval: u32, acquired: bool, deadline: &mut u32) -> bool {
    if !acquired {
        *deadline = now.wrapping_add(interval);
    }
    now.wrapping_sub(*deadline) as i32 >= 0
}

pub(super) fn shoot(
    m: &mut dyn GuestMemory,
    a: Atom,
    actor: u32,
    unit: u32,
    state: u32,
    start: bool,
) -> R<bool> {
    if actor == NONE {
        return Ok(true);
    }
    let record = datum(m, ACTORS, actor, ACTOR_SIZE)?;
    let Some(target) = point(m, a)? else {
        return Ok(true);
    };
    if m.u32(record + 24)? != unit {
        return Ok(true);
    }
    let (now, rate) = clock(m)?;
    if start {
        // Older generated scripts selected the same aim point before entry
        // and enabled shooting after the marker. Activate it here as well.
        m.set_u8(a.command + 0x45, 1)?;
        m.set_u16(a.command + 0x50, 0)?;
        let interval = control::tuning(m, a.scenario, unit)?.recovery;
        m.set_u32(state + 8, ticks(interval, rate).max(2 * u32::from(rate)))?;
        m.set_u32(state + 12, now)?;
        return Ok(false);
    }
    let acquired = accepted(m.u16(record + 0x722)?, m.vec3(record + 0x724)?, target);
    let mut deadline = m.u32(state + 12)?;
    let done = settled(now, m.u32(state + 8)?, acquired, &mut deadline);
    m.set_u32(state + 12, deadline)?;
    Ok(done)
}

fn facing_dot(origin: [f32; 3], facing: [f32; 3], target: [f32; 3], flight: bool) -> Option<f32> {
    let mut delta = std::array::from_fn(|i| target[i] - origin[i]);
    if !flight {
        delta[2] = 0.0;
    }
    let dir = direction([0.0; 3], delta)?;
    Some(dir.iter().zip(facing).map(|(d, f)| d * f).sum())
}

fn face_complete(origin: [f32; 3], facing: [f32; 3], target: [f32; 3], flight: bool) -> bool {
    facing_dot(origin, facing, target, flight).is_none_or(|dot| dot >= 0.984)
}

// H1 0x14c4f5 promotes the driver to flying for a vehicle with both flags.
pub(super) fn flying(m: &dyn GuestMemory, a: Atom, unit: u32) -> R<bool> {
    if control::tuning(m, a.scenario, unit)?.flying {
        return Ok(true);
    }
    let Some((_, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(false);
    };
    let Some(parent) = body.parent(m)? else {
        return Ok(false);
    };
    let Some((objects::VEHICLE, vehicle)) = objects::get(m, parent)? else {
        return Ok(false);
    };
    if m.u32(vehicle.addr + 0x248)? != unit {
        return Ok(false);
    }
    let Some(def) = vehicle.definition(m)? else {
        return Ok(false);
    };
    let Some(tag) = Tags.entry(m, def)? else {
        return Ok(false);
    };
    let address = tag.address(m)?;
    Ok(address != 0 && m.u32(address + 0x1ec)? & 0x1800 == 0x1800)
}

pub(super) fn face(m: &mut dyn GuestMemory, a: Atom, actor: u32, state: u32) -> R<bool> {
    let Some(target) = point(m, a)? else {
        return Ok(true);
    };
    let r = datum(m, ACTORS, actor, ACTOR_SIZE)?;
    let done = face_complete(
        m.vec3(r + 0x238)?,
        m.vec3(r + 0x290)?,
        target,
        m.u8(state + 8)? != 0,
    );
    if done {
        clear_gaze(m, a.command)?;
    }
    Ok(done)
}

// Priority 13's two default entries at H1 0x1f0478, selected at 0x15c244.
fn strength(modifier: u16, combat: i16) -> u8 {
    match modifier {
        1 => 5,
        2 => 2,
        3 => 8,
        4 => 7,
        _ if combat >= 4 => 2,
        _ => 7,
    }
}

fn perceived(m: &dyn GuestMemory, r: u32, object: u32) -> R<bool> {
    let mut id = m.u32(r + 0x58)?;
    while id != NONE {
        let prop = datum(m, 0x502418, id, 0x3c)?;
        if m.u32(prop + 0x20)? == object && m.u16(prop + 0x24)? as i16 >= 1 {
            return Ok(true);
        }
        id = m.u32(prop + 0x2c)?;
    }
    Ok(false)
}

fn clear_gaze(m: &mut dyn GuestMemory, command: u32) -> R<()> {
    m.set_u8(command + 0x46, 0)?;
    m.set_u16(command + 0x50, 0)?;
    m.set_u8(command + 0x52, 0)
}

pub(super) async fn begin_gaze(
    cx: Cx,
    a: Atom,
    actor: u32,
    unit: u32,
    state: u32,
    scratch: u32,
) -> Result<Ret, EntryError> {
    let c = a.command;
    let r = cx.read(|m| datum(m, ACTORS, actor, ACTOR_SIZE))?;
    let kind = cx.read(|m| m.u16(c + 0x48))?;
    let target = cx.read(|m| m.u32(c + 0x4c))?;
    let on = cx.read(|m| m.u8(c + 0x46))? != 0;
    let valid = match kind {
        1 if target != NONE => cx
            .read(|m| objects::get(m, DatumHandle(target)))?
            .is_some_and(|(k, _)| matches!(k, objects::BIPED | objects::VEHICLE)),
        2 => target != NONE,
        _ => false,
    };
    if !on || !valid || !(a.amount > 0.0) {
        cx.write(|m| clear_gaze(m, c))?;
        return Ok(Ret::new(2, 16));
    }
    let snapshot = kind == 1 && !cx.read(|m| perceived(m, r, target))?;
    if snapshot {
        // Same head marker and fallback matrix used by H2's player selector
        // at 0x276ed5. Translation of its second matrix is at +0x60.
        cx.call(
            GuestCall::new(0xb8d30)
                .ecx(0)
                .ebx(scratch)
                .args([target, 0x04000095, 1]),
        )
        .await?;
    }
    cx.write(|m| -> R<()> {
        let (now, rate) = clock(m)?;
        let level = strength(a.operand, m.u16(r + 0x86)? as i16);
        let cone = control::tuning(m, a.scenario, unit)?.gaze_cosine;
        let deadline = now.wrapping_add(ticks(a.amount, rate));
        m.set_u32(state + 12, deadline)?;
        m.set_u8(c + 0x1c, u8::from(snapshot))?;
        m.set_u8(c + 0x1d, level)?;
        m.set_f32(c + 0x20, cone)?;
        if snapshot {
            let xyz = m.vec3(scratch + 0x60)?;
            put_vector(m, c + 0x10, xyz)?;
        }
        m.set_u16(c + 0x54, kind)?;
        m.set_u32(c + 0x58, target)?;
        m.set_u8(c + 0x50, u8::from(level == 8))?;
        m.set_u8(c + 0x51, 0)?;
        m.set_u8(c + 0x52, u8::from(level >= 5))
    })?;
    Ok(Ret::new(0, 16))
}

pub(super) fn update_gaze(m: &mut dyn GuestMemory, a: Atom, state: u32) -> R<bool> {
    if (clock(m)?.0.wrapping_sub(m.u32(state + 12)?) as i32) < 0 {
        return Ok(false);
    }
    clear_gaze(m, a.command)?;
    Ok(true)
}

// Apply the target after H2 prepares the actor's per-tick command inputs.
// A live prop keeps its object descriptor. An unseen unit contributes the
// position of its head only on entry, even if it subsequently moves or dies.
fn gaze_targets(m: &mut dyn GuestMemory, actor: u32) -> R<()> {
    let r = datum(m, ACTORS, actor, ACTOR_SIZE)?;
    let id = m.u32(r + 0x858)?;
    if id == NONE {
        return Ok(());
    }
    let Some(a) = decode(m, id)? else {
        return Ok(());
    };
    let c = a.command;
    if a.kind != 4 || m.u8(c + 0x46)? == 0 {
        return Ok(());
    }
    let level = m.u8(c + 0x1d)?;
    let target = if m.u8(c + 0x1c)? != 0 {
        let xyz = m.vec3(c + 0x10)?;
        m.set_u16(r + 0x430, 2)?;
        m.set_u16(r + 0x434, 3)?;
        put_vector(m, r + 0x438, xyz)?;
        if level >= 5 {
            m.set_u16(r + 0x41c, 3)?;
            m.set_u16(r + 0x420, 3)?;
            put_vector(m, r + 0x424, xyz)?;
        }
        Some(xyz)
    } else if m.u16(r + 0x434)? == 3 {
        Some(m.vec3(r + 0x438)?)
    } else if m.u16(r + 0x434)? == 6 {
        let h = DatumHandle(m.u32(r + 0x438)?);
        match objects::get(m, h)? {
            Some((_, body)) => Some(bots::vehicle::read::unit_position(m, body)?),
            None => None,
        }
    } else {
        None
    };
    if level == 7
        && let Some(xyz) = target
    {
        let cosine = m.f32(c + 0x20)?;
        let within = facing_dot(m.vec3(r + 0x238)?, m.vec3(r + 0x290)?, xyz, false)
            .is_some_and(|dot| dot > cosine);
        m.set_u8(r + 0x44d, u8::from(!within))?;
    } else if level == 8 && target.is_some() {
        m.set_u8(r + 0x44d, 1)?;
    }
    Ok(())
}

// H2 builtin 207 tests the active channel, graph, animation index and
// custom state string (0x11b930). Interruption ends the wait immediately.
pub(super) fn playing(m: &dyn GuestMemory, unit: u32) -> R<bool> {
    let Some((_, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(false);
    };
    let offset = m.u16(body.addr + 0x12a)? as i16;
    if offset < 0 {
        return Ok(false);
    }
    let channel = body.addr + offset as u32;
    Ok(m.u32(channel + 0x68)? != NONE
        && m.u32(channel)? != NONE
        && m.u16(channel + 6)? != u16::MAX
        && m.u32(channel + 0x7c)? == 0x0e0000c2)
}

// EBX is a read-only firing pattern, preserved across 0x1fe620. Give
// only this call the authored burst range. H2 handles the weapon, aim,
// clip length, spread and its own firing state machine as usual.
const STRIDE: u32 = 0x50;
fn burst_pattern(
    m: &mut dyn GuestMemory,
    actor: u32,
    pattern: u32,
    scratch: u32,
) -> R<Option<u32>> {
    if actor & 0xffff >= 256 {
        return Ok(None);
    }
    let record = datum(m, ACTORS, actor, ACTOR_SIZE)?;
    let id = m.u32(record + 0x858)?;
    if id == NONE {
        return Ok(None);
    }
    let Some(a) = decode(m, id)? else {
        return Ok(None);
    };
    if a.kind != 7 || !(a.amount > 0.0) {
        return Ok(None);
    }
    let copy = scratch + (actor & 255) * STRIDE;
    let mut fields = [0; 0x40];
    m.read(pattern, &mut fields)?;
    let (_, rate) = clock(m)?;
    let seconds = ticks(a.amount, rate) as f32 / f32::from(rate);
    fields[0x20..0x24].copy_from_slice(&seconds.to_le_bytes());
    fields[0x24..0x28].copy_from_slice(&seconds.to_le_bytes());
    m.write(copy, &fields)?;
    m.set_u32(copy + 0x40, pattern)?;
    Ok(Some(copy))
}

pub(super) fn install(b: &mut feature::Builder) -> Result<(), feature::BuildError> {
    b.raw()
        .after(&points::AI_COMMAND_TARGETS, "resolve converted gaze", |c| {
            let mut memory = c.ctx.runtime().memory();
            let m: &mut dyn GuestMemory = &mut *memory;
            // The only argument is directly below ESP following ret 4.
            let actor = m.u32(c.cpu.esp.wrapping_sub(4)).map_err(control::fault)?;
            gaze_targets(m, actor).map_err(control::fault)
        });
    let scratch = b.guest_alloc(256 * STRIDE, runtime::guest::Permissions::RW)?;
    b.raw().before(
        &points::AI_FIRE_BURST,
        "authored command firing interval",
        move |c| {
            let mut m = c.ctx.runtime().memory();
            if let Some(copy) =
                burst_pattern(&mut *m, c.cpu.eax, c.cpu.ebx, scratch).map_err(control::fault)?
            {
                c.cpu.ebx = copy;
            }
            Ok(Flow::Continue)
        },
    );
    b.raw()
        .after(&points::AI_FIRE_BURST, "restore firing pattern", move |c| {
            let ptr = c.cpu.ebx;
            if (scratch..scratch + 256 * STRIDE).contains(&ptr) {
                c.cpu.ebx = c.ctx.runtime().memory().u32(ptr + 0x40)?;
            }
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests;
