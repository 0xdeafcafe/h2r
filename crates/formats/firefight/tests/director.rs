use std::collections::{BTreeMap, BTreeSet};

use firefight::director::{Kill, PlayerSeen, SQUAD_SLOTS, State, ZoneSeen};
use firefight::{
    Caps, Command, Count, Def, Difficulty, DifficultyMultipliers, Director, Drop, FORMAT, Lives,
    Observed, Outcome, Round, ScoreReason, Scoring, Skull, SquadSpec, Wave, builtin,
};

/// One round of five one-actor waves, two ticks apart, with Famine on.
fn small_def(pool: u16, shared: bool) -> Def {
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
        rounds: vec![Round {
            waves,
            skulls_on: vec![Skull::Famine],
        }],
        lives: Lives {
            pool_start: pool,
            per_round_bonus: 0,
            shared,
        },
        caps: Caps {
            live_actors_by_players: vec![(4, 4)],
        },
        scoring: Scoring {
            by_class: [("elite".into(), 100)].into(),
            difficulty_multiplier: DifficultyMultipliers {
                easy: 1_000,
                normal: 1_000,
                heroic: 1_000,
                legendary: 1_500,
            },
            skull_multipliers: [(Skull::Famine, 1_100)].into(),
            assist: 250,
        },
    }
}

fn settings(seed: u64) -> firefight::Settings {
    firefight::Settings {
        seed,
        difficulty: Difficulty::Legendary,
        skulls: BTreeSet::new(),
    }
}

/// A fake game: applies commands at the start of the next tick, as the host
/// runtime does, and reports what it holds.
struct World {
    players: BTreeMap<u8, bool>,
    squads: [(u16, u32); SQUAD_SLOTS],
    classes: [String; SQUAD_SLOTS],
    zones: Vec<ZoneSeen>,
    kills: Vec<Kill>,
    pending: Vec<Command>,
    finished: Option<Outcome>,
    /// Live actors right after the last apply, before any kill.
    applied_live: u16,
}

impl World {
    fn new(players: u8, zones: Vec<ZoneSeen>) -> Self {
        Self {
            players: (0..players).map(|slot| (slot, true)).collect(),
            squads: [(0, 0); SQUAD_SLOTS],
            classes: Default::default(),
            zones,
            kills: Vec::new(),
            pending: Vec::new(),
            finished: None,
            applied_live: 0,
        }
    }

