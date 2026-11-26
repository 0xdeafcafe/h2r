//! Scenarios shared by the director's tests: invented definitions, a fake
//! world that applies commands a tick later as the host runtime does, and
//! seeded random matches.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use firefight::director::{Kill, PlayerSeen, SQUAD_SLOTS, ZoneSeen};
use firefight::{
    Caps, Command, Count, Def, Difficulty, DifficultyMultipliers, Drop, FORMAT, Lives, Observed,
    Outcome, PlayerKill, Round, Scoring, SetRules, ScriptDirector, Skull, SquadSpec, Wave, builtin,
};

/// One round of five one-actor waves, two ticks apart, with Famine on.
pub fn small_def(pool: u16, shared: bool) -> Def {
    let waves = (0..5)
        .map(|wave| Wave {
            squads: vec![SquadSpec {
                class: "elite".into(),
                count: Count {
                    per_player: 1,
                    min: 1,
                    max: 1,
                },
                drop: Drop::Edge,
                weapon_tier: 0,
            }],
            boss: wave == 4,
            gap_ticks: 2,
        })
        .collect();
    Def {
        format: FORMAT,
        name: "Golden".into(),
        teams: 1,
        rounds: vec![Round {
            waves,
            skulls_on: vec![Skull::Famine],
        }],
        set: SetRules::finite(),
        lives: Lives {
            pool_start: pool,
            per_round_bonus: 0,
            shared,
            zero_holds: false,
        },
        caps: Caps {
            live_actors_by_players: vec![(4, 4)],
        },
        scoring: Scoring {
            by_class: [("elite".into(), 100)].into(),
            player_kill: 0,
            difficulty_multiplier: DifficultyMultipliers {
                easy: 1_000,
                normal: 1_000,
                heroic: 1_000,
                legendary: 1_500,
            },
            skull_multipliers: [(Skull::Famine, 1_100)].into(),
            assist: 250,
        },
        bonus: None,
    }
}

/// [`small_def`] for two teams: a player kill scores 40, and the caps
/// cover eight players.
pub fn pair_def(pool: u16, shared: bool) -> Def {
    let mut def = small_def(pool, shared);
    def.name = "Golden Pair".into();
    def.teams = 2;
    def.scoring.player_kill = 40;
    def.caps.live_actors_by_players = vec![(8, 8)];
    def
}

pub fn settings(seed: u64) -> firefight::Settings {
    firefight::Settings {
        seed,
        difficulty: Difficulty::Legendary,
        skulls: BTreeSet::new(),
    }
}

/// A fake game: applies commands at the start of the next tick, as the host
/// runtime does, and reports what it holds.
pub struct World {
    pub players: BTreeMap<u8, bool>,
    /// Each player's game team: even slots 0, odd slots 1.
    pub teams: BTreeMap<u8, u8>,
    pub player_kills: Vec<PlayerKill>,
    /// Each placed squad's target, from `Target` (0 without one).
    pub targets: [u8; SQUAD_SLOTS],
    pub squads: [(u16, u32); SQUAD_SLOTS],
    pub classes: [String; SQUAD_SLOTS],
    pub zones: Vec<ZoneSeen>,
    pub kills: Vec<Kill>,
    pub pending: Vec<Command>,
    pub finished: Option<Outcome>,
    /// Live actors right after the last apply, before any kill.
    pub applied_live: u16,
}

impl World {
    pub fn new(players: u8, zones: Vec<ZoneSeen>) -> Self {
        Self {
            players: (0..players).map(|slot| (slot, true)).collect(),
            teams: (0..players).map(|slot| (slot, slot % 2)).collect(),
            player_kills: Vec::new(),
            targets: [0; SQUAD_SLOTS],
            squads: [(0, 0); SQUAD_SLOTS],
            classes: Default::default(),
            zones,
            kills: Vec::new(),
            pending: Vec::new(),
            finished: None,
            applied_live: 0,
        }
    }

    pub fn apply(&mut self, tick: u32) {
        for command in std::mem::take(&mut self.pending) {
            match command {
                Command::PlaceSquad {
                    slot, class, count, ..
                } => {
                    let slot = usize::from(slot);
                    assert_eq!(self.squads[slot].0, 0, "placed into an occupied slot");
                    self.squads[slot] = (count, tick);
                    self.classes[slot] = class;
                    self.targets[slot] = 0;
                }
                Command::Target { slot, team } => self.targets[usize::from(slot)] = team,
                Command::EraseSquad { slot } => self.squads[usize::from(slot)].0 = 0,
                Command::Respawn { player } => {
                    self.players.insert(player, true);
                }
                Command::Finish { outcome } => self.finished = Some(outcome),
                _ => {}
            }
        }
        self.applied_live = self.live();
    }

    pub fn kill_squad(&mut self, slot: usize, killer: Option<u8>) {
        for _ in 0..self.squads[slot].0 {
            self.kill_one(slot, killer);
        }
    }

    pub fn kill_one(&mut self, slot: usize, killer: Option<u8>) {
        self.squads[slot].0 -= 1;
        self.kills.push(Kill {
            class: self.classes[slot].clone(),
            killer,
            assist: None,
        });
    }

    /// `victim` dies to `killer` (none: a fall).
    pub fn kill_player(&mut self, victim: u8, killer: Option<u8>) {
        self.players.insert(victim, false);
        self.player_kills.push(PlayerKill { killer, victim });
    }

    pub fn live(&self) -> u16 {
        self.squads.iter().map(|&(alive, _)| alive).sum()
    }

