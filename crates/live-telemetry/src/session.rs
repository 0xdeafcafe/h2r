use std::collections::BTreeMap;

use live_proto::game::{
    Frame, FrameActor, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle, GameEnded,
    MAX_FRAME_ACTORS, MAX_FRAME_HITS, MAX_FRAME_MACHINES, MAX_FRAME_OBJECTS, MAX_FRAME_SHOTS,
    MAX_FRAME_VEHICLES, ObjectKind, PlayerKilled, PlayerLeft, ScoreChanged,
};

use crate::event::{
    GameEvent, GameInfo, HitRecord, MachineState, ObjectState, Output, PlacedObject, Placements,
    PlayerState, Snapshot, VehicleState,
};
use crate::layout;
use crate::memory::yaw;
use crate::motion::{self, UnitMemory};
use crate::tags::TagNames;

/// Shots a player's held weapon fired since their previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingShots {
    weapon: u32,
    count: u32,
}

/// Weapons whose shots wait for the next frame's actors; past this many the
/// counts are dropped, not grown.
const MAX_ACTOR_WEAPONS: usize = 512;

/// Name for a vehicle or actor whose tag name is not known.
const UNKNOWN: &str = "unknown";

/// Most dead bodies a frame carries (with the `dead` stance, after the
/// living actors, within [`MAX_FRAME_ACTORS`]), nearest to a player first.
pub const MAX_FRAME_BODIES: usize = 32;

struct Match {
    game_id: String,
    map: String,
    campaign: bool,
    tick: u64,
    tick_rate: u64,
    team_play: bool,
    players: Vec<PlayerState>,
    last_emitted_scores: Vec<live_proto::game::PlayerScore>,
    last_emitted_teams: Vec<live_proto::game::TeamScore>,
    last_score_tick: u64,
    ended: bool,
    /// The loaded map's biped and weapon names, when the host found them.
    tags: Option<TagNames>,
    /// By player index; reset when the held weapon changes or the player
    /// has no weapon (dead, or none held).
    shots: BTreeMap<u32, PendingShots>,
    /// Every weapon's shots since the last frame, for actors (read only on
    /// frame ticks, so their held weapon is looked up then).
    actor_shots: BTreeMap<u32, u32>,
    /// Each player's and sent actor's unit at the previous frame, by unit
    /// datum, for `speed` and `actions`.
    units: BTreeMap<u32, UnitMemory>,
    /// Each machine as last sent, by datum: frames send only those that
    /// moved.
    machines_sent: BTreeMap<u32, MachineState>,
    /// Damage reports since the last frame (newest [`MAX_FRAME_HITS`]).
    hits: Vec<HitRecord>,
    /// The scenario's placements, for frame `objects`.
    placements: Placements,
    /// Scratch for [`frame_objects`], kept to spare an allocation a frame.
    found: Vec<u32>,
    /// Each player's unit while alive, and where it stood, by player index:
    /// a dead player is placed at their body (else where they fell).
    player_units: BTreeMap<u32, (u32, [f64; 3], f64)>,
    /// Every unit a player has had whose body is still in the world: those
    /// bodies are the players', not AI ones (the viewer leaves its own).
    player_bodies: std::collections::BTreeSet<u32>,
}

pub struct Session {
    host: String,
    console: Option<String>,
    /// The Live account signed in on local player slot 0, if any: that
    /// player's roster entries carry its XUID.
    signed_in: Option<live_proto::Xuid>,
    current: Option<Match>,
    /// Set while the console is in a matchmaking game: (name, id).
    playlist: Option<(String, String)>,
    /// The `MapId` of the map the next game starts on, when a package
    /// serves it (None: the disc's own map).
    map_id: Option<live_proto::maps::MapId>,
    /// The skulls the next multiplayer games run with (the host's set, as
    /// the boot applied it); campaign games read theirs from the array.
    multiplayer_skulls: Option<Vec<u8>>,
    /// Our own skulls (15 and up) the next campaign games run with; the
    /// array holds only the engine's fifteen.
    campaign_custom_skulls: Option<Vec<u8>>,
    output: Vec<Output>,
}

impl Session {
    pub fn new(host: impl Into<String>, console: Option<String>) -> Self {
        Self {
            host: host.into(),
            console,
            signed_in: None,
            current: None,
            playlist: None,
            map_id: None,
            multiplayer_skulls: None,
            campaign_custom_skulls: None,
            output: Vec::new(),
        }
    }

    /// Links the player on local slot 0 to the signed-in Live account.
    pub fn with_signed_in(mut self, xuid: Option<live_proto::Xuid>) -> Self {
        self.signed_in = xuid;
        self
    }

    /// Sets the signed-in account's XUID on the player who plays from local
    /// slot 0. Other players keep whatever the game said (nothing, today).
    fn link(&self, players: &mut [PlayerState]) {
        link(self.signed_in, players);
    }

    /// The matchmaking playlist (name, id) multiplayer games start from;
    /// `None` makes them custom games.
    pub fn set_playlist(&mut self, playlist: Option<(String, String)>) {
        self.playlist = playlist;
    }

    /// The loaded map's `MapId` for the games that start from now on
    /// (`NewGame.map_id`, docs/game/maps.md §5).
    pub fn set_map_id(&mut self, map_id: Option<live_proto::maps::MapId>) {
        self.map_id = map_id;
    }

    /// The skulls multiplayer games run with from now on
    /// (`NewGame.skulls`): only ones that work there are kept.
    pub fn set_multiplayer_skulls(&mut self, skulls: Option<Vec<u8>>) {
        self.multiplayer_skulls = skulls
            .and_then(live_proto::skulls::normalize)
            .filter(|s| live_proto::skulls::multiplayer_ok(s));
    }

    /// Our skulls campaign games run with from now on (`NewGame.skulls`,
    /// beside the array's): only indices past the engine's are kept.
    pub fn set_campaign_custom_skulls(&mut self, skulls: Option<Vec<u8>>) {
        self.campaign_custom_skulls = skulls
            .map(|s| {
                s.into_iter()
                    .filter(|i| *i >= live_proto::skulls::COUNT)
                    .collect::<Vec<_>>()
            })
            .and_then(live_proto::skulls::normalize);
    }

