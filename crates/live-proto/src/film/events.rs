//! A typed stats record in a TICK event payload (subtype 1). The surrounding
//! envelope supplies the film tick. The six original Live event bodies retain
//! their match-clock ticks, including across a campaign revert.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::game;
pub mod ballistics;
pub mod medals;
pub mod objective;

/// The upper bound applies to a single uncompressed event, before serde.
pub const MAX_EVENT_BYTES: usize = 128 << 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Event {
    GameStarted(game::GameStarted),
    PlayerJoined(game::PlayerJoined),
    PlayerLeft(game::PlayerLeft),
    PlayerKilled(game::PlayerKilled),
    ScoreChanged(game::ScoreChanged),
    GameEnded(game::GameEnded),
    /// The scoreboard reports the count. It does not identify which victim.
    Assist {
        player: u32,
        count: u32,
    },
    Medal {
        player: u32,
        /// Match tick of the qualifying kill. Last-word can be announced
        /// after another participant has taken the killer's slot.
        earned_at: u64,
        medal: String,
    },
    /// A bit in the fire mask gives an exact shot tick. Additional shots in
    /// a legacy/overflowing mask are reported together at the sample tick.
    /// Unit ids use the same p<slot> or object datum as FrameHit.
    WeaponFire {
        unit: String,
        weapon: Option<String>,
        count: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        barrel: Option<ballistics::Barrel>,
    },
    Damage(game::FrameHit),
    Impact(ballistics::Impact),
    ProjectileEnded(ballistics::Endpoint),
    Pickup {
        unit: String,
    },
    VehicleEntered {
        unit: String,
        vehicle: String,
        seat: Option<u32>,
    },
    VehicleExited {
        unit: String,
        vehicle: String,
        seat: Option<u32>,
    },
    /// Changes to the authoritative score in a named objective game type.
    /// Exactly one of player or team is set. Some game types score both.
    ObjectiveScore {
        game_type: String,
        player: Option<u32>,
        team: Option<i32>,
        score: i64,
    },
    ObjectiveState(objective::Change),
    /// The engine successfully applied the equipment's effect to this unit.
    PowerupTaken {
        object: u32,
        definition: u32,
        unit: u32,
        player: Option<u32>,
    },
}

impl Event {
    /// Feed the established Live projection exactly the records it already
    /// consumes. Highlights such as a medal do not produce a second kill.
    pub fn telemetry(&self) -> Option<(&'static str, Value)> {
        match self {
            Self::GameStarted(e) => Some((game::GAME_STARTED, json!(e))),
            Self::PlayerJoined(e) => Some((game::PLAYER_JOINED, json!(e))),
            Self::PlayerLeft(e) => Some((game::PLAYER_LEFT, json!(e))),
            Self::PlayerKilled(e) => Some((game::PLAYER_KILLED, json!(e))),
            Self::ScoreChanged(e) => Some((game::SCORE_CHANGED, json!(e))),
            Self::GameEnded(e) => Some((game::GAME_ENDED, json!(e))),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Original telemetry match clock (even for a delayed damage sample).
    pub tick: u64,
    #[serde(flatten)]
    pub event: Event,
}

impl Record {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_EVENT_BYTES {
            return Err("film event exceeds byte limit".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err("film event exceeds byte limit".into());
        }
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }

    /// Inspect the bounded JSON envelope and borrow its raw data. Live only
    /// materialises its six game records. A new
    /// presentation or highlight kind can therefore arrive before a server
    /// update, and a busy firefight allocates no hit or shot payloads here.
    pub fn decode_telemetry(bytes: &[u8]) -> Result<Option<(u64, &'static str, Value)>, String> {
        #[derive(Deserialize)]
        struct Envelope<'a> {
            tick: u64,
            #[serde(borrow)]
            kind: std::borrow::Cow<'a, str>,
            data: &'a serde_json::value::RawValue,
        }
        if bytes.len() > MAX_EVENT_BYTES {
            return Err("film event exceeds byte limit".into());
        }
        let row: Envelope<'_> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        fn body<T: serde::de::DeserializeOwned>(
            data: &serde_json::value::RawValue,
        ) -> Result<T, String> {
            serde_json::from_str(data.get()).map_err(|e| e.to_string())
        }
        let event = match row.kind.as_ref() {
            game::GAME_STARTED => Event::GameStarted(body(row.data)?),
            game::PLAYER_JOINED => Event::PlayerJoined(body(row.data)?),
            game::PLAYER_LEFT => Event::PlayerLeft(body(row.data)?),
            game::PLAYER_KILLED => Event::PlayerKilled(body(row.data)?),
            game::SCORE_CHANGED => Event::ScoreChanged(body(row.data)?),
            game::GAME_ENDED => Event::GameEnded(body(row.data)?),
            _ => return Ok(None),
        };
        Ok(event.telemetry().map(|(kind, data)| (row.tick, kind, data)))
    }
}

