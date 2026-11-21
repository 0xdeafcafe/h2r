//! A typed stats record in a TICK event payload (subtype 1). The surrounding
//! envelope supplies the film tick. The Live event bodies retain their
//! match-clock ticks, including across a campaign revert.
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
    ActorKilled(game::ActorKilled),
    /// A Firefight `Score` command as credited. The tracker also writes the
    /// player's running total as an `ObjectiveScore` of game type `firefight`.
    FirefightScore(crate::firefight::FirefightScore),
    FirefightEnded(crate::firefight::FirefightEnded),
    ScoreChanged(game::ScoreChanged),
    GameEnded(game::GameEnded),
    /// A bounce off a superbounce crease. The Tracker follows it with a
    /// `Medal` for each medal of its chain count.
    Superbounce(game::Superbounce),
    /// A death falling out of the map while trying a bounce. The Tracker
    /// follows it with a `bounced` `Medal`.
    Bounced(game::Bounced),
    /// A teabag (docs/game/teabag.md). Both players lose the chance of an
    /// `incel` `Medal`, which the Tracker writes at `GameEnded`.
    Teabag(game::Teabag),
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
            Self::Superbounce(e) => Some((game::SUPERBOUNCE, json!(e))),
            Self::Bounced(e) => Some((game::BOUNCED, json!(e))),
            Self::Teabag(e) => Some((game::TEABAG, json!(e))),
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
    /// materialises its game records: the six of the first films, then
    /// `Superbounce`, `Bounced` and `Teabag`. A reader without a kind
    /// passes over it.
    /// A new
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
            game::SUPERBOUNCE => Event::Superbounce(body(row.data)?),
            game::BOUNCED => Event::Bounced(body(row.data)?),
            game::TEABAG => Event::Teabag(body(row.data)?),
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
    /// Who completes the game without a teabag, by generation.
    incel: medals::Incel,
    participants: BTreeMap<u32, super::PlayerSummary>,
    ended: bool,
    // The same slot may hold a different participant after a leave.
    generations: BTreeMap<u32, u32>,
    next_generation: u32,
    last_scoring: Option<(u32, u64)>,
    vehicles: BTreeMap<String, (String, Option<u32>)>,
    scores: BTreeMap<u32, (u32, i64)>,
    team_scores: BTreeMap<i32, i64>,
    objective_type: Option<String>,
    /// Each player's Firefight points so far: the `ObjectiveScore` they emit.
    firefight_scores: BTreeMap<u32, i64>,
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

    fn join(&mut self, player: &game::Player) -> Result<(), String> {
        let seat = u8::try_from(player.index).map_err(|_| "film player seat exceeds 255")?;
        let generation = self.generation(player.index);
        self.incel.joined(generation);
        self.participants.insert(
            generation,
            super::PlayerSummary {
                seat,
                name: player.name.clone(),
                xuid: player.xuid.map(|x| x.0),
                team: player.team,
                ..Default::default()
            },
        );
        Ok(())
    }

    /// Populate the menu using the same participant generations and medal
    /// decisions that emitted the TICK records. Guest rows arrive in join
    /// order. Match account/name first, then slot order for unlinked guests
    /// whose displayed names differ. A transient participant may appear
    /// only on the telemetry side between two GAME_TICK observations.
    pub fn summary_medals(&self, rows: &mut Vec<super::PlayerSummary>) {
        let mut available: Vec<_> = (0..rows.len()).collect();
        let mut assigned = BTreeMap::new();
        // Reserve every exact account before trying names, and every name
        // before using join order. An unlinked guest must not consume the
        // only unambiguous row belonging to a later occupant of the seat.
        for pass in 0..3 {
            for (&generation, p) in &self.participants {
                if assigned.contains_key(&generation) {
                    continue;
                }
                let candidate = available.iter().position(|&index| {
                    let row = &rows[index];
                    if p.seat != row.seat
                        || matches!((p.xuid, row.xuid), (Some(a), Some(b)) if a != b)
                    {
                        return false;
                    }
                    match pass {
                        0 => p.xuid.is_some() && p.xuid == row.xuid,
                        1 => p.name == row.name,
                        _ => true,
                    }
                });
                if let Some(at) = candidate {
                    assigned.insert(generation, available.remove(at));
                }
            }
        }
        for (&generation, participant) in &self.participants {
            let index = assigned.get(&generation).copied().unwrap_or_else(|| {
                rows.push(participant.clone());
                rows.len() - 1
            });
            rows[index].medals = self.medals.counts(generation, self.ended);
        }
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
                for player in &start.game.players {
                    self.join(player)?;
                }
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
            Event::PlayerJoined(joined) => {
                self.join(&joined.player)?;
                fallback_tick
            }
            Event::PlayerLeft(e) => {
                self.generations.remove(&e.index);
                self.scores.remove(&e.index);
                self.vehicles.remove(&format!("p{}", e.index));
                fallback_tick
            }
            Event::PlayerKilled(kill) => {
                self.incel.killed();
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
            Event::ActorKilled(e) => e.tick,
            Event::FirefightScore(e) => {
                let total = self.firefight_scores.entry(e.player).or_default();
                *total += i64::from(e.points);
                extra.push(Event::ObjectiveScore {
                    game_type: crate::firefight::GAME_TYPE.into(),
                    player: Some(e.player),
                    team: None,
                    score: *total,
                });
                e.tick
            }
            Event::FirefightEnded(e) => e.tick,
            Event::Superbounce(bounce) => {
                let player = self.generation(bounce.player);
                self.medals.bounce_with(player, bounce.chain, |medal| {
                    extra.push(Event::Medal {
                        player: bounce.player,
                        earned_at: bounce.tick,
                        medal: medal.into(),
                    });
                });
                bounce.tick
            }
            Event::Bounced(death) => {
                let player = self.generation(death.player);
                self.medals.bounced_with(player, |medal| {
                    extra.push(Event::Medal {
                        player: death.player,
                        earned_at: death.tick,
                        medal: medal.into(),
                    });
                });
                death.tick
            }
            Event::Teabag(bag) => {
                let giver = self.generation(bag.player);
                let victim = self.generation(bag.victim);
                self.incel.teabag(giver, victim);
                bag.tick
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
                let first = !std::mem::replace(&mut self.ended, true);
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
                // Everyone still in the game who completed it without a
                // teabag, in slot order (docs/game/teabag.md, "The medal").
                let incel: Vec<(u32, u32)> = self
                    .generations
                    .iter()
                    .filter(|&(_, g)| {
                        first
                            && self.participants.contains_key(g)
                            && self.incel.earns(*g, &e.reason)
                    })
                    .map(|(&slot, &g)| (slot, g))
                    .collect();
                for (slot, generation) in incel {
                    self.medals.incel_with(generation, |medal| {
                        extra.push(Event::Medal {
                            player: slot,
                            earned_at: e.tick,
                            medal: medal.into(),
                        });
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
            if let Some(participant) = self
                .generations
                .get(&p.index)
                .and_then(|g| self.participants.get_mut(g))
            {
                participant.score = p.score;
            }
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
            if !sampled_combat && game::validate_frame_player(p).is_err() { continue; }

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
            if !sampled_combat && game::validate_frame_actor(a).is_err() { continue; }

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
    fn firefight_scores_become_running_objective_scores() {
        let mut tracker = Tracker::default();
        tracker
            .event(
                game::GAME_STARTED,
                &json!({"host":"h","game_id":"g","map":"lockout",
            "mode":"multiplayer","team_play":false,"players":[
            {"index":0,"name":"A","team":0},{"index":1,"name":"B","team":0}]}),
                0,
            )
            .unwrap();
        let mut scored = |player: u32, points: u32, tick: u64| {
            let data = json!({"tick": tick, "player": player, "points": points, "reason": "kill", "class": "grunt"});
            tracker.event(game::FIREFIGHT_SCORE, &data, tick).unwrap()
        };
        let first = scored(0, 100, 10);
        assert!(matches!(&first[0].event, Event::FirefightScore(s) if s.points == 100));
        assert_eq!(
            first[1].event,
            Event::ObjectiveScore {
                game_type: "firefight".into(),
                player: Some(0),
                team: None,
                score: 100
            }
        );
        scored(1, 30, 11);
        let third = scored(0, 50, 12);
        assert!(matches!(&third[1].event, Event::ObjectiveScore { player: Some(0), score: 150, .. }));
        // Public only: Live's six telemetry records are untouched.
        assert!(first[0].event.telemetry().is_none());
        assert_eq!(Record::decode_telemetry(&first[0].encode().unwrap()).unwrap(), None);
    }

    #[test]
    fn incomplete_match_preserves_guest_results_and_only_awards_announced_medals() {
        use crate::film::PlayerSummary;
        let mut tracker = Tracker::default();
        tracker
            .event(
                game::GAME_STARTED,
                &json!({"host":"h","game_id":"g","map":"lockout",
            "mode":"multiplayer","team_play":false,"players":[
            {"index":2,"name":"Guest","team":0},
            {"index":3,"name":"Opponent","team":0}]}),
                0,
            )
            .unwrap();
        for tick in [4, 6] {
            tracker
                .event(
                    game::PLAYER_KILLED,
                    &json!({"tick":tick,"killer":2,"victim":3,"kind":"kill"}),
                    tick,
                )
                .unwrap();
        }
        tracker
            .event(game::PLAYER_LEFT, &json!({"index":2}), 7)
            .unwrap();
        tracker
            .event(
                game::PLAYER_JOINED,
                &json!({"player":{"index":2,"name":"Guest","team":1,"xuid":"0009000000000001"}}),
                9,
            )
            .unwrap();
        // Registered names match, guest appearances differ, row order is reversed.
        let mut rows = vec![
            PlayerSummary {
                seat: 2,
                name: "Guest".into(),
                xuid: Some(0x0009000000000001),
                score: 42,
                ..Default::default()
            },
            PlayerSummary {
                seat: 2,
                name: "Guest".into(),
                score: 17,
                ..Default::default()
            },
        ];
        tracker.summary_medals(&mut rows);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].medals.is_empty());
        assert_eq!(rows[0].score, 42);
        assert_eq!(rows[1].score, 17);
        assert_eq!(
            rows[1].medals,
            BTreeMap::from([("opening-act".into(), 1), ("twofer".into(), 1)])
        );
        assert_eq!(rows[2].name, "Opponent");
    }

    /// A bounce is a stats record, then one `Medal` per medal of its chain
    /// count at the bounce's tick, and the summary's map counts them. A
    /// bounce death is a record and a `bounced` medal.
    #[test]
    fn a_superbounce_records_its_medals_and_round_trips() {
        use crate::film::PlayerSummary;
        let mut tracker = Tracker::default();
        tracker
            .event(
                game::GAME_STARTED,
                &json!({"host":"h","game_id":"g","map":"lockout","mode":"multiplayer",
                    "team_play":false,"players":[{"index":3,"name":"Ada","team":0}]}),
                0,
            )
            .unwrap();
        let mut medals = Vec::new();
        let chains = [
            (40, 1),
            (70, 2),
            (95, 3),
            (120, 4),
            (150, 5),
            (180, 6),
            (210, 7),
            (400, 1),
        ];
        for (tick, chain) in chains {
            let records = tracker
                .event(
                    game::SUPERBOUNCE,
                    &json!({"tick":tick,"player":3,"chain":chain}),
                    tick + 9,
                )
                .unwrap();
            assert!(records.iter().all(|r| r.tick == tick));
            assert_eq!(
                records[0].event,
                Event::Superbounce(game::Superbounce {
                    tick,
                    player: 3,
                    chain
                })
            );
            for r in &records {
                assert_eq!(Record::decode(&r.encode().unwrap()).unwrap(), *r);
                match &r.event {
                    Event::Medal {
                        player,
                        earned_at,
                        medal,
                    } => {
                        assert_eq!((*player, *earned_at), (3, tick));
                        medals.push((chain, medal.clone()));
                    }
                    Event::Superbounce(_) => {}
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        let ids = |chain| {
            medals
                .iter()
                .filter(|(c, _)| *c == chain)
                .map(|(_, m)| m.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(2), ["superbounce", "double-bounce"]);
        assert_eq!(ids(4), ["superbounce", "quad-bounce"]);
        assert_eq!(ids(5), ["superbounce", "chain-bounce"]);
        assert_eq!(ids(6), ["superbounce", "bouncetacular"]);
        assert_eq!(ids(7), ["superbounce"]);

        let death = tracker
            .event(
                game::BOUNCED,
                &json!({"tick":450,"player":3,"chain":0}),
                460,
            )
            .unwrap();
        assert_eq!(
            death,
            [
                Record {
                    tick: 450,
                    event: Event::Bounced(game::Bounced {
                        tick: 450,
                        player: 3,
                        chain: 0,
                    }),
                },
                Record {
                    tick: 450,
                    event: Event::Medal {
                        player: 3,
                        earned_at: 450,
                        medal: "bounced".into()
                    },
                },
            ]
        );
        let mut rows = vec![PlayerSummary {
            seat: 3,
            name: "Ada".into(),
            ..Default::default()
        }];
        tracker.summary_medals(&mut rows);
        assert_eq!(
            rows[0].medals,
            BTreeMap::from([
                ("superbounce".into(), 8),
                ("double-bounce".into(), 1),
                ("triple-bounce".into(), 1),
                ("quad-bounce".into(), 1),
                ("chain-bounce".into(), 1),
                ("bouncetacular".into(), 1),
                ("bounced".into(), 1),
            ])
        );

        // The stats reader takes the bounce and the death, and passes over
        // their medals.
        let bounce = Record {
            tick: 70,
            event: Event::Superbounce(game::Superbounce {
                tick: 70,
                player: 3,
                chain: 2,
            }),
        };
        let wire: Value = serde_json::from_slice(&bounce.encode().unwrap()).unwrap();
        assert_eq!(
            wire,
            json!({"tick":70,"kind":"Superbounce","data":{"tick":70,"player":3,"chain":2}})
        );
        assert_eq!(
            Record::decode_telemetry(&bounce.encode().unwrap()).unwrap(),
            Some((
                70,
                game::SUPERBOUNCE,
                json!({"tick":70,"player":3,"chain":2})
            ))
        );
        assert_eq!(
            Record::decode_telemetry(&death[0].encode().unwrap()).unwrap(),
            Some((450, game::BOUNCED, json!({"tick":450,"player":3,"chain":0})))
        );
        assert_eq!(
            Record::decode_telemetry(&death[1].encode().unwrap()).unwrap(),
            None
        );
        assert!(
            Record::decode_telemetry(br#"{"tick":9,"kind":"Superbounce","data":{"player":1}}"#)
                .is_err()
        );
    }

    /// Incel at the game's end (docs/game/teabag.md, "The medal"): one
    /// `Medal` at `GameEnded`'s tick for each player who was in the game
    /// before its first death, is still in it, and was on neither side of a
    /// `Teabag`; a bot counts. A teabag is a stats record.
    #[test]
    fn incel_goes_to_who_completed_the_game_without_a_teabag() {
        use crate::film::PlayerSummary;
        let started = json!({"host":"h","game_id":"g","map":"lockout","mode":"multiplayer",
            "team_play":false,"players":[
                {"index":0,"name":"Ada","team":0},{"index":1,"name":"Ben","team":0},
                {"index":2,"name":"Cy","team":0},{"index":3,"name":"Clanker 3","team":0,"bot":true}]});
        let play = |reason: &str| {
            let mut tracker = Tracker::default();
            tracker.event(game::GAME_STARTED, &started, 0).unwrap();
            // Before anyone dies: on time.
            tracker
                .event(
                    game::PLAYER_JOINED,
                    &json!({"player":{"index":4,"name":"Di","team":0}}),
                    5,
                )
                .unwrap();
            tracker
                .event(
                    game::PLAYER_KILLED,
                    &json!({"tick":10,"killer":0,"victim":1,"kind":"kill"}),
                    10,
                )
                .unwrap();
            // After the first death: late.
            tracker
                .event(
                    game::PLAYER_JOINED,
                    &json!({"player":{"index":5,"name":"Eve","team":0}}),
                    12,
                )
                .unwrap();
            let bag = tracker
                .event(game::TEABAG, &json!({"tick":40,"player":0,"victim":1}), 41)
                .unwrap();
            assert_eq!(
                bag,
                [Record {
                    tick: 40,
                    event: Event::Teabag(game::Teabag {
                        tick: 40,
                        player: 0,
                        victim: 1
                    })
                }]
            );
            tracker
                .event(game::PLAYER_LEFT, &json!({"index":2}), 50)
                .unwrap();
            let end = tracker
                .event(
                    game::GAME_ENDED,
                    &json!({"tick":900,"reason":reason,"players":[]}),
                    900,
                )
                .unwrap();
            (tracker, end, bag)
        };
        let (tracker, end, bag) = play("finished");
        let incel: Vec<_> = end
            .iter()
            .filter_map(|r| match &r.event {
                Event::Medal {
                    player,
                    earned_at,
                    medal,
                } if medal == medals::INCEL => Some((*player, *earned_at, r.tick)),
                _ => None,
            })
            .collect();
        assert_eq!(incel, [(3, 900, 900), (4, 900, 900)]);
        let mut rows: Vec<_> = [
            (0, "Ada"),
            (1, "Ben"),
            (3, "Clanker 3"),
            (4, "Di"),
            (5, "Eve"),
        ]
        .into_iter()
        .map(|(seat, name)| PlayerSummary {
            seat,
            name: name.into(),
            ..Default::default()
        })
        .collect();
        tracker.summary_medals(&mut rows);
        let has = |name: &str| {
            rows.iter()
                .find(|r| r.name == name)
                .is_some_and(|r| r.medals.contains_key(medals::INCEL))
        };
        assert!(has("Clanker 3") && has("Di"));
        assert!(!has("Ada") && !has("Ben") && !has("Eve"));

        // A game that did not finish gives none.
        for reason in ["quit", "lost"] {
            let (_, end, _) = play(reason);
            assert!(
                !end.iter().any(
                    |r| matches!(&r.event, Event::Medal { medal, .. } if medal == medals::INCEL)
                ),
                "{reason}"
            );
        }

        // The stats reader takes the teabag and passes over the medal.
        assert_eq!(
            Record::decode_telemetry(&bag[0].encode().unwrap()).unwrap(),
            Some((40, game::TEABAG, json!({"tick":40,"player":0,"victim":1})))
        );
        let medal = end
            .iter()
            .find(|r| matches!(r.event, Event::Medal { .. }))
            .unwrap();
        assert_eq!(
            Record::decode_telemetry(&medal.encode().unwrap()).unwrap(),
            None
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
