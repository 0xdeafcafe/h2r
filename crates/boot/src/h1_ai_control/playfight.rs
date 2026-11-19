//! Halo 1's playfight damage modifier in the native AI damage calculation.

use engine::{
    DatumHandle,
    entry::Cx,
    memory::{GuestMemory, MemFault},
};
use hs::h1_ai_slots::Registry;

use super::{actors, squad_control, state};

const WORDS: u32 = Registry::Playfight.words() as u32;
const DAMAGE_MULTIPLIER: f32 = 0.3;

type R<T> = Result<T, MemFault>;

fn flag(m: &dyn GuestMemory, reference: u32, base: u32) -> R<Option<(u32, u32)>> {
    let (Some(group), Some(start)) = (
        squad_control::encounter(m, reference)?,
        state::registry(m, base, WORDS)?,
    ) else {
        return Ok(None);
    };
    let group = u32::from(group);
    Ok((group < WORDS * 32).then_some((start + state::STRIDE * (group / 32), 1 << (group % 32))))
}

pub(super) fn set(cx: &Cx, reference: u32, enabled: bool, base: u32) -> R<()> {
    cx.write(|m| {
        if let Some((at, bit)) = flag(m, reference, base)? {
            let old = m.u32(at)?;
            m.set_u32(at, if enabled { old | bit } else { old & !bit })?;
        }
        Ok(())
    })
}

pub(super) fn scale_damage(m: &mut dyn GuestMemory, actor: u32, multiplier: u32) -> R<()> {
    let Some(base) = state::hook_base(m, Registry::Playfight)? else {
        return Ok(());
    };
    let Some(actor) = actors::by_handle(m, DatumHandle(actor))? else {
        return Ok(());
    };
    if let Some((at, bit)) = flag(m, actor.squad, base)?
        && m.u32(at)? & bit != 0
    {
        let damage = m.f32(multiplier)?;
        m.set_f32(multiplier, damage * DAMAGE_MULTIPLIER)?;
    }
    Ok(())
}