    pub fn take_output(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.output)
    }

    #[cfg(test)]
    pub fn game_id(&self) -> Option<&str> {
        self.current.as_ref().map(|m| m.game_id.as_str())
    }

    #[cfg(test)]
    pub fn start(&mut self, info: &GameInfo, players: Vec<PlayerState>) {
        self.start_with_teams(info, players, Vec::new());
    }

    pub fn start_with_teams(
        &mut self,
        info: &GameInfo,
        players: Vec<PlayerState>,
        teams: Vec<live_proto::game::TeamScore>,
    ) {
        let mut players = players;
        self.link(&mut players);
        // Campaign re-activates the same map on every checkpoint revert and
        // mission restart (`0x0018e700` calls the activation again with a
        // copy of the active options): that is still the same game.
        if let Some(current) = &mut self.current
            && !current.ended
            && current.campaign
            && info.mode == "campaign"
            && current.map == info.map
        {
            current.tick = info.tick;
            current.last_score_tick = info.tick;
            current.shots.clear();
            current.actor_shots.clear();
            current.units.clear();
            current.hits.clear();
            return;
        }
        self.end_previous();
        if !info.is_match() {
            return;
        }
        let game_id = live_proto::sign::random_id();
        let campaign = info.mode == "campaign";
        let playlist = if campaign {
            None
        } else {
            self.playlist.clone()
        };
        let kind = match (campaign, &playlist) {
            (true, _) => "campaign",
            (false, Some(_)) => "matchmaking",
            (false, None) => "custom",
        };
        let difficulty = if campaign {
            info.difficulty.clone()
        } else {
            None
        };
        let game = live_proto::game::NewGame {
            console: self.console.clone(),
            map: info.map.clone(),
            mode: info.mode.clone(),
            variant: info.variant.clone(),
            team_play: info.team_play,
            score_limit: info.score_limit,
            time_limit_s: info.time_limit_s,
            tick: Some(info.tick),
            players: players.iter().map(|p| p.wire_player()).collect(),
            kind: Some(kind.into()),
            playlist: playlist.as_ref().map(|p| p.0.clone()),
            playlist_id: playlist.map(|p| p.1),
            difficulty,
            // Live co-op does not exist yet.
            online: None,
            map_id: self.map_id,
            skulls: if campaign {
                live_proto::skulls::normalize(
                    info.skulls
                        .iter()
                        .chain(&self.campaign_custom_skulls)
                        .flatten()
                        .copied(),
                )
            } else {
                self.multiplayer_skulls.clone()
            },
        };
        self.output.push(Output::Event(GameEvent::new(
            game_id.clone(),
            live_proto::game::GAME_STARTED,
            live_proto::game::GameStarted {
                game_id: game_id.clone(),
                host: self.host.clone(),
                game,
            },
        )));
        self.current = Some(Match {
            game_id,
            map: info.map.clone(),
            campaign: info.mode == "campaign",
            tick: info.tick,
            tick_rate: u64::from(info.tick_rate.max(1)),
            team_play: info.team_play,
            last_emitted_scores: players
                .iter()
                .map(|p| p.stats.wire(p.player.index))
                .collect(),
            last_emitted_teams: teams,
            last_score_tick: info.tick,
            players,
            ended: false,
            tags: None,
            shots: BTreeMap::new(),
            actor_shots: BTreeMap::new(),
            units: BTreeMap::new(),
            machines_sent: BTreeMap::new(),
            hits: Vec::new(),
            placements: Placements::default(),
            found: Vec::new(),
            player_units: BTreeMap::new(),
            player_bodies: std::collections::BTreeSet::new(),
        });
    }

    /// The scenario's placements for the current match's frame `objects`;
    /// frames carry none until this is set.
    pub fn set_placements(&mut self, placements: Placements) {
        if let Some(current) = &mut self.current {
            current.placements = placements;
        }
    }

    /// Names for the current match's frame `species` and `weapon`; frames
    /// leave both out until (and unless) this is set.
    pub fn set_tag_names(&mut self, tags: TagNames) {
        if let Some(current) = &mut self.current {
            current.tags = Some(tags);
        }
    }

    fn end_previous(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        if current.ended {
            return;
        }
        let game_id = current.game_id.clone();
        Self::event_to(
            &mut self.output,
            game_id,
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: current.tick,
                reason: "quit".into(),
                winner: None,
                players: current
                    .players
                    .iter()
                    .map(|p| p.stats.wire(p.player.index))
                    .collect(),
            },
        );
    }

    fn event_to(
        output: &mut Vec<Output>,
        game_id: String,
        kind: &str,
        data: impl serde::Serialize,
    ) {
        output.push(Output::Event(GameEvent::new(game_id, kind, data)));
    }

    pub fn sample(&mut self, snapshot: &Snapshot) {
        let signed_in = self.signed_in;
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        current.tick = snapshot.tick;
        // Most ticks the roster is as it was: nothing joined or left, and
        // nothing to copy.
        if !same_roster(signed_in, &current.players, &snapshot.players) {
            let mut linked = snapshot.players.clone();
            link(signed_in, &mut linked);
            Self::roster_changed(&mut self.output, current, &snapshot.players, linked);
        }
        for (player, object) in snapshot.players.iter().zip(&snapshot.objects) {
            if let Some(o) = object.as_ref().filter(|_| player.unit != u32::MAX) {
                current
                    .player_units
                    .insert(player.player.index, (player.unit, o.pos, yaw(o.forward)));
                current.player_bodies.insert(player.unit);
            }
        }

        let scores = snapshot.scores();
        if snapshot.has_stats
            && snapshot.tick >= current.last_score_tick + current.tick_rate
            && (scores != current.last_emitted_scores
                || snapshot.teams != current.last_emitted_teams)
        {
            Self::event_to(
                &mut self.output,
                current.game_id.clone(),
                live_proto::game::SCORE_CHANGED,
                ScoreChanged {
                    tick: snapshot.tick,
                    players: scores.clone(),
                    teams: (!snapshot.teams.is_empty()).then(|| snapshot.teams.clone()),
                },
            );
            current.last_emitted_scores = scores;
            current.last_emitted_teams = snapshot.teams.clone();
            current.last_score_tick = snapshot.tick;
        }

        let mut shots = BTreeMap::new();
        for (player, object) in snapshot.players.iter().zip(&snapshot.objects) {
            let Some(weapon) = object.as_ref().and_then(|o| o.weapon) else {
                continue;
            };
            let index = player.player.index;
            let mut pending = current
                .shots
                .get(&index)
                .copied()
                .filter(|p| p.weapon == weapon.datum)
                .unwrap_or(PendingShots {
                    weapon: weapon.datum,
                    count: 0,
                });
            let fired = snapshot.shots.get(&weapon.datum).copied().unwrap_or(0);
            pending.count = pending.count.saturating_add(fired);
            shots.insert(index, pending);
        }
        current.shots = shots;
        for (weapon, n) in &snapshot.shots {
            if current.actor_shots.len() >= MAX_ACTOR_WEAPONS
                && !current.actor_shots.contains_key(weapon)
            {
                current.actor_shots.clear();
            }
            let count = current.actor_shots.entry(*weapon).or_insert(0);
            *count = count.saturating_add(*n);
        }

        current.hits.extend(snapshot.hits.iter().cloned());
        let over = current.hits.len().saturating_sub(MAX_FRAME_HITS);
        current.hits.drain(..over);

        if snapshot.tick.is_multiple_of(2) {
            let game_id = current.game_id.clone();
            let tags = current.tags.as_ref();
            let hits = std::mem::take(&mut current.hits);
            let player_of_unit: BTreeMap<u32, u32> = snapshot
                .players
                .iter()
                .filter(|p| p.unit != u32::MAX)
                .map(|p| (p.unit, p.player.index))
                .collect();
            let frame_hits = frame_hits(&hits, &player_of_unit, tags);
            // Units that dealt melee damage since the last frame.
            let meleed: std::collections::BTreeSet<u32> = hits
                .iter()
                .filter(|h| layout::damage_reporting_type(h.code).0 == "melee")
                .map(|h| h.owner_object)
                .collect();
            let previous = std::mem::take(&mut current.units);
            let tick = snapshot.tick;
            let tick_rate = current.tick_rate;
            let units = &mut current.units;
            let mut observe = |datum: u32, object: &ObjectState, alive: bool| {
                let (observed, memory) = motion::observe(
                    previous.get(&datum),
                    object,
                    alive,
                    tick,
                    tick_rate,
                    meleed.contains(&datum),
                );
                if alive {
                    units.insert(datum, memory);
                }
                observed
            };
            // A body the game has cleared away is no one's any more.
            let living: std::collections::BTreeSet<u32> = current
                .player_units
                .values()
                .map(|(unit, _, _)| *unit)
                .collect();
            current
                .player_bodies
                .retain(|d| living.contains(d) || snapshot.bodies.iter().any(|b| b.datum == *d));
            let player_units = &current.player_units;
            let players = snapshot
                .players
                .iter()
                .zip(&snapshot.objects)
                .map(|(player, object)| {
                    let alive = player.unit != u32::MAX;
                    // The dead lie at their body while it lasts, else where they fell.
                    let fallen = (!alive)
                        .then(|| player_units.get(&player.player.index))
                        .flatten()
                        .map(|(unit, pos, yaw)| {
                            snapshot
                                .bodies
                                .iter()
                                .find(|b| b.datum == *unit)
                                .map_or((*pos, *yaw), |b| {
                                    (b.object.pos, self::yaw(b.object.forward))
                                })
                        });
                    let index = player.player.index;
                    let shots = current.shots.get_mut(&index).map(|p| {
                        let n = p.count.min(MAX_FRAME_SHOTS);
                        p.count = 0;
                        n
                    });
                    let observed = match object {
                        Some(o) => observe(player.unit, o, alive),
                        None if !alive => motion::Observed {
                            stance: Some("dead".into()),
                            ..Default::default()
                        },
                        None => motion::Observed::default(),
                    };
                    FramePlayer {
                        index,
                        alive,
                        pos: object
                            .as_ref()
                            .map(|o| o.pos)
                            .or(fallen.map(|f| f.0))
                            .unwrap_or([0.0; 3]),
                        yaw: object
                            .as_ref()
                            .map(|o| yaw(o.forward))
                            .or(fallen.map(|f| f.1))
                            .unwrap_or(0.0),
                        vehicle: object
                            .as_ref()
                            .and_then(|o| o.vehicle)
                            .map(|v| format!("{v:08x}")),
                        species: object
                            .as_ref()
                            .and_then(|o| o.definition)
                            .zip(tags)
                            .and_then(|(d, t)| t.species(d)),
                        aim: object.as_ref().and_then(|o| o.aim),
                        weapon: object
                            .as_ref()
                            .and_then(|o| o.weapon)
                            .zip(tags)
                            .and_then(|(w, t)| t.weapon(w.definition)),
                        shots: shots.filter(|n| *n > 0),
                        stance: observed.stance,
                        speed: observed.speed,
                        actions: observed.actions,
                        grenade: observed.grenade,
                    }
                })
                .collect();
            let actor_shots = std::mem::take(&mut current.actor_shots);
            let mut actors = frame_actors(snapshot, tags, current.campaign, &actor_shots);
            for (actor, object, datum) in &mut actors {
                let observed = observe(*datum, object, true);
                actor.stance = observed.stance;
                actor.speed = observed.speed;
                actor.actions = observed.actions;
                actor.grenade = observed.grenade;
            }
            let vehicles = frame_vehicles(snapshot, &actors, tags);
            let bodies = frame_bodies(
                snapshot,
                &current.player_bodies,
                tags,
                current.campaign,
                MAX_FRAME_ACTORS.saturating_sub(actors.len()),
            );
            let machines = frame_machines(snapshot, &mut current.machines_sent, tags);
            let objects = frame_objects(&current.placements, &snapshot.placed, &mut current.found);
            self.output.push(Output::Frame(Frame {
                game_id,
                tick: snapshot.tick,
                players,
                vehicles,
                actors: actors
                    .into_iter()
                    .map(|(a, _, _)| a)
                    .chain(bodies)
                    .collect(),
                hits: frame_hits,
                bsp: snapshot.bsp,
                machines,
                objects,
            }));
        }
    }

    /// Joins and leaves between the current roster and `linked` (the
    /// fresh one, XUIDs linked), which then becomes the current one.
    fn roster_changed(
        output: &mut Vec<Output>,
        current: &mut Match,
        fresh: &[PlayerState],
        linked: Vec<PlayerState>,
    ) {
        let before: BTreeMap<u32, u32> = current
            .players
            .iter()
            .map(|p| (p.player.index, p.datum))
            .collect();
        let after: BTreeMap<u32, u32> = fresh.iter().map(|p| (p.player.index, p.datum)).collect();
        let left: Vec<u32> = before
            .iter()
            .filter(|(index, datum)| after.get(index) != Some(datum))
            .map(|(index, _)| *index)
            .collect();
        let joined: Vec<PlayerState> = linked
            .iter()
            .filter(|p| before.get(&p.player.index) != Some(&p.datum))
            .cloned()
            .collect();
        for index in left {
            Self::event_to(
                output,
                current.game_id.clone(),
                live_proto::game::PLAYER_LEFT,
                PlayerLeft { index },
            );
        }
        for player in joined {
            Self::event_to(
                output,
                current.game_id.clone(),
                live_proto::game::PLAYER_JOINED,
                live_proto::game::PlayerJoined {
                    player: player.wire_player(),
                },
            );
        }
        current.players = linked;
    }

    #[cfg(test)]
    fn is_campaign(&self) -> bool {
        self.current.as_ref().is_some_and(|m| m.campaign)
    }

    pub fn kill(
        &mut self,
        tick: u64,
        attacker: Option<u32>,
        victim: u32,
        betrayal: bool,
        object: Option<ObjectState>,
    ) {
        let kind = match attacker {
            None => "world",
            Some(killer) if killer == victim => "suicide",
            Some(_) if betrayal => "betrayal",
            Some(_) => "kill",
        };
        let Some(current) = &self.current else { return };
        if current.ended {
            return;
        }
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::PLAYER_KILLED,
            PlayerKilled {
                tick,
                victim,
                killer: attacker,
                weapon: None,
                kind: kind.into(),
                pos: object.map(|o| o.pos),
            },
        );
    }

    pub fn end(&mut self, snapshot: &Snapshot) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        current.ended = true;
        current.tick = snapshot.tick;
        let winner = snapshot.winner(current.team_play);
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: snapshot.tick,
                reason: "finished".into(),
                winner,
                players: snapshot.scores(),
            },
        );
    }
}

