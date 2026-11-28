//! Halo 2's native uncover behavior is used for Halo 1 playfights and
//! the one-shot maneuver override.

use guest_layout::{
    DatumHandle,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hooks::{Builder, Flow, points};

use super::{actors, playfight, preference, uncover_limits, uncover_targetless};

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
        "select Halo 1 uncover from presearch",
        |call| {
            if !super::is_converted_campaign(call) {
                return Ok(Flow::Continue);
            }
            let Some(actor) = call.arg_u32("actor") else {
                return Ok(Flow::Continue);
            };
            let (playfight, ordinary) = {
                let m = call.ctx.runtime().memory();
                (playfight_exception(&*m, actor).unwrap_or(false),
                 uncover_limits::allows(&*m, actor).unwrap_or(true))
            };
            if playfight {
                call.cpu.eax = u32::from(UNCOVER);
                return call.ret(8).map(Flow::Return);
            }
            if !ordinary {
                call.cpu.eax = u32::from(u16::MAX);
                return call.ret(8).map(Flow::Return);
            }
            Ok(Flow::Continue)
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
    GuestCall::new(INSTALL_BEHAVIOR).ebx(1).args(
        [actor, u32::from(UNCOVER), payload]
            .into_iter()
            .chain(uncover_targetless::payload()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playfight_exception_keeps_the_salted_target_and_command_checks() {
        use guest_layout::{datum::layout, testing::FakeGuest};

        const SCENARIO: u32 = 0x0080_0000;
        const DEFINITIONS: u32 = 0x0081_0000;
        const SQUADS: u32 = 0x0082_0000;
        const GROUPS: u32 = 0x0083_0000;
        const ACTOR: u32 = 0x0085_0000;
        const INTEREST: u32 = 0x0087_0000;
        const TRACKING: u32 = 0x0089_0000;
        const OBJECT: u32 = 0x008b_0000;
        const BODY: u32 = 0x008c_0000;
        const VM: u32 = 0x008d_0000;
        const GLOBALS: u32 = 0x008e_0000;
        const VALUES: u32 = GLOBALS + 0x41d * 8 + 4;
        const WORDS: u32 = hs::h1_ai_slots::Registry::Playfight.words() as u32;
        const ACTOR_HANDLE: u32 = 0x8100_0000;
        const INTEREST_HANDLE: u32 = 0x8200_0000;
        const TRACKING_HANDLE: u32 = 0x8300_0000;
        const OBJECT_HANDLE: u32 = 0x8400_0000;
        let mut guest = FakeGuest::new();
        for (global, row, handle) in [
            (actors::ACTORS, ACTOR, ACTOR_HANDLE),
            (preference::INTEREST, INTEREST, INTEREST_HANDLE),
            (preference::TRACKING, TRACKING, TRACKING_HANDLE),
            (points::OBJECTS.base, OBJECT, OBJECT_HANDLE),
        ] {
            let pool = row - 0x10000;
            guest
                .put_u32(global, pool)
                .put_u32(pool + layout::DATA, row)
                .put_u32(pool + layout::HIGH_WATER, 1)
                .put_u16(row, (handle >> 16) as u16);
        }
        guest
            .put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(SCENARIO + 0x1c0, WORDS + 1)
            .put_u32(SCENARIO + 0x1c4, DEFINITIONS)
            .put(
                DEFINITIONS + WORDS * 40,
                hs::h1_ai_slots::HOOK_STATE.as_bytes(),
            )
            .put_u32(SCENARIO + 0x158, 1)
            .put_u32(SCENARIO + 0x15c, GROUPS)
            .put_u32(SCENARIO + 0x160, 1)
            .put_u32(SCENARIO + 0x164, SQUADS)
            .put_u16(SQUADS + 0x26, 0)
            .put_u16(GROUPS + 0x20, u16::MAX)
            .put_u32(0x004f_9380, VM)
            .put_u32(VM + layout::DATA, GLOBALS)
            // Retail's 3072 global cells bound the registry.
            .put_u32(VM + layout::MAXIMUM_COUNT, 3072)
            .put_u32(VALUES + WORDS * 8, 0xffff_0000)
            .put_u32(VALUES, 1)
            .put_u32(ACTOR + 0x18, OBJECT_HANDLE)
            .put_u32(ACTOR + 0x30, 0)
            .put_u32(ACTOR + CURRENT_INTEREST, INTEREST_HANDLE)
            .put_u32(INTEREST + 0x14, TRACKING_HANDLE)
            .put_u32(OBJECT + 8, BODY)
            .put_u16(BODY + 0x10a, 0);
        assert_eq!(playfight_exception(&guest, ACTOR_HANDLE), Ok(true));
        guest.put_u16(ACTOR + BEHAVIOR_START, OBEY);
        assert_eq!(playfight_exception(&guest, ACTOR_HANDLE), Ok(false));
        guest
            .put_u16(ACTOR + BEHAVIOR_START, 0)
            .put_u16(TRACKING, 0x8500);
        assert_eq!(playfight_exception(&guest, ACTOR_HANDLE), Ok(false));
        guest.put_u16(TRACKING, 0x8300).put_u32(VALUES, 0);
        assert_eq!(playfight_exception(&guest, ACTOR_HANDLE), Ok(false));
    }

    #[test]
    fn maneuver_requests_one_native_uncover_action() {
        let call = request(0x8123_0010, 0x800100);
        assert_eq!(call.target, INSTALL_BEHAVIOR);
        assert_eq!(call.ebx, Some(1));
        assert_eq!(call.args[..3], [0x8123_0010, 35, 0x8000c0]);
        assert_eq!(call.args[3..], uncover_targetless::payload());
    }
}
