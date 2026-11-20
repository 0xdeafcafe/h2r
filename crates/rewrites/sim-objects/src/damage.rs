//! The damage update's events (docs/engine/object-update.md, "Slice 8
//! body"): [`Damage`] and [`Kill`] as read from guest memory at the two
//! guest functions that apply them, the damage report `d9640` and the
//! kill credit `15ce70`. The object damage update `d5de0` applies damage
//! only through those callees, so the events are read there, whoever
//! called them.
//!
//! Every read is a view over `guest_layout`'s layouts. What the guest
//! function itself reads (the report) must read; what only the event
//! adds (the tick, the victim's position, a player's unit) falls back to
//! none, so a missing side record never drops the event.

use crate::events::{Damage, Kill};
use crate::view::Objects;
use guest_layout::{DamageReport, Datum, DatumArray, Fault, GameTime, Mem, Object, ObjectType, Player, Unit};

/// The game tick, `None` without game time.
pub fn tick<M: Mem + ?Sized>(m: &M) -> Option<u32> {
    let time = guest_layout::game::through::<GameTime, _>(m, &guest_layout::global::GAME_TIME).ok()??;
    time.tick(m).ok()
}

/// The [`Damage`] the report at `report` does to `victim`, as `d9640`
/// is about to apply it.
pub fn damage<M: Mem + ?Sized>(m: &M, report: u32, victim: Datum) -> Result<Damage, Fault> {
    let r = DamageReport::at(report);
    let mut event = Damage {
        tick: tick(m),
        victim,
        victim_kind: u8::MAX,
        victim_player: Datum::NONE,
        owner_player: r.owner_player(m)?,
        owner_object: r.owner_object(m)?,
        owner_team: r.owner_team(m)?,
        code: r.code(m)?,
        effect: r.effect(m)?,
        flags: r.flags(m)?,
        body: r.body(m)?,
        shield: r.shield(m)?,
        direction: r.direction(m)?,
        origin: r.origin(m)?,
        position: None,
    };
    if let Some((kind, body)) = object(m, victim) {
        event.victim_kind = kind;
        event.position = body.position(m).ok();
        if is_unit(kind) {
            event.victim_player = Unit::at(body.addr).player(m).unwrap_or(Datum::NONE);
        }
    }
    Ok(event)
}

/// The [`Kill`] `15ce70` is about to credit: `attacker` and `victim` are
/// player datums (its first two arguments), `code` its fourth's low byte.
pub fn kill<M: Mem + ?Sized>(m: &M, attacker: Datum, victim: Datum, code: u8) -> Kill {
    let (victim_unit, position) = unit_of(m, victim);
    let (attacker_unit, attacker_position) = unit_of(m, attacker);
    Kill {
        tick: tick(m),
        attacker,
        victim,
        code,
        victim_unit,
        attacker_unit,
        position,
        attacker_position,
    }
}

fn is_unit(kind: u8) -> bool {
    matches!(ObjectType::from_u8(kind), Some(ObjectType::Biped | ObjectType::Vehicle))
}

/// A live object's type byte and body.
fn object<M: Mem + ?Sized>(m: &M, handle: Datum) -> Option<(u8, Object)> {
    let pool = Objects::read(m).ok()??;
    let o = pool.resolve(m, handle).ok()??;
    Some((o.header.kind(m).ok()?, o.body))
}

/// A player's unit and the unit's position.
fn unit_of<M: Mem + ?Sized>(m: &M, player: Datum) -> (Datum, Option<[f32; 3]>) {
    let Some(p) = player_at(m, player) else {
        return (Datum::NONE, None);
    };
    let unit = p.unit(m).unwrap_or(Datum::NONE);
    let position = object(m, unit).and_then(|(_, body)| body.position(m).ok());
    (unit, position)
}

/// The live player `player` names: a slot below the array's capacity
/// whose salt is the datum's.
fn player_at<M: Mem + ?Sized>(m: &M, player: Datum) -> Option<Player> {
    if player.is_none() {
        return None;
    }
    let array = DatumArray::at(guest_layout::global::PLAYERS.pointer(m).ok()??);
    let capacity = array.maximum_count(m).ok()?;
    if i32::from(player.index()) >= capacity {
        return None;
    }
    let size = array.element_size(m).ok()? as u32;
    let p = Player::at(array.data(m).ok()?.wrapping_add(u32::from(player.index()).wrapping_mul(size)));
    let salt = p.salt(m).ok()?;
    (salt != 0 && salt == player.salt()).then_some(p)
}

#[cfg(test)]
mod tests;
