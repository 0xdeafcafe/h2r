//! A forced Halo 1 uncover can choose a firing position with no enemy.
//! Keep the native uncover action and use the engine's non-targeted
//! firing-position planner for this form of its movement request.

use guest_layout::{
    DatumHandle,
    entry::{Cx, EntryError, GuestCall, Ret},
    memory::{GuestMemory, MemFault},
};
use hooks::{Builder, Call, Flow, points};

use super::{actors, is_converted_campaign, uncover_limits};

const MARKER: u32 = 0x4831_554e;
const MARKER_OFFSET: u32 = 0x3c;
// The normal uncover continuation at 0x1b8c60 finishes on this byte.
// The non-targeted planner at 0x1b21a3 also clears it on path adoption.
const FINISHED: u32 = 0x0c;
const FIXED_MASK: u32 = 0x0d;
const MASK: u32 = 0x0e;
const MODE: u32 = 0x10;
const DESTINATION: u32 = 0x14;
const PLANNING_TICK: u32 = 0x40;
const PATH: u32 = 0x418;
const PATH_STATE: u32 = 0x504;
const PATH_COMPLETE: u16 = 2;
const NO_POSITION: u32 = 0x0047_0fa0;
const PLAN_POSITION: u32 = 0x001b_1f70;

pub(super) fn payload() -> [u32; 16] {
    let mut payload = [0; 16];
    payload[(MARKER_OFFSET / 4) as usize] = MARKER;
    payload
}

fn marked(call: &mut Call<'_>) -> Option<(u32, u32)> {
    if !is_converted_campaign(call) {
        return None;
    }
    let actor = call.arg_u32("actor")?;
    let record = call.arg_u32("record")?;
    if record == 0 {
        return None;
    }
    let memory = call.ctx.runtime().memory();
    let marked = memory.u32(record + MARKER_OFFSET).ok()? == MARKER;
    marked.then_some((actor, record))
}

