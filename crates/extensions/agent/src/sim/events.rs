//! `sim.events` (docs/runtime/agent-api.md, "sim"): the tick's simulation
//! events from `sim_state::events`, read at a tick boundary, so the last
//! tick's. No guest reads here: the events were read when they happened.

use agent_proto::sim::events::{DamageEvent, EventKind, EventsParams, EventsReply, KillEvent, SimEvent};
use agent_proto::sim::objects::handle;
use agent_proto::{RpcError, code};
use guest_layout::Datum;
use guest_layout::damage::{modifier, reporting_type};
use sim_state::ObjectKind;

/// The kinds `p.kinds` names, refusing a name that is not one.
fn kinds(p: &EventsParams) -> Result<Vec<EventKind>, RpcError> {
    p.kinds
        .iter()
        .map(|k| {
            EventKind::parse(k).ok_or_else(|| {
                RpcError::new(
                    code::INVALID_PARAMS,
                    format!("kinds: {k:?} is not one of {:?}", EventKind::NAMES),
                )
            })
        })
        .collect()
}

fn datum(d: Datum) -> Option<String> {
    d.get().map(handle)
}

fn group(kind: u8) -> Option<String> {
    ObjectKind::from_u8(kind).map(|k| k.group().to_owned())
}

fn kind_of(e: &sim_state::SimEvent) -> EventKind {
    match e {
        sim_state::SimEvent::ObjectDeleted { .. } => EventKind::Deleted,
        sim_state::SimEvent::Damage(_) => EventKind::Damage,
        sim_state::SimEvent::Kill(_) => EventKind::Kill,
        sim_state::SimEvent::ActorKilled { .. } => EventKind::ActorKilled,
    }
}

/// One event on the wire.
pub fn event(e: &sim_state::SimEvent) -> SimEvent {
    match *e {
        sim_state::SimEvent::ObjectDeleted { id, kind, definition } => SimEvent::Deleted {
            id: handle(id.raw()),
            kind: group(kind),
            definition: datum(definition),
        },
        sim_state::SimEvent::Damage(d) => SimEvent::Damage(DamageEvent {
            tick: d.tick,
            victim: handle(d.victim.raw()),
            victim_kind: group(d.victim_kind),
            victim_player: datum(d.victim_player),
            owner_player: datum(d.owner_player),
            owner_object: datum(d.owner_object),
            owner_team: d.owner_team,
            code: d.code,
            damage_type: reporting_type(d.code),
            modifier: modifier(d.code),
            effect: datum(d.effect),
            flags: d.flags,
            depleting: d.depleting(),
            body: d.body,
            shield: d.shield,
            direction: d.direction,
            origin: d.origin,
            position: d.position,
        }),
        sim_state::SimEvent::Kill(k) => SimEvent::Kill(KillEvent {
            tick: k.tick,
            attacker: datum(k.attacker),
            victim: datum(k.victim),
            code: k.code,
            damage_type: reporting_type(k.code),
            modifier: modifier(k.code),
            victim_unit: datum(k.victim_unit),
            attacker_unit: datum(k.attacker_unit),
            position: k.position,
            attacker_position: k.attacker_position,
        }),
        sim_state::SimEvent::ActorKilled { unit, character, killer, weapon } => {
            SimEvent::ActorKilled {
                unit: handle(unit.raw()),
                character: datum(character),
                killer: datum(killer),
                weapon,
            }
        }
    }
}

/// The answer over `events`, in order, filtered by `p.kinds`.
pub fn reply(events: &[sim_state::SimEvent], p: &EventsParams) -> Result<EventsReply, RpcError> {
    let kinds = kinds(p)?;
    let events = events
        .iter()
        .filter(|e| kinds.is_empty() || kinds.contains(&kind_of(e)))
        .map(event)
        .collect();
    Ok(EventsReply { events })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_state::{Damage, Kill};

    fn damage() -> Damage {
        Damage {
            tick: Some(40),
            victim: Datum(0x8001_0001),
            victim_kind: ObjectKind::Biped as u8,
            victim_player: Datum(0xe100_0000),
            owner_player: Datum(0xe101_0001),
            owner_object: Datum::NONE,
            owner_team: 2,
            code: 0x4d,
            effect: Datum(0xe200_0007),
            flags: 1,
            body: 0.75,
            shield: 0.5,
            direction: [0.0, 1.0, 0.0],
            origin: [1.0, 2.0, 3.0],
            position: None,
        }
    }

    fn kill() -> Kill {
        Kill {
            tick: Some(40),
            attacker: Datum(0xe101_0001),
            victim: Datum(0xe100_0000),
            code: 0x0d,
            victim_unit: Datum(0x8001_0001),
            attacker_unit: Datum::NONE,
            position: Some([4.0, 5.0, 6.0]),
            attacker_position: None,
        }
    }

    fn all() -> Vec<sim_state::SimEvent> {
        vec![
            sim_state::SimEvent::ObjectDeleted { id: Datum(0x8003_0003), kind: 2, definition: Datum::NONE },
            sim_state::SimEvent::Damage(damage()),
            sim_state::SimEvent::Kill(kill()),
            sim_state::SimEvent::ActorKilled {
                unit: Datum(0x8004_0004),
                character: Datum(0xe200_0004),
                killer: Datum(0xe101_0001),
                weapon: 0x4d,
            },
        ]
    }

    #[test]
    fn each_event_goes_on_the_wire_with_handles_groups_and_the_code_split() {
        let r = reply(&all(), &EventsParams::default()).unwrap();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["events"][0]["type"], "deleted");
        assert_eq!(v["events"][0]["id"], "0x80030003");
        assert_eq!(v["events"][0]["kind"], "weap");
        assert_eq!(v["events"][0]["definition"], serde_json::Value::Null);

        let d = &v["events"][1];
        assert_eq!(d["type"], "damage");
        assert_eq!(d["victim"], "0x80010001");
        assert_eq!(d["victim_kind"], "bipd");
        assert_eq!(d["owner_player"], "0xe1010001");
        assert_eq!(d["owner_object"], serde_json::Value::Null);
        assert_eq!((d["damage_type"].as_u64(), d["modifier"].as_u64()), (Some(13), Some(1)));
        assert_eq!(d["depleting"], true);
        assert_eq!(d["body"], 0.75);
        assert_eq!(d["tick"], 40);

        let k = &v["events"][2];
        assert_eq!(k["type"], "kill");
        assert_eq!((k["attacker"].as_str(), k["victim"].as_str()), (Some("0xe1010001"), Some("0xe1000000")));
        assert_eq!(k["damage_type"], 13);
        assert_eq!(k["attacker_unit"], serde_json::Value::Null);
        assert_eq!(k["position"], serde_json::json!([4.0, 5.0, 6.0]));

        assert_eq!(v["events"][3], serde_json::json!({
            "type": "actor_killed",
            "unit": "0x80040004",
            "character": "0xe2000004",
            "killer": "0xe1010001",
            "weapon": 0x4d,
        }));

        // The wire type reads back.
        let back: EventsReply = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn kinds_filter_and_an_unknown_kind_is_refused() {
        let p = EventsParams { kinds: vec!["kill".into(), "deleted".into()] };
        let r = reply(&all(), &p).unwrap();
        assert!(matches!(r.events[..], [SimEvent::Deleted { .. }, SimEvent::Kill(_)]), "{r:?}");
        let e = reply(&all(), &EventsParams { kinds: vec!["moon".into()] }).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(reply(&[], &EventsParams::default()).unwrap().events.is_empty());
    }
}