/// Gives local player 0 the signed-in account's XUID.
fn link(signed_in: Option<live_proto::Xuid>, players: &mut [PlayerState]) {
    let Some(xuid) = signed_in else { return };
    for p in players.iter_mut().filter(|p| p.local_user == Some(0)) {
        p.player.xuid = Some(xuid);
    }
}

/// Whether `current` is `fresh` once [`link`]ed, without copying it.
fn same_roster(
    signed_in: Option<live_proto::Xuid>,
    current: &[PlayerState],
    fresh: &[PlayerState],
) -> bool {
    current.len() == fresh.len()
        && current.iter().zip(fresh).all(|(c, f)| {
            // Every field named, so a new one cannot be missed.
            let PlayerState {
                datum,
                player,
                appearance,
                local_user,
                unit,
                stats,
            } = f;
            let live_proto::game::Player {
                index,
                name,
                xuid,
                team,
                local,
                appearance: wire_appearance,
            } = player;
            let xuid = match signed_in {
                Some(x) if *local_user == Some(0) => Some(x),
                _ => *xuid,
            };
            c.datum == *datum
                && c.appearance == *appearance
                && c.local_user == *local_user
                && c.unit == *unit
                && c.stats == *stats
                && c.player.index == *index
                && c.player.name == *name
                && c.player.xuid == xuid
                && c.player.team == *team
                && c.player.local == *local
                && c.player.appearance == *wire_appearance
        })
}

fn finite_direction(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite()) && v.iter().map(|c| c * c).sum::<f64>() > 1e-12
}

fn team_name(team: i16, campaign: bool) -> String {
    campaign
        .then(|| layout::campaign_team(team))
        .flatten()
        .map_or_else(|| team.to_string(), str::to_owned)
}

/// The frame's actors, nearest to any live player first, capped; each with
/// its object state (for vehicles).
fn frame_actors(
    snapshot: &Snapshot,
    tags: Option<&TagNames>,
    campaign: bool,
    shots: &BTreeMap<u32, u32>,
) -> Vec<(FrameActor, ObjectState, u32)> {
    let players: Vec<[f64; 3]> = snapshot.objects.iter().flatten().map(|o| o.pos).collect();
    let distance = |pos: [f64; 3]| {
        players
            .iter()
            .map(|p| (0..3).map(|i| (p[i] - pos[i]).powi(2)).sum::<f64>())
            .fold(f64::INFINITY, f64::min)
    };
    let mut actors: Vec<_> = snapshot
        .actors
        .iter()
        .filter(|a| finite_direction(a.object.forward))
        .map(|a| (distance(a.object.pos), a))
        .collect();
    actors.sort_by(|a, b| a.0.total_cmp(&b.0));
    actors
        .into_iter()
        .take(MAX_FRAME_ACTORS)
        .map(|(_, a)| {
            let o = &a.object;
            let actor = FrameActor {
                id: format!("{:08x}", a.datum),
                species: o
                    .definition
                    .zip(tags)
                    .and_then(|(d, t)| t.actor_species(d))
                    .unwrap_or_else(|| UNKNOWN.into()),
                pos: o.pos,
                yaw: yaw(o.forward),
                aim: o.aim,
                weapon: o.weapon.zip(tags).and_then(|(w, t)| t.weapon(w.definition)),
                shots: o
                    .weapon
                    .and_then(|w| shots.get(&w.datum))
                    .map(|n| (*n).min(MAX_FRAME_SHOTS))
                    .filter(|n| *n > 0),
                team: o.team.map(|t| team_name(t, campaign)),
                vehicle: o.vehicle.map(|v| format!("{v:08x}")),
                ..FrameActor::default()
            };
            (actor, o.clone(), a.datum)
        })
        .collect()
}

/// The dead bodies that are not players' (AI, and bodies the level starts
/// with), nearest to any live player first, at most [`MAX_FRAME_BODIES`]
/// and `room`: actors with the `dead` stance, there until the game clears
/// the body away.
fn frame_bodies(
    snapshot: &Snapshot,
    players: &std::collections::BTreeSet<u32>,
    tags: Option<&TagNames>,
    campaign: bool,
    room: usize,
) -> Vec<FrameActor> {
    let near: Vec<[f64; 3]> = snapshot.objects.iter().flatten().map(|o| o.pos).collect();
    let distance = |pos: [f64; 3]| {
        near.iter()
            .map(|p| (0..3).map(|i| (p[i] - pos[i]).powi(2)).sum::<f64>())
            .fold(f64::INFINITY, f64::min)
    };
    let mut bodies: Vec<_> = snapshot
        .bodies
        .iter()
        .filter(|b| {
            !players.contains(&b.datum)
                && finite_direction(b.object.forward)
                && b.object.pos.iter().all(|c| c.is_finite())
        })
        .map(|b| (distance(b.object.pos), b))
        .collect();
    bodies.sort_by(|a, b| a.0.total_cmp(&b.0));
    bodies
        .into_iter()
        .take(MAX_FRAME_BODIES.min(room))
        .map(|(_, b)| {
            let o = &b.object;
            FrameActor {
                id: format!("{:08x}", b.datum),
                species: o
                    .definition
                    .zip(tags)
                    .and_then(|(d, t)| t.actor_species(d))
                    .unwrap_or_else(|| UNKNOWN.into()),
                pos: o.pos,
                yaw: yaw(o.forward),
                team: o.team.map(|t| team_name(t, campaign)),
                stance: Some("dead".into()),
                ..FrameActor::default()
            }
        })
        .collect()
}

/// Every vehicle, occupied ones first, capped: those a player or a sent
/// actor rides (their transform as the rider read it), then the world
/// scan's, occupied when a biped is parented to it.
fn frame_vehicles(
    snapshot: &Snapshot,
    actors: &[(FrameActor, ObjectState, u32)],
    tags: Option<&TagNames>,
) -> Vec<FrameVehicle> {
    let kind = |definition: Option<u32>| {
        definition
            .zip(tags)
            .and_then(|(d, t)| t.vehicle(d))
            .unwrap_or_else(|| UNKNOWN.into())
    };
    let hex = |d: u32| format!("{d:08x}");
    // The world scan's word on each vehicle: what it is mounted on and
    // where it aims (a turret), also for one a rider read.
    let scanned: BTreeMap<u32, &VehicleState> =
        snapshot.vehicles.iter().map(|v| (v.datum, v)).collect();
    let aim = |d: u32| {
        scanned
            .get(&d)
            .and_then(|v| v.aim)
            .filter(|a| finite_direction(*a))
    };
    let mut seen = std::collections::BTreeSet::new();
    let ridden = snapshot
        .objects
        .iter()
        .flatten()
        .chain(actors.iter().map(|(_, o, _)| o))
        .filter_map(|o| {
            let id = o.vehicle?;
            if !finite_direction(o.forward) || !finite_direction(o.up) || !seen.insert(id) {
                return None;
            }
            Some(FrameVehicle {
                id: hex(id),
                kind: kind(o.vehicle_definition),
                pos: o.pos,
                forward: o.forward,
                up: o.up,
                occupied: true,
                parent: o
                    .mount
                    .or_else(|| scanned.get(&id).and_then(|v| v.parent))
                    .map(hex),
                aim: aim(id),
            })
        })
        .collect::<Vec<_>>();
    let mut world: Vec<_> = snapshot
        .vehicles
        .iter()
        .filter(|v| {
            v.pos.iter().all(|c| c.is_finite())
                && finite_direction(v.forward)
                && finite_direction(v.up)
                && seen.insert(v.datum)
        })
        .map(|v| FrameVehicle {
            id: hex(v.datum),
            kind: kind(v.definition),
            pos: v.pos,
            forward: v.forward,
            up: v.up,
            occupied: v.occupied,
            parent: v.parent.map(hex),
            aim: aim(v.datum),
        })
        .collect();
    world.sort_by_key(|v| !v.occupied);
    ridden
        .into_iter()
        .chain(world)
        .take(MAX_FRAME_VEHICLES)
        .collect()
}

