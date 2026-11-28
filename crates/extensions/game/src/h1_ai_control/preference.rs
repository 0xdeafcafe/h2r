//! The script preference belongs to the observing actor. Halo 2's own
//! score still establishes whether a perceived object is a candidate.

use guest_layout::{DatumHandle, datum::DatumArray, entry::Cx, memory::{GuestMemory, MemFault}};
use hs::h1_ai_slots::{Operation, Registry};

use super::{NONE, actors, state};

const TARGETS: Registry = Registry::TargetPreference;
const WORDS: u32 = TARGETS.words() as u32;
const PLAYERS: u32 = 0x3fff_ffff;
pub(super) const INTEREST: u32 = 0x0050_2418;
const PROPS: u32 = 0x0050_241c;
pub(super) const TRACKING: u32 = 0x0050_2414;
const PROP_TYPE_TABLE: u32 = 0x0047_0f16;
const PREFERENCE_SCORE: f32 = 10.0;

type R<T> = Result<T, MemFault>;

fn row(start: u32, actor: DatumHandle) -> Option<u32> {
    let slot = u32::from(actor.index());
    (slot < WORDS / 2).then_some(start + slot * 2 * state::STRIDE)
}

pub(super) fn set(cx: &Cx, operation: Operation, args: &[u32]) -> R<()> {
    let target = match operation {
        Operation::PreferAi => args[1],
        Operation::PreferPlayer => PLAYERS,
        Operation::ClearPreference => NONE,
        _ => unreachable!(),
    };
    cx.write(|m| {
        let Some(start) = state::registry(m, args[args.len() - 1], WORDS)? else {
            return Ok(());
        };
        for actor in actors::selected(m, args[0])? {
            if let Some(at) = row(start, DatumHandle(actor.handle)) {
                m.set_u32(at, if target == NONE { NONE } else { actor.handle })?;
                m.set_u32(at + state::STRIDE, target)?;
            }
        }
        Ok(())
    })
}

pub(super) fn datum(m: &dyn GuestMemory, pool: u32, handle: u32) -> R<Option<u32>> {
    let at = m.u32(pool)?;
    if at == 0 {
        return Ok(None);
    }
    DatumArray::at(at).try_get(m, DatumHandle(handle))
}

pub(super) fn score_bonus(m: &dyn GuestMemory, stack_after_return: u32) -> R<f32> {
    let Some(base) = state::hook_base(m, TARGETS)? else {
        return Ok(0.0);
    };
    let Some(start) = state::registry(m, base, WORDS)? else {
        return Ok(0.0);
    };
    let actor = DatumHandle(m.u32(stack_after_return.wrapping_sub(8))?);
    let Some(at) = row(start, actor) else {
        return Ok(0.0);
    };
    if actor.raw() == NONE || actor.raw() != m.u32(at)? {
        return Ok(0.0);
    }
    let selector = m.u32(at + state::STRIDE)?;
    let interest = m.u32(stack_after_return.wrapping_sub(4))?;
    let Some(interest) = datum(m, INTEREST, interest)? else {
        return Ok(0.0);
    };
    let Some(prop) = datum(m, PROPS, m.u32(interest + 8)?)? else {
        return Ok(0.0);
    };
    let matches = if selector == PLAYERS {
        m.u8(prop + 0x25)? != 0
    } else if let Some(candidate) = actors::by_handle(m, DatumHandle(m.u32(prop + 0x1c)?))? {
        actors::matches_reference(m, candidate, selector)?
    } else {
        false
    };
    if !matches {
        return Ok(0.0);
    }
    // Native 0x25d690 picks the candidate's global or actor-specific
    // perception record. The stock preferred-target bit contributes the
    // same ten points at 0x2660b9, so combine the hints as a single bonus.
    let kind = u32::from(m.u16(interest + 0x18)?);
    let mut native = prop + 0xbf;
    if kind < 16
        && m.u16(PROP_TYPE_TABLE + kind * 16)? != 1
        && let Some(tracker) = datum(m, TRACKING, m.u32(interest + 0x14)?)?
    {
        native = tracker + 0x6b;
    }
    Ok(if m.u8(native)? == 0 {
        PREFERENCE_SCORE
    } else {
        0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{datum::layout, testing::FakeGuest};

    #[test]
    fn preferred_player_bonus_is_local_to_the_salted_observer() {
        const SCNR: u32 = 0x800000;
        const DEFINITIONS: u32 = 0x810000;
        const GLOBAL_POOL: u32 = 0x820000;
        const DATA: u32 = 0x830000;
        const INTEREST_POOL: u32 = 0x840000;
        const INTEREST_ROW: u32 = 0x850000;
        const PROP_POOL: u32 = 0x860000;
        const PROP_ROW: u32 = 0x870000;
        const STACK: u32 = 0x880100;
        let mut guest = FakeGuest::new();
        guest
            .put_u32(super::super::points::SCENARIO.base, SCNR)
            .put_u32(SCNR + 0x1c0, WORDS + 1)
            .put_u32(SCNR + 0x1c4, DEFINITIONS)
            .put(
                DEFINITIONS + WORDS * 40,
                hs::h1_ai_slots::HOOK_STATE.as_bytes(),
            )
            .put_u32(0x004f_9380, GLOBAL_POOL)
            .put_u32(GLOBAL_POOL + layout::DATA, DATA)
            // Retail's 3072 global cells bound the registry.
            .put_u32(GLOBAL_POOL + layout::MAXIMUM_COUNT, 3072)
            .put_u32(DATA + (0x41d + WORDS) * 8 + 4, 0xffff)
            .put_u32(DATA + 0x41d * 8 + 4, 0x8200_0000)
            .put_u32(DATA + 0x41d * 8 + 12, PLAYERS)
            .put_u32(INTEREST, INTEREST_POOL)
            .put_u32(INTEREST_POOL + layout::DATA, INTEREST_ROW)
            .put_u32(INTEREST_POOL + layout::HIGH_WATER, 1)
            .put_u16(INTEREST_ROW, 0x8300)
            .put_u32(INTEREST_ROW + 8, 0x8400_0000)
            .put_u32(INTEREST_ROW + 0x14, NONE)
            .put_u32(PROPS, PROP_POOL)
            .put_u32(PROP_POOL + layout::DATA, PROP_ROW)
            .put_u32(PROP_POOL + layout::HIGH_WATER, 1)
            .put_u16(PROP_ROW, 0x8400)
            .put_u8(PROP_ROW + 0x25, 1)
            .put_u16(PROP_TYPE_TABLE, 1)
            .put_u32(STACK - 4, 0x8300_0000)
            .put_u32(STACK - 8, 0x8200_0000);
        assert_eq!(score_bonus(&guest, STACK), Ok(10.0));
        guest.put_u8(PROP_ROW + 0xbf, 1);
        assert_eq!(score_bonus(&guest, STACK), Ok(0.0));
        guest
            .put_u8(PROP_ROW + 0xbf, 0)
            .put_u32(STACK - 8, 0x8201_0000);
        assert_eq!(score_bonus(&guest, STACK), Ok(0.0));
    }
}