    fn apply(&mut self, tick: u32) {
        for command in std::mem::take(&mut self.pending) {
            match command {
                Command::PlaceSquad {
                    slot, class, count, ..
                } => {
                    let slot = usize::from(slot);
                    assert_eq!(self.squads[slot].0, 0, "placed into an occupied slot");
                    self.squads[slot] = (count, tick);
                    self.classes[slot] = class;
                }
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

    fn kill_squad(&mut self, slot: usize, killer: Option<u8>) {
        for _ in 0..self.squads[slot].0 {
            self.kill_one(slot, killer);
        }
    }

    fn kill_one(&mut self, slot: usize, killer: Option<u8>) {
        self.squads[slot].0 -= 1;
        self.kills.push(Kill {
            class: self.classes[slot].clone(),
            killer,
            assist: None,
        });
    }

    fn live(&self) -> u16 {
        self.squads.iter().map(|&(alive, _)| alive).sum()
    }

    fn observe(&mut self) -> Observed {
        Observed {
            players: self
                .players
                .iter()
                .map(|(&slot, &alive)| PlayerSeen { slot, alive })
                .collect(),
            actors_alive_by_squad: self.squads.map(|(alive, _)| alive),
            zones: self.zones.clone(),
            kills: std::mem::take(&mut self.kills),
        }
    }
}

/// Every squad dies to player 0 three ticks after it appears.
fn kill_at_age_three(tick: u32, world: &mut World) {
    for slot in 0..SQUAD_SLOTS {
        if world.squads[slot].0 > 0 && tick - world.squads[slot].1 == 3 {
            world.kill_squad(slot, Some(0));
        }
    }
}

fn two_zones() -> Vec<ZoneSeen> {
    vec![
        ZoneSeen {
            seen: false,
            distance: 10,
        },
        ZoneSeen {
            seen: true,
            distance: 50,
        },
    ]
}

struct Run {
    commands: Vec<(u32, Command)>,
    observed: Vec<Observed>,
    outcome: Option<Outcome>,
}

fn run(
    director: &mut Director,
    world: &mut World,
    limit: u32,
    mut script: impl FnMut(u32, &mut World),
    mut check: impl FnMut(u32, &Director, &World, &Observed, &[Command]),
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

fn place(slot: u8) -> Command {
    Command::PlaceSquad {
        slot,
        class: "elite".into(),
        count: 1,
        zone: 0,
        weapon_tier: 0,
    }
}

fn score(points: u32) -> Command {
    Command::Score {
        player: 0,
        points,
        reason: ScoreReason::Kill,
    }
}

#[test]
fn a_scripted_match_issues_the_golden_command_list() {
    let mut director = Director::new(small_def(1, true), settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(0, false);
            }
            kill_at_age_three(tick, world);
        },
        |_, _, _, _, _| {},
    );

    // 100 * 1.5 (Legendary) * 1.1 (Famine) = 165.
    let golden = vec![
        (
            0,
            Command::Lives {
                player: None,
                pool: 1,
            },
        ),
        (2, Command::Round { index: 0 }),
        (
            2,
            Command::Skull {
                id: Skull::Famine,
                on: true,
            },
        ),
        (2, Command::Wave { index: 0 }),
        (2, place(0)),
        (4, Command::Respawn { player: 0 }),
        (
            4,
            Command::Lives {
                player: None,
                pool: 0,
            },
        ),
        (6, score(165)),
        (8, Command::Wave { index: 1 }),
        (8, place(0)),
        (12, score(165)),
        (14, Command::Wave { index: 2 }),
        (14, place(0)),
        (18, score(165)),
        (20, Command::Wave { index: 3 }),
        (20, place(0)),
        (24, score(165)),
        (26, Command::Wave { index: 4 }),
        (26, place(0)),
        (30, score(165)),
        (
            30,
            Command::Finish {
                outcome: Outcome::Won,
            },
        ),
    ];
    assert_eq!(run.commands, golden);
    assert_eq!(run.outcome, Some(Outcome::Won));
}

/// A small test generator, separate from the director's.
struct TestRng(u64);

impl TestRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn one_in(&mut self, n: u64) -> bool {
        self.next() % n == 0
    }
}

/// The built-in sets with short gaps, so a match takes a few thousand ticks.
fn quick_builtins() -> [Def; 3] {
    builtin::definitions().map(|mut def| {
        for wave in def.rounds.iter_mut().flat_map(|round| &mut round.waves) {
            wave.gap_ticks = 10;
        }
        def
    })
}

/// Random kills, deaths and sight lines from a seed. Every fourth seed is
/// lethal (players die far faster than the lives pools refill), so some
/// matches are lost; the rest almost never drain the pool.
fn random_script(seed: u64) -> impl FnMut(u32, &mut World) {
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
        for zone in &mut world.zones {
            zone.seen = rng.one_in(2);
            zone.distance = (rng.next() % 100) as u32;
        }
    }
}

fn eight_zones() -> Vec<ZoneSeen> {
    vec![
        ZoneSeen {
            seen: false,
            distance: 0,
        };
        8
    ]
}