/// The machines that moved since they were last sent (all of them the
/// first time), capped; remembers what it sends.
fn frame_machines(
    snapshot: &Snapshot,
    sent: &mut BTreeMap<u32, MachineState>,
    tags: Option<&TagNames>,
) -> Vec<FrameMachine> {
    const EPSILON: f64 = 1e-3;
    let near = |a: &[f64; 3], b: &[f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < EPSILON);
    let moved = |m: &MachineState| match sent.get(&m.datum) {
        None => true,
        Some(s) => {
            !near(&s.pos, &m.pos)
                || !near(&s.forward, &m.forward)
                || !near(&s.up, &m.up)
                || match (s.position, m.position) {
                    (Some(a), Some(b)) => (a - b).abs() >= EPSILON,
                    (a, b) => a.is_some() != b.is_some(),
                }
        }
    };
    let changed: Vec<MachineState> = snapshot
        .machines
        .iter()
        .filter(|m| {
            m.pos.iter().all(|c| c.is_finite())
                && finite_direction(m.forward)
                && finite_direction(m.up)
                && moved(m)
        })
        .take(MAX_FRAME_MACHINES)
        .cloned()
        .collect();
    changed
        .into_iter()
        .map(|m| {
            let kind = m
                .definition
                .zip(tags)
                .and_then(|(d, t)| t.machine(d))
                .unwrap_or_else(|| UNKNOWN.into());
            let node = m
                .placement
                .filter(|_| kind != UNKNOWN)
                .map(|i| format!("{kind}_{i}"))
                .filter(|n| live_proto::game::valid_node(n));
            let frame = FrameMachine {
                node,
                id: format!("{:08x}", m.datum),
                kind,
                pos: m.pos,
                forward: m.forward,
                up: m.up,
                position: m.position,
            };
            sent.insert(m.datum, m);
            frame
        })
        .collect()
}

/// How far an object may be from its placement and still count as there:
/// a fiftieth of a world unit, and two degrees (cosine) of either axis.
const PLACED_DISTANCE: f64 = 0.02;
const PLACED_COS: f64 = 0.999_390_827;

