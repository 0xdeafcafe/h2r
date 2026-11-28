//! Apply the custom channel's translation at the biped controller's velocity output.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Solid,
    Unobstructed,
}

impl Motion {
    fn from_modifier(modifier: u16) -> Option<Self> {
        match modifier {
            1 | 4 => Some(Self::Solid),
            2 | 5 => Some(Self::Unobstructed),
            _ => None,
        }
    }
}

// H1 0x4977f rotates the animation's XY velocity by the biped heading.
// Vertical movement stays in world Z even when the biped is pitched.
fn world_velocity(local: [f32; 3], heading: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = local;
    [
        heading[0] * x - heading[1] * y,
        heading[1] * x + heading[0] * y,
        z,
    ]
}

const SLOT: u32 = 32;

struct Step {
    motion: Motion,
    unit: u32,
    body: objects::Body,
    scratch: u32,
    state: u32,
}

fn step(m: &dyn GuestMemory, physics: u32, bank: u32) -> R<Option<Step>> {
    let unit = m.u32(physics + 4)?;
    let Some((objects::BIPED, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(None);
    };
    let actor = m.u32(body.addr + 0x12c)?;
    if actor & 0xffff >= 256 {
        return Ok(None);
    }
    let r = datum(m, ACTORS, actor, ACTOR_SIZE)?;
    let id = m.u32(r + 0x858)?;
    if id == u32::MAX {
        return Ok(None);
    }
    let Some(a) = decode(m, id)? else {
        return Ok(None);
    };
    if a.kind != 13 || !performance::playing(m, unit)? {
        return Ok(None);
    }
    let Some(motion) = Motion::from_modifier(a.operand) else {
        return Ok(None);
    };
    // 0x258c97 passes the actor's state to begin/update; 0x258c13
    // passes the swarm member's own command state instead.
    let state = if m.u8(r + 7)? == 0 {
        r + 0x864
    } else {
        let offset = m.u16(body.addr + 0x13a)? as i16;
        if offset < 0 || m.u32(body.addr + 0x134)? != 0 {
            return Ok(None);
        }
        body.addr + offset as u32 + 0x28
    };
    let scratch = bank + (actor & 255) * SLOT;
    if m.u32(state + 8)? != 0 {
        return Ok(None);
    }
    Ok(Some(Step {
        motion,
        unit,
        body,
        scratch,
        state,
    }))
}

async fn apply(cx: Cx, s: Step) -> Result<Ret, EntryError> {
    let regs = cx.entry();
    // Run H2's ground/flying/etc. state transition first. The reentry flag
    // belongs to this actor only and is released even if the call faults.
    cx.write(|m| m.set_u32(s.state + 8, 1))?;
    let stock = cx
        .call(
            GuestCall::new(points::CHARACTER_MOTION_STATE.base)
                .eax(regs.eax)
                .ecx(regs.ecx)
                .esi(regs.esi),
        )
        .await;
    cx.write(|m| m.set_u32(s.state + 8, 0))?;
    let stock = stock?;
    let channel = s.body.addr + u32::from(cx.read(|m| m.u16(s.body.addr + 0x12a))?);
    // H2's sample at the actual channel frame and playback rate. H2 already
    // applied its angular output to the facing at 0x1e5e28.
    cx.call(
        GuestCall::new(0x1c6d00)
            .ecx(channel)
            .edi(s.scratch + 4)
            .args([s.scratch + 16]),
    )
    .await?;
    let v =
        cx.read(|m| -> R<_> { Ok(world_velocity(m.vec3(s.scratch + 4)?, s.body.forward(m)?)) })?;
    if s.motion == Motion::Solid {
        // 0xe2b1d takes this vector through the ordinary Havok velocity path.
        cx.write(|m| put_vector(m, regs.esi + 12, v))?;
    } else {
        // 0x49d02 clears the source collision mask. Place the H2 body and
        // its physics proxy together, consuming the translation exactly once.
        let dest = s.scratch + 20;
        cx.write(|m| -> R<()> {
            let dt = m.f32(m.u32(points::GAME_TIME.base)? + 4)?;
            // The previous animation position is authoritative across contacts.
            let origin = m.vec3(s.state + 16)?;
            let next = std::array::from_fn(|i| origin[i] + v[i] * dt);
            put_vector(m, dest, next)?;
            put_vector(m, s.state + 16, next)?;
            put_vector(m, regs.esi + 12, [0.0; 3])
        })?;
        cx.call(
            GuestCall::new(0xb75a0)
                .eax(dest)
                .ecx(0)
                .edx(0)
                .args([s.unit, 0, 0]),
        )
        .await?;
    }
    Ok(Ret::new(stock.eax, 0))
}

pub(super) fn install(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    b.raw().before(
        &points::OBJECT_ANIMATION_BLEND,
        "six-tick command animation blend",
        |c| {
            let mut guest = c.ctx.runtime().memory();
            let m: &mut dyn GuestMemory = &mut *guest;
            // Only custom_animation's interpolated start from the executing
            // generated script. H1 0x3f321 passes six ticks to interpolation.
            if m.u32(c.cpu.esp).map_err(control::fault)? == 0x11b5cf
                && control::executing(m, c.cpu.esi).map_err(control::fault)?
            {
                m.set_f32(c.cpu.esp + 4, 6.0 / 30.0)
                    .map_err(control::fault)?;
            }
            Ok(Flow::Continue)
        },
    );
    let scratch = b.guest_alloc(256 * SLOT, runtime::guest::Permissions::RW)?;
    b.raw().before(
        &points::CHARACTER_MOTION_STATE,
        "command animation motion",
        move |c| {
            let s = step(&*c.ctx.runtime().memory(), c.cpu.ecx, scratch).map_err(control::fault)?;
            match s {
                Some(s) => c.answer_async(move |cx| Box::pin(apply(cx, s))),
                None => Ok(Flow::Continue),
            }
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_root_channel_drives_physics_once_per_step() {
        use guest_layout::entry::{self, EntryFuture, Regs, Step as Transfer};
        use std::sync::Mutex;
        let base = 0x500000;
        for motion in [Motion::Solid, Motion::Unobstructed] {
            let mut m = guest_layout::FlatMemory::new(base, 0x20000);
            let body = objects::Body::at(base + 0x1000);
            let state = base + 0x2000;
            let input = base + 0x3000;
            let output = base + 0x4000;
            let scratch = base + 0x5000;
            let unit = 0x80010003;
            m.set_u32(points::GAME_TIME.base, base + 0x6000).unwrap();
            m.set_f32(base + 0x6004, 1.0 / 30.0).unwrap();
            m.set_u16(body.addr + 0x12a, 0x500).unwrap();
            put_vector(&mut m, body.addr + 0x70, [0.0, 1.0, 0.0]).unwrap();
            // A contact displaced the physical body since the last step.
            put_vector(&mut m, state + 16, [3.0; 3]).unwrap();
            let regs = Regs {
                eax: input,
                esi: output,
                ..Regs::default()
            };
            let cx = Cx::new(Mutex::new(m), regs);
            let mut f: EntryFuture = Box::pin(apply(
                cx.clone(),
                Step {
                    motion,
                    unit,
                    body,
                    scratch,
                    state,
                },
            ));
            let Transfer::Call(stock) = entry::poll(&mut f, &cx) else {
                panic!("physics mode")
            };
            assert_eq!(stock.target, points::CHARACTER_MOTION_STATE.base);
            assert_eq!(cx.read(|m| m.u32(state + 8)).unwrap(), 1);
            entry::resume(&cx, regs);
            let Transfer::Call(sample) = entry::poll(&mut f, &cx) else {
                panic!("root sample")
            };
            assert_eq!(
                (sample.target, sample.ecx),
                (0x1c6d00, Some(body.addr + 0x500))
            );
            cx.write(|m| put_vector(m, sample.edi.unwrap(), [30.0, 60.0, -30.0]))
                .unwrap();
            entry::resume(&cx, regs);
            if motion == Motion::Unobstructed {
                let Transfer::Call(place) = entry::poll(&mut f, &cx) else {
                    panic!("placement")
                };
                assert_eq!((place.target, place.args), (0xb75a0, vec![unit, 0, 0]));
                assert_eq!(
                    cx.read(|m| m.vec3(place.eax.unwrap())).unwrap(),
                    [1.0, 4.0, 2.0]
                );
                entry::resume(&cx, regs);
            }
            assert_eq!(
                entry::poll(&mut f, &cx),
                Transfer::Done(Ok(Ret::new(input, 0)))
            );
            assert_eq!(
                cx.read(|m| m.vec3(output + 12)).unwrap(),
                match motion {
                    Motion::Solid => [-60.0, 30.0, -30.0],
                    Motion::Unobstructed => [0.0; 3],
                }
            );
            assert_eq!(cx.read(|m| m.u32(state + 8)).unwrap(), 0);
        }
    }

    #[test]
    fn interpolation_does_not_change_the_authored_physics_mode() {
        use Motion::*;
        assert_eq!(
            (0..6).map(Motion::from_modifier).collect::<Vec<_>>(),
            [
                None,
                Some(Solid),
                Some(Unobstructed),
                None,
                Some(Solid),
                Some(Unobstructed)
            ]
        );
        assert_eq!(
            world_velocity([1.5, -2.0, 0.75], [0.0, 1.0, 0.0]),
            [2.0, 1.5, 0.75]
        );
    }
}