fn random_match(
    index: u64,
    check: impl FnMut(u32, &Director, &World, &Observed, &[Command]),
) -> (Def, Run) {
    let def = quick_builtins()[(index % 3) as usize].clone();
    let players = (index % 16) as u8 + 1;
    let mut director = Director::new(def.clone(), settings(index)).unwrap();
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

#[test]
fn live_actors_never_exceed_the_cap() {
    let mut outcomes = BTreeSet::new();
    for index in 0..1_000 {
        let (def, run) = random_match(index, |tick, director, world, _, _| {
            let cap = director.cap(world.players.len() as u8);
            assert!(world.applied_live <= cap, "match {index} tick {tick}");
            assert!(director.live_actors() <= cap, "match {index} tick {tick}");
        });
        let outcome = run
            .outcome
            .unwrap_or_else(|| panic!("match {index} never ended"));
        outcomes.insert(format!("{outcome:?}"));

        // The reserve: each wave places its own squads, in order, once.
        let (mut round, mut wave, mut placed) = (0, 0, 0);
        for (_, command) in &run.commands {
            match command {
                Command::Round { index } => round = usize::from(*index),
                Command::Wave { index } => (wave, placed) = (usize::from(*index), 0),
                Command::PlaceSquad { class, count, .. } => {
                    let squads = &def.rounds[round].waves[wave].squads;
                    let squad = squads.get(placed).expect("placed beyond the reserve");
                    assert_eq!(*class, squad.class);
                    assert!((squad.count.min..=squad.count.max).contains(count));
                    placed += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(outcomes.len(), 2, "both outcomes occur: {outcomes:?}");
}

#[test]
fn no_zone_in_sight_is_chosen_while_another_exists() {
    let mut placed = 0;
    for index in 0..200 {
        random_match(index, |_, _, _, seen, out| {
            for command in out {
                if let Command::PlaceSquad { zone, .. } = command {
                    placed += 1;
                    let zone = &seen.zones[usize::from(*zone)];
                    if seen.zones.iter().any(|zone| !zone.seen) {
                        assert!(!zone.seen);
                    }
                }
            }
        });
    }
    assert!(placed > 1_000);
}

#[test]
fn zone_choice_follows_the_definition_and_the_seed() {
    let mut def = small_def(1, true);
    def.rounds[0].waves[0].squads[0].drop = Drop::PreferZone(2);
    def.rounds[0].waves[1].squads[0].drop = Drop::PreferZone(1);
    def.rounds[0].waves[2].squads[0].drop = Drop::PreferZone(9);
    let zones = || {
        // Zone 1 is in sight; zones 3 and 4 are the farthest unseen.
        [
            (false, 5),
            (true, 90),
            (false, 1),
            (false, 70),
            (false, 60),
            (false, 2),
        ]
        .map(|(seen, distance)| ZoneSeen { seen, distance })
        .to_vec()
    };
    let picks = |seed| {
        let mut director = Director::new(def.clone(), settings(seed)).unwrap();
        let mut world = World::new(1, zones());
        run(
            &mut director,
            &mut world,
            100,
            kill_at_age_three,
            |_, _, _, _, _| {},
        )
        .commands
        .into_iter()
        .filter_map(|(_, command)| match command {
            Command::PlaceSquad { zone, .. } => Some(zone),
            _ => None,
        })
        .collect::<Vec<_>>()
    };

    let first = picks(3);
    assert_eq!(first.len(), 5);
    // An unseen preferred zone is honoured.
    assert_eq!(first[0], 2);
    // A preferred zone in sight, or one the kit lacks, falls back to the
    // farther half of the unseen zones: 3, 4 and 0.
    for &zone in &first[1..] {
        assert!([3, 4, 0].contains(&zone), "{first:?}");
    }
    assert_eq!(picks(3), first);
}

#[test]
fn same_seed_same_commands() {
    for index in [1, 17, 302] {
        let (_, a) = random_match(index, |_, _, _, _, _| {});
        let (_, b) = random_match(index, |_, _, _, _, _| {});
        assert_eq!(a.commands, b.commands);
        assert!(a.outcome.is_some());
    }
}

#[test]
fn a_replay_of_the_observed_sequence_issues_the_same_commands() {
    let (def, original) = random_match(5, |_, _, _, _, _| {});
    let mut director = Director::new(def, settings(5)).unwrap();
    let mut replayed = Vec::new();
    for (tick, seen) in original.observed.iter().enumerate() {
        let tick = tick as u32;
        replayed.extend(
            director
                .step(tick, seen)
                .into_iter()
                .map(|command| (tick, command)),
        );
    }
    assert_eq!(replayed, original.commands);
}

#[test]
fn a_resumed_mode_blob_issues_the_same_commands() {
    let (def, original) = random_match(9, |_, _, _, _, _| {});
    let split = original.observed.len() / 2;
    let mut director = Director::new(def.clone(), settings(9)).unwrap();
    let mut commands = Vec::new();
    for (tick, seen) in original.observed[..split].iter().enumerate() {
        let tick = tick as u32;
        commands.extend(director.step(tick, seen).into_iter().map(|c| (tick, c)));
    }
    let blob = serde_json::to_string(director.state()).unwrap();
    let state: State = serde_json::from_str(&blob).unwrap();
    let mut resumed = Director::resume(def, Difficulty::Legendary, state).unwrap();
    for (tick, seen) in original.observed.iter().enumerate().skip(split) {
        let tick = tick as u32;
        commands.extend(resumed.step(tick, seen).into_iter().map(|c| (tick, c)));
    }
    assert_eq!(commands, original.commands);
}

#[test]
fn the_pool_at_zero_holds_respawns_until_the_wave_ends() {
    let mut director = Director::new(small_def(0, true), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(1, false);
            }
            kill_at_age_three(tick, world);
        },
        |_, _, _, _, _| {},
    );
    let respawns: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, command)| matches!(command, Command::Respawn { .. }))
        .collect();
    // Player 1 dies on tick 4; the first wave's actor dies on tick 6.
    assert_eq!(respawns, [&(6, Command::Respawn { player: 1 })]);
    assert_eq!(run.outcome, Some(Outcome::Won));
}

#[test]
fn everyone_dead_and_no_lives_finishes_lost() {
    let mut director = Director::new(small_def(0, true), settings(1)).unwrap();
    let mut world = World::new(1, two_zones());
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| {
            if tick == 4 {
                world.players.insert(0, false);
            }
        },
        |_, _, _, _, _| {},
    );
    assert_eq!(
        run.commands.last(),
        Some(&(
            4,
            Command::Finish {
                outcome: Outcome::Lost
            }
        ))
    );
    assert_eq!(run.outcome, Some(Outcome::Lost));
    // A finished director issues nothing more.
    assert!(director.step(5, &Observed::default()).is_empty());
}

