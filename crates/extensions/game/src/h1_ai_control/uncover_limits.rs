//! Ordinary uncover gates from Halo 1 0x163c50. The recent body damage
//! accumulator uses the source's 30 Hz aging rule.

use guest_layout::{DatumHandle, memory::{GuestMemory, MemFault}, view::{Record, objects}};
use engine::campaign_squad_pool;
use std::cmp::Ordering;

use hooks::{Builder, Flow, points};

use super::{NONE, actors, contact, is_converted_campaign, playfight, state};

const CHARACTER: u32 = 0x54;
const UNIT_ACTOR: u32 = 0x12c;
const FLAG_ALERT: u32 = 0x04;
const ALERTED: u32 = 1;
const GUARD: u32 = 2;
const BASELINE_SET: u32 = 4;
const DAMAGE: u32 = 0x08;
const HIT_TIME: u32 = 0x0c;
const FOCUS_HANDLE: u32 = 0x10;
const FOCUS_TIME: u32 = 0x14;
const APPLIED_BODY_DAMAGE: u32 = 0x28;
pub(super) use campaign_squad_pool::ACTOR_LAST_REPORT as REPORT_LATCH;
const DAMAGE_GRACE: u32 = 60;
const DAMAGE_DECAY: f32 = 1.0 / 60.0;
const NO_HIT: u32 = u32::MAX;

type R<T> = Result<T, MemFault>;

pub(super) fn hex(m: &dyn GuestMemory, at: u32) -> R<Option<u32>> {
    let mut raw = [0; 8];
    m.read(at, &mut raw)?;
    Ok(std::str::from_utf8(&raw)
        .ok()
        .and_then(|s| u32::from_str_radix(s, 16).ok()))
}

/// The perception table: a header naming the character and weapon row
/// counts, then the character rows (two floats each) and the weapon rows
/// (one report loudness each). Tables written before the weapon section
/// existed carry a one-word header and character rows only. The high
/// bit of the weapon count distinguishes the two-section format.
pub(super) struct PerceptionTable {
    start: u32,
    characters: u32,
    weapons: u32,
    /// A one-word header, from a table without the weapon section.
    legacy: bool,
}

pub(super) fn perception_table(m: &dyn GuestMemory) -> R<Option<PerceptionTable>> {
    let Some(index) = state::perception_global(m)? else {
        return Ok(None);
    };
    let offset = m.u32(state::addr(m, index)?)?;
    let Some((start, len)) = state::script_strings(m, offset)? else {
        return Ok(None);
    };
    if len < 8 {
        return Ok(None);
    }
    let Some(characters) = hex(m, start)? else {
        return Ok(None);
    };
    if characters > (len - 8) / 16 {
        return Ok(None);
    }
    // The second word's high bit marks the two-section table; a table
    // from before the weapon section exists has a one-word header whose
    // next bytes are the first row's floats and never carry the bit (an
    // authored threshold is a small non-negative float). Past the pool's
    // end the table simply has no second word.
    let second = if len >= 16 { hex(m, start + 8)? } else { None };
    let weapons = second
        .filter(|word| word & 0x8000_0000 != 0)
        .map(|word| word & 0x7fff_ffff);
    match weapons {
        Some(weapons) => {
            if characters > (len - 16) / 16
                || weapons > (len - 16 - characters * 16) / 8
            {
                return Ok(None);
            }
            Ok(Some(PerceptionTable {
                start,
                characters,
                weapons,
                legacy: false,
            }))
        }
        None => Ok(Some(PerceptionTable {
            start,
            characters,
            weapons: 0,
            legacy: true,
        })),
    }
}

impl PerceptionTable {
    /// The guest address of character row `row` (two floats).
    pub(super) fn character_row(&self, row: u32) -> u32 {
        self.rows() + 16 * row
    }

    /// The guest address of weapon row `row` (one float).
    pub(super) fn weapon_row(&self, row: u32) -> u32 {
        self.rows() + 16 * self.characters + 8 * row
    }

    pub(super) fn weapons(&self) -> u32 {
        self.weapons
    }

    pub(super) fn characters(&self) -> u32 {
        self.characters
    }

    fn rows(&self) -> u32 {
        self.start + if self.legacy { 8 } else { 16 }
    }
}