/// Produces highlights while the telemetry session still has fire masks,
/// action ages and the damage ring. State lasts for one match.
#[derive(Default)]
pub struct Tracker {
    medals: medals::Tally,
    // The same slot may hold a different participant after a leave.
    generations: BTreeMap<u32, u32>,
    next_generation: u32,
    last_scoring: Option<(u32, u64)>,
    vehicles: BTreeMap<String, (String, Option<u32>)>,
    scores: BTreeMap<u32, (u32, i64)>,
    team_scores: BTreeMap<i32, i64>,
    objective_type: Option<String>,
}

impl Tracker {
    fn generation(&mut self, slot: u32) -> u32 {
        if let Some(generation) = self.generations.get(&slot) {
            return *generation;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        self.generations.insert(slot, generation);
        generation
    }

    pub fn event(
        &mut self,
        kind: &str,
        data: &Value,
        fallback_tick: u64,
    ) -> Result<Vec<Record>, String> {
        let event: Event = serde_json::from_value(json!({"kind": kind, "data": data}))
            .map_err(|e| e.to_string())?;
        let mut extra = Vec::new();
        let tick = match &event {
            Event::GameStarted(start) => {
                *self = Self::default();
                self.objective_type = start.game.variant.clone().filter(|variant| {
                    matches!(
                        variant.as_str(),
                        "CTF"
                            | "Oddball"
                            | "King of the Hill"
                            | "Territories"
                            | "Assault"
                            | "Juggernaut"
                            | "Race"
                            | "Headhunter"
                    )
                });
                start.game.tick.unwrap_or(fallback_tick)
            }
            Event::PlayerJoined(_) => fallback_tick,
            Event::PlayerLeft(e) => {
                self.generations.remove(&e.index);
                self.scores.remove(&e.index);
                self.vehicles.remove(&format!("p{}", e.index));
                fallback_tick
            }
            Event::PlayerKilled(kill) => {
                let victim = self.generation(kill.victim);
                let killer = kill
                    .killer
                    .filter(|&k| k != kill.victim && kill.kind == "kill");
                let scoring = killer.map(|slot| {
                    self.last_scoring = Some((slot, kill.tick));
                    self.generation(slot)
                });
                self.medals.kill_with(kill.tick, scoring, victim, |medal| {
                    if let Some(player) = killer {
                        extra.push(Event::Medal {
                            player,
                            earned_at: kill.tick,
                            medal: medal.into(),
                        });
                    }
                });
                kill.tick
            }
            Event::ScoreChanged(e) => {
                self.score_changes(
                    &e.players,
                    e.teams.as_deref().unwrap_or_default(),
                    &mut extra,
                );
                e.tick
            }
            Event::GameEnded(e) => {
                self.score_changes(&e.players, &[], &mut extra);
                if let Some((player, earned_at)) = self
                    .last_scoring
                    .filter(|_| self.medals.last_word().is_some())
                {
                    extra.push(Event::Medal {
                        player,
                        earned_at,
                        medal: "last-word".into(),
                    });
                }
                e.tick
            }
            _ => return Err(format!("unsupported telemetry event {kind}")),
        };
        Ok(std::iter::once(event)
            .chain(extra)
            .map(|event| Record { tick, event })
            .collect())
    }

    fn score_changes(
        &mut self,
        players: &[game::PlayerScore],
        teams: &[game::TeamScore],
        out: &mut Vec<Event>,
    ) {
        for p in players {
            let (assists, score) = self
                .scores
                .insert(p.index, (p.assists, p.score))
                .unwrap_or_default();
            if p.assists > assists {
                out.push(Event::Assist {
                    player: p.index,
                    count: p.assists - assists,
                });
            }
            if let Some(game_type) = self.objective_type.as_ref().filter(|_| score != p.score) {
                out.push(Event::ObjectiveScore {
                    game_type: game_type.clone(),
                    player: Some(p.index),
                    team: None,
                    score: p.score,
                });
            }
        }
        for t in teams {
            let score = self.team_scores.insert(t.team, t.score).unwrap_or_default();
            if let Some(game_type) = self.objective_type.as_ref().filter(|_| score != t.score) {
                out.push(Event::ObjectiveScore {
                    game_type: game_type.clone(),
                    player: None,
                    team: Some(t.team),
                    score: t.score,
                });
            }
        }
    }

    /// Recover combat as well as state changes when translating an archive
    /// which only has telemetry frames.
    pub fn frame(&mut self, frame: &game::Frame) -> Vec<Record> {
        self.sample_changes(frame, true)
    }

    /// Engine recording already captures combat directly at its original
    /// tick. Only sample the actions and occupancy transitions here.
    pub fn frame_changes(&mut self, frame: &game::Frame) -> Vec<Record> {
        self.sample_changes(frame, false)
    }

    fn sample_changes(&mut self, frame: &game::Frame, sampled_combat: bool) -> Vec<Record> {
        let mut records = Vec::new();
        if sampled_combat {
            for hit in &frame.hits {
                let mut hit = hit.clone();
                let tick = frame
                    .tick
                    .saturating_sub(u64::from(hit.ago.take().unwrap_or_default()));
                records.push(Record {
                    tick,
                    event: Event::Damage(hit),
                });
            }
        }
        for p in &frame.players {
            self.unit(
                frame.tick,
                &format!("p{}", p.index),
                p.weapon.as_deref(),
                p.shots.filter(|_| sampled_combat),
                p.fire.filter(|_| sampled_combat),
                p.actions.as_deref(),
                p.actions_ago.as_deref(),
                p.vehicle.as_deref(),
                p.seat,
                &mut records,
            );
        }
        for a in &frame.actors {
            self.unit(
                frame.tick,
                &a.id,
                a.weapon.as_deref(),
                a.shots.filter(|_| sampled_combat),
                a.fire.filter(|_| sampled_combat),
                a.actions.as_deref(),
                a.actions_ago.as_deref(),
                a.vehicle.as_deref(),
                a.seat,
                &mut records,
            );
        }
        records.sort_by_key(|r| r.tick);
        records
    }

    #[allow(clippy::too_many_arguments)]
    fn unit(
        &mut self,
        tick: u64,
        unit: &str,
        weapon: Option<&str>,
        shots: Option<u32>,
        fire: Option<u32>,
        actions: Option<&[String]>,
        ago: Option<&[u32]>,
        vehicle: Option<&str>,
        seat: Option<u32>,
        records: &mut Vec<Record>,
    ) {
        let emit_fire = |when, count| Record {
            tick: when,
            event: Event::WeaponFire {
                unit: unit.to_owned(),
                weapon: weapon.map(str::to_owned),
                count,
                barrel: None,
            },
        };
        let mut mask = fire.unwrap_or_default();
        let unresolved = shots.unwrap_or_default().saturating_sub(mask.count_ones());
        while mask != 0 {
            let age = mask.trailing_zeros();
            records.push(emit_fire(tick.saturating_sub(u64::from(age)), 1));
            mask &= mask - 1;
        }
        if unresolved > 0 {
            records.push(emit_fire(tick, unresolved));
        }
        for (i, action) in actions.unwrap_or_default().iter().enumerate() {
            if action == "pickup" {
                let age = ago.and_then(|v| v.get(i)).copied().unwrap_or_default();
                records.push(Record {
                    tick: tick.saturating_sub(u64::from(age)),
                    event: Event::Pickup {
                        unit: unit.to_owned(),
                    },
                });
            }
        }
        let previous = self.vehicles.get(unit);
        if previous.map(|v| (v.0.as_str(), v.1)) == vehicle.map(|v| (v, seat)) {
            return;
        }
        if let Some((vehicle, seat)) = self.vehicles.remove(unit) {
            records.push(Record {
                tick,
                event: Event::VehicleExited {
                    unit: unit.into(),
                    vehicle,
                    seat,
                },
            });
        }
        if let Some(vehicle) = vehicle {
            self.vehicles.insert(unit.into(), (vehicle.into(), seat));
            records.push(Record {
                tick,
                event: Event::VehicleEntered {
                    unit: unit.into(),
                    vehicle: vehicle.into(),
                    seat,
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_actions_damage_and_vehicle_transitions_survive_individual_payloads() {
        let mut tracker = Tracker::default();
        let mut frame = game::Frame {
            tick: 140,
            players: vec![game::FramePlayer {
                index: 2,
                weapon: Some("smg".into()),
                shots: Some(3),
                fire: Some(0b110),
                actions: Some(vec!["pickup".into()]),
                actions_ago: Some(vec![3]),
                vehicle: Some("c300".into()),
                seat: Some(1),
                ..Default::default()
            }],
            hits: vec![game::FrameHit {
                from: Some("p2".into()),
                to: Some("p1".into()),
                ago: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        };
        let direct = Tracker::default().frame_changes(&frame);
        assert!(matches!(
            &direct[..],
            [
                Record {
                    event: Event::Pickup { .. },
                    ..
                },
                Record {
                    event: Event::VehicleEntered { .. },
                    ..
                }
            ]
        ));
        let first = tracker.frame(&frame);
        assert!(first.windows(2).all(|r| r[0].tick <= r[1].tick));
        assert!(
            matches!(&first[0], Record { tick:137, event: Event::Pickup {unit} } if unit == "p2")
        );
        let shots: Vec<_> = first
            .iter()
            .filter_map(|r| match r.event {
                Event::WeaponFire { count, .. } => Some((r.tick, count)),
                _ => None,
            })
            .collect();
        assert_eq!(shots, [(138, 1), (139, 1), (140, 1)]);
        assert!(
            first.iter().any(
                |r| matches!(&r.event, Event::Damage(hit) if r.tick == 139 && hit.ago.is_none())
            )
        );
        for r in &first {
            assert_eq!(Record::decode(&r.encode().unwrap()).unwrap(), *r);
        }
        frame.tick += 2;
        frame.hits.clear();
        let p = &mut frame.players[0];
        p.actions = None;
        p.shots = None;
        p.fire = None;
        assert!(tracker.frame(&frame).is_empty());
        frame.players[0].seat = Some(0);
        let change = tracker.frame(&frame);
        assert!(matches!(
            &change[..],
            [
                Record {
                    event: Event::VehicleExited { seat: Some(1), .. },
                    ..
                },
                Record {
                    event: Event::VehicleEntered { seat: Some(0), .. },
                    ..
                }
            ]
        ));
    }

    #[test]
    fn stats_decoder_skips_future_highlights_and_checks_known_data() {
        let decode = Record::decode_telemetry;
        assert_eq!(
            decode(br#"{"tick":9,"kind":"NewFx","data":[{"id":4}]}"#).unwrap(),
            None
        );
        assert!(decode(br#"{"tick":9,"kind":"NewFx","data":[{]}"#).is_err());
        assert!(decode(br#"{"tick":9,"kind":"PlayerKilled","data":{}}"#).is_err());
        let left = Record {
            tick: 10,
            event: Event::PlayerLeft(game::PlayerLeft { index: 3 }),
        };
        let (kind, data) = left.event.telemetry().unwrap();
        assert_eq!(
            decode(&left.encode().unwrap()).unwrap(),
            Some((left.tick, kind, data))
        );
    }

    #[test]
    fn event_decode_rejects_truncation_and_excessive_lengths() {
        let record = Record {
            tick: 123,
            event: Event::PlayerLeft(game::PlayerLeft { index: 2 }),
        };
        let data = record.encode().unwrap();
        let wire: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(
            wire,
            json!({"tick":123, "kind":"PlayerLeft", "data":{"index":2}})
        );
        assert!(Record::decode(&data[..data.len() - 1]).is_err());
        assert!(Record::decode(&vec![b' '; MAX_EVENT_BYTES + 1]).is_err());
    }
}
