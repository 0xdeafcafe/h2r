//! Execution of H1 command atoms with no exposed H2 command-script verb.
//! The converter encodes them as reserved bearings in the H2 direction
//! command. Only a compiler-generated command_script can select this ABI.
//! Both the actor and its blocking command remain owned by the AI scheduler.
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{Tags, objects};
use hooks::{Flow, points};

mod control;
mod motion;
mod performance;

type R<T> = Result<T, MemFault>;
const COMMANDS: u32 = 0x0050_2408;
const ACTORS: u32 = 0x004f_55f0;
const COMMAND_SIZE: u32 = 212;
const ACTOR_SIZE: u32 = 2184;

fn datum(m: &dyn GuestMemory, global: u32, id: u32, size: u32) -> R<u32> {
    Ok(m.u32(m.u32(global)? + 0x44)? + (id & 0xffff) * size)
}

#[derive(Clone, Copy)]
struct Atom {
    command: u32,
    scenario: u32,
    list: u32,
    kind: u16,
    operand: u16,
    amount: f32,
}

fn decode(m: &dyn GuestMemory, id: u32) -> R<Option<Atom>> {
    let command = datum(m, COMMANDS, id, COMMAND_SIZE)?;
    if m.u16(command + 4)? != 4 {
        return Ok(None);
    }
    let encoded = -m.f32(command + 12)?;
    if !(65536.0..16777216.0).contains(&encoded) || encoded.fract() != 0.0 {
        return Ok(None);
    }
    let bits = encoded as u32;
    let kind = ((bits >> 16) - 1) as u16;
    if !matches!(kind, 2 | 3 | 4 | 7 | 9 | 13 | 15 | 16 | 22) {
        return Ok(None);
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    let script = m.u32(command + 52)? as u16;
    if u32::from(script) >= m.u32(scenario + 0x1b8)? {
        return Ok(None);
    }
    let def = m.u32(scenario + 0x1bc)? + u32::from(script) * 40;
    if m.u16(def + 32)? != 5 || m.u32(def)? != u32::from_le_bytes(*b"h1_c") {
        return Ok(None);
    }
    let name = m.cstr(def, 31)?;
    let Some(list) = std::str::from_utf8(&name)
        .ok()
        .and_then(|n| n.strip_prefix("h1_cmd_"))
        .and_then(|n| n.parse::<u16>().ok())
    else {
        return Ok(None);
    };
    let amount = if kind == 22 && m.f32(command + 8)? == f32::MAX {
        m.f32(command + 40)?
    } else {
        m.f32(command + 8)?
    };
    Ok(Some(Atom {
        command,
        scenario,
        list: list.into(),
        kind,
        operand: bits as u16,
        amount,
    }))
}

fn point(m: &dyn GuestMemory, a: Atom) -> R<Option<[f32; 3]>> {
    if m.u32(a.scenario + 0x1d8)? == 0 {
        return Ok(None);
    }
    let data = m.u32(a.scenario + 0x1dc)?;
    if a.list >= m.u32(data)? {
        return Ok(None);
    }
    let set = m.u32(data + 4)? + a.list * 0x30;
    if u32::from(a.operand) >= m.u32(set + 0x20)? {
        return Ok(None);
    }
    let p = m.u32(set + 0x24)? + u32::from(a.operand) * 0x3c;
    Ok(Some(m.vec3(p + 0x20)?))
}

fn direction(from: [f32; 3], to: [f32; 3]) -> Option<[f32; 3]> {
    let [x, y, z] = std::array::from_fn(|i| to[i] - from[i]);
    let len = (x * x + y * y + z * z).sqrt();
    (len > 0.0 && len.is_finite()).then(|| [x / len, y / len, z / len])
}

fn put_vector(m: &mut dyn GuestMemory, addr: u32, xyz: [f32; 3]) -> R<()> {
    for (i, c) in xyz.into_iter().enumerate() {
        m.set_f32(addr + 4 * i as u32, c)?;
    }
    Ok(())
}

// H1 0x3bd39 tests the driver and gunner bits. H2's vehicle definitions
// use the same two bits at the start of each seat (0x274c10).
fn seat_matches(role: u16, flags: u32) -> bool {
    match role {
        0 => flags & 4 == 0,
        1 => flags & 8 != 0,
        2 => flags & 12 == 0,
        3 => flags & 4 != 0,
        _ => true,
    }
}

fn seats(m: &dyn GuestMemory, pos: [f32; 3], radius: f32, role: u16) -> R<Vec<(u32, u16)>> {
    let Some(objects) = objects::array(m)? else {
        return Ok(Vec::new());
    };
    let mut nearby = Vec::new();
    for (h, header) in objects.live(m, 2048)? {
        if header.kind(m)? != objects::VEHICLE || header.body(m)? == 0 {
            continue;
        }
        let body = objects::Body::at(header.body(m)?);
        let at = bots::vehicle::read::unit_position(m, body)?;
        let d: f32 = (0..3).map(|i| (at[i] - pos[i]).powi(2)).sum();
        if radius != 0.0 && d > radius * radius {
            continue;
        }
        nearby.push((d, h, body));
        if nearby.len() == 16 {
            break;
        } // H1 0x16bd99
    }
    nearby.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    for (_, id, body) in nearby {
        let Some(def) = body.definition(m)? else {
            continue;
        };
        let Some(tag) = Tags.entry(m, def)? else {
            continue;
        };
        let base = tag.address(m)?;
        if base == 0 {
            continue;
        }
        let count = m.i32(base + 0x1c8)?.clamp(0, 64) as u32;
        let entries = m.u32(base + 0x1cc)?;
        for i in 0..count {
            let flags = m.u32(entries + i * 0xb0)?;
            if flags & ((1 << 16) | (1 << 11)) == 0 && seat_matches(role, flags) {
                out.push((id.raw(), i as u16));
            }
        }
    }
    Ok(out)
}

async fn execute(
    cx: Cx,
    a: Atom,
    actor: u32,
    unit: u32,
    state: u32,
    scratch: u32,
) -> Result<Ret, EntryError> {
    let done = Ret::new(2, 16);
    let Some((_, body)) = cx.read(|m| objects::get(m, DatumHandle(unit)))? else {
        return Ok(done);
    };
    let origin = cx.read(|m| bots::vehicle::read::unit_position(m, body))?;
    match a.kind {
        3 | 22 => {
            let timed = a.kind == 22;
            let dir = if timed {
                None
            } else {
                let Some(to) = cx.read(|m| point(m, a))? else {
                    return Ok(done);
                };
                let Some(dir) = direction(origin, to) else {
                    return Ok(done);
                };
                Some(dir)
            };
            cx.write(|m| -> R<()> {
                put_vector(m, state + 24, origin)?;
                let mode = m.u16(a.command + 6)?;
                m.set_u16(state + 8, mode)?;
                let flags = m.u8(state + 3)?;
                m.set_u8(state + 3, (flags & !3) | if timed { 3 } else { 1 })?;
                if let Some(dir) = dir {
                    put_vector(m, state + 12, dir)?;
                }
                if timed {
                    let ticks_per_second = m.u16(m.u32(points::GAME_TIME.base)? + 2)?;
                    m.set_u16(
                        state,
                        (a.amount * f32::from(ticks_per_second))
                            .round_ties_even()
                            .clamp(0.0, i16::MAX as f32) as u16,
                    )?;
                    // Keep the source duration for other members of a swarm sharing this command.
                    m.set_f32(a.command + 40, a.amount)?;
                    m.set_f32(a.command + 8, f32::MAX)?;
                }
                Ok(())
            })?;
            if timed {
                // Same four-way facing-relative vector as H1 0x16a8a0.
                cx.call(GuestCall::new(0x25a130).eax(actor).edx(unit).ecx(state))
                    .await?;
            } else if cx.read(|m| m.u32(datum(m, ACTORS, actor, ACTOR_SIZE)? + 24))? == unit {
                cx.call(GuestCall::new(0x1f86a0).eax(actor)).await?;
            }
            Ok(Ret::new(0, 16))
        }
        2 => {
            let aligned = cx.write(|m| {
                let flight = performance::flying(m, a, unit)?;
                m.set_u8(state + 8, u8::from(flight))?;
                performance::face(m, a, actor, state)
            })?;
            Ok(Ret::new(if aligned { 2 } else { 0 }, 16))
        }
        4 => performance::begin_gaze(cx, a, actor, unit, state, scratch).await,
        13 => {
            let playing = cx.read(|m| performance::playing(m, unit))?;
            if playing {
                cx.write(|m| {
                    m.set_u32(state + 8, 0)?;
                    put_vector(m, state + 16, origin)
                })?;
            }
            Ok(Ret::new(if playing { 0 } else { 2 }, 16))
        }
        7 => {
            let finished = cx.write(|m| performance::shoot(m, a, actor, unit, state, true))?;
            Ok(Ret::new(if finished { 2 } else { 0 }, 16))
        }
        16 => {
            let sound = cx.read(|m| {
                crate::h1_conversation::dialogue::source_sound(m, unit, a.operand)
            })?;
            if sound != u32::MAX {
                crate::h1_conversation::play_spatial(&cx, scratch, unit, sound).await?;
            }
            Ok(done)
        }
        9 => {
            let candidates = cx.read(|m| seats(m, origin, a.amount, a.operand))?;
            for (vehicle, seat) in candidates {
                let ok = cx
                    .call(
                        GuestCall::new(0xc8200)
                            .eax(vehicle)
                            .edx(u32::from(seat))
                            .ecx(unit),
                    )
                    .await?;
                if ok.eax & 255 == 0 {
                    continue;
                }
                // H2 ai_vehicle_enter, 0x274d09: behavior 76, the vehicle,
                // seat, flags and two unbounded distances in a 64-byte order.
                cx.write(|m| -> R<()> {
                    m.fill(scratch, 64, 0)?;
                    m.set_u32(scratch + 28, vehicle)?;
                    m.set_u16(scratch + 32, seat)?;
                    m.set_u8(scratch + 34, 0x60)?;
                    m.set_f32(scratch + 40, f32::MAX)?;
                    m.set_f32(scratch + 44, f32::MAX)
                })?;
                let result = cx
                    .call(GuestCall::new(0x1a80e0).ebx(1).args([actor, 76, scratch]))
                    .await?;
                if result.eax & 255 != 0 {
                    break;
                }
            }
            Ok(done)
        }
        _ => unreachable!("handled during dispatch"),
    }
}

pub struct CommandAtoms;

impl hooks::Feature for CommandAtoms {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("h1-command-atoms", "Halo 1 AI command atoms")
            .doc("docs/halo1/ai.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        control::install(b);
        performance::install(b)?;
        motion::install(b)?;
        let scratch = b.guest_alloc(256 * 128, runtime::guest::Permissions::RW)?;
        b.raw().before(
            &points::AI_COMMAND_BEGIN,
            "execute converted command",
            move |c| {
                let id = c.arg_u32("command").unwrap_or(u32::MAX);
                if id == u32::MAX {
                    return Ok(Flow::Continue);
                }
                let Some(a) = decode(&*c.ctx.runtime().memory(), id).ok().flatten() else {
                    return Ok(Flow::Continue);
                };
                if a.kind == 15 {
                    // Both games implement this same eleven-value action switch.
                    // H2 exposes kind 10 internally at 0x259088.
                    let mut m = c.ctx.runtime().memory();
                    m.write_bytes(a.command + 4, &10u16.to_le_bytes(), 1)?;
                    m.write_bytes(a.command + 6, &a.operand.to_le_bytes(), 1)?;
                    return Ok(Flow::Continue);
                }
                let actor = c.arg_u32("actor").unwrap_or(u32::MAX);
                let unit = c.arg_u32("unit").unwrap_or(u32::MAX);
                let state = c.arg_u32("state").unwrap_or(0);
                if actor & 0xffff >= 256 || state == 0 {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| {
                    Box::pin(execute(
                        cx,
                        a,
                        actor,
                        unit,
                        state,
                        scratch + (actor & 255) * 128,
                    ))
                })
            },
        );
        b.raw().before(
            &points::AI_COMMAND_UPDATE,
            "update a converted command",
            |c| {
                let id = c.arg_u32("command").unwrap_or(u32::MAX);
                let state = c.arg_u32("state").unwrap_or(0);
                if id == u32::MAX || state == 0 {
                    return Ok(Flow::Continue);
                }
                let unit = c.arg_u32("unit").unwrap_or(u32::MAX);
                let actor = c.arg_u32("actor").unwrap_or(u32::MAX);
                let outcome = {
                    let mut m = c.ctx.runtime().memory();
                    match decode(&*m, id).map_err(control::fault)? {
                        Some(a) if a.kind == 2 => Some(u32::from(
                            performance::face(&mut *m, a, actor, state).map_err(control::fault)?,
                        )),
                        Some(a) if a.kind == 4 => Some(u32::from(
                            performance::update_gaze(&mut *m, a, state).map_err(control::fault)?,
                        )),
                        Some(a) if a.kind == 7 => Some(u32::from(
                            performance::shoot(&mut *m, a, actor, unit, state, false)
                                .map_err(control::fault)?,
                        )),
                        Some(a) if a.kind == 13 => Some(u32::from(
                            !performance::playing(&*m, unit).map_err(control::fault)?,
                        )),
                        Some(a) if a.kind == 22 && m.u16(state)? == 0 => Some(1),
                        _ => None,
                    }
                };
                if let Some(result) = outcome {
                    // The stock update returns 1 at 0x2597fa. Cleanup belongs
                    // to the caller and the other units keep their own countdown.
                    c.cpu.eax = result;
                    return c.ret(16).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_action_requires_a_generated_command_script() {
        let mut m = guest_layout::FlatMemory::new(0x004e_0000, 0x40000);
        let array = 0x004e_2000;
        let cmd = 0x004e_3000;
        let scenario = 0x004e_4000;
        let script = 0x004e_5000;
        m.set_u32(COMMANDS, array).unwrap();
        m.set_u32(array + 68, cmd).unwrap();
        m.set_u16(cmd + 4, 4).unwrap();
        m.set_f32(cmd + 12, -((16 * 65536 + 8) as f32)).unwrap();
        m.set_u32(points::SCENARIO.base, scenario).unwrap();
        m.set_u32(scenario + 0x1b8, 1).unwrap();
        m.set_u32(scenario + 0x1bc, script).unwrap();
        m.set_u16(script + 32, 5).unwrap();
        m.write(script, b"patrol\0").unwrap();
        assert!(decode(&m, 0).unwrap().is_none());
        m.write(script, b"h1_cmd_42\0").unwrap();
        let a = decode(&m, 0).unwrap().unwrap();
        assert_eq!((a.kind, a.operand, a.list), (15, 8, 42));
        m.set_f32(cmd + 12, -90.0).unwrap();
        assert!(decode(&m, 0).unwrap().is_none());
    }
    #[test]
    fn live_direction_includes_height_and_driver_selection_uses_seat_flags() {
        assert_eq!(
            direction([3.0, 4.0, 1.0], [3.0, 4.0, 2.0]),
            Some([0.0, 0.0, 1.0])
        );
        assert_eq!(direction([3.0; 3], [3.0; 3]), None);
        assert!(seat_matches(3, 4));
        assert!(!seat_matches(3, 8));
        assert!(seat_matches(1, 8));
        assert!(!seat_matches(2, 4 | 8));
    }
}