fn thresholds(m: &dyn GuestMemory, actor: actors::Actor) -> R<Option<[f32; 2]>> {
    let Some(table) = perception_table(m)? else {
        return Ok(None);
    };
    let scenario = m.u32(points::SCENARIO.base)?;
    let Some(first) = m.u32(scenario + 0x1c8)?.checked_sub(table.characters) else {
        return Ok(None);
    };
    let references = m.u32(scenario + 0x1cc)?;
    let wanted = m.u32(actor.at + CHARACTER)?;
    for row in 0..table.characters {
        let reference = references + 8 * (first + row);
        if m.u32(reference)? == u32::from_be_bytes(*b"char") && m.u32(reference + 4)? == wanted {
            let at = table.character_row(row);
            return Ok(hex(m, at)?
                .zip(hex(m, at + 8)?)
                .map(|(time, hit)| [f32::from_bits(time), f32::from_bits(hit)]));
        }
    }
    Ok(None)
}

fn ordinary([seconds, damage]: [f32; 2], focused: contact::Contact, now: u32, recent: f32) -> bool {
    // NaN counts as not positive, as in the source's x87 compare.
    let time_ok = seconds.partial_cmp(&0.0) != Some(Ordering::Greater)
        || (focused.level < 7
            && focused.recognized_at.is_none_or(|seen| {
                let ticks = (f64::from(seconds) * 30.0) as i32 as i16;
                (now as i32) >= (seen as i32).wrapping_add(i32::from(ticks))
            }));
    // The source's x87 branch only rejects an ordered less-than result.
    time_ok
        && (damage.partial_cmp(&0.0) != Some(Ordering::Greater)
            || recent.partial_cmp(&damage) != Some(Ordering::Less))
}

pub(super) fn actor_row(m: &dyn GuestMemory, actor: actors::Actor) -> R<Option<u32>> {
    campaign_squad_pool::actor_state(m, DatumHandle(actor.handle).index())
}

pub(super) fn reset_row(m: &mut dyn GuestMemory, row: u32, actor: actors::Actor) -> R<()> {
    if m.u32(row)? != actor.handle {
        m.set_u32(row, actor.handle)?;
        m.set_u32(row + FLAG_ALERT, 0)?;
        m.set_f32(row + DAMAGE, 0.0)?;
        m.set_u32(row + HIT_TIME, NO_HIT)?;
        m.set_u32(row + FOCUS_HANDLE, NONE)?;
        m.set_u32(row + FOCUS_TIME, NONE)?;
        // The report latch at +0x18 belongs to this actor's unit, like the
        // alert bits Halo 1 keeps on the unit object.
        m.set_u32(row + REPORT_LATCH, u32::MAX)?;
    }
    Ok(())
}

pub(super) fn timer_starts(
    m: &mut dyn GuestMemory,
    actor: actors::Actor,
    focused: contact::Contact,
    baseline: u16,
) -> R<bool> {
    let Some(row) = actor_row(m, actor)? else {
        return Ok(false);
    };
    reset_row(m, row, actor)?;
    let old = m.u32(row + FLAG_ALERT)?;
    let mut flags = old;
    // Authored guards start alerted. For other actors the source alert
    // transition is driven by a perception report or damage.
    if flags & BASELINE_SET == 0 {
        flags |= BASELINE_SET;
        if baseline == 2 {
            flags |= GUARD | ALERTED;
        }
    }
    if focused.alarm {
        flags |= ALERTED;
    }
    if flags != old {
        m.set_u32(row + FLAG_ALERT, flags)?;
    }
    Ok(focused.starts_timer(flags & ALERTED != 0, if flags & GUARD != 0 { 2 } else { 0 }))
}

fn damage_at(m: &dyn GuestMemory, row: u32, now: u32) -> R<f32> {
    let hit = m.u32(row + HIT_TIME)?;
    if hit == NO_HIT {
        return Ok(0.0);
    }
    let age = now.wrapping_sub(hit);
    let steps = age.saturating_add(1).saturating_sub(DAMAGE_GRACE);
    let mut amount = m.f32(row + DAMAGE)?;
    for _ in 0..steps {
        amount -= DAMAGE_DECAY;
        if amount <= 0.0 {
            return Ok(0.0);
        }
    }
    Ok(amount)
}

