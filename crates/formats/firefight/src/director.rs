//! The Firefight director: a deterministic state machine over a [`Def`].
//!
//! The host feeds it what it observed each tick and appends the commands it
//! returns to the host log. It reads no clock, touches no game memory and
//! draws only from its own generator, so the same definition, settings and
//! observed sequence always give the same commands.
//! See `docs/game/firefight.md`, "The director".

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::scoring::{self, ScoreReason};
use crate::{Def, Difficulty, Drop, MAX_TEAMS, Result, Skull, WAVES_PER_ROUND};

/// Squad slots the kit provides on every map.
pub const SQUAD_SLOTS: usize = 16;
/// Squads placed in one step at most.
pub const PLACE_PER_TICK: usize = 2;
/// Live actors at or under which a wave enters cleanup.
pub const CLEANUP_ACTORS: u16 = 3;
/// Ticks of cleanup before the director erases what is left (60 seconds).
pub const CLEANUP_TICKS: u32 = 60 * 30;
/// Ticks a placed squad may go unobserved, or an erased one may stay
/// observed, before the director treats it as gone (3 seconds).
pub const SLOT_TIMEOUT_TICKS: u32 = 3 * 30;

/// Match options chosen in Game Setup or by the hopper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The match seed from the session data.
    pub seed: u64,
    pub difficulty: Difficulty,
    /// Skulls on from the start, beside the definition's round schedule.
    pub skulls: BTreeSet<Skull>,
}

