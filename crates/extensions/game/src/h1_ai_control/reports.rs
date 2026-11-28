//! Halo 1's alert reports and forced combat levels, rebuilt as events the
//! perception bridge can consume. Halo 1 keeps both on the unit object:
//! the alert-report copy at `0x0014e8a0` moves the alert bits at unit
//! `+0xc0` and the sighting timestamps at `+0xb8`, `+0x3e4` and `+0x3e8`
//! from one linked unit of an encounter to another, and the event-report
//! path at `0x001506e7` (Halo 1 `0x001505f0`, driven from the queue
//! `0x0014d3a0` walks for actors with no selected prop) sets or clears
//! the forced-combat bit 30 of unit `+0x134` from the report's own flag.
//! Halo 2 selects no interest for such an actor, so the bridge keeps the
//! same state in the campaign pool's actor rows instead. A heard shot's
//! reach is Halo 1's exact formula (`0x0014e379`): the weapon's authored
//! loudness (`weap +0x270`, carried by the converter in the perception
//! table's weapon rows) times three, plus the hearing unit's own radius
//! at object body `+0x3c` — the scaled bounding radius, which Halo 2
//! keeps at the same offset (`0x000be075` writes it from the definition
//! tag's `+0x4` bounding radius times the object's scale).

use std::collections::BTreeMap;

use guest_layout::{DatumHandle, memory::{GuestMemory, MemFault}, view::{Record, objects}};
use hooks::{Builder, Flow, points};

use super::{
    NONE, actors, contact, is_converted_campaign, state, uncover_limits,
    uncover_limits::REPORT_LATCH as LAST_SHOT,
};
/// The hearing unit's radius: the scaled bounding radius both engines
/// keep at object body `+0x3c`.
const UNIT_RADIUS: u32 = 0x3c;
const ITEM_BARRELS_COUNT: u32 = 0x2d0;
const ITEM_BARRELS: u32 = 0x2d4;
const BARREL_SIZE: u32 = 0xec;
const BARREL_FIRING_NOISE: u32 = 0x36;
/// Halo 1 re-arms the forced level on every report; three seconds of
/// quiet ends one.
const FORCE_TICKS: u32 = 90;

type R<T> = Result<T, MemFault>;

/// The forced combat level of `0x001506e7`: while a recent report holds,
/// the actor counts as combat level 5 (timers' release level) regardless
/// of its own interest.
pub(super) fn forced(m: &dyn GuestMemory, actor: actors::Actor, now: u32) -> R<bool> {
    let Some(row) = uncover_limits::actor_row(m, actor)? else {
        return Ok(false);
    };
    if m.u32(row)? != actor.handle {
        return Ok(false);
    }
    let shot = m.u32(row + LAST_SHOT)?;
    Ok(shot != u32::MAX && now.wrapping_sub(shot) < FORCE_TICKS)
}

/// The best contact an encounter's alert reports carry to a member that
/// has not reached it alone. The source's level, alert bit and sighting
/// sample are exactly the fields the copy at `0x0014e8a0` moves.
pub(super) fn merge(mut own: contact::Contact, report: contact::Contact) -> contact::Contact {
    if report.level > own.level {
        own.level = report.level;
    }
    own.alarm |= report.alarm;
    if own.recognized_at.is_none() {
        own.recognized_at = report.recognized_at;
    }
    own
}

/// One report per encounter group: the strongest alarmed contact of its
/// members, following the source bits the Halo 1 copy moves.
pub(super) fn collect(
    reports: &mut BTreeMap<u16, contact::Contact>,
    group: Option<u16>,
    contact: contact::Contact,
) {
    if !contact.alarm {
        return;
    }
    let Some(group) = group else {
        return;
    };
    let entry = reports.entry(group).or_insert(contact::Contact {
        level: 0,
        recognized_at: None,
        alarm: false,
    });
    *entry = merge(*entry, contact);
}