pub(super) fn allows(m: &dyn GuestMemory, handle: u32) -> R<bool> {
    let Some(actor) = actors::by_handle(m, DatumHandle(handle))? else {
        return Ok(true);
    };
    let Some(rule) = thresholds(m, actor)? else {
        return Ok(true);
    };
    if playfight::playing_actor(m, handle)?.is_some() {
        return Ok(true);
    }
    let Some((tick, rate)) = state::clock(m)? else {
        return Ok(true);
    };
    let now = state::authored_tick(tick, rate);
    let mut focus = contact::focused(m, actor, rate)?;
    let mut damage = 0.0;
    if let Some(row) = actor_row(m, actor)?
        && m.u32(row)? == actor.handle
    {
        damage = damage_at(m, row, now)?;
        let interest = m.u32(actor.at + contact::FOCUS)?;
        let time = m.u32(row + FOCUS_TIME)?;
        if focus.recognized_at.is_none() && interest == m.u32(row + FOCUS_HANDLE)? && time != NONE {
            focus.recognized_at = Some(time);
        }
    }
    Ok(ordinary(rule, focus, now, damage))
}

fn observe_focus(m: &mut dyn GuestMemory, actor: actors::Actor, rate: u32) -> R<()> {
    let Some(row) = actor_row(m, actor)? else {
        return Ok(());
    };
    reset_row(m, row, actor)?;
    let interest = m.u32(actor.at + contact::FOCUS)?;
    let focused = contact::focused(m, actor, rate)?;
    remember_focus(m, row, interest, focused.recognized_at)?;
    if focused.alarm {
        let flags = m.u32(row + FLAG_ALERT)?;
        m.set_u32(row + FLAG_ALERT, flags | ALERTED)?;
    }
    Ok(())
}

fn remember_focus(m: &mut dyn GuestMemory, row: u32, interest: u32, sample: Option<u32>) -> R<()> {
    if interest != m.u32(row + FOCUS_HANDLE)? {
        m.set_u32(row + FOCUS_HANDLE, interest)?;
        m.set_u32(row + FOCUS_TIME, NONE)?;
    }
    if let Some(time) = sample {
        m.set_u32(row + FOCUS_TIME, time)?;
    }
    Ok(())
}