/// What the host saw this tick, after the game tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// The players in the match by roster slot. Order does not matter.
    pub players: Vec<PlayerSeen>,
    /// Live actors in each kit squad slot.
    pub actors_alive_by_squad: [u16; SQUAD_SLOTS],
    /// The kit's drop zones in kit order.
    pub zones: Vec<ZoneSeen>,
    /// Actors killed this tick (`SimEvent::ActorKilled`), in event order.
    pub kills: Vec<Kill>,
    /// Players killed by players this tick, in event order. Only a kill
    /// across the two teams scores ([`crate::Scoring::player_kill`]).
    pub player_kills: Vec<PlayerKill>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerSeen {
    pub slot: u8,
    pub alive: bool,
    /// The player's game team. With two Firefight teams, team 0 is the
    /// first and every other team the second; with one, it is ignored.
    pub team: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneSeen {
    /// Some player has line of sight to the zone (the AI's visibility test).
    pub seen: bool,
    /// Distance from the players' centre in any fixed integer unit.
    pub distance: u32,
    /// Distance from each Firefight team's players' centre, in the same
    /// unit; `u32::MAX` for a team with no unit. Read with two teams only.
    pub team_distance: [u32; MAX_TEAMS],
}

/// A player killed by a player (`SimEvent` kill of a player's unit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerKill {
    /// The credited player's roster slot; none for a fall or the AI.
    pub killer: Option<u8>,
    pub victim: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kill {
    /// The kit class of the killed actor's character.
    pub class: String,
    /// The credited player's roster slot.
    pub killer: Option<u8>,
    /// A second player who damaged the actor in its last five seconds.
    pub assist: Option<u8>,
}

/// A director command; the host log carries each as `Entry<Command>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    PlaceSquad {
        slot: u8,
        class: String,
        count: u16,
        zone: u8,
        weapon_tier: u8,
    },
    /// With two teams, right after a `PlaceSquad`: the Firefight team the
    /// squad is sent at. Its cleanup `Hunt` goes to that team's base. With
    /// one team there is no `Target` and every squad's target is team 0.
    Target {
        slot: u8,
        team: u8,
    },
    /// Send the squad at the players (cleanup): at its target's base.
    Hunt {
        slot: u8,
    },
    EraseSquad {
        slot: u8,
    },
    /// The zero-based round starts.
    Round {
        index: u8,
    },
    /// The zero-based wave of the current round starts.
    Wave {
        index: u8,
    },
    Skull {
        id: Skull,
        on: bool,
    },
    /// The shared pool (one team), or one player's count when lives are
    /// not shared.
    Lives {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<u8>,
        pool: u16,
    },
    /// A team's shared pool, with two teams.
    TeamLives {
        team: u8,
        pool: u16,
    },
    Score {
        player: u8,
        points: u32,
        reason: ScoreReason,
    },
    Respawn {
        player: u8,
    },
    Finish {
        outcome: Outcome,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The last round's boss wave was cleared (one team).
    Won,
    /// Every player was dead with no lives left (one team, or the only
    /// team present of two).
    Lost,
    /// Two teams: the other team ran out of lives, or this team led when
    /// the last round was cleared.
    TeamWon { team: u8 },
    /// Two teams: level on score and on lives left at the end.
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before the first wave, until the tick given.
    Warmup {
        until: u32,
    },
    /// Squads of the wave are still in the reserve.
    Spawning,
    /// Every squad is placed and enough actors are alive.
    Fighting,
    /// The last few actors hunt the players; the rest is erased at the deadline.
    Cleanup {
        since: u32,
    },
    /// Between waves or rounds, until the tick given.
    Intermission {
        until: u32,
    },
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Free,
    /// A placed squad. `alive` is the count until the host first reports
    /// actors, then the lowest count reported.
    Placed {
        count: u16,
        alive: u16,
        seen: bool,
        placed_at: u32,
        /// The Firefight team the squad is sent at (0 with one team).
        target: u8,
    },
    /// Erased; free once the host reports it empty or the timeout passes.
    Draining {
        since: u32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    /// The player's own lives when lives are not shared.
    pub lives: u16,
    /// A respawn was issued and the player has not been seen alive since.
    pub respawning: bool,
    /// The player's Firefight team, as last seen (0 with one team).
    pub team: u8,
}

/// The director's whole state: the mode blob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub phase: Phase,
    pub round: u8,
    pub wave: u8,
    /// The next squad of the wave to place; the rest is the reserve.
    pub next_squad: u8,
    /// Actors placed so far in this wave.
    pub wave_actors: u16,
    pub slots: [Slot; SQUAD_SLOTS],
    /// Each team's shared pool (unused when lives are not shared). With
    /// one team the second stays 0.
    pub pools: [u16; MAX_TEAMS],
    /// Each team's points: the sum of its players' `Score` commands.
    pub scores: [u32; MAX_TEAMS],
    pub players: BTreeMap<u8, PlayerState>,
    pub skulls: BTreeSet<Skull>,
    pub rng: u64,
    pub started: bool,
}

pub struct Director {
    def: Def,
    difficulty: Difficulty,
    state: State,
}

impl Director {
    /// Start a match. The definition must be structurally valid.
    pub fn new(def: Def, settings: Settings) -> Result<Self> {
        let id = def.id()?;
        let mut prefix = [0; 8];
        prefix.copy_from_slice(&id.0[..8]);
        let state = State {
            phase: Phase::Warmup { until: 0 },
            round: 0,
            wave: 0,
            next_squad: 0,
            wave_actors: 0,
            slots: [Slot::Free; SQUAD_SLOTS],
            pools: if def.teams > 1 {
                [def.lives.pool_start; MAX_TEAMS]
            } else {
                [def.lives.pool_start, 0]
            },
            scores: [0; MAX_TEAMS],
            players: BTreeMap::new(),
            skulls: settings.skulls,
            rng: settings.seed ^ u64::from_le_bytes(prefix),
            started: false,
        };
        Ok(Self {
            def,
            difficulty: settings.difficulty,
            state,
        })
    }