pub(super) fn register(b: &mut Builder<'_>) {
    b.raw()
        .before(&points::AI_UNCOVER_ENTER, "apply Halo 1 uncover eligibility", |call| {
            let Some((_, record)) = marked(call) else {
                if is_converted_campaign(call) && let Some(actor) = call.arg_u32("actor") {
                    let allowed = {
                        let m = call.ctx.runtime().memory();
                        uncover_limits::allows(&*m, actor).unwrap_or(true)
                    };
                    if !allowed {
                        call.cpu.eax = 0;
                        return call.ret(8).map(Flow::Return);
                    }
                }
                return Ok(Flow::Continue);
            };
            {
                let mut m = call.ctx.runtime().memory();
                m.set_u8(record + FINISHED, 0)?;
            }
            call.cpu.eax = 1;
            call.ret(8).map(Flow::Return)
        });
    b.raw().before(
        &points::AI_UNCOVER_BEGIN,
        "choose a targetless uncover position",
        |call| {
            let Some((actor, record)) = marked(call) else {
                return Ok(Flow::Continue);
            };
            call.answer_async(move |cx| Box::pin(begin(cx, actor, record)))
        },
    );
    b.raw().before(
        &points::AI_UNCOVER_UPDATE,
        "track targetless uncover completion",
        |call| {
            let Some((actor, record)) = marked(call) else {
                return Ok(Flow::Continue);
            };
            {
                let mut m = call.ctx.runtime().memory();
                let _ = update(&mut *m, actor, record);
            }
            call.ret(8).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::AI_UNCOVER_CONTROL,
        "keep targetless uncover free of an aim target",
        |call| {
            if marked(call).is_none() {
                return Ok(Flow::Continue);
            }
            call.ret(8).map(Flow::Return)
        },
    );
}

async fn begin(cx: Cx, handle: u32, record: u32) -> Result<Ret, EntryError> {
    let Some(actor) = cx.read(|m| actors::by_handle(m, DatumHandle(handle)))? else {
        return Ok(Ret::new(0, 8));
    };
    let adopted = drive_position(&cx, actor, record).await?;
    Ok(Ret::new(u32::from(adopted), 8))
}

async fn drive_position(cx: &Cx, actor: actors::Actor, record: u32) -> Result<bool, EntryError> {
    let (scheduled, no_position) = cx.write(|m| {
        let scheduled = m.u8(actor.at + PLANNING_TICK)?;
        let no_position = m.u32(NO_POSITION)?;
        // These are the native guard planner's request fields. Force a
        // new query in the squad's active areas, with no prior destination.
        m.set_u8(record + FINISHED, 1)?;
        m.set_u8(record + FIXED_MASK, 1)?;
        m.set_u16(record + MASK, u16::MAX)?;
        m.set_u16(record + MODE, 3)?;
        m.set_u32(record + DESTINATION, no_position)?;
        m.set_u8(actor.at + PLANNING_TICK, 1)?;
        Ok::<_, MemFault>((scheduled, no_position))
    })?;
    let result = cx
        .call(GuestCall::new(PLAN_POSITION).args([actor.handle, record]))
        .await;
    cx.write(|m| m.set_u8(actor.at + PLANNING_TICK, scheduled))?;
    let result = result?;
    cx.write(|m| {
        let path = m.u32(actor.at + PATH)?;
        let adopted = result.eax as u8 != 0 && m.u8(record + FINISHED)? == 0 && path != no_position;
        m.set_u8(record + FINISHED, u8::from(!adopted))?;
        m.set_u32(record + DESTINATION, path)?;
        Ok::<_, MemFault>(adopted)
    })
    .map_err(Into::into)
}

fn update(m: &mut dyn GuestMemory, actor: u32, record: u32) -> Result<(), MemFault> {
    if m.u8(record + FINISHED)? == 0 {
        let complete = match actors::by_handle(m, DatumHandle(actor))? {
            Some(actor) => {
                m.u16(actor.at + PATH_STATE)? == PATH_COMPLETE
                    || m.u32(actor.at + PATH)? != m.u32(record + DESTINATION)?
            }
            None => true,
        };
        m.set_u8(record + FINISHED, u8::from(complete))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{entry::{EntryFuture, Regs, testing}, testing::FakeGuest};

    #[test]
    fn a_targetless_request_installs_a_real_position_and_restores_the_tick_gate() {
        const ACTOR: actors::Actor = actors::Actor {
            at: 0x800000,
            handle: 0x8100_0000,
            unit: 0x8200_0000,
            squad: 0,
        };
        const RECORD: u32 = 0x801000;
        let mut mem = FakeGuest::new();
        mem.put_u32(NO_POSITION, u32::MAX)
            .put_u32(ACTOR.at + PATH, u32::MAX)
            .put_u32(RECORD + MARKER_OFFSET, MARKER);
        let run = testing::run(
            &|cx| -> EntryFuture {
                Box::pin(async move {
                    Ok(Ret::new(
                        u32::from(drive_position(&cx, ACTOR, RECORD).await?),
                        8,
                    ))
                })
            },
            mem,
            Regs::default(),
            |call, m| {
                assert_eq!(call.target, PLAN_POSITION);
                assert_eq!(call.args, [ACTOR.handle, RECORD]);
                assert_eq!(m.u8(ACTOR.at + PLANNING_TICK), Ok(1));
                assert_eq!(m.u8(RECORD + FIXED_MASK), Ok(1));
                assert_eq!(m.u16(RECORD + MASK), Ok(u16::MAX));
                m.set_u32(ACTOR.at + PATH, 0x20001).unwrap();
                m.set_u8(RECORD + FINISHED, 0).unwrap();
                Regs {
                    eax: 1,
                    ..Default::default()
                }
            },
        );
        assert_eq!(run.result, Ok(Ret::new(1, 8)));
        assert_eq!(run.guest.u32(RECORD + DESTINATION), Ok(0x20001));
        assert_eq!(run.guest.u8(ACTOR.at + PLANNING_TICK), Ok(0));
        assert_eq!(run.guest.u32(RECORD + MARKER_OFFSET), Ok(MARKER));
    }
}