#[test]
fn unshared_lives_are_counted_per_player() {
    let mut director = Director::new(small_def(1, false), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let run = run(
        &mut director,
        &mut world,
        5,
        |tick, world| {
            if tick == 3 {
                world.players.insert(1, false);
            }
        },
        |_, _, _, _, _| {},
    );
    let lives: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, command)| matches!(command, Command::Lives { .. } | Command::Respawn { .. }))
        .cloned()
        .collect();
    assert_eq!(
        lives,
        [
            (
                0,
                Command::Lives {
                    player: Some(0),
                    pool: 1
                }
            ),
            (
                0,
                Command::Lives {
                    player: Some(1),
                    pool: 1
                }
            ),
            (3, Command::Respawn { player: 1 }),
            (
                3,
                Command::Lives {
                    player: Some(1),
                    pool: 0
                }
            ),
        ]
    );
}

#[test]
fn kills_score_by_the_definition_with_assists() {
    let mut director = Director::new(small_def(1, true), settings(1)).unwrap();
    let mut world = World::new(2, two_zones());
    let mut seen = world.observe();
    seen.kills = vec![
        Kill {
            class: "elite".into(),
            killer: Some(1),
            assist: Some(0),
        },
        // The world's kills and unknown classes score nothing.
        Kill {
            class: "elite".into(),
            killer: None,
            assist: Some(0),
        },
        Kill {
            class: "grunt".into(),
            killer: Some(1),
            assist: None,
        },
    ];
    let out = director.step(0, &seen);
    // No skull is on before the first round: 100 * 1.5 = 150; assist 25%.
    assert_eq!(
        out[1..],
        [
            Command::Score {
                player: 1,
                points: 150,
                reason: ScoreReason::Kill
            },
            Command::Score {
                player: 0,
                points: 37,
                reason: ScoreReason::Assist
            },
        ]
    );
}

#[test]
fn commands_have_canonical_integer_json() {
    let command = Command::PlaceSquad {
        slot: 3,
        class: "elite".into(),
        count: 4,
        zone: 7,
        weapon_tier: 1,
    };
    let value = serde_json::to_value(&command).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"place_squad": {"slot": 3, "class": "elite", "count": 4, "zone": 7, "weapon_tier": 1}})
    );
    let back: Command = serde_json::from_value(value).unwrap();
    assert_eq!(back, command);
}