    /// Resume from a mode blob, e.g. a film key or a host handover.
    pub fn resume(def: Def, difficulty: Difficulty, state: State) -> Result<Self> {
        def.id()?;
        Ok(Self {
            def,
            difficulty,
            state,
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn def(&self) -> &Def {
        &self.def
    }

    /// Actors the director counts as alive.
    pub fn live_actors(&self) -> u16 {
        self.state.slots.iter().map(slot_alive).sum()
    }

    /// Actors of the current wave not yet placed, at `players`.
    pub fn reserve(&self, players: u8) -> u16 {
        let players = self.supported_players(players);
        self.current_wave().map_or(0, |wave| {
            wave.squads[usize::from(self.state.next_squad).min(wave.squads.len())..]
                .iter()
                .map(|squad| squad.count.for_players(players))
                .sum()
        })
    }

    /// The live actor cap at a player count, clamped to the definition's tiers.
    pub fn cap(&self, players: u8) -> u16 {
        let players = self.supported_players(players);
        self.def.caps.for_players(players).unwrap_or(0)
    }

    /// Advance one tick. `tick` must not go backwards.
    pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command> {
        let mut out = Vec::new();
        if self.state.phase == Phase::Done {
            return out;
        }
        if !self.state.started {
            self.state.started = true;
            self.state.phase = Phase::Warmup {
                until: tick.saturating_add(self.gap(0, 0)),
            };
            if self.def.lives.shared {
                if self.two_teams() {
                    for team in 0..MAX_TEAMS {
                        out.push(Command::TeamLives {
                            team: team as u8,
                            pool: self.state.pools[team],
                        });
                    }
                } else {
                    out.push(Command::Lives {
                        player: None,
                        pool: self.state.pools[0],
                    });
                }
            }
        }

        let players = present(seen, self.def.teams);
        self.observe_players(&players, &mut out);
        self.score(seen, &mut out);
        self.score_players(seen, &mut out);
        self.observe_slots(tick, seen);
        self.respawn(&players, &mut out);
        if let Some(outcome) = self.out_of_lives(&players) {
            self.finish(outcome, &mut out);
            return out;
        }
        self.advance(tick, seen, &players, &mut out);
        out
    }

    fn two_teams(&self) -> bool {
        self.def.teams > 1
    }

    fn observe_players(&mut self, players: &BTreeMap<u8, Present>, out: &mut Vec<Command>) {
        for (&slot, seen) in players {
            let state = self.state.players.entry(slot).or_insert_with(|| {
                if !self.def.lives.shared {
                    out.push(Command::Lives {
                        player: Some(slot),
                        pool: self.def.lives.pool_start,
                    });
                }
                PlayerState {
                    lives: self.def.lives.pool_start,
                    respawning: false,
                    team: seen.team,
                }
            });
            state.team = seen.team;
            if seen.alive {
                state.respawning = false;
            }
        }
    }

    /// The Firefight team of a roster slot: its registered team, else 0.
    fn team_of(&self, slot: u8) -> usize {
        self.state
            .players
            .get(&slot)
            .map_or(0, |player| usize::from(player.team))
    }

    fn credit(&mut self, player: u8, points: u32, reason: ScoreReason, out: &mut Vec<Command>) {
        let team = self.team_of(player);
        self.state.scores[team] = self.state.scores[team].saturating_add(points);
        out.push(Command::Score {
            player,
            points,
            reason,
        });
    }

    fn score(&mut self, seen: &Observed, out: &mut Vec<Command>) {
        for kill in &seen.kills {
            let Some(killer) = kill.killer else {
                continue;
            };
            let Some(points) = scoring::kill_points(
                &self.def.scoring,
                &kill.class,
                self.difficulty,
                &self.state.skulls,
            ) else {
                continue;
            };
            self.credit(killer, points, ScoreReason::Kill, out);
            if let Some(assist) = kill.assist.filter(|&assist| assist != killer) {
                let points = scoring::assist_points(&self.def.scoring, points);
                if points > 0 {
                    self.credit(assist, points, ScoreReason::Assist, out);
                }
            }
        }
    }

    /// With two teams, a player who kills a player of the other team scores
    /// the definition's fixed `player_kill`. A suicide, a team kill or a
    /// kill nobody is credited with scores nothing.
    fn score_players(&mut self, seen: &Observed, out: &mut Vec<Command>) {
        let points = u32::from(self.def.scoring.player_kill);
        if !self.two_teams() || points == 0 {
            return;
        }
        for kill in &seen.player_kills {
            let Some(killer) = kill.killer else {
                continue;
            };
            if killer == kill.victim || self.team_of(killer) == self.team_of(kill.victim) {
                continue;
            }
            self.credit(killer, points, ScoreReason::PlayerKill, out);
        }
    }

    fn observe_slots(&mut self, tick: u32, seen: &Observed) {
        for (slot, &observed) in self.state.slots.iter_mut().zip(&seen.actors_alive_by_squad) {
            match slot {
                Slot::Free => {}
                Slot::Placed {
                    alive,
                    seen: was_seen,
                    placed_at,
                    ..
                } => {
                    let observed = observed.min(*alive);
                    if observed > 0 {
                        *was_seen = true;
                    }
                    if *was_seen {
                        *alive = observed;
                    } else if tick >= placed_at.saturating_add(SLOT_TIMEOUT_TICKS) {
                        *alive = 0;
                    }
                    if *alive == 0 {
                        *slot = Slot::Free;
                    }
                }
                Slot::Draining { since } => {
                    if observed == 0 || tick >= since.saturating_add(SLOT_TIMEOUT_TICKS) {
                        *slot = Slot::Free;
                    }
                }
            }
        }
    }

    /// Spend a life on each dead player not yet respawning, from the
    /// player's team pool or own count. With none left, the player waits
    /// for the wave to end; between waves respawns are free.
    fn respawn(&mut self, players: &BTreeMap<u8, Present>, out: &mut Vec<Command>) {
        let between_waves = matches!(
            self.state.phase,
            Phase::Warmup { .. } | Phase::Intermission { .. }
        );
        let shared = self.def.lives.shared;
        let two_teams = self.two_teams();
        for (&slot, seen) in players {
            let player = self.state.players.get_mut(&slot).expect("registered");
            if seen.alive || player.respawning {
                continue;
            }
            let team = usize::from(player.team);
            let lives = if shared {
                &mut self.state.pools[team]
            } else {
                &mut player.lives
            };
            if *lives > 0 {
                *lives -= 1;
                let pool = *lives;
                player.respawning = true;
                out.push(Command::Respawn { player: slot });
                out.push(if shared && two_teams {
                    Command::TeamLives {
                        team: team as u8,
                        pool,
                    }
                } else {
                    Command::Lives {
                        player: (!shared).then_some(slot),
                        pool,
                    }
                });
            } else if between_waves {
                player.respawning = true;
                out.push(Command::Respawn { player: slot });
            }
        }
    }

    /// A team is out when it has players present, all dead with no respawn
    /// coming. With one team, or one of two present, that team out is lost.
    /// With both present, one team out means the other wins, and both out
    /// in the same tick is decided as the last round is ([`Self::decide`]).
    fn out_of_lives(&self, players: &BTreeMap<u8, Present>) -> Option<Outcome> {
        let mut present = [false; MAX_TEAMS];
        let mut up = [false; MAX_TEAMS];
        for (slot, seen) in players {
            let team = usize::from(seen.team);
            present[team] = true;
            if seen.alive || self.state.players[slot].respawning {
                up[team] = true;
            }
        }
        let out = [0, 1].map(|team| present[team] && !up[team]);
        if !self.two_teams() || present != [true, true] {
            return out.contains(&true).then_some(Outcome::Lost);
        }
        match out {
            [true, true] => Some(self.decide()),
            [true, false] => Some(Outcome::TeamWon { team: 1 }),
            [false, true] => Some(Outcome::TeamWon { team: 0 }),
            [false, false] => None,
        }
    }

    /// Two teams at the end: the higher score wins; level scores go to the
    /// team with more lives left; level on both is a draw.
    fn decide(&self) -> Outcome {
        let key = |team: usize| (self.state.scores[team], self.lives_left(team));
        match key(0).cmp(&key(1)) {
            std::cmp::Ordering::Greater => Outcome::TeamWon { team: 0 },
            std::cmp::Ordering::Less => Outcome::TeamWon { team: 1 },
            std::cmp::Ordering::Equal => Outcome::Draw,
        }
    }

    /// A team's lives left: its pool, or its registered players' own counts.
    fn lives_left(&self, team: usize) -> u32 {
        if self.def.lives.shared {
            return u32::from(self.state.pools[team]);
        }
        self.state
            .players
            .values()
            .filter(|player| usize::from(player.team) == team)
            .map(|player| u32::from(player.lives))
            .sum()
    }

    fn advance(
        &mut self,
        tick: u32,
        seen: &Observed,
        players: &BTreeMap<u8, Present>,
        out: &mut Vec<Command>,
    ) {
        loop {
            match self.state.phase {
                Phase::Warmup { until } | Phase::Intermission { until } => {
                    if tick < until {
                        return;
                    }
                    self.start_wave(out);
                }
                Phase::Spawning => {
                    self.place(tick, seen, players, out);
                    let squads = self.current_wave().map_or(0, |wave| wave.squads.len());
                    if usize::from(self.state.next_squad) < squads {
                        return;
                    }
                    self.state.phase = Phase::Fighting;
                }
                Phase::Fighting => {
                    let live = self.live_actors();
                    if live == 0 {
                        self.end_wave(tick, players, out);
                        continue;
                    }
                    if live <= CLEANUP_ACTORS && live < self.state.wave_actors {
                        for (index, slot) in self.state.slots.iter().enumerate() {
                            if matches!(slot, Slot::Placed { .. }) {
                                out.push(Command::Hunt { slot: index as u8 });
                            }
                        }
                        self.state.phase = Phase::Cleanup { since: tick };
                    }
                    return;
                }
                Phase::Cleanup { since } => {
                    if self.live_actors() > 0 {
                        if tick < since.saturating_add(CLEANUP_TICKS) {
                            return;
                        }
                        for (index, slot) in self.state.slots.iter_mut().enumerate() {
                            if matches!(slot, Slot::Placed { .. }) {
                                out.push(Command::EraseSquad { slot: index as u8 });
                                *slot = Slot::Draining { since: tick };
                            }
                        }
                    }
                    self.end_wave(tick, players, out);
                }
                Phase::Done => return,
            }
        }
    }

    fn start_wave(&mut self, out: &mut Vec<Command>) {
        let round = usize::from(self.state.round);
        if self.state.wave == 0 {
            out.push(Command::Round {
                index: self.state.round,
            });
            for &skull in &self.def.rounds[round].skulls_on {
                if self.state.skulls.insert(skull) {
                    out.push(Command::Skull {
                        id: skull,
                        on: true,
                    });
                }
            }
        }
        out.push(Command::Wave {
            index: self.state.wave,
        });
        self.state.next_squad = 0;
        self.state.wave_actors = 0;
        self.state.phase = Phase::Spawning;
    }

    /// The team a new squad is sent at, with two teams: the only team
    /// present, else the one fewer live actors are sent at, else by the
    /// squad's place in the wave (even squads at team 0).
    fn choose_target(&self, players: &BTreeMap<u8, Present>) -> u8 {
        let mut present = [false; MAX_TEAMS];
        for seen in players.values() {
            present[usize::from(seen.team)] = true;
        }
        match present {
            [true, false] => return 0,
            [false, true] => return 1,
            _ => {}
        }
        let mut load = [0u32; MAX_TEAMS];
        for slot in &self.state.slots {
            if let Slot::Placed { alive, target, .. } = *slot {
                load[usize::from(target)] += u32::from(alive);
            }
        }
        match load[0].cmp(&load[1]) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Equal => self.state.next_squad % 2,
        }
    }

    /// Place squads from the reserve, in definition order, while the next
    /// one fits under the cap and a slot and a zone are free.
    fn place(
        &mut self,
        tick: u32,
        seen: &Observed,
        players: &BTreeMap<u8, Present>,
        out: &mut Vec<Command>,
    ) {
        let count_players = self.supported_players(u8::try_from(players.len()).unwrap_or(u8::MAX));
        let cap = self.def.caps.for_players(count_players).unwrap_or(0);
        let round = usize::from(self.state.round);
        let wave = usize::from(self.state.wave);
        let mut live = self.live_actors();
        for _ in 0..PLACE_PER_TICK {
            let squads = &self.def.rounds[round].waves[wave].squads;
            let Some(squad) = squads.get(usize::from(self.state.next_squad)) else {
                return;
            };
            let count = squad.count.for_players(count_players);
            if live.saturating_add(count) > cap {
                return;
            }
            let Some(slot) = self.state.slots.iter().position(|s| *s == Slot::Free) else {
                return;
            };
            let (drop, class, weapon_tier) = (squad.drop, squad.class.clone(), squad.weapon_tier);
            let target = self.two_teams().then(|| self.choose_target(players));
            let Some(zone) = choose_zone(drop, &seen.zones, target, &mut self.state.rng) else {
                return;
            };
            self.state.slots[slot] = Slot::Placed {
                count,
                alive: count,
                seen: false,
                placed_at: tick,
                target: target.unwrap_or(0),
            };
            out.push(Command::PlaceSquad {
                slot: slot as u8,
                class,
                count,
                zone,
                weapon_tier,
            });
            if let Some(team) = target {
                out.push(Command::Target {
                    slot: slot as u8,
                    team,
                });
            }
            self.state.next_squad += 1;
            self.state.wave_actors = self.state.wave_actors.saturating_add(count);
            live += count;
        }
    }

    fn end_wave(&mut self, tick: u32, players: &BTreeMap<u8, Present>, out: &mut Vec<Command>) {
        for (&slot, seen) in players {
            let player = self.state.players.get_mut(&slot).expect("registered");
            if !seen.alive && !player.respawning {
                player.respawning = true;
                out.push(Command::Respawn { player: slot });
            }
        }

        if usize::from(self.state.wave) + 1 < WAVES_PER_ROUND {
            self.state.wave += 1;
        } else if usize::from(self.state.round) + 1 < self.def.rounds.len() {
            self.state.round += 1;
            self.state.wave = 0;
            let bonus = self.def.lives.per_round_bonus;
            if self.def.lives.shared {
                if self.two_teams() {
                    for team in 0..MAX_TEAMS {
                        self.state.pools[team] = self.state.pools[team].saturating_add(bonus);
                        out.push(Command::TeamLives {
                            team: team as u8,
                            pool: self.state.pools[team],
                        });
                    }
                } else {
                    self.state.pools[0] = self.state.pools[0].saturating_add(bonus);
                    out.push(Command::Lives {
                        player: None,
                        pool: self.state.pools[0],
                    });
                }
            } else {
                for (&slot, player) in &mut self.state.players {
                    player.lives = player.lives.saturating_add(bonus);
                    out.push(Command::Lives {
                        player: Some(slot),
                        pool: player.lives,
                    });
                }
            }
        } else {
            let outcome = self.last_round_outcome(players);
            self.finish(outcome, out);
            return;
        }
        let gap = self.gap(usize::from(self.state.round), usize::from(self.state.wave));
        self.state.phase = Phase::Intermission {
            until: tick.saturating_add(gap),
        };
    }

    /// Clearing the last round: won with one team; with two, the only team
    /// present wins, else the result is decided by score and lives.
    fn last_round_outcome(&self, players: &BTreeMap<u8, Present>) -> Outcome {
        if !self.two_teams() {
            return Outcome::Won;
        }
        let mut present = [false; MAX_TEAMS];
        for seen in players.values() {
            present[usize::from(seen.team)] = true;
        }
        match present {
            [true, false] => Outcome::TeamWon { team: 0 },
            [false, true] => Outcome::TeamWon { team: 1 },
            _ => self.decide(),
        }
    }

    fn finish(&mut self, outcome: Outcome, out: &mut Vec<Command>) {
        out.push(Command::Finish { outcome });
        self.state.phase = Phase::Done;
    }

    fn gap(&self, round: usize, wave: usize) -> u32 {
        self.def.rounds[round].waves[wave].gap_ticks
    }

    fn current_wave(&self) -> Option<&crate::Wave> {
        self.def
            .rounds
            .get(usize::from(self.state.round))?
            .waves
            .get(usize::from(self.state.wave))
    }

    /// Clamp a player count to the definition's tiers (1 to the last bound).
    fn supported_players(&self, players: u8) -> u8 {
        let largest = self
            .def
            .caps
            .live_actors_by_players
            .last()
            .map_or(1, |&(upper, _)| upper);
        players.clamp(1, largest.max(1))
    }
}

fn slot_alive(slot: &Slot) -> u16 {
    match *slot {
        Slot::Placed { alive, .. } => alive,
        Slot::Free | Slot::Draining { .. } => 0,
    }
}

/// A present player: alive, and its Firefight team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Present {
    pub alive: bool,
    pub team: u8,
}

/// The Firefight team of a game team: 0 with one team; with two, game
/// team 0 is the first and every other team the second.
pub fn firefight_team(teams: u8, game_team: u8) -> u8 {
    u8::from(teams > 1 && game_team != 0)
}

/// Present players by slot; a slot listed twice counts as alive if either
/// entry is, and keeps its first entry's team.
pub(crate) fn present(seen: &Observed, teams: u8) -> BTreeMap<u8, Present> {
    let mut players = BTreeMap::new();
    for player in &seen.players {
        players
            .entry(player.slot)
            .or_insert(Present {
                alive: false,
                team: firefight_team(teams, player.team),
            })
            .alive |= player.alive;
    }
    players
}

/// An unseen preferred zone, else a seeded pick among the farther half of
/// the unseen zones (of all zones when every one is in sight). With a
/// target team, the candidates are first narrowed to the zones at least as
/// near that team as the other (when any is), and are ranked by distance
/// from the target team.
fn choose_zone(drop: Drop, zones: &[ZoneSeen], target: Option<u8>, rng: &mut u64) -> Option<u8> {
    let zones = &zones[..zones.len().min(usize::from(u8::MAX) + 1)];
    if let Drop::PreferZone(zone) = drop
        && zones.get(usize::from(zone)).is_some_and(|z| !z.seen)
    {
        return Some(zone);
    }
    let mut candidates: Vec<usize> = (0..zones.len()).filter(|&i| !zones[i].seen).collect();
    if candidates.is_empty() {
        candidates = (0..zones.len()).collect();
    }
    if candidates.is_empty() {
        return None;
    }
    let distance = |i: usize| match target {
        None => zones[i].distance,
        Some(team) => zones[i].team_distance[usize::from(team)],
    };
    if let Some(team) = target {
        let (ours, theirs) = (usize::from(team), 1 - usize::from(team));
        let near: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| zones[i].team_distance[ours] <= zones[i].team_distance[theirs])
            .collect();
        if !near.is_empty() {
            candidates = near;
        }
    }
    candidates.sort_by(|&a, &b| distance(b).cmp(&distance(a)).then(a.cmp(&b)));
    let farther = candidates.len().div_ceil(2);
    let pick = if farther > 1 {
        (next(rng) % farther as u64) as usize
    } else {
        0
    };
    Some(candidates[pick] as u8)
}

/// SplitMix64.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
