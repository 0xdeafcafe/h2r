//! The API's front-end signatures, keyed by entry number, and the events'
//! argument types. `caps::ENTRIES` and `ir::Event::args` keep the IR
//! types; each front-end signature erases to them (the test below). See
//! `docs/game/rules-language-types.md`, "Front-end types".

use super::types::{Handle, SetKind, Type};
use crate::caps::{Entry, id};
use crate::ir::Event;

const P: Type = Type::PLAYER;
const P_: Type = Type::H(Handle::Player, true);
const T: Type = Type::TEAM;
const T_: Type = Type::H(Handle::Team, true);
const O: Type = Type::OBJECT;
const Z: Type = Type::ZONE;
const W: Type = Type::WEAPON;
const W_: Type = Type::H(Handle::Weapon, true);

/// A front-end signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sig {
    pub args: Vec<Type>,
    pub result: Option<Type>,
}

/// The front-end signature of `e`, or `None` for an entry the language
/// does not call by name (`label_of`: `x is <label>` walks the label's
/// members instead, `lower::member_test`).
/// The rows that differ from "the IR type, never `none`" are those of the
/// design's table.
pub fn of(e: &Entry) -> Option<Sig> {
    let given = |args: &[Type], result: Option<Type>| Some(Sig { args: args.to_vec(), result });
    match e.id {
        id::LABEL_OF => None,
        id::INSIDE => given(&[P, Z], Some(Type::Bool)),
        id::TICKS_HELD => given(&[Z, T], Some(Type::Time)),
        id::ROUND_TIME_LEFT | id::NOW => given(&[], Some(Type::Time)),
        id::CARRIER_OF | id::PLAYER_OF => given(&[O], Some(P_)),
        id::OBJECT_TEAM => given(&[O], Some(T_)),
        id::END_ROUND | id::FF_TEAM_FINISH => given(&[T_], None),
        id::FF_LIVES => given(&[P_, Type::Int], None),
        id::REGISTER_OBJECTIVE => given(&[O, T_], None),
        id::SPAWN_OBJECTIVE => given(&[Type::TAG, O, T_], None),
        id::SET_LOADOUT => given(&[P, W_, W_], None),
        id::GIVE => given(&[P, W], None),
        id::SPAWN_AREAS => given(&[T, Type::Set(SetKind::Spawns), Type::Int], None),
        id::SPAWN_NEAR => given(&[T, Type::H(Handle::Zone, true), Type::Int], None),
        // A `vec` is three registers in the IR (`Type::width`). The `at` of
        // `teleport` and `move_to` may also be an object, which the checker
        // and the lowering turn into its position.
        id::TELEPORT => given(&[O, Type::Vec], None),
        id::MOVE_TO => given(&[O, Type::Vec, Type::Time], None),
        id::SHOW_AREA => given(&[Z, T_], None),
        id::SOUND_AT => given(&[Type::SOUND, O], None),
        id::SOUND_TO => given(&[Type::SOUND, P], None),
        id::SOUND_TO_TEAM => given(&[Type::SOUND, T], None),
        id::SOUND_TO_ALL | id::MUSIC => given(&[Type::SOUND], None),
        id::HIDE_AREA => given(&[Z], None),
        id::SET_BODY => given(&[T, Type::H(Handle::Tag, true)], None),
        id::FLY => given(&[O, Type::Set(SetKind::Paths), Type::Time], None),
        id::PLACE_SQUAD => given(&[Type::Squad, Type::Class, Type::Int, Type::Set(SetKind::Objects)], None),
        _ => {
            let mut args = Vec::with_capacity(e.args.len());
            for &a in e.args {
                args.push(Type::from_ir(a)?);
            }
            let result = match e.result {
                Some(r) => Some(Type::from_ir(r)?),
                None => None,
            };
            Some(Sig { args, result })
        }
    }
}

/// The front-end types of an event's arguments. `on timer` names a timer
/// instead.
pub fn event_args(ev: Event) -> &'static [Type] {
    match ev {
        Event::Kill => &[P, P_],
        Event::Spawn | Event::Death | Event::Join | Event::Quit | Event::Bounced | Event::Grounded => &[P],
        Event::Pickup | Event::Drop | Event::Press | Event::Edit => &[P, O],
        Event::Bounce => &[P, Z, Type::Int, Type::Bool],
        Event::Enter | Event::Leave => &[Z, O],
        Event::Arrive => &[O],
        Event::SquadDead => &[Type::Squad],
        Event::Damage => &[O, P_, Type::Fixed],
        Event::Touchdown => &[P, Type::Fixed, Type::Fixed],
        Event::Launch => &[P, Type::H(Handle::Zone, true), Type::Fixed],
        Event::ActorKill => &[P_, Type::Class],
        Event::ActorAssist => &[P_, P_, Type::Class],
        Event::Init | Event::RoundStart | Event::Tick | Event::Timer | Event::RoundEnd => &[],
    }
}

/// Where a handler may name a label in its event's arguments, and of
/// what: a zone label in the zone's place of `on enter`, `on leave` and
/// `on bounce`, an object label in the object's place of `on pickup`,
/// `on drop`, `on press` and `on edit`.
pub fn label_arg(ev: Event) -> Option<(usize, Handle)> {
    match ev {
        Event::Enter | Event::Leave => Some((0, Handle::Zone)),
        Event::Bounce => Some((1, Handle::Zone)),
        Event::Pickup | Event::Drop | Event::Press | Event::Edit => Some((1, Handle::Object)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::ENTRIES;

    #[test]
    fn every_signature_erases_to_its_entry() {
        for e in ENTRIES {
            let Some(sig) = of(e) else {
                assert_eq!(e.id, id::LABEL_OF, "{}", e.name);
                continue;
            };
            let args: Vec<_> = sig
                .args
                .iter()
                .flat_map(|t| std::iter::repeat_n(t.ir(), t.width()))
                .collect();
            assert_eq!(args, e.args, "{}", e.name);
            assert_eq!(sig.result.map(|t| t.ir()), e.result, "{}", e.name);
        }
    }

    #[test]
    fn every_event_erases_to_its_registers() {
        for ev in Event::ALL {
            let args: Vec<_> = event_args(ev).iter().map(|t| t.ir()).collect();
            assert_eq!(args, ev.args(), "{}", ev.name());
            if let Some((k, _)) = label_arg(ev) {
                assert!(event_args(ev)[k].handle().is_some_and(|h| h.fits(Handle::Object)));
            }
        }
    }
}
