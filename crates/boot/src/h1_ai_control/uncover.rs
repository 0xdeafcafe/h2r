//! Halo 2's native uncover behavior is used for Halo 1 playfights and
//! the one-shot maneuver override.

use engine::{
    DatumHandle,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::{Record, objects},
};
use feature::{Builder, Flow, points};

use super::{NONE, actors, playfight, preference};

const UNCOVER: u16 = 35;
const OBEY: u16 = 4;
const VEHICLE: u16 = 76;
const VEHICLE_BOARD: u16 = 80;
const INSTALL_BEHAVIOR: u32 = 0x001a_80e0;
const CURRENT_INTEREST: u32 = 0x338;
const BLOCK_UNCOVER: u32 = 0x223;
const BEHAVIOR_DEPTH: u32 = 0x190;
const BEHAVIOR_START: u32 = 0x90;
const BEHAVIOR_SIZE: u32 = 0x40;

type R<T> = Result<T, MemFault>;

fn current_interest(m: &dyn GuestMemory, actor: actors::Actor) -> R<Option<u32>> {
    preference::datum(m, preference::INTEREST, m.u32(actor.at + CURRENT_INTEREST)?)
}

fn available(m: &dyn GuestMemory, actor: actors::Actor) -> R<bool> {
    let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
        return Ok(false);
    };
    if unit.damage_flags(m)? & 4 != 0 || m.u8(actor.at + BLOCK_UNCOVER)? != 0 {
        return Ok(false);
    }
    let depth = (m.u16(actor.at + BEHAVIOR_DEPTH)? as i16).min(4);
    for index in 0..=i32::from(depth) {
        let index = index as u32;
        let at = actor.at + BEHAVIOR_START + index * BEHAVIOR_SIZE;
        if matches!(m.u16(at)?, OBEY | VEHICLE | VEHICLE_BOARD | UNCOVER) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn register_playfight_hook(builder: &mut Builder<'_>) {
    builder.raw().before(
        &points::AI_PRESEARCH_UNCOVER,
        "allow native uncover while playfighting",
        |call| {
            if !super::is_converted_campaign(call) {
                return Ok(Flow::Continue);
            }
            let Some(actor) = call.arg_u32("actor") else {
                return Ok(Flow::Continue);
            };
            let allowed = {
                let m = call.ctx.runtime().memory();
                playfight_exception(&*m, actor).unwrap_or(false)
            };
            if !allowed {
                return Ok(Flow::Continue);
            }
            call.cpu.eax = u32::from(UNCOVER);
            call.ret(8).map(Flow::Return)
        },
    );
}

fn playfight_exception(m: &dyn GuestMemory, handle: u32) -> R<bool> {
    let Some(actor) = playfight::playing_actor(m, handle)? else {
        return Ok(false);
    };
    if !available(m, actor)? {
        return Ok(false);
    }
    let Some(interest) = current_interest(m, actor)? else {
        return Ok(false);
    };
    Ok(preference::datum(m, preference::TRACKING, m.u32(interest + 0x14)?)?.is_some())
}

pub(super) async fn maneuver(cx: &Cx, actor: actors::Actor) -> Result<(), EntryError> {
    if cx.read(|m| available(m, actor))? {
        cx.call(request(actor.handle, cx.entry().esp)).await?;
    }
    Ok(())
}

fn request(actor: u32, stack: u32) -> GuestCall {
    // The native installer initializes and copies the 64-byte behavior
    // record, using the same depth as the native scripted vehicle order.
    let payload = stack.wrapping_sub(BEHAVIOR_SIZE);
    GuestCall::new(INSTALL_BEHAVIOR)
        .ebx(1)
        .args([actor, u32::from(UNCOVER), payload].into_iter().chain([0; 16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maneuver_requests_one_native_uncover_action() {
        let call = request(0x8123_0010, 0x800100);
        assert_eq!(call.target, INSTALL_BEHAVIOR);
        assert_eq!(call.ebx, Some(1));
        assert_eq!(call.args[..3], [0x8123_0010, 35, 0x8000c0]);
        assert_eq!(call.args[3..], [0; 16]);
    }
}