fn hit_actor(m: &dyn GuestMemory, unit: u32) -> R<Option<actors::Actor>> {
    let Some((kind, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(None);
    };
    if !matches!(kind, objects::BIPED | objects::VEHICLE) {
        return Ok(None);
    }
    let actor = m.u32(body.addr() + UNIT_ACTOR)?;
    Ok(actors::by_handle(m, DatumHandle(actor))?.filter(|actor| actor.unit == unit))
}

fn record_hit(m: &mut dyn GuestMemory, unit: u32, injury: f32) -> R<()> {
    let Some(actor) = hit_actor(m, unit)? else {
        return Ok(());
    };
    let Some(row) = actor_row(m, actor)? else {
        return Ok(());
    };
    if let Some((tick, rate)) = state::clock(m)? {
        reset_row(m, row, actor)?;
        let now = state::authored_tick(tick, rate);
        let recent = damage_at(m, row, now)?;
        m.set_f32(row + DAMAGE, (recent + injury).min(1.0))?;
        m.set_u32(row + HIT_TIME, now)?;
        let flags = m.u32(row + FLAG_ALERT)?;
        m.set_u32(row + FLAG_ALERT, flags | ALERTED)?;
    }
    Ok(())
}

pub(super) fn register(b: &mut Builder<'_>) {
    b.raw().after(
        &points::AI_SELECT_INTEREST,
        "remember Halo 1 target recognition",
        |call| {
            if is_converted_campaign(call) {
                let mut guest = call.ctx.runtime().memory();
                if contact::enabled(&*guest).unwrap_or(false)
                    && let Ok(handle) = guest.u32(call.cpu.esp.wrapping_sub(4))
                    && let Ok(Some(actor)) = actors::by_handle(&*guest, DatumHandle(handle))
                    && let Ok(Some((_, rate))) = state::clock(&*guest)
                {
                    let _ = observe_focus(&mut *guest, actor, rate);
                }
            }
            Ok(())
        },
    );
    b.raw().before(
        &points::AI_UNCOVER_RANK,
        "rank ordinary Halo 1 uncover",
        |call| {
            if is_converted_campaign(call)
                && let Some(actor) = call.arg_u32("actor")
            {
                let denied = {
                    let guest = call.ctx.runtime().memory();
                    !allows(&*guest, actor).unwrap_or(true)
                };
                if denied {
                    call.cpu.eax = 0;
                    return call.ret(4).map(Flow::Return);
                }
            }
            Ok(Flow::Continue)
        },
    );
    b.raw().after(
        &points::BODY_DAMAGE,
        "record Halo 1 actor body damage",
        |call| {
            if is_converted_campaign(call) {
                let mut guest = call.ctx.runtime().memory();
                // The caller zeroes the packet and body damage stores its
                // applied normalized amount at +0x28. EDI still owns the
                // packet and the popped first argument is at ESP-12.
                if let Ok(injury) = guest.f32(call.cpu.edi + APPLIED_BODY_DAMAGE)
                    && injury > 0.0
                    && contact::enabled(&*guest).unwrap_or(false)
                    && let Ok(unit) = guest.u32(call.cpu.esp.wrapping_sub(12))
                {
                    let _ = record_hit(&mut *guest, unit, injury);
                }
            }
            Ok(())
        },
    )
    // Inside the conversations' damage dialogue check: the hit is on the
    // actor's record before the dialogue reacts to it.
    .after_feature("h1-conversations");
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn ordinary_uncover_keeps_both_source_threshold_boundaries() {
        let rule = [0.5, 0.1];
        let contact = contact::Contact {
            level: 5,
            recognized_at: Some(40),
            alarm: false,
        };
        assert!(!ordinary(rule, contact, 54, 0.10));
        assert!(!ordinary(rule, contact, 55, 0.09));
        assert!(ordinary(rule, contact, 55, 0.10));
        let danger = contact::Contact {
            level: 7,
            recognized_at: None,
            alarm: false,
        };
        assert!(!ordinary(rule, danger, 500, 1.0));
        assert!(ordinary([0.0, 0.1], danger, 500, 0.1));
        assert!(ordinary(
            rule,
            contact::Contact {
                level: 0,
                recognized_at: None,
                alarm: false
            },
            0,
            0.1
        ));
    }

    #[test]
    fn damage_ages_on_the_sixtieth_source_tick_and_floors_at_zero() {
        let row = 0x4000;
        let mut guest = FakeGuest::new();
        guest
            .put_u32(row + HIT_TIME, 10)
            .put_u32(row + DAMAGE, 0.15f32.to_bits());
        assert_eq!(damage_at(&guest, row, 69), Ok(0.15));
        assert_eq!(damage_at(&guest, row, 70), Ok(0.15 - DAMAGE_DECAY));
        assert_eq!(
            damage_at(&guest, row, 71),
            Ok(0.15 - DAMAGE_DECAY - DAMAGE_DECAY)
        );
        assert_eq!(damage_at(&guest, row, 200), Ok(0.0));
    }

    #[test]
    fn loss_of_sight_keeps_the_sample_and_a_focus_change_clears_it() {
        let row = 0x4000;
        let mut guest = FakeGuest::new();
        guest.map(row, 24).put_u32(row + FOCUS_HANDLE, NONE);
        remember_focus(&mut guest, row, 0x8101_0001, Some(40)).unwrap();
        remember_focus(&mut guest, row, 0x8101_0001, None).unwrap();
        assert_eq!(guest.u32(row + FOCUS_TIME), Ok(40));
        remember_focus(&mut guest, row, 0x8102_0001, None).unwrap();
        assert_eq!(guest.u32(row + FOCUS_TIME), Ok(NONE));
        remember_focus(&mut guest, row, 0x8102_0001, Some(57)).unwrap();
        assert_eq!(guest.u32(row + FOCUS_TIME), Ok(57));
    }

    #[test]
    fn a_timer_requires_an_alert_and_keeps_an_actors_baseline_across_migration() {
        let mut guest = FakeGuest::new();
        let squad_storage = 0x10000;
        let group = squad_storage + campaign_squad_pool::SQUAD_BYTES;
        guest
            .put_u32(0x51e9d8, squad_storage)
            .put_u32(0x51e9dc, group)
            .map(group, campaign_squad_pool::GROUP_ALLOCATION_BYTES);
        let actor = actors::Actor {
            handle: 0x8123_0002,
            at: 0,
            unit: NONE,
            squad: 0,
        };
        let remembered = contact::Contact {
            level: 2,
            recognized_at: None,
            alarm: false,
        };
        assert!(!timer_starts(&mut guest, actor, remembered, 2).unwrap());
        assert!(!timer_starts(&mut guest, actor, remembered, 0).unwrap());
        assert!(
            timer_starts(
                &mut guest,
                actor,
                contact::Contact {
                    level: 3,
                    ..remembered
                },
                0
            )
            .unwrap()
        );

        let reused = actors::Actor {
            handle: 0x8124_0002,
            ..actor
        };
        assert!(!timer_starts(&mut guest, reused, remembered, 0).unwrap());
        let reported = contact::Contact {
            alarm: true,
            ..remembered
        };
        assert!(timer_starts(&mut guest, reused, reported, 0).unwrap());
        assert!(timer_starts(&mut guest, reused, remembered, 0).unwrap());
    }

    #[test]
    fn aliased_variants_use_their_full_character_datum_for_eligibility() {
        let mut guest = FakeGuest::new();
        let scenario = 0x10000;
        let declarations = 0x11000;
        let strings = 0x12000;
        let globals = 0x13000;
        let values = 0x14000;
        let actor = actors::Actor {
            handle: 0,
            at: 0x20000,
            unit: NONE,
            squad: NONE,
        };
        let refs = 0x16000;
        // Two character rows, no weapon rows: the two-word header the
        // converter writes, at 16 + 2 * 16 bytes.
        let data: &[u8] = b"00000002800000003f0000003dcccccd3f8000003e99999a\0";
        guest
            .put_u32(points::SCENARIO.base, scenario)
            .put_u32(scenario + 0x1b0, data.len() as u32)
            .put_u32(scenario + 0x1b4, strings)
            .put_u32(scenario + 0x1c0, 1)
            .put_u32(scenario + 0x1c4, declarations)
            .put_u32(scenario + 0x1c8, 3)
            .put_u32(scenario + 0x1cc, refs)
            .put_u32(refs + 8, u32::from_be_bytes(*b"char"))
            .put_u32(refs + 12, 0x8120_0000)
            .put_u32(refs + 16, u32::from_be_bytes(*b"char"))
            .put_u32(refs + 20, 0x8121_0000)
            .put(declarations, hs::h1_ai_slots::PERCEPTION_STATE.as_bytes())
            .put(strings, data)
            .put_u32(0x4f9380, globals)
            .put_u32(globals + 0x44, values)
            .put_u32(values + 0x41d * 8 + 4, 0);
        for (id, expect) in [
            (0x8120_0000, Some([0.5, 0.1])),
            (0x8121_0000, Some([1.0, 0.3])),
            (0x8122_0000, None),
        ] {
            guest.put_u32(actor.at + CHARACTER, id);
            assert_eq!(thresholds(&guest, actor), Ok(expect));
        }
    }

    #[test]
    fn a_legacy_one_word_table_still_reads() {
        let mut guest = FakeGuest::new();
        let scenario = 0x10000;
        let declarations = 0x11000;
        let strings = 0x12000;
        let globals = 0x13000;
        let values = 0x14000;
        let actor = actors::Actor {
            handle: 0,
            at: 0x20000,
            unit: NONE,
            squad: NONE,
        };
        let refs = 0x16000;
        // A table written before the weapon section existed: 8 + 2 * 16.
        let data: &[u8] = b"000000023f0000003dcccccd3f8000003e99999a\0";
        guest
            .put_u32(points::SCENARIO.base, scenario)
            .put_u32(scenario + 0x1b0, data.len() as u32)
            .put_u32(scenario + 0x1b4, strings)
            .put_u32(scenario + 0x1c0, 1)
            .put_u32(scenario + 0x1c4, declarations)
            .put_u32(scenario + 0x1c8, 2)
            .put_u32(scenario + 0x1cc, refs)
            .put_u32(refs, u32::from_be_bytes(*b"char"))
            .put_u32(refs + 4, 0x8120_0000)
            .put_u32(refs + 8, u32::from_be_bytes(*b"char"))
            .put_u32(refs + 12, 0x8121_0000)
            .put(declarations, hs::h1_ai_slots::PERCEPTION_STATE.as_bytes())
            .put(strings, data)
            .put_u32(0x4f9380, globals)
            .put_u32(globals + 0x44, values)
            .put_u32(values + 0x41d * 8 + 4, 0);
        guest.put_u32(actor.at + CHARACTER, 0x8120_0000);
        assert_eq!(thresholds(&guest, actor), Ok(Some([0.5, 0.1])));
        assert_eq!(perception_table(&guest).unwrap().unwrap().weapons(), 0);
    }
}
