//! The tick's simulation events (docs/engine/sim-rewrite-objects.md
//! section 8): appended only on the game thread, at the retail moment of
//! the event (by the serial bodies, and by `hooks::sim_objects`' observers
//! at the guest functions that apply damage and credit kills), and read at
//! tick end.
//!
//! The buffer belongs to the thread that runs the simulation. It is
//! cleared when a tick starts (`hooks::sim_objects` at `GAME_TICK`'s
//! entry), so at tick end it holds that tick's events. Events are not
//! persistent state: not in keyframes, snapshots or the hash, and nothing
//! in the simulation reads them.

use guest_layout::Datum;
use std::cell::RefCell;

/// One event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimEvent {
    /// Finalize's delete pass is about to delete the object
    /// (`b7150` pass B, before `b83b0`): its handle, its type byte (header
    /// `+3`) and its definition tag (body `+0`, none when the body could
    /// not be read).
    ObjectDeleted { id: Datum, kind: u8, definition: Datum },
    /// A damage instance is applied to an object ([`Damage`]).
    Damage(Damage),
    /// A player was killed and the kill credited ([`Kill`]).
    Kill(Kill),
}

/// One damage instance, read where the game applies it: the damage report
/// function `d9640`'s entry (`guest_layout::DamageReport`, the victim
/// object), once per instance, in the order the game applies them. What a
/// carnage report needs: who was hit, who it is credited to, with what,
/// how much, where and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Damage {
    /// The game tick (`guest_layout::GameTime::TICK`), `None` without game
    /// time.
    pub tick: Option<u32>,
    /// The object damaged.
    pub victim: Datum,
    /// Its type byte (header `+3`); `u8::MAX` when the handle does not
    /// resolve.
    pub victim_kind: u8,
    /// The player controlling it, when it is a unit with one.
    pub victim_player: Datum,
    /// The player and object the damage is credited to, and the owner's
    /// team. Recent-damage attribution can credit a kill to someone else;
    /// [`Kill::attacker`] is final.
    pub owner_player: Datum,
    pub owner_object: Datum,
    pub owner_team: i16,
    /// The packed damage code: the damage reporting type in the low 6
    /// bits (the weapon or cause the kill feed names), a modifier in the
    /// high 2 (`guest_layout::damage::reporting_type`, `modifier`).
    pub code: u8,
    /// The damage effect (`jpt!`) tag: the most precise identity of what
    /// hit, not a weapon tag.
    pub effect: Datum,
    /// The report flags (`guest_layout::damage::DEPLETING`: this damage
    /// depleted the body).
    pub flags: u32,
    /// The body and shield damage, fractions of full vitality.
    pub body: f32,
    pub shield: f32,
    /// The damage's direction and where it came from.
    pub direction: [f32; 3],
    pub origin: [f32; 3],
    /// The victim's position (parent-relative under a parent); `None`
    /// when the handle does not resolve.
    pub position: Option<[f32; 3]>,
}

impl Damage {
    /// This damage depleted the victim's body.
    pub const fn depleting(&self) -> bool {
        self.flags & guest_layout::damage::DEPLETING != 0
    }
}

/// A kill, read where the game credits it: `PLAYER_KILLED` (`15ce70`)'s
/// entry, after recent-damage attribution, so the attacker is final. Only
/// players are credited there; an object's death without a player is a
/// [`Damage`] that is [`Damage::depleting`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kill {
    /// The game tick, `None` without game time.
    pub tick: Option<u32>,
    /// The killer's player datum, none for none (a fall, a suicide by the
    /// world).
    pub attacker: Datum,
    /// The killed player's datum.
    pub victim: Datum,
    /// The packed damage code, as [`Damage::code`].
    pub code: u8,
    /// Each player's unit at the kill, none when it has none.
    pub victim_unit: Datum,
    pub attacker_unit: Datum,
    /// Each unit's position, `None` without a unit.
    pub position: Option<[f32; 3]>,
    pub attacker_position: Option<[f32; 3]>,
}

thread_local! {
    static EVENTS: RefCell<Vec<SimEvent>> = const { RefCell::new(Vec::new()) };
}

/// Append an event.
pub fn push(event: SimEvent) {
    EVENTS.with_borrow_mut(|e| e.push(event));
}

/// Drop every event: a new tick starts (`GAME_TICK`'s entry).
pub fn clear() {
    EVENTS.with_borrow_mut(Vec::clear);
}

/// The events so far this tick, in the order they happened.
pub fn with<R>(f: impl FnOnce(&[SimEvent]) -> R) -> R {
    EVENTS.with_borrow(|e| f(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_append_in_order_and_clear() {
        clear();
        let a = SimEvent::ObjectDeleted { id: Datum(0x8001_0001), kind: 2, definition: Datum::NONE };
        let b = SimEvent::ObjectDeleted { id: Datum(0x8002_0002), kind: 5, definition: Datum(0xe000_0010) };
        push(a);
        push(b);
        assert_eq!(with(<[SimEvent]>::to_vec), [a, b]);
        clear();
        assert!(with(<[SimEvent]>::is_empty));
    }
}