    pub fn observe(&mut self) -> Observed {
        Observed {
            players: self
                .players
                .iter()
                .map(|(&slot, &alive)| PlayerSeen {
                    slot,
                    alive,
                    team: self.teams.get(&slot).copied().unwrap_or(0),
                })
                .collect(),
            actors_alive_by_squad: self.squads.map(|(alive, _)| alive),
            zones: self.zones.clone(),
            kills: std::mem::take(&mut self.kills),
            player_kills: std::mem::take(&mut self.player_kills),
        }
    }
}

/// Every squad dies to player 0 three ticks after it appears.
pub fn kill_at_age_three(tick: u32, world: &mut World) {
    for slot in 0..SQUAD_SLOTS {
        if world.squads[slot].0 > 0 && tick - world.squads[slot].1 == 3 {
            world.kill_squad(slot, Some(0));
        }
    }
}

/// A zone the same distance from both teams.
pub fn zone(seen: bool, distance: u32) -> ZoneSeen {
    ZoneSeen {
        seen,
        distance,
        team_distance: [distance; 2],
        roles: firefight::ALL_ZONE_ROLES,
    }
}

pub fn two_zones() -> Vec<ZoneSeen> {
    vec![zone(false, 10), zone(true, 50)]
}

pub struct Run {
    pub commands: Vec<(u32, Command)>,
    pub observed: Vec<Observed>,
    pub outcome: Option<Outcome>,
}

pub fn run(
    director: &mut ScriptDirector,
    world: &mut World,
    limit: u32,
    mut script: impl FnMut(u32, &mut World),
    mut check: impl FnMut(u32, &ScriptDirector, &World, &Observed, &[Command]),
) -> Run {
    let mut commands = Vec::new();
    let mut observed = Vec::new();
    for tick in 0..limit {
        world.apply(tick);
        if world.finished.is_some() {
            break;
        }
        script(tick, world);
        let seen = world.observe();
        let out = director.step(tick, &seen);
        check(tick, director, world, &seen, &out);
        commands.extend(out.iter().cloned().map(|command| (tick, command)));
        world.pending = out;
        observed.push(seen);
    }
    Run {
        commands,
        observed,
        outcome: world.finished,
    }
}

/// A small test generator, separate from the director's.
pub struct TestRng(pub u64);

impl TestRng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn one_in(&mut self, n: u64) -> bool {
        self.next() % n == 0
    }
}

/// The built-in sets with short gaps, so a match takes a few thousand ticks.
pub fn quick_builtins() -> [Def; 5] {
    builtin::definitions().map(|mut def| {
        for wave in def.rounds.iter_mut().flat_map(|round| &mut round.waves) {
            wave.gap_ticks = 10;
        }
        def
    })
}

/// Random kills, deaths and sight lines from a seed. Every fourth seed is
/// lethal (players die far faster than the lives pools refill), so some
/// matches are lost; the rest almost never drain the pool. Players also
/// kill each other now and then, across teams and within one, and each
/// zone's distance from each team varies.
pub fn random_script(seed: u64) -> impl FnMut(u32, &mut World) {
    let mut rng = TestRng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let death_one_in = if seed % 4 == 0 { 15 } else { 300 };
    move |_, world| {
        let alive: Vec<u8> = world
            .players
            .iter()
            .filter(|&(_, &alive)| alive)
            .map(|(&slot, _)| slot)
            .collect();
        for slot in 0..SQUAD_SLOTS {
            for _ in 0..world.squads[slot].0 {
                if rng.one_in(20) {
                    let killer = (!alive.is_empty())
                        .then(|| alive[(rng.next() % alive.len() as u64) as usize]);
                    world.kill_one(slot, killer);
                }
            }
        }
        for alive in world.players.values_mut() {
            if *alive && rng.one_in(death_one_in) {
                *alive = false;
            }
        }
        if alive.len() >= 2 && rng.one_in(death_one_in * 2) {
            let victim = alive[(rng.next() % alive.len() as u64) as usize];
            let killer = alive[(rng.next() % alive.len() as u64) as usize];
            if world.players[&victim] {
                world.kill_player(victim, Some(killer));
            }
        }
        for zone in &mut world.zones {
            zone.seen = rng.one_in(2);
            zone.distance = (rng.next() % 100) as u32;
            zone.team_distance = [(rng.next() % 100) as u32, (rng.next() % 100) as u32];
        }
    }
}

pub fn eight_zones() -> Vec<ZoneSeen> {
    vec![zone(false, 0); 8]
}

pub fn random_match(
    index: u64,
    check: impl FnMut(u32, &ScriptDirector, &World, &Observed, &[Command]),
) -> (Def, Run) {
    let def = quick_builtins()[(index % 4) as usize].clone();
    let players = (index % 16) as u8 + 1;
    let mut director = ScriptDirector::new(def.clone(), settings(index)).unwrap();
    let mut world = World::new(players, eight_zones());
    let run = run(
        &mut director,
        &mut world,
        50_000,
        random_script(index),
        check,
    );
    (def, run)
}

/// Waves 0 to 2 prefer zones 2 (unseen), 1 (in sight) and 9 (absent).
pub fn zone_choice_def() -> Def {
    let mut def = small_def(1, true);
    def.rounds[0].waves[0].squads[0].drop = Drop::PreferZone(2);
    def.rounds[0].waves[1].squads[0].drop = Drop::PreferZone(1);
    def.rounds[0].waves[2].squads[0].drop = Drop::PreferZone(9);
    def
}

/// Zone 1 is in sight; zones 3 and 4 are the farthest unseen.
pub fn six_zones() -> Vec<ZoneSeen> {
    [
        (false, 5),
        (true, 90),
        (false, 1),
        (false, 70),
        (false, 60),
        (false, 2),
    ]
    .map(|(seen, distance)| zone(seen, distance))
    .to_vec()
}