/// Every placement whose object is not where the scenario put it, by kind
/// then index: moved by more than [`PLACED_DISTANCE`] or turned more than
/// two degrees (with its pose), or gone: no object, or (weapons, equipment
/// and netgame spawns) the object is held. Other parented objects are
/// left out (their transform is parent-relative).
fn frame_objects(
    placements: &Placements,
    placed: &[PlacedObject],
    found: &mut Vec<u32>,
) -> Vec<FrameObject> {
    const NONE: u32 = u32::MAX;
    let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let unit = |v: [f64; 3]| {
        let len = dot(v, v).sqrt();
        (len.is_finite() && len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
    };
    // Each kind's run of `found` (one slot a placement, `placements.kinds`
    // holds each kind once), by kind; one pass over `placed` fills them all.
    const KINDS: usize = 32;
    let mut runs = [(0usize, 0usize); KINDS];
    let mut total = 0;
    for (kind, poses) in &placements.kinds {
        if let Some(run) = runs.get_mut(*kind as usize) {
            *run = (total, poses.len());
            total += poses.len();
        }
    }
    found.clear();
    found.resize(total, NONE);
    for (n, p) in placed.iter().enumerate() {
        let Some(&(start, len)) = runs.get(p.kind as usize) else {
            continue;
        };
        if usize::from(p.index) >= len {
            continue;
        }
        let slot = &mut found[start + usize::from(p.index)];
        // One on its spawn beats one a player holds.
        if *slot == NONE || (placed[*slot as usize].parented && !p.parented) {
            *slot = n as u32;
        }
    }
    let mut out = Vec::new();
    for (kind, poses) in &placements.kinds {
        let Some(&(start, len)) = runs.get(*kind as usize) else {
            continue;
        };
        let found = &found[start..start + len];
        let held_is_gone = matches!(
            kind,
            ObjectKind::Weapon | ObjectKind::Equipment | ObjectKind::NetgameEquipment
        );
        for (i, (pose, slot)) in poses.iter().zip(found.iter()).enumerate() {
            let object = (*slot != NONE).then(|| &placed[*slot as usize]);
            let entry = match object {
                Some(o) if o.parented && !held_is_gone => None,
                None | Some(PlacedObject { parented: true, .. }) => Some(FrameObject {
                    k: *kind,
                    i: i as u32,
                    p: None,
                    f: None,
                    u: None,
                    gone: true,
                }),
                Some(o) => {
                    let (Some(forward), Some(up)) = (unit(o.forward), unit(o.up)) else {
                        continue;
                    };
                    if !o.pos.iter().all(|c| c.is_finite()) {
                        continue;
                    }
                    let moved = pose.is_none_or(|pose| {
                        let d = [0, 1, 2].map(|k| o.pos[k] - pose.pos[k]);
                        dot(d, d) > PLACED_DISTANCE * PLACED_DISTANCE
                            || dot(forward, pose.forward) < PLACED_COS
                            || dot(up, pose.up) < PLACED_COS
                    });
                    moved.then_some(FrameObject {
                        k: *kind,
                        i: i as u32,
                        p: Some(o.pos),
                        f: Some(forward),
                        u: Some(up),
                        gone: false,
                    })
                }
            };
            if let Some(entry) = entry {
                if out.len() == MAX_FRAME_OBJECTS {
                    return out;
                }
                out.push(entry);
            }
        }
    }
    out
}

/// The frame's hits: attacker and victim as `p<index>` for a player's unit
/// (or the reporting player), else the object datum; the weapon from the
/// damage reporting type, else the damage effect's stem.
fn frame_hits(
    hits: &[HitRecord],
    player_of_unit: &BTreeMap<u32, u32>,
    tags: Option<&TagNames>,
) -> Vec<FrameHit> {
    let object = |datum: u32| {
        (datum != u32::MAX).then(|| match player_of_unit.get(&datum) {
            Some(index) => format!("p{index}"),
            None => format!("{datum:08x}"),
        })
    };
    hits.iter()
        .filter_map(|h| {
            let pos = h.pos?;
            let (kind, weapon) = layout::damage_reporting_type(h.code);
            let from = if h.owner_player != u32::MAX {
                Some(format!("p{}", h.owner_player & 0xffff))
            } else {
                object(h.owner_object)
            };
            let amount = f64::from(h.body.max(0.0)) + f64::from(h.shield.max(0.0));
            Some(FrameHit {
                from,
                to: object(h.victim),
                weapon: weapon.map(str::to_owned).or_else(|| {
                    tags.and_then(|t| t.damage_effect(h.effect))
                        .filter(|s| live_proto::game::valid_model_id(s))
                }),
                pos,
                amount: amount.is_finite().then_some(amount),
                kind: Some(kind.into()),
            })
        })
        .take(MAX_FRAME_HITS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Appearance, ObjectState, PlayerStats};

    fn info() -> GameInfo {
        GameInfo {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            difficulty: None,
            skulls: None,
            variant: Some("Slayer".into()),
            team_play: false,
            score_limit: Some(25),
            time_limit_s: Some(600),
            tick_rate: 30,
            tick: 0,
        }
    }

    fn player(index: u32, score: i64) -> PlayerState {
        PlayerState {
            datum: 0x80010000 | index,
            player: live_proto::game::Player {
                index,
                name: format!("P{index}"),
                xuid: None,
                team: 0,
                local: Some(index == 0),
                appearance: None,
            },
            appearance: Appearance::default(),
            local_user: (index == 0).then_some(0),
            unit: u32::MAX,
            stats: PlayerStats {
                score,
                ..PlayerStats::ZERO
            },
        }
    }

    fn snapshot(tick: u64, players: Vec<PlayerState>) -> Snapshot {
        Snapshot {
            tick,
            has_stats: true,
            objects: players.iter().map(|_| None).collect(),
            players,
            teams: Vec::new(),
            shots: BTreeMap::new(),
            actors: Vec::new(),
            vehicles: Vec::new(),
            machines: Vec::new(),
            placed: Vec::new(),
            bodies: Vec::new(),
            bsp: None,
            hits: Vec::new(),
        }
    }

    fn kinds(session: &mut Session) -> Vec<String> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e.kind),
                Output::Frame(_) => None,
            })
            .collect()
    }

    #[test]
    fn start_roster_score_and_end_events() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameStarted"]);
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerJoined"]);
        session.sample(&snapshot(4, vec![player(0, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerLeft"]);

        session.sample(&snapshot(15, vec![player(0, 1)]));
        assert!(
            kinds(&mut session).is_empty(),
            "score changes wait one second"
        );
        session.sample(&snapshot(30, vec![player(0, 2)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);
        session.sample(&snapshot(31, vec![player(0, 3)]));
        assert!(kinds(&mut session).is_empty());
        session.sample(&snapshot(60, vec![player(0, 4)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);

        session.end(&snapshot(61, vec![player(0, 5)]));
        let outputs = session.take_output();
        let Output::Event(event) = &outputs[0] else {
            panic!()
        };
        assert_eq!(event.kind, "GameEnded");
        assert_eq!(event.data["winner"]["player"], 0);
        assert_eq!(event.data["players"][0]["score"], 5);
        session.end(&snapshot(62, vec![player(0, 6)]));
        assert!(session.take_output().is_empty(), "end is idempotent");
    }

    #[test]
    fn roster_events_carry_appearance() {
        let mut session = Session::new("test", None);
        let mut host = player(0, 0);
        host.appearance = Appearance {
            primary: Some(5),
            secondary: Some(9),
            tertiary: Some(2),
            quaternary: Some(11),
            emblem_foreground: Some(30),
            emblem_background: Some(7),
        };
        session.start(&info(), vec![host, player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.kind, "GameStarted");
        assert_eq!(
            started.data["players"][0]["appearance"],
            serde_json::json!({
                "primary": 5, "secondary": 9, "tertiary": 2, "quaternary": 11,
                "emblem_foreground": 30, "emblem_background": 7
            })
        );
        assert!(
            started.data["players"][1].get("appearance").is_none(),
            "an unknown appearance is omitted entirely"
        );

        // A join carries the new player's appearance; an unchanged roster
        // never re-sends one, and there is no appearance anywhere else.
        let mut joiner = player(2, 0);
        joiner.appearance = Appearance {
            primary: Some(1),
            ..Appearance::default()
        };
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), joiner]));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "PlayerJoined");
        assert_eq!(events[0].data["player"]["index"], 2);
        assert_eq!(events[0].data["player"]["appearance"]["primary"], 1);

        session.sample(&snapshot(
            30,
            vec![player(0, 1), player(1, 0), player(2, 0)],
        ));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "ScoreChanged");
        assert!(
            events[0].data.get("appearance").is_none(),
            "appearance never rides score or frame events"
        );
    }

    #[test]
    fn kills_include_kind_victim_position_and_attribution() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.take_output();
        let object = ObjectState {
            pos: [4.0, 5.0, 6.0],
            forward: [1.0, 0.0, 0.0],
            ..ObjectState::default()
        };
        session.kill(9, Some(0), 1, false, Some(object.clone()));
        session.kill(10, Some(0), 0, false, Some(object.clone()));
        session.kill(11, Some(0), 1, true, Some(object.clone()));
        session.kill(12, None, 1, false, Some(object));
        let events = session.take_output();
        let expected = [
            ("kill", 9),
            ("suicide", 10),
            ("betrayal", 11),
            ("world", 12),
        ];
        for (output, (kind, tick)) in events.into_iter().zip(expected) {
            let Output::Event(event) = output else {
                panic!()
            };
            assert_eq!(event.kind, "PlayerKilled");
            assert_eq!(event.data["kind"], kind);
            assert_eq!(event.data["tick"], tick);
            assert_eq!(event.data["pos"], serde_json::json!([4.0, 5.0, 6.0]));
        }
    }

    #[test]
    fn post_end_kills_are_ignored() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.end(&snapshot(30, vec![player(0, 0), player(1, 0)]));
        session.take_output();
        session.kill(31, Some(0), 1, false, None);
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn replacement_and_nonmatch_close_the_previous_game() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        let mut menu = info();
        menu.mode = "main_menu".into();
        session.start(&menu, Vec::new());
        assert_eq!(kinds(&mut session), ["GameEnded"]);
        session.sample(&snapshot(2, vec![player(0, 0)]));
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn frames_are_every_second_tick_with_yaw_and_vehicle() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [0.0, -1.0, 0.0],
            vehicle: Some(0x90020001),
            ..ObjectState::default()
        });
        session.sample(&state);
        assert!(session.take_output().is_empty());
        state.tick = 2;
        session.sample(&state);
        let outputs = session.take_output();
        let Output::Frame(frame) = &outputs[0] else {
            panic!()
        };
        assert_eq!(frame.players[0].yaw, -std::f64::consts::FRAC_PI_2);
        assert_eq!(frame.players[0].vehicle.as_deref(), Some("90020001"));
        assert!(frame.players[0].alive);
    }

    fn frames(session: &mut Session) -> Vec<Frame> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Frame(f) => Some(f),
                Output::Event(_) => None,
            })
            .collect()
    }

    fn armed(weapon: u32, definition: u32) -> ObjectState {
        ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            definition: Some(0xe100_0010),
            aim: Some([0.0, 0.6, 0.8]),
            weapon: Some(crate::event::Weapon {
                datum: weapon,
                definition,
            }),
            ..ObjectState::default()
        }
    }

    fn names() -> TagNames {
        TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0010,
                    *b"bipd",
                    "objects\\characters\\elite\\elite_mp".to_string(),
                ),
                (
                    0xe100_0020,
                    *b"weap",
                    "objects\\weapons\\rifle\\battle_rifle\\battle_rifle".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn frames_carry_species_aim_and_weapon_from_tag_names() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone(), player(1, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![p.clone(), player(1, 0)]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));

        // Without names, only the aim (read straight from memory) is sent.
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].aim, Some([0.0, 0.6, 0.8]));
        assert_eq!(frame.players[0].species, None);
        assert_eq!(frame.players[0].weapon, None);

        session.set_tag_names(names());
        state.tick = 4;
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].species.as_deref(), Some("elite"));
        assert_eq!(frame.players[0].weapon.as_deref(), Some("battle_rifle"));
        // A dead player (no unit, no object) has none of them, only the
        // stance.
        assert_eq!(
            frame.players[1],
            FramePlayer {
                index: 1,
                stance: Some("dead".into()),
                ..FramePlayer::default()
            }
        );
        assert_eq!(
            serde_json::to_value(&frame.players[0]).unwrap()["species"],
            "elite"
        );

        // An unknown definition sends nothing rather than a guess.
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0099));
        state.tick = 6;
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);

        // A new match forgets the old map's names.
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(8, vec![p]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);
    }

    #[test]
    fn shots_add_up_between_frames_and_reset_on_weapon_change_and_death() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let rifle = 0x9005_0005;
        let pistol = 0x9006_0006;
        let sample =
            |session: &mut Session, tick: u64, weapon: Option<u32>, fired: &[(u32, u32)]| {
                let mut state = snapshot(tick, vec![p.clone()]);
                state.objects[0] = weapon.map(|w| armed(w, 0xe100_0020));
                state.shots = fired.iter().copied().collect();
                session.sample(&state);
                frames(session).first().map(|f| f.players[0].shots)
            };

        // Odd ticks add up into the next even tick's frame.
        assert_eq!(sample(&mut session, 1, Some(rifle), &[(rifle, 2)]), None);
        assert_eq!(
            sample(&mut session, 2, Some(rifle), &[(rifle, 1), (pistol, 4)]),
            Some(Some(3)),
            "only the held weapon's shots count"
        );
        assert_eq!(
            sample(&mut session, 4, Some(rifle), &[]),
            Some(None),
            "0 is absent"
        );
        // Switching weapons drops shots counted for the old one.
        assert_eq!(sample(&mut session, 5, Some(rifle), &[(rifle, 5)]), None);
        assert_eq!(
            sample(&mut session, 6, Some(pistol), &[(pistol, 1)]),
            Some(Some(1))
        );
        // Death drops pending shots, and a dead player sends none.
        assert_eq!(sample(&mut session, 7, Some(pistol), &[(pistol, 2)]), None);
        assert_eq!(sample(&mut session, 8, None, &[(pistol, 1)]), Some(None));
        // Counts are capped at the protocol's limit.
        assert_eq!(
            sample(&mut session, 10, Some(rifle), &[(rifle, 500)]),
            Some(Some(MAX_FRAME_SHOTS))
        );
    }

    fn campaign() -> GameInfo {
        GameInfo {
            map: "03a_oldmombasa".into(),
            mode: "campaign".into(),
            difficulty: Some("heroic".into()),
            skulls: None,
            variant: None,
            team_play: false,
            score_limit: None,
            time_limit_s: None,
            tick_rate: 30,
            tick: 0,
        }
    }

    #[test]
    fn campaign_reverts_continue_the_same_game() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        assert!(session.is_campaign());
        let id = session.game_id().unwrap().to_owned();
        assert_eq!(kinds(&mut session), ["GameStarted"]);
        session.sample(&snapshot(600, vec![player(0, 0)]));
        session.take_output();
        // A checkpoint revert: same map, the clock goes back.
        let mut revert = campaign();
        revert.tick = 400;
        session.start(&revert, vec![player(0, 0)]);
        assert!(kinds(&mut session).is_empty(), "no new game");
        assert_eq!(session.game_id(), Some(id.as_str()));
        session.sample(&snapshot(402, vec![player(0, 0)]));
        assert_eq!(frames(&mut session)[0].tick, 402);

        // The next mission is a new game; so is a multiplayer map.
        let mut next = campaign();
        next.map = "03b_newmombasa".into();
        session.start(&next, vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        assert_ne!(session.game_id(), Some(id.as_str()));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        // Multiplayer re-activations stay separate games, as before.
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
    }

    fn started(session: &mut Session) -> live_proto::game::NewGame {
        let event = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "GameStarted" => Some(e),
                _ => None,
            })
            .expect("a GameStarted");
        let game: live_proto::game::GameStarted = serde_json::from_value(event.data).unwrap();
        assert_eq!(live_proto::game::validate_new(&game.game), Ok(()));
        game.game
    }

    #[test]
    fn games_carry_the_loaded_maps_id() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).map_id, None, "the disc's map");
        let id = live_proto::maps::MapId([3; 32]);
        session.set_map_id(Some(id));
        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.map_id, Some(id));
        assert_eq!(game.map, info().map, "map stays the base map's name");
    }

    #[test]
    fn games_carry_their_skulls() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, None, "none on is left out");
        // Multiplayer: the host's set as the boot applied it.
        session.set_multiplayer_skulls(Some(vec![6, 4]));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![4, 6]));
        // A skull that does nothing in multiplayer is never claimed.
        session.set_multiplayer_skulls(Some(vec![12]));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, None);
        // Campaign: the array as read, whatever the multiplayer set.
        session.set_multiplayer_skulls(Some(vec![6]));
        let campaign = GameInfo {
            map: "03a_oldmombasa".into(),
            mode: "campaign".into(),
            skulls: Some(vec![0, 12]),
            ..info()
        };
        session.start(&campaign, vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.skulls, Some(vec![0, 12]));
        assert_eq!(live_proto::game::validate_new(&game), Ok(()));
        // Our own skulls join the array's in a campaign, never a
        // multiplayer game; engine indices handed in that way are ignored.
        session.set_campaign_custom_skulls(Some(vec![20, 3, 15]));
        // (The next mission: the same one again continues its game.)
        let next = GameInfo {
            map: "03b_newmombasa".into(),
            ..campaign.clone()
        };
        session.start(&next, vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.skulls, Some(vec![0, 12, 15, 20]));
        assert_eq!(live_proto::game::validate_new(&game), Ok(()));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![6]));
        session.set_campaign_custom_skulls(Some(vec![17]));
        let bare = GameInfo {
            skulls: None,
            ..campaign
        };
        session.start(&bare, vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![17]));
    }

    #[test]
    fn games_say_their_kind_playlist_and_difficulty() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("campaign"));
        assert_eq!(game.difficulty.as_deref(), Some("heroic"));
        assert_eq!((game.playlist, game.online), (None, None));

        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("custom"));
        assert_eq!((game.playlist, game.difficulty), (None, None));

        session.set_playlist(Some(("Team Slayer".into(), "3".into())));
        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("matchmaking"));
        assert_eq!(game.playlist.as_deref(), Some("Team Slayer"));
        assert_eq!(game.playlist_id.as_deref(), Some("3"));

        // A playlist never makes a campaign game a matchmaking one.
        session.start(&campaign(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("campaign"));
        assert_eq!(game.playlist, None);
    }

    fn actor(slot: u32, pos: [f64; 3], team: i16) -> crate::event::ActorState {
        crate::event::ActorState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            object: ObjectState {
                pos,
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.0, 1.0],
                definition: Some(0xe100_0011),
                team: Some(team),
                ..ObjectState::default()
            },
        }
    }

    fn campaign_names() -> TagNames {
        TagNames::new(
            1,
            [
                (
                    0xe100_0011,
                    *b"bipd",
                    "objects\\characters\\grunt\\grunt".to_string(),
                ),
                (
                    0xe100_0020,
                    *b"weap",
                    "objects\\weapons\\pistol\\plasma_pistol\\plasma_pistol".to_string(),
                ),
                (
                    0xe100_0030,
                    *b"vehi",
                    "objects\\vehicles\\ghost\\ghost".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn the_dead_lie_at_their_bodies_and_ai_bodies_stay_until_cleared() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(campaign_names());
        session.take_output();
        let standing = |pos| ObjectState {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        };
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(standing([3.0, 4.0, 5.0]));
        session.sample(&state);
        frames(&mut session);

        // The player dies; their body has slid a little. A grunt's body and
        // a living grunt are about.
        let mut dead = p.clone();
        dead.unit = u32::MAX;
        let mut state = snapshot(4, vec![dead.clone()]);
        let mut own = actor(1, [3.5, 4.0, 5.0], 0);
        own.datum = 0x9001_0001;
        own.object.forward = [0.0, 1.0, 0.0];
        let grunt_body = actor(2, [9.0, 0.0, 0.0], 3);
        state.bodies = vec![own, grunt_body];
        state.actors = vec![actor(3, [20.0, 0.0, 0.0], 3)];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        let me = &frame.players[0];
        assert!(!me.alive);
        assert_eq!(me.pos, [3.5, 4.0, 5.0], "at the body");
        assert!((me.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(me.stance.as_deref(), Some("dead"));
        // The living first, then the AI body as a dead actor; the player's
        // own body is not an actor.
        assert_eq!(frame.actors.len(), 2);
        assert_eq!(frame.actors[0].id, "90030003");
        assert_ne!(frame.actors[0].stance.as_deref(), Some("dead"));
        let body = &frame.actors[1];
        assert_eq!(body.id, "90020002");
        assert_eq!(body.stance.as_deref(), Some("dead"));
        assert_eq!(body.species, "grunt");
        assert_eq!(body.pos, [9.0, 0.0, 0.0]);
        assert_eq!(body.team.as_deref(), Some("covenant"));
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));

        // The game clears the player's body away: they lie where they fell.
        let mut state = snapshot(6, vec![dead]);
        state.bodies = vec![];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.players[0].pos, [3.0, 4.0, 5.0]);
        assert!(frame.actors.is_empty(), "the grunt's body is gone too");
    }

    #[test]
    fn bodies_are_capped_within_the_actors() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![player(0, 0)]);
        state.actors = (0..MAX_FRAME_ACTORS as u32 - 4)
            .map(|i| actor(i, [0.0; 3], 3))
            .collect();
        state.bodies = (0..10)
            .map(|i| actor(200 + i, [i as f64, 0.0, 0.0], 3))
            .collect();
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.actors.len(), MAX_FRAME_ACTORS);
        let dead = frame
            .actors
            .iter()
            .filter(|a| a.stance.as_deref() == Some("dead"))
            .count();
        assert_eq!(dead, 4, "the nearest bodies fill what room is left");
        state.actors.clear();
        state.tick = 4;
        state.bodies = (0..100)
            .map(|i| actor(200 + i, [i as f64, 0.0, 0.0], 3))
            .collect();
        session.sample(&state);
        assert_eq!(
            frames(&mut session).remove(0).actors.len(),
            MAX_FRAME_BODIES
        );
    }

    #[test]
    fn campaign_frames_carry_actors_and_occupied_vehicles() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(campaign_names());
        session.take_output();

        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        });
        // An odd tick's shots wait for the frame.
        state.shots = [(0x9009_0009, 2)].into();
        session.sample(&state);
        assert!(frames(&mut session).is_empty());

        let far = actor(3, [50.0, 0.0, 0.0], 3);
        let mut near = actor(4, [5.0, 0.0, 0.0], 2);
        near.object.weapon = Some(crate::event::Weapon {
            datum: 0x9009_0009,
            definition: 0xe100_0020,
        });
        near.object.aim = Some([1.0, 0.0, 0.0]);
        let mut driver = actor(5, [20.0, 0.0, 1.0], 3);
        driver.object.vehicle = Some(0x9006_0006);
        driver.object.vehicle_definition = Some(0xe100_0030);
        driver.object.up = [0.0, 0.6, 0.8];
        let mut nameless = actor(7, [30.0, 0.0, 0.0], 12);
        nameless.object.definition = Some(0xe100_0099);
        state.tick = 2;
        state.shots = [(0x9009_0009, 1)].into();
        state.actors = vec![far, near, driver, nameless];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);

        let ids: Vec<&str> = frame.actors.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["90040004", "90050005", "90070007", "90030003"],
            "nearest first"
        );
        let near = &frame.actors[0];
        assert_eq!(near.species, "grunt");
        assert_eq!(near.team.as_deref(), Some("human"));
        assert_eq!(near.weapon.as_deref(), Some("plasma_pistol"));
        assert_eq!(near.shots, Some(3), "both ticks' shots");
        assert_eq!(near.aim, Some([1.0, 0.0, 0.0]));
        assert!((near.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(frame.actors[1].team.as_deref(), Some("covenant"));
        assert_eq!(frame.actors[1].vehicle.as_deref(), Some("90060006"));
        assert_eq!(frame.actors[2].species, "unknown");
        assert_eq!(frame.actors[2].team.as_deref(), Some("12"), "raw index");
        assert_eq!(
            frame.vehicles,
            [FrameVehicle {
                id: "90060006".into(),
                kind: "ghost".into(),
                pos: [20.0, 0.0, 1.0],
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.6, 0.8],
                occupied: true,
                parent: None,
                aim: None,
            }]
        );
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));

        // Shots were handed out; the next frame has none.
        state.tick = 4;
        state.shots.clear();
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].actors[0].shots, None);
    }

    #[test]
    fn actor_count_is_capped_and_vehicles_are_listed_once() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![player(0, 0)]);
        state.actors = (0..MAX_FRAME_ACTORS as u32 + 20)
            .map(|i| {
                let mut a = actor(i + 1, [i as f64, 0.0, 0.0], 3);
                // Every actor rides the same vehicle; one has a bad up.
                a.object.vehicle = Some(0x9fff_0fff);
                if i == 0 {
                    a.object.up = [f64::NAN, 0.0, 0.0];
                }
                a
            })
            .collect();
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.actors.len(), MAX_FRAME_ACTORS);
        assert_eq!(frame.vehicles.len(), 1);
        assert_eq!(frame.vehicles[0].kind, "unknown", "no names set");
        assert_eq!(frame.vehicles[0].up, [0.0, 0.0, 1.0]);
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));
    }

    #[test]
    fn multiplayer_teams_stay_numbers() {
        assert_eq!(team_name(3, false), "3");
        assert_eq!(team_name(3, true), "covenant");
        assert_eq!(team_name(1, true), "player");
    }

    #[test]
    fn unchanged_initial_teams_do_not_create_a_score_event() {
        let mut session = Session::new("test", None);
        let teams = vec![live_proto::game::TeamScore { team: 0, score: 0 }];
        session.start_with_teams(&info(), vec![player(0, 0)], teams.clone());
        session.take_output();
        let mut state = snapshot(30, vec![player(0, 0)]);
        state.teams = teams;
        session.sample(&state);
        assert!(kinds(&mut session).is_empty());
    }

    #[test]
    fn team_winner_and_ties() {
        let mut state = snapshot(1, vec![player(0, 5), player(1, 2)]);
        state.teams = vec![
            live_proto::game::TeamScore { team: 0, score: 7 },
            live_proto::game::TeamScore { team: 1, score: 2 },
        ];
        assert_eq!(
            state.winner(true),
            Some(live_proto::game::Winner::Team { team: 0 })
        );
        state.teams[1].score = 7;
        assert_eq!(state.winner(true), None);
    }

    #[test]
    fn an_unchanged_roster_is_the_same_once_linked() {
        let xuid = live_proto::Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let fresh = vec![player(0, 1), player(1, 2)];
        let mut current = fresh.clone();
        link(Some(xuid), &mut current);
        assert!(same_roster(Some(xuid), &current, &fresh));
        assert!(
            !same_roster(None, &current, &fresh),
            "the link is part of it"
        );
        let mut renamed = fresh.clone();
        renamed[1].player.name = "Q".into();
        assert!(!same_roster(Some(xuid), &current, &renamed));
        let mut scored = fresh.clone();
        scored[0].stats.kills = 1;
        assert!(!same_roster(Some(xuid), &current, &scored));
        assert!(!same_roster(Some(xuid), &current, &fresh[..1]));
    }

    #[test]
    fn a_stats_change_reaches_the_roster_the_end_reports() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();
        session.sample(&snapshot(2, vec![player(0, 5)]));
        assert_eq!(session.current.as_ref().unwrap().players[0].stats.score, 5);
    }

    #[test]
    fn the_signed_in_account_is_the_player_on_local_slot_zero() {
        let xuid = live_proto::Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let mut session = Session::new("test", None).with_signed_in(Some(xuid));
        // Slot 0 is the signed-in local player; 1 is remote; 2 is a second
        // local player on another controller (not the signed-in account).
        let mut guest = player(2, 0);
        guest.local_user = Some(1);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.data["players"][0]["xuid"], "0009a1b2c3d4e5f6");
        assert_eq!(
            started.data["players"][0]["name"], "P0",
            "the in-game name stays"
        );
        assert!(started.data["players"][1].get("xuid").is_none());

        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), guest]));
        let outputs = session.take_output();
        let joined: Vec<_> = outputs
            .iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(
            joined.len(),
            1,
            "linking does not make the roster look changed"
        );
        assert!(joined[0].data["player"].get("xuid").is_none());

        // A player who rejoins on slot 0 (a new datum) is linked again.
        let mut back = player(0, 0);
        back.datum ^= 0x0001_0000;
        session.sample(&snapshot(4, vec![back]));
        let rejoin = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .unwrap();
        assert_eq!(rejoin.data["player"]["xuid"], "0009a1b2c3d4e5f6");
    }

    #[test]
    fn without_a_signed_in_account_players_have_no_xuid() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert!(started.data["players"][0].get("xuid").is_none());
    }

    fn machine(slot: u32, pos: [f64; 3], position: Option<f64>) -> MachineState {
        MachineState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            definition: Some(0xe100_0030),
            placement: Some(slot as u16),
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            position,
        }
    }

    fn world_names() -> TagNames {
        TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0030,
                    *b"mach",
                    "scenarios\\objects\\covenant\\military\\scarab\\scarab".to_string(),
                ),
                (
                    0xe100_0040,
                    *b"jpt!",
                    "objects\\weapons\\melee\\big_punch".to_string(),
                ),
                (
                    0xe100_0050,
                    *b"vehi",
                    "objects\\vehicles\\warthog\\warthog".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn frames_carry_bsp_hits_vehicles_and_moved_machines() {
        use crate::event::{HitRecord, VehicleState};
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(world_names());
        session.take_output();
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        });
        state.bsp = Some(1);
        let vehicle = |slot: u32, occupied| VehicleState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            definition: Some(0xe100_0050),
            pos: [slot as f64, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            occupied,
            parent: None,
            aim: None,
        };
        // Vehicle 9 is vehicle 6's turret, aiming up and left.
        let mut turret = vehicle(9, true);
        turret.parent = Some(0x9006_0006);
        turret.aim = Some([0.0, 0.6, 0.8]);
        state.vehicles = vec![vehicle(5, false), vehicle(6, true), turret];
        state.machines = vec![
            machine(7, [1.0, 1.0, 1.0], Some(0.0)),
            machine(8, [2.0, 2.0, 2.0], None),
        ];
        state.hits = vec![
            // Player 0 shoots an AI with a battle rifle.
            HitRecord {
                code: 10,
                effect: 0xe100_0099,
                owner_player: 0x8001_0000,
                owner_object: 0x9001_0001,
                victim: 0x9003_0003,
                pos: Some([3.0, 0.0, 0.0]),
                body: 0.25,
                shield: 0.5,
            },
            // An AI punches player 0: a melee for the AI and a weapon from
            // the damage effect's name.
            HitRecord {
                code: 3,
                effect: 0xe100_0040,
                owner_player: u32::MAX,
                owner_object: 0x9003_0003,
                victim: 0x9001_0001,
                pos: Some([0.0, 0.0, 0.0]),
                body: 0.1,
                shield: 0.0,
            },
            // No position: not sent.
            HitRecord {
                code: 1,
                effect: 0,
                owner_player: u32::MAX,
                owner_object: u32::MAX,
                victim: 0x900a_000a,
                pos: None,
                body: 0.1,
                shield: 0.0,
            },
        ];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));
        assert_eq!(frame.bsp, Some(1));
        assert_eq!(frame.hits.len(), 2);
        let shot = &frame.hits[0];
        assert_eq!(shot.from.as_deref(), Some("p0"));
        assert_eq!(shot.to.as_deref(), Some("90030003"));
        assert_eq!(shot.weapon.as_deref(), Some("battle_rifle"));
        assert_eq!(shot.kind.as_deref(), Some("bullet"));
        assert_eq!(shot.amount, Some(0.75));
        let punch = &frame.hits[1];
        assert_eq!(punch.from.as_deref(), Some("90030003"));
        assert_eq!(punch.to.as_deref(), Some("p0"));
        assert_eq!(punch.weapon.as_deref(), Some("big_punch"));
        assert_eq!(punch.kind.as_deref(), Some("melee"));
        let vehicles: Vec<_> = frame
            .vehicles
            .iter()
            .map(|v| (v.id.as_str(), v.kind.as_str(), v.occupied))
            .collect();
        assert_eq!(
            vehicles,
            [
                ("90060006", "warthog", true),
                ("90090009", "warthog", true),
                ("90050005", "warthog", false)
            ],
            "occupied first"
        );
        assert_eq!(frame.vehicles[0].parent, None);
        assert_eq!(frame.vehicles[0].aim, None);
        assert_eq!(frame.vehicles[1].parent.as_deref(), Some("90060006"));
        assert_eq!(frame.vehicles[1].aim, Some([0.0, 0.6, 0.8]));
        let machines: Vec<_> = frame
            .machines
            .iter()
            .map(|m| (m.node.as_deref(), m.kind.as_str(), m.position))
            .collect();
        assert_eq!(
            machines,
            [
                (Some("scarab_7"), "scarab", Some(0.0)),
                (Some("scarab_8"), "scarab", None)
            ],
            "all of them in the first frame"
        );

        // Next frame: only the machine that moved, no old hits.
        state.tick = 4;
        state.hits.clear();
        state.machines = vec![
            machine(7, [1.0, 1.0, 1.0], Some(0.5)),
            machine(8, [2.0, 2.0, 2.0], None),
        ];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert!(frame.hits.is_empty());
        assert_eq!(frame.machines.len(), 1);
        assert_eq!(frame.machines[0].position, Some(0.5));
        state.tick = 6;
        session.sample(&state);
        assert!(frames(&mut session)[0].machines.is_empty(), "nothing moved");
    }

    #[test]
    fn frames_carry_stance_speed_actions_and_grenade() {
        use crate::event::{Motion, Weapon};
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let unit = |pos: [f64; 3], grenades: u32, crouch: f64| ObjectState {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            weapon: Some(Weapon {
                datum: 0x9002_0002,
                definition: 1,
            }),
            motion: Motion {
                crouch: Some(crouch),
                physics_mode: Some(layout::BIPED_MODE_GROUND),
                grenade: Some(1),
                grenades: Some(grenades),
                weapons: Some([0x9002_0002, u32::MAX, u32::MAX, u32::MAX]),
                ..Motion::default()
            },
            ..ObjectState::default()
        };
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(unit([0.0; 3], 2, 0.0));
        session.sample(&state);
        let first = &frames(&mut session)[0].players[0];
        assert_eq!(first.stance.as_deref(), Some("stand"));
        assert_eq!(first.speed, None, "no previous frame");
        assert_eq!(first.actions, None);
        assert_eq!(first.grenade.as_deref(), Some("plasma"));

        state.tick = 4;
        state.objects[0] = Some(unit([0.4, 0.0, 0.0], 1, 1.0));
        session.sample(&state);
        let second = &frames(&mut session)[0].players[0];
        assert_eq!(second.stance.as_deref(), Some("crouch"));
        assert_eq!(second.speed, Some(6.0));
        assert_eq!(second.actions, Some(vec!["grenade".to_string()]));
    }

    fn pose(pos: [f64; 3]) -> Option<crate::event::Pose> {
        Some(crate::event::Pose {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        })
    }

    fn placed(kind: ObjectKind, index: u16, pos: [f64; 3], parented: bool) -> PlacedObject {
        PlacedObject {
            kind,
            index,
            parented,
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// An invented scenario: four crates, three weapons, two vehicles and
    /// two netgame spawns.
    fn table() -> Placements {
        Placements {
            kinds: vec![
                (
                    ObjectKind::Vehicle,
                    vec![pose([0.0; 3]), pose([5.0, 0.0, 0.0])],
                ),
                (
                    ObjectKind::Weapon,
                    vec![
                        pose([1.0, 0.0, 0.0]),
                        pose([2.0, 0.0, 0.0]),
                        pose([3.0, 0.0, 0.0]),
                    ],
                ),
                (
                    ObjectKind::Crate,
                    vec![pose([0.0; 3]), pose([1.0; 3]), pose([2.0; 3]), None],
                ),
                (
                    ObjectKind::NetgameEquipment,
                    vec![pose([7.0; 3]), pose([8.0; 3])],
                ),
            ],
        }
    }

    fn objects(placed: &[PlacedObject]) -> Vec<(ObjectKind, u32, bool)> {
        frame_objects(&table(), placed, &mut Vec::new())
            .into_iter()
            .map(|o| (o.k, o.i, o.gone))
            .collect()
    }

    #[test]
    fn objects_where_the_scenario_put_them_are_not_sent() {
        let all = [
            placed(ObjectKind::Vehicle, 0, [0.0; 3], false),
            placed(ObjectKind::Vehicle, 1, [5.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 0, [1.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 1, [2.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 2, [3.0, 0.0, 0.0], false),
            // Jitter under the threshold.
            placed(ObjectKind::Crate, 0, [0.01, 0.0, 0.0], false),
            placed(ObjectKind::Crate, 1, [1.0; 3], false),
            placed(ObjectKind::Crate, 2, [2.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 0, [7.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 1, [8.0; 3], false),
        ];
        // Crate 3's placement did not read: present means moved.
        assert_eq!(objects(&all), [(ObjectKind::Crate, 3, true)]);
    }

    #[test]
    fn moved_turned_gone_and_held_objects_are_sent_in_order() {
        let mut turned = placed(ObjectKind::Crate, 1, [1.0; 3], false);
        // Three degrees about z.
        let (s, c) = 3f64.to_radians().sin_cos();
        turned.forward = [c, s, 0.0];
        let mut tipped = placed(ObjectKind::Crate, 2, [2.0; 3], false);
        tipped.up = [0.0, 0.1, 1.0];
        let world = [
            // Vehicle 0 driven off; vehicle 1 has a parent (ignored, as
            // a parented vehicle's transform is not world space).
            placed(ObjectKind::Vehicle, 0, [0.0, 3.0, 0.0], false),
            placed(ObjectKind::Vehicle, 1, [0.0; 3], true),
            // Weapon 0 held by a player: gone. Weapon 1 dropped elsewhere
            // and another copy held: the dropped one wins. Weapon 2 absent.
            placed(ObjectKind::Weapon, 0, [9.0; 3], true),
            placed(ObjectKind::Weapon, 1, [0.0; 3], true),
            placed(ObjectKind::Weapon, 1, [2.0, 1.0, 0.0], false),
            placed(ObjectKind::Crate, 0, [0.0; 3], false),
            turned,
            tipped,
            placed(ObjectKind::Crate, 3, [4.0; 3], false),
            // Picked up from spawn 1; spawn 0 respawned in place. An index
            // past the block is ignored.
            placed(ObjectKind::NetgameEquipment, 0, [7.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 9, [7.0; 3], false),
        ];
        assert_eq!(
            objects(&world),
            [
                (ObjectKind::Vehicle, 0, false),
                (ObjectKind::Weapon, 0, true),
                (ObjectKind::Weapon, 1, false),
                (ObjectKind::Weapon, 2, true),
                (ObjectKind::Crate, 1, false),
                (ObjectKind::Crate, 2, false),
                (ObjectKind::Crate, 3, false),
                (ObjectKind::NetgameEquipment, 1, true),
            ]
        );
        let sent = frame_objects(&table(), &world, &mut Vec::new());
        assert_eq!(sent[2].p, Some([2.0, 1.0, 0.0]));
        assert_eq!(sent[0].f, Some([1.0, 0.0, 0.0]));
        let up = sent[5].u.unwrap();
        assert!(
            (up.iter().map(|c| c * c).sum::<f64>() - 1.0).abs() < 1e-9,
            "sent normalised"
        );
        assert!(sent[1].p.is_none() && sent[1].f.is_none() && sent[1].u.is_none());
        // The frame validates.
        let mut frame = Frame {
            objects: sent,
            ..Frame::default()
        };
        frame.tick = 2;
        live_proto::game::validate_frame(&frame).unwrap();
    }

    #[test]
    fn no_placements_no_objects() {
        let world = [placed(ObjectKind::Crate, 0, [5.0; 3], false)];
        assert!(frame_objects(&Placements::default(), &world, &mut Vec::new()).is_empty());
    }

    #[test]
    fn frames_carry_objects_once_placements_are_set() {
        let mut session = Session::new("test", None);
        let p = player(0, 0);
        session.start(&campaign(), vec![p.clone()]);
        session.set_placements(table());
        session.take_output();
        let mut state = snapshot(2, vec![p]);
        state.placed = vec![placed(ObjectKind::Crate, 0, [0.0, 0.0, 1.0], false)];
        session.sample(&state);
        let frame = frames(&mut session).pop().unwrap();
        let sent: Vec<_> = frame.objects.iter().map(|o| (o.k, o.i, o.gone)).collect();
        assert!(sent.contains(&(ObjectKind::Crate, 0, false)));
        assert!(sent.contains(&(ObjectKind::Weapon, 0, true)));
        assert_eq!(frame.objects[0].k, ObjectKind::Vehicle);
    }

    /// Game-thread cost of a frame for a ~60-actor campaign scene: the
    /// object table scan, the frame build, and its JSON or packed form.
    /// `cargo test --release -p live-telemetry measure_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_frame_cost() {
        use crate::memory::{fixture, world};
        use std::time::Instant;
        let mut guest = fixture::memory();
        let mut slot = 1;
        for i in 0..60 {
            fixture::put_actor(&mut guest, slot, 2, [i as f32, 1.0, 2.0]);
            slot += 1;
        }
        for _ in 0..8 {
            fixture::put_vehicle(&mut guest, slot);
            slot += 1;
        }
        for i in 0..12 {
            fixture::put_machine(&mut guest, slot, i, 0.5);
            slot += 1;
        }
        for i in 0..90 {
            let kind = [2u8, 6, 11][i % 3];
            fixture::put_placed(
                &mut guest,
                slot,
                kind,
                i as i16,
                -1,
                [i as f32, 0.0, 0.0],
                u32::MAX,
            );
            slot += 1;
        }
        let poses = |n: usize| (0..n).map(|i| pose([i as f64 * 3.0, 0.0, 0.0])).collect();
        let table = Placements {
            kinds: vec![
                (ObjectKind::Weapon, poses(40)),
                (ObjectKind::Scenery, poses(40)),
                (ObjectKind::Crate, poses(40)),
            ],
        };
        let mut session = Session::new("test", None);
        let p = player(0, 0);
        session.start(&campaign(), vec![p.clone()]);
        session.set_placements(table);
        session.take_output();
        let runs = 2000u32;
        let (mut scan, mut build, mut json, mut packed) = (0.0, 0.0, 0.0, 0.0);
        let mut encoder = live_proto::pack::Encoder::new(0, 150);
        let mut bytes = (0, 0);
        for n in 0..runs {
            let start = Instant::now();
            let found = world(&guest).unwrap();
            scan += start.elapsed().as_secs_f64();
            let mut state = snapshot(2 + 2 * u64::from(n), vec![p.clone()]);
            state.actors = found.actors;
            state.vehicles = found.vehicles;
            state.machines = found.machines;
            state.placed = found.placed;
            let start = Instant::now();
            session.sample(&state);
            let frame = frames(&mut session).pop().unwrap();
            build += start.elapsed().as_secs_f64();
            let start = Instant::now();
            let text = serde_json::to_string(&frame).unwrap();
            json += start.elapsed().as_secs_f64();
            let start = Instant::now();
            let (record, _) = encoder.encode(&frame).unwrap();
            packed += start.elapsed().as_secs_f64();
            bytes = (text.len(), record.len());
        }
        let us = |t: f64| t * 1e6 / f64::from(runs);
        println!(
            "scan {:.1} us, build {:.1} us, json {:.1} us ({} B), packed {:.1} us ({} B)",
            us(scan),
            us(build),
            us(json),
            bytes.0,
            us(packed),
            bytes.1
        );
    }
}