/// The firing barrel's authored noise class. The converted `weap` keeps
/// Halo 1's trigger `+0x2e` value (the def's evidence line): zero is the
/// silent end of the enum, and only a shot that carries a report can
/// force a combat level.
fn audible_barrel(m: &dyn GuestMemory, definition: Option<DatumHandle>, barrel: u32) -> R<bool> {
    use guest_layout::tags::{GroupTag, tag_get_checked};

    let Some(definition) = definition else {
        return Ok(true);
    };
    let Some(at) = tag_get_checked(m, definition, GroupTag::from_chars(*b"weap"))? else {
        return Ok(true);
    };
    let count = m.u32(at + ITEM_BARRELS_COUNT)?;
    if barrel >= count {
        return Ok(true);
    }
    let barrels = m.u32(at + ITEM_BARRELS)?;
    Ok((m.u16(barrels + barrel * BARREL_SIZE + BARREL_FIRING_NOISE)? as i16) > 0)
}

/// The weapon's authored report loudness from the perception table's
/// weapon rows. A weapon without a row — Halo 2 content no Halo 1 tag
/// sourced — behaves as a zero-loudness Halo 1 weapon: its shots report
/// no further than the hearing unit's own radius.
fn weapon_loudness(m: &dyn GuestMemory, definition: Option<DatumHandle>) -> R<f32> {
    let (Some(table), Some(definition)) = (uncover_limits::perception_table(m)?, definition) else {
        return Ok(0.0);
    };
    let scenario = m.u32(points::SCENARIO.base)?;
    // The weapon references run just before the character references,
    // which anchor the end of the scenario's reference list.
    let Some(first) = m
        .u32(scenario + 0x1c8)?
        .checked_sub(table.characters() + table.weapons())
    else {
        return Ok(0.0);
    };
    let references = m.u32(scenario + 0x1cc)?;
    for row in 0..table.weapons() {
        let reference = references + 8 * (first + row);
        if m.u32(reference)? == u32::from_be_bytes(*b"weap")
            && m.u32(reference + 4)? == definition.raw()
        {
            return Ok(uncover_limits::hex(m, table.weapon_row(row))?.map_or(0.0, f32::from_bits));
        }
    }
    Ok(0.0)
}

fn record_shot(m: &mut dyn GuestMemory, weapon: u32, barrel: u32) -> R<()> {
    if !contact::enabled(m)? {
        return Ok(());
    }
    let Some((tick, rate)) = state::clock(m)? else {
        return Ok(());
    };
    let now = state::authored_tick(tick, rate);
    let Some((_, body)) = objects::get(m, DatumHandle(weapon))? else {
        return Ok(());
    };
    let definition = body.definition(m)?;
    if !audible_barrel(m, definition, barrel)? {
        return Ok(());
    }
    // An attached weapon can carry a stale origin; the holder's place is
    // where the sound left, as the bots' hearing already reads it.
    let mut at = body.position(m)?;
    if let Some(parent) = body.parent(m)? {
        if let Some((_, holder)) = objects::get(m, parent)? {
            at = holder.position(m)?;
        }
    }
    if !at.iter().all(|value| value.is_finite()) {
        return Ok(());
    }
    // Halo 1's reach (`0x0014e379`): the weapon's loudness times three,
    // plus each hearing unit's own radius.
    let loud = weapon_loudness(m, definition)? * 3.0;
    for actor in actors::all(m)? {
        if actor.squad == NONE {
            continue;
        }
        let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
            continue;
        };
        if unit.damage_flags(m)? & 4 != 0 {
            continue;
        }
        let reach = loud + m.f32(unit.addr() + UNIT_RADIUS)?;
        let place = unit.position(m)?;
        let squared = place
            .iter()
            .zip(at)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>();
        if squared > reach * reach {
            continue;
        }
        if let Some(row) = uncover_limits::actor_row(m, actor)? {
            uncover_limits::reset_row(m, row, actor)?;
            m.set_u32(row + LAST_SHOT, now)?;
        }
    }
    Ok(())
}

