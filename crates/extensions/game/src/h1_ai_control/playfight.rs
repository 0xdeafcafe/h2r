//! Halo 1 playfight modifiers at the native AI damage and firing transitions.

use guest_layout::{DatumHandle, entry::Cx, memory::{GuestMemory, MemFault}};
use hooks::{Builder, points};
use hs::h1_ai_slots::Registry;

use super::{actors, squad_control, state};

const WORDS: u32 = Registry::Playfight.words() as u32;
const DAMAGE_MULTIPLIER: f32 = 0.3;
const BURST_MULTIPLIER: f32 = 0.6;
const PAUSE_MULTIPLIER: f32 = 1.7;
const SPREAD_ADDITION: f32 = std::f32::consts::PI / 180.0;
const FIRING_TIMER: u32 = 0x700;
const BURST_SPREAD: u32 = 0x7bc;

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

pub(super) fn playing_actor(m: &dyn GuestMemory, handle: u32) -> R<Option<actors::Actor>> {
    let Some(base) = state::hook_base(m, Registry::Playfight)? else {
        return Ok(None);
    };
    let Some(actor) = actors::by_handle(m, DatumHandle(handle))? else {
        return Ok(None);
    };
    if let Some((at, bit)) = flag(m, actor.squad, base)?
        && m.u32(at)? & bit != 0
    {
        return Ok(Some(actor));
    }
    Ok(None)
}

pub(super) fn scale_damage(m: &mut dyn GuestMemory, actor: u32, multiplier: u32) -> R<()> {
    if playing_actor(m, actor)?.is_some() {
        let damage = m.f32(multiplier)?;
        m.set_f32(multiplier, damage * DAMAGE_MULTIPLIER)?;
    }
    Ok(())
}

pub(super) fn register_firing_hooks(builder: &mut Builder<'_>) {
    builder.raw().after(
        &points::AI_FIRE_BURST,
        "apply Halo 1 playfight burst and spread",
        |call| {
            if super::is_converted_campaign(call) {
                // The only caller keeps its actor handle in ESI.
                let handle = call.cpu.esi;
                let mut guest = call.ctx.runtime().memory();
                let _ = rescale_firing(&mut *guest, handle, true);
            }
            Ok(())
        },
    ).after_feature("h1-command-atoms");
    builder.raw().after(
        &points::AI_FIRE_PAUSE,
        "apply Halo 1 playfight firing pause",
        |call| {
            if super::is_converted_campaign(call) {
                let handle = call.cpu.esi;
                let mut guest = call.ctx.runtime().memory();
                let _ = rescale_firing(&mut *guest, handle, false);
            }
            Ok(())
        },
    );
}

fn rescale_firing(m: &mut dyn GuestMemory, handle: u32, burst: bool) -> R<()> {
    let Some(actor) = playing_actor(m, handle)? else {
        return Ok(());
    };
    let timer = actor.at + FIRING_TIMER;
    let scale = if burst {
        BURST_MULTIPLIER
    } else {
        PAUSE_MULTIPLIER
    };
    let ticks = m.u16(timer)? as i16;
    if ticks > 0 {
        m.set_u16(timer, scaled_ticks(ticks, scale) as u16)?;
    }
    if burst {
        let spread = actor.at + BURST_SPREAD;
        let radians = m.f32(spread)?;
        m.set_f32(spread, radians * 2.0 + SPREAD_ADDITION)?;
    }
    Ok(())
}

fn scaled_ticks(ticks: i16, scale: f32) -> i16 {
    (f32::from(ticks) * scale).round_ties_even() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playfight_shortens_bursts_and_lengthens_pauses() {
        assert_eq!(scaled_ticks(20, BURST_MULTIPLIER), 12);
        assert_eq!(scaled_ticks(20, PAUSE_MULTIPLIER), 34);
        assert_eq!(scaled_ticks(i16::MAX, PAUSE_MULTIPLIER), i16::MAX);
        assert!((SPREAD_ADDITION.to_degrees() - 1.0).abs() < f32::EPSILON);
    }
}