pub(super) fn register(b: &mut Builder<'_>) {
    b.raw().before(
        &points::WEAPON_FIRE,
        "force Halo 1 combat levels on heard shots",
        |call| {
            if is_converted_campaign(call)
                && let (Some(weapon), Some(barrel)) =
                    (call.arg_u32("weapon"), call.arg_u32("barrel"))
            {
                let mut guest = call.ctx.runtime().memory();
                let _ = record_shot(&mut *guest, weapon, barrel);
            }
            Ok(Flow::Continue)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::campaign_squad_pool;
    use guest_layout::datum::layout;
    use guest_layout::tags;
    use guest_layout::testing::FakeGuest;
    use hooks::points;

    fn shot_guest() -> (FakeGuest, u32) {
        const ACTOR_ARRAY: u32 = 0x0083_0000;
        const ACTOR_ROWS: u32 = 0x0084_0000;
        const ACTOR_BITS: u32 = 0x0085_0000;
        const OBJECT_ARRAY: u32 = 0x0086_0000;
        const OBJECT_ROWS: u32 = 0x0087_0000;
        const OBJECT_BITS: u32 = 0x0088_0000;
        const WEAPON_BODY: u32 = 0x0089_0000;
        const NEAR_BODY: u32 = 0x008a_0000;
        const FAR_BODY: u32 = 0x008b_0000;
        const EDGE_BODY: u32 = 0x008c_0000;
        const TAG_HEADER: u32 = 0x0060_0000;
        const GROUP_ROWS: u32 = 0x0061_0000;
        const TAG_ROWS: u32 = 0x0062_0000;
        const WEAP_TAG: u32 = 0x0063_0000;
        const BARRELS: u32 = 0x0063_1000;
        const SCENARIO: u32 = 0x0070_0000;
        const DECLARATIONS: u32 = 0x0071_0000;
        const STRINGS: u32 = 0x0072_0000;
        const REFS: u32 = 0x0073_0000;
        const GLOBALS: u32 = 0x004f_9380;
        const VM_ARRAY: u32 = 0x0074_0000;
        const VM_VALUES: u32 = 0x0075_0000;
        const WEAP: u32 = u32::from_be_bytes(*b"weap");

        // The perception table: no character rows, one weapon row holding
        // the shot weapon's authored loudness (6 world units).
        let table = format!("00000000{:08x}{:08x}", 0x8000_0001u32, 6.0f32.to_bits());
        let squad_storage = 0x10000;
        let group = squad_storage + campaign_squad_pool::SQUAD_BYTES;
        let mut guest = FakeGuest::new();
        guest
            .put_u32(points::GAME_TIME.base, 0x31_0000)
            .put_u16(0x31_0000 + 2, 30)
            .put_u32(0x31_0000 + 8, 600)
            .put_u32(0x51e9d8, squad_storage)
            .put_u32(0x51e9dc, group)
            .map(group, campaign_squad_pool::GROUP_ALLOCATION_BYTES)
            .put_u32(actors::ACTORS, ACTOR_ARRAY)
            .put_u32(ACTOR_ARRAY + layout::ELEMENT_SIZE, 0x100)
            .put_u32(ACTOR_ARRAY + layout::HIGH_WATER, 3)
            .put_u32(ACTOR_ARRAY + layout::DATA, ACTOR_ROWS)
            .put_u32(ACTOR_ARRAY + layout::BITMAP, ACTOR_BITS)
            .put_u32(ACTOR_BITS, 0b111)
            .put_u32(points::OBJECTS.base, OBJECT_ARRAY)
            .put_u32(OBJECT_ARRAY + layout::ELEMENT_SIZE, 0xc)
            .put_u32(OBJECT_ARRAY + layout::HIGH_WATER, 4)
            .put_u32(OBJECT_ARRAY + layout::DATA, OBJECT_ROWS)
            .put_u32(OBJECT_ARRAY + layout::BITMAP, OBJECT_BITS)
            .put_u32(OBJECT_BITS, 0b1111)
            // The published tag index: one weap group, one weap tag.
            .put_u32(tags::TAG_INDEX_GLOBAL, TAG_HEADER)
            .put_u32(TAG_HEADER + tags::index_layout::GROUPS, GROUP_ROWS)
            .put_u32(TAG_HEADER + tags::index_layout::GROUP_COUNT, 1)
            .put_u32(TAG_HEADER + tags::index_layout::TAG_COUNT, 1)
            .put_u32(tags::TAG_ENTRIES_GLOBAL, TAG_ROWS)
            .put_u32(GROUP_ROWS, WEAP)
            .put_u32(GROUP_ROWS + 4, u32::from_be_bytes(*b"item"))
            .put_u32(GROUP_ROWS + 8, u32::from_be_bytes(*b"obje"))
            .put_u32(TAG_ROWS, WEAP)
            .put_u32(TAG_ROWS + 4, 0x8130_0000)
            .put_u32(TAG_ROWS + 8, WEAP_TAG)
            // The weapon tag: two barrels, the first audible, the second
            // silent (Halo 1 trigger +0x2e order: silent first).
            .put_u32(WEAP_TAG + ITEM_BARRELS_COUNT, 2)
            .put_u32(WEAP_TAG + ITEM_BARRELS, BARRELS)
            .put_u16(BARRELS + BARREL_FIRING_NOISE, 1)
            .put_u16(BARRELS + BARREL_SIZE + BARREL_FIRING_NOISE, 0)
            // The perception state global points at the table, whose one
            // weapon reference names the firing weapon's definition.
            .put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(SCENARIO + 0x1b0, table.len() as u32)
            .put_u32(SCENARIO + 0x1b4, STRINGS)
            .put(STRINGS, table.as_bytes())
            .put_u32(SCENARIO + 0x1c0, 1)
            .put_u32(SCENARIO + 0x1c4, DECLARATIONS)
            .put(DECLARATIONS, hs::h1_ai_slots::PERCEPTION_STATE.as_bytes())
            .put_u32(SCENARIO + 0x1c8, 1)
            .put_u32(SCENARIO + 0x1cc, REFS)
            .put_u32(REFS, WEAP)
            .put_u32(REFS + 4, 0x8130_0000)
            .put_u32(GLOBALS, VM_ARRAY)
            .put_u32(VM_ARRAY + layout::DATA, VM_VALUES)
            .put_u32(VM_VALUES + 0x41d * 8 + 4, 0);
        // The firing weapon, held by nobody, at the origin.
        put_object(&mut guest, OBJECT_ROWS, 0, 2, objects::WEAPON, WEAPON_BODY);
        put_body(&mut guest, WEAPON_BODY, [0.0, 0.0, 0.0], 0.25);
        put_object(&mut guest, OBJECT_ROWS, 1, 3, objects::BIPED, NEAR_BODY);
        put_body(&mut guest, NEAR_BODY, [10.0, 0.0, 0.0], 1.0);
        put_object(&mut guest, OBJECT_ROWS, 2, 4, objects::BIPED, FAR_BODY);
        put_body(&mut guest, FAR_BODY, [100.0, 0.0, 0.0], 1.0);
        put_object(&mut guest, OBJECT_ROWS, 3, 5, objects::BIPED, EDGE_BODY);
        put_body(&mut guest, EDGE_BODY, [19.5, 0.0, 0.0], 1.0);
        put_actor(&mut guest, ACTOR_ROWS, 0, 0x8123, 0x0003_0001, 0);
        put_actor(&mut guest, ACTOR_ROWS, 1, 0x8124, 0x0004_0002, 1);
        put_actor(&mut guest, ACTOR_ROWS, 2, 0x8125, 0x0005_0003, 2);
        (guest, ACTOR_ROWS)
    }

    fn put_object(g: &mut FakeGuest, rows: u32, slot: u32, salt: u16, kind: u8, body: u32) {
        g.put_u16(rows + slot * 0xc, salt)
            .put_u8(rows + slot * 0xc + 3, kind)
            .put_u32(rows + slot * 0xc + 8, body);
    }

    fn put_body(g: &mut FakeGuest, at: u32, position: [f32; 3], radius: f32) {
        g.map(at, 0x10c)
            .put_u32(at, 0x8130_0000)
            .put_u32(at + 0x14, u32::MAX)
            .put_f32(at + 0x3c, radius)
            .put_f32(at + 0x64, position[0])
            .put_f32(at + 0x68, position[1])
            .put_f32(at + 0x6c, position[2]);
    }

    fn put_actor(g: &mut FakeGuest, rows: u32, slot: u32, salt: u16, unit: u32, squad: u32) {
        g.put_u16(rows + slot * 0x100, salt)
            .put_u32(rows + slot * 0x100 + 0x18, unit)
            .put_u32(rows + slot * 0x100 + 0x30, squad);
    }

    #[test]
    fn a_heard_shot_reaches_loudness_times_three_plus_the_units_radius() {
        let (mut guest, actor_rows) = shot_guest();
        record_shot(&mut guest, 0x0002_0000, 0).unwrap();
        let near = actors::Actor {
            handle: 0x8123_0000,
            at: actor_rows,
            unit: 0x0003_0001,
            squad: 0,
        };
        let far = actors::Actor {
            handle: 0x8124_0001,
            at: actor_rows + 0x100,
            unit: 0x0004_0002,
            squad: 1,
        };
        // Halo 1's reach: 6 * 3 + 1 = 19 world units. The actor at 10 is
        // inside, the actor at 19.5 is half a unit past it.
        let near_row = campaign_squad_pool::actor_state(&guest, 0)
            .unwrap()
            .unwrap();
        let far_row = campaign_squad_pool::actor_state(&guest, 1)
            .unwrap()
            .unwrap();
        let edge_row = campaign_squad_pool::actor_state(&guest, 2)
            .unwrap()
            .unwrap();
        assert_eq!(guest.u32(near_row + LAST_SHOT), Ok(600));
        assert_eq!(guest.u32(far_row + LAST_SHOT), Ok(0));
        assert_eq!(guest.u32(edge_row + LAST_SHOT), Ok(0));
        assert!(forced(&guest, near, 600 + FORCE_TICKS - 1).unwrap());
        assert!(!forced(&guest, near, 600 + FORCE_TICKS).unwrap());
        assert!(!forced(&guest, far, 600).unwrap());
        assert_eq!(guest.u32(far_row), Ok(0));
        let replacement = actors::Actor { handle: 0x8126_0000, ..near };
        assert!(!forced(&guest, replacement, 600).unwrap());
    }

    #[test]
    fn a_silent_barrel_carries_no_report() {
        let (mut guest, _) = shot_guest();
        record_shot(&mut guest, 0x0002_0000, 1).unwrap();
        let near_row = campaign_squad_pool::actor_state(&guest, 0)
            .unwrap()
            .unwrap();
        assert_eq!(guest.u32(near_row + LAST_SHOT), Ok(0));
    }

    #[test]
    fn a_report_raises_the_level_latch_and_sample_of_a_quieter_member() {
        let own = contact::Contact {
            level: 0,
            recognized_at: None,
            alarm: false,
        };
        let source = contact::Contact {
            level: 2,
            recognized_at: Some(40),
            alarm: true,
        };
        let reported = merge(own, source);
        assert_eq!(reported.level, 2);
        assert!(reported.alarm);
        assert_eq!(reported.recognized_at, Some(40));
        // A member that sees more than the report keeps its own sight.
        let ahead = merge(
            contact::Contact {
                level: 5,
                recognized_at: Some(12),
                alarm: true,
            },
            source,
        );
        assert_eq!((ahead.level, ahead.recognized_at), (5, Some(12)));
    }

    #[test]
    fn collection_keeps_the_strongest_reported_contact_per_encounter() {
        let mut reports = BTreeMap::new();
        collect(
            &mut reports,
            Some(3),
            contact::Contact {
                level: 1,
                recognized_at: None,
                alarm: true,
            },
        );
        collect(
            &mut reports,
            Some(3),
            contact::Contact {
                level: 2,
                recognized_at: Some(40),
                alarm: true,
            },
        );
        collect(
            &mut reports,
            None,
            contact::Contact {
                level: 7,
                recognized_at: Some(1),
                alarm: true,
            },
        );
        collect(
            &mut reports,
            Some(4),
            contact::Contact {
                level: 7,
                recognized_at: None,
                alarm: false,
            },
        );
        let report = reports.get(&3).copied().unwrap();
        assert_eq!(report.level, 2);
        assert_eq!(report.recognized_at, Some(40));
        assert!(!reports.contains_key(&4));
    }
}
